//! Adding a device without typing a password into it.
//!
//! What a joining device needs is the account key, and the account key is the
//! one thing that must never go through the server. So the two devices agree
//! on a key of their own first — **SPAKE2**, where a short code spoken out loud
//! turns into a strong shared secret — and everything after that travels
//! sealed under it, through a server that carries it without understanding it.
//!
//! Why three words are enough where a three-word password would not be: SPAKE2
//! gives whoever does not know the code exactly **one** online guess. A wrong
//! guess leaves the two sides with different keys and nothing to show for it,
//! and the session is over.
//!
//! The order of the handshake matters, and it is not the obvious one:
//!
//! 1. Each side sends its SPAKE2 message.
//! 2. **The joining device proves it derived the same key** — before anything
//!    worth having is sent.
//! 3. Only then does the device that is already in hand over the account key,
//!    the fingerprint to pin, a one-time enrolment token and the manifest the
//!    joining device must see before it can trust that it has everything.
//! 4. The joining device says who it is, so the other can show a name rather
//!    than "a device".
//!
//! Sending the secret first and asking questions later would hand it to the
//! one guess an attacker gets. This way that guess buys nothing.

use crate::engine::TransportError;
use crate::http::Server;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use uuid::Uuid;
use uwunews_store::ManifestFloor;
use uwunews_vault::{AccountKey, Flavor, Sealed};
use zeroize::Zeroizing;

/// Words short enough to read out over the phone and distinct enough to hear
/// correctly. Exactly 128 of them, so one random byte picks one without
/// favouring any, and three words carry 21 bits — which, with the single guess
/// SPAKE2 allows, is more than enough.
const WORDS: [&str; 128] = [
    "anker", "apfel", "arena", "aroma", "atlas", "auto", "bambus", "banane", "basalt", "beton",
    "bingo", "bison", "bonus", "bravo", "cello", "chili", "curry", "delta", "diesel", "disco",
    "domino", "drama", "echo", "eule", "fagott", "fauna", "flora", "forum", "galerie", "gamma",
    "gecko", "gitarre", "gnu", "gong", "gorilla", "granit", "guru", "hafen", "harfe", "helium",
    "hobby", "hotel", "humor", "hyäne", "idol", "iglu", "index", "insel", "jaguar", "jasmin",
    "jodel", "judo", "jumbo", "kajak", "kaktus", "kamera", "kanu", "karat", "karotte", "kiosk",
    "kiwi", "koala", "kobra", "kokos", "kompass", "konto", "korb", "krater", "kredit", "lama",
    "lampe", "lava", "lemur", "limbo", "lotus", "lupe", "magma", "magnet", "mango", "marathon",
    "melone", "memo", "meteor", "mikro", "mimose", "moped", "mosaik", "motor", "motto", "mumie",
    "museum", "nektar", "neon", "ninja", "nomade", "nougat", "nova", "oase", "obelisk", "ozean",
    "panda", "papaya", "paprika", "pilot", "pinguin", "pizza", "planet", "plasma", "podium",
    "pony", "quarz", "radio", "rakete", "rubin", "salami", "salat", "signal", "sofa", "sushi",
    "taxi", "tiger", "tofu", "tomate", "tunnel", "turbo", "vanille", "zebra", "zitrone",
];

/// How many words a code has.
const WORD_COUNT: usize = 3;

/// The labels that keep a pairing blob from being openable as anything else —
/// one per direction, so a message can't be put back to the side that sent it
/// and read there as the other side's answer.
///
/// All of them carry the account's flavor: a pairing speaks the labels of the
/// app that made the account, so UwUSSH and UwURDP devices can add this one.
struct Labels {
    a_to_b: Vec<u8>,
    b_to_a: Vec<u8>,
    /// What the joining device sends to prove it derived the same key.
    confirm: Vec<u8>,
    side_a: Vec<u8>,
    side_b: Vec<u8>,
}

impl Labels {
    fn of(flavor: Flavor) -> Self {
        Self {
            a_to_b: flavor.label("pairing/v2/a-to-b"),
            b_to_a: flavor.label("pairing/v2/b-to-a"),
            confirm: flavor.label("pairing/confirm"),
            side_a: flavor.label("pair/a"),
            side_b: flavor.label("pair/b"),
        }
    }
}

/// How long to keep asking the relay before giving up. The server forgets a
/// session after ten minutes, so there is nothing to wait for after that.
pub const DEADLINE: Duration = Duration::from_secs(10 * 60);

