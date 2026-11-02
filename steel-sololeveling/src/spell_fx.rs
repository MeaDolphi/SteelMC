//! Visual identity of spells: particle-shape primitives and per-spell themes.
//!
//! Every spell reads as three layers built from these primitives: a cast ring
//! and swirl at the caster ([`cast`]), a trail along the projectile path
//! ([`beam`]/[`trail`]), and an impact burst with the school sound ([`impact`]).
//! Only vanilla particle types and sound events are used, and a full cast stays
//! around a hundred particles, so effects stay cheap next to vanilla fireworks.
//! Damage, cooldowns and mana are untouched here; this module only draws.

pub mod display;
pub mod runtime;

use std::f64::consts::{PI, TAU};

use glam::DVec3;
use steel_core::world::World;
use steel_registry::particle_type::{
    ColorParticleOption, DustParticleOptions, ParticleData, ParticleType,
};
use steel_registry::sound_event::SoundEvent;
use steel_registry::{sound_events, vanilla_particle_types as particle};
use steel_utils::{ArgbColor, RgbColor};

use crate::sound::{SpellSound, play_at};
use crate::spell::SpellId;

/// How far the cast ring sits from the caster's feet.
const CAST_RING_RADIUS: f64 = 1.2;
/// Spacing of beam particles, in blocks.
const BEAM_SPACING: f64 = 0.75;

// ---------------------------------------------------------------------------
// Pure geometry; the emitters below send particles at these positions.
// ---------------------------------------------------------------------------

/// Positions on a horizontal ring of `radius` around `center`, first point
/// due east of `center`.
#[must_use]
pub fn ring_points(center: DVec3, radius: f64, count: usize) -> Vec<DVec3> {
    (0..count)
        .map(|index| {
            let angle = index as f64 / count as f64 * TAU;
            center + DVec3::new(radius * angle.cos(), 0.0, radius * angle.sin())
        })
        .collect()
}

/// Positions of a helix rising `height` blocks over `turns` full circles,
/// starting at `center` and ending straight above it.
#[must_use]
pub fn spiral_points(
    center: DVec3,
    radius: f64,
    height: f64,
    turns: f64,
    count: usize,
) -> Vec<DVec3> {
    let last = count.saturating_sub(1).max(1) as f64;
    (0..count)
        .map(|index| {
            let t = index as f64 / last;
            let angle = t * turns * TAU;
            center + DVec3::new(radius * angle.cos(), height * t, radius * angle.sin())
        })
        .collect()
}

/// Positions of a cone fan from `apex` along `direction`: the apex itself,
/// then rings that widen with distance up to `length`, covering `angle_deg`.
#[must_use]
pub fn cone_points(
    apex: DVec3,
    direction: DVec3,
    length: f64,
    angle_deg: f64,
    count: usize,
) -> Vec<DVec3> {
    let direction = direction.normalize_or_zero();
    let up = if direction.abs_diff_eq(DVec3::Y, 1e-6) {
        DVec3::X
    } else {
        direction.cross(DVec3::Y).normalize()
    };
    let side = direction.cross(up);
    let slope = angle_deg.to_radians().tan();
    let mut points = Vec::with_capacity(count);
    for index in 0..count {
        if index == 0 {
            points.push(apex);
            continue;
        }
        let t = index as f64 / (count - 1) as f64;
        let distance = length * t;
        let ring_radius = distance * slope;
        // The golden angle spreads ring points without a visible seam.
        let golden = index as f64 * 2.399_963_229_728_653;
        let offset = up * (ring_radius * golden.cos()) + side * (ring_radius * golden.sin());
        points.push(apex + direction * distance + offset);
    }
    points
}

/// Evenly spaced positions from `from` to `to` inclusive, `spacing` blocks apart.
#[must_use]
pub fn line_points(from: DVec3, to: DVec3, spacing: f64) -> Vec<DVec3> {
    let steps = (from.distance(to) / spacing).ceil().max(1.0) as u32;
    (0..=steps)
        .map(|step| from.lerp(to, f64::from(step) / f64::from(steps)))
        .collect()
}

