//! System errands, weekly trials and the Hunter's Path.
//!
//! Besides the mandatory daily quest, the System hands every hunter three
//! optional errands a day and three trials a week, drawn from pools fitted to
//! the hunter's rank and class. The Hunter's Path is a chain of one-time
//! achievements leading a newcomer through every system of the server. All
//! of it counts [`Goal`]s the gameplay reports; everything here is plain data
//! and testable without a world.

use serde::{Deserialize, Serialize};

use crate::class::HunterClass;
use crate::job_change::JobChangeProgress;
use crate::organization::rank_multiplier;
use crate::rank::HunterRank;

/// Errands the System gives every day.
pub const DAILY_ERRANDS: usize = 3;
/// Trials the System gives every week.
pub const WEEKLY_TRIALS: usize = 3;
/// A gate cleared faster than this counts as a fast raid.
pub const FAST_GATE_SECONDS: u64 = 8 * 60;

/// Something the gameplay reports and errands, trials and the path count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Goal {
    /// A gate mob killed.
    MobKill,
    /// A berserker, shaman, bomber or broodmother killed.
    EliteKill,
    /// A gate cleared.
    GateCleared,
    /// A gate of the hunter's own rank cleared.
    OwnRankGate,
    /// A gate boss defeated.
    BossKill,
    /// A gate cleared with nobody dying.
    CleanGate,
    /// A gate cleared faster than [`FAST_GATE_SECONDS`].
    FastGate,
    /// A gate cleared by a party of two or more.
    PartyGate,
    /// A red gate cleared.
    RedGate,
    /// A magic crystal mined.
    Crystal,
    /// A mob material received.
    Material,
    /// An island ant killed.
    Ant,
    /// An island nest destroyed.
    Nest,
    /// The Ant King defeated.
    AntKing,
    /// A Demon Castle floor cleared.
    CastleFloor,
    /// Crystals sold to the Organization.
    CrystalSale,
    /// An enchantment applied.
    Enchant,
    /// A crafting order finished.
    CraftOrder,
    /// A shadow raised.
    ShadowRaise,
    /// A spell cast.
    SpellCast,
    /// Health restored to allies by spells.
    SpellHealing,
    /// Blocks run.
    Distance,
    /// The starter kit claimed.
    StarterKit,
    /// A class chosen.
    ClassChosen,
    /// A spell unlocked.
    SpellUnlocked,
    /// A guild joined.
    GuildJoined,
    /// A lot sold on the auction.
    AuctionSale,
    /// The Cartenon Temple was cleared.
    TempleCleared,
    /// The Knight Igris was defeated.
    IgrisDefeated,
}

/// One daily errand or weekly trial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Errand {
    /// What it counts.
    pub goal: Goal,
    /// How much finishes it.
    pub target: u32,
    /// How much was done.
    pub progress: u32,
}

impl Errand {
    /// Returns whether the errand is finished.
    #[must_use]
    pub const fn done(self) -> bool {
        self.progress >= self.target
    }
}