/// A post box at the relay: one side of one pairing session.
///
/// A trait rather than the concrete client, so the handshake can be run
/// against two ends of a channel in a test and against the real server
/// everywhere else.
pub trait Postbox {
    /// Leave a message for the other side.
    fn put(&self, message: &[u8]) -> Result<(), TransportError>;
    /// What the other side has said, from `after` onwards. May wait.
    fn take(&self, after: usize, wait: bool) -> Result<Vec<Vec<u8>>, TransportError>;
}

/// One side of a session at a real server.
pub struct ServerPostbox<'a> {
    server: &'a Server,
    id: String,
    side: &'static str,
    /// For side `b`: the secret this device holds its side with. Side `a`
    /// signs in instead, and needs none.
    claim: Option<String>,
}

impl Server {
    /// The post box for a pairing session. `side` is `"a"` for the device that
    /// opened it and `"b"` for the one joining.
    pub fn postbox(&self, id: &str, side: &'static str) -> ServerPostbox<'_> {
        use rand::RngCore;
        let claim = (side == "b").then(|| {
            let mut secret = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut secret);
            crate::http::encode_base64(secret)
        });
        ServerPostbox {
            server: self,
            id: id.to_string(),
            side,
            claim,
        }
    }
}

impl Postbox for ServerPostbox<'_> {
    fn put(&self, message: &[u8]) -> Result<(), TransportError> {
        self.server
            .pair_send(&self.id, self.side, self.claim.as_deref(), message)
    }

    fn take(&self, after: usize, wait: bool) -> Result<Vec<Vec<u8>>, TransportError> {
        self.server
            .pair_receive(&self.id, self.side, self.claim.as_deref(), after, wait)
    }
}

/// What the device that is already in hands over. Everything here is a secret
/// or a decision the joining device cannot make for itself.
#[derive(Clone, Serialize, Deserialize)]
pub struct Handover {
    /// Where the server is, as this device reaches it.
    pub server_url: String,
    /// The fingerprint to pin, when the server brought its own certificate.
    pub tls_fingerprint: Option<String>,
    pub account_id: Uuid,
    /// 128 bits, base64 — the part that must never reach the server.
    pub account_key: Option<String>,
    /// A one-time token for joining the account.
    pub enrolment: String,
    /// This device's manifest as the server last confirmed it. The joining
    /// device does not trust itself to have everything until it holds that
    /// manifest, at least that new — so a server that hands a new device
    /// nothing but old versions cannot pass that off as the whole vault. It
    /// travels in here because the server cannot change what is in here.
    /// Missing from builds before manifests, and while this device has
    /// published none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<ManifestFloor>,
}

impl std::fmt::Debug for Handover {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The account key is in here. Never print it, not even by accident.
        f.write_str("Handover(redacted)")
    }
}

impl Handover {
    pub fn account_key(&self) -> Option<AccountKey> {
        let bytes = crate::http::decode_base64(self.account_key.as_deref()?)?;
        Some(AccountKey::from_bytes(bytes.try_into().ok()?))
    }
}

/// What the joining device says back, once it is in.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Joined {
    pub device_name: String,
    pub device_id: Uuid,
}

/// A code, as it is shown and as it is pasted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// The session the server opened.
    pub id: String,
    /// Three words, the part that is actually secret.
    pub words: String,
    /// What to read out: the id and the words.
    pub spoken: String,
    /// One string to paste, for when the two devices can copy and paste. It
    /// carries the address and the fingerprint as well, so nothing else has to
    /// be typed.
    pub pasteable: String,
}

/// What a joining device needs to find the other one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub server_url: String,
    pub tls_fingerprint: Option<String>,
    pub id: String,
    pub words: String,
}

/// Three words from the list, from the operating system's randomness.
pub fn words() -> String {
    let mut chosen = Vec::with_capacity(WORD_COUNT);
    for byte in random_words(WORD_COUNT) {
        chosen.push(WORDS[byte as usize % WORDS.len()]);
    }
    chosen.join("-")
}

fn random_words(count: usize) -> Vec<u8> {
    use rand::RngCore;
    let mut bytes = vec![0u8; count];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes
}

