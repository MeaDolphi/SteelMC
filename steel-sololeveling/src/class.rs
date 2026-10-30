//! Hunter classes.
//!
//! The five roles are canonical hunter classes of Solo Leveling: Fighter,
//! Tanker, Assassin, Ranger and Mage. The Healer of the source is folded into
//! the Mage, whose grimoire carries a school of healing; saves that still name
//! the Healer read as no class with one free change. The source describes roles, not
//! numbers, so every bonus below is a server adaptation for Minecraft balance.
//!
//! A hunter picks a class from rank D. The first pick is free; afterwards the
//! class may change once per seven days for [`CHANGE_PRICE`] Won.
//!
//! The constants below are the rank-D values; [`rank_scale`] grows them with
//! the hunter's rank up to twice as much at S, so a class keeps pace with the
//! gates it is fighting.

use serde::{Deserialize, Serialize};
use steel_core::entity::attribute::{AttributeMap, AttributeModifier, AttributeModifierOperation};
use steel_registry::attribute::AttributeRef;
use steel_registry::vanilla_attributes;

use crate::economy::Won;
use crate::identifier::from_sololeveling;
use crate::rank::HunterRank;

/// Lowest hunter rank that may pick a class.
pub const UNLOCK_RANK: HunterRank = HunterRank::D;
/// Seconds between two class changes.
pub const CHANGE_COOLDOWN_SECONDS: u64 = 7 * 24 * 60 * 60;
/// Won charged for every change after the first pick.
pub const CHANGE_PRICE: u64 = 1_000;

/// Fighter bonus to melee damage, as a fraction.
pub const FIGHTER_MELEE_BONUS: f32 = 0.25;
/// Fighter bonus to attack speed, as a fraction of the base.
pub const FIGHTER_ATTACK_SPEED: f64 = 0.15;
/// Share of the Fighter's melee damage returned as health.
pub const FIGHTER_LIFESTEAL: f32 = 0.05;
/// Tanker bonus to maximum health, in half-hearts.
pub const TANKER_MAX_HEALTH: f64 = 8.0;
/// Tanker reduction of incoming damage, as a fraction.
pub const TANKER_DAMAGE_REDUCTION: f32 = 0.20;
/// Highest Tanker damage reduction at any rank.
pub const TANKER_MAX_REDUCTION: f32 = 0.35;
/// Tanker knockback resistance; it does not scale with rank.
pub const TANKER_KNOCKBACK_RESISTANCE: f64 = 0.5;
/// Assassin bonus to movement speed, as a fraction.
pub const ASSASSIN_SPEED: f64 = 0.15;
/// Assassin bonus to every melee hit, as a fraction.
pub const ASSASSIN_MELEE_BONUS: f32 = 0.10;
/// Assassin bonus to melee damage from behind the target, as a fraction.
pub const ASSASSIN_BACKSTAB_BONUS: f32 = 0.60;
/// Ranger bonus to projectile damage, as a fraction.
pub const RANGER_PROJECTILE_BONUS: f32 = 0.40;
/// Ranger bonus to movement speed, as a fraction.
pub const RANGER_SPEED: f64 = 0.10;
/// Mage bonus to magic damage the hunter causes, as a fraction.
pub const MAGE_MAGIC_BONUS: f32 = 0.50;

/// Returns how much of the rank-D bonuses a hunter of `rank` gets.
#[must_use]
pub const fn rank_scale(rank: HunterRank) -> f32 {
    match rank {
        HunterRank::E | HunterRank::D => 1.0,
        HunterRank::C => 1.25,
        HunterRank::B => 1.5,
        HunterRank::A => 1.75,
        HunterRank::S => 2.0,
    }
}

/// Returns the Tanker's reduction of incoming damage at `rank`.
#[must_use]
pub fn tanker_reduction(rank: HunterRank) -> f32 {
    (TANKER_DAMAGE_REDUCTION * rank_scale(rank)).min(TANKER_MAX_REDUCTION)
}

/// Formats a fraction as whole percent.
fn percent(fraction: f32) -> u32 {
    (fraction * 100.0).round() as u32
}

/// One hunter class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HunterClass {
    /// Melee damage dealer.
    Fighter,
    /// Frontline defender.
    Tanker,
    /// Fast striker who hits hardest from behind.
    Assassin,
    /// Archer.
    Ranger,
    /// Spellcaster: destruction, healing, control and utility magic.
    Mage,
}