/// Returns the Russian title of an errand counting `goal` up to `target`.
#[must_use]
pub fn errand_title(goal: Goal, target: u32) -> String {
    match goal {
        Goal::MobKill => format!("Охота: убить {target} мобов во вратах"),
        Goal::EliteKill => format!("Охота на элиту: убить {target} элитных мобов"),
        Goal::GateCleared => format!("Зачистка: закрыть {target} врат"),
        Goal::OwnRankGate => format!("Сила ранга: закрыть {target} врат своего ранга"),
        Goal::BossKill => format!("Охотник за головами: победить {target} боссов"),
        Goal::CleanGate => format!("Без потерь: закрыть {target} врат без смертей"),
        Goal::FastGate => format!("Быстрый рейд: закрыть {target} врат быстрее 8 минут"),
        Goal::PartyGate => format!("Отряд: закрыть {target} врат в группе"),
        Goal::RedGate => format!("Красная угроза: закрыть {target} красных врат"),
        Goal::Crystal => format!("Шахтёр: добыть {target} кристаллов"),
        Goal::Material => format!("Сбор трофеев: получить {target} материалов"),
        Goal::Ant => format!("Островная зачистка: убить {target} муравьёв"),
        Goal::Nest => format!("Разорение гнёзд: уничтожить {target} гнёзд"),
        Goal::AntKing => "Победить Короля муравьёв".to_owned(),
        Goal::CastleFloor => format!("Восхождение: пройти {target} этажей замка"),
        Goal::CrystalSale => "Торговец: продать кристаллы Организации".to_owned(),
        Goal::Enchant => format!("Кузнец: наложить {target} зачарований"),
        Goal::CraftOrder => format!("Мастерская: сделать {target} заказов"),
        Goal::ShadowRaise => format!("Тени: поднять {target} теней"),
        Goal::SpellCast => format!("Маг: применить {target} заклинаний"),
        Goal::SpellHealing => format!("Целитель: восстановить союзникам {target} HP"),
        Goal::Distance => format!("Тренировка: пробежать {target} блоков"),
        Goal::StarterKit
        | Goal::ClassChosen
        | Goal::SpellUnlocked
        | Goal::GuildJoined
        | Goal::AuctionSale
        | Goal::TempleCleared
        | Goal::IgrisDefeated => format!("Выполнить {target} раз"),
    }
}

/// Returns the rank-D target of a daily errand counting `goal`.
const fn daily_target(goal: Goal) -> u32 {
    match goal {
        Goal::MobKill | Goal::SpellCast => 30,
        Goal::EliteKill => 5,
        Goal::GateCleared | Goal::BossKill | Goal::Nest => 2,
        Goal::Crystal => 20,
        Goal::Material => 10,
        Goal::Ant => 25,
        Goal::SpellHealing => 60,
        Goal::Distance => 2_000,
        _ => 1,
    }
}

/// Returns the rank-D target of a weekly trial counting `goal`.
const fn weekly_target(goal: Goal) -> u32 {
    match goal {
        Goal::GateCleared => 15,
        Goal::BossKill | Goal::Nest => 10,
        Goal::CastleFloor | Goal::CleanGate => 5,
        Goal::Crystal => 150,
        Goal::RedGate => 3,
        Goal::MobKill => 300,
        _ => 1,
    }
}

/// Daily errands, with the lowest rank and the class they need.
const DAILY_POOL: [(Goal, HunterRank, Option<HunterClass>); 20] = [
    (Goal::MobKill, HunterRank::E, None),
    (Goal::EliteKill, HunterRank::E, None),
    (Goal::GateCleared, HunterRank::E, None),
    (Goal::OwnRankGate, HunterRank::E, None),
    (Goal::BossKill, HunterRank::E, None),
    (Goal::CleanGate, HunterRank::E, None),
    (Goal::FastGate, HunterRank::E, None),
    (Goal::Crystal, HunterRank::E, None),
    (Goal::Material, HunterRank::E, None),
    (Goal::Ant, HunterRank::C, None),
    (Goal::Nest, HunterRank::C, None),
    (Goal::CastleFloor, HunterRank::E, None),
    (Goal::CrystalSale, HunterRank::E, None),
    (Goal::Enchant, HunterRank::E, None),
    (Goal::CraftOrder, HunterRank::D, None),
    (Goal::ShadowRaise, HunterRank::E, None),
    (Goal::SpellCast, HunterRank::D, Some(HunterClass::Mage)),
    (Goal::SpellHealing, HunterRank::D, Some(HunterClass::Mage)),
    (Goal::PartyGate, HunterRank::E, None),
    (Goal::Distance, HunterRank::E, None),
];

/// Weekly trials, with the lowest rank they need.
const WEEKLY_POOL: [(Goal, HunterRank); 8] = [
    (Goal::GateCleared, HunterRank::E),
    (Goal::BossKill, HunterRank::E),
    (Goal::CastleFloor, HunterRank::E),
    (Goal::Nest, HunterRank::C),
    (Goal::Crystal, HunterRank::E),
    (Goal::RedGate, HunterRank::D),
    (Goal::MobKill, HunterRank::E),
    (Goal::CleanGate, HunterRank::E),
];

