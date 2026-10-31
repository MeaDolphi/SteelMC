//! Recoverable publication of player snapshots and opaque gameplay documents.
//!
//! Callers must reserve the affected live inventories/state before capturing
//! after-images and keep the reservation until the server job applies the result.
//! This module coordinates persistence, not live gameplay locks.

use std::path::{Component, Path, PathBuf};

use rustc_hash::FxHashSet;
use steel_utils::Identifier;
use tokio::{fs, io, io::AsyncWriteExt};
use uuid::Uuid;
use wincode::{SchemaRead, SchemaWrite};

use super::file_storage::FilePlayerDataStorage;
use super::stats::{PlayerStatsFile, serialize_player_stats_file};
use super::{PlayerDataFile, encode_player_file};
use crate::player::player_data::PersistentPlayerData;

/// One player's complete domain after-image in a storage transaction.
pub struct PlayerStorageWrite {
    /// Domain containing this player's inventory.
    pub domain: String,
    /// Stable identity, independent of respawned entity bodies.
    pub uuid: Uuid,
    /// Captured after-image.
    pub data: PersistentPlayerData,
}

/// A compare-and-swap of a gameplay-owned document, outside vanilla player data.
pub struct ExtensionStorageWrite {
    /// Owner-scoped key, for example `sololeveling:players/<uuid>`.
    pub key: Identifier,
    /// Exact previous bytes, or `None` if this document must not exist yet.
    pub expected: Option<Vec<u8>>,
    /// Complete replacement document. The owner defines its format.
    pub replacement: Vec<u8>,
}

/// A group of player snapshots and gameplay documents published together.
#[derive(Default)]
pub struct PlayerStorageTransaction {
    /// Player inventory/state after-images.
    pub players: Vec<PlayerStorageWrite>,
    /// Owner-scoped documents with optimistic concurrency checks.
    pub extensions: Vec<ExtensionStorageWrite>,
}

/// The publication point is record rename, not the last checkpoint write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionCommit {
    /// The redo record was synced. Checkpoint failures will be recovered.
    Committed,
    /// Rename succeeded but directory durability could not be confirmed.
    /// The caller must install the after-images, not retry as a new purchase.
    DurabilityUncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite)]
enum Target {
    Player { domain: String, uuid: [u8; 16] },
    Stats { domain: String, uuid: [u8; 16] },
    Extension { namespace: String, path: String },
}

#[derive(SchemaRead, SchemaWrite)]
struct FileWrite {
    target: Target,
    contents: Vec<u8>,
}

#[derive(SchemaRead, SchemaWrite)]
struct RedoRecord {
    version: u32,
    writes: Vec<FileWrite>,
}

const RECORD_VERSION: u32 = 1;

/// Disk operations run only behind `PlayerDataStorage`'s transaction gate.
pub(super) struct FileTransactions {
    root: PathBuf,
}

impl FileTransactions {
    pub(super) const fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(super) async fn initialize(&self) -> io::Result<()> {
        create_synced_directories(parent_of(&self.record_path())?).await
    }

    fn record_path(&self) -> PathBuf {
        self.root.join("global/transactions/pending")
    }

    fn target_path(&self, target: &Target) -> io::Result<PathBuf> {
        let relative = match target {
            Target::Player { domain, uuid } => {
                validate_component(domain)?;
                PathBuf::from(domain).join(format!("players/data/{}", Uuid::from_bytes(*uuid)))
            }
            Target::Stats { domain, uuid } => {
                validate_component(domain)?;
                PathBuf::from(domain)
                    .join(format!("players/stats/{}.toml", Uuid::from_bytes(*uuid)))
            }
            Target::Extension { namespace, path } => {
                validate_component(namespace)?;
                validate_relative(path)?;
                PathBuf::from("global/extensions")
                    .join(namespace)
                    .join(format!("{path}.document"))
            }
        };
        Ok(self.root.join(relative))
    }

