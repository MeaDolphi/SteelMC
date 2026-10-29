//! Account after-images and application of committed deltas to live sessions.

use std::io;

use rustc_hash::FxHashSet;
use steel_core::player::player_data_storage::{
    PlayerDataStorage as CoreStorage,
    transaction::{ExtensionStorageWrite, PlayerStorageTransaction, TransactionCommit},
};
use steel_utils::Identifier;
use tokio::task;
use uuid::Uuid;

use crate::{
    combat_reward::HunterReward,
    economy::{SystemPoints, Won},
    organization::OrganizationBook,
    player_data::{PlayerDataStorage, SoloLevelingPlayerData},
    state::SoloLevelingState,
};

pub(super) fn key(uuid: Uuid) -> Identifier {
    Identifier::new("sololeveling", format!("players/{uuid}"))
}

pub(super) struct Loaded {
    pub data: SoloLevelingPlayerData,
    pub bytes: Option<Vec<u8>>,
}

/// What one commit of [`PreparedCommit`] actually achieved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CommitReport {
    /// The redo record was published, so the after-images must be installed.
    pub committed: bool,
    /// The record's directory entry is durably synchronized.
    pub confirmed: bool,
}

/// Player accounts and gameplay documents published as one transaction.
///
/// Account extension writes come first, in `owners` order, and gameplay
/// documents follow them, because installation relies on that order.
pub(crate) struct PreparedCommit {
    owners: Vec<Uuid>,
    pub transaction: PlayerStorageTransaction,
    updates: Vec<AccountUpdate>,
}

pub(crate) enum AccountUpdate {
    Won {
        uuid: Uuid,
        balance: Won,
    },
    Progress {
        uuid: Uuid,
        reward: HunterReward,
        points: SystemPoints,
    },
    Organization {
        uuid: Uuid,
        book: OrganizationBook,
    },
}

impl AccountUpdate {
    const fn owner(&self) -> Uuid {
        match self {
            Self::Won { uuid, .. }
            | Self::Progress { uuid, .. }
            | Self::Organization { uuid, .. } => *uuid,
        }
    }

    fn apply(self, state: &SoloLevelingState, committed: bool) {
        let uuid = self.owner();
        let progress = state.sessions.with_mut(uuid, |session| {
            session.economic_busy = false;
            if !committed {
                return None;
            }
            match self {
                Self::Won { balance, .. } => {
                    session.won = balance;
                    None
                }
                Self::Progress { reward, points, .. } => {
                    let level_up = session.stats.grant_experience(reward.experience);
                    session.system_points = points;
                    Some((reward, level_up, session.stats.level))
                }
                Self::Organization { book, .. } => {
                    session.organization = book;
                    None
                }
            }
        });
        if let Some(Some((reward, level_up, level))) = progress {
            state.notify_progress(uuid, reward, level_up, level);
        }
    }
}

impl PreparedCommit {
    /// Builds the changes that reward a list of hunters.
    pub fn rewards(
        state: &SoloLevelingState,
        rewards: Vec<(Uuid, HunterReward)>,
    ) -> io::Result<Self> {
        let mut snapshots = Vec::with_capacity(rewards.len());
        let mut updates = Vec::with_capacity(rewards.len());
        for (uuid, reward) in rewards {
            let mut snapshot = state
                .sessions
                .with_mut(uuid, |session| session.saved_data(uuid))
                .ok_or_else(|| io::Error::other("reward recipient has no session"))?;
            if !snapshot.system_points.credit(reward.system_points) {
                return Err(io::Error::other("System reward overflows account"));
            }
            snapshot.stats.grant_experience(reward.experience);
            updates.push(AccountUpdate::Progress {
                uuid,
                reward,
                points: snapshot.system_points,
            });
            snapshots.push(snapshot);
        }
        Self::prepare(state, snapshots, updates)
    }

