//! Telegraphs and class kits of the treasure-cache guards.

use std::sync::Arc;

use glam::DVec3;
use rustc_hash::FxHashMap;
use steel_core::entity::damage::DamageSource;
use steel_core::entity::{Entity, LivingEntity, MobEffectInstance, SharedEntity};
use steel_core::physics::{CollisionWorld as _, WorldCollisionProvider};
use steel_core::server::Server;
use steel_core::world::World;
use steel_registry::{
    sound_events, vanilla_damage_types, vanilla_mob_effects, vanilla_particle_types,
};
use steel_utils::axis::Axis;
use steel_utils::{Identifier, WorldAabb};
use uuid::Uuid;

use crate::dungeon::mobs::MobClass;
use crate::spell_fx as fx;

const INITIAL_DELAY: u64 = 40;
const CHARGE_DELAY: u64 = 16;
const SLAM_DELAY: u64 = 20;
const WARD_DELAY: u64 = 20;
const FLURRY_DELAY: u64 = 15;
const FLURRY_INTERVAL: u64 = 6;
const FLURRY_PULSES: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EliteKind {
    Warrior,
    Guard,
    Berserker,
}

impl EliteKind {
    fn of(class: MobClass) -> Option<Self> {
        match class {
            MobClass::Warrior => Some(Self::Warrior),
            MobClass::Guard => Some(Self::Guard),
            MobClass::Berserker => Some(Self::Berserker),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Pending {
    Charge { target: i32, due: u64 },
    Slam { due: u64 },
    Ward { due: u64 },
    Flurry { remaining: u8, next_hit: u64 },
}

#[derive(Debug)]
struct EliteState {
    kind: EliteKind,
    next_ability: u64,
    next_rage: u64,
    pending: Option<Pending>,
}

impl EliteState {
    fn new(kind: EliteKind, tick: u64) -> Self {
        Self {
            kind,
            next_ability: tick + INITIAL_DELAY,
            next_rage: tick,
            pending: None,
        }
    }

    fn advance_pending(
        &mut self,
        world: &World,
        caster: i32,
        power: f32,
        tick: u64,
    ) -> Option<EliteAction> {
        match self.pending? {
            Pending::Charge { target, due } if tick >= due => {
                self.pending = None;
                Some(EliteAction::new(
                    world,
                    caster,
                    EliteActionKind::Charge {
                        target,
                        damage: power * 0.75,
                    },
                ))
            }
            Pending::Slam { due } if tick >= due => {
                self.pending = None;
                Some(EliteAction::new(
                    world,
                    caster,
                    EliteActionKind::Slam {
                        damage: power * 0.65,
                    },
                ))
            }
            Pending::Ward { due } if tick >= due => {
                self.pending = None;
                Some(EliteAction::new(
                    world,
                    caster,
                    EliteActionKind::Ward {
                        damage: power * 0.5,
                    },
                ))
            }
            Pending::Flurry {
                remaining,
                next_hit,
            } if tick >= next_hit => {
                self.pending = (remaining > 1).then_some(Pending::Flurry {
                    remaining: remaining - 1,
                    next_hit: tick + FLURRY_INTERVAL,
                });
                Some(EliteAction::new(
                    world,
                    caster,
                    EliteActionKind::Flurry {
                        damage: power * 0.35,
                    },
                ))
            }
            _ => None,
        }
    }

    fn tick(
        &mut self,
        world: &Arc<World>,
        caster: &SharedEntity,
        target: Option<&SharedEntity>,
        power: f32,
        tick: u64,
    ) -> Vec<EliteAction> {
        let mut actions = Vec::new();
        if self.kind == EliteKind::Berserker
            && caster
                .as_living_entity()
                .is_some_and(|living| living.get_health() <= living.get_max_health() * 0.5)
            && tick >= self.next_rage
        {
            self.next_rage = tick + 40;
            actions.push(EliteAction::new(world, caster.id(), EliteActionKind::Rage));
        }
        if self.pending.is_some() {
            if let Some(action) = self.advance_pending(world, caster.id(), power, tick) {
                actions.push(action);
            }
            return actions;
        }
        if tick < self.next_ability {
            return actions;
        }
        let Some(target) = target else {
            return actions;
        };
        let distance = caster.position().distance(target.position());
        match self.kind {
            EliteKind::Warrior if distance <= 4.0 => {
                self.pending = Some(Pending::Slam {
                    due: tick + SLAM_DELAY,
                });
                self.next_ability = tick + 180;
                fx::ring(
                    world,
                    &vanilla_particle_types::LARGE_SMOKE,
                    caster.position(),
                    4.0,
                    32,
                );
            }
            EliteKind::Warrior => {
                self.pending = Some(Pending::Charge {
                    target: target.id(),
                    due: tick + CHARGE_DELAY,
                });
                self.next_ability = tick + 180;
                fx::beam(
                    world,
                    &vanilla_particle_types::CRIT,
                    caster.position() + DVec3::Y,
                    target.position() + DVec3::Y,
                );
            }
            EliteKind::Guard if distance <= 6.0 => {
                self.pending = Some(Pending::Ward {
                    due: tick + WARD_DELAY,
                });
                self.next_ability = tick + 240;
                actions.push(EliteAction::new(
                    world,
                    caster.id(),
                    EliteActionKind::WardStart,
                ));
                fx::ring(
                    world,
                    &vanilla_particle_types::SOUL_FIRE_FLAME,
                    caster.position(),
                    4.5,
                    32,
                );
            }
            EliteKind::Berserker if distance <= 5.0 => {
                self.pending = Some(Pending::Flurry {
                    remaining: FLURRY_PULSES,
                    next_hit: tick + FLURRY_DELAY,
                });
                self.next_ability = tick + 180;
                fx::burst(
                    world,
                    &vanilla_particle_types::SWEEP_ATTACK,
                    caster.position() + DVec3::Y,
                    16,
                    0.7,
                    0.04,
                );
            }
            _ => {}
        }
        actions
    }
}

/// Class kits of live elites, separate from ordinary dungeon casters.
#[derive(Debug, Default)]
pub struct EliteSkills {
    elites: FxHashMap<i32, EliteState>,
}

impl EliteSkills {
    /// Tracks a cache guard when its class has a unique kit.
    pub fn track(&mut self, id: i32, class: MobClass, tick: u64) {
        if let Some(kind) = EliteKind::of(class) {
            self.elites.insert(id, EliteState::new(kind, tick));
        }
    }

    /// Plans telegraphs and due actions without changing any entity.
    pub fn tick(
        &mut self,
        world: &Arc<World>,
        members: &[Uuid],
        power: f32,
        tick: u64,
    ) -> Vec<EliteAction> {
        let mut actions = Vec::new();
        self.elites.retain(|id, state| {
            let Some(caster) = world.get_entity_by_id(*id) else {
                return false;
            };
            if caster
                .as_living_entity()
                .is_none_or(LivingEntity::is_dead_or_dying)
            {
                return false;
            }
            let target = nearest(world, members, caster.position(), 14.0);
            actions.extend(state.tick(world, &caster, target.as_ref(), power, tick));
            true
        });
        actions
    }
}

fn can_target(entity: &dyn Entity, members: &[Uuid]) -> bool {
    if entity
        .as_living_entity()
        .is_none_or(LivingEntity::is_dead_or_dying)
    {
        return false;
    }
    if entity
        .as_player()
        .is_some_and(|player| members.contains(&player.gameprofile.id()))
    {
        return true;
    }
    entity.as_mob().is_some_and(|mob| {
        mob.allied_owner()
            .is_some_and(|owner| members.contains(&owner))
    })
}

fn targets_near(world: &World, members: &[Uuid], center: DVec3, radius: f64) -> Vec<SharedEntity> {
    let area = WorldAabb::of_size(center, radius * 2.0, 6.0, radius * 2.0);
    world.get_entities_in_aabb_matching(&area, |entity| {
        can_target(entity, members) && entity.position().distance(center) <= radius
    })
}

fn nearest(world: &World, members: &[Uuid], center: DVec3, radius: f64) -> Option<SharedEntity> {
    targets_near(world, members, center, radius)
        .into_iter()
        .min_by(|first, second| {
            first
                .position()
                .distance_squared(center)
                .total_cmp(&second.position().distance_squared(center))
        })
}

/// One guard action to apply outside the dungeon-runtime lock.
#[derive(Debug, Clone)]
pub struct EliteAction {
    world: Identifier,
    caster: i32,
    kind: EliteActionKind,
}

#[derive(Debug, Clone)]
enum EliteActionKind {
    Rage,
    WardStart,
    Charge { target: i32, damage: f32 },
    Slam { damage: f32 },
    Ward { damage: f32 },
    Flurry { damage: f32 },
}

impl EliteAction {
    fn new(world: &World, caster: i32, kind: EliteActionKind) -> Self {
        Self {
            world: world.key.clone(),
            caster,
            kind,
        }
    }

    /// Applies a guard's action without holding dungeon state.
    pub fn apply(&self, server: &Server, members: &[Uuid]) {
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
        match self.kind {
            EliteActionKind::Rage => {
                if let Some(living) = caster.as_living_entity() {
                    living.add_mob_effect(MobEffectInstance::with_duration(
                        vanilla_mob_effects::STRENGTH,
                        60,
                        0,
                    ));
                    living.add_mob_effect(MobEffectInstance::with_duration(
                        vanilla_mob_effects::SPEED,
                        60,
                        0,
                    ));
                }
                fx::burst(
                    world,
                    &vanilla_particle_types::ANGRY_VILLAGER,
                    caster.position() + DVec3::Y,
                    16,
                    0.5,
                    0.05,
                );
            }
            EliteActionKind::WardStart => {
                if let Some(living) = caster.as_living_entity() {
                    living.add_mob_effect(MobEffectInstance::with_duration(
                        vanilla_mob_effects::RESISTANCE,
                        100,
                        1,
                    ));
                }
                caster.play_sound(&sound_events::ENTITY_RAVAGER_ROAR, 0.8, 0.8);
            }
            EliteActionKind::Charge { target, damage } => {
                charge(world, &caster, members, target, damage);
            }
            EliteActionKind::Slam { damage } => {
                area_hit(world, &caster, members, 4.0, damage, true);
            }
            EliteActionKind::Ward { damage } => {
                area_hit(world, &caster, members, 4.5, damage, true);
            }
            EliteActionKind::Flurry { damage } => {
                area_hit(world, &caster, members, 3.5, damage, false);
            }
        }
    }
}

fn charge(world: &Arc<World>, caster: &SharedEntity, members: &[Uuid], target: i32, damage: f32) {
    let Some(target) = world.get_entity_by_id(target) else {
        return;
    };
    if !can_target(target.as_ref(), members) || target.position().distance(caster.position()) > 18.0
    {
        return;
    }
    let away = caster.position() - target.position();
    let away = DVec3::new(away.x, 0.0, away.z).normalize_or_zero();
    let landing = target.position() + away * 1.4;
    if !standable(world, caster, landing) || caster.teleport_to(landing).is_err() {
        return;
    }
    caster.play_sound(&sound_events::ENTITY_ENDERMAN_TELEPORT, 1.0, 0.8);
    hit(world, caster, &target, damage, false);
}

fn area_hit(
    world: &Arc<World>,
    caster: &SharedEntity,
    members: &[Uuid],
    radius: f64,
    damage: f32,
    knockback: bool,
) {
    caster.play_sound(&sound_events::ENTITY_PLAYER_ATTACK_SWEEP, 1.0, 0.7);
    for target in targets_near(world, members, caster.position(), radius) {
        hit(world, caster, &target, damage, knockback);
    }
}

fn hit(world: &World, caster: &SharedEntity, target: &SharedEntity, damage: f32, knockback: bool) {
    let source = DamageSource::direct(&vanilla_damage_types::MOB_ATTACK, Arc::clone(caster));
    target.hurt(world, &source, damage);
    if !knockback {
        return;
    }
    let away = target.position() - caster.position();
    let away = DVec3::new(away.x, 0.0, away.z).normalize_or_zero();
    target.push_impulse(away * 0.8 + DVec3::new(0.0, 0.25, 0.0));
}

fn standable(world: &Arc<World>, caster: &SharedEntity, position: DVec3) -> bool {
    let collision = WorldCollisionProvider::for_entity(world, caster.as_entity_event_source());
    let body = caster
        .bounding_box()
        .translate(position - caster.position());
    let support = WorldAabb::new(
        body.min(Axis::X),
        body.min(Axis::Y) - 0.1,
        body.min(Axis::Z),
        body.max(Axis::X),
        body.min(Axis::Y),
        body.max(Axis::Z),
    );
    !collision.has_block_collision(&body)
        && !collision.has_entity_collision(&body)
        && !collision.has_world_border_collision(&body)
        && collision.has_block_collision(&support)
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use steel_core::entity::entities::ZombieEntity;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::*;

    fn zombie(id: i32, x: f64) -> SharedEntity {
        Arc::new(ZombieEntity::new(
            &vanilla_entities::ZOMBIE,
            id,
            DVec3::new(x, 0.0, 0.0),
            Weak::new(),
        ))
    }

    #[test]
    fn warrior_charge_waits_for_its_telegraph() {
        init_vanilla_registry();
        let world =
            crate::test_support::world(Arc::new(steel_core::gameplay_hooks::NoopGameplayHooks));
        let caster = zombie(1, 0.0);
        let target = zombie(2, 8.0);
        let mut state = EliteState::new(EliteKind::Warrior, 0);
        assert!(
            state
                .tick(&world.world, &caster, Some(&target), 10.0, 39)
                .is_empty()
        );
        assert!(
            state
                .tick(&world.world, &caster, Some(&target), 10.0, 40)
                .is_empty()
        );
        assert!(
            state
                .tick(&world.world, &caster, Some(&target), 10.0, 55)
                .is_empty()
        );
        let actions = state.tick(&world.world, &caster, Some(&target), 10.0, 56);
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            actions[0].kind,
            EliteActionKind::Charge { target: 2, damage } if (damage - 7.5).abs() < 1e-6
        ));
        assert!(state.pending.is_none());
    }

    #[test]
    fn guard_raises_a_ward_before_the_answer() {
        init_vanilla_registry();
        let world =
            crate::test_support::world(Arc::new(steel_core::gameplay_hooks::NoopGameplayHooks));
        let caster = zombie(1, 0.0);
        let target = zombie(2, 3.0);
        let mut state = EliteState::new(EliteKind::Guard, 0);
        let start = state.tick(&world.world, &caster, Some(&target), 10.0, 40);
        assert_eq!(start.len(), 1);
        assert!(matches!(start[0].kind, EliteActionKind::WardStart));
        assert!(
            state
                .tick(&world.world, &caster, Some(&target), 10.0, 59)
                .is_empty()
        );
        let answer = state.tick(&world.world, &caster, Some(&target), 10.0, 60);
        assert_eq!(answer.len(), 1);
        assert!(matches!(answer[0].kind, EliteActionKind::Ward { .. }));
    }

    #[test]
    fn berserker_rage_does_not_stack_and_flurry_has_three_pulses() {
        init_vanilla_registry();
        let world =
            crate::test_support::world(Arc::new(steel_core::gameplay_hooks::NoopGameplayHooks));
        let caster = zombie(1, 0.0);
        caster
            .as_living_entity()
            .expect("living caster")
            .set_health(10.0);
        let target = zombie(2, 3.0);
        let mut state = EliteState::new(EliteKind::Berserker, 0);
        let rage = state.tick(&world.world, &caster, Some(&target), 10.0, 0);
        assert_eq!(rage.len(), 1);
        assert!(matches!(rage[0].kind, EliteActionKind::Rage));
        assert!(
            state
                .tick(&world.world, &caster, Some(&target), 10.0, 1)
                .is_empty()
        );
        let start = state.tick(&world.world, &caster, Some(&target), 10.0, 40);
        assert_eq!(start.len(), 1);
        assert!(matches!(start[0].kind, EliteActionKind::Rage));
        assert!(
            state
                .tick(&world.world, &caster, Some(&target), 10.0, 54)
                .is_empty()
        );
        for (tick, remaining) in [(55, 2), (61, 1), (67, 0)] {
            let hit = state.tick(&world.world, &caster, Some(&target), 10.0, tick);
            assert_eq!(hit.len(), 1, "pulse at {tick}");
            assert!(matches!(hit[0].kind, EliteActionKind::Flurry { .. }));
            assert_eq!(
                state.pending.map(|pending| match pending {
                    Pending::Flurry { remaining, .. } => remaining,
                    _ => 99,
                }),
                (remaining > 0).then_some(remaining)
            );
        }
        assert!(
            state
                .tick(&world.world, &caster, Some(&target), 10.0, 68)
                .is_empty()
        );
    }

    #[test]
    fn only_hunters_and_their_shadows_are_elite_targets() {
        init_vanilla_registry();
        let world =
            crate::test_support::world(Arc::new(steel_core::gameplay_hooks::NoopGameplayHooks));
        let player = crate::test_support::player(&world.world, "Hunter", 1);
        let hunter = player.gameprofile.id();
        assert!(can_target(player.as_ref(), &[hunter]));
        assert!(!can_target(player.as_ref(), &[]));

        let shadow = zombie(2, 0.0);
        shadow
            .as_mob()
            .expect("shadow body")
            .set_allied_owner(Some(hunter));
        assert!(can_target(shadow.as_ref(), &[hunter]));
        assert!(!can_target(shadow.as_ref(), &[Uuid::new_v4()]));
        let enemy = zombie(3, 0.0);
        assert!(!can_target(enemy.as_ref(), &[hunter]));
    }
}
