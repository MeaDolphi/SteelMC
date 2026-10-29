//! Serialized gameplay persistence, using the core transaction coordinator.
//!
//! Ordinary saves capture live state when dequeued, not when requested, so a
//! queued snapshot cannot undo a payment that completed in the meantime.

mod account;
use account::load_account;
pub(crate) use account::{AccountUpdate, CommitReport, PreparedCommit};

use std::{
    collections::VecDeque,
    io,
    sync::{Arc, Weak},
};

use rustc_hash::{FxHashMap, FxHashSet};
use steel_core::{player::Player, player::connection::NetworkConnection as _, server::Server};
use text_components::TextComponent;
use tokio::sync::oneshot;
use uuid::Uuid;

use super::SoloLevelingState;
use super::auction::AuctionRequest;
use crate::{
    combat_reward::HunterReward, economy::PaymentError, player_data::SoloLevelingPlayerData,
    rank::HunterRank,
};

#[derive(Debug)]
enum Request {
    Load {
        uuid: Uuid,
        reply: oneshot::Sender<io::Result<SoloLevelingPlayerData>>,
    },
    Save {
        uuid: Uuid,
        retire: bool,
    },
    Pay {
        sender: Weak<Player>,
        recipient: Weak<Player>,
        amount: u64,
    },
    Credit {
        uuid: Uuid,
        amount: u64,
    },
    Award {
        rewards: Vec<(Uuid, HunterReward)>,
    },
    Auction(AuctionRequest),
    AuctionDeliver {
        hunter: Uuid,
    },
}

#[derive(Debug, Default)]
pub(super) struct Persistence {
    requests: VecDeque<Request>,
    saves: FxHashSet<Uuid>,
    expected: FxHashMap<Uuid, Option<Vec<u8>>>,
    prepared: FxHashMap<Uuid, SoloLevelingPlayerData>,
    failed: FxHashSet<Uuid>,
    busy: bool,
    quiescing: bool,
}

impl Persistence {
    /// Returns whether a new write for `uuid` may still be accepted.
    ///
    /// A refused account stays refused for the rest of the process, and a
    /// quiescing store is draining the shutdown fence.
    pub fn accepts_writes(&self, uuid: Uuid) -> bool {
        !self.quiescing && !self.failed.contains(&uuid)
    }

    /// Returns whether auction maintenance is already waiting in this lane.
    pub fn has_auction_work(&self) -> bool {
        self.requests.iter().any(|request| {
            matches!(
                request,
                Request::Auction(_) | Request::AuctionDeliver { .. }
            )
        })
    }
}

impl SoloLevelingState {
    /// Waits off-tick for the account load ordered after earlier saves/payments.
    pub async fn prepare_join(self: Arc<Self>, uuid: Uuid) -> Result<(), String> {
        let (reply, receiver) = oneshot::channel();
        {
            let mut persistence = self.persistence.lock();
            if persistence.quiescing || persistence.failed.contains(&uuid) {
                return Err("Gameplay storage is unavailable".to_owned());
            }
            persistence
                .requests
                .push_back(Request::Load { uuid, reply });
        }
        let data = receiver
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        self.persistence.lock().prepared.insert(uuid, data);
        Ok(())
    }

    /// Transfers prepared data to the player's tick-owned admission hook.
    pub fn take_prepared_player(&self, uuid: Uuid) -> Option<SoloLevelingPlayerData> {
        self.persistence.lock().prepared.remove(&uuid)
    }

    /// Queues a current-state save; this is not a durable acknowledgement.
    pub fn save_session(&self, uuid: Uuid) -> io::Result<()> {
        let mut persistence = self.persistence.lock();
        if persistence.failed.contains(&uuid) {
            return Err(io::Error::other("gameplay account persistence failed"));
        }
        if persistence.saves.insert(uuid) {
            persistence.requests.push_back(Request::Save {
                uuid,
                retire: false,
            });
        }
        Ok(())
    }

    /// Saves and removes a disconnected session after earlier financial work.
    pub fn retire_session(&self, uuid: Uuid) {
        self.persistence
            .lock()
            .requests
            .push_back(Request::Save { uuid, retire: true });
    }

    /// Queues a payment; only its completion sends a durable success message.
    pub fn request_payment(
        &self,
        sender: &Arc<Player>,
        recipient: &Arc<Player>,
        amount: u64,
    ) -> Result<(), PaymentError> {
        let from = sender.gameprofile.id();
        let to = recipient.gameprofile.id();
        if from == to {
            return Err(PaymentError::SelfPayment);
        }
        let from_balance = self
            .sessions
            .with_mut(from, |session| session.won)
            .ok_or(PaymentError::Unavailable)?;
        let to_balance = self
            .sessions
            .with_mut(to, |session| session.won)
            .ok_or(PaymentError::Unavailable)?;
        from_balance.payment(to_balance, amount)?;
        let mut persistence = self.persistence.lock();
        if persistence.failed.contains(&from)
            || persistence.failed.contains(&to)
            || persistence.quiescing
        {
            return Err(PaymentError::Storage);
        }
        persistence.requests.push_back(Request::Pay {
            sender: Arc::downgrade(sender),
            recipient: Arc::downgrade(recipient),
            amount,
        });
        Ok(())
    }

