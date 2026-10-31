//! Rules of artifact abilities, independent of entities and the clock.
//!
//! Every function takes the current Unix time in milliseconds, so the rules
//! are testable and cooldowns survive reconnects as wall-clock deadlines.
//! Cooldowns are keyed by ability, not by item, so swapping two copies of the
//! same artifact cannot skip one.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::item::artifact::Ability;

/// Returns the current Unix time in milliseconds.
#[must_use]
pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

const fn seconds(seconds: u64) -> u64 {
    seconds.saturating_mul(1_000)
}

/// Persisted ability cooldown deadlines, in Unix milliseconds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbilityCooldowns(BTreeMap<String, u64>);

impl AbilityCooldowns {
    /// Creates an empty cooldown table.
    #[must_use]
    pub const fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Returns whether `ability` may trigger at `now`.
    #[must_use]
    pub fn ready(&self, ability: Ability, now: u64) -> bool {
        self.0
            .get(ability.cooldown_key())
            .is_none_or(|deadline| *deadline <= now)
    }

    /// Starts the cooldown of `ability` at `now`.
    pub fn start(&mut self, ability: Ability, now: u64) {
        self.0.insert(
            ability.cooldown_key().to_owned(),
            now.saturating_add(seconds(ability.cooldown_seconds())),
        );
    }

    /// Shortens the cooldowns started since `before` by `reduction`, at most
    /// by half; the Focus enchantment calls this after abilities trigger.
    pub fn shorten_started(&mut self, before: &Self, now: u64, reduction: f32) {
        if reduction <= 0.0 {
            return;
        }
        let keep = 1.0 - f64::from(reduction.min(0.5));
        for (key, deadline) in &mut self.0 {
            if before.0.get(key) != Some(deadline) && *deadline > now {
                let left = (*deadline - now) as f64 * keep;
                *deadline = now + left as u64;
            }
        }
    }

    /// Forgets cooldowns that already ran out, keeping the saved file small.
    pub fn prune(&mut self, now: u64) {
        self.0.retain(|_, deadline| *deadline > now);
    }
}

/// Short-lived effects of triggered abilities; never persisted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActiveAbilities {
    /// Successful melee hits counted toward the gauntlet shield.
    hits: u8,
    /// End of the movement-speed burst.
    pub speed_until: Option<u64>,
    /// End of the low-health damage reduction.
    reduction_until: Option<u64>,
    /// Shields by source ability key, with their end.
    pub shields: Vec<(&'static str, u64)>,
}

/// Something a triggered ability asks the world to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AbilityEffect {
    /// Raise movement speed by a fraction until the deadline.
    Speed {
        /// Bonus as a fraction of the total attribute.
        percent: f64,
        /// End of the burst, in Unix milliseconds.
        until: u64,
    },
    /// Grant absorption health from one source until the deadline.
    Shield {
        /// Source ability key; one shield per source.
        source: &'static str,
        /// Absorption health.
        health: f32,
        /// End of the shield, in Unix milliseconds.
        until: u64,
    },
    /// Start a storm around the struck target.
    Storm {
        /// Radius in blocks.
        radius: f64,
        /// Damage per second.
        damage_per_second: f32,
        /// End of the storm, in Unix milliseconds.
        until: u64,
    },
}

/// Applies the outgoing-damage abilities of the attacker's weapon.
#[must_use]
pub fn outgoing_damage(
    abilities: &[Ability],
    strength: u32,
    victim_armored: bool,
    amount: f32,
) -> f32 {
    let mut damage = amount;
    for ability in abilities {
        match *ability {
            Ability::ArmorPiercing { percent } if victim_armored => {
                damage *= 1.0 + percent;
            }
            Ability::StrengthScaling { per_point, maximum } => {
                let bonus = (f64::from(strength) * per_point).min(maximum);
                damage += bonus as f32;
            }
            Ability::SmilingAura { percent } => {
                damage *= 1.0 + percent;
            }
            _ => {}
        }
    }
    damage
}

/// Applies the incoming-damage abilities of the victim's armor.
///
/// A badly hurt holder starts the reduction window, which also covers the hit
/// that started it.
pub fn incoming_damage(
    abilities: &[Ability],
    cooldowns: &mut AbilityCooldowns,
    active: &mut ActiveAbilities,
    health_fraction: f32,
    now: u64,
    amount: f32,
) -> f32 {
    let mut damage = amount;
    for ability in abilities {
        let Ability::LowHealthReduction {
            threshold,
            reduction,
            seconds: duration,
            ..
        } = *ability
        else {
            continue;
        };
        let running = active.reduction_until.is_some_and(|until| until > now);
        if !running && health_fraction < threshold && cooldowns.ready(*ability, now) {
            active.reduction_until = Some(now.saturating_add(seconds(duration)));
            cooldowns.start(*ability, now);
        }
        if active.reduction_until.is_some_and(|until| until > now) {
            damage *= 1.0 - reduction;
        }
    }
    damage
}

