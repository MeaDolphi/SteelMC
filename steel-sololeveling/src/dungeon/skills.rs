//! Abilities of the dungeon mob classes that do more than fight in melee.
//!
//! Decisions are made under the runtime lock, like the rest of a run. Healing
//! and explosions damage or heal entities, which runs gameplay hooks that take
//! the runtime lock again, so they are returned as [`SkillAction`]s and applied
//! once the lock is released. Hatching only spawns entities, as waves do, and
//! happens in place.

use std::sync::Arc;

use glam::DVec3;
use rustc_hash::FxHashMap;
use steel_core::entity::damage::DamageSource;
use steel_core::entity::{Entity as _, LivingEntity, MobEffectInstance, SharedEntity, spawn_mob};
use steel_core::physics::{CollisionWorld, WorldCollisionProvider};
use steel_core::server::Server;
use steel_core::world::World;
use steel_registry::particle_type::{ParticleData, ParticleType};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::{
    sound_events, vanilla_damage_types, vanilla_entities, vanilla_mob_effects,
    vanilla_particle_types,
};
use steel_utils::{BlockPos, Identifier, WorldAabb};
use text_components::format::Color;

use super::boss::line_of_sight;
use super::mobs::{MobClass, MobLabel, MobRarity};

/// Ticks between two shaman chants.
const SHAMAN_COOLDOWN_TICKS: u64 = 120;
/// Reach of a shaman chant over its allies.
const SHAMAN_RANGE: f64 = 8.0;
/// Share of an ally's maximum health one chant restores.
const SHAMAN_HEAL_SHARE: f32 = 0.25;
/// Distance at which a bomber lights its charge.
const BOMBER_TRIGGER_RANGE: f64 = 3.0;
/// Fuse of a lit charge.
const BOMBER_FUSE_TICKS: u64 = 30;
/// Reach of the blast.
const BOMBER_BLAST_RADIUS: f64 = 4.5;
/// Ticks between two broods of a broodmother.
pub(crate) const BROOD_COOLDOWN_TICKS: u64 = 160;
/// Larvae one brood hatches.
const BROOD_SIZE: usize = 2;
/// Live larvae one broodmother keeps at most.
const BROOD_LIMIT: usize = 4;
/// Candidate blocks one block around a broodmother, keeping hatchlings apart.
const LARVA_SPAWN_OFFSETS: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];
/// Minimum horizontal distance between a hatchling's spawn point and an occupant.
const LARVA_MIN_SEPARATION_SQUARED: f64 = 0.64;
/// How close a hunter has to come before a shaman or broodmother acts.
const AWARE_RANGE: f64 = 14.0;
/// Height of a mob's eyes above its feet, for sight lines.
const EYE_HEIGHT: f64 = 1.6;
/// Range at which a hunter who can see a mob reads its nameplate.
const PLATE_RANGE: f64 = 24.0;

/// Returns the nameplate of a hatched larva.
#[must_use]
pub fn larva_label() -> MobLabel {
    MobLabel {
        name: "Личинка".to_owned(),
        title: "Рой".to_owned(),
        color: Color::DarkPurple,
        is_tank: false,
        is_broodmother: false,
        rarity: MobRarity::Common,
    }
}

#[derive(Debug)]
struct SkillState {
    class: MobClass,
    ready_tick: u64,
    fuse_until: Option<u64>,
    larvae: Vec<i32>,
}

/// Ability state of every caster in one run.
#[derive(Debug, Default)]
pub struct MobSkills {
    casters: FxHashMap<i32, SkillState>,
}

/// What the abilities of one tick produced inside the run.
#[derive(Debug, Default)]
pub struct SkillTick {
    /// Work to apply once the runtime lock is released.
    pub actions: Vec<SkillAction>,
    /// Larvae hatched this tick, to track and discard with the run.
    pub hatched: Vec<(i32, MobLabel)>,
}

impl MobSkills {
    /// Starts tracking `id` when its class has an ability.
    pub fn track(&mut self, id: i32, class: MobClass, tick: u64) {
        if matches!(
            class,
            MobClass::Shaman | MobClass::Bomber | MobClass::Broodmother
        ) {
            self.casters.insert(
                id,
                SkillState {
                    class,
                    // A fresh caster waits a moment before its first use.
                    ready_tick: tick + 40,
                    fuse_until: None,
                    larvae: Vec::new(),
                },
            );
        }
    }

