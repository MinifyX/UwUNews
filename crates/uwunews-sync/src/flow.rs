//! The three things a person actually does: connect a server, add a device,
//! join from one.
//!
//! Everything below this is a piece — a vault, a transport, a handshake, a
//! store. This is where they are put in the right order, once, so that the
//! interface above has three calls and no chance to get the order wrong. And
//! the order does matter: the account key is generated before the vault is
//! wrapped, the vault header goes up before the first record, and a device
//! remembers its pairing only after the server has accepted it.
//!
//! The device key and the account key go into the store's secret store, and
//! only there: this crate never keeps a secret of its own.
//!
//! An account made here uses UwUSSH's labels ([`Flavor::Ssh`]). Joining takes
//! the flavor of the app that made the account, so a device can join one that
//! UwUSSH or UwURDP made.

use crate::engine::TransportError;
use crate::http::{self, Server};
use crate::pairing::{self, Handover, Joined, Offer, Target};
use std::time::Instant;
use uuid::Uuid;
use uwunews_store::{Enrolment, Store, StoreError};
use uwunews_vault::{AccountKey, Flavor, KdfParams, VaultError};

#[derive(Debug, thiserror::Error)]
pub enum FlowError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Vault(#[from] VaultError),
    #[error("that setup code is not one")]
    BadSetupCode,
    #[error("the vault has to be unlocked first")]
    VaultLocked,
    #[error("this device is not paired with a server")]
    NotPaired,
    #[error("the other device did not send an account key")]
    NoAccountKey,
}

type Result<T> = std::result::Result<T, FlowError>;

/// A setup code, as the server prints it: where it is, what to pin, and an
/// invite good for one account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setup {
    pub server_url: String,
    pub tls_fingerprint: Option<String>,
    pub invite: String,
}

/// Read a `uwu1_…` setup code.
pub fn parse_setup(text: &str) -> Option<Setup> {
    let body = text.trim().strip_prefix("uwu1_")?;
    let json: serde_json::Value = serde_json::from_slice(&http::decode_base64(body)?).ok()?;
    let setup = Setup {
        server_url: json["u"].as_str()?.to_string(),
        tls_fingerprint: json["f"].as_str().map(str::to_string),
        invite: json["i"].as_str().unwrap_or_default().to_string(),
    };
    (!setup.server_url.is_empty()).then_some(setup)
}

/// What a device has once it is in.
pub struct Paired {
    pub account_id: Uuid,
    pub device_id: Uuid,
    /// The account key, on the one device that made it — to be shown once, as
    /// the recovery kit, and then never again.
    pub recovery: Option<AccountKey>,
}

impl std::fmt::Debug for Paired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Paired({}, device {}, recovery {})",
            self.account_id,
            self.device_id,
            if self.recovery.is_some() {
                "to show"
            } else {
                "none"
            }
        )
    }
}

/// Turn a setup code into an account, with this device as its first.
///
/// The account key is made here and goes into the vault's derivation, so from
/// this moment the vault needs both the password and the key — which is why
/// the kit comes back to be shown before anything else happens.
///
/// The vault is a new one, made for the account, and every source, collection
/// and mark this device has goes along into it: rows are plaintext here and
/// only sealed on the way out. Nothing is kept until the server said yes.
pub fn create_account(
    store: &Store,
    setup: &Setup,
    password: &[u8],
    device_name: &str,
) -> Result<(Server, Paired)> {
    let account_key = AccountKey::generate();
    let (header, vault) = uwunews_vault::create_with(
        password,
        Some(&account_key),
        Uuid::now_v7(),
        KdfParams::RECOMMENDED,
        Flavor::Ssh,
    )?;
    let login_key = header.login_key(password, Some(&account_key))?;

    let (seed, public_key) = device_key();
    let server = Server::connect(&setup.server_url, setup.tls_fingerprint.as_deref())?;
    let admitted = server.create_account(&uwunews_proto::api::CreateAccount {
        invite: setup.invite.clone(),
        vault: http::to_wire(&header),
        auth_key: http::encode_base64(*login_key),
        device: uwunews_proto::api::NewDevice {
            name: device_name.to_string(),
            public_key,
        },
    })?;

    // The account key first: a vault header that needs it is never kept
    // without it.
    remember(
        store,
        setup.server_url.clone(),
        setup.tls_fingerprint.clone(),
        &admitted,
        &seed,
        Some(&account_key),
    )?;
    store.adopt_vault(
        header.vault_id,
        vault.export_key(),
        Flavor::Ssh,
        Some(&header),
    )?;
    Ok((
        server.as_device(admitted.account_id, admitted.device_id, &seed),
        Paired {
            account_id: admitted.account_id,
            device_id: admitted.device_id,
            recovery: Some(account_key),
        },
    ))
}

