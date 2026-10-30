//! Artifact enchanting, modelled on Daggerfall's item maker.
//!
//! The concept follows DFO Online's port of Daggerfall Unity's item maker
//! (MIT): every item carries a budget of enchantment points, every power
//! spends points, and every side effect has a *negative* cost, so accepting a
//! drawback buys room for more power. The Won price counts powers only, at ten
//! Won per point, so a drawback buys budget but never a discount. One item
//! holds at most ten enchantments, each at most once.
//!
//! The budget comes from the artifact's rarity instead of Daggerfall's
//! materials, and the effects are a server adaptation: most are attribute
//! modifiers written onto the stack, lifesteal and recoil run on melee hits.
//! Enchanting an artifact never removes its personal binding, and the
//! artifact's own abilities cost none of its points.

use steel_core::entity::attribute::AttributeModifierOperation;
use steel_registry::attribute::AttributeRef;
use steel_registry::vanilla_attributes;

use super::artifact::{Category, Rarity};

/// Most enchantments one item may hold.
pub const MAX_ENCHANTMENTS: usize = 10;

/// Won charged per enchantment point of a power.
pub const WON_PER_POINT: u64 = 10;

/// Health a lifesteal hit restores, as a fraction of the damage dealt.
pub const LIFESTEAL_FRACTION: f32 = 0.1;

/// Health a recoil hit takes from the wielder; recoil never kills.
pub const RECOIL_DAMAGE: f32 = 1.0;

/// Returns the enchantment points an artifact of `rarity` carries.
#[must_use]
pub const fn budget(rarity: Rarity) -> i64 {
    match rarity {
        Rarity::Common => 200,
        Rarity::Rare => 400,
        Rarity::Epic => 800,
        Rarity::Legendary => 1500,
    }
}

/// What an enchantment does once applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Effect {
    /// An attribute modifier on the stack, active in the artifact's slot.
    Attribute {
        /// Vanilla attribute the modifier applies to.
        attribute: AttributeRef,
        /// Modifier amount, interpreted by `operation`.
        amount: f64,
        /// How `amount` combines with the attribute's other modifiers.
        operation: AttributeModifierOperation,
    },
    /// A melee hit heals the wielder by part of the damage dealt.
    Lifesteal,
    /// A melee hit costs the wielder health, never below one.
    Recoil,
}

/// One enchantment the item maker offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Enchantment {
    /// Weapon power: flat attack damage.
    Sharpness,
    /// Weapon power: faster attacks.
    Swiftblade,
    /// Weapon power: heals on hit.
    Lifesteal,
    /// Armor power: flat armor.
    Protection,
    /// Armor power: maximum health.
    Vitality,
    /// Armor power: movement speed.
    Fleetness,
    /// Armor power: knockback resistance.
    Steadfast,
    /// Side effect: slower movement.
    Sluggish,
    /// Side effect: lower maximum health.
    Frail,
    /// Weapon side effect: slower attacks.
    Heavy,
    /// Weapon side effect: hits hurt the wielder.
    Recoil,
}

/// Every enchantment, powers first, in menu order.
pub const ALL: [Enchantment; 11] = [
    Enchantment::Sharpness,
    Enchantment::Swiftblade,
    Enchantment::Lifesteal,
    Enchantment::Protection,
    Enchantment::Vitality,
    Enchantment::Fleetness,
    Enchantment::Steadfast,
    Enchantment::Sluggish,
    Enchantment::Frail,
    Enchantment::Heavy,
    Enchantment::Recoil,
];

