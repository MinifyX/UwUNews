use std::sync::Arc;
use uuid::Uuid;
use uwunews_store::{ArticleDraft, MemorySecrets, SourceDraft, SourceRecord, Store};
use uwunews_sync::{sync_once, MemoryServer, SyncReport};
use uwunews_vault::{create_with, Flavor, KdfParams, UnlockedVault, VaultHeader};

pub fn store() -> Arc<Store> {
    Arc::new(Store::open_in_memory(Arc::new(MemorySecrets::new())).expect("open"))
}

pub fn source(store: &Store, url: &str) -> SourceRecord {
    store
        .add_source(SourceDraft {
            url: url.into(),
            ..Default::default()
        })
        .expect("add source")
}

/// The same item, as every device's fetch of the feed finds it.
pub fn article(store: &Store, source_id: Uuid, guid: &str) -> String {
    store
        .upsert_articles(
            source_id,
            &[ArticleDraft {
                guid: guid.into(),
                title: format!("Item {guid}"),
                link: Some(format!("https://news.example.com/{guid}")),
                published_at: Some(1_700_000_000_000),
                ..Default::default()
            }],
        )
        .expect("upsert");
    uwunews_store::article_key(source_id, guid)
}

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

/// Every store takes the same vault, as pairing leaves them.
pub fn share(stores: &[&Store], flavor: Flavor) -> Uuid {
    let (header, unlocked) = vault(flavor);
    for store in stores {
        store
            .adopt_vault(
                unlocked.vault_id(),
                unlocked.export_key(),
                flavor,
                Some(&header),
            )
            .expect("adopt");
    }
    unlocked.vault_id()
}

pub fn pass(store: &Store, server: &MemoryServer) -> SyncReport {
    let report = sync_once(store, server).expect("sync");
    assert_eq!(report.apply.rejected, 0, "{report:?}");
    report
}