/// Every class, in menu order.
pub const ALL: [HunterClass; 5] = [
    HunterClass::Fighter,
    HunterClass::Tanker,
    HunterClass::Assassin,
    HunterClass::Ranger,
    HunterClass::Mage,
];

impl HunterClass {
    /// Returns the Russian display name.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Fighter => "Боец",
            Self::Tanker => "Танк",
            Self::Assassin => "Убийца",
            Self::Ranger => "Стрелок",
            Self::Mage => "Маг",
        }
    }

    /// Returns the stable command argument of the class.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Fighter => "fighter",
            Self::Tanker => "tanker",
            Self::Assassin => "assassin",
            Self::Ranger => "ranger",
            Self::Mage => "mage",
        }
    }

    /// Parses a command argument, English or Russian.
    #[must_use]
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.to_lowercase();
        ALL.into_iter()
            .find(|class| class.name() == input || class.display_name().to_lowercase() == input)
    }

    /// Returns the Russian summary of the class bonuses at `rank`.
    #[must_use]
    pub fn describe(self, rank: HunterRank) -> Vec<String> {
        let scale = rank_scale(rank);
        let wide = f64::from(scale);
        match self {
            Self::Fighter => vec![
                format!(
                    "+{}% урона в ближнем бою",
                    percent(FIGHTER_MELEE_BONUS * scale)
                ),
                format!(
                    "+{}% скорости атаки",
                    percent((FIGHTER_ATTACK_SPEED * wide) as f32)
                ),
                format!(
                    "Лечение на {}% нанесённого урона",
                    percent(FIGHTER_LIFESTEAL * scale)
                ),
            ],
            Self::Tanker => vec![
                format!("+{} максимального здоровья", TANKER_MAX_HEALTH * wide),
                format!("-{}% входящего урона", percent(tanker_reduction(rank))),
                format!(
                    "+{}% устойчивости к отбрасыванию",
                    percent(TANKER_KNOCKBACK_RESISTANCE as f32)
                ),
            ],
            Self::Assassin => vec![
                format!("+{}% скорости", percent((ASSASSIN_SPEED * wide) as f32)),
                format!(
                    "+{}% урона в ближнем бою",
                    percent(ASSASSIN_MELEE_BONUS * scale)
                ),
                format!(
                    "+{}% урона в спину",
                    percent(ASSASSIN_BACKSTAB_BONUS * scale)
                ),
            ],
            Self::Ranger => vec![
                format!(
                    "+{}% урона снарядами",
                    percent(RANGER_PROJECTILE_BONUS * scale)
                ),
                format!("+{}% скорости", percent((RANGER_SPEED * wide) as f32)),
            ],
            Self::Mage => vec![
                format!("+{}% магического урона", percent(MAGE_MAGIC_BONUS * scale)),
                "Мана и гримуар заклинаний; мана растёт от Интеллекта".to_owned(),
            ],
        }
    }

    /// Attribute bonuses of the class at `rank`, applied while it is chosen.
    ///
    /// Every class touches the same attributes at every rank, so removing a
    /// class needs no rank.
    fn attributes(self, rank: HunterRank) -> Vec<ClassModifier> {
        let scale = f64::from(rank_scale(rank));
        match self {
            Self::Fighter => vec![(
                vanilla_attributes::ATTACK_SPEED,
                FIGHTER_ATTACK_SPEED * scale,
                AttributeModifierOperation::AddMultipliedBase,
            )],
            Self::Tanker => vec![
                (
                    vanilla_attributes::MAX_HEALTH,
                    TANKER_MAX_HEALTH * scale,
                    AttributeModifierOperation::AddValue,
                ),
                (
                    vanilla_attributes::KNOCKBACK_RESISTANCE,
                    TANKER_KNOCKBACK_RESISTANCE,
                    AttributeModifierOperation::AddValue,
                ),
            ],
            Self::Assassin => vec![(
                vanilla_attributes::MOVEMENT_SPEED,
                ASSASSIN_SPEED * scale,
                AttributeModifierOperation::AddMultipliedBase,
            )],
            Self::Ranger => vec![(
                vanilla_attributes::MOVEMENT_SPEED,
                RANGER_SPEED * scale,
                AttributeModifierOperation::AddMultipliedBase,
            )],
            Self::Mage => Vec::new(),
        }
    }
}

/// One attribute bonus of a class: attribute, amount and operation.
type ClassModifier = (AttributeRef, f64, AttributeModifierOperation);