/// Returns how much a rank multiplies counted targets.
const fn target_scale(rank: HunterRank) -> f32 {
    match rank {
        HunterRank::E | HunterRank::D => 1.0,
        HunterRank::C => 1.5,
        HunterRank::B => 2.0,
        HunterRank::A => 2.5,
        HunterRank::S => 3.0,
    }
}

fn scaled(target: u32, rank: HunterRank) -> u32 {
    if target <= 1 {
        return target;
    }
    (target as f32 * target_scale(rank)).round() as u32
}

/// Picks `count` distinct entries of `pool` with `roll`.
fn pick<T: Copy>(pool: &[T], count: usize, mut roll: impl FnMut() -> u64) -> Vec<T> {
    let mut left = pool.to_vec();
    let mut picked = Vec::with_capacity(count);
    while picked.len() < count && !left.is_empty() {
        let index = (roll() % left.len() as u64) as usize;
        picked.push(left.swap_remove(index));
    }
    picked
}

/// Rolls the daily errands of a hunter of `rank` and `class`.
#[must_use]
pub fn roll_daily(
    rank: HunterRank,
    class: Option<HunterClass>,
    roll: impl FnMut() -> u64,
) -> Vec<Errand> {
    let pool: Vec<Goal> = DAILY_POOL
        .iter()
        .filter(|(_, minimum, needs)| {
            rank >= *minimum && needs.is_none_or(|needs| class == Some(needs))
        })
        .map(|(goal, _, _)| *goal)
        .collect();
    pick(&pool, DAILY_ERRANDS, roll)
        .into_iter()
        .map(|goal| Errand {
            goal,
            target: scaled(daily_target(goal), rank),
            progress: 0,
        })
        .collect()
}

/// Rolls the weekly trials of a hunter of `rank`.
#[must_use]
pub fn roll_weekly(rank: HunterRank, roll: impl FnMut() -> u64) -> Vec<Errand> {
    let pool: Vec<Goal> = WEEKLY_POOL
        .iter()
        .filter(|(_, minimum)| rank >= *minimum)
        .map(|(goal, _)| *goal)
        .collect();
    pick(&pool, WEEKLY_TRIALS, roll)
        .into_iter()
        .map(|goal| Errand {
            goal,
            target: scaled(weekly_target(goal), rank),
            progress: 0,
        })
        .collect()
}

/// System points one finished errand pays at `rank`.
#[must_use]
pub const fn errand_points(rank: HunterRank) -> u64 {
    20 * rank_multiplier(rank)
}

/// Hunter experience one finished errand pays at `rank`.
#[must_use]
pub const fn errand_experience(rank: HunterRank) -> u64 {
    50 * rank_multiplier(rank)
}

/// Won one finished weekly trial pays at `rank`.
#[must_use]
pub const fn trial_won(rank: HunterRank) -> u64 {
    1_000 * rank_multiplier(rank)
}

/// One step of the Hunter's Path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Achievement {
    /// Clear a first gate.
    FirstGate,
    /// Defeat a first boss.
    FirstBoss,
    /// Clear a first red gate.
    FirstRedGate,
    /// Reach rank D.
    RankD,
    /// Reach rank C.
    RankC,
    /// Reach rank B.
    RankB,
    /// Reach rank A.
    RankA,
    /// Reach rank S.
    RankS,
    /// Reach level 10.
    Level10,
    /// Reach level 25.
    Level25,
    /// Reach level 50.
    Level50,
    /// Claim the starter kit.
    StarterKit,
    /// Apply a first enchantment.
    FirstEnchant,
    /// Finish a first crafting order.
    FirstCraft,
    /// Choose a class.
    ClassChosen,
    /// Unlock a first spell.
    FirstSpell,
    /// Raise a first shadow.
    FirstShadow,
    /// Join a guild.
    JoinGuild,
    /// Clear a gate in a party.
    FirstParty,
    /// Destroy a first island nest.
    FirstNest,
    /// Defeat the Ant King.
    AntKing,
    /// Clear Demon Castle floor 10.
    Castle10,
    /// Clear Demon Castle floor 30.
    Castle30,
    /// Clear Demon Castle floor 60.
    Castle60,
    /// Mine 1000 crystals.
    Crystals1000,
    /// Clear 100 gates.
    Gates100,
    /// Sell a first lot on the auction.
    FirstAuctionSale,
    /// Clear the Cartenon Temple.
    CartenonTemple,
    /// Complete the Second Awakening.
    SecondAwakening,
}

