//! Per-player Solo Leveling data, persisted separately from vanilla
//! `PersistentPlayerData`.
//!
//! Each player's data is stored as its own JSON file, named by `Uuid`, under
//! a base directory supplied at construction time (a sibling of the vanilla
//! world save directory, not inside it).

use std::fs;
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use steel_utils::locks::SyncMutex;
use uuid::Uuid;

use crate::economy::{SystemPoints, Won};
use crate::organization::OrganizationBook;
use crate::quest::SystemQuest;
use crate::shadow::ShadowRoster;
use crate::shop::ShopDay;
use crate::stats::HunterStats;

/// Solo Leveling data tracked for a single player.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoloLevelingPlayerData {
    /// The player's `Uuid`, duplicated into the file for integrity checks
    /// independent of the file name.
    pub uuid: Uuid,
    /// This hunter's stats, level, and experience.
    #[serde(default)]
    pub stats: HunterStats,
    /// Hunter currency, separate from vanilla inventory items.
    #[serde(default)]
    pub won: Won,
    /// Non-transferable currency used exclusively by the personal System shop.
    #[serde(default)]
    pub system_points: SystemPoints,
    /// Current system-quest progress and pending penalty status.
    #[serde(default)]
    pub quest: SystemQuest,
    /// Persistent Shadow Monarch roster. Live summoned entities are recreated
    /// from these records and are never saved as vanilla world entities.
    #[serde(default)]
    pub shadows: ShadowRoster,
    /// Daily personal-shop purchases, reset by the Moscow day counter.
    #[serde(default)]
    pub shop: ShopDay,
    /// Personal Adventurer Organization contracts and today's allowance.
    #[serde(default)]
    pub organization: OrganizationBook,
}

impl SoloLevelingPlayerData {
    /// Creates fresh Solo Leveling data for a player who has none yet.
    #[must_use]
    pub fn new(uuid: Uuid) -> Self {
        Self {
            uuid,
            stats: HunterStats::new(),
            won: Won::starting_balance(),
            system_points: SystemPoints::default(),
            quest: SystemQuest::uninitialized(),
            shadows: ShadowRoster::default(),
            shop: ShopDay::default(),
            organization: OrganizationBook::new(),
        }
    }
}

/// Loads and saves [`SoloLevelingPlayerData`] as one JSON file per player,
/// under a base directory.
#[derive(Debug, Clone)]
pub struct PlayerDataStorage {
    base_dir: PathBuf,
    writes: Arc<SyncMutex<()>>,
}

/// Whether the redo record's directory entry was durably synchronized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentDurability {
    /// The payment can be acknowledged as committed.
    Confirmed,
    /// Live balances must reflect the record, but the client must not be told
    /// that a durable commit was confirmed.
    Uncertain,
}

impl PlayerDataStorage {
    /// Creates a new storage rooted at `base_dir`.
    ///
    /// The directory is not created here; call [`PlayerDataStorage::init`]
    /// before use.
    #[must_use]
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            base_dir,
            writes: Arc::new(SyncMutex::new(())),
        }
    }

    /// Creates the base directory if it does not already exist.
    pub fn init(&self) -> io::Result<()> {
        let _guard = self.writes.lock();
        fs::create_dir_all(&self.base_dir)?;
        self.recover_payment()
    }

    /// Returns the JSON file path for a given player.
    #[must_use]
    pub fn path_for(&self, uuid: Uuid) -> PathBuf {
        self.base_dir.join(format!("{uuid}.json"))
    }

    /// Loads a player's Solo Leveling data, returning `None` if no file
    /// exists for them yet (a new hunter).
    pub fn load(&self, uuid: Uuid) -> io::Result<Option<SoloLevelingPlayerData>> {
        let _guard = self.writes.lock();
        self.recover_payment()?;
        let path = self.path_for(uuid);
        if !path.exists() {
            return Ok(None);
        }
        let contents = fs::read_to_string(&path)?;
        let data = serde_json::from_str(&contents)
            .map_err(|err| io::Error::new(ErrorKind::InvalidData, err))?;
        Ok(Some(data))
    }

    /// Loads a player's data, or creates fresh data if none exists yet.
    pub fn load_or_create(&self, uuid: Uuid) -> io::Result<SoloLevelingPlayerData> {
        match self.load(uuid)? {
            Some(data) => Ok(data),
            None => Ok(SoloLevelingPlayerData::new(uuid)),
        }
    }

    /// Saves a player's Solo Leveling data, overwriting any existing file.
    pub fn save(&self, data: &SoloLevelingPlayerData) -> io::Result<()> {
        let _guard = self.writes.lock();
        self.recover_payment()?;
        self.save_player(data)
    }

    fn save_player(&self, data: &SoloLevelingPlayerData) -> io::Result<()> {
        let path = self.path_for(data.uuid);
        let contents = serde_json::to_string_pretty(data)
            .map_err(|err| io::Error::new(ErrorKind::InvalidData, err))?;
        self.replace_file(&path, contents.as_bytes())
    }

    /// Commits both player snapshots using one durable redo record.
    ///
    /// Once committed, a failed checkpoint is retried before any subsequent
    /// load/write and on startup. The caller must then install both live balances.
    pub fn save_payment(
        &self,
        players: &[SoloLevelingPlayerData; 2],
    ) -> io::Result<PaymentDurability> {
        let _guard = self.writes.lock();
        self.recover_payment()?;
        if players[0].uuid == players[1].uuid {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "identical payment accounts",
            ));
        }
        let contents = serde_json::to_vec(players)
            .map_err(|err| io::Error::new(ErrorKind::InvalidData, err))?;
        let path = self.base_dir.join("pending-payment.json");
        let temporary = path.with_extension("json.tmp");
        let mut record = fs::File::create(&temporary)?;
        record.write_all(&contents)?;
        record.sync_all()?;
        fs::rename(temporary, &path)?;
        // A directory sync failure after rename is not an ordinary rejection:
        // recovery can already observe the payment. Do not roll back live balances.
        if let Err(error) = fs::File::open(&self.base_dir).and_then(|dir| dir.sync_all()) {
            log::error!("sololeveling: payment commit durability uncertain: {error}");
            return Ok(PaymentDurability::Uncertain);
        }
        if let Err(error) = self.recover_payment() {
            log::error!("sololeveling: payment committed; checkpoint pending: {error}");
        }
        Ok(PaymentDurability::Confirmed)
    }

    fn recover_payment(&self) -> io::Result<()> {
        let path = self.base_dir.join("pending-payment.json");
        let contents = match fs::read(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let players: [SoloLevelingPlayerData; 2] = serde_json::from_slice(&contents)
            .map_err(|err| io::Error::new(ErrorKind::InvalidData, err))?;
        for player in &players {
            self.save_player(player)?;
        }
        fs::remove_file(path)?;
        fs::File::open(&self.base_dir)?.sync_all()
    }

    fn replace_file(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        let temporary = path.with_extension("json.tmp");
        let mut file = fs::File::create(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        fs::rename(temporary, path)?;
        fs::File::open(&self.base_dir)?.sync_all()
    }

    /// Returns the base directory this storage reads from and writes to.
    #[must_use]
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }
}

