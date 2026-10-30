//! The Mage's spells: the catalog, mana, and a hunter's spell book.
//!
//! Spells are unlocked for system points and kept for good, even across a
//! class change. Only prepared spells can be cast; the grimoire cycles
//! through them. Everything here is plain data, so the rules are testable
//! without a world.

use std::collections::BTreeMap;

use glam::DVec3;
use serde::{Deserialize, Serialize};

use crate::rank::HunterRank;

/// Mana of a Mage with no Intelligence.
pub const BASE_MANA: f32 = 100.0;
/// Extra maximum mana per point of Intelligence.
pub const MANA_PER_INTELLIGENCE: f32 = 5.0;
/// Mana restored per second with no Intelligence.
pub const BASE_REGEN: f32 = 2.0;
/// Extra mana restored per second per point of Intelligence.
pub const REGEN_PER_INTELLIGENCE: f32 = 0.1;
/// Regeneration multiplier once the hunter took no damage for a while.
pub const CALM_REGEN_MULTIPLIER: f32 = 2.0;
/// Milliseconds without damage before regeneration counts as calm.
pub const CALM_AFTER_MILLIS: u64 = 8_000;

/// Returns a Mage's maximum mana.
#[must_use]
pub fn max_mana(intelligence: u32) -> f32 {
    BASE_MANA + MANA_PER_INTELLIGENCE * intelligence as f32
}

/// Returns a Mage's mana regeneration per second.
#[must_use]
pub fn mana_regen(intelligence: u32, calm: bool) -> f32 {
    let regen = BASE_REGEN + REGEN_PER_INTELLIGENCE * intelligence as f32;
    if calm {
        regen * CALM_REGEN_MULTIPLIER
    } else {
        regen
    }
}

/// Returns how many spells a hunter of `rank` may keep prepared.
#[must_use]
pub const fn prepared_slots(rank: HunterRank) -> usize {
    match rank {
        HunterRank::E | HunterRank::D | HunterRank::C => 4,
        HunterRank::B | HunterRank::A => 5,
        HunterRank::S => 6,
    }
}

/// A school of magic: one row of the grimoire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum School {
    /// Damage.
    Destruction,
    /// Healing and protection of allies.
    Healing,
    /// Crowd control and party buffs.
    Control,
    /// Movement, sight and mana.
    Utility,
}

impl School {
    /// Every school, in grimoire order.
    pub const ALL: [Self; 4] = [
        Self::Destruction,
        Self::Healing,
        Self::Control,
        Self::Utility,
    ];

    /// Returns the Russian name.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Destruction => "Разрушение",
            Self::Healing => "Исцеление",
            Self::Control => "Контроль и поддержка",
            Self::Utility => "Пространство и утилиты",
        }
    }
}

/// One spell of the grimoire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpellId {
    /// A bolt along the line of sight.
    MagicArrow,
    /// An exploding, burning bolt.
    Fireball,
    /// A bolt that slows.
    IceSpear,
    /// A bolt that jumps to nearby mobs.
    ChainLightning,
    /// A storm around the caster.
    ManaStorm,
    /// A delayed impact where the caster looks.
    Meteor,
    /// A beam through every mob on the line.
    VoidLance,
    /// Cheaper, stronger destruction for a while.
    MonarchsWrath,
    /// A quick heal for one ally.
    LesserHeal,
    /// Healing over time for the party.
    Restoration,
    /// Removes harmful effects and fire from the party.
    Cleanse,
    /// An absorption shield for the party.
    Barrier,
    /// A large heal for one ally.
    GreaterHeal,
    /// A healing, protecting circle.
    Sanctuary,
    /// The party cannot drop below one health for a while.
    LastStand,
    /// Pushes nearby mobs away.
    Repulse,
    /// Party movement speed.
    Haste,
    /// Roots one mob.
    Shackles,
    /// Party damage.
    BlessingOfStrength,
    /// Weakens nearby mobs' attacks.
    Weaken,
    /// Pulls mobs to a point and slows them.
    GravityWell,
    /// Party night vision.
    NightVision,
    /// A short teleport along the line of sight.
    Blink,
    /// Reveals nearby mobs and the boss.
    Detection,
    /// Channelled mana recovery.
    Meditation,
    /// Damage drains mana instead of health.
    ManaShield,
}

