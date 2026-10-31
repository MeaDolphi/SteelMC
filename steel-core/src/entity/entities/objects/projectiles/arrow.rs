//! Arrow projectile entity (`Arrow`).
//!
//! Mirrors vanilla `Arrow` on the Steel
//! `Projectile → AbstractArrow` trait stack.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_entities;
use steel_registry::vanilla_entity_data::ArrowEntityData;
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::projectile::{
    AbstractArrow, AbstractArrowBase, Projectile, ProjectileBase, ProjectileHit,
};
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntitySyncedData, SharedEntity, next_entity_id,
};
use crate::world::World;

/// A shot arrow.
#[entity_behavior(class = "Arrow")]
pub struct ArrowEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<ArrowEntityData>,
    projectile_base: ProjectileBase,
    abstract_arrow_base: AbstractArrowBase,
}

// SAFETY: The owner-scoped type key uniquely identifies ArrowEntity.
unsafe impl DowncastType for ArrowEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/arrow");
}

impl ArrowEntity {
    /// Creates a new arrow with no owner and default data.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(ArrowEntityData::new()),
            projectile_base: ProjectileBase::new(),
            abstract_arrow_base: AbstractArrowBase::new(),
        }
    }

    /// Creates an arrow from saved base data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(ArrowEntityData::new()),
            projectile_base: ProjectileBase::new(),
            abstract_arrow_base: AbstractArrowBase::new(),
        }
    }

    /// Spawns an arrow shot by `owner` using `weapon` and `ammo`.
    pub fn spawn(
        world: &Arc<World>,
        owner: &dyn Entity,
        weapon: Option<&ItemStack>,
        _ammo: &ItemStack,
        power: f32,
        spread: f32,
    ) -> Option<SharedEntity> {
        let position = owner.position();
        let spawn_pos = DVec3::new(position.x, owner.get_eye_y() - 0.1, position.z);

        let arrow = ArrowEntity::new(
            &vanilla_entities::ARROW,
            next_entity_id(),
            spawn_pos,
            Arc::downgrade(world),
        );
        arrow.set_owner_uuid(Some(owner.uuid()));
        if let Some(weapon) = weapon {
            arrow.set_weapon_item(Some(weapon.clone()));
        }

        let (yaw, pitch) = owner.rotation();
        arrow.shoot_from_rotation(owner, pitch, yaw, 0.0, power, spread);

        let arrow: SharedEntity = Arc::new(arrow);
        world.try_add_entity(Arc::clone(&arrow)).ok()?;
        Some(arrow)
    }
}

impl Entity for ArrowEntity {
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

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_projectile(nbt);
        self.save_abstract_arrow(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_projectile(nbt);
        self.load_abstract_arrow(nbt);
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {}
}

impl Projectile for ArrowEntity {
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

impl AbstractArrow for ArrowEntity {
    fn abstract_arrow_base(&self) -> &AbstractArrowBase {
        &self.abstract_arrow_base
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use crate::entity::Entity;
    use crate::world::World;

    use super::*;

    #[test]
    fn arrow_has_default_gravity_and_damage() {
        init_vanilla_registry();

        let arrow = ArrowEntity::new(
            &vanilla_entities::ARROW,
            1,
            DVec3::ZERO,
            Weak::<World>::new(),
        );

        assert_eq!(arrow.get_default_gravity(), 0.05);
        assert_eq!(arrow.base_damage(), 2.0);
        assert!(!arrow.in_ground());
    }
}
