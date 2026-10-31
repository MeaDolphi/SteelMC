//! Hostile skeleton entity.
//!
//! Mirrors vanilla `Skeleton` with bow-and-arrow ranged attacks.

use std::iter::empty;
use std::sync::Weak;

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::SkeletonEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_items};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, Downcast, DowncastType, DowncastTypeKey};

use crate::entity::ai::goal::{
    HurtByTargetGoal, LookAtPlayerGoal, NearestAttackableTargetGoal, RandomLookAroundGoal,
    RangedBowAttackGoal, WaterAvoidingRandomStrollGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::entities::ArrowEntity;
use crate::entity::monster::DEFAULT_XP_REWARD;
use crate::entity::projectile::Projectile;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, Monster, PathfinderMob, RangedAttackMob, SharedEntity,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::MoveResult;
use crate::world::World;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const ATTACK_RADIUS: f64 = 15.0;
const ATTACK_INTERVAL_MIN: i32 = 20;
const ATTACK_INTERVAL_MAX: i32 = 40;

/// A hostile skeleton entity.
#[entity_behavior(class = "Skeleton")]
pub struct SkeletonEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<SkeletonEntityData>,
}

// SAFETY: The owner-scoped type key uniquely identifies SkeletonEntity.
unsafe impl DowncastType for SkeletonEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/skeleton");
}

impl SkeletonEntity {
    /// Creates a new skeleton entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved skeleton entity.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let living_base = LivingEntityBase::new(entity_type);
        let mob_base = MobBase::new();
        mob_base.set_xp_reward(DEFAULT_XP_REWARD);
        let mut entity_data = SkeletonEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        {
            let mut equipment = living_base.equipment().lock();
            equipment.set(EquipmentSlot::MainHand, ItemStack::new(&vanilla_items::BOW));
        }

        {
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(
                4,
                RangedBowAttackGoal::new(ATTACK_RADIUS, ATTACK_INTERVAL_MIN, ATTACK_INTERVAL_MAX),
            );
            goal_selector.add_goal(7, WaterAvoidingRandomStrollGoal::new(1.0));
            goal_selector.add_goal(8, LookAtPlayerGoal::new(8.0));
            goal_selector.add_goal(8, RandomLookAroundGoal::new());

            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(1, HurtByTargetGoal::new().set_alert_others(empty()));
            target_selector.add_goal(
                2,
                NearestAttackableTargetGoal::new_for_players(true, |_, _| true),
            );
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
        }
    }

    fn update_dirty_mob_effect_entity_data(&self) {
        if !self.living_base.take_effects_dirty() {
            return;
        }

        let display = self.living_base.mob_effect_display_state();

        {
            let mut entity_data = self.entity_data.lock();
            let living = entity_data.living_entity_mut();
            living.effect_particles.set(display.particles);
            living.effect_ambience.set(display.ambient);
        }

        self.entity_data.set_base_invisible_flag(display.invisible);
    }
}

impl Entity for SkeletonEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if self.entity_type.fixed {
            self.entity_type.dimensions
        } else {
            self.entity_type.dimensions.scale(scale)
        }
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn update_data_before_sync(&self) {
        self.update_dirty_mob_effect_entity_data();
    }

    fn max_up_step(&self) -> f32 {
        self.attributes()
            .lock()
            .get_value(vanilla_attributes::STEP_HEIGHT)
            .unwrap_or(f64::from(DEFAULT_STEP_HEIGHT)) as f32
    }

    fn sound_source(&self) -> SoundSource {
        Monster::sound_source_monster(self)
    }

    fn fall_sounds(&self) -> (SoundEventRef, SoundEventRef) {
        Monster::fall_sounds_monster(self)
    }

    fn swim_sound(&self) -> SoundEventRef {
        Monster::swim_sound_monster(self)
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(&sound_events::ENTITY_SKELETON_STEP, 0.15, 1.0);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
    }
}

impl LivingEntity for SkeletonEntity {
    fn living_base(&self) -> &LivingEntityBase {
        &self.living_base
    }

    fn get_health(&self) -> f32 {
        *self.entity_data.lock().living_entity().health.get()
    }