impl Enchantment {
    /// Returns the stable tag value of the enchantment.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sharpness => "sharpness",
            Self::Swiftblade => "swiftblade",
            Self::Lifesteal => "lifesteal",
            Self::Protection => "protection",
            Self::Vitality => "vitality",
            Self::Fleetness => "fleetness",
            Self::Steadfast => "steadfast",
            Self::Sluggish => "sluggish",
            Self::Frail => "frail",
            Self::Heavy => "heavy",
            Self::Recoil => "recoil",
        }
    }

    /// Parses the stable tag value.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        ALL.into_iter()
            .find(|enchantment| enchantment.name() == name)
    }

    /// Returns the Russian display name.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Sharpness => "Острота",
            Self::Swiftblade => "Быстрый клинок",
            Self::Lifesteal => "Вампиризм",
            Self::Protection => "Защита",
            Self::Vitality => "Живучесть",
            Self::Fleetness => "Лёгкость",
            Self::Steadfast => "Стойкость",
            Self::Sluggish => "Медлительность",
            Self::Frail => "Хрупкость",
            Self::Heavy => "Тяжесть",
            Self::Recoil => "Отдача",
        }
    }

    /// Returns the enchantment cost in points; side effects are negative.
    #[must_use]
    pub const fn cost(self) -> i64 {
        match self {
            Self::Sharpness | Self::Protection | Self::Fleetness => 150,
            Self::Swiftblade => 120,
            Self::Lifesteal => 300,
            Self::Vitality => 200,
            Self::Steadfast => 100,
            Self::Sluggish | Self::Heavy => -150,
            Self::Frail => -200,
            Self::Recoil => -250,
        }
    }

    /// Returns whether the enchantment is a side effect.
    #[must_use]
    pub const fn is_side_effect(self) -> bool {
        self.cost() < 0
    }

    /// Returns the Won price; side effects are free.
    #[must_use]
    pub const fn price(self) -> u64 {
        if self.cost() > 0 {
            self.cost().unsigned_abs() * WON_PER_POINT
        } else {
            0
        }
    }

    /// Returns whether an artifact of `category` may take the enchantment.
    #[must_use]
    pub const fn fits(self, category: Category) -> bool {
        match self {
            Self::Sharpness | Self::Swiftblade | Self::Lifesteal | Self::Heavy | Self::Recoil => {
                matches!(category, Category::Weapon)
            }
            Self::Protection | Self::Vitality | Self::Fleetness | Self::Steadfast => {
                matches!(category, Category::Armor)
            }
            Self::Sluggish | Self::Frail => true,
        }
    }

    /// Returns what the enchantment does.
    #[must_use]
    pub fn effect(self) -> Effect {
        let attribute = |attribute, amount, operation| Effect::Attribute {
            attribute,
            amount,
            operation,
        };
        match self {
            Self::Sharpness => attribute(
                vanilla_attributes::ATTACK_DAMAGE,
                1.0,
                AttributeModifierOperation::AddValue,
            ),
            Self::Swiftblade => attribute(
                vanilla_attributes::ATTACK_SPEED,
                0.1,
                AttributeModifierOperation::AddMultipliedBase,
            ),
            Self::Protection => attribute(
                vanilla_attributes::ARMOR,
                1.0,
                AttributeModifierOperation::AddValue,
            ),
            Self::Vitality => attribute(
                vanilla_attributes::MAX_HEALTH,
                2.0,
                AttributeModifierOperation::AddValue,
            ),
            Self::Fleetness => attribute(
                vanilla_attributes::MOVEMENT_SPEED,
                0.05,
                AttributeModifierOperation::AddMultipliedBase,
            ),
            Self::Steadfast => attribute(
                vanilla_attributes::KNOCKBACK_RESISTANCE,
                0.1,
                AttributeModifierOperation::AddValue,
            ),
            Self::Sluggish => attribute(
                vanilla_attributes::MOVEMENT_SPEED,
                -0.05,
                AttributeModifierOperation::AddMultipliedBase,
            ),
            Self::Frail => attribute(
                vanilla_attributes::MAX_HEALTH,
                -2.0,
                AttributeModifierOperation::AddValue,
            ),
            Self::Heavy => attribute(
                vanilla_attributes::ATTACK_SPEED,
                -0.1,
                AttributeModifierOperation::AddMultipliedBase,
            ),
            Self::Lifesteal => Effect::Lifesteal,
            Self::Recoil => Effect::Recoil,
        }
    }

    /// Returns a one-line Russian description for tooltips.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Sharpness => "+1 урона атаки",
            Self::Swiftblade => "+10% скорости атаки",
            Self::Lifesteal => "Попадание лечит на 10% нанесённого урона",
            Self::Protection => "+1 брони",
            Self::Vitality => "+2 максимального здоровья",
            Self::Fleetness => "+5% скорости",
            Self::Steadfast => "+10% сопротивления отбрасыванию",
            Self::Sluggish => "-5% скорости",
            Self::Frail => "-2 максимального здоровья",
            Self::Heavy => "-10% скорости атаки",
            Self::Recoil => "Попадание отнимает 1 HP у владельца (не убивает)",
        }
    }
}

/// Why an enchantment cannot be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnchantError {
    /// Only catalog artifacts can be enchanted.
    NotArtifact,
    /// The enchantment does not fit this kind of artifact.
    WrongCategory,
    /// The artifact already has this enchantment.
    AlreadyApplied,
    /// The artifact already holds the maximum number of enchantments.
    TooMany,
    /// The enchantment would exceed the artifact's point budget.
    OverBudget,
    /// The hunter cannot pay the Won price.
    InsufficientFunds,
    /// Storage is unavailable or another payment is still committing.
    Storage,
}