// ---------------------------------------------------------------------------
// Emitters: one particle per position keeps shapes crisp.
// ---------------------------------------------------------------------------

/// Sends one `particle` at every position.
fn shape(world: &World, particle: ParticleData, positions: &[DVec3]) {
    for position in positions {
        world.send_particles(particle.clone(), *position, 1, DVec3::ZERO, 0.0);
    }
}

/// A vanilla particle puff: `count` particles in a `spread` cube.
pub fn burst(
    world: &World,
    kind: &'static ParticleType,
    at: DVec3,
    count: i32,
    spread: f64,
    speed: f64,
) {
    world.send_particles(
        ParticleData::simple(kind),
        at,
        count,
        DVec3::splat(spread),
        speed,
    );
}

/// The white flash used where lightning strikes.
pub fn flash(world: &World, at: DVec3) {
    let data = ParticleData::new(
        &particle::FLASH,
        ColorParticleOption::new(ArgbColor::new(-1)),
    );
    world.send_particles(data, at, 1, DVec3::ZERO, 0.0);
}

/// A school-tinted dust puff.
pub fn dust(world: &World, tint: RgbColor, at: DVec3, count: i32, spread: f64, speed: f64) {
    let data = ParticleData::new(&particle::DUST, DustParticleOptions::new(tint, 1.2));
    world.send_particles(data, at, count, DVec3::splat(spread), speed);
}

/// A horizontal ring of `kind` around `center`.
pub fn ring(world: &World, kind: &'static ParticleType, center: DVec3, radius: f64, count: usize) {
    shape(
        world,
        ParticleData::simple(kind),
        &ring_points(center, radius, count),
    );
}

/// A horizontal ring of school-tinted dust around `center`.
pub fn ring_dust(world: &World, tint: RgbColor, center: DVec3, radius: f64, count: usize) {
    let data = ParticleData::new(&particle::DUST, DustParticleOptions::new(tint, 1.2));
    shape(world, data, &ring_points(center, radius, count));
}

/// A rising helix of `kind` around `center`.
pub fn spiral(
    world: &World,
    kind: &'static ParticleType,
    center: DVec3,
    radius: f64,
    height: f64,
    turns: f64,
    count: usize,
) {
    shape(
        world,
        ParticleData::simple(kind),
        &spiral_points(center, radius, height, turns, count),
    );
}

/// A cone fan of `kind` from `apex` along `direction`.
pub fn cone(
    world: &World,
    kind: &'static ParticleType,
    apex: DVec3,
    direction: DVec3,
    length: f64,
    angle_deg: f64,
    count: usize,
) {
    shape(
        world,
        ParticleData::simple(kind),
        &cone_points(apex, direction, length, angle_deg, count),
    );
}

/// A line of `kind` from `from` to `to`.
pub fn beam(world: &World, kind: &'static ParticleType, from: DVec3, to: DVec3) {
    shape(
        world,
        ParticleData::simple(kind),
        &line_points(from, to, BEAM_SPACING),
    );
}

/// A deterministic jagged lightning path with exact endpoints.
#[must_use]
pub fn lightning_points(from: DVec3, to: DVec3) -> Vec<DVec3> {
    let delta = to - from;
    let length = delta.length();
    let direction = delta.normalize_or_zero();
    let side = direction.cross(DVec3::Y).normalize_or_zero();
    let up = direction.cross(side).normalize_or_zero();
    let segments = (length * 3.0).ceil().max(2.0) as usize;
    (0..=segments)
        .map(|index| {
            let t = index as f64 / segments as f64;
            let envelope = (t * PI).sin();
            let jitter = (index as f64 * 2.17).sin() * 0.42 * envelope;
            let lift = (index as f64 * 3.73).cos() * 0.24 * envelope;
            from.lerp(to, t) + side * jitter + up * lift
        })
        .collect()
}