/// Static data of one spell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpellInfo {
    /// Russian name.
    pub name: &'static str,
    /// School the spell belongs to.
    pub school: School,
    /// One-line Russian description at rank D.
    pub description: &'static str,
    /// Mana spent on every cast.
    pub mana: f32,
    /// Cooldown in milliseconds.
    pub cooldown_millis: u64,
    /// Unlock price in system points; zero for the starting spells.
    pub price: u64,
    /// Lowest hunter rank that may unlock it.
    pub rank: HunterRank,
}

const fn info(
    name: &'static str,
    school: School,
    description: &'static str,
    mana: f32,
    cooldown_seconds: u64,
    price: u64,
    rank: HunterRank,
) -> SpellInfo {
    SpellInfo {
        name,
        school,
        description,
        mana,
        cooldown_millis: cooldown_seconds * 1_000,
        price,
        rank,
    }
}

/// Every spell, in grimoire order: school by school.
pub const ALL: [SpellId; 26] = [
    SpellId::MagicArrow,
    SpellId::Fireball,
    SpellId::IceSpear,
    SpellId::ChainLightning,
    SpellId::ManaStorm,
    SpellId::Meteor,
    SpellId::VoidLance,
    SpellId::MonarchsWrath,
    SpellId::LesserHeal,
    SpellId::Restoration,
    SpellId::Cleanse,
    SpellId::Barrier,
    SpellId::GreaterHeal,
    SpellId::Sanctuary,
    SpellId::LastStand,
    SpellId::Repulse,
    SpellId::Haste,
    SpellId::Shackles,
    SpellId::BlessingOfStrength,
    SpellId::Weaken,
    SpellId::GravityWell,
    SpellId::NightVision,
    SpellId::Blink,
    SpellId::Detection,
    SpellId::Meditation,
    SpellId::ManaShield,
];

/// Spells every Mage knows from the first pick.
pub const STARTING: [SpellId; 2] = [SpellId::MagicArrow, SpellId::LesserHeal];