/// A code to show, and the session it belongs to.
pub struct PairingOffer {
    pub offer: Offer,
    /// A token for the joining device, handed over inside the handshake and
    /// never shown to anyone.
    enrolment: String,
    handover_url: String,
    handover_fingerprint: Option<String>,
    account_id: Uuid,
    /// Whose labels the account speaks — and so the handshake.
    flavor: Flavor,
}

/// Open a pairing session and make the code to read out.
///
/// Two steps rather than one, because the interface has to show the code
/// before anything waits: this returns, the code goes on screen, and
/// [`wait_for_device`] does the waiting.
pub fn offer_pairing(store: &Store, server: &Server) -> Result<PairingOffer> {
    let state = store.sync_state()?;
    let (Some(url), Some(account_id)) = (state.server_url.clone(), state.account_id) else {
        return Err(FlowError::NotPaired);
    };
    let flavor = match store.vault_header()? {
        Some(header) => header.flavor,
        None => store.vault_flavor().ok_or(FlowError::VaultLocked)?,
    };
    let session = server.open_pairing()?;
    let enrolment = server.enrolment_token()?;
    let words = pairing::words();
    Ok(PairingOffer {
        offer: pairing::offer(&session.id, &words, &url, state.tls_fingerprint.as_deref()),
        enrolment: enrolment.token,
        handover_url: url,
        handover_fingerprint: state.tls_fingerprint,
        account_id,
        flavor,
    })
}

/// Wait for the other device to answer, and hand it what it needs.
///
/// The account key comes out of what this device kept from its own pairing,
/// in the secret store.
pub fn wait_for_device(
    store: &Store,
    server: &Server,
    offer: &PairingOffer,
    deadline: Instant,
) -> Result<Joined> {
    let keys = store.enrolment_keys()?.ok_or(FlowError::NotPaired)?;
    let handover = Handover {
        server_url: offer.handover_url.clone(),
        tls_fingerprint: offer.handover_fingerprint.clone(),
        account_id: offer.account_id,
        account_key: keys
            .account_key
            .as_ref()
            .map(|key| http::encode_base64(key.as_bytes())),
        enrolment: offer.enrolment.clone(),
        manifest: store.published_manifest()?,
    };

    let postbox = server.postbox(&offer.offer.id, "a");
    let joined = pairing::hand_over(
        &postbox,
        offer.flavor,
        &offer.offer.id,
        &offer.offer.words,
        &handover,
        deadline,
    );
    // Done either way — a wrong guess and a timeout included: a session left
    // open is a code that still works. A retry makes new words.
    let _ = server.close_pairing(&offer.offer.id);
    Ok(joined?)
}