/// Draws a bright, jagged lightning bolt.
pub fn lightning(world: &World, from: DVec3, to: DVec3) {
    for offset in [
        DVec3::ZERO,
        DVec3::new(0.16, 0.08, -0.12),
        DVec3::new(-0.14, -0.06, 0.16),
    ] {
        let points = lightning_points(from + offset, to - offset);
        for point in &points {
            world.send_particles(
                ParticleData::simple(&particle::ELECTRIC_SPARK),
                *point,
                2,
                DVec3::ZERO,
                0.0,
            );
        }
        shape(
            world,
            ParticleData::simple(&particle::END_ROD),
            &points.iter().step_by(2).copied().collect::<Vec<_>>(),
        );
    }
    let center = from.lerp(to, 0.5);
    flash(world, center);
    burst(world, &particle::ELECTRIC_SPARK, center, 24, 0.4, 0.06);
}

/// A projectile trail: the theme's primary particle along the whole path and
/// its secondary one sparser, so the core reads bright inside a soft wake.
pub fn trail(world: &World, theme: &SpellTheme, from: DVec3, to: DVec3) {
    beam(world, theme.primary, from, to);
    let positions: Vec<DVec3> = line_points(from, to, BEAM_SPACING * 2.0)
        .into_iter()
        .enumerate()
        .filter(|(index, _)| index % 2 == 0)
        .map(|(_, position)| position)
        .collect();
    shape(world, ParticleData::simple(theme.secondary), &positions);
}

// ---------------------------------------------------------------------------
// Per-spell themes, grouped by visual school.
// ---------------------------------------------------------------------------

/// The visual identity of one spell: school-tinted dust, two particle kinds
/// and the vanilla sounds of its school.
#[derive(Debug, Clone, Copy)]
pub struct SpellTheme {
    /// School color for shaped dust.
    pub tint: RgbColor,
    /// Main particle of trails and impact bursts.
    pub primary: &'static ParticleType,
    /// Secondary particle of the cast swirl and impact rings.
    pub secondary: &'static ParticleType,
    /// School sound at the caster.
    pub cast_sound: &'static SoundEvent,
    /// School sound where the spell lands.
    pub impact_sound: &'static SoundEvent,
}

