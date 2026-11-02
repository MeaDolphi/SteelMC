//! Resource-pack models used by short-lived spell displays.

use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

use glam::{DVec3, Quat, Vec3};
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
    spawn_transformed(
        world,
        model,
        position,
        scale,
        if horizontal {
            DisplayBillboard::Fixed
        } else {
            DisplayBillboard::Center
        },
        rotation(horizontal, 0.0),
    )
}

/// Spawns a spear whose long axis points along its flight, not at the camera.
pub(crate) fn spawn_projectile(
    world: &Arc<World>,
    model: SpellModel,
    position: DVec3,
    scale: f32,
    direction: DVec3,
) -> Option<i32> {
    spawn_transformed(
        world,
        model,
        position,
        scale,
        DisplayBillboard::Fixed,
        rotation_towards(direction),
    )
}

fn spawn_transformed(
    world: &Arc<World>,
    model: SpellModel,
    position: DVec3,
    scale: f32,
    billboard: DisplayBillboard,
    rotation: Quaternionf,
) -> Option<i32> {
    let display = ItemDisplayEntity::new(
        &vanilla_entities::ITEM_DISPLAY,
        next_entity_id(),
        position,
        Arc::downgrade(world),
    );
    display.set_item(model.stack());
    display.set_billboard(billboard);
    display.set_transformation(Vector3f::ZERO, Vector3f::new(scale, scale, scale), rotation);
    display.set_interpolation(1);
    display.set_brightness(15, 15);
    display.set_persistent(false);
    let id = display.base().id();
    let entity: SharedEntity = Arc::new(display);
    world.try_add_entity(entity).ok().map(|()| id)
}

/// Rotates a ground rune only inside its own plane. Rotating around world Y
/// after laying it down would tilt the circle back towards vertical.
#[must_use]
pub(crate) fn rotation(horizontal: bool, spin: f32) -> Quaternionf {
    if !horizontal {
        return Quaternionf::IDENTITY;
    }
    let rotation = Quat::from_rotation_x(-FRAC_PI_2) * Quat::from_rotation_z(spin);
    let [x, y, z, w] = rotation.to_array();
    Quaternionf::new(x, y, z, w)
}

/// Aligns the model's local Y axis with the flight direction.
#[must_use]
pub(crate) fn rotation_towards(direction: DVec3) -> Quaternionf {
    let direction = direction.normalize_or_zero();
    if direction.length_squared() < 1e-8 {
        return Quaternionf::IDENTITY;
    }
    let target = Vec3::new(direction.x as f32, direction.y as f32, direction.z as f32);
    let rotation = Quat::from_rotation_arc(Vec3::Y, target);
    let [x, y, z, w] = rotation.to_array();
    Quaternionf::new(x, y, z, w)
}

#[cfg(test)]
mod tests {
    use glam::{DVec3, Quat, Vec3};

    use super::{rotation, rotation_towards};

    #[test]
    fn rune_spin_never_tilts_its_ground_plane() {
        for spin in [0.0, 0.7, 2.4, -1.8] {
            let q = rotation(true, spin);
            let rotation = Quat::from_xyzw(q.x, q.y, q.z, q.w);
            let normal = rotation * Vec3::Z;
            assert!(
                (normal - Vec3::Y).length() < 1e-5,
                "spin {spin} tilted the rune: {normal:?}"
            );
        }
    }

    #[test]
    fn a_spear_points_along_its_flight() {
        let direction = DVec3::new(2.0, -1.0, 3.0).normalize();
        let q = rotation_towards(direction);
        let rotation = Quat::from_xyzw(q.x, q.y, q.z, q.w);
        let actual = rotation * Vec3::Y;
        let expected = Vec3::new(direction.x as f32, direction.y as f32, direction.z as f32);
        assert!(
            (actual - expected).length() < 1e-5,
            "{actual:?} != {expected:?}"
        );
    }
}
