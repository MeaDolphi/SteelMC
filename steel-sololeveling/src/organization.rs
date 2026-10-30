//! Adventurer Organization contracts: templates, progress and payment.
//!
//! A contract counts only what happens after it was accepted. Personal
//! contracts live in the hunter's saved data; group contracts live on the
//! party that took them and share its in-memory lifetime; guild contracts
//! are saved with the guild.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::rank::HunterRank;

/// Active personal contracts one hunter may hold.
pub const PERSONAL_LIMIT: usize = 3;
/// Active group contracts one party may hold.
pub const GROUP_LIMIT: usize = 2;
/// Active guild contracts one guild may hold.
pub const GUILD_LIMIT: usize = 2;
/// How many times a guild contract multiplies a template's target and pay.
pub const GUILD_SCALE: u32 = 5;

/// What a contract asks the hunter to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Objective {
    /// Kill any mobs.
    Mobs,
    /// Kill zombies, the class the Organization tracks.
    Zombies,
    /// Defeat a dungeon boss.
    Boss,
    /// Clear one dungeon of exactly the contract rank.
    RankedDungeon,
    /// Clear dungeons of any rank.
    Dungeons,
}

/// Every objective, in board order.
pub const OBJECTIVES: [Objective; 5] = [
    Objective::Mobs,
    Objective::Zombies,
    Objective::Boss,
    Objective::RankedDungeon,
    Objective::Dungeons,
];

/// Every rank, in board order.
pub const RANKS: [HunterRank; 6] = [
    HunterRank::E,
    HunterRank::D,
    HunterRank::C,
    HunterRank::B,
    HunterRank::A,
    HunterRank::S,
];

impl Objective {
    /// Returns how many events finish the contract.
    #[must_use]
    pub const fn target(self) -> u32 {
        match self {
            Self::Mobs => 20,
            Self::Zombies => 10,
            Self::Boss | Self::RankedDungeon => 1,
            Self::Dungeons => 3,
        }
    }

    /// Returns the E-rank payment in Won.
    #[must_use]
    pub const fn base_payment(self) -> u64 {
        match self {
            Self::Mobs | Self::Zombies => 100,
            Self::Boss => 150,
            Self::RankedDungeon => 200,
            Self::Dungeons => 500,
        }
    }
}

/// One repeatable contract offered on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    /// What has to be done.
    pub objective: Objective,
    /// Rank fixed when the contract is accepted.
    #[serde(with = "rank_letter")]
    pub rank: HunterRank,
}

impl Template {
    /// Returns the payment one hunter receives for finishing this contract.
    #[must_use]
    pub const fn payment(self) -> u64 {
        self.objective.base_payment() * rank_multiplier(self.rank)
    }

    /// Returns a short name that fits one sidebar line.
    #[must_use]
    pub fn short_title(self) -> String {
        let rank = self.rank.name();
        match self.objective {
            Objective::Mobs => format!("[{rank}] Мобы"),
            Objective::Zombies => format!("[{rank}] Зомби"),
            Objective::Boss => format!("[{rank}] Босс"),
            Objective::RankedDungeon => format!("[{rank}] Данж {rank}"),
            Objective::Dungeons => format!("[{rank}] Данжи"),
        }
    }

    /// Returns the contract name shown on the board.
    #[must_use]
    pub fn title(self) -> String {
        let rank = self.rank.name();
        let target = self.objective.target();
        match self.objective {
            Objective::Mobs => format!("[{rank}] Убить {target} любых мобов"),
            Objective::Zombies => format!("[{rank}] Убить {target} зомби"),
            Objective::Boss => format!("[{rank}] Победить босса данжа"),
            Objective::RankedDungeon => format!("[{rank}] Закрыть данж ранга {rank}"),
            Objective::Dungeons => format!("[{rank}] Закрыть {target} данжа"),
        }
    }
}

/// Returns the payment multiplier of a contract rank.
#[must_use]
pub const fn rank_multiplier(rank: HunterRank) -> u64 {
    match rank {
        HunterRank::E => 1,
        HunterRank::D => 2,
        HunterRank::C => 4,
        HunterRank::B => 8,
        HunterRank::A => 16,
        HunterRank::S => 32,
    }
}

/// Something a hunter did that a contract may count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// A non-player mob died to the hunter or one of their shadows.
    Kill {
        /// Whether the mob was a zombie.
        zombie: bool,
    },
    /// The hunter's squad defeated a dungeon boss and left alive.
    Boss,
    /// The hunter cleared a dungeon of the given final rank.
    Dungeon(HunterRank),
}

