//! Resource-pack models used by short-lived spell displays.

use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

use glam::{DVec3, Quat};
use steel_core::entity::entities::{DisplayBillboard, ItemDisplayEntity};
use steel_core::entity::{Entity as _, SharedEntity, next_entity_id};
use steel_core::world::World;
use steel_registry::entity_data::{Quaternionf, Vector3f};
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::{vanilla_entities, vanilla_items};

use crate::item::model;

/// A model that the spell renderer can place in the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpellModel {
    RuneArcane,
    RuneFire,
    RuneFrost,
    RuneHoly,
    RuneVoid,
    RuneStorm,
    ArcaneBolt,
    Fireball,
    IceSpear,
    VoidLance,
    Shard,
}

impl SpellModel {
    #[must_use]
    pub(crate) const fn path(self) -> &'static str {
        match self {
            Self::RuneArcane => "spell/rune_arcane",
            Self::RuneFire => "spell/rune_fire",
            Self::RuneFrost => "spell/rune_frost",
            Self::RuneHoly => "spell/rune_holy",
            Self::RuneVoid => "spell/rune_void",
            Self::RuneStorm => "spell/rune_storm",
            Self::ArcaneBolt => "spell/arcane_bolt",
            Self::Fireball => "spell/fireball",
            Self::IceSpear => "spell/ice_spear",
            Self::VoidLance => "spell/void_lance",
            Self::Shard => "spell/shard",
        }
    }

    fn holder(self) -> ItemRef {
        match self {
            Self::RuneArcane | Self::ArcaneBolt | Self::RuneStorm => &vanilla_items::AMETHYST_SHARD,
            Self::RuneFire | Self::Fireball => &vanilla_items::FIRE_CHARGE,
            Self::RuneFrost | Self::IceSpear | Self::Shard => &vanilla_items::DIAMOND,
            Self::RuneHoly => &vanilla_items::NETHER_STAR,
            Self::RuneVoid | Self::VoidLance => &vanilla_items::ENDER_PEARL,
        }
    }

    #[must_use]
    pub(crate) fn stack(self) -> ItemStack {
        let mut stack = ItemStack::new(self.holder());
        model::apply(&mut stack, self.path());
        stack
    }
}

/// Spawns a non-persistent model and returns its entity id.
pub(crate) fn spawn(
    world: &Arc<World>,
    model: SpellModel,
    position: DVec3,
    scale: f32,
    horizontal: bool,
) -> Option<i32> {
    let display = ItemDisplayEntity::new(
        &vanilla_entities::ITEM_DISPLAY,
        next_entity_id(),
        position,
        Arc::downgrade(world),
    );
    display.set_item(model.stack());
    display.set_billboard(if horizontal {
        DisplayBillboard::Fixed
    } else {
        DisplayBillboard::Center
    });
    display.set_transformation(
        Vector3f::ZERO,
        Vector3f::new(scale, scale, scale),
        rotation(horizontal, 0.0),
    );
    display.set_interpolation(1);
    display.set_brightness(15, 15);
    display.set_persistent(false);
    let id = display.base().id();
    let entity: SharedEntity = Arc::new(display);
    world.try_add_entity(entity).ok().map(|()| id)
}

/// Rotates a ground rune around its vertical axis, or leaves a projectile
/// facing the viewer.
#[must_use]
pub(crate) fn rotation(horizontal: bool, spin: f32) -> Quaternionf {
    if !horizontal {
        return Quaternionf::IDENTITY;
    }
    let rotation = Quat::from_rotation_x(-FRAC_PI_2) * Quat::from_rotation_y(spin);
    let [x, y, z, w] = rotation.to_array();
    Quaternionf::new(x, y, z, w)
}