/// Reacts to one successful melee hit by the holder of `abilities`.
pub fn on_hit(
    abilities: &[Ability],
    cooldowns: &mut AbilityCooldowns,
    active: &mut ActiveAbilities,
    now: u64,
) -> Vec<AbilityEffect> {
    let mut effects = Vec::new();
    for ability in abilities {
        match *ability {
            Ability::SpeedOnHit {
                percent,
                seconds: duration,
                ..
            } if cooldowns.ready(*ability, now) => {
                cooldowns.start(*ability, now);
                let until = now.saturating_add(seconds(duration));
                active.speed_until = Some(until);
                effects.push(AbilityEffect::Speed { percent, until });
            }
            Ability::HitCounterShield {
                hits,
                shield_health,
                seconds: duration,
                ..
            } => {
                active.hits = active.hits.saturating_add(1).min(hits);
                if active.hits >= hits && cooldowns.ready(*ability, now) {
                    active.hits = 0;
                    cooldowns.start(*ability, now);
                    effects.push(shield(active, *ability, shield_health, duration, now));
                }
            }
            Ability::Storm {
                radius,
                seconds: duration,
                damage_per_second,
                ..
            } if cooldowns.ready(*ability, now) => {
                cooldowns.start(*ability, now);
                effects.push(AbilityEffect::Storm {
                    radius,
                    damage_per_second,
                    until: now.saturating_add(seconds(duration)),
                });
            }
            _ => {}
        }
    }
    effects
}

/// Decides whether the holder survives an otherwise lethal hit.
///
/// Returns the health to keep and the shield to grant.
pub fn on_lethal(
    abilities: &[Ability],
    cooldowns: &mut AbilityCooldowns,
    active: &mut ActiveAbilities,
    now: u64,
) -> Option<(f32, AbilityEffect)> {
    abilities.iter().find_map(|ability| {
        let Ability::LethalSurvival {
            health,
            shield_health,
            seconds: duration,
            ..
        } = *ability
        else {
            return None;
        };
        if !cooldowns.ready(*ability, now) {
            return None;
        }
        cooldowns.start(*ability, now);
        Some((
            health,
            shield(active, *ability, shield_health, duration, now),
        ))
    })
}

fn shield(
    active: &mut ActiveAbilities,
    ability: Ability,
    health: f32,
    duration: u64,
    now: u64,
) -> AbilityEffect {
    let source = ability.cooldown_key();
    let until = now.saturating_add(seconds(duration));
    active.shields.retain(|(key, _)| *key != source);
    active.shields.push((source, until));
    AbilityEffect::Shield {
        source,
        health,
        until,
    }
}

/// Returns the extra crystals a pickaxe adds for one mined ore.
#[must_use]
pub fn extra_crystals(abilities: &[Ability], roll: u32) -> i32 {
    abilities
        .iter()
        .filter(|ability| matches!(ability, Ability::CrystalLuck { percent } if roll < *percent))
        .count()
        .try_into()
        .unwrap_or(0)
}

