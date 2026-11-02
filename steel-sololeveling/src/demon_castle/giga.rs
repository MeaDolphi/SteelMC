//! The giga bosses of every tenth floor: one guardian per theme, far larger
//! and tougher than a floor boss, fighting in three phases.
//!
//! The fight escalates as the guardian loses health: at two thirds it calls
//! adds and the arena starts erupting under the hunters' feet; at one third
//! it enrages, calls more adds and erupts faster.

use steel_registry::mob_effect::MobEffectRef;
use steel_registry::vanilla_mob_effects;

use super::theme::CastleTheme;
use crate::castle;
use crate::dungeon::boss::{BossAbility, BossKind, DungeonBoss, boss_for_gate};

/// Health of a giga boss over the floor boss it replaces.
const HEALTH: f64 = 3.0;
/// Melee and ability damage of a giga boss over the floor boss.
const DAMAGE: f64 = 1.3;
/// Body scale of a giga boss over the floor boss.
const SCALE: f64 = 1.8;
/// Largest body scale, so the guardian still fits under the arena chandelier.
const MAX_SCALE: f64 = 4.5;

/// Effects an enraged guardian fights under.
static RAGE: [MobEffectRef; 3] = [
    vanilla_mob_effects::STRENGTH,
    vanilla_mob_effects::SPEED,
    vanilla_mob_effects::RESISTANCE,
];

/// Who guards the tenth floor of a theme.
#[derive(Debug, Clone, Copy)]
pub struct Guardian {
    /// Name over the boss bar.
    pub name: &'static str,
    /// Body and fighting style.
    pub kind: BossKind,
    /// Abilities by priority; wider than any floor boss's kit.
    pub abilities: &'static [BossAbility],
}

impl CastleTheme {
    /// Returns the guardian of this theme's tenth floor.
    #[must_use]
    pub const fn guardian(self) -> Guardian {
        use BossAbility::{
            ArcaneBolt, Blink, DreadRoar, FrostNova, Meteor, ShadowStep, Slam, SmokeBomb,
            VenomBlades, WarCry,
        };
        match self {
            Self::Fortress => Guardian {
                name: "Чёрный владыка крепости",
                kind: BossKind::Warlord,
                abilities: &[Slam, Meteor, WarCry, DreadRoar],
            },
            Self::Inferno => Guardian {
                name: "Пожиратель пламени",
                kind: BossKind::Archmage,
                abilities: &[Blink, Meteor, FrostNova, ArcaneBolt, DreadRoar],
            },
            Self::FrozenCitadel => Guardian {
                name: "Ледяная владычица бездны",
                kind: BossKind::Archmage,
                abilities: &[FrostNova, Blink, ArcaneBolt, Meteor, DreadRoar],
            },
            Self::DemonCity => Guardian {
                name: "Князь демонического города",
                kind: BossKind::Assassin,
                abilities: &[VenomBlades, ShadowStep, SmokeBomb, Meteor, DreadRoar],
            },
            Self::Catacombs => Guardian {
                name: "Костяной император",
                kind: BossKind::Warlord,
                abilities: &[Slam, VenomBlades, WarCry, Meteor, DreadRoar],
            },
        }
    }
}

/// Returns the stats of the guardian of `floor`.
#[must_use]
pub fn guardian_boss(theme: CastleTheme, floor: u32) -> DungeonBoss {
    let guardian = theme.guardian();
    let mut boss = boss_for_gate(guardian.kind, castle::rank_for_floor(floor), false);
    let multiplier = castle::floor_multiplier(floor);
    boss.health_multiplier *= multiplier * HEALTH;
    boss.damage_multiplier *= multiplier * DAMAGE;
    boss.power *= multiplier * DAMAGE;
    boss.scale = (boss.scale * SCALE).min(MAX_SCALE);
    boss.knockback_resistance = 1.0;
    boss.abilities = guardian.abilities;
    boss
}

/// How far a giga fight has escalated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// Above two thirds of its health.
    Awakened,
    /// Above one third: adds and eruptions.
    Furious,
    /// The last third: enraged, more adds, faster eruptions.
    Desperate,
}

impl Phase {
    /// Returns the phase of a guardian at `health` of its maximum.
    #[must_use]
    pub fn at(health: f32) -> Self {
        if health > 2.0 / 3.0 {
            Self::Awakened
        } else if health > 1.0 / 3.0 {
            Self::Furious
        } else {
            Self::Desperate
        }
    }

    /// Returns how many adds the guardian calls on entering this phase.
    #[must_use]
    pub const fn adds(self) -> usize {
        match self {
            Self::Awakened => 0,
            Self::Furious => 4,
            Self::Desperate => 6,
        }
    }

    /// Returns the ticks between two eruptions, or `None` before they start.
    #[must_use]
    pub const fn eruption_interval(self) -> Option<u64> {
        match self {
            Self::Awakened => None,
            Self::Furious => Some(120),
            Self::Desperate => Some(70),
        }
    }

    /// Returns how many eruptions strike the open arena beside the ones
    /// under the hunters.
    #[must_use]
    pub const fn stray_eruptions(self) -> usize {
        match self {
            Self::Awakened => 0,
            Self::Furious => 2,
            Self::Desperate => 4,
        }
    }

    /// Returns the effects the guardian gains on entering this phase.
    #[must_use]
    pub fn rage(self) -> &'static [MobEffectRef] {
        match self {
            Self::Awakened | Self::Furious => &[],
            Self::Desperate => &RAGE,
        }
    }

    /// Returns the line announcing this phase.
    #[must_use]
    pub const fn announcement(self) -> &'static str {
        match self {
            Self::Awakened => "Великий страж пробудился.",
            Self::Furious => "Страж в ярости: он зовёт свиту, и земля начинает извергаться!",
            Self::Desperate => {
                "Страж на последнем издыхании и неистовствует: извержения учащаются!"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Phase, guardian_boss};
    use crate::castle;
    use crate::demon_castle::theme::CastleTheme;
    use crate::dungeon::boss::boss_for_gate;

    #[test]
    fn a_giga_boss_towers_over_its_floor_boss_and_escalates_in_order() {
        for floor in [10_u32, 20, 50, 60, 120] {
            let theme = CastleTheme::of_floor(floor);
            let giga = guardian_boss(theme, floor);
            let plain = boss_for_gate(theme.guardian().kind, castle::rank_for_floor(floor), false);
            assert!(
                giga.health_multiplier > plain.health_multiplier * castle::floor_multiplier(floor)
            );
            assert!(giga.scale > plain.scale);
        }
        assert_eq!(Phase::at(1.0), Phase::Awakened);
        assert_eq!(Phase::at(0.5), Phase::Furious);
        assert_eq!(Phase::at(0.2), Phase::Desperate);
        assert!(Phase::Desperate > Phase::Furious);
        assert!(Phase::Desperate.eruption_interval() < Phase::Furious.eruption_interval());
    }
}