/// Everything the other device has to be told, in one pasteable string.
pub fn offer(id: &str, words: &str, server_url: &str, fingerprint: Option<&str>) -> Offer {
    let mut body = serde_json::json!({ "u": server_url, "i": id, "w": words });
    if let Some(fingerprint) = fingerprint {
        body["f"] = serde_json::json!(fingerprint);
    }
    Offer {
        id: id.to_string(),
        words: words.to_string(),
        spoken: format!("{id}-{words}"),
        pasteable: format!("uwu2_{}", crate::http::encode_base64(body.to_string())),
    }
}

/// The pasteable form, read back.
pub fn parse_offer(text: &str) -> Option<Target> {
    let body = text.trim().strip_prefix("uwu2_")?;
    let json: serde_json::Value =
        serde_json::from_slice(&crate::http::decode_base64(body)?).ok()?;
    let target = Target {
        server_url: json["u"].as_str()?.to_string(),
        tls_fingerprint: json["f"].as_str().map(str::to_string),
        id: json["i"].as_str()?.to_string(),
        words: json["w"].as_str()?.to_string(),
    };
    (!target.id.is_empty() && !target.words.is_empty() && !target.server_url.is_empty())
        .then_some(target)
}

/// A code as it was read out: `K7M4Q-tiger-radio-kiwi`.
pub fn parse_spoken(text: &str) -> Option<(String, String)> {
    let cleaned = text.trim().to_lowercase().replace([' ', '_'], "-");
    let mut parts = cleaned.split('-').filter(|part| !part.is_empty());
    let id = parts.next()?.to_uppercase();
    let words: Vec<String> = parts.map(str::to_string).collect();
    if id.is_empty() || words.len() != WORD_COUNT {
        return None;
    }
    // Every word has to be one of ours, or the code was misheard rather than
    // mistyped — and the difference is worth saying.
    if !words.iter().all(|word| WORDS.contains(&word.as_str())) {
        return None;
    }
    Some((id, words.join("-")))
}

/// The SPAKE2 password: the words, bound to the session they belong to, so a
/// message from another session cannot be replayed into this one.
fn password(id: &str, words: &str) -> Vec<u8> {
    format!(
        "{}|{}",
        id.trim().to_uppercase(),
        words.trim().to_lowercase()
    )
    .into_bytes()
}