impl ActiveAbilities {
    /// Creates a state with nothing running.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            hits: 0,
            speed_until: None,
            reduction_until: None,
            shields: Vec::new(),
        }
    }

    /// Removes effects that ended, returning whether the speed burst ended
    /// and which shield sources ended.
    pub fn expire(&mut self, now: u64) -> (bool, Vec<&'static str>) {
        let speed = self.speed_until.is_some_and(|until| until <= now);
        if speed {
            self.speed_until = None;
        }
        let mut ended = Vec::new();
        self.shields.retain(|(key, until)| {
            let keep = *until > now;
            if !keep {
                ended.push(*key);
            }
            keep
        });
        (speed, ended)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AbilityCooldowns, AbilityEffect, ActiveAbilities, extra_crystals, incoming_damage, on_hit,
        on_lethal, outgoing_damage,
    };
    use crate::item::artifact::{Ability, ArtifactId};

    fn abilities(id: ArtifactId) -> &'static [Ability] {
        id.artifact().abilities
    }

    #[test]
    fn weapon_bonuses_follow_armor_and_strength() {
        let knight = abilities(ArtifactId::KnightsKiller);
        assert!((outgoing_damage(knight, 0, false, 10.0) - 10.0).abs() < 1e-4);
        assert!((outgoing_damage(knight, 0, true, 10.0) - 11.5).abs() < 1e-4);

        let daggers = abilities(ArtifactId::DemonKingsDaggers);
        assert!((outgoing_damage(daggers, 40, false, 10.0) - 12.0).abs() < 1e-4);
        assert!((outgoing_damage(daggers, 1_000, false, 10.0) - 15.0).abs() < 1e-4);
    }

    #[test]
    fn speed_burst_respects_its_cooldown() {
        let dagger = abilities(ArtifactId::BarukasDagger);
        let mut cooldowns = AbilityCooldowns::default();
        let mut active = ActiveAbilities::default();
        let effects = on_hit(dagger, &mut cooldowns, &mut active, 0);
        assert_eq!(
            effects,
            vec![AbilityEffect::Speed {
                percent: 0.15,
                until: 3_000
            }]
        );
        assert_eq!(on_hit(dagger, &mut cooldowns, &mut active, 9_999), vec![]);
        assert_eq!(on_hit(dagger, &mut cooldowns, &mut active, 10_000).len(), 1);
        assert_eq!(active.expire(13_000), (true, vec![]));
    }

    #[test]
    fn every_fifth_hit_grants_one_shield() {
        let gauntlet = abilities(ArtifactId::HighKnightsGauntlet);
        let mut cooldowns = AbilityCooldowns::default();
        let mut active = ActiveAbilities::default();
        for _ in 0..4 {
            assert_eq!(on_hit(gauntlet, &mut cooldowns, &mut active, 0), vec![]);
        }
        let effects = on_hit(gauntlet, &mut cooldowns, &mut active, 0);
        assert!(matches!(
            effects.as_slice(),
            [AbilityEffect::Shield { health, until: 5_000, .. }] if (*health - 4.0).abs() < 1e-4
        ));
        for _ in 0..5 {
            assert_eq!(on_hit(gauntlet, &mut cooldowns, &mut active, 1_000), vec![]);
        }
        assert_eq!(
            on_hit(gauntlet, &mut cooldowns, &mut active, 15_000).len(),
            1
        );
    }

    #[test]
    fn low_health_reduction_opens_a_window_then_cools_down() {
        let chest = abilities(ArtifactId::HighKnightsChestplate);
        let mut cooldowns = AbilityCooldowns::default();
        let mut active = ActiveAbilities::default();
        let full = incoming_damage(chest, &mut cooldowns, &mut active, 0.9, 0, 10.0);
        assert!((full - 10.0).abs() < 1e-4);
        let reduced = incoming_damage(chest, &mut cooldowns, &mut active, 0.2, 0, 10.0);
        assert!((reduced - 8.5).abs() < 1e-4);
        let still = incoming_damage(chest, &mut cooldowns, &mut active, 0.9, 3_999, 10.0);
        assert!((still - 8.5).abs() < 1e-4);
        let after = incoming_damage(chest, &mut cooldowns, &mut active, 0.2, 4_000, 10.0);
        assert!((after - 10.0).abs() < 1e-4);
        let again = incoming_damage(chest, &mut cooldowns, &mut active, 0.2, 30_000, 10.0);
        assert!((again - 8.5).abs() < 1e-4);
    }

    #[test]
    fn lethal_survival_is_once_per_cooldown_and_survives_reconnect() {
        let helmet = abilities(ArtifactId::RedKnightsHelmet);
        let mut cooldowns = AbilityCooldowns::default();
        let mut active = ActiveAbilities::default();
        assert!(on_lethal(helmet, &mut cooldowns, &mut active, 0).is_some());
        let saved = serde_json::to_string(&cooldowns).expect("serialize");
        let mut restored: AbilityCooldowns = serde_json::from_str(&saved).expect("deserialize");
        let mut fresh = ActiveAbilities::default();
        assert!(on_lethal(helmet, &mut restored, &mut fresh, 119_999).is_none());
        assert!(on_lethal(helmet, &mut restored, &mut fresh, 120_000).is_some());
    }

    #[test]
    fn storm_and_crystal_luck() {
        let sword = abilities(ArtifactId::DemonKingsLongsword);
        let mut cooldowns = AbilityCooldowns::default();
        let mut active = ActiveAbilities::default();
        assert!(matches!(
            on_hit(sword, &mut cooldowns, &mut active, 0).as_slice(),
            [AbilityEffect::Storm { until: 3_000, .. }]
        ));
        let pickaxe = abilities(ArtifactId::CrystalPickaxe);
        assert_eq!(extra_crystals(pickaxe, 49), 1);
        assert_eq!(extra_crystals(pickaxe, 50), 0);
        assert_eq!(
            extra_crystals(abilities(ArtifactId::KasakasVenomFang), 0),
            0
        );
    }
}