impl SpellId {
    /// Returns the static data of the spell.
    #[must_use]
    #[expect(
        clippy::too_many_lines,
        reason = "the catalog reads best as one table of spells"
    )]
    pub const fn info(self) -> SpellInfo {
        use HunterRank::{A, B, C, D, S};
        use School::{Control, Destruction, Healing, Utility};
        match self {
            Self::MagicArrow => info(
                "Магическая стрела",
                Destruction,
                "Луч до 24 блоков: 6 магического урона",
                10.0,
                1,
                0,
                D,
            ),
            Self::Fireball => info(
                "Огненный шар",
                Destruction,
                "Взрыв радиусом 3 в точке взгляда: 8 урона и поджог",
                25.0,
                4,
                100,
                D,
            ),
            Self::IceSpear => info(
                "Ледяное копьё",
                Destruction,
                "10 урона и замедление 60% на 3 с",
                25.0,
                5,
                200,
                C,
            ),
            Self::ChainLightning => info(
                "Цепная молния",
                Destruction,
                "8 урона цели и до 3 соседям, каждому следующему −25%",
                35.0,
                6,
                300,
                C,
            ),
            Self::ManaStorm => info(
                "Буря маны",
                Destruction,
                "6 с вокруг мага: 4 урона в секунду в радиусе 5",
                60.0,
                25,
                800,
                B,
            ),
            Self::Meteor => info(
                "Метеор",
                Destruction,
                "Через 1,5 с в точку взгляда: 25 урона в радиусе 5",
                70.0,
                20,
                1_500,
                B,
            ),
            Self::VoidLance => info(
                "Копьё пустоты",
                Destruction,
                "Пробивает всех на линии: 30 урона, половина брони не защищает",
                100.0,
                30,
                3_000,
                A,
            ),
            Self::MonarchsWrath => info(
                "Гнев монарха",
                Destruction,
                "10 с: разрушение −50% маны и +30% урона",
                80.0,
                120,
                6_000,
                S,
            ),
            Self::LesserHeal => info(
                "Малое исцеление",
                Healing,
                "Союзник в прицеле или сам маг: +6 HP",
                20.0,
                3,
                0,
                D,
            ),
            Self::Restoration => info(
                "Восстановление",
                Healing,
                "Группа в радиусе 10: +1 HP в секунду 8 с",
                40.0,
                15,
                150,
                D,
            ),
            Self::Cleanse => info(
                "Очищение",
                Healing,
                "Снимает с группы яд, иссушение, замедление, слабость и огонь",
                25.0,
                10,
                200,
                C,
            ),
            Self::Barrier => info(
                "Барьер",
                Healing,
                "Группе щит поглощения 8 HP на 8 с",
                45.0,
                20,
                400,
                C,
            ),
            Self::GreaterHeal => info(
                "Великое исцеление",
                Healing,
                "Союзник в прицеле или сам маг: +50% здоровья",
                60.0,
                20,
                1_000,
                B,
            ),
            Self::Sanctuary => info(
                "Святилище",
                Healing,
                "Круг радиусом 6 на 10 с: союзники −30% урона и +1 HP в секунду",
                90.0,
                45,
                2_500,
                A,
            ),
            Self::LastStand => info(
                "Последний рубеж",
                Healing,
                "6 с союзники в радиусе 10 не опускаются ниже 1 HP",
                120.0,
                120,
                5_000,
                S,
            ),
            Self::Repulse => info(
                "Отталкивание",
                Control,
                "Отбрасывает мобов в радиусе 5",
                20.0,
                8,
                100,
                D,
            ),
            Self::Haste => info(
                "Ускорение",
                Control,
                "Группе +40% скорости на 10 с",
                30.0,
                30,
                150,
                D,
            ),
            Self::Shackles => info(
                "Оковы",
                Control,
                "Цель обездвижена на 3 с (босс — на 1 с)",
                30.0,
                12,
                250,
                C,
            ),
            Self::BlessingOfStrength => info(
                "Благословение силы",
                Control,
                "Группе +15% урона на 30 с",
                50.0,
                60,
                600,
                C,
            ),
            Self::Weaken => info(
                "Ослабление",
                Control,
                "Мобы в радиусе 6 наносят −25% урона 8 с",
                40.0,
                20,
                700,
                B,
            ),
            Self::GravityWell => info(
                "Гравитационный колодец",
                Control,
                "Стягивает мобов в радиусе 8 к точке взгляда и замедляет",
                50.0,
                20,
                900,
                B,
            ),
            Self::NightVision => info(
                "Ночное зрение",
                Utility,
                "Группе ночное зрение на 3 минуты",
                10.0,
                5,
                50,
                D,
            ),
            Self::Blink => info(
                "Скачок",
                Utility,
                "Телепорт до 8 блоков по взгляду, сквозь стены нельзя",
                25.0,
                6,
                200,
                D,
            ),
            Self::Detection => info(
                "Обнаружение",
                Utility,
                "Подсвечивает мобов в радиусе 32 на 15 с и показывает, где босс",
                20.0,
                30,
                150,
                D,
            ),
            Self::Meditation => info(
                "Медитация",
                Utility,
                "4 с стоя на месте: +50% маны; прерывается уроном и движением",
                0.0,
                60,
                300,
                C,
            ),
            Self::ManaShield => info(
                "Мана-щит",
                Utility,
                "10 с урон снимается с маны вместо здоровья (1 HP = 2 маны)",
                30.0,
                30,
                800,
                B,
            ),
        }
    }
}

