//! SQLite implementation of player and gameplay-document storage.
//!
//! The schema deliberately uses portable relational types and standard
//! `ON CONFLICT` upserts so the same storage contract can gain a PostgreSQL
//! backend without changing callers.

use std::{path::PathBuf, time::Duration};

use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use tokio::{fs, io};
use uuid::Uuid;

use super::{
    GlobalPlayerData, PlayerDataFile, decode_player_file, encode_player_file,
    known_players::{KnownPlayersFile, decode_known_players_file, encode_known_players_file},
    permissions::{PlayerPermissionsFile, serialize_player_permissions_file},
    stats::{PlayerStatsFile, serialize_player_stats_file},
    transaction::{PlayerStorageTransaction, TransactionCommit},
};
use crate::{
    permission::PermissionSubjectIndex,
    player::{KnownPlayers, player_data::PersistentPlayerData},
};
use steel_utils::Identifier;

/// `SQLx` pool used by the `steel:sqlite` player-storage backend.
pub(super) struct SqlitePlayerDataStorage {
    pool: SqlitePool,
}

impl SqlitePlayerDataStorage {
    /// Opens or creates a SQLite database, using WAL for concurrent readers and
    /// full synchronous commits for durable inventory/account transactions.
    pub(super) async fn open(path: PathBuf) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await
            .map_err(storage_error)?;
        let storage = Self { pool };
        storage.initialize().await?;
        Ok(storage)
    }

    async fn initialize(&self) -> io::Result<()> {
        for statement in [
            "CREATE TABLE IF NOT EXISTS player_domain_data (\
                domain TEXT NOT NULL,\
                uuid TEXT NOT NULL,\
                data BYTEA NOT NULL,\
                stats TEXT NOT NULL,\
                save_order TEXT NOT NULL,\
                PRIMARY KEY (domain, uuid)\
            )",
            "CREATE TABLE IF NOT EXISTS global_player_data (\
                uuid TEXT PRIMARY KEY,\
                last_active_domain TEXT NOT NULL\
            )",
            "CREATE TABLE IF NOT EXISTS gameplay_documents (\
                namespace TEXT NOT NULL,\
                path TEXT NOT NULL,\
                payload BYTEA NOT NULL,\
                PRIMARY KEY (namespace, path)\
            )",
            "CREATE TABLE IF NOT EXISTS player_permissions (\
                singleton INTEGER PRIMARY KEY,\
                payload TEXT NOT NULL\
            )",
            "CREATE TABLE IF NOT EXISTS known_players (\
                singleton INTEGER PRIMARY KEY,\
                payload BYTEA NOT NULL\
            )",
        ] {
            sqlx::query(statement)
                .execute(&self.pool)
                .await
                .map_err(storage_error)?;
        }
        Ok(())
    }

    pub(super) async fn save_domain_data(
        &self,
        domain: &str,
        uuid: Uuid,
        data: &PersistentPlayerData,
    ) -> io::Result<()> {
        let file = PlayerDataFile::from_persistent(data)?;
        let player_data = encode_player_file(&file)?;
        let stats_file = PlayerStatsFile::from_persistent_stats(&data.stats)?;
        let stats = serialize_player_stats_file(&stats_file);
        sqlx::query(
            "INSERT INTO player_domain_data (domain, uuid, data, stats, save_order) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(domain, uuid) DO UPDATE SET \
                 data = excluded.data, stats = excluded.stats, save_order = excluded.save_order",
        )
        .bind(domain)
        .bind(uuid.hyphenated().to_string())
        .bind(player_data)
        .bind(stats)
        .bind(data.save_order.to_string())
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(())
    }

    pub(super) async fn load_domain(
        &self,
        domain: &str,
        uuid: Uuid,
    ) -> io::Result<Option<PersistentPlayerData>> {
        let row = sqlx::query(
            "SELECT data, stats, save_order FROM player_domain_data WHERE domain = ? AND uuid = ?",
        )
        .bind(domain)
        .bind(uuid.hyphenated().to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let data: Vec<u8> = row.try_get("data").map_err(storage_error)?;
        let stats: String = row.try_get("stats").map_err(storage_error)?;
        let save_order: String = row.try_get("save_order").map_err(storage_error)?;
        let mut data = decode_player_file(&data)?.into_persistent()?;
        data.stats = toml::from_str::<PlayerStatsFile>(&stats)
            .map_err(io::Error::other)?
            .into_persistent_stats();
        data.save_order = save_order
            .parse()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(Some(data))
    }

    pub(super) async fn load_extension(&self, key: &Identifier) -> io::Result<Option<Vec<u8>>> {
        let row =
            sqlx::query("SELECT payload FROM gameplay_documents WHERE namespace = ? AND path = ?")
                .bind(key.namespace.as_ref())
                .bind(key.path.as_ref())
                .fetch_optional(&self.pool)
                .await
                .map_err(storage_error)?;
        row.map(|row| row.try_get("payload").map_err(storage_error))
            .transpose()
    }

    pub(super) async fn commit_transaction(
        &self,
        transaction: &PlayerStorageTransaction,
    ) -> io::Result<TransactionCommit> {
        // The compare-and-swap reads before it writes. A deferred transaction
        // would take only a read lock first, and SQLite refuses to upgrade it
        // with SQLITE_BUSY as soon as another connection has written, without
        // waiting for the busy timeout. Taking the write lock up front makes
        // concurrent commits queue on the busy timeout instead.
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        for extension in &transaction.extensions {
            let current = sqlx::query(
                "SELECT payload FROM gameplay_documents WHERE namespace = ? AND path = ?",
            )
            .bind(extension.key.namespace.as_ref())
            .bind(extension.key.path.as_ref())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?
            .map(|row| row.try_get::<Vec<u8>, _>("payload"))
            .transpose()
            .map_err(storage_error)?;
            if current != extension.expected {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "gameplay document changed",
                ));
            }
        }
        for player in &transaction.players {
            let file = PlayerDataFile::from_persistent(&player.data)?;
            let data = encode_player_file(&file)?;
            let stats_file = PlayerStatsFile::from_persistent_stats(&player.data.stats)?;
            let stats = serialize_player_stats_file(&stats_file);
            sqlx::query(
                "INSERT INTO player_domain_data (domain, uuid, data, stats, save_order) \
                 VALUES (?, ?, ?, ?, ?) \
                 ON CONFLICT(domain, uuid) DO UPDATE SET \
                     data = excluded.data, stats = excluded.stats, save_order = excluded.save_order",
            )
            .bind(&player.domain)
            .bind(player.uuid.hyphenated().to_string())
            .bind(data)
            .bind(stats)
            .bind(player.data.save_order.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        }
        for extension in &transaction.extensions {
            sqlx::query(
                "INSERT INTO gameplay_documents (namespace, path, payload) VALUES (?, ?, ?) \
                 ON CONFLICT(namespace, path) DO UPDATE SET payload = excluded.payload",
            )
            .bind(extension.key.namespace.as_ref())
            .bind(extension.key.path.as_ref())
            .bind(&extension.replacement)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(TransactionCommit::Committed)
    }

    pub(super) async fn load_global(&self, uuid: Uuid) -> io::Result<Option<GlobalPlayerData>> {
        let row = sqlx::query("SELECT last_active_domain FROM global_player_data WHERE uuid = ?")
            .bind(uuid.hyphenated().to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?;
        row.map(|row| {
            row.try_get("last_active_domain")
                .map(|last_active_domain| GlobalPlayerData { last_active_domain })
                .map_err(storage_error)
        })
        .transpose()
    }

    pub(super) async fn save_global(&self, uuid: Uuid, data: &GlobalPlayerData) -> io::Result<()> {
        sqlx::query(
            "INSERT INTO global_player_data (uuid, last_active_domain) VALUES (?, ?) \
             ON CONFLICT(uuid) DO UPDATE SET last_active_domain = excluded.last_active_domain",
        )
        .bind(uuid.hyphenated().to_string())
        .bind(&data.last_active_domain)
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(())
    }

    pub(super) async fn load_permission_subjects(&self) -> io::Result<PermissionSubjectIndex> {
        let payload = sqlx::query("SELECT payload FROM player_permissions WHERE singleton = 1")
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .map(|row| row.try_get::<String, _>("payload"))
            .transpose()
            .map_err(storage_error)?;
        let Some(payload) = payload else {
            return Ok(PermissionSubjectIndex::new());
        };
        let file: PlayerPermissionsFile = toml::from_str(&payload).map_err(io::Error::other)?;
        file.validate()?;
        file.into_subject_index()
    }

    pub(super) async fn save_permission_subjects(
        &self,
        subjects: &PermissionSubjectIndex,
    ) -> io::Result<()> {
        let file = PlayerPermissionsFile::from_subject_index(subjects);
        let payload = serialize_player_permissions_file(&file).map_err(io::Error::other)?;
        sqlx::query(
            "INSERT INTO player_permissions (singleton, payload) VALUES (1, ?) \
             ON CONFLICT(singleton) DO UPDATE SET payload = excluded.payload",
        )
        .bind(payload)
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(())
    }

    pub(super) async fn load_known_players(&self) -> io::Result<KnownPlayers> {
        let payload = sqlx::query("SELECT payload FROM known_players WHERE singleton = 1")
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .map(|row| row.try_get::<Vec<u8>, _>("payload"))
            .transpose()
            .map_err(storage_error)?;
        let Some(payload) = payload else {
            return Ok(KnownPlayers::default());
        };
        decode_known_players_file(&payload)?.into_known_players()
    }

    pub(super) async fn save_known_players_if_current(
        &self,
        players: &KnownPlayers,
        is_current: impl FnOnce() -> bool + Send,
    ) -> io::Result<bool> {
        if !is_current() {
            return Ok(false);
        }
        let payload = encode_known_players_file(&KnownPlayersFile::from_known_players(players))?;
        sqlx::query(
            "INSERT INTO known_players (singleton, payload) VALUES (1, ?) \
             ON CONFLICT(singleton) DO UPDATE SET payload = excluded.payload",
        )
        .bind(payload)
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(true)
    }
}

fn storage_error(error: sqlx::Error) -> io::Error {
    io::Error::other(error)
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::sync::Arc;

    use futures::future::join_all;

    use super::*;
    use crate::player::player_data_storage::transaction::ExtensionStorageWrite;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_commits_wait_for_the_write_lock_instead_of_failing() {
        let path = env::temp_dir().join(format!("steel-player-data-{}.sqlite", Uuid::new_v4()));
        let storage = Arc::new(
            SqlitePlayerDataStorage::open(path.clone())
                .await
                .expect("open SQLite storage"),
        );
        let commits = (0..32).map(|index| {
            let storage = Arc::clone(&storage);
            tokio::spawn(async move {
                let transaction = PlayerStorageTransaction {
                    players: Vec::new(),
                    extensions: vec![ExtensionStorageWrite {
                        key: Identifier::new("test", format!("document_{index}")),
                        expected: None,
                        replacement: vec![1],
                    }],
                };
                storage.commit_transaction(&transaction).await
            })
        });
        for commit in join_all(commits).await {
            assert_eq!(
                commit.expect("task").expect("commit"),
                TransactionCommit::Committed
            );
        }
        drop(storage);
        fs::remove_file(path).await.expect("remove test database");
    }

    #[tokio::test]
    async fn document_batch_is_atomic_and_survives_reopening_the_database() {
        let path = env::temp_dir().join(format!("steel-player-data-{}.sqlite", Uuid::new_v4()));
        let account = Identifier::new_static("test", "account");
        let registry = Identifier::new_static("test", "registry");
        let storage = SqlitePlayerDataStorage::open(path.clone())
            .await
            .expect("open SQLite storage");
        let transaction = PlayerStorageTransaction {
            players: Vec::new(),
            extensions: vec![
                ExtensionStorageWrite {
                    key: account.clone(),
                    expected: None,
                    replacement: b"account-v1".to_vec(),
                },
                ExtensionStorageWrite {
                    key: registry.clone(),
                    expected: None,
                    replacement: b"registry-v1".to_vec(),
                },
            ],
        };
        assert_eq!(
            storage
                .commit_transaction(&transaction)
                .await
                .expect("commit new document"),
            TransactionCommit::Committed
        );
        drop(storage);

        let storage = SqlitePlayerDataStorage::open(path.clone())
            .await
            .expect("reopen SQLite storage");
        assert_eq!(
            storage
                .load_extension(&account)
                .await
                .expect("load account"),
            Some(b"account-v1".to_vec())
        );
        assert_eq!(
            storage
                .load_extension(&registry)
                .await
                .expect("load registry"),
            Some(b"registry-v1".to_vec())
        );
        let conflict = PlayerStorageTransaction {
            players: Vec::new(),
            extensions: vec![
                ExtensionStorageWrite {
                    key: account.clone(),
                    expected: Some(b"account-v1".to_vec()),
                    replacement: b"account-v2".to_vec(),
                },
                ExtensionStorageWrite {
                    key: registry.clone(),
                    expected: Some(b"stale".to_vec()),
                    replacement: b"registry-v2".to_vec(),
                },
            ],
        };
        assert_eq!(
            storage
                .commit_transaction(&conflict)
                .await
                .expect_err("reject stale document")
                .kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(
            storage
                .load_extension(&account)
                .await
                .expect("load account"),
            Some(b"account-v1".to_vec())
        );
        assert_eq!(
            storage
                .load_extension(&registry)
                .await
                .expect("load registry"),
            Some(b"registry-v1".to_vec())
        );
        drop(storage);
        fs::remove_file(path).await.expect("remove test database");
    }
}
