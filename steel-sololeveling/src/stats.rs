//! Hunter stats (Strength, Vitality, Agility, Intelligence, Sense), level,
//! experience, and their application to vanilla combat/movement attributes.
//!
//! Each stat point is applied as a percentage [`AttributeModifier`] on the
//! relevant *vanilla* attribute (`generic.attack_damage`, `generic.max_health`,
//! `generic.movement_speed`, `generic.attack_speed`) using
//! [`AttributeModifierOperation::AddMultipliedTotal`]. Hunter stats
//! themselves are not registered as new attributes in `steel-core`'s
//! attribute registry; they are Solo Leveling-owned data, and this module is
//! the only place that translates them into vanilla attribute modifiers.

use serde::{Deserialize, Serialize};
use steel_core::entity::attribute::{AttributeMap, AttributeModifier, AttributeModifierOperation};
use steel_registry::attribute::AttributeRef;
use steel_registry::vanilla_attributes;

use crate::identifier::from_sololeveling;
use crate::rank::HunterRank;

/// Experience required to advance from level 1 to level 2.
pub const INITIAL_LEVEL_EXPERIENCE: u64 = 100;

/// Additional unspent stat points granted for every level gained.
pub const STAT_POINTS_PER_LEVEL: u32 = 3;

/// Percentage bonus to `generic.attack_damage` per Strength point, as a
/// fraction (`0.01` = +1%).
pub const STRENGTH_ATTACK_DAMAGE_PERCENT_PER_POINT: f64 = 0.01;

/// Percentage bonus to `generic.max_health` per Vitality point, as a
/// fraction (`0.01` = +1%).
pub const VITALITY_MAX_HEALTH_PERCENT_PER_POINT: f64 = 0.01;

/// Percentage bonus to `generic.movement_speed` per Agility point, as a
/// fraction (`0.005` = +0.5%).
pub const AGILITY_MOVEMENT_SPEED_PERCENT_PER_POINT: f64 = 0.005;

/// Percentage bonus to `generic.attack_speed` per Agility point, as a
/// fraction (`0.005` = +0.5%).
pub const AGILITY_ATTACK_SPEED_PERCENT_PER_POINT: f64 = 0.005;

/// One allocatable hunter stat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunterStat {
    /// Strength: increases vanilla attack damage.
    Strength,
    /// Vitality: increases vanilla maximum health.
    Vitality,
    /// Agility: increases vanilla movement and attack speed.
    Agility,
    /// Intelligence: reserved for future skill systems.
    Intelligence,
    /// Sense: reserved for future sensing systems.
    Sense,
}

/// A hunter's allocatable stats, level, and experience.
///
/// Intelligence and Sense are tracked but have no mechanical effect yet;
/// they are reserved for future systems (Shadow Monarch skills, dungeon/gate
/// sensing) and are not applied as attribute modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HunterStats {
    /// Hunter level.
    pub level: u32,
    /// Accumulated experience toward the next level.
    pub experience: u64,
    /// Stat points not yet allocated to a stat.
    pub unspent_points: u32,
    /// Strength: increases `generic.attack_damage`.
    pub strength: u32,
    /// Vitality: increases `generic.max_health`.
    pub vitality: u32,
    /// Agility: increases `generic.movement_speed` and `generic.attack_speed`.
    pub agility: u32,
    /// Intelligence: no mechanical effect yet, reserved for future systems.
    pub intelligence: u32,
    /// Sense: no mechanical effect yet, reserved for future systems.
    pub sense: u32,
}