#[cfg(test)]
mod tests {
    use std::env;

    use super::*;

    #[test]
    fn save_then_load_round_trips() {
        let dir = env::temp_dir().join(format!("sololeveling-test-{}", Uuid::new_v4()));
        let storage = PlayerDataStorage::new(dir.clone());
        storage.init().expect("init should create the directory");

        let uuid = Uuid::new_v4();
        let data = SoloLevelingPlayerData::new(uuid);
        storage.save(&data).expect("save should succeed");

        let loaded = storage
            .load(uuid)
            .expect("load should succeed")
            .expect("data should exist after save");
        assert_eq!(loaded.uuid, uuid);

        fs::remove_dir_all(&dir).expect("cleanup should succeed");
    }

    #[test]
    fn load_missing_player_returns_none() {
        let dir = env::temp_dir().join(format!("sololeveling-test-{}", Uuid::new_v4()));
        let storage = PlayerDataStorage::new(dir.clone());
        storage.init().expect("init should create the directory");

        let loaded = storage.load(Uuid::new_v4()).expect("load should succeed");
        assert!(loaded.is_none());

        fs::remove_dir_all(&dir).expect("cleanup should succeed");
    }

    #[test]
    fn committed_payment_recovers_after_only_one_account_was_written() {
        let dir = env::temp_dir().join(format!("sololeveling-payment-test-{}", Uuid::new_v4()));
        let storage = PlayerDataStorage::new(dir.clone());
        storage.init().expect("storage should initialize");
        let mut sender = SoloLevelingPlayerData::new(Uuid::new_v4());
        let recipient = SoloLevelingPlayerData::new(Uuid::new_v4());
        sender.won.credit(100);
        storage.save(&sender).expect("initial sender should save");
        storage
            .save(&recipient)
            .expect("initial recipient should save");

        let (sender_balance, recipient_balance) = sender
            .won
            .payment(recipient.won, 30)
            .expect("payment should validate");
        sender.won = sender_balance;
        let mut recipient = recipient;
        recipient.won = recipient_balance;
        let players = [sender, recipient];
        let record = serde_json::to_vec(&players).expect("record should serialize");
        storage
            .replace_file(&dir.join("pending-payment.json"), &record)
            .expect("commit should persist");
        storage
            .save_player(&players[0])
            .expect("first checkpoint should save");

        let restarted = PlayerDataStorage::new(dir.clone());
        restarted.init().expect("restart should recover payment");
        for expected in &players {
            let actual = restarted
                .load(expected.uuid)
                .expect("read")
                .expect("account");
            assert_eq!(actual.won, expected.won);
        }
        restarted.init().expect("recovery is idempotent");
        assert!(!dir.join("pending-payment.json").exists());
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn system_points_survive_save_and_reload_separately_from_won() {
        let dir = env::temp_dir().join(format!("sololeveling-points-test-{}", Uuid::new_v4()));
        let storage = PlayerDataStorage::new(dir.clone());
        storage.init().expect("init");
        let mut data = SoloLevelingPlayerData::new(Uuid::new_v4());
        data.won.credit(25);
        assert!(data.system_points.credit(90));
        storage.save(&data).expect("save");
        let loaded = storage.load(data.uuid).expect("load").expect("account");
        assert_eq!(loaded.won.amount(), 25);
        assert_eq!(loaded.system_points.amount(), 90);
        fs::remove_dir_all(dir).expect("cleanup");
    }
}