/// Why a class cannot be chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassError {
    /// The hunter is below [`UNLOCK_RANK`].
    RankTooLow,
    /// The hunter already has this class.
    AlreadyChosen,
    /// The last change was less than seven days ago.
    TooSoon {
        /// Seconds until the next change.
        seconds_left: u64,
    },
    /// The hunter cannot pay [`CHANGE_PRICE`].
    InsufficientFunds,
    /// Storage is unavailable or a payment is still committing.
    Storage,
}

impl ClassError {
    /// Returns the Russian message shown to the hunter.
    #[must_use]
    pub fn message(self) -> String {
        match self {
            Self::RankTooLow => "Класс доступен с ранга D.".to_owned(),
            Self::AlreadyChosen => "Этот класс уже выбран.".to_owned(),
            Self::TooSoon { seconds_left } => format!(
                "Сменить класс можно через {} ч.",
                seconds_left.div_ceil(3600)
            ),
            Self::InsufficientFunds => format!("Смена класса стоит {CHANGE_PRICE} Won."),
            Self::Storage => "Хранилище занято или недоступно: повторите позже.".to_owned(),
        }
    }
}

/// A hunter's chosen class and when it last changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "SavedClassChoice")]
pub struct ClassChoice {
    /// The chosen class, `None` before the first pick.
    pub class: Option<HunterClass>,
    /// Unix second of the last change.
    #[serde(default)]
    pub changed_at: u64,
    /// The next change skips the wait and the price; granted to hunters
    /// whose removed class was dropped.
    #[serde(default)]
    pub free_change: bool,
}

/// [`ClassChoice`] as saved, where the class may name a removed class.
#[derive(Deserialize)]
struct SavedClassChoice {
    #[serde(default)]
    class: Option<String>,
    #[serde(default)]
    changed_at: u64,
    #[serde(default)]
    free_change: bool,
}

impl From<SavedClassChoice> for ClassChoice {
    fn from(saved: SavedClassChoice) -> Self {
        let class = saved.class.as_deref().and_then(HunterClass::parse);
        Self {
            class,
            changed_at: saved.changed_at,
            // A class that no longer exists, the Healer, frees the next pick.
            free_change: saved.free_change || (saved.class.is_some() && class.is_none()),
        }
    }
}

impl ClassChoice {
    /// Picks or changes the class, debiting `won` for a change.
    ///
    /// Returns the Won charged. Nothing changes on an error.
    pub fn choose(
        &mut self,
        class: HunterClass,
        rank: HunterRank,
        now: u64,
        won: &mut Won,
    ) -> Result<u64, ClassError> {
        if rank < UNLOCK_RANK {
            return Err(ClassError::RankTooLow);
        }
        if self.class == Some(class) {
            return Err(ClassError::AlreadyChosen);
        }
        let price = if self.class.is_some() && !self.free_change {
            let ready = self.changed_at.saturating_add(CHANGE_COOLDOWN_SECONDS);
            if now < ready {
                return Err(ClassError::TooSoon {
                    seconds_left: ready - now,
                });
            }
            if !won.debit(CHANGE_PRICE) {
                return Err(ClassError::InsufficientFunds);
            }
            CHANGE_PRICE
        } else {
            0
        };
        self.class = Some(class);
        self.changed_at = now;
        self.free_change = false;
        Ok(price)
    }

    /// Writes the class attribute modifiers for `rank`, removing those of
    /// other classes.
    pub fn apply_to(&self, attributes: &mut AttributeMap, rank: HunterRank) {
        for class in ALL {
            for (attribute, amount, operation) in class.attributes(rank) {
                let id = from_sololeveling(format!("class/{}", class.name()));
                if self.class == Some(class) {
                    attributes.set_modifier(
                        attribute,
                        AttributeModifier {
                            id,
                            amount,
                            operation,
                        },
                        false,
                    );
                } else {
                    attributes.remove_modifier(attribute, &id);
                }
            }
        }
    }
}

/// Returns whether an attacker at `attacker` stands behind a target at
/// `target` looking along `target_look`.
#[must_use]
pub fn is_behind(target: glam::DVec3, target_look: glam::DVec3, attacker: glam::DVec3) -> bool {
    let to_attacker = glam::DVec3::new(attacker.x - target.x, 0.0, attacker.z - target.z);
    let look = glam::DVec3::new(target_look.x, 0.0, target_look.z);
    to_attacker.length_squared() > f64::EPSILON && look.dot(to_attacker) < 0.0
}

