//! Hostile creeper entity.
//!
//! Mirrors vanilla `Creeper` with swell-and-explode behavior.

use std::iter::empty;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::CreeperEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_damage_types};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::ai::goal::{
    HurtByTargetGoal, LookAtPlayerGoal, NearestAttackableTargetGoal, RandomLookAroundGoal,
    SwellGoal, WaterAvoidingRandomStrollGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::monster::DEFAULT_XP_REWARD;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, Monster, PathfinderMob, RemovalReason, SharedEntity,
};
use crate::physics::MoveResult;
use crate::world::World;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const EXPLOSION_RADIUS: f64 = 3.0;
const EXPLOSION_DAMAGE: f32 = 43.0;

/// A hostile creeper entity.
#[entity_behavior(class = "Creeper")]
pub struct CreeperEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<CreeperEntityData>,
}

// SAFETY: The owner-scoped type key uniquely identifies CreeperEntity.
unsafe impl DowncastType for CreeperEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/creeper");
}

impl CreeperEntity {
    /// Creates a new creeper entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved creeper entity.
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
        let mut entity_data = CreeperEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        {
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(2, SwellGoal::new());
            goal_selector.add_goal(5, WaterAvoidingRandomStrollGoal::new(1.0));
            goal_selector.add_goal(6, LookAtPlayerGoal::new(8.0));
            goal_selector.add_goal(6, RandomLookAroundGoal::new());

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

    /// Returns the current creeper swell direction (-1 = deflating).
    #[must_use]
    pub fn swell_dir(&self) -> i32 {
        *self.entity_data.lock().swell_dir.get()
    }

    /// Sets the creeper swell direction.
    pub fn set_swell_dir(&self, dir: i32) {
        self.entity_data.lock().swell_dir.set(dir);
    }

    /// Returns whether this creeper is charged by a lightning strike.
    #[must_use]
    pub fn is_powered(&self) -> bool {
        *self.entity_data.lock().is_powered.get()
    }

    /// Sets whether this creeper is charged by a lightning strike.
    pub fn set_powered(&self, powered: bool) {
        self.entity_data.lock().is_powered.set(powered);
    }

    /// Returns whether this creeper has been manually ignited (flint and steel).
    #[must_use]
    pub fn is_ignited(&self) -> bool {
        *self.entity_data.lock().is_ignited.get()
    }

    /// Sets whether this creeper has been manually ignited.
    pub fn set_ignited(&self, ignited: bool) {
        self.entity_data.lock().is_ignited.set(ignited);
    }

    /// Explodes the creeper, dealing area damage and removing it.
    pub fn explode(&self, entity: &SharedEntity) {
        let Some(world) = self.level() else {
            self.set_removed(RemovalReason::Killed);
            return;
        };

        let origin = self.position();
        let radius = if self.is_powered() {
            EXPLOSION_RADIUS * 2.0
        } else {
            EXPLOSION_RADIUS
        };
        let radius_sqr = radius * radius;
        let search_box = self.bounding_box().inflate_xyz(radius, radius, radius);

        let source = DamageSource::environment(&vanilla_damage_types::EXPLOSION)
            .with_direct_entity(Arc::clone(entity));

        for nearby in world.get_entities_in_aabb(&search_box) {
            if nearby.id() == self.id() {
                continue;
            }
            if nearby.as_living_entity().is_none() {
                continue;
            }
            let distance_sqr = origin.distance_squared(nearby.position());
            if distance_sqr > radius_sqr {
                continue;
            }
            let factor = (1.0 - distance_sqr / radius_sqr).sqrt() as f32;
            let damage = EXPLOSION_DAMAGE * factor;
            nearby.hurt(&world, &source, damage);
        }

        self.set_removed(RemovalReason::Killed);
    }
}

impl Entity for CreeperEntity {
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
        // Creepers have no footstep sound.
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert("powered", i8::from(self.is_powered()));
        nbt.insert("ignited", i8::from(self.is_ignited()));
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.set_powered(nbt.byte("powered").is_some_and(|b| b != 0));
        self.set_ignited(nbt.byte("ignited").is_some_and(|b| b != 0));
    }
}

impl LivingEntity for CreeperEntity {
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
        Some(&sound_events::ENTITY_CREEPER_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_CREEPER_DEATH)
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

impl Mob for CreeperEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl Monster for CreeperEntity {}

impl PathfinderMob for CreeperEntity {}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use steel_registry::{init_vanilla_registry, vanilla_attributes, vanilla_entities};
    use steel_utils::{BlockPos, ChunkPos, Downcast as _};

    use super::CreeperEntity;
    use crate::entity::monster::DEFAULT_XP_REWARD;
    use crate::entity::registry::init_entities;
    use crate::entity::{Entity, LivingEntity, Mob, SharedEntity, spawn_mob};
    use crate::test_support::{fresh_test_world, insert_ready_full_chunk};

    #[test]
    fn spawning_a_creeper_uses_vanilla_attributes_and_reward() {
        init_vanilla_registry();
        init_entities();
        let world_fixture = fresh_test_world("creeper_spawn");
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));

        let spawned: SharedEntity =
            spawn_mob(world, &vanilla_entities::CREEPER, BlockPos::new(8, 64, 8))
                .expect("the creeper factory should be registered");

        assert_eq!(spawned.entity_type().key, vanilla_entities::CREEPER.key);
        let creeper = spawned
            .downcast_ref::<CreeperEntity>()
            .expect("creeper is concrete");

        let attributes = creeper.attributes().lock();
        assert_eq!(
            attributes.get_value(vanilla_attributes::MAX_HEALTH),
            Some(20.0)
        );
        assert_eq!(
            attributes.get_value(vanilla_attributes::MOVEMENT_SPEED),
            Some(0.25)
        );
        drop(attributes);

        assert_eq!(creeper.get_health(), 20.0);
        assert_eq!(creeper.xp_reward(), DEFAULT_XP_REWARD);
        assert!(spawned.as_monster().is_some());
        assert!(!creeper.is_removed());
    }

    #[test]
    fn creeper_swell_state_tracks_and_resets() {
        init_vanilla_registry();
        let creeper = CreeperEntity::new(
            &vanilla_entities::CREEPER,
            1,
            glam::DVec3::ZERO,
            Weak::new(),
        );

        assert_eq!(creeper.swell_dir(), -1);
        creeper.set_swell_dir(10);
        assert_eq!(creeper.swell_dir(), 10);
        creeper.set_swell_dir(-1);
        assert_eq!(creeper.swell_dir(), -1);
    }

    #[test]
    fn creeper_can_be_powered_and_ignited() {
        init_vanilla_registry();
        let creeper = CreeperEntity::new(
            &vanilla_entities::CREEPER,
            1,
            glam::DVec3::ZERO,
            Weak::new(),
        );

        assert!(!creeper.is_powered());
        assert!(!creeper.is_ignited());
        creeper.set_powered(true);
        creeper.set_ignited(true);
        assert!(creeper.is_powered());
        assert!(creeper.is_ignited());
    }
}