/// Returns the visual theme of `spell`, by school:
/// arcane (Magic Arrow, Mana Storm, Mana Shield, Meditation), fire (Fireball,
/// Meteor), ice (Ice Spear, Shackles), lightning (Chain Lightning), void
/// (Void Lance, Gravity Well, Weaken, Monarch's Wrath), holy (healing and
/// protection), wind (Repulse, Haste) and space (Blink, Detection).
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "the theme table reads best as one row per spell"
)]
pub const fn theme(spell: SpellId) -> SpellTheme {
    use sound_events as sound;
    match spell {
        SpellId::MagicArrow => SpellTheme {
            tint: RgbColor::new(0xA2_6B_E8),
            primary: &particle::END_ROD,
            secondary: &particle::ENCHANTED_HIT,
            cast_sound: &sound::ENTITY_ILLUSIONER_CAST_SPELL,
            impact_sound: &sound::ENTITY_ILLUSIONER_MIRROR_MOVE,
        },
        SpellId::Fireball => SpellTheme {
            tint: RgbColor::new(0xFF_7A_2A),
            primary: &particle::FLAME,
            secondary: &particle::SMOKE,
            cast_sound: &sound::ENTITY_BLAZE_SHOOT,
            impact_sound: &sound::ENTITY_GENERIC_EXPLODE,
        },
        SpellId::IceSpear => SpellTheme {
            tint: RgbColor::new(0x9F_CE_FF),
            primary: &particle::SNOWFLAKE,
            secondary: &particle::POOF,
            cast_sound: &sound::ENTITY_SNOW_GOLEM_SHOOT,
            impact_sound: &sound::ENTITY_PLAYER_HURT_FREEZE,
        },
        SpellId::ChainLightning => SpellTheme {
            tint: RgbColor::new(0xFF_E9_4A_u32 as i32),
            primary: &particle::ELECTRIC_SPARK,
            secondary: &particle::END_ROD,
            cast_sound: &sound::ITEM_TRIDENT_THUNDER,
            impact_sound: &sound::ENTITY_LIGHTNING_BOLT_THUNDER,
        },
        SpellId::ManaStorm => SpellTheme {
            tint: RgbColor::new(0x8F_5B_E0),
            primary: &particle::ENCHANT,
            secondary: &particle::END_ROD,
            cast_sound: &sound::BLOCK_BEACON_ACTIVATE,
            impact_sound: &sound::BLOCK_BEACON_AMBIENT,
        },
        SpellId::Meteor => SpellTheme {
            tint: RgbColor::new(0xFF_5A_1A),
            primary: &particle::LAVA,
            secondary: &particle::LARGE_SMOKE,
            cast_sound: &sound::ENTITY_TNT_PRIMED,
            impact_sound: &sound::ENTITY_DRAGON_FIREBALL_EXPLODE,
        },
        SpellId::VoidLance => SpellTheme {
            tint: RgbColor::new(0x7B_2F_CB),
            primary: &particle::PORTAL,
            secondary: &particle::SCULK_SOUL,
            cast_sound: &sound::ENTITY_WITHER_SHOOT,
            impact_sound: &sound::ENTITY_WITHER_SHOOT,
        },
        SpellId::MonarchsWrath => SpellTheme {
            tint: RgbColor::new(0x7A_2B_E2),
            primary: &particle::SOUL_FIRE_FLAME,
            secondary: &particle::SOUL,
            cast_sound: &sound::ENTITY_WITHER_SPAWN,
            impact_sound: &sound::ENTITY_WITHER_SPAWN,
        },
        SpellId::LesserHeal => SpellTheme {
            tint: RgbColor::new(0x8C_FF_9E),
            primary: &particle::HEART,
            secondary: &particle::HAPPY_VILLAGER,
            cast_sound: &sound::ENTITY_ZOMBIE_VILLAGER_CURE,
            impact_sound: &sound::ENTITY_ZOMBIE_VILLAGER_CURE,
        },
        SpellId::Restoration => SpellTheme {
            tint: RgbColor::new(0x6F_E0_8A),
            primary: &particle::HAPPY_VILLAGER,
            secondary: &particle::END_ROD,
            cast_sound: &sound::ENTITY_ZOMBIE_VILLAGER_CURE,
            impact_sound: &sound::ENTITY_ZOMBIE_VILLAGER_CURE,
        },
        SpellId::Cleanse => SpellTheme {
            tint: RgbColor::new(0xE8_F6_FF),
            primary: &particle::WHITE_ASH,
            secondary: &particle::HAPPY_VILLAGER,
            cast_sound: &sound::ENTITY_GENERIC_EXTINGUISH_FIRE,
            impact_sound: &sound::ENTITY_GENERIC_EXTINGUISH_FIRE,
        },
        SpellId::Barrier => SpellTheme {
            tint: RgbColor::new(0xFF_D7_5A_u32 as i32),
            primary: &particle::END_ROD,
            secondary: &particle::HAPPY_VILLAGER,
            cast_sound: &sound::ITEM_SHIELD_BLOCK,
            impact_sound: &sound::ITEM_SHIELD_BLOCK,
        },
        SpellId::GreaterHeal => SpellTheme {
            tint: RgbColor::new(0x8C_FF_9E),
            primary: &particle::TOTEM_OF_UNDYING,
            secondary: &particle::HEART,
            cast_sound: &sound::ENTITY_ZOMBIE_VILLAGER_CURE,
            impact_sound: &sound::ENTITY_ZOMBIE_VILLAGER_CURE,
        },
        SpellId::Sanctuary => SpellTheme {
            tint: RgbColor::new(0xFF_E9_A8_u32 as i32),
            primary: &particle::END_ROD,
            secondary: &particle::HAPPY_VILLAGER,
            cast_sound: &sound::BLOCK_BEACON_ACTIVATE,
            impact_sound: &sound::BLOCK_BEACON_ACTIVATE,
        },
        SpellId::LastStand => SpellTheme {
            tint: RgbColor::new(0xFF_D2_3A_u32 as i32),
            primary: &particle::TOTEM_OF_UNDYING,
            secondary: &particle::ENCHANT,
            cast_sound: &sound::ITEM_TOTEM_USE,
            impact_sound: &sound::ITEM_TOTEM_USE,
        },
        SpellId::Repulse => SpellTheme {
            tint: RgbColor::new(0xDD_E3_EA),
            primary: &particle::CLOUD,
            secondary: &particle::WHITE_SMOKE,
            cast_sound: &sound::ENTITY_PLAYER_ATTACK_SWEEP,
            impact_sound: &sound::ENTITY_PLAYER_ATTACK_SWEEP,
        },
        SpellId::Haste => SpellTheme {
            tint: RgbColor::new(0xDD_E3_EA),
            primary: &particle::SWEEP_ATTACK,
            secondary: &particle::CLOUD,
            cast_sound: &sound::BLOCK_AMETHYST_BLOCK_CHIME,
            impact_sound: &sound::BLOCK_AMETHYST_BLOCK_CHIME,
        },
        SpellId::Shackles => SpellTheme {
            tint: RgbColor::new(0x9F_CE_FF),
            primary: &particle::ENCHANT,
            secondary: &particle::SNOWFLAKE,
            cast_sound: &sound::ENTITY_ILLUSIONER_PREPARE_MIRROR,
            impact_sound: &sound::ENTITY_PLAYER_HURT_FREEZE,
        },
        SpellId::BlessingOfStrength => SpellTheme {
            tint: RgbColor::new(0xFF_B4_3A_u32 as i32),
            primary: &particle::CRIT,
            secondary: &particle::HAPPY_VILLAGER,
            cast_sound: &sound::BLOCK_BEACON_POWER_SELECT,
            impact_sound: &sound::BLOCK_BEACON_POWER_SELECT,
        },
        SpellId::Weaken => SpellTheme {
            tint: RgbColor::new(0x5B_4A_6B),
            primary: &particle::WITCH,
            secondary: &particle::SMOKE,
            cast_sound: &sound::ENTITY_WITCH_AMBIENT,
            impact_sound: &sound::ENTITY_WITCH_AMBIENT,
        },
        SpellId::GravityWell => SpellTheme {
            tint: RgbColor::new(0x6C_3B_B8),
            primary: &particle::REVERSE_PORTAL,
            secondary: &particle::PORTAL,
            cast_sound: &sound::ENTITY_ENDERMAN_TELEPORT,
            impact_sound: &sound::ENTITY_ENDERMAN_TELEPORT,
        },
        SpellId::NightVision => SpellTheme {
            tint: RgbColor::new(0xFF_F3_B0_u32 as i32),
            primary: &particle::NOTE,
            secondary: &particle::END_ROD,
            cast_sound: &sound::BLOCK_NOTE_BLOCK_BELL,
            impact_sound: &sound::BLOCK_NOTE_BLOCK_BELL,
        },
        SpellId::Blink => SpellTheme {
            tint: RgbColor::new(0xB5_8C_FF),
            primary: &particle::REVERSE_PORTAL,
            secondary: &particle::PORTAL,
            cast_sound: &sound::ENTITY_ENDERMAN_TELEPORT,
            impact_sound: &sound::ENTITY_ENDERMAN_TELEPORT,
        },
        SpellId::Detection => SpellTheme {
            tint: RgbColor::new(0x9F_E8_FF),
            primary: &particle::END_ROD,
            secondary: &particle::NOTE,
            cast_sound: &sound::BLOCK_NOTE_BLOCK_BELL,
            impact_sound: &sound::BLOCK_NOTE_BLOCK_BELL,
        },
        SpellId::Meditation => SpellTheme {
            tint: RgbColor::new(0x8F_D0_FF),
            primary: &particle::ENCHANT,
            secondary: &particle::NOTE,
            cast_sound: &sound::BLOCK_ENCHANTMENT_TABLE_USE,
            impact_sound: &sound::BLOCK_ENCHANTMENT_TABLE_USE,
        },
        SpellId::ManaShield => SpellTheme {
            tint: RgbColor::new(0x5A_DF_FF),
            primary: &particle::ENCHANT,
            secondary: &particle::END_ROD,
            cast_sound: &sound::BLOCK_CONDUIT_ACTIVATE,
            impact_sound: &sound::BLOCK_CONDUIT_ACTIVATE,
        },
    }
}

