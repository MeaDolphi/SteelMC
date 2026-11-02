//! Text display entity implementation.

use std::sync::Weak;

use glam::DVec3;
use steel_macros::entity_behavior;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::vanilla_entity_data::TextDisplayEntityData;
use steel_utils::locks::SyncMutex;
use steel_utils::{DowncastType, DowncastTypeKey};
use text_components::TextComponent;
use uuid::Uuid;

use crate::entity::damage::DamageSource;
use crate::entity::{Entity, EntityBase, EntityBaseLoad, EntitySyncedData};
use crate::world::World;

/// How a display entity turns towards the viewer, in vanilla metadata order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i8)]
pub enum DisplayBillboard {
    /// Keeps the entity's own rotation.
    Fixed = 0,
    /// Rotates only around the vertical axis.
    Vertical = 1,
    /// Rotates only around the horizontal axis.
    Horizontal = 2,
    /// Always faces the viewer.
    Center = 3,
}

/// A non-colliding entity that renders one text component.
#[entity_behavior(class = "TextDisplay")]
pub struct TextDisplayEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<TextDisplayEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `TextDisplayEntity`.
unsafe impl DowncastType for TextDisplayEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/text_display");
}

impl TextDisplayEntity {
    /// Creates a text display with a fresh server-generated UUID.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(TextDisplayEntityData::new()),
        }
    }

    /// Creates a text display with a specific persistent UUID.
    #[must_use]
    pub fn with_uuid(
        entity_type: EntityTypeRef,
        id: i32,
        position: DVec3,
        uuid: Uuid,
        world: Weak<World>,
    ) -> Self {
        Self {
            base: EntityBase::with_uuid(id, uuid, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(TextDisplayEntityData::new()),
        }
    }

    /// Creates a text display from its saved base state.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(TextDisplayEntityData::new()),
        }
    }

    /// Replaces the rendered text.
    pub fn set_text(&self, text: TextComponent) {
        self.entity_data.lock().text.set(Box::new(text));
    }

    /// Changes how the text turns towards the viewer.
    pub fn set_billboard(&self, billboard: DisplayBillboard) {
        self.entity_data
            .lock()
            .display
            .billboard_render_constraints
            .set(billboard as i8);
    }
}

impl Entity for TextDisplayEntity {
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
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::vanilla_entities;
    use text_components::TextComponent;

    use super::{DisplayBillboard, TextDisplayEntity};
    use crate::test_support::test_world;

    #[test]
    fn text_and_billboard_are_synced() {
        let world = test_world();
        let display = TextDisplayEntity::new(
            &vanilla_entities::TEXT_DISPLAY,
            1,
            DVec3::ZERO,
            Arc::downgrade(world),
        );
        display.set_text(TextComponent::plain("Exit"));
        display.set_billboard(DisplayBillboard::Center);
        let data = display.entity_data.lock();
        assert_eq!(data.text.get().as_ref(), &TextComponent::plain("Exit"));
        assert_eq!(*data.display.billboard_render_constraints.get(), 3);
    }
}