    /// Advances every caster; `hunters` are the positions of the run's live
    /// members, `allies` the ids of mobs a shaman may heal, and `power` the
    /// blast damage at its centre.
    pub fn tick(
        &mut self,
        world: &Arc<World>,
        hunters: &[DVec3],
        allies: &[i32],
        power: f32,
        tick: u64,
    ) -> SkillTick {
        let mut out = SkillTick::default();
        self.casters.retain(|id, state| {
            let Some(caster) = world.get_entity_by_id(*id) else {
                return false;
            };
            if caster
                .as_living_entity()
                .is_none_or(LivingEntity::is_dead_or_dying)
            {
                return false;
            }
            let position = caster.position();
            let nearest = hunters
                .iter()
                .map(|hunter| hunter.distance(position))
                .fold(f64::INFINITY, f64::min);
            match state.class {
                MobClass::Shaman => {
                    if tick >= state.ready_tick && nearest <= AWARE_RANGE {
                        let targets = allies
                            .iter()
                            .copied()
                            .filter(|ally| {
                                world.get_entity_by_id(*ally).is_some_and(|entity| {
                                    entity.position().distance(position) <= SHAMAN_RANGE
                                })
                            })
                            .collect::<Vec<_>>();
                        if !targets.is_empty() {
                            state.ready_tick = tick + SHAMAN_COOLDOWN_TICKS;
                            out.actions.push(SkillAction {
                                world: world.key.clone(),
                                caster: *id,
                                kind: SkillKind::Chant { targets },
                            });
                        }
                    }
                    true
                }
                MobClass::Bomber => match state.fuse_until {
                    None if tick >= state.ready_tick && nearest <= BOMBER_TRIGGER_RANGE => {
                        state.fuse_until = Some(tick + BOMBER_FUSE_TICKS);
                        caster.play_sound(&sound_events::ENTITY_TNT_PRIMED, 1.0, 1.0);
                        particles(world, &vanilla_particle_types::SMOKE, position, 20, 0.4);
                        true
                    }
                    Some(fuse_until) if tick >= fuse_until => {
                        out.actions.push(SkillAction {
                            world: world.key.clone(),
                            caster: *id,
                            kind: SkillKind::Blast { power },
                        });
                        false
                    }
                    Some(_) => {
                        particles(world, &vanilla_particle_types::SMOKE, position, 3, 0.2);
                        true
                    }
                    None => true,
                },
                MobClass::Broodmother => {
                    state
                        .larvae
                        .retain(|larva| world.get_entity_by_id(*larva).is_some());
                    if tick >= state.ready_tick
                        && nearest <= AWARE_RANGE
                        && state.larvae.len() < BROOD_LIMIT
                    {
                        state.ready_tick = tick + BROOD_COOLDOWN_TICKS;
                        caster.play_sound(&sound_events::ENTITY_TURTLE_EGG_HATCH, 1.0, 0.6);
                        particles(world, &vanilla_particle_types::PORTAL, position, 30, 0.6);
                        let wanted = BROOD_SIZE.min(BROOD_LIMIT - state.larvae.len());
                        let mut occupied = state
                            .larvae
                            .iter()
                            .filter_map(|id| world.get_entity_by_id(*id))
                            .map(|entity| entity.position())
                            .collect::<Vec<_>>();
                        occupied.push(position);
                        for _ in 0..wanted {
                            if let Some(larva) = hatch(world, position, &occupied) {
                                occupied.push(larva.position());
                                state.larvae.push(larva.id());
                                out.hatched.push((larva.id(), larva_label()));
                            }
                        }
                    }
                    true
                }
                MobClass::Warrior | MobClass::Guard | MobClass::Berserker => false,
            }
        });
        out
    }
}