/// The rune model and pack sound shared by a spell's cast and impact.
#[must_use]
pub(crate) const fn school_assets(spell: SpellId) -> (display::SpellModel, SpellSound) {
    use display::SpellModel;
    match spell {
        SpellId::Fireball | SpellId::Meteor => (SpellModel::RuneFire, SpellSound::Fire),
        SpellId::IceSpear | SpellId::Shackles => (SpellModel::RuneFrost, SpellSound::Frost),
        SpellId::ChainLightning
        | SpellId::ManaStorm
        | SpellId::Repulse
        | SpellId::Haste
        | SpellId::Detection => (SpellModel::RuneStorm, SpellSound::Lightning),
        SpellId::LesserHeal
        | SpellId::Restoration
        | SpellId::Cleanse
        | SpellId::Barrier
        | SpellId::GreaterHeal
        | SpellId::Sanctuary
        | SpellId::LastStand
        | SpellId::BlessingOfStrength
        | SpellId::NightVision => (SpellModel::RuneHoly, SpellSound::Holy),
        SpellId::MonarchsWrath => (SpellModel::RuneVoid, SpellSound::Monarch),
        SpellId::VoidLance | SpellId::Weaken | SpellId::GravityWell | SpellId::Blink => {
            (SpellModel::RuneVoid, SpellSound::Void)
        }
        SpellId::MagicArrow | SpellId::Meditation | SpellId::ManaShield => {
            (SpellModel::RuneArcane, SpellSound::Arcane)
        }
    }
}