    fn set_health(&self, health: f32) {
        let max_health = self.get_max_health();
        let clamped = health.clamp(0.0, max_health);
        self.entity_data
            .lock()
            .living_entity_mut()
            .health
            .set(clamped);
    }

    fn sound_volume(&self) -> f32 {
        0.4
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SKELETON_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SKELETON_DEATH)
    }

    fn should_drop_experience(&self) -> bool {
        Monster::should_drop_experience_monster(self)
    }

    fn should_drop_loot(&self, world: &World) -> bool {
        Monster::should_drop_loot_monster(self, world)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        Monster::monster_ai_step(self, entity)
    }
}

impl Mob for SkeletonEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SKELETON_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl Monster for SkeletonEntity {}

impl PathfinderMob for SkeletonEntity {}

impl RangedAttackMob for SkeletonEntity {
    fn perform_ranged_attack(
        &self,
        entity: &SharedEntity,
        _target: &SharedEntity,
        _distance_sq: f64,
    ) {
        let Some(world) = self.level() else {
            return;
        };

        let ammo = ItemStack::new(&vanilla_items::ARROW);
        let weapon = {
            let mut bow = ItemStack::empty();
            self.with_equipment_slot(EquipmentSlot::MainHand, &mut |stack| {
                bow = stack.clone();
            });
            bow
        };

        let Some(arrow) = ArrowEntity::spawn(&world, self, Some(&weapon), &ammo, 1.6, 10.0) else {
            return;
        };

        if let Some(arrow) = arrow.as_ref().downcast_ref::<ArrowEntity>() {
            arrow.set_owner_entity(Some(entity));
        }

        world.play_sound_at(
            &sound_events::ENTITY_SKELETON_SHOOT,
            self.sound_source(),
            self.position(),
            1.0,
            1.0,
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use steel_registry::{
        init_vanilla_registry, vanilla_attributes, vanilla_entities, vanilla_items,
    };
    use steel_utils::{BlockPos, ChunkPos, Downcast as _};

    use super::SkeletonEntity;
    use crate::entity::monster::DEFAULT_XP_REWARD;
    use crate::entity::registry::init_entities;
    use crate::entity::{Entity, LivingEntity, Mob, SharedEntity, spawn_mob};
    use crate::inventory::equipment::EquipmentSlot;
    use crate::test_support::{fresh_test_world, insert_ready_full_chunk};

    #[test]
    fn spawning_a_skeleton_uses_vanilla_attributes_and_reward() {
        init_vanilla_registry();
        init_entities();
        let world_fixture = fresh_test_world("skeleton_spawn");
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));

        let spawned: SharedEntity =
            spawn_mob(world, &vanilla_entities::SKELETON, BlockPos::new(8, 64, 8))
                .expect("the skeleton factory should be registered");

        assert_eq!(spawned.entity_type().key, vanilla_entities::SKELETON.key);
        let skeleton = spawned
            .downcast_ref::<SkeletonEntity>()
            .expect("skeleton is concrete");

        let attributes = skeleton.attributes().lock();
        assert_eq!(
            attributes.get_value(vanilla_attributes::MAX_HEALTH),
            Some(20.0)
        );
        assert_eq!(
            attributes.get_value(vanilla_attributes::MOVEMENT_SPEED),
            Some(0.25)
        );
        drop(attributes);

        assert_eq!(skeleton.get_health(), 20.0);
        assert_eq!(skeleton.xp_reward(), DEFAULT_XP_REWARD);
        assert!(spawned.as_monster().is_some());
        assert!(spawned.as_ranged_attack_mob().is_some());
        assert!(!skeleton.is_removed());
    }

    #[test]
    fn skeleton_spawns_with_bow_in_main_hand() {
        init_vanilla_registry();
        let skeleton = SkeletonEntity::new(
            &vanilla_entities::SKELETON,
            1,
            glam::DVec3::ZERO,
            Weak::new(),
        );

        let mut main_hand = None;
        skeleton.with_equipment_slot(EquipmentSlot::MainHand, &mut |stack| {
            main_hand = Some(stack.clone());
        });
        assert!(main_hand.is_some_and(|stack| stack.is(&vanilla_items::BOW)));
    }
}