fn hatch(world: &Arc<World>, around: DVec3, occupied: &[DVec3]) -> Option<SharedEntity> {
    let position = larva_spawn_position(around, occupied)?;
    let entity = spawn_mob(world, &vanilla_entities::ENDERMITE, position).ok()?;
    let living = entity.as_living_entity()?;
    larva_label().show(world, living);
    Some(entity)
}

/// Hatches an allied larva only where it has space and solid support.
pub(crate) fn hatch_shadow(
    world: &Arc<World>,
    around: DVec3,
    occupied: &[DVec3],
) -> Option<SharedEntity> {
    let collision = WorldCollisionProvider::new(world);
    let dimensions = &vanilla_entities::ENDERMITE.dimensions;
    let radius = f64::from(dimensions.width) / 2.0;
    let height = f64::from(dimensions.height);
    for position in larva_spawn_candidates(around, occupied) {
        let x = f64::from(position.x()) + 0.5;
        let y = f64::from(position.y());
        let z = f64::from(position.z()) + 0.5;
        let body = WorldAabb::new(
            x - radius,
            y,
            z - radius,
            x + radius,
            y + height,
            z + radius,
        );
        let support = WorldAabb::new(x - radius, y - 0.1, z - radius, x + radius, y, z + radius);
        if collision.has_block_collision(&body)
            || world.has_entity_in_aabb_matching(&body, |entity| {
                !entity.is_removed() && !entity.is_spectator()
            })
            || collision.has_world_border_collision(&body)
            || !collision.has_block_collision(&support)
        {
            continue;
        }
        match spawn_mob(world, &vanilla_entities::ENDERMITE, position) {
            Ok(entity) => {
                if let Some(living) = entity.as_living_entity() {
                    larva_label().show(world, living);
                }
                return Some(entity);
            }
            Err(error) => {
                log::debug!(
                    "sololeveling: shadow larva could not spawn at {position:?}: {error:?}"
                );
            }
        }
    }
    None
}

fn larva_spawn_position(around: DVec3, occupied: &[DVec3]) -> Option<BlockPos> {
    larva_spawn_candidates(around, occupied).next()
}

fn larva_spawn_candidates(
    around: DVec3,
    occupied: &[DVec3],
) -> impl Iterator<Item = BlockPos> + '_ {
    let base = BlockPos::from(around);
    LARVA_SPAWN_OFFSETS
        .iter()
        .filter_map(move |(offset_x, offset_z)| {
            let candidate = base.offset(*offset_x, 0, *offset_z);
            let center_x = f64::from(candidate.x()) + 0.5;
            let center_z = f64::from(candidate.z()) + 0.5;
            occupied
                .iter()
                .all(|point| {
                    let dx = center_x - point.x;
                    let dz = center_z - point.z;
                    dx * dx + dz * dz >= LARVA_MIN_SEPARATION_SQUARED
                })
                .then_some(candidate)
        })
}

fn particles(world: &World, particle: &'static ParticleType, at: DVec3, count: i32, spread: f64) {
    world.send_particles(
        ParticleData::simple(particle),
        at + DVec3::new(0.0, 1.0, 0.0),
        count,
        DVec3::new(spread, spread, spread),
        0.02,
    );
}

/// Shows the plates of the mobs some hunter can see and hides the rest, so
/// plates no longer lead hunters through walls.
pub fn refresh_plates<'a>(world: &World, hunters: &[DVec3], mobs: impl Iterator<Item = &'a i32>) {
    let eyes = hunters
        .iter()
        .map(|hunter| *hunter + DVec3::new(0.0, EYE_HEIGHT, 0.0))
        .collect::<Vec<_>>();
    for id in mobs {
        let Some(mob) = world.get_entity_by_id(*id) else {
            continue;
        };
        let mob_eye = mob.position() + DVec3::new(0.0, EYE_HEIGHT, 0.0);
        let seen = eyes
            .iter()
            .any(|eye| eye.distance(mob_eye) <= PLATE_RANGE && line_of_sight(world, *eye, mob_eye));
        mob.set_custom_name_visible(seen);
    }
}

/// One ability to apply outside the runtime lock.
#[derive(Debug, Clone)]
pub struct SkillAction {
    world: Identifier,
    caster: i32,
    kind: SkillKind,
}

