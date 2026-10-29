use super::*;
use crate::player::player_data::PLAYER_DATA_VERSION;
use crate::player::player_data::PersistentSlot;
use crate::player::player_data_storage::{PlayerDataStorage, tests::sample_player_file};
use std::env;
use steel_registry::{init_vanilla_registry, item_stack::ItemStack, vanilla_items};

fn directory() -> PathBuf {
    env::temp_dir().join(format!("steel-player-transaction-{}", Uuid::new_v4()))
}

fn player_write(uuid: Uuid) -> PlayerStorageWrite {
    init_vanilla_registry();
    let mut data = sample_player_file(PLAYER_DATA_VERSION)
        .into_persistent()
        .expect("fixture");
    data.inventory.push(PersistentSlot {
        slot: 0,
        item: ItemStack::new(&vanilla_items::DIAMOND),
    });
    PlayerStorageWrite {
        domain: "lobby".to_owned(),
        uuid,
        data,
    }
}

fn document(
    path: &'static str,
    expected: Option<&[u8]>,
    replacement: &[u8],
) -> ExtensionStorageWrite {
    ExtensionStorageWrite {
        key: Identifier::new("test", path),
        expected: expected.map(<[u8]>::to_vec),
        replacement: replacement.to_vec(),
    }
}

async fn exercise_atomicity(storage: PlayerDataStorage) {
    let uuid = Uuid::new_v4();
    let transaction = PlayerStorageTransaction {
        players: vec![player_write(uuid)],
        extensions: vec![document("account", None, b"paid")],
    };
    assert_eq!(
        storage
            .commit_transaction(&transaction)
            .await
            .expect("commit"),
        TransactionCommit::Committed
    );
    let loaded = storage
        .load_domain("lobby", uuid)
        .await
        .expect("load")
        .expect("player");
    assert_eq!(loaded.inventory[0].item.item, &*vanilla_items::DIAMOND);
    assert_eq!(
        storage
            .load_extension(&Identifier::new("test", "account"))
            .await
            .expect("document"),
        Some(b"paid".to_vec())
    );

    let second_uuid = Uuid::new_v4();
    let rejected = PlayerStorageTransaction {
        players: vec![player_write(second_uuid)],
        extensions: vec![
            document("new-document", None, b"must not appear"),
            document("account", None, b"double spend"),
        ],
    };
    assert_eq!(
        storage
            .commit_transaction(&rejected)
            .await
            .expect_err("CAS should reject")
            .kind(),
        io::ErrorKind::WouldBlock
    );
    assert!(
        storage
            .load_domain("lobby", second_uuid)
            .await
            .expect("load")
            .is_none()
    );
    assert!(
        storage
            .load_extension(&Identifier::new("test", "new-document"))
            .await
            .expect("load")
            .is_none()
    );
}

#[tokio::test]
async fn memory_and_disk_apply_or_reject_the_whole_batch() {
    exercise_atomicity(PlayerDataStorage::in_memory()).await;
    let root = directory();
    exercise_atomicity(
        PlayerDataStorage::on_disk(root.clone())
            .await
            .expect("storage"),
    )
    .await;
    fs::remove_dir_all(root).await.expect("cleanup");
}

#[tokio::test]
async fn startup_replays_a_committed_partially_checkpointed_transaction() {
    let root = directory();
    let files = FileTransactions::new(root.clone());
    let uuid = Uuid::new_v4();
    let transaction = PlayerStorageTransaction {
        players: vec![player_write(uuid)],
        extensions: vec![document("account", None, b"paid")],
    };
    let record = FileTransactions::encode_transaction(&transaction).expect("encode");
    let path = files.record_path();
    create_synced_directories(parent_of(&path).expect("parent"))
        .await
        .expect("mkdir");
    write_synced(&path, &wincode::serialize(&record).expect("encode record"))
        .await
        .expect("record");
    // The first player file reached disk, the statistics and payment did not.
    let first = &record.writes[0];
    let target = files.target_path(&first.target).expect("path");
    create_synced_directories(parent_of(&target).expect("parent"))
        .await
        .expect("mkdir");
    write_synced(&target, &first.contents)
        .await
        .expect("partial checkpoint");

    let restarted = PlayerDataStorage::on_disk(root.clone())
        .await
        .expect("recover");
    assert!(
        restarted
            .load_domain("lobby", uuid)
            .await
            .expect("load")
            .is_some()
    );
    assert_eq!(
        restarted
            .load_extension(&Identifier::new("test", "account"))
            .await
            .expect("document"),
        Some(b"paid".to_vec())
    );
    assert!(!fs::try_exists(&path).await.expect("record removed"));
    drop(restarted);
    let again = PlayerDataStorage::on_disk(root.clone())
        .await
        .expect("idempotent recovery");
    assert_eq!(
        again
            .load_extension(&Identifier::new("test", "account"))
            .await
            .expect("document"),
        Some(b"paid".to_vec())
    );
    fs::remove_dir_all(root).await.expect("cleanup");
}