/// An accepted contract and its progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contract {
    /// The accepted template.
    pub template: Template,
    /// Events counted so far, capped at the target.
    pub progress: u32,
    /// Multiplier of the template's target and payment.
    #[serde(default = "single")]
    pub scale: u32,
}

const fn single() -> u32 {
    1
}

impl Contract {
    /// Starts an empty contract.
    #[must_use]
    pub const fn new(template: Template) -> Self {
        Self::scaled(template, 1)
    }

    /// Starts an empty contract with `scale` times the target and payment.
    #[must_use]
    pub const fn scaled(template: Template, scale: u32) -> Self {
        Self {
            template,
            progress: 0,
            scale,
        }
    }

    /// Returns how many events finish the contract.
    #[must_use]
    pub const fn target(self) -> u32 {
        self.template.objective.target().saturating_mul(self.scale)
    }

    /// Returns the payment one entitled hunter receives.
    #[must_use]
    pub const fn payment(self) -> u64 {
        self.template.payment().saturating_mul(self.scale as u64)
    }

    /// Returns whether the contract can be turned in.
    #[must_use]
    pub const fn is_complete(self) -> bool {
        self.progress >= self.target()
    }

    /// Counts `event` and reports whether this event finished the contract.
    pub fn observe(&mut self, event: Event) -> bool {
        if self.is_complete() || !counts(self.template, event) {
            return false;
        }
        self.progress += 1;
        self.is_complete()
    }
}

fn counts(template: Template, event: Event) -> bool {
    match (template.objective, event) {
        (Objective::Mobs, Event::Kill { .. })
        | (Objective::Zombies, Event::Kill { zombie: true })
        | (Objective::Boss, Event::Boss)
        | (Objective::Dungeons, Event::Dungeon(_)) => true,
        (Objective::RankedDungeon, Event::Dungeon(rank)) => rank == template.rank,
        _ => false,
    }
}

/// Why the Organization refused a contract operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrganizationError {
    /// The hunter is offline or not loaded yet.
    NoSession,
    /// The contract rank is above the hunter's (or the party minimum) rank.
    RankTooHigh,
    /// The personal or party contract limit is reached.
    TooManyContracts,
    /// This template was already taken today.
    UsedToday,
    /// A party member already took this template today.
    MemberUsedToday,
    /// The contract is not finished yet.
    NotComplete,
    /// No contract exists at that position.
    Missing,
    /// Only the party leader may manage group contracts.
    NotLeader,
    /// The hunter is not in a party.
    NotInParty,
    /// The account cannot accept writes right now.
    Storage,
    /// The hunter is not in a guild.
    NotInGuild,
    /// Only the guildmaster and officers may manage guild contracts.
    NotOfficer,
    /// Contracts are taken and turned in at an Organization board.
    NotAtBoard,
}

impl OrganizationError {
    /// Returns how the refusal reads in chat.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::NoSession => "Данные охотника ещё не загружены.",
            Self::RankTooHigh => "Ранг задания выше допустимого.",
            Self::TooManyContracts => "Достигнут лимит активных заданий.",
            Self::UsedToday => "Это задание уже бралось сегодня.",
            Self::MemberUsedToday => "Кто-то из группы уже брал это задание сегодня.",
            Self::NotComplete => "Задание ещё не выполнено.",
            Self::Missing => "Такого задания нет.",
            Self::NotLeader => "Групповыми заданиями управляет лидер группы.",
            Self::NotInParty => "Вы не состоите в группе.",
            Self::Storage => "Организация не может рассчитаться сейчас. Попробуйте позже.",
            Self::NotInGuild => "Вы не состоите в гильдии.",
            Self::NotOfficer => "Гильдийными заданиями управляют мастер и офицеры.",
            Self::NotAtBoard => "Брать и сдавать задания можно у доски Организации.",
        }
    }
}

/// A hunter's personal contracts and today's used templates.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrganizationBook {
    /// Moscow day the `used` list belongs to.
    #[serde(default)]
    day: u64,
    /// Templates already taken today, personally or as a group participant.
    #[serde(default)]
    used: Vec<Template>,
    /// Active personal contracts, in acceptance order.
    #[serde(default)]
    pub contracts: Vec<Contract>,
}