#[derive(Debug, Clone)]
enum SkillKind {
    /// A shaman heals and empowers its allies.
    Chant { targets: Vec<i32> },
    /// A bomber blows up, hurting hunters around it.
    Blast { power: f32 },
}

impl SkillAction {
    /// Heals, buffs or blasts; a caster that died in the meantime does nothing.
    pub fn apply(&self, server: &Server) {
        let Some(world) = server.worlds.get(&self.world) else {
            return;
        };
        let Some(caster) = world.get_entity_by_id(self.caster) else {
            return;
        };
        if caster
            .as_living_entity()
            .is_none_or(LivingEntity::is_dead_or_dying)
        {
            return;
        }
        match &self.kind {
            SkillKind::Chant { targets } => chant(world, &caster, targets),
            SkillKind::Blast { power } => blast(world, &caster, *power),
        }
    }
}

fn chant(world: &World, caster: &SharedEntity, targets: &[i32]) {
    play(caster, &sound_events::ENTITY_EVOKER_CAST_SPELL, 0.8);
    for target in targets.iter().filter_map(|id| world.get_entity_by_id(*id)) {
        let Some(living) = target.as_living_entity() else {
            continue;
        };
        if living.is_dead_or_dying() {
            continue;
        }
        living.heal(living.get_max_health() * SHAMAN_HEAL_SHARE);
        let _ = living.add_mob_effect(MobEffectInstance::with_duration(
            vanilla_mob_effects::STRENGTH,
            100,
            0,
        ));
        let _ = living.add_mob_effect(MobEffectInstance::with_duration(
            vanilla_mob_effects::SPEED,
            100,
            0,
        ));
        particles(
            world,
            &vanilla_particle_types::HAPPY_VILLAGER,
            target.position(),
            8,
            0.4,
        );
    }
}

fn blast(world: &World, caster: &SharedEntity, power: f32) {
    let center = caster.position();
    world.send_particles(
        ParticleData::simple(&vanilla_particle_types::EXPLOSION_EMITTER),
        center,
        1,
        DVec3::ZERO,
        0.0,
    );
    play(caster, &sound_events::ENTITY_GENERIC_EXPLODE, 1.0);
    let source = DamageSource::environment(&vanilla_damage_types::EXPLOSION)
        .with_causing_entity(Arc::clone(caster));
    let mut hit = Vec::new();
    world.players.iter_players(|_, player| {
        let distance = player.position().distance(center);
        if distance <= BOMBER_BLAST_RADIUS {
            hit.push((Arc::clone(player), distance));
        }
        true
    });
    for (player, distance) in hit {
        let falloff = (1.0 - distance / BOMBER_BLAST_RADIUS).max(0.2) as f32;
        player.hurt(world, &source, power * falloff);
        let away = player.position() - center;
        let horizontal = DVec3::new(away.x, 0.0, away.z);
        if horizontal.length() > 0.01 {
            player.push_impulse(horizontal.normalize() * 0.8 + DVec3::new(0.0, 0.4, 0.0));
        }
    }
    caster.hurt(
        world,
        &DamageSource::environment(&vanilla_damage_types::GENERIC_KILL),
        f32::MAX,
    );
}

fn play(caster: &SharedEntity, sound: SoundEventRef, pitch: f32) {
    caster.play_sound(sound, 1.2, pitch);
}

#[cfg(test)]
mod tests {
    use glam::DVec3;

    use super::larva_spawn_position;

    #[test]
    fn brood_hatchlings_get_distinct_positions_outside_the_mother() {
        let mother = DVec3::new(10.5, 64.0, 10.5);
        let first = larva_spawn_position(mother, &[mother]).expect("first spawn position");
        let first_center = center(first);
        let second =
            larva_spawn_position(mother, &[mother, first_center]).expect("second spawn position");
        let second_center = center(second);

        assert_ne!(first, second);
        assert!(first_center.distance(mother) > 0.8);
        assert!(second_center.distance(mother) > 0.8);
        assert!(first_center.distance(second_center) > 0.8);
    }

    fn center(pos: steel_utils::BlockPos) -> DVec3 {
        DVec3::new(
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()),
            f64::from(pos.z()) + 0.5,
        )
    }
}