/// Every step of the Hunter's Path, in path order.
pub const PATH: [Achievement; 29] = [
    Achievement::StarterKit,
    Achievement::FirstGate,
    Achievement::FirstBoss,
    Achievement::Level10,
    Achievement::RankD,
    Achievement::ClassChosen,
    Achievement::FirstEnchant,
    Achievement::FirstParty,
    Achievement::JoinGuild,
    Achievement::FirstShadow,
    Achievement::FirstSpell,
    Achievement::FirstCraft,
    Achievement::FirstAuctionSale,
    Achievement::FirstRedGate,
    Achievement::Castle10,
    Achievement::RankC,
    Achievement::Level25,
    Achievement::FirstNest,
    Achievement::Crystals1000,
    Achievement::Castle30,
    Achievement::RankB,
    Achievement::Gates100,
    Achievement::AntKing,
    Achievement::Level50,
    Achievement::RankA,
    Achievement::Castle60,
    Achievement::CartenonTemple,
    Achievement::SecondAwakening,
    Achievement::RankS,
];

impl Achievement {
    /// Returns the Russian title.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::FirstGate => "Первые врата",
            Self::FirstBoss => "Первый босс",
            Self::FirstRedGate => "Красные врата",
            Self::RankD => "Ранг D",
            Self::RankC => "Ранг C",
            Self::RankB => "Ранг B",
            Self::RankA => "Ранг A",
            Self::RankS => "Ранг S",
            Self::Level10 => "Уровень 10",
            Self::Level25 => "Уровень 25",
            Self::Level50 => "Уровень 50",
            Self::StarterKit => "Помощь Ассоциации",
            Self::FirstEnchant => "Первое зачарование",
            Self::FirstCraft => "Первый заказ у мастера",
            Self::ClassChosen => "Выбор пути",
            Self::FirstSpell => "Первое заклинание",
            Self::FirstShadow => "Первая тень",
            Self::JoinGuild => "Гильдия",
            Self::FirstParty => "Первая группа",
            Self::FirstNest => "Первое гнездо",
            Self::AntKing => "Король муравьёв",
            Self::Castle10 => "Замок: этаж 10",
            Self::Castle30 => "Замок: этаж 30",
            Self::Castle60 => "Замок: этаж 60",
            Self::Crystals1000 => "Тысяча кристаллов",
            Self::Gates100 => "Сотня врат",
            Self::FirstAuctionSale => "Первая сделка",
            Self::CartenonTemple => "Картенонский храм",
            Self::SecondAwakening => "Второе пробуждение",
        }
    }

    /// Returns the Russian description of what to do.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::FirstGate => "Закройте первые врата.",
            Self::FirstBoss => "Победите босса врат.",
            Self::FirstRedGate => "Закройте красные врата.",
            Self::RankD => "Достигните ранга D.",
            Self::RankC => "Достигните ранга C.",
            Self::RankB => "Достигните ранга B.",
            Self::RankA => "Достигните ранга A.",
            Self::RankS => "Достигните ранга S.",
            Self::Level10 => "Достигните 10 уровня.",
            Self::Level25 => "Достигните 25 уровня.",
            Self::Level50 => "Достигните 50 уровня.",
            Self::StarterKit => "Получите стартовый набор в Ассоциации.",
            Self::FirstEnchant => "Наложите зачарование на артефакт.",
            Self::FirstCraft => "Сделайте заказ у мастера.",
            Self::ClassChosen => "Выберите класс охотника.",
            Self::FirstSpell => "Откройте заклинание в гримуаре.",
            Self::FirstShadow => "Поднимите тень.",
            Self::JoinGuild => "Вступите в гильдию.",
            Self::FirstParty => "Закройте врата в группе.",
            Self::FirstNest => "Уничтожьте гнездо на острове.",
            Self::AntKing => "Победите Короля муравьёв.",
            Self::Castle10 => "Пройдите 10 этаж Замка демонов.",
            Self::Castle30 => "Пройдите 30 этаж Замка демонов.",
            Self::Castle60 => "Пройдите 60 этаж Замка демонов.",
            Self::Crystals1000 => "Добудьте 1000 кристаллов.",
            Self::Gates100 => "Закройте 100 врат.",
            Self::FirstAuctionSale => "Продайте лот на аукционе.",
            Self::CartenonTemple => "Пройдите Картенонский храм.",
            Self::SecondAwakening => "Завершите Второе пробуждение.",
        }
    }

    /// Returns the Won and system points the step pays.
    #[must_use]
    pub const fn reward(self) -> (u64, u64) {
        match self {
            Self::StarterKit | Self::FirstGate | Self::FirstBoss | Self::Level10 => (300, 50),
            Self::RankD | Self::ClassChosen | Self::FirstEnchant | Self::FirstParty => (600, 100),
            Self::JoinGuild
            | Self::FirstShadow
            | Self::FirstSpell
            | Self::FirstCraft
            | Self::FirstAuctionSale => (800, 120),
            Self::FirstRedGate | Self::Castle10 | Self::RankC | Self::Level25 => (2_000, 300),
            Self::FirstNest | Self::Crystals1000 | Self::Castle30 => (4_000, 500),
            Self::RankB | Self::Gates100 | Self::AntKing => (8_000, 1_000),
            Self::Level50 | Self::RankA | Self::Castle60 => (20_000, 2_500),
            Self::CartenonTemple => (25_000, 3_000),
            Self::SecondAwakening => (30_000, 4_000),
            Self::RankS => (50_000, 5_000),
        }
    }
}

