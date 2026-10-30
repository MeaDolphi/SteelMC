use steel_registry::vanilla_game_rules::UNIVERSAL_ANGER;
use steel_utils::{DowncastTypeKey, WorldAabb};

use super::selector::{Goal, GoalControls};
use super::target_goal::{TargetGoalBase, follow_distance};
use crate::entity::ai::targeting::TargetingConditions;
use crate::entity::{PathfinderMob, SharedEntity};

const HURT_BY_UNSEEN_MEMORY_TICKS: i32 = 300;
const ALERT_RANGE_Y: f64 = 10.0;

pub(crate) struct HurtByTargetGoal {
    target_goal: TargetGoalBase,
    targeting: TargetingConditions,
    timestamp: i32,
    ignore_damage_types: Vec<DowncastTypeKey>,
    alert_same_type: bool,
    ignore_alert_types: Vec<DowncastTypeKey>,
}

impl HurtByTargetGoal {
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            target_goal: TargetGoalBase::new(true, false),
            targeting: TargetingConditions::for_combat()
                .ignore_line_of_sight()
                .ignore_invisibility_testing(),
            timestamp: 0,
            ignore_damage_types: Vec::new(),
            alert_same_type: false,
            ignore_alert_types: Vec::new(),
        }
    }

    #[must_use]
    pub(crate) fn with_ignored_damage_types(
        mut self,
        types: impl IntoIterator<Item = DowncastTypeKey>,
    ) -> Self {
        self.ignore_damage_types = types.into_iter().collect();
        self
    }

    #[must_use]
    pub(crate) fn set_alert_others(
        mut self,
        except_types: impl IntoIterator<Item = DowncastTypeKey>,
    ) -> Self {
        self.alert_same_type = true;
        self.ignore_alert_types = except_types.into_iter().collect();
        self
    }

    fn alert_others(&self, mob: &dyn PathfinderMob, hurt_by_mob: &SharedEntity) {
        let Some(world) = mob.level() else {
            return;
        };

        let within = follow_distance(mob);
        let position = mob.position();
        let search_box = WorldAabb::new(
            position.x,
            position.y,
            position.z,
            position.x + 1.0,
            position.y + 1.0,
            position.z + 1.0,
        )
        .inflate_xyz(within, ALERT_RANGE_Y, within);
        let mob_type_key = mob.downcast_type_key();
        let mob_uuid = mob.uuid();

        for entity in world.get_entities_in_aabb_matching(&search_box, |entity| {
            if entity.uuid() == mob_uuid
                || entity.downcast_type_key() != mob_type_key
                || self
                    .ignore_alert_types
                    .contains(&entity.downcast_type_key())
            {
                return false;
            }

            let Some(other) = entity.as_mob() else {
                return false;
            };

            //TODO tamable animal owner check is required here
            other.target().is_none() && !other.is_allied_to(hurt_by_mob.as_ref())
        }) {
            if let Some(other) = entity.as_mob() {
                let _ = other.set_target(Some(hurt_by_mob));
            }
        }
    }
}

