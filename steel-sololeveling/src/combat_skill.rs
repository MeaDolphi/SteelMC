//! Class and general combat skills, their ranks, points and cooldowns.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::abilities::now_millis;
use crate::class::HunterClass;
use crate::rank::HunterRank;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CombatSkill {
    Mutilation,
    Taunt,
    Stealth,
    VitalShot,
    RulersAuthority,
    DaggerThrow,
    ShadowExchange,
}

impl CombatSkill {
    pub const CLASS: [Self; 5] = [
        Self::Mutilation,
        Self::Taunt,
        Self::Stealth,
        Self::VitalShot,
        Self::RulersAuthority,
    ];
    pub const GENERAL: [Self; 2] = [Self::DaggerThrow, Self::ShadowExchange];

    #[must_use]
    pub const fn for_class(class: HunterClass) -> Self {
        match class {
            HunterClass::Fighter => Self::Mutilation,
            HunterClass::Tanker => Self::Taunt,
            HunterClass::Assassin => Self::Stealth,
            HunterClass::Ranger => Self::VitalShot,
            HunterClass::Mage => Self::RulersAuthority,
        }
    }

    #[must_use]
    pub const fn is_general(self) -> bool {
        matches!(self, Self::DaggerThrow | Self::ShadowExchange)
    }

    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Mutilation => "Рассечение",
            Self::Taunt => "Провокация",
            Self::Stealth => "Скрытность",
            Self::VitalShot => "Прицельный выстрел",
            Self::RulersAuthority => "Власть правителя",
            Self::DaggerThrow => "Бросок кинжала",
            Self::ShadowExchange => "Обмен тенью",
        }
    }

    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Mutilation => "Серия физических ударов по конусу перед охотником.",
            Self::Taunt => "Противники выбирают Танка целью; Танк получает щит.",
            Self::Stealth => "Невидимость и сброс целей. Атака раскрывает охотника.",
            Self::VitalShot => "Один усиленный стрелковый снаряд по линии взгляда.",
            Self::RulersAuthority => "Притяжение противников и магический урон. Стоит 10 маны.",
            Self::DaggerThrow => "Физический снаряд перед собой. Доступен любому классу.",
            Self::ShadowExchange => "Обмен местами с ближайшей активной тенью. Нужен ранг B.",
        }
    }

    #[must_use]
    pub const fn cooldown_seconds(self) -> u64 {
        match self {
            Self::Mutilation => 12,
            Self::Taunt => 20,
            Self::Stealth => 25,
            Self::VitalShot => 10,
            Self::RulersAuthority => 18,
            Self::DaggerThrow => 8,
            Self::ShadowExchange => 30,
        }
    }

    #[must_use]
    pub const fn required_rank(self) -> HunterRank {
        if matches!(self, Self::ShadowExchange) {
            HunterRank::B
        } else {
            HunterRank::E
        }
    }

    const fn general_base_cost(self) -> u32 {
        if self.is_general() { 1 } else { 0 }
    }

    #[must_use]
    pub const fn next_rank_cost(self, current_rank: u8) -> u32 {
        self.next_cost(current_rank + 1)
    }

    const fn next_cost(self, next_rank: u8) -> u32 {
        match next_rank {
            1 => self.general_base_cost(),
            2 => 2,
            3 => 3,
            _ => 0,
        }
    }

    const fn spent_on(self, rank: u8) -> u32 {
        match rank {
            0 => 0,
            1 => self.general_base_cost(),
            2 => self.general_base_cost() + 2,
            _ => self.general_base_cost() + 2 + 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradeError {
    ClassLocked,
    RankLocked,
    MaxRank,
    NoPoints,
    NotGeneral,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CombatSkillBook {
    #[serde(default)]
    pub ranks: BTreeMap<CombatSkill, u8>,
    #[serde(default)]
    pub selected_general: Option<CombatSkill>,
    #[serde(default)]
    pub cooldowns: BTreeMap<CombatSkill, u64>,
}

impl Default for CombatSkillBook {
    fn default() -> Self {
        Self::new()
    }
}

impl CombatSkillBook {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            ranks: BTreeMap::new(),
            selected_general: None,
            cooldowns: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn available_points(&self, level: u32) -> u32 {
        let spent = self
            .ranks
            .iter()
            .map(|(skill, rank)| skill.spent_on(*rank))
            .sum::<u32>();
        (level / 2).saturating_sub(spent)
    }

    #[must_use]
    pub fn class_rank(&self, class: HunterClass) -> u8 {
        self.ranks
            .get(&CombatSkill::for_class(class))
            .copied()
            .unwrap_or(0)
    }

    #[must_use]
    pub fn general_rank(&self, skill: CombatSkill) -> u8 {
        self.ranks.get(&skill).copied().unwrap_or(0)
    }

    pub fn unlock_class(&mut self, class: HunterClass, rank: HunterRank) {
        if rank >= HunterRank::D {
            self.ranks.entry(CombatSkill::for_class(class)).or_insert(1);
        }
    }

    pub fn upgrade_class(
        &mut self,
        class: HunterClass,
        level: u32,
        rank: HunterRank,
    ) -> Result<u8, UpgradeError> {
        if rank < HunterRank::D {
            return Err(UpgradeError::ClassLocked);
        }
        self.unlock_class(class, rank);
        let skill = CombatSkill::for_class(class);
        self.upgrade(skill, level)
    }

    pub fn upgrade_general(
        &mut self,
        skill: CombatSkill,
        level: u32,
        hunter_rank: HunterRank,
    ) -> Result<u8, UpgradeError> {
        if !skill.is_general() {
            return Err(UpgradeError::NotGeneral);
        }
        if hunter_rank < skill.required_rank() {
            return Err(UpgradeError::RankLocked);
        }
        let rank = self.upgrade(skill, level)?;
        self.selected_general = Some(skill);
        Ok(rank)
    }

    fn upgrade(&mut self, skill: CombatSkill, level: u32) -> Result<u8, UpgradeError> {
        let current = self.ranks.get(&skill).copied().unwrap_or(0);
        if current >= 3 {
            return Err(UpgradeError::MaxRank);
        }
        let next = current + 1;
        if self.available_points(level) < skill.next_cost(next) {
            return Err(UpgradeError::NoPoints);
        }
        self.ranks.insert(skill, next);
        Ok(next)
    }

    pub fn reset(&mut self) {
        self.ranks.retain(|skill, _| !skill.is_general());
        for rank in self.ranks.values_mut() {
            *rank = 1;
        }
        self.selected_general = None;
    }

    #[must_use]
    pub fn ready(&self, skill: CombatSkill, now: u64) -> bool {
        self.cooldowns.get(&skill).is_none_or(|until| *until <= now)
    }

    pub fn start_cooldown(&mut self, skill: CombatSkill, sense: u32, now: u64) {
        let reduction = sense.min(30) as u64;
        let millis = skill.cooldown_seconds() * (100 - reduction) * 10;
        self.cooldowns.insert(skill, now.saturating_add(millis));
    }

    #[must_use]
    pub fn cooldown_millis(&self, skill: CombatSkill, now: u64) -> u64 {
        self.cooldowns
            .get(&skill)
            .map_or(0, |until| until.saturating_sub(now))
    }

    #[must_use]
    pub fn now() -> u64 {
        now_millis()
    }
}

/// Runtime-only effects that must not survive a reconnect.
#[derive(Debug, Clone)]
pub struct CombatSkillRuntime {
    pub flurry: Option<Flurry>,
    pub stealth_until: Option<u64>,
    pub stealth_strike: bool,
    pub taunt_until: Option<u64>,
    pub taunt_absorption: f32,
}

impl Default for CombatSkillRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl CombatSkillRuntime {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            flurry: None,
            stealth_until: None,
            stealth_strike: false,
            taunt_until: None,
            taunt_absorption: 0.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Flurry {
    pub world: steel_utils::Identifier,
    pub targets: Vec<i32>,
    pub damage: f32,
    pub remaining: u8,
    pub next_tick: i32,
}

#[cfg(test)]
mod tests {
    use super::{CombatSkill, CombatSkillBook, UpgradeError};
    use crate::class::HunterClass;
    use crate::rank::HunterRank;

    #[test]
    fn points_cover_existing_levels_and_general_upgrades() {
        let mut book = CombatSkillBook::default();
        assert_eq!(book.available_points(10), 5);
        book.unlock_class(HunterClass::Fighter, HunterRank::D);
        assert_eq!(book.available_points(10), 5);
        assert_eq!(
            book.upgrade_general(CombatSkill::DaggerThrow, 12, HunterRank::E),
            Ok(1)
        );
        assert_eq!(book.available_points(12), 5);
        assert_eq!(
            book.upgrade_general(CombatSkill::DaggerThrow, 12, HunterRank::E),
            Ok(2)
        );
        assert_eq!(book.available_points(12), 3);
        assert_eq!(
            book.upgrade_general(CombatSkill::DaggerThrow, 12, HunterRank::E),
            Ok(3)
        );
        assert_eq!(book.available_points(12), 0);
        assert_eq!(
            book.upgrade_general(CombatSkill::DaggerThrow, 12, HunterRank::E),
            Err(UpgradeError::MaxRank)
        );
    }

    #[test]
    fn reset_refunds_paid_ranks_but_keeps_class_and_cooldowns() {
        let mut book = CombatSkillBook::default();
        book.unlock_class(HunterClass::Tanker, HunterRank::D);
        assert_eq!(
            book.upgrade_class(HunterClass::Tanker, 12, HunterRank::D),
            Ok(2)
        );
        assert_eq!(
            book.upgrade_general(CombatSkill::DaggerThrow, 12, HunterRank::E),
            Ok(1)
        );
        book.start_cooldown(CombatSkill::DaggerThrow, 0, 100);
        book.reset();
        assert_eq!(book.class_rank(HunterClass::Tanker), 1);
        assert_eq!(book.general_rank(CombatSkill::DaggerThrow), 0);
        assert_eq!(book.selected_general, None);
        assert_eq!(book.available_points(12), 6);
        assert!(!book.ready(CombatSkill::DaggerThrow, 101));
    }

    #[test]
    fn sense_reduction_is_capped_and_serialization_round_trips() {
        let mut book = CombatSkillBook::default();
        book.start_cooldown(CombatSkill::Taunt, 50, 1_000);
        assert_eq!(book.cooldown_millis(CombatSkill::Taunt, 1_000), 14_000);
        let encoded = serde_json::to_string(&book).expect("serialize");
        let restored: CombatSkillBook = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(restored, book);
        let old: CombatSkillBook = serde_json::from_str("{}").expect("old save");
        assert_eq!(old, CombatSkillBook::default());
    }
}