/// What the path checks besides counted goals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standing {
    /// The hunter's rank.
    pub rank: HunterRank,
    /// The hunter's level.
    pub level: u32,
    /// Highest Demon Castle floor cleared.
    pub castle_best: u32,
}

/// A hunter's errands, trials, achievements and lifetime counters.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct QuestBook {
    /// Moscow day the errands belong to.
    #[serde(default)]
    pub day: u64,
    /// Today's errands.
    #[serde(default)]
    pub errands: Vec<Errand>,
    /// Today's errand chest was handed out.
    #[serde(default)]
    pub errand_chest: bool,
    /// Week the trials belong to.
    #[serde(default)]
    pub week: u64,
    /// This week's trials.
    #[serde(default)]
    pub trials: Vec<Errand>,
    /// This week's trial chest was handed out.
    #[serde(default)]
    pub trial_chest: bool,
    /// Finished steps of the Hunter's Path.
    #[serde(default)]
    pub achievements: Vec<Achievement>,
    /// Lifetime counts of the goals the path needs.
    #[serde(default)]
    pub lifetime: Vec<(Goal, u64)>,
    /// Moscow day the Association salary was last paid.
    #[serde(default)]
    pub salary_day: u64,
    /// Moscow day the temple key was last rolled for this hunter.
    #[serde(default)]
    pub temple_key_day: u64,
    /// Whether the temple key has already been rolled today.
    #[serde(default)]
    pub temple_key_rolled: bool,
    /// Progress of the Second Awakening quest chain.
    #[serde(default)]
    pub job_change: JobChangeProgress,
}

/// What recording a goal finished.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Finished {
    /// Errands finished by this record.
    pub errands: Vec<Errand>,
    /// All of today's errands are now done and the chest is owed.
    pub errand_chest: bool,
    /// Trials finished by this record.
    pub trials: Vec<Errand>,
    /// All of this week's trials are now done and the chest is owed.
    pub trial_chest: bool,
}