impl HunterStats {
    /// Starting stats for a new hunter: level 1, no experience, no points
    /// allocated or unspent.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            level: 1,
            experience: 0,
            unspent_points: 0,
            strength: 0,
            vitality: 0,
            agility: 0,
            intelligence: 0,
            sense: 0,
        }
    }

    /// Applies this hunter's stats to a player's attribute map as vanilla
    /// attribute modifiers, replacing any previously applied Solo Leveling
    /// modifiers.
    ///
    /// Call this whenever a hunter's `strength`, `vitality`, or `agility`
    /// changes (stat allocation, level up, join), so the player's vanilla
    /// attributes stay in sync.
    pub fn apply_to(&self, attributes: &mut AttributeMap) {
        set_percent_modifier(
            attributes,
            vanilla_attributes::ATTACK_DAMAGE,
            "strength_attack_damage",
            f64::from(self.strength) * STRENGTH_ATTACK_DAMAGE_PERCENT_PER_POINT,
        );
        set_percent_modifier(
            attributes,
            vanilla_attributes::MAX_HEALTH,
            "vitality_max_health",
            f64::from(self.vitality) * VITALITY_MAX_HEALTH_PERCENT_PER_POINT,
        );
        set_percent_modifier(
            attributes,
            vanilla_attributes::MOVEMENT_SPEED,
            "agility_movement_speed",
            f64::from(self.agility) * AGILITY_MOVEMENT_SPEED_PERCENT_PER_POINT,
        );
        set_percent_modifier(
            attributes,
            vanilla_attributes::ATTACK_SPEED,
            "agility_attack_speed",
            f64::from(self.agility) * AGILITY_ATTACK_SPEED_PERCENT_PER_POINT,
        );
    }

    /// Increments one allocatable stat by one point.
    pub const fn increment(&mut self, stat: HunterStat) {
        match stat {
            HunterStat::Strength => self.strength += 1,
            HunterStat::Vitality => self.vitality += 1,
            HunterStat::Agility => self.agility += 1,
            HunterStat::Intelligence => self.intelligence += 1,
            HunterStat::Sense => self.sense += 1,
        }
    }

    /// Removes one point from one of the nonzero allocated stats.
    ///
    /// `roll` selects among the eligible values, keeping selection policy at
    /// the gameplay boundary while preserving this type's mutation rule.
    pub fn decrement_nonzero_for_roll(&mut self, roll: usize) -> Option<HunterStat> {
        let eligible = [
            (HunterStat::Strength, self.strength),
            (HunterStat::Vitality, self.vitality),
            (HunterStat::Agility, self.agility),
            (HunterStat::Intelligence, self.intelligence),
            (HunterStat::Sense, self.sense),
        ];
        let mut remaining = roll
            % eligible
                .iter()
                .filter(|(_, value)| *value > 0)
                .count()
                .max(1);
        for (stat, value) in eligible {
            if value == 0 {
                continue;
            }
            if remaining != 0 {
                remaining -= 1;
                continue;
            }
            match stat {
                HunterStat::Strength => self.strength -= 1,
                HunterStat::Vitality => self.vitality -= 1,
                HunterStat::Agility => self.agility -= 1,
                HunterStat::Intelligence => self.intelligence -= 1,
                HunterStat::Sense => self.sense -= 1,
            }
            return Some(stat);
        }
        None
    }

    /// Returns this hunter's automatic rank.
    #[must_use]
    pub const fn rank(&self) -> HunterRank {
        HunterRank::from_level(self.level)
    }

    /// Returns experience required to advance from this level to the next.
    ///
    /// The level-one requirement is 100 XP. Every later level costs 110% of
    /// the preceding requirement, rounded up so it always grows.
    #[must_use]
    pub const fn experience_to_next_level(&self) -> u64 {
        experience_required_for_level(self.level)
    }

    /// Grants experience and applies every resulting level-up.
    ///
    /// Experience beyond a level threshold carries toward the next level.
    /// Each gained level grants [`STAT_POINTS_PER_LEVEL`] unspent points.
    pub const fn grant_experience(&mut self, amount: u64) -> LevelUp {
        self.experience = self.experience.saturating_add(amount);
        let initial_level = self.level;
        while self.level != u32::MAX {
            let required = self.experience_to_next_level();
            if self.experience < required {
                break;
            }
            self.experience -= required;
            self.level += 1;
            self.unspent_points = self.unspent_points.saturating_add(STAT_POINTS_PER_LEVEL);
        }
        LevelUp {
            levels_gained: self.level - initial_level,
            new_rank: self.rank(),
        }
    }
}

/// Outcome of applying experience to a hunter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelUp {
    /// Number of levels gained.
    pub levels_gained: u32,
    /// Rank after the experience was applied.
    pub new_rank: HunterRank,
}

/// Returns the requirement to advance from `level` to `level + 1`.
#[must_use]
pub const fn experience_required_for_level(level: u32) -> u64 {
    let mut required = INITIAL_LEVEL_EXPERIENCE;
    let mut current_level = 1;
    while current_level < level {
        required = increase_requirement(required);
        current_level += 1;
    }
    required
}

const fn increase_requirement(required: u64) -> u64 {
    let Some(product) = required.checked_mul(110) else {
        return u64::MAX;
    };
    product / 100 + if product % 100 == 0 { 0 } else { 1 }
}

impl Default for HunterStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Sets (replacing any existing) a persistent percentage modifier identified
/// by `modifier_path` on `attribute`, in the `sololeveling` namespace.
///
/// A `percent` of `0.0` still sets a (no-op) modifier rather than removing
/// it, keeping the attribute's modifier set stable across stat changes.
fn set_percent_modifier(
    attributes: &mut AttributeMap,
    attribute: AttributeRef,
    modifier_path: &'static str,
    percent: f64,
) {
    attributes.set_modifier(
        attribute,
        AttributeModifier {
            id: from_sololeveling(modifier_path),
            amount: percent,
            operation: AttributeModifierOperation::AddMultipliedTotal,
        },
        true,
    );
}

