//! Item display entity implementation.

use std::sync::Weak;
use std::sync::atomic::{AtomicBool, Ordering};

use glam::DVec3;
use steel_macros::entity_behavior;
use steel_registry::entity_data::{Quaternionf, Vector3f};
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_entity_data::ItemDisplayEntityData;
use steel_utils::locks::SyncMutex;
use steel_utils::{DowncastType, DowncastTypeKey};
use uuid::Uuid;

use super::DisplayBillboard;
use crate::entity::damage::DamageSource;
use crate::entity::{Entity, EntityBase, EntityBaseLoad, EntitySyncedData};
use crate::world::World;

/// The model transform an item display renders its item with, in vanilla
/// `ItemDisplayContext` id order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i8)]
pub enum ItemDisplayContext {
    /// No model transform.
    None = 0,
    /// Held in the third-person left hand.
    ThirdPersonLeftHand = 1,
    /// Held in the third-person right hand.
    ThirdPersonRightHand = 2,
    /// Held in the first-person left hand.
    FirstPersonLeftHand = 3,
    /// Held in the first-person right hand.
    FirstPersonRightHand = 4,
    /// Worn on the head.
    Head = 5,
    /// Drawn in an inventory slot.
    Gui = 6,
    /// Lying on the ground.
    Ground = 7,
    /// Mounted in an item frame.
    Fixed = 8,
    /// Standing on a shelf.
    OnShelf = 9,
}

/// A non-colliding entity that renders one item stack.
///
/// Like vanilla, a display is saved with its chunk by default. Steel adds
/// [`ItemDisplayEntity::set_persistent`] for runtime-owned visuals that their
/// owner rebuilds on its own: saving those would leave a stale copy behind
/// after every restart.
#[entity_behavior(class = "ItemDisplay")]
pub struct ItemDisplayEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<ItemDisplayEntityData>,
    persistent: AtomicBool,
}

// SAFETY: This key is owned by Steel and uniquely identifies `ItemDisplayEntity`.
unsafe impl DowncastType for ItemDisplayEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/item_display");
}

impl ItemDisplayEntity {
    /// Creates an item display with a fresh server-generated UUID.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::from_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Creates an item display with a specific persistent UUID.
    #[must_use]
    pub fn with_uuid(
        entity_type: EntityTypeRef,
        id: i32,
        position: DVec3,
        uuid: Uuid,
        world: Weak<World>,
    ) -> Self {
        Self::from_base(
            EntityBase::with_uuid(id, uuid, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Creates an item display from its saved base state.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::from_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn from_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        Self {
            base,
            entity_type,
            entity_data: SyncMutex::new(ItemDisplayEntityData::new()),
            persistent: AtomicBool::new(true),
        }
    }

    /// Replaces the rendered item.
    pub fn set_item(&self, item: ItemStack) {
        self.entity_data.lock().item_stack.set(item);
    }

    /// Changes the model transform the item renders with.
    pub fn set_item_context(&self, context: ItemDisplayContext) {
        self.entity_data.lock().item_display.set(context as i8);
    }

    /// Changes how the item turns towards the viewer.
    pub fn set_billboard(&self, billboard: DisplayBillboard) {
        self.entity_data
            .lock()
            .display
            .billboard_render_constraints
            .set(billboard as i8);
    }

    /// Sets the translation, scale and left rotation of the rendered item.
    pub fn set_transformation(
        &self,
        translation: Vector3f,
        scale: Vector3f,
        rotation: Quaternionf,
    ) {
        let mut data = self.entity_data.lock();
        data.display.translation.set(translation);
        data.display.scale.set(scale);
        data.display.left_rotation.set(rotation);
    }

    /// Sets client-side position and transformation interpolation in ticks.
    pub fn set_interpolation(&self, duration: i32) {
        let mut data = self.entity_data.lock();
        data.display
            .pos_rot_interpolation_duration
            .set(duration.max(0));
        data.display
            .transformation_interpolation_duration
            .set(duration.max(0));
    }

    /// Renders the item at fixed block and sky light levels, each 0 to 15,
    /// instead of the light at its position.
    pub fn set_brightness(&self, block: u8, sky: u8) {
        // Vanilla `LightCoordsUtil.pack`.
        let packed = (i32::from(block.min(15)) << 4) | (i32::from(sky.min(15)) << 20);
        self.entity_data
            .lock()
            .display
            .brightness_override
            .set(packed);
    }

    /// Chooses whether the display is saved with its chunk.
    pub fn set_persistent(&self, persistent: bool) {
        self.persistent.store(persistent, Ordering::Relaxed);
    }
}

impl Entity for ItemDisplayEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn is_ignoring_block_triggers(&self) -> bool {
        true
    }

    fn no_physics(&self) -> bool {
        true
    }

    fn hurt(&self, _world: &World, _source: &DamageSource, _amount: f32) -> bool {
        false
    }

    fn should_save(&self) -> bool {
        self.persistent.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::vanilla_entities;

    use super::ItemDisplayEntity;
    use crate::entity::Entity as _;
    use crate::test_support::test_world;

    #[test]
    fn a_runtime_visual_is_left_out_of_chunk_storage() {
        let world = test_world();
        let display = ItemDisplayEntity::new(
            &vanilla_entities::ITEM_DISPLAY,
            1,
            DVec3::ZERO,
            Arc::downgrade(world),
        );
        assert!(display.should_save(), "a display is saved like in vanilla");
        display.set_persistent(false);
        assert!(!display.should_save());
    }
}