impl OrganizationBook {
    /// Creates a book with no contracts and no spent allowance.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            day: 0,
            used: Vec::new(),
            contracts: Vec::new(),
        }
    }

    /// Forgets yesterday's used templates.
    pub fn roll_over(&mut self, today: u64) {
        if self.day != today {
            self.day = today;
            self.used.clear();
        }
    }

    /// Returns whether `template` was taken on `today`.
    #[must_use]
    pub fn used_today(&self, template: Template, today: u64) -> bool {
        self.day == today && self.used.contains(&template)
    }

    /// Spends today's allowance of `template`.
    pub fn mark_used(&mut self, template: Template, today: u64) {
        self.roll_over(today);
        if !self.used.contains(&template) {
            self.used.push(template);
        }
    }

    /// Accepts a personal contract for a hunter of `rank`.
    pub fn accept(
        &mut self,
        template: Template,
        rank: HunterRank,
        today: u64,
    ) -> Result<(), OrganizationError> {
        if template.rank > rank {
            return Err(OrganizationError::RankTooHigh);
        }
        if self.contracts.len() >= PERSONAL_LIMIT {
            return Err(OrganizationError::TooManyContracts);
        }
        if self.used_today(template, today) {
            return Err(OrganizationError::UsedToday);
        }
        self.mark_used(template, today);
        self.contracts.push(Contract::new(template));
        Ok(())
    }

    /// Cancels a contract; the day's allowance stays spent.
    pub fn cancel(&mut self, index: usize) -> Result<Contract, OrganizationError> {
        if index >= self.contracts.len() {
            return Err(OrganizationError::Missing);
        }
        Ok(self.contracts.remove(index))
    }

    /// Removes a finished contract so it can be paid exactly once.
    pub fn turn_in(&mut self, index: usize) -> Result<Contract, OrganizationError> {
        let contract = self
            .contracts
            .get(index)
            .ok_or(OrganizationError::Missing)?;
        if !contract.is_complete() {
            return Err(OrganizationError::NotComplete);
        }
        Ok(self.contracts.remove(index))
    }

    /// Counts `event` on every contract, returning the ones it finished.
    pub fn observe(&mut self, event: Event) -> Vec<Contract> {
        self.contracts
            .iter_mut()
            .filter_map(|contract| contract.observe(event).then_some(*contract))
            .collect()
    }
}

/// A contract a party took, with its members fixed at acceptance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupContract {
    /// The shared contract.
    pub contract: Contract,
    /// Members entitled to the payment; leaving forfeits the share.
    pub participants: Vec<Uuid>,
    /// An accepted payout transaction is in flight.
    pub(crate) pending_turn_in: bool,
}

/// Returns the highest contract rank a guild of `level` may take.
#[must_use]
pub const fn guild_rank(level: u32) -> HunterRank {
    match level {
        ..=1 => HunterRank::E,
        2 => HunterRank::D,
        3 => HunterRank::C,
        4 => HunterRank::B,
        5 => HunterRank::A,
        6.. => HunterRank::S,
    }
}

/// A contract a guild took, with the members who advanced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuildContract {
    /// The shared contract, scaled by [`GUILD_SCALE`].
    pub contract: Contract,
    /// Members whose actions were counted; each is paid once.
    #[serde(default)]
    pub contributors: Vec<Uuid>,
    /// Runtime guard against duplicate settlements while the storage job runs.
    #[serde(skip)]
    pub pending_turn_in: bool,
}

/// A guild's contracts, today's allowance and payments still owed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuildBoard {
    /// Moscow day the `used` list belongs to.
    #[serde(default)]
    day: u64,
    /// Templates the guild already took today.
    #[serde(default)]
    used: Vec<Template>,
    /// Active guild contracts.
    #[serde(default)]
    pub contracts: Vec<GuildContract>,
    /// Won owed to contributors who were offline when a contract was paid.
    #[serde(default)]
    pub owed: BTreeMap<Uuid, u64>,
}

impl GuildBoard {
    /// Returns whether the guild took `template` on `today`.
    #[must_use]
    pub fn used_today(&self, template: Template, today: u64) -> bool {
        self.day == today && self.used.contains(&template)
    }

    /// Accepts a guild contract for a guild allowed up to `rank`.
    pub fn accept(
        &mut self,
        template: Template,
        rank: HunterRank,
        today: u64,
    ) -> Result<(), OrganizationError> {
        if template.rank > rank {
            return Err(OrganizationError::RankTooHigh);
        }
        if self.contracts.len() >= GUILD_LIMIT {
            return Err(OrganizationError::TooManyContracts);
        }
        if self.used_today(template, today) {
            return Err(OrganizationError::UsedToday);
        }
        if self.day != today {
            self.day = today;
            self.used.clear();
        }
        self.used.push(template);
        self.contracts.push(GuildContract {
            contract: Contract::scaled(template, GUILD_SCALE),
            contributors: Vec::new(),
            pending_turn_in: false,
        });
        Ok(())
    }