// ---------------------------------------------------------------------------
// Layers a cast is built from.
// ---------------------------------------------------------------------------

/// The cast layer: a school-tinted ring at the caster's feet, a swirl of the
/// school's particles rising around them, and the cast sound.
pub fn cast(world: &World, spell: SpellId, feet: DVec3) {
    let theme = theme(spell);
    let base = DVec3::new(feet.x, feet.y + 0.1, feet.z);
    ring_dust(world, theme.tint, base, CAST_RING_RADIUS, 20);
    spiral(world, theme.secondary, base, 0.6, 2.0, 2.0, 24);
    school_assets(spell).1.play_at(world, feet, 0.9, 1.0);
}

/// The impact layer: a burst of the school's main particle, a ring of its
/// secondary particle and the impact sound.
pub fn impact(world: &World, spell: SpellId, at: DVec3) {
    let theme = theme(spell);
    burst(world, theme.primary, at, 32, 0.45, 0.12);
    ring(world, theme.secondary, at, 1.8, 24);
    school_assets(spell).1.play_at(world, at, 1.0, 0.95);
}

/// The shared fire explosion of Fireball and Meteor: the emitter core, a
/// firework flash, a flame burst and a smoke ring, with the explosion sound.
pub fn explosion(world: &World, at: DVec3) {
    burst(world, &particle::EXPLOSION_EMITTER, at, 2, 0.2, 0.0);
    burst(world, &particle::FIREWORK, at, 40, 0.8, 0.22);
    burst(world, &particle::FLAME, at, 48, 0.9, 0.16);
    burst(world, &particle::LAVA, at, 24, 1.0, 0.08);
    ring(
        world,
        &particle::LARGE_SMOKE,
        DVec3::new(at.x, at.y + 0.2, at.z),
        2.4,
        28,
    );
    ring_dust(
        world,
        RgbColor::new(0xFF_B4_4A_u32 as i32),
        DVec3::new(at.x, at.y + 0.15, at.z),
        3.2,
        40,
    );
    play_at(world, &sound_events::ENTITY_GENERIC_EXPLODE, at, 1.2, 0.85);
    SpellSound::Fire.play_at(world, at, 1.0, 0.8);
}

