//! In-memory per-player Solo Leveling session state.
//!
//! Holds the live [`HunterStats`] for each online player (loaded once at
//! join, mutated in memory, and persisted by [`crate::hooks::SoloLevelingHooks`]
//! on changes and disconnect) plus the bookkeeping needed to throttle
//! client-facing UI updates such as the sidebar.

use glam::{DVec3, Vec3Swizzles};
use rustc_hash::FxHashMap;
use steel_core::player::PlayerInput;
use steel_utils::locks::SyncRwLock;
use uuid::Uuid;

use crate::abilities::{AbilityCooldowns, ActiveAbilities};
use crate::economy::{PaymentError, SystemPoints, Won};
use crate::organization::OrganizationBook;
use crate::player_data::{PaymentDurability, PlayerDataStorage, SoloLevelingPlayerData};
use crate::quest::{PENALTY_DELAY_SECONDS, SystemQuest};
use crate::shadow::ShadowRoster;
use crate::shop::ShopDay;
use crate::sidebar::SidebarView;
use crate::stats::HunterStats;

/// How many ticks to wait between sidebar refreshes for a given player.
///
/// 20 ticks is once per second; hunter stats change rarely enough (level
/// up, stat allocation) that this keeps the sidebar responsive without
/// sending redundant packets every tick.
pub const SIDEBAR_UPDATE_INTERVAL_TICKS: u64 = 20;

/// Live session state tracked for one online player.
#[derive(Debug, Clone)]
pub struct HunterSession {
    /// The player's current hunter stats, kept in sync with the persisted
    /// copy by [`crate::hooks::SoloLevelingHooks`].
    pub stats: HunterStats,
    /// Current balance in won.
    pub won: Won,
    /// Personal shop currency, independent of hunter and shadow stat points.
    pub system_points: SystemPoints,
    /// A financial commit owns this balance until its I/O result is installed.
    pub(crate) economic_busy: bool,
    /// Persisted system-quest progress.
    pub quest: SystemQuest,
    /// Persistent Shadow Monarch roster.
    pub shadows: ShadowRoster,
    /// Daily personal-shop purchases.
    pub shop: ShopDay,
    /// Personal Adventurer Organization contracts.
    pub organization: OrganizationBook,
    /// Artifact ability cooldown deadlines.
    pub cooldowns: AbilityCooldowns,
    /// Running effects of triggered artifact abilities.
    pub abilities: ActiveAbilities,
    /// Unix second from which an owed penalty zone may start; `None` while
    /// nothing is owed.
    penalty_ready_at: Option<u64>,
    /// Ticks elapsed since the sidebar was last refreshed for this player.
    ticks_since_sidebar_update: u64,
    /// Values last sent to the sidebar, so unchanged state is not re-sent.
    last_sidebar: Option<SidebarView>,
    /// Latest observed player position, used to measure horizontal travel.
    last_quest_position: Option<DVec3>,
    /// Last jump input, used to count presses rather than ticks spent holding it.
    last_jump_pressed: bool,
    /// Last sneak input, used to count presses rather than ticks spent holding it.
    last_sneak_pressed: bool,
}

impl HunterSession {
    /// Creates a new session for a player who just joined.
    #[must_use]
    pub const fn new(
        stats: HunterStats,
        won: Won,
        quest: SystemQuest,
        shadows: ShadowRoster,
    ) -> Self {
        Self {
            stats,
            won,
            system_points: SystemPoints::zero(),
            economic_busy: false,
            quest,
            shadows,
            shop: ShopDay::unset(),
            organization: OrganizationBook::new(),
            cooldowns: AbilityCooldowns::new(),
            abilities: ActiveAbilities::new(),
            penalty_ready_at: None,
            ticks_since_sidebar_update: 0,
            last_sidebar: None,
            last_quest_position: None,
            last_jump_pressed: false,
            last_sneak_pressed: false,
        }
    }

    /// Captures the persistent part of a live session.
    #[must_use]
    pub fn saved_data(&self, uuid: Uuid) -> SoloLevelingPlayerData {
        SoloLevelingPlayerData {
            uuid,
            stats: self.stats,
            won: self.won,
            system_points: self.system_points,
            quest: self.quest,
            shadows: self.shadows.clone(),
            shop: self.shop,
            organization: self.organization.clone(),
            cooldowns: self.cooldowns.clone(),
        }
    }

    /// Advances the per-tick throttle counter and reports whether a sidebar
    /// refresh is due, and if so, resets the counter.
    ///
    /// Does not by itself decide whether the sidebar content actually
    /// changed; see [`HunterSession::sidebar_changed`].
    pub const fn tick_sidebar_throttle(&mut self) -> bool {
        self.ticks_since_sidebar_update += 1;
        if self.ticks_since_sidebar_update >= SIDEBAR_UPDATE_INTERVAL_TICKS {
            self.ticks_since_sidebar_update = 0;
            true
        } else {
            false
        }
    }

