//! Persistent Shadow Monarch roster and shadow progression rules.

use serde::{Deserialize, Serialize};
use steel_utils::Identifier;
use uuid::Uuid;

/// Highest level a retained shadow can reach.
pub const MAX_SHADOW_LEVEL: u16 = 100;
/// Maximum distance from a player kill at which its soul can be retained.
pub const ARISE_RANGE: f64 = 8.0;
/// Health a destroyed shadow recovers each minute.
pub const RECOVERY_HEALTH_PER_MINUTE: u16 = 10;
/// Additional maximum health per allocated health point.
pub const HEALTH_PER_POINT: f64 = 2.0;
/// Additional attack damage per allocated attack point.
pub const ATTACK_PER_POINT: f64 = 0.5;
/// Additional movement speed per allocated speed point.
pub const SPEED_PER_POINT: f64 = 0.01;

/// Returns the number of shadows a hunter may have summoned at once.
#[must_use]
pub fn army_limit(hunter_level: u32) -> usize {
    (3 + hunter_level / 20).min(10) as usize
}

/// One persistent shadow stat that may receive an earned point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowStat {
    /// Raises maximum health.
    Health,
    /// Raises melee attack damage.
    Attack,
    /// Raises movement speed.
    Speed,
}

/// Persisted recovery state of a destroyed shadow body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowRecovery {
    /// Health restored so far, starting at zero when the body dies.
    pub health: u16,
    /// Full health of the body at the moment it died.
    pub maximum_health: u16,
}

/// A shadow retained by one hunter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shadow {
    /// Stable roster identity, independent of a currently summoned entity.
    pub id: Uuid,
    /// Player-visible name, unique within one hunter's roster.
    pub name: String,
    /// Vanilla mob type recreated when this shadow is summoned.
    pub entity_type: Identifier,
    /// Current shadow level, starting at one.
    pub level: u16,
    /// Experience accumulated toward the next level.
    pub experience: u64,
    /// Points earned by levelling that have not been assigned to a stat.
    pub unspent_points: u16,
    /// Permanent health upgrades.
    pub health: u16,
    /// Permanent attack-damage upgrades.
    pub attack: u16,
    /// Permanent movement-speed upgrades.
    pub speed: u16,
    /// Present only while this shadow is rebuilding after its summoned body died.
    #[serde(default)]
    pub recovery: Option<ShadowRecovery>,
}

impl Shadow {
    /// Creates a newly arisen shadow from a vanilla hostile mob type.
    #[must_use]
    pub fn new(entity_type: Identifier, ordinal: usize) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: format!("Shadow {ordinal}"),
            entity_type,
            level: 1,
            experience: 0,
            unspent_points: 0,
            health: 0,
            attack: 0,
            speed: 0,
            recovery: None,
        }
    }

    /// Experience needed to advance from this level.
    #[must_use]
    pub const fn experience_to_next_level(&self) -> u64 {
        self.level as u64 * 100
    }

    /// Grants kill experience, returning the number of levels gained.
    pub const fn grant_experience(&mut self, amount: u64) -> u16 {
        if self.level >= MAX_SHADOW_LEVEL {
            return 0;
        }
        self.experience = self.experience.saturating_add(amount);
        let mut gained = 0;
        while self.level < MAX_SHADOW_LEVEL && self.experience >= self.experience_to_next_level() {
            self.experience -= self.experience_to_next_level();
            self.level += 1;
            self.unspent_points += 1;
            gained += 1;
        }
        gained
    }

    /// Allocates one earned point to `stat`.
    pub const fn allocate_point(&mut self, stat: ShadowStat) -> bool {
        if self.unspent_points == 0 {
            return false;
        }
        self.unspent_points -= 1;
        match stat {
            ShadowStat::Health => self.health += 1,
            ShadowStat::Attack => self.attack += 1,
            ShadowStat::Speed => self.speed += 1,
        }
        true
    }

    /// Starts rebuilding this shadow from zero health.
    pub const fn begin_recovery(&mut self, maximum_health: u16) {
        self.recovery = Some(ShadowRecovery {
            health: 0,
            maximum_health,
        });
    }

    /// Restores health and reports whether persistent recovery state changed.
    pub const fn recover(&mut self, health: u16) -> bool {
        let Some(recovery) = &mut self.recovery else {
            return false;
        };
        recovery.health = recovery.health.saturating_add(health);
        if recovery.health >= recovery.maximum_health {
            self.recovery = None;
        }
        true
    }

    /// Reports whether this shadow must finish rebuilding before it can be summoned.
    #[must_use]
    pub const fn is_recovering(&self) -> bool {
        self.recovery.is_some()
    }
}