#[cfg(test)]
mod tests {
    use glam::DVec3;

    use super::{
        cone_points, lightning_points, line_points, ring_points, school_assets, spiral_points,
    };
    use crate::item::model::SPELL_MODELS;
    use crate::spell::ALL;

    #[test]
    fn a_ring_stays_at_radius_and_wraps_the_center() {
        let center = DVec3::new(3.0, 5.0, -2.0);
        let points = ring_points(center, 2.0, 24);
        assert_eq!(points.len(), 24);
        for point in &points {
            let flat = DVec3::new(point.x - center.x, 0.0, point.z - center.z);
            assert!((flat.length() - 2.0).abs() < 1e-9);
            assert!((point.y - center.y).abs() < 1e-9);
        }
        let mean = points.iter().sum::<DVec3>() / points.len() as f64;
        assert!(mean.distance(center) < 1e-9);
    }

    #[test]
    fn a_spiral_rises_and_keeps_its_radius() {
        let base = DVec3::new(0.0, 1.0, 0.0);
        let points = spiral_points(base, 0.5, 3.0, 2.0, 25);
        assert_eq!(points.len(), 25);
        assert!((points[0].y - base.y).abs() < 1e-9);
        let top = points[24];
        assert!((top.y - (base.y + 3.0)).abs() < 1e-9);
        let top_flat = DVec3::new(top.x - base.x, 0.0, top.z - base.z);
        assert!((top_flat.length() - 0.5).abs() < 1e-9);
        for window in points.windows(2) {
            assert!(window[1].y >= window[0].y);
            let flat = DVec3::new(window[1].x - base.x, 0.0, window[1].z - base.z);
            assert!((flat.length() - 0.5).abs() < 1e-9);
        }
    }

    #[test]
    fn a_cone_starts_at_the_apex_and_stays_within_its_angle() {
        let apex = DVec3::new(1.0, 2.0, 3.0);
        let direction = DVec3::new(1.0, 0.0, 0.0);
        let points = cone_points(apex, direction, 5.0, 30.0, 31);
        assert_eq!(points.len(), 31);
        assert!(points[0].distance(apex) < 1e-9);
        // Surface points reach length / cos(angle), the slant of the cone.
        let slant = 5.0 / 30f64.to_radians().cos();
        for point in &points[1..] {
            let offset = *point - apex;
            assert!(offset.length() <= slant + 1e-9);
            let angle = offset.angle_between(direction).to_degrees();
            assert!(angle <= 30.0 + 1e-9, "point {point:?} escapes the cone");
        }
    }

    #[test]
    fn a_line_hits_both_ends_and_steps_evenly() {
        let from = DVec3::new(0.0, 0.0, 0.0);
        let to = DVec3::new(3.0, 0.0, 0.0);
        let points = line_points(from, to, 0.5);
        assert!(points.first().expect("a line has points").distance(from) < 1e-9);
        assert!(points.last().expect("a line has points").distance(to) < 1e-9);
        for window in points.windows(2) {
            assert!((window[1].distance(window[0]) - 0.5).abs() < 1e-9);
        }
    }

    #[test]
    fn lightning_has_exact_endpoints_and_a_jagged_body() {
        let from = DVec3::new(1.0, 2.0, 3.0);
        let to = DVec3::new(8.0, 2.0, 3.0);
        let points = lightning_points(from, to);
        assert_eq!(points.first(), Some(&from));
        assert_eq!(points.last(), Some(&to));
        assert!(
            points[1..points.len() - 1]
                .iter()
                .any(|point| (point.z - from.z).abs() > 0.01)
        );
    }

    #[test]
    fn every_spell_school_model_is_registered_for_the_pack_test() {
        for spell in ALL {
            let (model, _) = school_assets(spell);
            assert!(
                SPELL_MODELS.contains(&model.path()),
                "{spell:?} uses an unregistered model"
            );
        }
    }
}