impl Default for HurtByTargetGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for HurtByTargetGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::TARGET
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let timestamp = mob.last_hurt_by_mob_timestamp();
        if timestamp == self.timestamp {
            return false;
        }

        let Some(hurt_by_mob) = mob.last_hurt_by_mob() else {
            return false;
        };
        if self
            .ignore_damage_types
            .contains(&hurt_by_mob.downcast_type_key())
        {
            return false;
        }

        if hurt_by_mob.as_player().is_some()
            && mob
                .level()
                .is_some_and(|world| world.get_game_rule(&UNIVERSAL_ANGER))
        {
            return false;
        }

        self.target_goal
            .can_attack(mob, hurt_by_mob.as_living_entity(), &self.targeting)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if let (Some(current), Some(attacker), Some(world)) =
            (mob.target(), mob.last_hurt_by_mob(), mob.level())
            && current.uuid() != attacker.uuid()
            && world
                .gameplay_hooks
                .should_retarget_hurt_mob(&world, mob.id(), attacker.id())
        {
            return false;
        }

        self.target_goal.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let Some(hurt_by_mob) = mob.last_hurt_by_mob() else {
            return;
        };

        let _ = mob.set_target(Some(&hurt_by_mob));
        self.target_goal.set_target_mob(mob.target());
        self.timestamp = mob.last_hurt_by_mob_timestamp();
        self.target_goal
            .set_unseen_memory_ticks(HURT_BY_UNSEEN_MEMORY_TICKS);

        if self.alert_same_type {
            self.alert_others(mob, &hurt_by_mob);
        }

        self.target_goal.start();
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.target_goal.stop(mob);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::entity::ai::goal::GoalSelector;
    use crate::entity::entities::{CowEntity, PigEntity};
    use crate::entity::{Entity, LivingEntity, Mob, SharedEntity};
    use crate::gameplay_hooks::GameplayHooks;
    use crate::test_support::{
        fresh_test_world, fresh_test_world_with_gameplay_hooks, insert_ready_full_chunk,
    };
    use crate::world::World;
    use steel_utils::ChunkPos;

    struct RetargetOwnedMobAttacks;

    impl GameplayHooks for RetargetOwnedMobAttacks {
        fn should_retarget_hurt_mob(&self, world: &World, _mob_id: i32, attacker_id: i32) -> bool {
            let Some(attacker_entity) = world.get_entity_by_id(attacker_id) else {
                return false;
            };
            let Some(attacker) = attacker_entity.as_mob() else {
                return false;
            };
            attacker.allied_owner().is_some()
        }
    }

    #[test]
    fn targets_attacker_and_alerts_unassigned_same_type_mobs() {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world("hurt_by_target_goal");
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));

        let hunter = Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            1,
            DVec3::new(8.0, 65.0, 8.0),
            Arc::downgrade(world),
        ));
        let ally = Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            2,
            DVec3::new(9.0, 65.0, 8.0),
            Arc::downgrade(world),
        ));
        let attacker = Arc::new(CowEntity::new(
            &vanilla_entities::COW,
            3,
            DVec3::new(10.0, 65.0, 8.0),
            Arc::downgrade(world),
        ));

        let hunter_entity: SharedEntity = Arc::<PigEntity>::clone(&hunter);
        let ally_entity: SharedEntity = Arc::<PigEntity>::clone(&ally);
        let attacker_entity: SharedEntity = Arc::<CowEntity>::clone(&attacker);
        for entity in [hunter_entity, ally_entity, Arc::clone(&attacker_entity)] {
            world
                .try_add_entity(entity)
                .expect("test entity should attach to the loaded chunk");
        }

        hunter.advance_tick_count();
        hunter.set_last_hurt_by_mob(Some(&attacker_entity));
        let mut goal = HurtByTargetGoal::new().set_alert_others([]);

        assert!(goal.can_use(hunter.as_ref()));
        goal.start(hunter.as_ref());

        let Some(hunter_target) = hunter.target() else {
            panic!("hurt-by-target goal should assign the attacker");
        };
        let Some(ally_target) = ally.target() else {
            panic!("alerted same-type mob should receive the attacker");
        };
        assert_eq!(hunter_target.uuid(), attacker.uuid());
        assert_eq!(ally_target.uuid(), attacker.uuid());
    }

    #[test]
    fn a_new_shadow_attacker_retargets_a_running_hurt_by_goal() {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world_with_gameplay_hooks(
            "hurt_by_target_retargets_shadow",
            Arc::new(RetargetOwnedMobAttacks),
        );
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));

        let victim = Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            1,
            DVec3::new(8.0, 65.0, 8.0),
            Arc::downgrade(world),
        ));
        let first_attacker: SharedEntity = Arc::new(CowEntity::new(
            &vanilla_entities::COW,
            2,
            DVec3::new(9.0, 65.0, 8.0),
            Arc::downgrade(world),
        ));
        let shadow = Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            3,
            DVec3::new(10.0, 65.0, 8.0),
            Arc::downgrade(world),
        ));
        shadow.set_allied_owner(Some(uuid::Uuid::new_v4()));
        let shadow_entity: SharedEntity = shadow;
        let victim_entity: SharedEntity = Arc::<PigEntity>::clone(&victim);
        for entity in [
            Arc::clone(&victim_entity),
            Arc::clone(&first_attacker),
            Arc::clone(&shadow_entity),
        ] {
            world
                .try_add_entity(entity)
                .expect("test entity should attach to the loaded chunk");
        }

        let mut selector = GoalSelector::new();
        selector.add_goal(2, HurtByTargetGoal::new());

        victim.advance_tick_count();
        victim.set_last_hurt_by_mob(Some(&first_attacker));
        selector.tick(victim.as_ref(), &victim_entity);
        assert_eq!(
            victim.target().map(|target| target.uuid()),
            Some(first_attacker.uuid())
        );

        victim.advance_tick_count();
        victim.set_last_hurt_by_mob(Some(&shadow_entity));
        selector.tick(victim.as_ref(), &victim_entity);
        assert_eq!(
            victim.target().map(|target| target.uuid()),
            Some(shadow_entity.uuid())
        );
    }
}