    /// Builds the changes for a batch of account after-images.
    pub fn prepare(
        state: &SoloLevelingState,
        snapshots: Vec<SoloLevelingPlayerData>,
        updates: Vec<AccountUpdate>,
    ) -> io::Result<Self> {
        let persistence = state.persistence.lock();
        let owners: Vec<_> = snapshots.iter().map(|data| data.uuid).collect();
        let distinct: FxHashSet<_> = owners.iter().copied().collect();
        if owners.is_empty() || distinct.len() != owners.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty or duplicate account batch",
            ));
        }
        let extensions = snapshots
            .into_iter()
            .map(|data| {
                if persistence.failed.contains(&data.uuid) {
                    return Err(io::Error::other("account persistence previously failed"));
                }
                let expected = persistence
                    .expected
                    .get(&data.uuid)
                    .cloned()
                    .ok_or_else(|| {
                        io::Error::other("account has not been loaded from core storage")
                    })?;
                Ok(ExtensionStorageWrite {
                    key: key(data.uuid),
                    expected,
                    replacement: serde_json::to_vec(&data).map_err(io::Error::other)?,
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        drop(persistence);
        for update in &updates {
            state
                .sessions
                .with_mut(update.owner(), |session| session.economic_busy = true);
        }
        Ok(Self {
            owners,
            transaction: PlayerStorageTransaction {
                players: Vec::new(),
                extensions,
            },
            updates,
        })
    }

    /// Builds a commit that only publishes one gameplay document.
    ///
    /// Used when a resolution moves no money immediately, for example when a
    /// lot expires with its seller offline.
    pub fn document(key: Identifier, expected: Option<Vec<u8>>, replacement: Vec<u8>) -> Self {
        Self {
            owners: Vec::new(),
            transaction: PlayerStorageTransaction {
                players: Vec::new(),
                extensions: vec![ExtensionStorageWrite {
                    key,
                    expected,
                    replacement,
                }],
            },
            updates: Vec::new(),
        }
    }

    /// Appends one gameplay document to the transaction.
    ///
    /// `expected` is the exact document bytes the caller last observed, so the
    /// commit refuses to overwrite a document somebody else replaced.
    pub fn with_document(
        mut self,
        key: Identifier,
        expected: Option<Vec<u8>>,
        replacement: Vec<u8>,
    ) -> Self {
        self.transaction.extensions.push(ExtensionStorageWrite {
            key,
            expected,
            replacement,
        });
        self
    }

    /// Installs the after-images and reports what the storage achieved.
    pub fn complete(
        self,
        state: &SoloLevelingState,
        result: &io::Result<TransactionCommit>,
    ) -> CommitReport {
        let committed = result.is_ok();
        let confirmed = matches!(result, Ok(TransactionCommit::Committed));
        {
            let mut persistence = state.persistence.lock();
            persistence.busy = false;
            if committed {
                // Gameplay documents carry their own concurrency token, so only
                // the account writes that precede them are recorded here.
                for (uuid, extension) in
                    self.owners.iter().copied().zip(self.transaction.extensions)
                {
                    persistence
                        .expected
                        .insert(uuid, Some(extension.replacement));
                }
            } else {
                persistence.failed.extend(self.owners);
            }
        }
        for update in self.updates {
            update.apply(state, committed);
        }
        if !confirmed {
            log::error!("sololeveling: gameplay persistence not confirmed: {result:?}");
        }
        CommitReport {
            committed,
            confirmed,
        }
    }
}

pub(super) async fn load_account(
    storage: &CoreStorage,
    legacy: PlayerDataStorage,
    uuid: Uuid,
) -> io::Result<Loaded> {
    if let Some(bytes) = storage.load_extension(&key(uuid)).await? {
        let data: SoloLevelingPlayerData =
            serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if data.uuid != uuid {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "gameplay account UUID mismatch",
            ));
        }
        return Ok(Loaded {
            data,
            bytes: Some(bytes),
        });
    }
    // Legacy JSON is a one-time import source, not a second live store.
    let data = task::spawn_blocking(move || legacy.load_or_create(uuid))
        .await
        .map_err(io::Error::other)??;
    if data.uuid != uuid {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "legacy account UUID mismatch",
        ));
    }
    // Admission can fail. Publish only after join, not for abandoned connections.
    Ok(Loaded { data, bytes: None })
}

#[cfg(test)]
mod tests;