    /// Records `view` as shown and reports what was shown before it.
    pub fn sidebar_changed(&mut self, view: &SidebarView) -> SidebarChange {
        if self.last_sidebar.as_ref() == Some(view) {
            return SidebarChange::Unchanged;
        }
        SidebarChange::Changed {
            previous: self.last_sidebar.replace(view.clone()),
        }
    }

    /// Rolls the quest into today when the hunter joins.
    ///
    /// A penalty owed from an earlier day waits [`PENALTY_DELAY_SECONDS`] after
    /// every login, so reconnecting restarts the countdown.
    pub const fn start_day(&mut self, day: u64, now_seconds: u64) {
        self.quest.advance_window(day);
        self.penalty_ready_at = if self.quest.penalty_pending {
            Some(now_seconds.saturating_add(PENALTY_DELAY_SECONDS))
        } else {
            None
        };
    }

    /// Returns whether an owed penalty zone should start at `now_seconds`.
    #[must_use]
    pub fn penalty_due(&self, now_seconds: u64) -> bool {
        self.quest.penalty_pending
            && self
                .penalty_ready_at
                .is_none_or(|ready_at| ready_at <= now_seconds)
    }

    /// Takes an owed penalty zone exactly once.
    pub const fn claim_penalty(&mut self) -> bool {
        if !self.quest.penalty_pending {
            return false;
        }
        self.quest.penalty_pending = false;
        self.penalty_ready_at = None;
        true
    }

    /// Observes vanilla client input and position for this player's active
    /// system quest.
    ///
    /// One held jump or sneak key counts once until released. Movement is
    /// horizontal and capped per tick so teleports cannot complete the quest.
    pub fn observe_quest(
        &mut self,
        window: u64,
        position: DVec3,
        input: PlayerInput,
    ) -> QuestUpdate {
        if self.quest.window == u64::MAX {
            self.quest = SystemQuest::new(window);
        }
        let penalty_pending = self.quest.advance_window(window);
        if penalty_pending {
            // The day ended while the hunter was online: the penalty is immediate.
            self.penalty_ready_at = Some(0);
        }
        let Some(previous_position) = self.last_quest_position.replace(position) else {
            self.last_jump_pressed = input.jump();
            self.last_sneak_pressed = input.shift();
            return QuestUpdate {
                penalty_pending,
                ..QuestUpdate::default()
            };
        };

        if !self.quest.completed {
            if input.jump() && !self.last_jump_pressed {
                self.quest.record_jump();
            }
            if input.shift() && !self.last_sneak_pressed {
                self.quest.record_sneak();
            }

            let horizontal_distance = (position.xz() - previous_position.xz()).length().min(5.0);
            self.quest.record_distance(horizontal_distance);
            if self.quest.tasks_complete() {
                self.quest.completed = true;
                return QuestUpdate {
                    completed: true,
                    penalty_pending,
                };
            }
        }

        self.last_jump_pressed = input.jump();
        self.last_sneak_pressed = input.shift();
        QuestUpdate {
            penalty_pending,
            ..QuestUpdate::default()
        }
    }
}

/// Whether the sidebar has to be rewritten.
#[derive(Debug, Clone, PartialEq)]
pub enum SidebarChange {
    /// The client already shows this view.
    Unchanged,
    /// The view differs from what the client shows.
    Changed {
        /// What was shown before, if anything.
        previous: Option<SidebarView>,
    },
}

/// Observable changes produced by one system-quest update.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct QuestUpdate {
    /// The player completed the active quest and received its reward.
    pub completed: bool,
    /// An expired unfinished quest created a pending penalty-zone challenge.
    pub penalty_pending: bool,
}

/// Registry of live [`HunterSession`]s for all online players, keyed by
/// player `Uuid`.
#[derive(Debug, Default)]
pub struct SessionRegistry {
    sessions: SyncRwLock<FxHashMap<Uuid, HunterSession>>,
}