/// The persisted roster owned by one hunter.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowRoster {
    /// Shadows in roster order.
    pub shadows: Vec<Shadow>,
}

impl ShadowRoster {
    /// Adds a shadow with a name that is unique within this roster.
    pub fn arise(&mut self, entity_type: Identifier) -> &mut Shadow {
        let index = self.shadows.len();
        let ordinal = index + 1;
        self.shadows.push(Shadow::new(entity_type, ordinal));
        &mut self.shadows[index]
    }

    /// Looks up a shadow by its player-visible name.
    #[must_use]
    pub fn named(&self, name: &str) -> Option<&Shadow> {
        self.shadows
            .iter()
            .find(|shadow| shadow.name.eq_ignore_ascii_case(name))
    }

    /// Returns a mutable shadow selected by its player-visible name.
    pub fn named_mut(&mut self, name: &str) -> Option<&mut Shadow> {
        self.shadows
            .iter_mut()
            .find(|shadow| shadow.name.eq_ignore_ascii_case(name))
    }

    /// Restores every destroyed shadow by `health`, returning whether anything changed.
    pub fn recover(&mut self, health: u16) -> bool {
        let mut changed = false;
        for shadow in &mut self.shadows {
            changed |= shadow.recover(health);
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use steel_utils::Identifier;

    use super::{Shadow, ShadowRoster};

    #[test]
    fn arisen_shadows_have_stable_unique_ids_and_case_insensitive_names() {
        let mut roster = ShadowRoster::default();
        let first = roster.arise(Identifier::vanilla_static("zombie")).clone();
        let second = roster.arise(Identifier::vanilla_static("zombie")).clone();

        assert_ne!(first.id, second.id);
        assert_eq!(roster.named("sHaDoW 1"), Some(&first));
    }

    #[test]
    fn experience_levels_a_shadow_and_turns_into_upgrade_points() {
        let mut roster = ShadowRoster::default();
        let shadow = roster.arise(Identifier::vanilla_static("zombie"));
        assert_eq!(shadow.grant_experience(100), 1);
        assert_eq!(shadow.level, 2);
        assert_eq!(shadow.unspent_points, 1);
        assert!(shadow.allocate_point(super::ShadowStat::Attack));
        assert_eq!(shadow.attack, 1);
    }

    #[test]
    fn army_limit_grows_every_twenty_hunter_levels_and_caps_at_ten() {
        assert_eq!(super::army_limit(1), 3);
        assert_eq!(super::army_limit(20), 4);
        assert_eq!(super::army_limit(140), 10);
    }

    #[test]
    fn destroyed_shadow_cannot_be_summoned_until_its_full_health_recovers() {
        let mut roster = ShadowRoster::default();
        let shadow = roster.arise(Identifier::vanilla_static("zombie"));
        shadow.begin_recovery(20);
        assert!(shadow.is_recovering());
        assert!(shadow.recover(10));
        assert!(shadow.is_recovering());
        assert!(shadow.recover(10));
        assert!(!shadow.is_recovering());
    }

    #[test]
    fn recovery_advances_every_destroyed_shadow_in_the_roster() {
        let mut roster = ShadowRoster::default();
        roster
            .arise(Identifier::vanilla_static("zombie"))
            .begin_recovery(20);
        roster
            .arise(Identifier::vanilla_static("skeleton"))
            .begin_recovery(20);

        assert!(roster.recover(10));
        assert!(roster.shadows.iter().all(Shadow::is_recovering));
        assert!(roster.recover(10));
        assert!(roster.shadows.iter().all(|shadow| !shadow.is_recovering()));
    }
}