impl EnchantError {
    /// Returns the Russian message shown to the hunter.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::NotArtifact => "Зачаровать можно только артефакт в главной руке.",
            Self::WrongCategory => "Это зачарование не подходит для этого предмета.",
            Self::AlreadyApplied => "Это зачарование уже наложено.",
            Self::TooMany => "На предмете уже 10 зачарований.",
            Self::OverBudget => "Не хватает очков зачарования предмета.",
            Self::InsufficientFunds => "Недостаточно Won.",
            Self::Storage => "Хранилище занято или недоступно: повторите позже.",
        }
    }
}

/// Returns the points the applied enchantments spend; side effects subtract.
#[must_use]
pub fn points_used(applied: &[Enchantment]) -> i64 {
    applied.iter().map(|enchantment| enchantment.cost()).sum()
}

/// Checks whether `next` may be added to an artifact holding `applied`.
///
/// Like Daggerfall's item maker, the price is checked before the budget, so a
/// hunter who can afford neither hears about the money.
pub fn check(
    rarity: Rarity,
    category: Category,
    applied: &[Enchantment],
    next: Enchantment,
    balance: u64,
) -> Result<(), EnchantError> {
    if !next.fits(category) {
        return Err(EnchantError::WrongCategory);
    }
    if applied.contains(&next) {
        return Err(EnchantError::AlreadyApplied);
    }
    if applied.len() >= MAX_ENCHANTMENTS {
        return Err(EnchantError::TooMany);
    }
    if balance < next.price() {
        return Err(EnchantError::InsufficientFunds);
    }
    if points_used(applied) + next.cost() > budget(rarity) {
        return Err(EnchantError::OverBudget);
    }
    Ok(())
}

/// Encodes an enchantment list for the item tag.
#[must_use]
pub fn encode(applied: &[Enchantment]) -> String {
    applied
        .iter()
        .map(|enchantment| enchantment.name())
        .collect::<Vec<_>>()
        .join(",")
}

/// Decodes an enchantment list, skipping unknown names.
#[must_use]
pub fn decode(stored: &str) -> Vec<Enchantment> {
    stored
        .split(',')
        .filter_map(Enchantment::from_name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_effects_buy_budget_but_not_a_discount() {
        let rare = [Enchantment::Lifesteal];
        assert_eq!(
            check(
                Rarity::Rare,
                Category::Weapon,
                &rare,
                Enchantment::Sharpness,
                u64::MAX
            ),
            Err(EnchantError::OverBudget)
        );
        let with_drawback = [Enchantment::Lifesteal, Enchantment::Recoil];
        assert!(
            check(
                Rarity::Rare,
                Category::Weapon,
                &with_drawback,
                Enchantment::Sharpness,
                u64::MAX
            )
            .is_ok()
        );
        assert_eq!(points_used(&with_drawback), 50);
        assert_eq!(Enchantment::Recoil.price(), 0);
        assert_eq!(Enchantment::Sharpness.price(), 1500);
    }

    #[test]
    fn the_price_is_checked_before_the_budget() {
        let full = [Enchantment::Lifesteal];
        assert_eq!(
            check(
                Rarity::Rare,
                Category::Weapon,
                &full,
                Enchantment::Sharpness,
                0
            ),
            Err(EnchantError::InsufficientFunds)
        );
    }

    #[test]
    fn categories_duplicates_and_the_limit_are_enforced() {
        assert_eq!(
            check(
                Rarity::Epic,
                Category::Armor,
                &[],
                Enchantment::Sharpness,
                u64::MAX
            ),
            Err(EnchantError::WrongCategory)
        );
        assert_eq!(
            check(
                Rarity::Epic,
                Category::Armor,
                &[Enchantment::Frail],
                Enchantment::Frail,
                u64::MAX
            ),
            Err(EnchantError::AlreadyApplied)
        );
        let ten = [Enchantment::Sluggish; MAX_ENCHANTMENTS];
        assert_eq!(
            check(
                Rarity::Legendary,
                Category::Armor,
                &ten,
                Enchantment::Frail,
                u64::MAX
            ),
            Err(EnchantError::TooMany)
        );
    }

    #[test]
    fn a_list_round_trips_through_its_tag() {
        let applied = vec![Enchantment::Protection, Enchantment::Frail];
        assert_eq!(decode(&encode(&applied)), applied);
        assert_eq!(decode(""), Vec::<Enchantment>::new());
    }
}