/// Why a spell cannot be unlocked or prepared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpellError {
    /// The spell is already unlocked.
    AlreadyKnown,
    /// The hunter's rank is below the spell's.
    RankTooLow,
    /// Not enough system points.
    InsufficientPoints,
    /// The spell is not unlocked yet.
    Unknown,
    /// Every prepared slot is taken.
    SlotsFull,
    /// Prepared spells change only outside instances.
    InInstance,
    /// The hunter is not a Mage.
    NotAMage,
    /// Storage is unavailable or a payment is still committing.
    Storage,
}

impl SpellError {
    /// Returns the Russian message shown to the hunter.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::AlreadyKnown => "Заклинание уже открыто.",
            Self::RankTooLow => "Ранг охотника слишком низкий для этого заклинания.",
            Self::InsufficientPoints => "Недостаточно очков Системы.",
            Self::Unknown => "Сначала откройте заклинание.",
            Self::SlotsFull => "Все ячейки заняты: уберите другое заклинание.",
            Self::InInstance => "Набор заклинаний меняется только вне подземелий.",
            Self::NotAMage => "Заклинания доступны только магу.",
            Self::Storage => "Хранилище занято или недоступно: повторите позже.",
        }
    }
}

/// A hunter's unlocked and prepared spells.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SpellBook {
    /// Unlocked spells, in unlock order.
    #[serde(default)]
    pub unlocked: Vec<SpellId>,
    /// Prepared spells, in casting order.
    #[serde(default)]
    pub prepared: Vec<SpellId>,
    /// Index of the selected prepared spell.
    #[serde(default)]
    pub selected: usize,
}

impl SpellBook {
    /// An empty spell book.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            unlocked: Vec::new(),
            prepared: Vec::new(),
            selected: 0,
        }
    }

    /// Teaches the starting spells and prepares them if there is room.
    pub fn learn_starting(&mut self) {
        for spell in STARTING {
            if !self.knows(spell) {
                self.unlocked.push(spell);
            }
            if !self.prepared.contains(&spell) {
                self.prepared.push(spell);
            }
        }
    }

    /// Returns whether `spell` is unlocked.
    #[must_use]
    pub fn knows(&self, spell: SpellId) -> bool {
        self.unlocked.contains(&spell)
    }

    /// Checks whether `spell` may be unlocked by a hunter of `rank` with
    /// `points`, and returns its price.
    pub fn unlock_price(
        &self,
        spell: SpellId,
        rank: HunterRank,
        points: u64,
    ) -> Result<u64, SpellError> {
        let info = spell.info();
        if self.knows(spell) {
            return Err(SpellError::AlreadyKnown);
        }
        if rank < info.rank {
            return Err(SpellError::RankTooLow);
        }
        if points < info.price {
            return Err(SpellError::InsufficientPoints);
        }
        Ok(info.price)
    }

    /// Records an unlocked spell; the caller already took the price.
    pub fn unlock(&mut self, spell: SpellId) {
        if !self.knows(spell) {
            self.unlocked.push(spell);
        }
    }

    /// Prepares an unprepared spell or puts a prepared one away.
    pub fn toggle_prepared(&mut self, spell: SpellId, rank: HunterRank) -> Result<(), SpellError> {
        if let Some(index) = self.prepared.iter().position(|known| *known == spell) {
            self.prepared.remove(index);
            self.clamp_selection();
            return Ok(());
        }
        if !self.knows(spell) {
            return Err(SpellError::Unknown);
        }
        if self.prepared.len() >= prepared_slots(rank) {
            return Err(SpellError::SlotsFull);
        }
        self.prepared.push(spell);
        Ok(())
    }

    /// Returns the selected prepared spell.
    #[must_use]
    pub fn selected_spell(&self) -> Option<SpellId> {
        self.prepared.get(self.selected).copied()
    }

    /// Selects the next prepared spell and returns it.
    pub fn cycle(&mut self) -> Option<SpellId> {
        if self.prepared.is_empty() {
            return None;
        }
        self.selected = (self.selected + 1) % self.prepared.len();
        self.selected_spell()
    }

    /// Selects the prepared spell at `index`.
    pub const fn select(&mut self, index: usize) {
        if index < self.prepared.len() {
            self.selected = index;
        }
    }

    const fn clamp_selection(&mut self) {
        if self.selected >= self.prepared.len() {
            self.selected = 0;
        }
    }
}