#[cfg(test)]
mod tests {
    use glam::DVec3;

    use super::*;

    #[test]
    fn the_first_pick_is_free_and_changes_wait_a_week_and_cost_won() {
        let mut choice = ClassChoice::default();
        let mut won = Won::zero();
        assert_eq!(
            choice.choose(HunterClass::Mage, HunterRank::E, 0, &mut won),
            Err(ClassError::RankTooLow)
        );
        assert_eq!(
            choice.choose(HunterClass::Mage, HunterRank::D, 100, &mut won),
            Ok(0)
        );
        assert_eq!(
            choice.choose(HunterClass::Mage, HunterRank::D, 200, &mut won),
            Err(ClassError::AlreadyChosen)
        );
        assert!(matches!(
            choice.choose(HunterClass::Tanker, HunterRank::D, 200, &mut won),
            Err(ClassError::TooSoon { .. })
        ));
        let later = 100 + CHANGE_COOLDOWN_SECONDS;
        assert_eq!(
            choice.choose(HunterClass::Tanker, HunterRank::D, later, &mut won),
            Err(ClassError::InsufficientFunds)
        );
        won.credit(CHANGE_PRICE);
        assert_eq!(
            choice.choose(HunterClass::Tanker, HunterRank::D, later, &mut won),
            Ok(CHANGE_PRICE)
        );
        assert_eq!(won.amount(), 0);
        assert_eq!(choice.class, Some(HunterClass::Tanker));
    }

    #[test]
    fn bonuses_double_from_rank_d_to_s_and_the_tank_is_capped() {
        assert!((rank_scale(HunterRank::D) - 1.0).abs() < f32::EPSILON);
        assert!((rank_scale(HunterRank::S) - 2.0).abs() < f32::EPSILON);
        assert!((tanker_reduction(HunterRank::D) - 0.20).abs() < 1e-6);
        assert!((tanker_reduction(HunterRank::S) - TANKER_MAX_REDUCTION).abs() < 1e-6);
        assert_eq!(
            HunterClass::Fighter.describe(HunterRank::S)[0],
            "+50% урона в ближнем бою"
        );
        for class in ALL {
            let at_d: Vec<_> = class
                .attributes(HunterRank::D)
                .iter()
                .map(|(attribute, ..)| attribute.key.clone())
                .collect();
            let at_s: Vec<_> = class
                .attributes(HunterRank::S)
                .iter()
                .map(|(attribute, ..)| attribute.key.clone())
                .collect();
            assert_eq!(at_d, at_s, "{class:?} must touch the same attributes");
        }
    }

    #[test]
    fn a_saved_healer_becomes_no_class_with_a_free_change() {
        let choice: ClassChoice =
            serde_json::from_str(r#"{"class":"healer","changed_at":500}"#).expect("old save");
        assert_eq!(choice.class, None);
        assert!(choice.free_change);
        let mage: ClassChoice =
            serde_json::from_str(r#"{"class":"mage","changed_at":500}"#).expect("save");
        assert_eq!(mage.class, Some(HunterClass::Mage));
        assert!(!mage.free_change);
        let round: ClassChoice =
            serde_json::from_str(&serde_json::to_string(&mage).expect("encode")).expect("decode");
        assert_eq!(round, mage);
    }

    #[test]
    fn a_free_change_skips_the_wait_and_the_price_once() {
        let mut choice = ClassChoice {
            class: Some(HunterClass::Mage),
            changed_at: 100,
            free_change: true,
        };
        let mut won = Won::zero();
        assert_eq!(
            choice.choose(HunterClass::Tanker, HunterRank::D, 101, &mut won),
            Ok(0)
        );
        assert!(!choice.free_change);
    }

    #[test]
    fn class_names_parse_in_both_languages() {
        for class in ALL {
            assert_eq!(HunterClass::parse(class.name()), Some(class));
            assert_eq!(HunterClass::parse(class.display_name()), Some(class));
        }
        assert_eq!(HunterClass::parse("necromancer"), None);
    }

    #[test]
    fn behind_means_opposite_the_target_look() {
        let target = DVec3::ZERO;
        let look = DVec3::new(0.0, 0.0, 1.0);
        assert!(is_behind(target, look, DVec3::new(0.0, 0.0, -2.0)));
        assert!(!is_behind(target, look, DVec3::new(0.0, 0.0, 2.0)));
    }
}