#[cfg(test)]
mod tests {
    use steel_core::bootstrap::init_globals;
    use steel_core::entity::attribute::AttributeMap;
    use steel_registry::vanilla_entities;

    use super::*;

    fn player_attributes() -> AttributeMap {
        init_globals();
        AttributeMap::new_for_entity(&vanilla_entities::PLAYER)
    }

    #[test]
    fn zero_stats_leave_base_attribute_values_unchanged() {
        let mut attributes = player_attributes();
        let base_damage = attributes.required_value(vanilla_attributes::ATTACK_DAMAGE);

        HunterStats::new().apply_to(&mut attributes);

        assert!(
            (attributes.required_value(vanilla_attributes::ATTACK_DAMAGE) - base_damage).abs()
                < f64::EPSILON
        );
    }

    #[test]
    fn strength_increases_attack_damage_by_configured_percent_per_point() {
        let mut attributes = player_attributes();
        let base_damage = attributes.required_value(vanilla_attributes::ATTACK_DAMAGE);

        let stats = HunterStats {
            strength: 10,
            ..HunterStats::new()
        };
        stats.apply_to(&mut attributes);

        let expected = base_damage * (1.0 + 10.0 * STRENGTH_ATTACK_DAMAGE_PERCENT_PER_POINT);
        assert!(
            (attributes.required_value(vanilla_attributes::ATTACK_DAMAGE) - expected).abs() < 1e-9
        );
    }

    #[test]
    fn agility_increases_both_movement_and_attack_speed() {
        let mut attributes = player_attributes();
        let base_speed = attributes.required_value(vanilla_attributes::MOVEMENT_SPEED);
        let base_attack_speed = attributes.required_value(vanilla_attributes::ATTACK_SPEED);

        let stats = HunterStats {
            agility: 20,
            ..HunterStats::new()
        };
        stats.apply_to(&mut attributes);

        let expected_speed = base_speed * (1.0 + 20.0 * AGILITY_MOVEMENT_SPEED_PERCENT_PER_POINT);
        let expected_attack_speed =
            base_attack_speed * (1.0 + 20.0 * AGILITY_ATTACK_SPEED_PERCENT_PER_POINT);
        assert!(
            (attributes.required_value(vanilla_attributes::MOVEMENT_SPEED) - expected_speed).abs()
                < 1e-9
        );
        assert!(
            (attributes.required_value(vanilla_attributes::ATTACK_SPEED) - expected_attack_speed)
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn reapplying_stats_replaces_rather_than_stacks_modifiers() {
        let mut attributes = player_attributes();
        let base_damage = attributes.required_value(vanilla_attributes::ATTACK_DAMAGE);

        let mut stats = HunterStats {
            strength: 5,
            ..HunterStats::new()
        };
        stats.apply_to(&mut attributes);
        stats.strength = 10;
        stats.apply_to(&mut attributes);

        let expected = base_damage * (1.0 + 10.0 * STRENGTH_ATTACK_DAMAGE_PERCENT_PER_POINT);
        assert!(
            (attributes.required_value(vanilla_attributes::ATTACK_DAMAGE) - expected).abs() < 1e-9
        );
    }

    #[test]
    fn experience_requirements_grow_by_ten_percent_rounded_up() {
        assert_eq!(experience_required_for_level(1), 100);
        assert_eq!(experience_required_for_level(2), 110);
        assert_eq!(experience_required_for_level(3), 121);
        assert_eq!(experience_required_for_level(4), 134);
    }

    #[test]
    fn penalty_loss_never_deducts_from_an_empty_stat() {
        let base = HunterStats {
            vitality: 2,
            sense: 1,
            ..HunterStats::new()
        };
        for roll in 0..6 {
            let mut stats = base;
            let deducted = stats
                .decrement_nonzero_for_roll(roll)
                .expect("a nonzero stat is available");
            assert!(matches!(deducted, HunterStat::Vitality | HunterStat::Sense));
            assert_eq!(stats.strength, 0);
            assert_eq!(stats.agility, 0);
            assert_eq!(stats.intelligence, 0);
            assert_eq!(stats.vitality + stats.sense, 2);
        }

        let mut empty = HunterStats::new();
        assert_eq!(empty.decrement_nonzero_for_roll(3), None);
    }

    #[test]
    fn experience_levels_up_with_carry_and_awards_stat_points() {
        let mut stats = HunterStats::new();

        let level_up = stats.grant_experience(210);

        assert_eq!(level_up.levels_gained, 2);
        assert_eq!(level_up.new_rank, HunterRank::E);
        assert_eq!(stats.level, 3);
        assert_eq!(stats.experience, 0);
        assert_eq!(stats.unspent_points, 2 * STAT_POINTS_PER_LEVEL);
        assert_eq!(stats.experience_to_next_level(), 121);
    }
}