    /// Reward sources enqueue Won mutations in the same lane as payments.
    pub fn credit_won(&self, uuid: Uuid, amount: u64) {
        self.persistence
            .lock()
            .requests
            .push_back(Request::Credit { uuid, amount });
    }

    /// Awards only the qualifying killer/owner, never the rest of the party.
    pub fn credit_dungeon_mob(&self, uuid: Uuid, rank: HunterRank) {
        self.persistence.lock().requests.push_back(Request::Award {
            rewards: vec![(uuid, HunterReward::for_mob(rank))],
        });
    }

    /// Queues one auction operation on the serialized persistence lane.
    ///
    /// Auction commits write the lot ledger together with the accounts they
    /// charge, so they must not interleave with any other account write.
    pub fn request_auction(&self, request: AuctionRequest) {
        self.persistence
            .lock()
            .requests
            .push_back(Request::Auction(request));
    }

    /// Queues the delivery of everything the auction house owes one hunter.
    pub fn request_auction_delivery(&self, hunter: Uuid) {
        self.persistence
            .lock()
            .requests
            .push_back(Request::AuctionDeliver { hunter });
    }

    /// Returns whether auction maintenance is already queued.
    pub fn auction_maintenance_queued(&self) -> bool {
        self.persistence.lock().has_auction_work()
    }

    /// Releases the lane after a commit that another module started.
    ///
    /// The lane is taken when a request is dequeued; a commit submitted outside
    /// [`SoloLevelingState::submit_commit`] must return it once it is resolved.
    pub(super) fn release_persistence_lane(&self) {
        self.persistence.lock().busy = false;
    }

    /// Settles the shared boss pool only for the captured living finishers.
    pub fn credit_dungeon_boss(&self, survivors: Vec<Uuid>, rank: HunterRank) {
        let rewards = HunterReward::boss_shares(rank, survivors.len());
        if rewards.is_empty() {
            return;
        }
        self.persistence.lock().requests.push_back(Request::Award {
            rewards: survivors.into_iter().zip(rewards).collect(),
        });
    }