impl SessionRegistry {
    /// Lists live and retiring accounts for the shutdown persistence fence.
    pub fn player_ids(&self) -> Vec<Uuid> {
        self.sessions.read().keys().copied().collect()
    }

    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sessions: SyncRwLock::new(FxHashMap::default()),
        }
    }

    /// Inserts a session for a player who just joined, replacing any stale
    /// entry left over from an unclean disconnect.
    pub fn insert(&self, uuid: Uuid, session: HunterSession) {
        self.sessions.write().insert(uuid, session);
    }

    /// Removes a player's session, returning it if present.
    pub fn remove(&self, uuid: Uuid) -> Option<HunterSession> {
        self.sessions.write().remove(&uuid)
    }

    /// Runs `f` with mutable access to a player's session, if they have one.
    ///
    /// Returns `None` if the player has no session (not yet joined, or
    /// already disconnected).
    pub fn with_mut<R>(&self, uuid: Uuid, f: impl FnOnce(&mut HunterSession) -> R) -> Option<R> {
        self.sessions.write().get_mut(&uuid).map(f)
    }

    /// Commits a payment while both live balances are protected by one lock.
    pub fn pay(
        &self,
        storage: &PlayerDataStorage,
        sender: Uuid,
        recipient: Uuid,
        amount: u64,
    ) -> Result<(), PaymentError> {
        if sender == recipient {
            return Err(PaymentError::SelfPayment);
        }
        let mut sessions = self.sessions.write();
        let [Some(sender_session), Some(recipient_session)] =
            sessions.get_disjoint_mut([&sender, &recipient])
        else {
            return Err(PaymentError::Unavailable);
        };
        let (sender_balance, recipient_balance) =
            sender_session.won.payment(recipient_session.won, amount)?;
        let mut players = [
            sender_session.saved_data(sender),
            recipient_session.saved_data(recipient),
        ];
        players[0].won = sender_balance;
        players[1].won = recipient_balance;
        let durability = storage.save_payment(&players).map_err(|error| {
            log::error!("sololeveling: failed to persist payment: {error}");
            PaymentError::Storage
        })?;
        sender_session.won = sender_balance;
        recipient_session.won = recipient_balance;
        match durability {
            PaymentDurability::Confirmed => Ok(()),
            PaymentDurability::Uncertain => Err(PaymentError::CommitUncertain),
        }
    }

    /// Restores every destroyed roster shadow and returns owners whose save data changed.
    pub fn recover_shadows(&self, health: u16) -> Vec<Uuid> {
        self.sessions
            .write()
            .iter_mut()
            .filter_map(|(owner, session)| session.shadows.recover(health).then_some(*owner))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::{env, fs};

    use glam::DVec3;
    use steel_core::player::PlayerInput;

    use super::*;
    use crate::quest::{REQUIRED_DISTANCE_BLOCKS, REQUIRED_JUMPS, REQUIRED_SNEAKS};
    use crate::sidebar::SidebarView;

    #[test]
    fn sidebar_throttle_fires_once_per_interval() {
        let mut session = HunterSession::new(
            HunterStats::new(),
            Won::default(),
            SystemQuest::default(),
            ShadowRoster::default(),
        );
        for _ in 1..SIDEBAR_UPDATE_INTERVAL_TICKS {
            assert!(!session.tick_sidebar_throttle());
        }
        assert!(session.tick_sidebar_throttle());
        assert!(!session.tick_sidebar_throttle());
    }

    #[test]
    fn sidebar_change_detection_only_fires_once_per_change() {
        let mut session = HunterSession::new(
            HunterStats::new(),
            Won::default(),
            SystemQuest::default(),
            ShadowRoster::default(),
        );
        let mut view = SidebarView {
            stats: session.stats,
            won: session.won,
            system_points: session.system_points,
            quest: session.quest,
            contracts: Vec::new(),
        };
        assert_eq!(
            session.sidebar_changed(&view),
            SidebarChange::Changed { previous: None }
        );
        assert_eq!(session.sidebar_changed(&view), SidebarChange::Unchanged);

        let shown = view.clone();
        view.stats.level = 2;
        assert_eq!(
            session.sidebar_changed(&view),
            SidebarChange::Changed {
                previous: Some(shown)
            }
        );
        assert_eq!(session.sidebar_changed(&view), SidebarChange::Unchanged);
    }

    #[test]
    fn registry_round_trips_insert_and_remove() {
        let registry = SessionRegistry::new();
        let uuid = Uuid::new_v4();
        registry.insert(
            uuid,
            HunterSession::new(
                HunterStats::new(),
                Won::default(),
                SystemQuest::default(),
                ShadowRoster::default(),
            ),
        );

        assert!(registry.with_mut(uuid, |_| ()).is_some());
        assert!(registry.remove(uuid).is_some());
        assert!(registry.with_mut(uuid, |_| ()).is_none());
    }

    #[test]
    fn payment_updates_both_live_and_saved_accounts_or_neither() {
        let dir = env::temp_dir().join(format!("sololeveling-pay-test-{}", Uuid::new_v4()));
        let storage = PlayerDataStorage::new(dir.clone());
        storage.init().expect("init");
        let registry = SessionRegistry::new();
        let sender = Uuid::new_v4();
        let recipient = Uuid::new_v4();
        for owner in [sender, recipient] {
            let mut session = HunterSession::new(
                HunterStats::new(),
                Won::zero(),
                SystemQuest::default(),
                ShadowRoster::default(),
            );
            if owner == sender {
                session.won.credit(100);
            }
            registry.insert(owner, session);
        }
        registry
            .pay(&storage, sender, recipient, 40)
            .expect("payment");
        assert_eq!(
            registry.with_mut(sender, |session| session.won.amount()),
            Some(60)
        );
        assert_eq!(
            registry.with_mut(recipient, |session| session.won.amount()),
            Some(40)
        );
        assert_eq!(
            storage
                .load(sender)
                .expect("load")
                .expect("sender")
                .won
                .amount(),
            60
        );
        assert_eq!(
            storage
                .load(recipient)
                .expect("load")
                .expect("recipient")
                .won
                .amount(),
            40
        );

        assert_eq!(
            registry.pay(&storage, sender, recipient, 61),
            Err(PaymentError::InsufficientFunds)
        );
        assert_eq!(
            registry.pay(&storage, sender, sender, 1),
            Err(PaymentError::SelfPayment)
        );
        registry.remove(recipient);
        assert_eq!(
            registry.pay(&storage, sender, recipient, 1),
            Err(PaymentError::Unavailable)
        );
        assert_eq!(
            registry.with_mut(sender, |session| session.won.amount()),
            Some(60)
        );
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn rejected_persistence_does_not_debit_either_live_account() {
        let dir = env::temp_dir().join(format!("sololeveling-pay-failure-{}", Uuid::new_v4()));
        let storage = PlayerDataStorage::new(dir.clone());
        storage.init().expect("init");
        let registry = SessionRegistry::new();
        let sender = Uuid::new_v4();
        let recipient = Uuid::new_v4();
        for owner in [sender, recipient] {
            let mut session = HunterSession::new(
                HunterStats::new(),
                Won::zero(),
                SystemQuest::default(),
                ShadowRoster::default(),
            );
            session.won.credit(100);
            registry.insert(owner, session);
        }
        fs::create_dir(dir.join("pending-payment.json.tmp")).expect("block record creation");
        assert_eq!(
            registry.pay(&storage, sender, recipient, 40),
            Err(PaymentError::Storage)
        );
        for owner in [sender, recipient] {
            assert_eq!(
                registry.with_mut(owner, |session| session.won.amount()),
                Some(100)
            );
            assert!(storage.load(owner).expect("load").is_none());
        }
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn quest_observation_counts_input_edges_and_grants_the_completion_reward_once() {
        let mut session = HunterSession::new(
            HunterStats::new(),
            Won::default(),
            SystemQuest::new(1),
            ShadowRoster::default(),
        );
        let empty = PlayerInput::EMPTY;
        let _ = session.observe_quest(1, DVec3::ZERO, empty);
        session.quest.jumps = REQUIRED_JUMPS - 1;
        session.quest.sneaks = REQUIRED_SNEAKS - 1;
        session.quest.distance_blocks = REQUIRED_DISTANCE_BLOCKS - 1.0;

        let update = session.observe_quest(
            1,
            DVec3::new(1.0, 0.0, 0.0),
            PlayerInput::from_flags(0x10 | 0x20),
        );

        assert!(update.completed);
        assert_eq!(
            session.stats.unspent_points, 0,
            "the reward is a chest, not stat points"
        );
        assert!(session.quest.completed);
    }

    #[test]
    fn a_penalty_from_an_earlier_day_waits_after_each_login() {
        let mut session = HunterSession::new(
            HunterStats::new(),
            Won::default(),
            SystemQuest::new(1),
            ShadowRoster::default(),
        );
        session.start_day(3, 1_000);
        assert!(session.quest.penalty_pending);
        assert!(!session.penalty_due(1_599));
        session.start_day(3, 1_500);
        assert!(!session.penalty_due(2_099));
        assert!(session.penalty_due(2_100));
        assert!(session.claim_penalty());
        assert!(!session.claim_penalty());
        assert!(!session.penalty_due(10_000));
    }

    #[test]
    fn a_day_ending_online_owes_the_penalty_at_once() {
        let mut session = HunterSession::new(
            HunterStats::new(),
            Won::default(),
            SystemQuest::new(1),
            ShadowRoster::default(),
        );
        session.start_day(1, 0);
        let update = session.observe_quest(2, DVec3::ZERO, PlayerInput::EMPTY);
        assert!(update.penalty_pending);
        assert!(session.penalty_due(0));
    }

    #[test]
    fn default_quest_initializes_without_creating_a_penalty() {
        let mut session = HunterSession::new(
            HunterStats::new(),
            Won::default(),
            SystemQuest::default(),
            ShadowRoster::default(),
        );

        let update = session.observe_quest(42, DVec3::ZERO, PlayerInput::EMPTY);

        assert!(!update.penalty_pending);
        assert_eq!(session.quest.window, 42);
    }
}
