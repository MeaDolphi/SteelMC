//! Hunter classes.
//!
//! The six roles are the canonical hunter classes of Solo Leveling: Fighter,
//! Tanker, Assassin, Ranger, Mage and Healer. The source describes roles, not
//! numbers, so every bonus below is a server adaptation for Minecraft balance.
//!
//! A hunter picks a class from rank D. The first pick is free; afterwards the
//! class may change once per seven days for [`CHANGE_PRICE`] Won.

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
pub const FIGHTER_MELEE_BONUS: f32 = 0.10;
/// Tanker bonus to maximum health, in half-hearts.
pub const TANKER_MAX_HEALTH: f64 = 4.0;
/// Tanker reduction of incoming damage, as a fraction.
pub const TANKER_DAMAGE_REDUCTION: f32 = 0.10;
/// Assassin bonus to movement speed, as a fraction.
pub const ASSASSIN_SPEED: f64 = 0.10;
/// Assassin bonus to melee damage from behind the target, as a fraction.
pub const ASSASSIN_BACKSTAB_BONUS: f32 = 0.30;
/// Ranger bonus to projectile damage, as a fraction.
pub const RANGER_PROJECTILE_BONUS: f32 = 0.20;
/// Mage bonus to magic damage the hunter causes, as a fraction.
pub const MAGE_MAGIC_BONUS: f32 = 0.25;
/// Health a Healer restores to each nearby party member per pulse.
pub const HEALER_PULSE_HEALTH: f32 = 1.0;
/// Milliseconds between two Healer pulses.
pub const HEALER_PULSE_MILLIS: u64 = 5_000;
/// Radius of a Healer pulse in blocks.
pub const HEALER_RADIUS: f64 = 8.0;

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
    /// Magic damage dealer.
    Mage,
    /// Keeps the party alive.
    Healer,
}

/// Every class, in menu order.
pub const ALL: [HunterClass; 6] = [
    HunterClass::Fighter,
    HunterClass::Tanker,
    HunterClass::Assassin,
    HunterClass::Ranger,
    HunterClass::Mage,
    HunterClass::Healer,
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
            Self::Healer => "Целитель",
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
            Self::Healer => "healer",
        }
    }

    /// Parses a command argument, English or Russian.
    #[must_use]
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.to_lowercase();
        ALL.into_iter()
            .find(|class| class.name() == input || class.display_name().to_lowercase() == input)
    }

    /// Returns a one-line Russian summary of the class bonus.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Fighter => "+10% урона в ближнем бою",
            Self::Tanker => "+4 максимального здоровья, -10% входящего урона",
            Self::Assassin => "+10% скорости, +30% урона в спину",
            Self::Ranger => "+20% урона снарядами",
            Self::Mage => "+25% магического урона (буря и способности)",
            Self::Healer => "Каждые 5 с лечит группу в радиусе 8 блоков на 1 HP",
        }
    }

    /// Attribute bonuses of the class, applied while it is chosen.
    fn attributes(self) -> &'static [ClassModifier] {
        match self {
            Self::Tanker => &TANKER_MODIFIERS,
            Self::Assassin => &ASSASSIN_MODIFIERS,
            Self::Fighter | Self::Ranger | Self::Mage | Self::Healer => &[],
        }
    }
}

/// One attribute bonus of a class: attribute, amount and operation.
type ClassModifier = (AttributeRef, f64, AttributeModifierOperation);

static TANKER_MODIFIERS: [ClassModifier; 1] = [(
    vanilla_attributes::MAX_HEALTH,
    TANKER_MAX_HEALTH,
    AttributeModifierOperation::AddValue,
)];
static ASSASSIN_MODIFIERS: [ClassModifier; 1] = [(
    vanilla_attributes::MOVEMENT_SPEED,
    ASSASSIN_SPEED,
    AttributeModifierOperation::AddMultipliedBase,
)];

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
pub struct ClassChoice {
    /// The chosen class, `None` before the first pick.
    pub class: Option<HunterClass>,
    /// Unix second of the last change.
    #[serde(default)]
    pub changed_at: u64,
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
        let price = if self.class.is_some() {
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
        Ok(price)
    }

    /// Writes the class attribute modifiers, removing those of other classes.
    pub fn apply_to(&self, attributes: &mut AttributeMap) {
        for class in ALL {
            for (attribute, amount, operation) in class.attributes() {
                let id = from_sololeveling(format!("class/{}", class.name()));
                if self.class == Some(class) {
                    attributes.set_modifier(
                        attribute,
                        AttributeModifier {
                            id,
                            amount: *amount,
                            operation: *operation,
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