#[tokio::test]
async fn delayed_ordinary_save_cannot_undo_a_newer_transaction() {
    let storage = PlayerDataStorage::in_memory();
    let uuid = Uuid::new_v4();
    let old = player_write(uuid);
    let newer = player_write(uuid);
    storage
        .commit_transaction(&PlayerStorageTransaction {
            players: vec![newer],
            extensions: vec![],
        })
        .await
        .expect("commit");
    let mut old_data = old.data;
    old_data.inventory.clear();
    storage
        .save_domain_data("lobby", uuid, &old_data)
        .await
        .expect("stale save ignored");
    assert_eq!(
        storage
            .load_domain("lobby", uuid)
            .await
            .expect("load")
            .expect("player")
            .inventory
            .len(),
        1
    );
    assert_eq!(
        storage
            .commit_transaction(&PlayerStorageTransaction {
                players: vec![PlayerStorageWrite {
                    domain: "lobby".to_owned(),
                    uuid,
                    data: old_data
                }],
                extensions: vec![document("stale-account", None, b"must not appear")],
            })
            .await
            .expect_err("stale transaction rejected")
            .kind(),
        io::ErrorKind::WouldBlock
    );
    assert!(
        storage
            .load_extension(&Identifier::new("test", "stale-account"))
            .await
            .expect("load")
            .is_none()
    );
}

#[tokio::test]
async fn duplicate_and_escaping_keys_are_rejected_before_publication() {
    for storage in [PlayerDataStorage::in_memory()] {
        for extensions in [
            vec![document("../outside", None, b"bad")],
            vec![
                document("same", None, b"one"),
                document("same", None, b"two"),
            ],
        ] {
            assert_eq!(
                storage
                    .commit_transaction(&PlayerStorageTransaction {
                        players: vec![],
                        extensions,
                    })
                    .await
                    .expect_err("invalid transaction")
                    .kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }
}

#[tokio::test]
async fn document_suffixes_do_not_alias_and_uncommitted_temp_is_not_replayed() {
    let root = directory();
    let files = FileTransactions::new(root.clone());
    let temporary = files.record_path().with_extension("tmp");
    create_synced_directories(parent_of(&temporary).expect("parent"))
        .await
        .expect("mkdir");
    write_synced(&temporary, b"incomplete record")
        .await
        .expect("temporary");
    let storage = PlayerDataStorage::on_disk(root.clone())
        .await
        .expect("ignore uncommitted temporary");
    storage
        .commit_transaction(&PlayerStorageTransaction {
            players: vec![],
            extensions: vec![
                document("item.a", None, b"first"),
                document("item.b", None, b"second"),
            ],
        })
        .await
        .expect("commit distinct keys");
    assert_eq!(
        storage
            .load_extension(&Identifier::new("test", "item.a"))
            .await
            .expect("load"),
        Some(b"first".to_vec())
    );
    assert_eq!(
        storage
            .load_extension(&Identifier::new("test", "item.b"))
            .await
            .expect("load"),
        Some(b"second".to_vec())
    );
    fs::remove_dir_all(root).await.expect("cleanup");
}

#[tokio::test]
async fn corrupt_committed_record_fails_startup_without_overwriting_accounts() {
    let root = directory();
    let files = FileTransactions::new(root.clone());
    let path = files.record_path();
    create_synced_directories(parent_of(&path).expect("parent"))
        .await
        .expect("mkdir");
    write_synced(&path, b"not a transaction")
        .await
        .expect("corrupt record");
    assert!(PlayerDataStorage::on_disk(root.clone()).await.is_err());
    assert_eq!(
        fs::read(&path).await.expect("record retained"),
        b"not a transaction"
    );
    fs::remove_dir_all(root).await.expect("cleanup");
}

#[tokio::test]
async fn checkpoint_failure_keeps_commit_and_recovery_blocks_unrelated_updates() {
    let root = directory();
    let storage = PlayerDataStorage::on_disk(root.clone())
        .await
        .expect("storage");
    let uuid = Uuid::new_v4();
    // An unwriteable player destination is discovered during checkpoint, not commit.
    let blocked = root.join(format!("lobby/players/data/{uuid}"));
    fs::create_dir_all(&blocked)
        .await
        .expect("block player destination");
    let transaction = PlayerStorageTransaction {
        players: vec![player_write(uuid)],
        extensions: vec![document("account", None, b"paid")],
    };
    assert_eq!(
        storage
            .commit_transaction(&transaction)
            .await
            .expect("record committed"),
        TransactionCommit::Committed,
    );
    assert!(
        storage
            .load_extension(&Identifier::new("test", "account"))
            .await
            .is_err()
    );
    assert!(
        storage
            .commit_transaction(&PlayerStorageTransaction {
                players: vec![],
                extensions: vec![document("unrelated", None, b"must wait")],
            })
            .await
            .is_err()
    );
    fs::remove_dir(&blocked).await.expect("repair destination");
    assert_eq!(
        storage
            .load_extension(&Identifier::new("test", "account"))
            .await
            .expect("recover"),
        Some(b"paid".to_vec()),
    );
    assert!(
        storage
            .load_domain("lobby", uuid)
            .await
            .expect("load")
            .is_some()
    );
    assert!(
        storage
            .load_extension(&Identifier::new("test", "unrelated"))
            .await
            .expect("load")
            .is_none()
    );
    fs::remove_dir_all(root).await.expect("cleanup");
}

#[tokio::test]
async fn a_relative_save_root_is_created_under_the_working_directory() {
    let root = PathBuf::from(format!("steel-relative-storage-{}", Uuid::new_v4()));
    FileTransactions::new(root.clone())
        .initialize()
        .await
        .expect("a save root without an existing top-level directory");
    assert!(
        fs::try_exists(root.join("global/transactions"))
            .await
            .expect("probe")
    );
    fs::remove_dir_all(root).await.expect("cleanup");
}