/// Spellcasting bonuses of Mage artifacts, summed over the held staff and
/// the System gear.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SpellGear {
    /// Extra outgoing magic damage, as a fraction.
    pub magic: f32,
    /// Extra maximum mana.
    pub mana: f32,
    /// Extra mana regeneration, as a fraction.
    pub regen: f32,
    /// Change of every spell's mana cost, as a fraction; negative is cheaper.
    pub cost: f32,
    /// Change of destruction spells' mana cost, as a fraction.
    pub destruction_cost: f32,
    /// Reduction of every spell's cooldown, as a fraction.
    pub cooldown: f32,
    /// Extra healing, as a fraction.
    pub healing: f32,
    /// Extra Barrier shield health.
    pub barrier: f32,
    /// Every Nth cast is free; zero when no item grants it.
    pub free_every: u32,
    /// Percent chance that a destruction spell strikes twice.
    pub echo_percent: u32,
    /// Fire spells burn twice as long.
    pub burning: bool,
    /// Meditation has no cooldown.
    pub free_meditation: bool,
}

/// Cheapest a spell can get, as a fraction of its base cost.
const MIN_COST_FACTOR: f32 = 0.2;
/// Largest cooldown reduction.
const MAX_COOLDOWN_REDUCTION: f32 = 0.5;

impl SpellGear {
    /// No bonuses.
    pub const NONE: Self = Self {
        magic: 0.0,
        mana: 0.0,
        regen: 0.0,
        cost: 0.0,
        destruction_cost: 0.0,
        cooldown: 0.0,
        healing: 0.0,
        barrier: 0.0,
        free_every: 0,
        echo_percent: 0,
        burning: false,
        free_meditation: false,
    };

    /// Returns the sum of two sets of bonuses.
    #[must_use]
    pub fn combine(self, other: Self) -> Self {
        Self {
            magic: self.magic + other.magic,
            mana: self.mana + other.mana,
            regen: self.regen + other.regen,
            cost: self.cost + other.cost,
            destruction_cost: self.destruction_cost + other.destruction_cost,
            cooldown: self.cooldown + other.cooldown,
            healing: self.healing + other.healing,
            barrier: self.barrier + other.barrier,
            free_every: match (self.free_every, other.free_every) {
                (0, n) | (n, 0) => n,
                (a, b) => a.min(b),
            },
            echo_percent: (self.echo_percent + other.echo_percent).min(100),
            burning: self.burning || other.burning,
            free_meditation: self.free_meditation || other.free_meditation,
        }
    }

    /// Returns the mana a cast of `spell` costs with these bonuses.
    #[must_use]
    pub fn cost_of(self, spell: SpellId) -> f32 {
        let destruction = if spell.info().school == School::Destruction {
            self.destruction_cost
        } else {
            0.0
        };
        spell.info().mana * (1.0 + self.cost + destruction).max(MIN_COST_FACTOR)
    }

    /// Returns the cooldown of `spell` with these bonuses, in milliseconds.
    #[must_use]
    pub fn cooldown_of(self, spell: SpellId) -> u64 {
        if spell == SpellId::Meditation && self.free_meditation {
            return 0;
        }
        let factor = 1.0 - self.cooldown.clamp(0.0, MAX_COOLDOWN_REDUCTION);
        (spell.info().cooldown_millis as f32 * factor) as u64
    }

