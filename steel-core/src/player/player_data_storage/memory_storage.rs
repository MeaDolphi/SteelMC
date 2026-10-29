use rustc_hash::FxHashMap;
use steel_utils::locks::SyncMutex;
use uuid::Uuid;

use super::GlobalPlayerData;
use super::transaction::{PlayerStorageTransaction, TransactionCommit};
use crate::permission::PermissionSubjectIndex;
use crate::player::KnownPlayers;
use crate::player::player_data::PersistentPlayerData;
use steel_utils::Identifier;
use tokio::io;

/// Player data storage that keeps everything in RAM and never touches disk.
#[derive(Default)]
pub(crate) struct MemoryPlayerDataStorage {
    state: SyncMutex<MemoryState>,
}

#[derive(Default)]
struct MemoryState {
    domain_players: FxHashMap<String, FxHashMap<Uuid, PersistentPlayerData>>,
    global_players: FxHashMap<Uuid, GlobalPlayerData>,
    permission_subjects: PermissionSubjectIndex,
    known_players: KnownPlayers,
    extensions: FxHashMap<Identifier, Vec<u8>>,
}

impl MemoryPlayerDataStorage {
    pub(crate) fn save_domain_data(&self, domain: &str, uuid: Uuid, data: &PersistentPlayerData) {
        self.state
            .lock()
            .domain_players
            .entry(domain.to_owned())
            .or_default()
            .insert(uuid, data.clone());
    }

    pub(crate) fn load_domain(&self, domain: &str, uuid: Uuid) -> Option<PersistentPlayerData> {
        self.state
            .lock()
            .domain_players
            .get(domain)?
            .get(&uuid)
            .cloned()
    }

    pub(super) fn load_extension(&self, key: &Identifier) -> Option<Vec<u8>> {
        self.state.lock().extensions.get(key).cloned()
    }

    pub(super) fn commit_transaction(
        &self,
        transaction: &PlayerStorageTransaction,
    ) -> io::Result<TransactionCommit> {
        let mut state = self.state.lock();
        for extension in &transaction.extensions {
            if state.extensions.get(&extension.key) != extension.expected.as_ref() {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "gameplay document changed",
                ));
            }
        }
        for player in &transaction.players {
            state
                .domain_players
                .entry(player.domain.clone())
                .or_default()
                .insert(player.uuid, player.data.clone());
        }
        for extension in &transaction.extensions {
            state
                .extensions
                .insert(extension.key.clone(), extension.replacement.clone());
        }
        Ok(TransactionCommit::Committed)
    }

    pub(crate) fn save_global(&self, uuid: Uuid, data: &GlobalPlayerData) {
        self.state.lock().global_players.insert(uuid, data.clone());
    }

    pub(crate) fn load_global(&self, uuid: Uuid) -> Option<GlobalPlayerData> {
        self.state.lock().global_players.get(&uuid).cloned()
    }

    pub(crate) fn save_permission_subjects(&self, subjects: &PermissionSubjectIndex) {
        self.state.lock().permission_subjects = subjects.clone();
    }

    pub(crate) fn load_permission_subjects(&self) -> PermissionSubjectIndex {
        self.state.lock().permission_subjects.clone()
    }

    /// Mirrors the file backend: the publication lock is held across `is_current`
    /// so a stale snapshot cannot overwrite a newer one.
    pub(crate) fn save_known_players_if_current(
        &self,
        players: &KnownPlayers,
        is_current: impl FnOnce() -> bool + Send,
    ) -> bool {
        let mut state = self.state.lock();
        if !is_current() {
            return false;
        }
        state.known_players = players.clone();
        true
    }

    pub(crate) fn load_known_players(&self) -> KnownPlayers {
        self.state.lock().known_players.clone()
    }
}