    /// Cancels a contract; the day's allowance stays spent.
    pub fn cancel(&mut self, index: usize) -> Result<Contract, OrganizationError> {
        if index >= self.contracts.len() {
            return Err(OrganizationError::Missing);
        }
        Ok(self.contracts.remove(index).contract)
    }

    /// Settles a finished contract, owing its payment to every contributor
    /// that `is_member` still accepts, and returns them.
    pub fn turn_in(
        &mut self,
        index: usize,
        is_member: impl Fn(Uuid) -> bool,
    ) -> Result<(Contract, Vec<Uuid>), OrganizationError> {
        let group = self
            .contracts
            .get(index)
            .ok_or(OrganizationError::Missing)?;
        if !group.contract.is_complete() {
            return Err(OrganizationError::NotComplete);
        }
        if group.pending_turn_in {
            return Err(OrganizationError::Storage);
        }
        let group = self.contracts.remove(index);
        let payment = group.contract.payment();
        let paid: Vec<Uuid> = group
            .contributors
            .into_iter()
            .filter(|contributor| is_member(*contributor))
            .collect();
        for contributor in &paid {
            let owed = self.owed.entry(*contributor).or_default();
            *owed = owed.saturating_add(payment);
        }
        Ok((group.contract, paid))
    }

    /// Counts one event done by `hunters`, all members of this guild.
    ///
    /// The event counts once per contract however many members took part,
    /// and every one of them becomes a contributor. Returns whether anything
    /// changed and the contracts the event finished.
    pub fn observe(&mut self, hunters: &[Uuid], event: Event) -> (bool, Vec<Contract>) {
        let mut changed = false;
        let mut finished = Vec::new();
        for group in &mut self.contracts {
            let before = group.contract.progress;
            let done = group.contract.observe(event);
            if group.contract.progress == before {
                continue;
            }
            changed = true;
            for hunter in hunters {
                if !group.contributors.contains(hunter) {
                    group.contributors.push(*hunter);
                }
            }
            if done {
                finished.push(group.contract);
            }
        }
        (changed, finished)
    }

    /// Takes everything owed to `hunter`.
    pub fn take_owed(&mut self, hunter: Uuid) -> u64 {
        self.owed.remove(&hunter).unwrap_or(0)
    }
}

/// Single-letter rank serialization, read case-insensitively.
mod rank_letter {
    use serde::{Deserialize as _, Deserializer, Serializer, de::Error as _};

