//! Vanilla Mannequin entity: a player-shaped figure without AI.
//!
//! Servers use it for non-player characters: it renders with a player skin,
//! stands still, and shows an optional description line below its name.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_registry::entity_data::ResolvableProfile;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::vanilla_entity_data::MannequinEntityData;
use steel_utils::locks::SyncMutex;
use steel_utils::{DowncastType, DowncastTypeKey};
use text_components::TextComponent;

use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntitySyncedData, LivingEntity, LivingEntityBase,
    SharedEntity,
};
use crate::world::World;

/// Every outer skin layer (cape, jacket, sleeves, trousers, hat) shown.
const ALL_SKIN_LAYERS: i8 = 0x7f;
const TAG_IMMOVABLE: &str = "immovable";

/// Vanilla mannequin entity.
#[entity_behavior(class = "Mannequin")]
pub struct MannequinEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    entity_data: SyncMutex<MannequinEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `MannequinEntity`.
unsafe impl DowncastType for MannequinEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/mannequin");
}

impl MannequinEntity {
    /// Creates a new mannequin at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a mannequin from persisted base entity state.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let living_base = LivingEntityBase::new(entity_type);
        let mut entity_data = MannequinEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data
            .avatar_mut()
            .player_mode_customization
            .set(ALL_SKIN_LAYERS);
        Self {
            base,
            entity_type,
            living_base,
            entity_data: SyncMutex::new(entity_data),
        }
    }

    /// Sets the profile whose skin the mannequin wears.
    pub fn set_profile(&self, profile: ResolvableProfile) {
        self.entity_data.lock().mannequin_mut().profile.set(profile);
    }

    /// Sets the line shown below the mannequin's name, or hides it.
    pub fn set_description(&self, description: Option<TextComponent>) {
        self.entity_data
            .lock()
            .mannequin_mut()
            .description
            .set(description.map(Box::new));
    }

    /// Returns whether players can push the mannequin around.
    #[must_use]
    pub fn is_immovable(&self) -> bool {
        *self.entity_data.lock().mannequin().immovable.get()
    }

    /// Sets whether players can push the mannequin around.
    pub fn set_immovable(&self, immovable: bool) {
        self.entity_data
            .lock()
            .mannequin_mut()
            .immovable
            .set(immovable);
    }
}

impl Entity for MannequinEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        let entity: SharedEntity = Arc::<Self>::clone(&self);
        self.tick_living_entity(&entity);
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn is_pushable(&self) -> bool {
        !self.is_immovable()
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        nbt.insert(TAG_IMMOVABLE, i8::from(self.is_immovable()));
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.set_immovable(nbt.byte(TAG_IMMOVABLE).unwrap_or(0) != 0);
    }
}

impl LivingEntity for MannequinEntity {
    fn living_base(&self) -> &LivingEntityBase {
        &self.living_base
    }

    fn get_health(&self) -> f32 {
        *self.entity_data.lock().living_entity().health.get()
    }

    fn set_health(&self, health: f32) {
        let clamped = health.clamp(0.0, self.get_max_health());
        self.entity_data
            .lock()
            .living_entity_mut()
            .health
            .set(clamped);
    }

    fn push_entities(&self) {}
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use simdnbt::owned::NbtCompound;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::MannequinEntity;
    use crate::entity::Entity;

    #[test]
    fn immovable_mannequins_cannot_be_pushed_and_keep_the_flag() {
        init_vanilla_registry();
        let mannequin =
            MannequinEntity::new(&vanilla_entities::MANNEQUIN, 1, DVec3::ZERO, Weak::new());
        assert!(mannequin.is_pushable());
        mannequin.set_immovable(true);
        assert!(!mannequin.is_pushable());

        let mut nbt = NbtCompound::new();
        mannequin.save_additional(&mut nbt);
        assert_eq!(nbt.byte("immovable"), Some(1));
    }
}
