//! Spectral arrow projectile entity (`SpectralArrow`).
//!
//! Mirrors vanilla `SpectralArrow` on the Steel
//! `Projectile → AbstractArrow` trait stack.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_entity_data::SpectralArrowEntityData;
use steel_registry::vanilla_items;
use steel_registry::vanilla_mob_effects;
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::projectile::{
    AbstractArrow, AbstractArrowBase, Projectile, ProjectileBase, ProjectileHit,
};
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntitySyncedData, LivingEntity, MobEffectInstance,
    SharedEntity,
};
use crate::player::Player;
use crate::world::World;

/// Vanilla default glowing duration of a spectral arrow hit, in ticks.
const DEFAULT_GLOWING_DURATION: i32 = 200;

/// A shot spectral arrow.
#[entity_behavior(class = "SpectralArrow")]
pub struct SpectralArrowEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<SpectralArrowEntityData>,
    projectile_base: ProjectileBase,
    abstract_arrow_base: AbstractArrowBase,
    duration: SyncMutex<i32>,
}

// SAFETY: The owner-scoped type key uniquely identifies SpectralArrowEntity.
unsafe impl DowncastType for SpectralArrowEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/spectral_arrow");
}

impl SpectralArrowEntity {
    /// Creates a new spectral arrow with no owner and default data.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(SpectralArrowEntityData::new()),
            projectile_base: ProjectileBase::new(),
            abstract_arrow_base: AbstractArrowBase::new(),
            duration: SyncMutex::new(DEFAULT_GLOWING_DURATION),
        }
    }

    /// Creates a spectral arrow from saved base data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(SpectralArrowEntityData::new()),
            projectile_base: ProjectileBase::new(),
            abstract_arrow_base: AbstractArrowBase::new(),
            duration: SyncMutex::new(DEFAULT_GLOWING_DURATION),
        }
    }

    /// Vanilla `SpectralArrow.duration`: how long the hit entity glows.
    #[must_use]
    pub fn glowing_duration(&self) -> i32 {
        *self.duration.lock()
    }

    /// Sets how long a hit entity glows, in ticks.
    pub fn set_glowing_duration(&self, duration: i32) {
        *self.duration.lock() = duration;
    }
}

impl Entity for SpectralArrowEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        self.abstract_arrow_tick();
    }

    fn get_default_gravity(&self) -> f64 {
        self.abstract_arrow_default_gravity()
    }

    fn sound_source(&self) -> SoundSource {
        SoundSource::Neutral
    }

    fn spawn_data(&self) -> i32 {
        self.get_owner().map_or(0, |owner| owner.id())
    }

    fn restore_owner_reference(&self, owner: &SharedEntity) {
        self.cache_owner_entity(owner);
    }

    fn projectile_owner_uuid(&self) -> Option<uuid::Uuid> {
        self.owner_uuid()
    }

    fn projectile_owner(&self) -> Option<SharedEntity> {
        self.get_owner()
    }

    fn player_touch(self: Arc<Self>, player: &Arc<Player>) {
        self.abstract_arrow_player_touch(player);
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_projectile(nbt);
        self.save_abstract_arrow(nbt);
        nbt.insert("Duration", self.glowing_duration());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_projectile(nbt);
        self.load_abstract_arrow(nbt);
        if let Some(duration) = nbt.int("Duration") {
            self.set_glowing_duration(duration);
        }
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {}
}

impl Projectile for SpectralArrowEntity {
    fn projectile_base(&self) -> &ProjectileBase {
        &self.projectile_base
    }

    fn on_hit_entity(self: Arc<Self>, entity: &SharedEntity, location: DVec3) {
        AbstractArrow::on_hit_entity(self, entity, location);
    }

    fn on_hit(self: Arc<Self>, hit: &ProjectileHit) {
        AbstractArrow::on_hit(self, hit);
    }
}

impl AbstractArrow for SpectralArrowEntity {
    fn abstract_arrow_base(&self) -> &AbstractArrowBase {
        &self.abstract_arrow_base
    }

    fn default_pickup_item(&self) -> ItemStack {
        ItemStack::new(&vanilla_items::SPECTRAL_ARROW)
    }

    /// Vanilla `SpectralArrow.doPostHurtEffects`: the hit entity glows for
    /// this arrow's duration.
    fn do_post_hurt_effects(self: &Arc<Self>, _world: &World, entity: &dyn LivingEntity) {
        let effect = MobEffectInstance::with_duration(
            vanilla_mob_effects::GLOWING,
            self.glowing_duration(),
            0,
        );
        entity.add_mob_effect_with_source(effect, None);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Weak};

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_items};
    use steel_utils::ChunkPos;

    use crate::behavior::init_behaviors;
    use crate::entity::LivingEntity;
    use crate::test_support::{TestPlayerBuilder, fresh_test_world, insert_ready_full_chunk};
    use crate::world::World;

    use super::*;

    #[test]
    fn spectral_arrow_defaults_to_spectral_pickup_item() {
        init_vanilla_registry();

        let arrow = SpectralArrowEntity::new(
            &vanilla_entities::SPECTRAL_ARROW,
            1,
            DVec3::ZERO,
            Weak::<World>::new(),
        );

        assert!(arrow.pickup_item().is(&vanilla_items::SPECTRAL_ARROW));
        assert_eq!(arrow.glowing_duration(), 200);
    }

    #[test]
    fn spectral_arrow_applies_glowing_on_hit() {
        init_vanilla_registry();
        init_behaviors();

        let world_fixture = fresh_test_world("spectral_arrow_hit");
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let target = TestPlayerBuilder::new(Arc::clone(world), "Target", 1).build();

        let arrow = Arc::new(SpectralArrowEntity::new(
            &vanilla_entities::SPECTRAL_ARROW,
            1000,
            DVec3::new(0.5, 70.0, 0.5),
            Arc::downgrade(world),
        ));
        arrow.set_velocity(DVec3::new(3.0, 0.0, 0.0));
        let shared: SharedEntity = Arc::clone(&arrow) as SharedEntity;
        world
            .try_add_entity(shared)
            .expect("arrow enters the world");

        arrow.do_post_hurt_effects(world, target.as_ref());

        let effect = target
            .mob_effect(vanilla_mob_effects::GLOWING)
            .expect("spectral arrow applies glowing");
        assert_eq!(effect.duration(), 200);
        assert_eq!(effect.amplifier(), 0);
    }
}