    /// Starts at most one I/O operation without awaiting it on the game tick.
    ///
    /// # Panics
    ///
    /// Panics when the account load job fails, which means the storage layer is
    /// unusable and the server cannot keep serving gameplay state.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per persistence request keeps the single I/O lane legible"
    )]
    pub fn tick_persistence(self: &Arc<Self>, server: &Arc<Server>) {
        let request = {
            let mut persistence = self.persistence.lock();
            if persistence.busy {
                return;
            }
            let Some(request) = persistence.requests.pop_front() else {
                return;
            };
            persistence.busy = true;
            if let Request::Save { uuid, .. } = &request {
                persistence.saves.remove(uuid);
            }
            request
        };
        match request {
            Request::Load { uuid, reply } => {
                let state = Arc::clone(self);
                let server_task = Arc::clone(server);
                let legacy = self.storage.clone();
                server.jobs.spawn_io(
                    async move { load_account(&server_task.player_data_storage, legacy, uuid).await },
                    move |result| {
                        let loaded = match result {
                            Ok(result) => result,
                            Err(error) => panic!("gameplay account load task failed: {error}"),
                        };
                        let mut persistence = state.persistence.lock();
                        persistence.busy = false;
                        match loaded {
                            Ok(loaded) => {
                                persistence.expected.insert(uuid, loaded.bytes);
                                if reply.send(Ok(loaded.data)).is_err() {
                                    persistence.expected.remove(&uuid);
                                }
                            }
                            Err(error) => {
                                let _ = reply.send(Err(error));
                            }
                        }
                    },
                );
            }
            Request::Save { uuid, retire } => {
                let snapshot = self
                    .sessions
                    .with_mut(uuid, |session| session.saved_data(uuid));
                let Some(snapshot) = snapshot else {
                    self.persistence.lock().busy = false;
                    return;
                };
                self.submit_commit(
                    server,
                    PreparedCommit::prepare(self, vec![snapshot], Vec::new()),
                    move |state, report| {
                        if report.confirmed && retire {
                            state.sessions.remove(uuid);
                            state.persistence.lock().expected.remove(&uuid);
                        }
                    },
                );
            }
            Request::Pay {
                sender,
                recipient,
                amount,
            } => {
                let Some(sender) = sender.upgrade() else {
                    self.persistence.lock().busy = false;
                    return;
                };
                let Some(recipient) = recipient.upgrade() else {
                    self.persistence.lock().busy = false;
                    return;
                };
                if sender.connection.closed() || recipient.connection.closed() {
                    self.persistence.lock().busy = false;
                    sender.send_message(&TextComponent::plain(
                        "Перевод отменён: получатель или отправитель вышел.",
                    ));
                    return;
                }
                let from = sender.gameprofile.id();
                let to = recipient.gameprofile.id();
                let snapshots = self
                    .sessions
                    .with_mut(from, |session| session.saved_data(from))
                    .zip(self.sessions.with_mut(to, |session| session.saved_data(to)));
                let Some((mut from_data, mut to_data)) = snapshots else {
                    self.persistence.lock().busy = false;
                    return;
                };
                let balances = from_data.won.payment(to_data.won, amount);
                let Ok((from_balance, to_balance)) = balances else {
                    self.persistence.lock().busy = false;
                    sender.send_message(&TextComponent::plain(
                        "Перевод отменён: недостаточно средств или переполнение баланса.",
                    ));
                    return;
                };
                from_data.won = from_balance;
                to_data.won = to_balance;
                self.submit_commit(server, PreparedCommit::prepare(self, vec![from_data, to_data], vec![
                    AccountUpdate::Won { uuid: from, balance: from_balance },
                    AccountUpdate::Won { uuid: to, balance: to_balance },
                ]), move |_, report| {
                    if report.confirmed {
                        sender.send_message(&TextComponent::plain(format!("Передано {amount} Won игроку {}.", recipient.gameprofile.name())));
                        recipient.send_message(&TextComponent::plain(format!("Получено {amount} Won от {}.", sender.gameprofile.name())));
                    } else {
                        sender.send_message(&TextComponent::plain("Перевод не подтверждён. Не повторяйте его до проверки администратором."));
                    }
                });
            }
            Request::Credit { uuid, amount } => {
                let Some(mut snapshot) = self
                    .sessions
                    .with_mut(uuid, |session| session.saved_data(uuid))
                else {
                    self.persistence.lock().busy = false;
                    return;
                };
                if !snapshot.won.try_credit(amount) {
                    self.persistence.lock().busy = false;
                    log::error!("sololeveling: Won reward overflows account {uuid}");
                    return;
                }
                let balance = snapshot.won;
                self.submit_commit(
                    server,
                    PreparedCommit::prepare(
                        self,
                        vec![snapshot],
                        vec![AccountUpdate::Won { uuid, balance }],
                    ),
                    |_, _| {},
                );
            }
            Request::Award { rewards } => {
                self.submit_commit(server, PreparedCommit::rewards(self, rewards), |_, _| {});
            }
            Request::Auction(request) => {
                if !self.begin_auction(server, request) {
                    self.persistence.lock().busy = false;
                }
            }
            Request::AuctionDeliver { hunter } => {
                if !self.begin_auction_delivery(server, hunter) {
                    self.persistence.lock().busy = false;
                }
            }
        }
    }

    /// Starts one transaction without awaiting it on the game tick.
    pub(super) fn submit_commit(
        self: &Arc<Self>,
        server: &Arc<Server>,
        prepared: io::Result<PreparedCommit>,
        completion: impl FnOnce(&Self, CommitReport) + Send + 'static,
    ) {
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.persistence.lock().busy = false;
                log::error!("sololeveling: could not prepare account write: {error}");
                completion(
                    self,
                    CommitReport {
                        committed: false,
                        confirmed: false,
                    },
                );
                return;
            }
        };
        let state = Arc::clone(self);
        let server_task = Arc::clone(server);
        server.jobs.spawn_io(
            async move {
                let result = server_task
                    .player_data_storage
                    .commit_transaction(&prepared.transaction)
                    .await;
                (prepared, result)
            },
            move |result| {
                let (prepared, result) = match result {
                    Ok(result) => result,
                    Err(error) => panic!("gameplay account commit task failed: {error}"),
                };
                let report = prepared.complete(&state, &result);
                completion(&state, report);
            },
        );
    }

    /// Drains queued writes off-tick, rejecting new admissions during shutdown.
    pub async fn flush_persistence(self: Arc<Self>, server: Arc<Server>) -> Result<(), String> {
        self.persistence.lock().quiescing = true;
        for uuid in self.sessions.player_ids() {
            self.save_session(uuid).map_err(|error| error.to_string())?;
        }
        loop {
            server.jobs.finish_all().await;
            let empty = {
                let persistence = self.persistence.lock();
                if !persistence.failed.is_empty() {
                    return Err(format!(
                        "Gameplay accounts with failed writes: {:?}",
                        persistence.failed
                    ));
                }
                persistence.requests.is_empty() && !persistence.busy
            };
            if empty {
                return Ok(());
            }
            self.tick_persistence(&server);
        }
    }
}