    pub(super) async fn load_extension(&self, key: &Identifier) -> io::Result<Option<Vec<u8>>> {
        let path = self.target_path(&extension_target(key))?;
        if !FilePlayerDataStorage::recover_missing_atomic_path_locked(&path).await? {
            return Ok(None);
        }
        match fs::read(path).await {
            Ok(contents) => Ok(Some(contents)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Republishes the last committed backup over the current document bytes.
    ///
    /// Gameplay owners call this when the current document no longer decodes;
    /// the backup file itself is kept, so the restore stays repeatable until
    /// the next committed write refreshes it.
    pub(super) async fn restore_extension_backup(
        &self,
        key: &Identifier,
    ) -> io::Result<Option<Vec<u8>>> {
        let path = self.target_path(&extension_target(key))?;
        let backup = FilePlayerDataStorage::atomic_backup_path(&path);
        let bytes = match fs::read(&backup).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let parent = parent_of(&path)?;
        create_synced_directories(parent).await?;
        let temp = FilePlayerDataStorage::atomic_temp_path(&path);
        write_synced(&temp, &bytes).await?;
        fs::rename(&temp, &path).await?;
        sync_directory(parent).await?;
        Ok(Some(bytes))
    }

    pub(super) async fn commit(
        &self,
        transaction: &PlayerStorageTransaction,
    ) -> io::Result<TransactionCommit> {
        let record = Self::encode_transaction(transaction)?;
        self.validate_record(&record)?;
        for extension in &transaction.extensions {
            if self.load_extension(&extension.key).await? != extension.expected {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "gameplay document changed",
                ));
            }
        }
        let bytes = wincode::serialize(&record).map_err(io::Error::other)?;
        let path = self.record_path();
        let parent = parent_of(&path)?;
        create_synced_directories(parent).await?;
        let temp = path.with_extension("tmp");
        write_synced(&temp, &bytes).await?;
        fs::rename(temp, &path).await?;
        if let Err(error) = sync_directory(parent).await {
            log::error!("Player transaction published; durability uncertain: {error}");
            return Ok(TransactionCommit::DurabilityUncertain);
        }
        if let Err(error) = self.recover().await {
            log::error!("Player transaction committed; checkpoint must be recovered: {error}");
        }
        Ok(TransactionCommit::Committed)
    }

    fn encode_transaction(transaction: &PlayerStorageTransaction) -> io::Result<RedoRecord> {
        let mut writes = Vec::new();
        for player in &transaction.players {
            writes.push(FileWrite {
                target: Target::Player {
                    domain: player.domain.clone(),
                    uuid: *player.uuid.as_bytes(),
                },
                contents: encode_player_file(&PlayerDataFile::from_persistent(&player.data)?)?,
            });
            writes.push(FileWrite {
                target: Target::Stats {
                    domain: player.domain.clone(),
                    uuid: *player.uuid.as_bytes(),
                },
                contents: serialize_player_stats_file(&PlayerStatsFile::from_persistent_stats(
                    &player.data.stats,
                )?)
                .into_bytes(),
            });
        }
        for extension in &transaction.extensions {
            writes.push(FileWrite {
                target: extension_target(&extension.key),
                contents: extension.replacement.clone(),
            });
        }
        Ok(RedoRecord {
            version: RECORD_VERSION,
            writes,
        })
    }

    fn validate_record(&self, record: &RedoRecord) -> io::Result<()> {
        if record.version != RECORD_VERSION || record.writes.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid player transaction version or empty transaction",
            ));
        }
        let mut paths = FxHashSet::default();
        for write in &record.writes {
            let path = self.target_path(&write.target)?;
            if !paths.insert(path) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "duplicate transaction destination",
                ));
            }
        }
        for path in &paths {
            if path
                .ancestors()
                .skip(1)
                .any(|parent| paths.contains(parent))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "transaction destination is also a parent directory",
                ));
            }
        }
        Ok(())
    }

    /// Called at startup and before every participating storage read/write.
    pub(super) async fn recover(&self) -> io::Result<()> {
        let path = self.record_path();
        let bytes = match fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let record: RedoRecord = wincode::deserialize(&bytes).map_err(io::Error::other)?;
        self.validate_record(&record)?;
        for write in &record.writes {
            let target = self.target_path(&write.target)?;
            // Replay through the same atomic publication as ordinary saves so
            // every checkpointed target keeps its `_old` backup for recovery.
            FilePlayerDataStorage::write_atomic_path_locked(&target, &write.contents).await?;
        }
        fs::remove_file(&path).await?;
        sync_directory(parent_of(&path)?).await
    }
}

fn extension_target(key: &Identifier) -> Target {
    Target::Extension {
        namespace: key.namespace.to_string(),
        path: key.path.to_string(),
    }
}

pub(super) fn validate_key(key: &Identifier) -> io::Result<()> {
    validate_component(&key.namespace)?;
    validate_relative(&key.path)
}

pub(super) fn validate_domain(domain: &str) -> io::Result<()> {
    validate_component(domain)
}

fn validate_component(value: &str) -> io::Result<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid storage path component",
        ));
    }
    Ok(())
}

fn validate_relative(value: &str) -> io::Result<()> {
    if value.is_empty()
        || value
            .split('/')
            .any(|part| validate_component(part).is_err())
        || Path::new(value)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid relative storage key",
        ));
    }
    Ok(())
}

fn parent_of(path: &Path) -> io::Result<&Path> {
    path.parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "storage path has no parent"))
}

async fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::File::create(path).await?;
    file.write_all(bytes).await?;
    file.sync_all().await
}

async fn sync_directory(path: &Path) -> io::Result<()> {
    // A relative save root such as `saves` has an empty parent, which names the
    // working directory.
    let path = if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    };
    if cfg!(unix) {
        fs::File::open(path).await?.sync_all().await?;
    }
    Ok(())
}

async fn create_synced_directories(path: &Path) -> io::Result<()> {
    let mut missing = Vec::new();
    for directory in path.ancestors() {
        // The empty path names the working directory, which always exists and
        // cannot be created.
        if directory.as_os_str().is_empty() {
            break;
        }
        if fs::try_exists(directory).await? {
            break;
        }
        missing.push(directory);
    }
    for directory in missing.into_iter().rev() {
        match fs::create_dir(directory).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        sync_directory(parent_of(directory)?).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
