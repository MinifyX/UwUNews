use std::sync::Arc;
use uuid::Uuid;
use uwunews_proto::Envelope;
use uwunews_store::{ApplyReport, MemorySecrets, Pushed, SourceDraft, SourceRecord, Store};
use uwunews_vault::{create_with, Flavor, KdfParams, UnlockedVault, VaultHeader};

pub fn store() -> (Store, Arc<MemorySecrets>) {
    let secrets = Arc::new(MemorySecrets::new());
    let store = Store::open_in_memory(secrets.clone()).expect("open");
    (store, secrets)
}

pub fn source(store: &Store, url: &str) -> SourceRecord {
    store
        .add_source(SourceDraft {
            url: url.into(),
            ..Default::default()
        })
        .expect("add source")
}

/// A fresh vault, as `create_account` makes one.
pub fn vault(flavor: Flavor) -> (VaultHeader, UnlockedVault) {
    create_with(
        b"correct horse",
        None,
        Uuid::now_v7(),
        KdfParams::INSECURE_FOR_TESTS,
        flavor,
    )
    .expect("vault")
}

/// Both stores take the same vault.
pub fn share(a: &Store, b: &Store, flavor: Flavor) -> Uuid {
    let (header, unlocked) = vault(flavor);
    let id = unlocked.vault_id();
    a.adopt_vault(id, unlocked.export_key(), flavor, Some(&header))
        .expect("adopt a");
    b.adopt_vault(id, unlocked.export_key(), flavor, Some(&header))
        .expect("adopt b");
    id
}

/// A stand-in for the server: hands out sequence numbers.
#[derive(Default)]
pub struct Server {
    pub seq: u64,
}

impl Server {
    /// Everything `from` has pending, accepted.
    pub fn push(&mut self, from: &Store) -> Vec<Envelope> {
        let (mut envelopes, left_out) = from.pending_envelopes(1000).expect("pending");
        assert_eq!(left_out, 0);
        let mut pushed = Vec::new();
        for env in &mut envelopes {
            self.seq += 1;
            env.seq = Some(self.seq);
            pushed.push(Pushed {
                id: env.id,
                kind: env.kind,
                updated_at: env.updated_at,
                seq: self.seq,
            });
        }
        from.mark_pushed(&pushed).expect("mark pushed");
        envelopes
    }

    /// Push from `from`, apply on `to`.
    pub fn relay(&mut self, from: &Store, to: &Store) -> ApplyReport {
        let envelopes = self.push(from);
        to.apply_envelopes(&envelopes).expect("apply")
    }
}