    /// Returns the Russian lore lines describing these bonuses.
    #[must_use]
    pub fn describe(self) -> Vec<String> {
        let percent = |value: f32| (value * 100.0).round();
        let mut lines = Vec::new();
        if self.magic != 0.0 {
            lines.push(format!("Магический урон {:+}%", percent(self.magic)));
        }
        if self.mana != 0.0 {
            lines.push(format!("Мана {:+}", self.mana));
        }
        if self.regen != 0.0 {
            lines.push(format!("Восстановление маны {:+}%", percent(self.regen)));
        }
        if self.cost != 0.0 {
            lines.push(format!("Стоимость заклинаний {:+}%", percent(self.cost)));
        }
        if self.destruction_cost != 0.0 {
            lines.push(format!(
                "Стоимость заклинаний разрушения {:+}%",
                percent(self.destruction_cost)
            ));
        }
        if self.cooldown != 0.0 {
            lines.push(format!(
                "Перезарядка заклинаний -{}%",
                percent(self.cooldown)
            ));
        }
        if self.healing != 0.0 {
            lines.push(format!("Сила исцеления {:+}%", percent(self.healing)));
        }
        if self.barrier != 0.0 {
            lines.push(format!("Барьер даёт ещё {} HP щита", self.barrier));
        }
        if self.free_every > 0 {
            lines.push(format!("Каждое {}-е заклинание бесплатно", self.free_every));
        }
        if self.echo_percent > 0 {
            lines.push(format!(
                "Заклинания разрушения с шансом {}% срабатывают дважды",
                self.echo_percent
            ));
        }
        if self.burning {
            lines.push("Огненные заклинания поджигают вдвое дольше".to_owned());
        }
        if self.free_meditation {
            lines.push("Медитация без перезарядки".to_owned());
        }
        lines
    }
}

/// Live magic state of one hunter; never persisted.
///
/// Party buffs land on every member, so the deadlines live here for any
/// class, while mana and cooldowns only matter to a Mage.
#[derive(Debug, Clone, Default)]
pub struct MagicState {
    /// Current mana; `None` until the first tick fills it.
    pub mana: Option<f32>,
    /// Millisecond the hunter last took damage.
    pub last_hurt: u64,
    /// Cooldown deadline of every spell cast recently.
    pub cooldowns: BTreeMap<SpellId, u64>,
    /// End of Monarch's Wrath.
    pub wrath_until: u64,
    /// End of Blessing of Strength.
    pub blessed_until: u64,
    /// End of the Sanctuary's protection.
    pub ward_until: u64,
    /// End of Last Stand.
    pub last_stand_until: u64,
    /// End of the Mana Shield.
    pub mana_shield_until: u64,
    /// Start and place of a running Meditation.
    pub meditation: Option<(u64, DVec3)>,
    /// A grimoire is owed and handed out on the next tick.
    pub grimoire_due: bool,
    /// Mana last shown on the experience bar.
    pub shown_mana: Option<i32>,
    /// Casts since the last free one, for items that make every Nth free.
    pub casts: u32,
}