fn seal(key: &[u8; 32], label: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, TransportError> {
    let sealed = uwunews_vault::encrypt_keyed(key, label, plaintext)
        .map_err(|_| TransportError::Refused("could not seal the handover".into()))?;
    Ok([sealed.nonce, sealed.blob].concat())
}

fn open(key: &[u8; 32], label: &[u8], message: &[u8]) -> Result<Vec<u8>, TransportError> {
    if message.len() < 24 {
        return Err(TransportError::Refused(
            "a message that is too short".into(),
        ));
    }
    let (nonce, blob) = message.split_at(24);
    uwunews_vault::decrypt_keyed(
        key,
        label,
        &Sealed {
            nonce: nonce.to_vec(),
            blob: blob.to_vec(),
        },
    )
    .map_err(|_| TransportError::Refused("that code does not match".into()))
}

/// Wait for the other side to say something new.
///
/// A real relay holds the request open for twenty seconds, so this loop asks
/// only a handful of times. A post box that answers at once instead — a test
/// double — would otherwise turn this into a spin, hence the pause.
fn await_message(
    postbox: &impl Postbox,
    after: usize,
    deadline: Instant,
) -> Result<Vec<u8>, TransportError> {
    loop {
        // Asking the relay to hold the request open is what keeps a handshake
        // down to a handful of requests — but not when there is less time
        // left than it would hold for, or giving up would take longer than
        // waiting was supposed to.
        let remaining = deadline.saturating_duration_since(Instant::now());
        let messages = postbox.take(after, remaining > RELAY_HOLD)?;
        if let Some(message) = messages.into_iter().next() {
            return Ok(message);
        }
        if Instant::now() >= deadline {
            return Err(TransportError::Unreachable(
                "the other device never answered".into(),
            ));
        }
        std::thread::sleep(POLL_PAUSE.min(deadline.saturating_duration_since(Instant::now())));
    }
}

/// How long to pause between asking, when asking did not block.
const POLL_PAUSE: Duration = Duration::from_millis(200);

/// How long the relay holds a request open before answering with nothing.
const RELAY_HOLD: Duration = Duration::from_secs(20);

/// The side that is already in: hand the secrets over, once the other side has
/// proved it knows the code.
pub fn hand_over(
    postbox: &impl Postbox,
    flavor: Flavor,
    id: &str,
    words: &str,
    handover: &Handover,
    deadline: Instant,
) -> Result<Joined, TransportError> {
    use spake2::{Ed25519Group, Identity, Password, Spake2};
    let labels = Labels::of(flavor);

    let (state, ours) = Spake2::<Ed25519Group>::start_a(
        &Password::new(password(id, words)),
        &Identity::new(&labels.side_a),
        &Identity::new(&labels.side_b),
    );
    postbox.put(&ours)?;

    let theirs = await_message(postbox, 0, deadline)?;
    let key: [u8; 32] = state
        .finish(&theirs)
        .map_err(|_| TransportError::Refused("that code does not match".into()))?
        .try_into()
        .map_err(|_| TransportError::Refused("the handshake went wrong".into()))?;
    let key = Zeroizing::new(key);

    // Nothing worth having goes out until the other side has shown it derived
    // the same key. Otherwise the one guess SPAKE2 allows would be enough.
    let confirmation = await_message(postbox, 1, deadline)?;
    if open(&key, &labels.b_to_a, &confirmation)? != labels.confirm {
        return Err(TransportError::Refused(
            "the other device did not answer with the right code".into(),
        ));
    }

    let payload = serde_json::to_vec(handover)
        .map_err(|_| TransportError::Refused("could not pack the handover".into()))?;
    postbox.put(&seal(&key, &labels.a_to_b, &payload)?)?;

    // And it says who it is, so the other side can name it.
    let joined = await_message(postbox, 2, deadline)?;
    serde_json::from_slice(&open(&key, &labels.b_to_a, &joined)?)
        .map_err(|_| TransportError::Refused("the joining device answered oddly".into()))
}

/// The joining side: prove the code, take the secrets.
pub fn take_over(
    postbox: &impl Postbox,
    flavor: Flavor,
    id: &str,
    words: &str,
    deadline: Instant,
) -> Result<(Handover, Zeroizing<[u8; 32]>), TransportError> {
    use spake2::{Ed25519Group, Identity, Password, Spake2};
    let labels = Labels::of(flavor);

    let (state, ours) = Spake2::<Ed25519Group>::start_b(
        &Password::new(password(id, words)),
        &Identity::new(&labels.side_a),
        &Identity::new(&labels.side_b),
    );
    let theirs = await_message(postbox, 0, deadline)?;
    postbox.put(&ours)?;

    let key: [u8; 32] = state
        .finish(&theirs)
        .map_err(|_| TransportError::Refused("that code does not match".into()))?
        .try_into()
        .map_err(|_| TransportError::Refused("the handshake went wrong".into()))?;
    let key = Zeroizing::new(key);

    postbox.put(&seal(&key, &labels.b_to_a, &labels.confirm)?)?;
    let payload = await_message(postbox, 1, deadline)?;
    let handover: Handover = serde_json::from_slice(&open(&key, &labels.a_to_b, &payload)?)
        .map_err(|_| TransportError::Refused("the handover made no sense".into()))?;
    Ok((handover, key))
}

/// The joining side, once it is in: tell the other device what it is called.
pub fn say_joined(
    postbox: &impl Postbox,
    flavor: Flavor,
    key: &[u8; 32],
    joined: &Joined,
) -> Result<(), TransportError> {
    let payload = serde_json::to_vec(joined)
        .map_err(|_| TransportError::Refused("could not pack the answer".into()))?;
    postbox.put(&seal(key, &Labels::of(flavor).b_to_a, &payload)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::{Condvar, Mutex};
    use std::sync::Arc;

    /// A relay in memory: two sides, two lists of messages. A side asking for
    /// a message waits until there is one — so nothing here ever sleeps — and
    /// learns the other side is gone when it is dropped.
    #[derive(Default)]
    struct Relay {
        state: Mutex<Boxes>,
        changed: Condvar,
    }

    #[derive(Default)]
    struct Boxes {
        a: Vec<Vec<u8>>,
        b: Vec<Vec<u8>>,
        closed: bool,
    }

    struct Side {
        relay: Arc<Relay>,
        mine: bool,
    }

    impl Drop for Side {
        fn drop(&mut self) {
            self.relay.state.lock().closed = true;
            self.relay.changed.notify_all();
        }
    }

    impl Postbox for Side {
        fn put(&self, message: &[u8]) -> Result<(), TransportError> {
            let mut boxes = self.relay.state.lock();
            if self.mine {
                boxes.a.push(message.to_vec());
            } else {
                boxes.b.push(message.to_vec());
            }
            self.relay.changed.notify_all();
            Ok(())
        }

        fn take(&self, after: usize, _wait: bool) -> Result<Vec<Vec<u8>>, TransportError> {
            let mut boxes = self.relay.state.lock();
            loop {
                let theirs = if self.mine { &boxes.b } else { &boxes.a };
                if theirs.len() > after {
                    return Ok(theirs[after..].to_vec());
                }
                if boxes.closed {
                    return Err(TransportError::Unreachable("the other side left".into()));
                }
                self.relay.changed.wait(&mut boxes);
            }
        }
    }

    fn sides() -> (Side, Side) {
        let relay = Arc::new(Relay::default());
        (
            Side {
                relay: relay.clone(),
                mine: true,
            },
            Side { relay, mine: false },
        )
    }

    fn handover() -> Handover {
        Handover {
            server_url: "https://sync.example.com:8443".into(),
            tls_fingerprint: Some("SHA256:abc".into()),
            account_id: Uuid::now_v7(),
            account_key: Some(crate::http::encode_base64(
                AccountKey::from_bytes([9u8; 16]).as_bytes(),
            )),
            enrolment: "one-time-token".into(),
            manifest: Some(ManifestFloor {
                id: Uuid::now_v7(),
                clock: uwunews_proto::Hlc::new(1_700_000_000_000, 2, 7),
            }),
        }
    }

    /// The whole handshake, both sides, through a relay that understands none
    /// of it — which is the point.
    #[test]
    fn a_message_cannot_be_reflected_back_to_its_sender() {
        let key = [9u8; 32];
        let labels = Labels::of(Flavor::Ssh);
        let sealed = seal(&key, &labels.a_to_b, b"handover").unwrap();
        assert_eq!(open(&key, &labels.a_to_b, &sealed).unwrap(), b"handover");
        assert!(open(&key, &labels.b_to_a, &sealed).is_err());
        // Nor read under another app's labels.
        assert!(open(&key, &Labels::of(Flavor::Rdp).a_to_b, &sealed).is_err());
    }

    #[test]
    fn two_devices_agree_on_a_key_and_hand_the_account_key_over() {
        for flavor in [Flavor::Ssh, Flavor::Rdp] {
            handshake(flavor);
        }
    }

    fn handshake(flavor: Flavor) {
        let (a, b) = sides();
        let id = "K7M4Q";
        let words = words();
        let secrets = handover();
        let deadline = Instant::now() + Duration::from_secs(5);

        let joining = std::thread::spawn({
            let words = words.clone();
            move || {
                let (handover, key) = take_over(&b, flavor, id, &words, deadline).unwrap();
                say_joined(
                    &b,
                    flavor,
                    &key,
                    &Joined {
                        device_name: "LVLaptop".into(),
                        device_id: Uuid::now_v7(),
                    },
                )
                .unwrap();
                handover
            }
        });

        let joined = hand_over(&a, flavor, id, &words, &secrets, deadline).unwrap();
        assert_eq!(joined.device_name, "LVLaptop");

        let taken = joining.join().unwrap();
        assert_eq!(taken.server_url, secrets.server_url);
        assert_eq!(taken.enrolment, "one-time-token");
        assert_eq!(
            taken.account_key().unwrap().as_bytes(),
            &[9u8; 16],
            "the account key came across"
        );
        assert_eq!(taken.manifest, secrets.manifest, "and so did the floor");
    }

    #[test]
    fn a_handover_from_a_build_before_manifests_still_reads() {
        let older = serde_json::json!({
            "server_url": "https://sync.example.com:8443",
            "tls_fingerprint": null,
            "account_id": Uuid::now_v7(),
            "account_key": null,
            "enrolment": "one-time-token",
        });
        let handover: Handover = serde_json::from_value(older).unwrap();
        assert_eq!(handover.manifest, None);
        assert_eq!(handover.enrolment, "one-time-token");

        // And one with nothing to say leaves the field out, for a build that
        // might refuse what it does not know.
        let written = serde_json::to_value(Handover {
            manifest: None,
            ..handover
        })
        .unwrap();
        assert!(written.get("manifest").is_none());
    }

    #[test]
    fn the_wrong_code_gets_nothing_and_the_secret_never_goes_out() {
        let (a, b) = sides();
        let id = "K7M4Q";
        let deadline = Instant::now() + Duration::from_secs(5);

        let guessing = std::thread::spawn(move || {
            // Somebody who guessed the id but not the words.
            take_over(&b, Flavor::Ssh, id, "tiger-tiger-tiger", deadline)
        });

        let outcome = hand_over(&a, Flavor::Ssh, id, &words(), &handover(), deadline);
        assert!(outcome.is_err(), "the handover must not happen");
        let relay = a.relay.clone();
        drop(a);
        assert!(guessing.join().unwrap().is_err(), "and nothing arrives");

        // What did travel: two SPAKE2 messages and one confirmation that did
        // not check out. No sealed handover at all.
        let boxes = relay.state.lock();
        assert_eq!(boxes.a.len(), 1, "only the SPAKE2 message from A");
        assert_eq!(boxes.b.len(), 2);
    }

    #[test]
    fn a_code_from_another_session_does_not_open_this_one() {
        let (a, b) = sides();
        let words = words();
        let deadline = Instant::now() + Duration::from_secs(5);

        let joining = std::thread::spawn({
            let words = words.clone();
            // The same words, a different session.
            move || take_over(&b, Flavor::Ssh, "OTHER", &words, deadline)
        });
        assert!(hand_over(&a, Flavor::Ssh, "K7M4Q", &words, &handover(), deadline).is_err());
        drop(a);
        assert!(joining.join().unwrap().is_err());
    }

    #[test]
    fn a_device_joining_with_another_apps_labels_gets_nothing() {
        let (a, b) = sides();
        let words = words();
        let deadline = Instant::now() + Duration::from_secs(5);
        let joining = std::thread::spawn({
            let words = words.clone();
            move || take_over(&b, Flavor::Ssh, "K7M4Q", &words, deadline)
        });
        assert!(hand_over(&a, Flavor::Rdp, "K7M4Q", &words, &handover(), deadline).is_err());
        drop(a);
        assert!(joining.join().unwrap().is_err());
    }

    #[test]
    fn a_code_reads_back_as_it_was_read_out() {
        let offer = offer(
            "K7M4Q",
            "tiger-radio-kiwi",
            "https://sync.example.com:8443",
            Some("SHA256:abc"),
        );
        assert_eq!(offer.spoken, "K7M4Q-tiger-radio-kiwi");

        let target = parse_offer(&offer.pasteable).unwrap();
        assert_eq!(target.server_url, "https://sync.example.com:8443");
        assert_eq!(target.tls_fingerprint.as_deref(), Some("SHA256:abc"));
        assert_eq!(target.id, "K7M4Q");
        assert_eq!(target.words, "tiger-radio-kiwi");

        assert!(parse_offer("uwu2_notbase64!!").is_none());
        assert!(parse_offer("something else").is_none());
    }

    #[test]
    fn a_spoken_code_is_taken_as_it_was_said() {
        for said in [
            "K7M4Q-tiger-radio-kiwi",
            "k7m4q tiger radio kiwi",
            "  K7M4Q-Tiger-Radio-Kiwi  ",
            "k7m4q_tiger_radio_kiwi",
        ] {
            assert_eq!(
                parse_spoken(said),
                Some(("K7M4Q".to_string(), "tiger-radio-kiwi".to_string())),
                "{said}"
            );
        }

        // A word nobody would have shown: misheard rather than mistyped.
        assert!(parse_spoken("K7M4Q-tiger-radio-klingon").is_none());
        assert!(parse_spoken("K7M4Q-tiger-radio").is_none());
        assert!(parse_spoken("K7M4Q").is_none());
    }

    #[test]
    fn the_words_are_words_and_they_do_not_repeat_themselves() {
        let first = words();
        assert_eq!(first.split('-').count(), WORD_COUNT);
        assert!(first.split('-').all(|word| WORDS.contains(&word)));
        // Not a proof of randomness, just that it is not a constant.
        let many: std::collections::HashSet<String> = (0..20).map(|_| words()).collect();
        assert!(many.len() > 1);
    }

    #[test]
    fn the_word_list_has_no_duplicates() {
        let unique: std::collections::HashSet<&str> = WORDS.iter().copied().collect();
        assert_eq!(unique.len(), WORDS.len(), "a duplicate would waste a bit");
    }

    #[test]
    fn a_handover_never_prints_what_it_carries() {
        let printed = format!("{:?}", handover());
        assert_eq!(printed, "Handover(redacted)");
    }
}