impl QuestBook {
    /// An empty book.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            day: 0,
            errands: Vec::new(),
            errand_chest: false,
            week: 0,
            trials: Vec::new(),
            trial_chest: false,
            achievements: Vec::new(),
            lifetime: Vec::new(),
            salary_day: 0,
            temple_key_day: 0,
            temple_key_rolled: false,
            job_change: JobChangeProgress::None,
        }
    }

    /// Rolls new errands and trials when the day or the week changed;
    /// returns whether anything was rolled.
    pub fn roll_over(
        &mut self,
        day: u64,
        week: u64,
        rank: HunterRank,
        class: Option<HunterClass>,
        mut roll: impl FnMut() -> u64,
    ) -> bool {
        let mut changed = false;
        if self.day != day || self.errands.is_empty() {
            self.day = day;
            self.errands = roll_daily(rank, class, &mut roll);
            self.errand_chest = false;
            changed = true;
        }
        if self.week != week || self.trials.is_empty() {
            self.week = week;
            self.trials = roll_weekly(rank, &mut roll);
            self.trial_chest = false;
            changed = true;
        }
        changed
    }

    /// Counts `amount` of `goal` toward errands, trials and lifetime totals.
    pub fn record(&mut self, goal: Goal, amount: u32) -> Finished {
        let mut finished = Finished::default();
        if amount == 0 {
            return finished;
        }
        for errand in self.errands.iter_mut().filter(|errand| errand.goal == goal) {
            let was = errand.done();
            errand.progress = errand.progress.saturating_add(amount).min(errand.target);
            if !was && errand.done() {
                finished.errands.push(*errand);
            }
        }
        for trial in self.trials.iter_mut().filter(|trial| trial.goal == goal) {
            let was = trial.done();
            trial.progress = trial.progress.saturating_add(amount).min(trial.target);
            if !was && trial.done() {
                finished.trials.push(*trial);
            }
        }
        if !self.errand_chest && !self.errands.is_empty() && self.errands.iter().all(|e| e.done()) {
            self.errand_chest = true;
            finished.errand_chest = true;
        }
        if !self.trial_chest && !self.trials.is_empty() && self.trials.iter().all(|t| t.done()) {
            self.trial_chest = true;
            finished.trial_chest = true;
        }
        match self
            .lifetime
            .iter_mut()
            .find(|(counted, _)| *counted == goal)
        {
            Some((_, total)) => *total = total.saturating_add(u64::from(amount)),
            None => self.lifetime.push((goal, u64::from(amount))),
        }
        finished
    }

    /// Returns the lifetime count of `goal`.
    #[must_use]
    pub fn total(&self, goal: Goal) -> u64 {
        self.lifetime
            .iter()
            .find_map(|(counted, total)| (*counted == goal).then_some(*total))
            .unwrap_or(0)
    }

    /// Marks every newly reached step of the path and returns them.
    pub fn check_path(&mut self, standing: Standing) -> Vec<Achievement> {
        let reached: Vec<Achievement> = PATH
            .into_iter()
            .filter(|step| !self.achievements.contains(step) && self.reached(*step, standing))
            .collect();
        self.achievements.extend(&reached);
        reached
    }

    fn reached(&self, step: Achievement, standing: Standing) -> bool {
        let has = |goal| self.total(goal) > 0;
        match step {
            Achievement::FirstGate => has(Goal::GateCleared),
            Achievement::FirstBoss => has(Goal::BossKill),
            Achievement::FirstRedGate => has(Goal::RedGate),
            Achievement::RankD => standing.rank >= HunterRank::D,
            Achievement::RankC => standing.rank >= HunterRank::C,
            Achievement::RankB => standing.rank >= HunterRank::B,
            Achievement::RankA => standing.rank >= HunterRank::A,
            Achievement::RankS => standing.rank >= HunterRank::S,
            Achievement::Level10 => standing.level >= 10,
            Achievement::Level25 => standing.level >= 25,
            Achievement::Level50 => standing.level >= 50,
            Achievement::StarterKit => has(Goal::StarterKit),
            Achievement::FirstEnchant => has(Goal::Enchant),
            Achievement::FirstCraft => has(Goal::CraftOrder),
            Achievement::ClassChosen => has(Goal::ClassChosen),
            Achievement::FirstSpell => has(Goal::SpellUnlocked),
            Achievement::FirstShadow => has(Goal::ShadowRaise),
            Achievement::JoinGuild => has(Goal::GuildJoined),
            Achievement::FirstParty => has(Goal::PartyGate),
            Achievement::FirstNest => has(Goal::Nest),
            Achievement::AntKing => has(Goal::AntKing),
            Achievement::Castle10 => standing.castle_best >= 10,
            Achievement::Castle30 => standing.castle_best >= 30,
            Achievement::Castle60 => standing.castle_best >= 60,
            Achievement::Crystals1000 => self.total(Goal::Crystal) >= 1_000,
            Achievement::Gates100 => self.total(Goal::GateCleared) >= 100,
            Achievement::FirstAuctionSale => has(Goal::AuctionSale),
            Achievement::CartenonTemple => has(Goal::TempleCleared),
            Achievement::SecondAwakening => has(Goal::IgrisDefeated),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counter() -> impl FnMut() -> u64 {
        let mut next = 0;
        move || {
            next += 7;
            next
        }
    }

    #[test]
    fn errands_fit_the_rank_and_class() {
        for _ in 0..20 {
            let errands = roll_daily(HunterRank::E, None, counter());
            assert_eq!(errands.len(), DAILY_ERRANDS);
            for errand in &errands {
                assert!(!matches!(
                    errand.goal,
                    Goal::Ant
                        | Goal::Nest
                        | Goal::SpellCast
                        | Goal::SpellHealing
                        | Goal::CraftOrder
                ));
            }
        }
        let mut goals: Vec<Goal> = roll_daily(HunterRank::S, Some(HunterClass::Mage), counter())
            .iter()
            .map(|errand| errand.goal)
            .collect();
        goals.dedup();
        assert_eq!(goals.len(), DAILY_ERRANDS);
        assert_eq!(scaled(30, HunterRank::S), 90);
        assert_eq!(scaled(1, HunterRank::S), 1);
    }

    #[test]
    fn recording_finishes_errands_trials_and_chests_once() {
        let mut book = QuestBook::new();
        book.errands = vec![
            Errand {
                goal: Goal::MobKill,
                target: 2,
                progress: 0,
            },
            Errand {
                goal: Goal::Crystal,
                target: 1,
                progress: 0,
            },
            Errand {
                goal: Goal::BossKill,
                target: 1,
                progress: 0,
            },
        ];
        book.trials = vec![Errand {
            goal: Goal::MobKill,
            target: 3,
            progress: 0,
        }];
        assert_eq!(book.record(Goal::MobKill, 1).errands, Vec::new());
        let done = book.record(Goal::MobKill, 5);
        assert_eq!(done.errands.len(), 1);
        assert_eq!(done.trials.len(), 1);
        assert!(done.trial_chest);
        book.record(Goal::Crystal, 1);
        let last = book.record(Goal::BossKill, 1);
        assert!(last.errand_chest);
        assert!(!book.record(Goal::BossKill, 1).errand_chest);
        assert_eq!(book.total(Goal::MobKill), 6);
    }

    #[test]
    fn the_day_and_week_roll_new_tasks() {
        let mut book = QuestBook::new();
        assert!(book.roll_over(10, 1, HunterRank::D, None, counter()));
        assert!(!book.roll_over(10, 1, HunterRank::D, None, counter()));
        assert!(book.roll_over(11, 1, HunterRank::D, None, counter()));
        assert_eq!(book.trials.len(), WEEKLY_TRIALS);
    }

    #[test]
    fn the_path_marks_each_step_once() {
        let mut book = QuestBook::new();
        book.record(Goal::GateCleared, 1);
        let standing = Standing {
            rank: HunterRank::D,
            level: 12,
            castle_best: 0,
        };
        let reached = book.check_path(standing);
        assert!(reached.contains(&Achievement::FirstGate));
        assert!(reached.contains(&Achievement::RankD));
        assert!(reached.contains(&Achievement::Level10));
        assert!(!reached.contains(&Achievement::RankC));
        assert_eq!(book.check_path(standing), Vec::new());
        let mut seen = PATH.to_vec();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), PATH.len());
    }

    #[test]
    fn a_book_round_trips_through_json() {
        let mut book = QuestBook::new();
        book.roll_over(3, 0, HunterRank::C, None, counter());
        book.record(Goal::Crystal, 4);
        let back: QuestBook =
            serde_json::from_str(&serde_json::to_string(&book).expect("encode")).expect("decode");
        assert_eq!(back, book);
    }
}