/// Join an account from a code the other device showed.
///
/// Everything this device needs arrives through the handshake — the address,
/// the fingerprint to pin, the account key, a one-time token — so the only
/// thing asked of the person is the master password — and which app made the
/// account, since the handshake already speaks its labels.
pub fn join(
    store: &Store,
    target: &Target,
    password: &[u8],
    device_name: &str,
    flavor: Flavor,
    deadline: Instant,
) -> Result<(Server, Paired)> {
    // The relay first, with what the code said.
    let relay = Server::connect(&target.server_url, target.tls_fingerprint.as_deref())?;
    let postbox = relay.postbox(&target.id, "b");
    let (handover, session_key) =
        pairing::take_over(&postbox, flavor, &target.id, &target.words, deadline)?;

    // From here on, what the other device said counts: it came sealed under a
    // key only the two of them have.
    let account_key = handover.account_key();
    let server = Server::connect(&handover.server_url, handover.tls_fingerprint.as_deref())?;

    // The salt and costs, so the password can become the login key.
    let params = server.vault_params(&handover.enrolment)?;
    if params.needs_account_key && account_key.is_none() {
        return Err(FlowError::NoAccountKey);
    }
    let login_key = login_key_from(&params, password, account_key.as_ref(), flavor)?;

    let (seed, public_key) = device_key();
    let admitted = server.enrol(&uwunews_proto::api::EnrolDevice {
        enrolment: handover.enrolment.clone(),
        auth_key: http::encode_base64(*login_key),
        device: uwunews_proto::api::NewDevice {
            name: device_name.to_string(),
            public_key,
        },
    })?;
    let server = server.as_device(admitted.account_id, admitted.device_id, &seed);

    // Now the wrapped key may be had, and opened with what the person typed
    // and what the other device sent.
    let header = http::from_wire(&server.vault()?, flavor)?;
    let key = header
        .unlock_with(password, account_key.as_ref())?
        .export_key();

    remember(
        store,
        handover.server_url.clone(),
        handover.tls_fingerprint.clone(),
        &admitted,
        &seed,
        account_key.as_ref(),
    )?;
    store.adopt_vault(header.vault_id, key, flavor, Some(&header))?;
    // What the other device had published: until that manifest is here, this
    // one cannot tell the whole vault from whatever the server chose to send.
    store.set_manifest_floor(handover.manifest)?;

    // And tell the other device what to call this one.
    let _ = pairing::say_joined(
        &postbox,
        flavor,
        &session_key,
        &Joined {
            device_name: device_name.to_string(),
            device_id: admitted.device_id,
        },
    );

    Ok((
        server,
        Paired {
            account_id: admitted.account_id,
            device_id: admitted.device_id,
            recovery: None,
        },
    ))
}

/// A server this device is already paired with, ready to sync: the address and
/// fingerprint from the store, the identity from the secret store.
pub fn reconnect(store: &Store) -> Result<Server> {
    let state = store.sync_state()?;
    let (Some(url), Some(account_id), Some(device_id)) =
        (state.server_url, state.account_id, state.device_id)
    else {
        return Err(FlowError::NotPaired);
    };
    let keys = store.enrolment_keys()?.ok_or(FlowError::NotPaired)?;
    let server = Server::connect(&url, state.tls_fingerprint.as_deref())?.as_device(
        account_id,
        device_id,
        &keys.device_key,
    );
    server.sign_in()?;
    Ok(server)
}

/// The account key this device kept, for unlocking the vault without asking
/// for the kit.
pub fn account_key(store: &Store) -> Result<Option<AccountKey>> {
    Ok(store.enrolment_keys()?.and_then(|keys| keys.account_key))
}

/// A device's signing key: the seed to keep, the public half to enrol.
fn device_key() -> ([u8; 32], String) {
    use ed25519_dalek::SigningKey;
    use rand::RngCore;
    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let public = SigningKey::from_bytes(&seed).verifying_key().to_bytes();
    (seed, http::encode_base64(public))
}

/// The login key, from what a joining device downloaded rather than from a
/// header it does not have yet.
fn login_key_from(
    params: &uwunews_proto::api::WireVaultParams,
    password: &[u8],
    account_key: Option<&AccountKey>,
    flavor: Flavor,
) -> Result<zeroize::Zeroizing<[u8; 32]>> {
    let salt = http::decode_base64(&params.salt)
        .ok_or_else(|| TransportError::Refused("the vault's salt".into()))?;
    let kdf = KdfParams {
        memory_kib: params.kdf_memory_kib,
        time_cost: params.kdf_time_cost,
        parallelism: params.kdf_parallelism,
    };
    if !kdf.within_limits() {
        return Err(TransportError::Refused(
            "that vault asks for more work than any device can do".into(),
        )
        .into());
    }
    // With an account key mixed in, the login key is out of reach of a guess
    // whatever the costs. Without one, the password alone guards it — and the
    // costs came from the server, which would like them to be cheap.
    if account_key.is_none() && !kdf.strong_enough() {
        return Err(TransportError::Refused(
            "that vault's password protection is too weak to join without an account key".into(),
        )
        .into());
    }
    let secrets =
        uwunews_vault::derive_master_secrets_for(password, &salt, kdf, account_key, flavor)?;
    Ok(uwunews_vault::server_auth_key(&secrets)?)
}