impl MagicState {
    /// A hunter with no mana yet and no running effects.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            mana: None,
            last_hurt: 0,
            cooldowns: BTreeMap::new(),
            wrath_until: 0,
            blessed_until: 0,
            ward_until: 0,
            last_stand_until: 0,
            mana_shield_until: 0,
            meditation: None,
            grimoire_due: false,
            shown_mana: None,
            casts: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spell_is_listed_once_and_schools_stay_together() {
        let mut seen = ALL.to_vec();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), ALL.len());
        let mut order: Vec<School> = ALL.iter().map(|spell| spell.info().school).collect();
        order.dedup();
        assert_eq!(order, School::ALL.to_vec());
        for spell in STARTING {
            assert_eq!(spell.info().price, 0);
        }
        for spell in ALL {
            assert!(STARTING.contains(&spell) || spell.info().price > 0);
        }
    }

    #[test]
    fn mana_grows_with_intelligence() {
        assert!((max_mana(0) - 100.0).abs() < f32::EPSILON);
        assert!((max_mana(10) - 150.0).abs() < f32::EPSILON);
        assert!((mana_regen(10, false) - 3.0).abs() < 1e-6);
        assert!((mana_regen(10, true) - 6.0).abs() < 1e-6);
    }

    #[test]
    fn unlocking_checks_rank_points_and_duplicates() {
        let mut book = SpellBook::default();
        book.learn_starting();
        assert_eq!(
            book.unlock_price(SpellId::MagicArrow, HunterRank::D, 0),
            Err(SpellError::AlreadyKnown)
        );
        assert_eq!(
            book.unlock_price(SpellId::IceSpear, HunterRank::D, 1_000),
            Err(SpellError::RankTooLow)
        );
        assert_eq!(
            book.unlock_price(SpellId::Fireball, HunterRank::D, 50),
            Err(SpellError::InsufficientPoints)
        );
        assert_eq!(
            book.unlock_price(SpellId::Fireball, HunterRank::D, 100),
            Ok(100)
        );
    }

    #[test]
    fn prepared_slots_limit_the_set_and_cycling_wraps() {
        let mut book = SpellBook::default();
        book.learn_starting();
        for spell in [SpellId::Fireball, SpellId::Repulse, SpellId::Blink] {
            book.unlock(spell);
        }
        assert_eq!(
            book.toggle_prepared(SpellId::Haste, HunterRank::D),
            Err(SpellError::Unknown)
        );
        book.toggle_prepared(SpellId::Fireball, HunterRank::D)
            .expect("slot");
        book.toggle_prepared(SpellId::Repulse, HunterRank::D)
            .expect("slot");
        assert_eq!(
            book.toggle_prepared(SpellId::Blink, HunterRank::D),
            Err(SpellError::SlotsFull)
        );
        book.toggle_prepared(SpellId::Blink, HunterRank::B)
            .expect("fifth slot at B");
        assert_eq!(book.selected_spell(), Some(SpellId::MagicArrow));
        for _ in 0..5 {
            book.cycle();
        }
        assert_eq!(book.selected_spell(), Some(SpellId::MagicArrow));
        book.select(4);
        book.toggle_prepared(SpellId::Blink, HunterRank::B)
            .expect("remove");
        assert_eq!(book.selected, 0);
    }

    #[test]
    fn gear_bonuses_add_up_and_stay_within_bounds() {
        let staff = SpellGear {
            destruction_cost: -0.1,
            free_every: 5,
            ..SpellGear::default()
        };
        let orb = SpellGear {
            cost: 0.2,
            cooldown: 0.4,
            free_every: 7,
            ..SpellGear::default()
        };
        let both = staff.combine(orb).combine(SpellGear {
            cooldown: 0.4,
            ..SpellGear::default()
        });
        assert_eq!(both.free_every, 5);
        assert!((both.cost_of(SpellId::Fireball) - 27.5).abs() < 1e-4);
        assert!((both.cost_of(SpellId::LesserHeal) - 24.0).abs() < 1e-4);
        assert_eq!(both.cooldown_of(SpellId::Fireball), 2_000);
        let cheap = SpellGear {
            cost: -5.0,
            ..SpellGear::default()
        };
        assert!((cheap.cost_of(SpellId::Fireball) - 5.0).abs() < 1e-4);
        let calm = SpellGear {
            free_meditation: true,
            ..SpellGear::default()
        };
        assert_eq!(calm.cooldown_of(SpellId::Meditation), 0);
    }

    #[test]
    fn a_book_round_trips_through_json() {
        let mut book = SpellBook::default();
        book.learn_starting();
        let json = serde_json::to_string(&book).expect("encode");
        assert!(json.contains("magic_arrow"));
        let back: SpellBook = serde_json::from_str(&json).expect("decode");
        assert_eq!(back, book);
    }
}