    use super::RANKS;
    use crate::rank::HunterRank;

    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "serde's `with` contract passes the field by reference"
    )]
    pub fn serialize<S: Serializer>(rank: &HunterRank, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(rank.name())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<HunterRank, D::Error> {
        let name = String::deserialize(deserializer)?;
        RANKS
            .into_iter()
            .find(|rank| rank.name().eq_ignore_ascii_case(&name))
            .ok_or_else(|| D::Error::custom(format!("unknown hunter rank {name}")))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Contract, Event, GUILD_LIMIT, GuildBoard, Objective, OrganizationBook, OrganizationError,
        PERSONAL_LIMIT, Template, guild_rank,
    };
    use crate::rank::HunterRank;
    use uuid::Uuid;

    const fn template(objective: Objective, rank: HunterRank) -> Template {
        Template { objective, rank }
    }

    #[test]
    fn payment_scales_by_rank() {
        assert_eq!(template(Objective::Mobs, HunterRank::E).payment(), 100);
        assert_eq!(template(Objective::Boss, HunterRank::C).payment(), 600);
        assert_eq!(
            template(Objective::Dungeons, HunterRank::S).payment(),
            16_000
        );
    }

    #[test]
    fn contracts_count_only_matching_events_and_finish_once() {
        let mut zombies = Contract::new(template(Objective::Zombies, HunterRank::E));
        assert!(!zombies.observe(Event::Kill { zombie: false }));
        for _ in 0..9 {
            assert!(!zombies.observe(Event::Kill { zombie: true }));
        }
        assert!(zombies.observe(Event::Kill { zombie: true }));
        assert!(!zombies.observe(Event::Kill { zombie: true }));
        assert_eq!(zombies.progress, 10);

        let mut ranked = Contract::new(template(Objective::RankedDungeon, HunterRank::C));
        assert!(!ranked.observe(Event::Dungeon(HunterRank::D)));
        assert!(ranked.observe(Event::Dungeon(HunterRank::C)));

        let mut any = Contract::new(template(Objective::Dungeons, HunterRank::C));
        assert!(!any.observe(Event::Dungeon(HunterRank::E)));
        assert_eq!(any.progress, 1);
    }

    #[test]
    fn acceptance_enforces_rank_limit_and_daily_allowance() {
        let mut book = OrganizationBook::default();
        let mobs = template(Objective::Mobs, HunterRank::E);
        assert_eq!(
            book.accept(template(Objective::Mobs, HunterRank::D), HunterRank::E, 1),
            Err(OrganizationError::RankTooHigh)
        );
        book.accept(mobs, HunterRank::E, 1).expect("first take");
        book.cancel(0).expect("cancel");
        assert_eq!(
            book.accept(mobs, HunterRank::E, 1),
            Err(OrganizationError::UsedToday)
        );
        book.accept(mobs, HunterRank::E, 2).expect("next day");

        for objective in [Objective::Zombies, Objective::Boss] {
            book.accept(template(objective, HunterRank::E), HunterRank::E, 2)
                .expect("within limit");
        }
        assert_eq!(book.contracts.len(), PERSONAL_LIMIT);
        assert_eq!(
            book.accept(
                template(Objective::Dungeons, HunterRank::E),
                HunterRank::E,
                2
            ),
            Err(OrganizationError::TooManyContracts)
        );
    }

    #[test]
    fn only_finished_contracts_are_turned_in() {
        let mut book = OrganizationBook::default();
        book.accept(template(Objective::Boss, HunterRank::E), HunterRank::E, 1)
            .expect("accept");
        assert_eq!(book.turn_in(0), Err(OrganizationError::NotComplete));
        assert_eq!(book.observe(Event::Boss).len(), 1);
        let paid = book.turn_in(0).expect("turn in");
        assert_eq!(paid.template.payment(), 150);
        assert_eq!(book.contracts, Vec::<Contract>::new());
    }

    #[test]
    fn book_round_trips_through_json() {
        let mut book = OrganizationBook::default();
        book.accept(
            template(Objective::RankedDungeon, HunterRank::B),
            HunterRank::S,
            7,
        )
        .expect("accept");
        let json = serde_json::to_string(&book).expect("serialize");
        assert!(json.contains("\"B\""));
        let restored: OrganizationBook = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored, book);
        assert!(restored.used_today(template(Objective::RankedDungeon, HunterRank::B), 7));
    }

    #[test]
    fn guild_contracts_scale_by_five_and_pay_each_remaining_contributor() {
        let mut board = GuildBoard::default();
        let boss = template(Objective::Boss, HunterRank::E);
        assert_eq!(
            board.accept(template(Objective::Boss, HunterRank::D), guild_rank(1), 1),
            Err(OrganizationError::RankTooHigh)
        );
        board.accept(boss, guild_rank(1), 1).expect("accept");
        assert_eq!(
            board.accept(boss, guild_rank(1), 1),
            Err(OrganizationError::UsedToday)
        );
        board
            .accept(template(Objective::Mobs, HunterRank::E), guild_rank(1), 1)
            .expect("second");
        assert_eq!(board.contracts.len(), GUILD_LIMIT);

        let (first, second, leaver) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        assert_eq!(board.contracts[0].contract.target(), 5);
        board.observe(&[first, second], Event::Boss);
        for _ in 0..3 {
            board.observe(&[leaver], Event::Boss);
        }
        let (changed, finished) = board.observe(&[first], Event::Boss);
        assert!(changed);
        assert_eq!(finished.len(), 1);
        assert_eq!(board.contracts[0].contract.progress, 5);

        let (contract, paid) = board.turn_in(0, |member| member != leaver).expect("paid");
        assert_eq!(contract.payment(), 750);
        assert_eq!(paid, vec![first, second]);
        assert_eq!(board.take_owed(first), 750);
        assert_eq!(board.take_owed(first), 0);
        assert_eq!(board.take_owed(leaver), 0);
    }

    #[test]
    fn an_old_contract_without_scale_reads_as_single() {
        let contract: Contract =
            serde_json::from_str(r#"{"template":{"objective":"Boss","rank":"E"},"progress":0}"#)
                .expect("deserialize");
        assert_eq!(contract.scale, 1);
        assert_eq!(contract.target(), 1);
    }
}