/// Write down what a pairing agreed on, the secrets in the secret store.
fn remember(
    store: &Store,
    server_url: String,
    tls_fingerprint: Option<String>,
    admitted: &uwunews_proto::api::Admitted,
    device_key: &[u8; 32],
    account_key: Option<&AccountKey>,
) -> Result<()> {
    store.save_enrolment(
        &Enrolment {
            server_url,
            account_id: admitted.account_id,
            device_id: admitted.device_id,
            tls_fingerprint,
        },
        device_key,
        account_key,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setup_code_reads_back_as_what_the_server_printed() {
        // Exactly what `uwusync-server invite` writes.
        let body = serde_json::json!({
            "u": "https://nas.test:8443",
            "f": "SHA256:abc",
            "i": "K7M4Q-9PQ2R-T5XYZ"
        })
        .to_string();
        let code = format!("uwu1_{}", http::encode_base64(&body));

        let setup = parse_setup(&code).unwrap();
        assert_eq!(setup.server_url, "https://nas.test:8443");
        assert_eq!(setup.tls_fingerprint.as_deref(), Some("SHA256:abc"));
        assert_eq!(setup.invite, "K7M4Q-9PQ2R-T5XYZ");

        // A server behind a real certificate prints no fingerprint.
        let body = serde_json::json!({ "u": "https://news.example.com", "i": "X" }).to_string();
        let setup = parse_setup(&format!("uwu1_{}", http::encode_base64(&body))).unwrap();
        assert_eq!(setup.tls_fingerprint, None);

        for nonsense in ["", "uwu1_", "uwu1_notbase64!!", "hello", "uwu2_abc"] {
            assert!(parse_setup(nonsense).is_none(), "{nonsense}");
        }
    }

    #[test]
    fn a_device_that_is_not_paired_says_so_instead_of_reaching_for_a_server() {
        let store =
            Store::open_in_memory(std::sync::Arc::new(uwunews_store::MemorySecrets::default()))
                .unwrap();

        assert!(matches!(reconnect(&store), Err(FlowError::NotPaired)));
        assert!(matches!(account_key(&store), Ok(None)));
    }

    #[test]
    fn what_is_shown_once_is_named_as_such() {
        let paired = Paired {
            account_id: Uuid::now_v7(),
            device_id: Uuid::now_v7(),
            recovery: Some(AccountKey::generate()),
        };
        assert!(format!("{paired:?}").contains("recovery to show"));
        let joined = Paired {
            recovery: None,
            ..paired
        };
        assert!(format!("{joined:?}").contains("recovery none"));
    }

    #[test]
    fn cheap_costs_from_the_server_are_refused_unless_an_account_key_guards_the_login() {
        let params = |kdf: KdfParams| uwunews_proto::api::WireVaultParams {
            vault_id: uuid::Uuid::nil(),
            kdf_memory_kib: kdf.memory_kib,
            kdf_time_cost: kdf.time_cost,
            kdf_parallelism: kdf.parallelism,
            salt: http::encode_base64([7u8; 16]),
            needs_account_key: false,
        };
        let cheap = params(KdfParams::INSECURE_FOR_TESTS);
        let ssh = Flavor::Ssh;
        assert!(login_key_from(&cheap, b"pw", None, ssh).is_err());
        let key = AccountKey::generate();
        let as_ssh = login_key_from(&cheap, b"pw", Some(&key), ssh).unwrap();
        let as_rdp = login_key_from(&cheap, b"pw", Some(&key), Flavor::Rdp).unwrap();
        assert_ne!(
            *as_ssh, *as_rdp,
            "each app's accounts log in with their own key"
        );
        assert!(login_key_from(&params(KdfParams::FLOOR), b"pw", None, ssh).is_ok());
    }
}
