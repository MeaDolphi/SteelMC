//! The swirl that fills an open gate's frame.
//!
//! A gate is walked into through its opening ([`super::instance::opening_contains`]);
//! the swirl only shows that the gate is open and what colour it has. It is
//! an item display rendering a pack model across the two-by-three opening, so
//! hunters pass through it like air. The displays are runtime visuals, never
//! saved with their chunk: each sync puts back the ones an unloaded chunk took
//! along.

use std::sync::Arc;

use glam::DVec3;
use rustc_hash::{FxHashMap, FxHashSet};
use steel_core::entity::entities::ItemDisplayEntity;
use steel_core::entity::{Entity as _, RemovalReason, SharedEntity, next_entity_id};
use steel_core::server::Server;
use steel_core::world::World;
use steel_registry::entity_data::{Quaternionf, Vector3f};
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::{vanilla_entities, vanilla_items};
use steel_utils::{BlockPos, Identifier};

use super::Gate;
use super::instance::{FrameStyle, place_gate_frame, standing_frame_style};
use crate::item::model;

/// Colour of one gate's swirl.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VortexStyle {
    /// An ordinary gate.
    Blue,
    /// A gate that turned red behind its hunters.
    Red,
    /// A Demon Castle portal.
    Castle,
}

impl VortexStyle {
    /// Every style, for the pack test.
    pub const ALL: [Self; 3] = [Self::Blue, Self::Red, Self::Castle];

    /// Returns the style `gate` shows right now.
    #[must_use]
    pub const fn of(gate: &Gate) -> Self {
        if gate.floor.is_some() {
            Self::Castle
        } else if gate.red {
            Self::Red
        } else {
            Self::Blue
        }
    }

    /// Returns the pack model of the swirl.
    #[must_use]
    pub const fn model(self) -> &'static str {
        match self {
            Self::Blue => "gate/vortex_blue",
            Self::Red => "gate/vortex_red",
            Self::Castle => "gate/vortex_castle",
        }
    }

    /// Returns the item a client without the pack sees in its place: a pane
    /// of the same colour.
    fn holder(self) -> ItemRef {
        match self {
            Self::Blue => &vanilla_items::LIGHT_BLUE_STAINED_GLASS_PANE,
            Self::Red => &vanilla_items::RED_STAINED_GLASS_PANE,
            Self::Castle => &vanilla_items::PURPLE_STAINED_GLASS_PANE,
        }
    }

    /// Returns the frame a gate of this style stands in.
    const fn frame(self) -> FrameStyle {
        match self {
            Self::Blue => FrameStyle::Normal,
            Self::Red => FrameStyle::Red,
            Self::Castle => FrameStyle::Castle,
        }
    }
}

/// One swirl standing in the world.
struct Vortex {
    world: Identifier,
    entity: i32,
    style: VortexStyle,
}

/// The swirls of every standing gate, by gate id.
#[derive(Default)]
pub struct GateVortices {
    live: FxHashMap<u64, Vortex>,
}

impl GateVortices {
    /// Keeps one swirl in the standing frame of every gate in `gates`, and
    /// restyles a frame whose gate changed colour.
    ///
    /// A frame that does not stand, whether not placed yet or in an unloaded
    /// chunk, gets no swirl.
    pub fn sync<'a>(&mut self, server: &Server, gates: impl Iterator<Item = &'a Gate>) {
        let mut seen = FxHashSet::default();
        for gate in gates {
            let Some(world) = server.worlds.get(&gate.world) else {
                continue;
            };
            seen.insert(gate.id);
            let style = VortexStyle::of(gate);
            let Some(frame) = standing_frame_style(world, gate.entrance) else {
                self.discard(server, gate.id);
                continue;
            };
            if frame != style.frame() {
                place_gate_frame(world, gate.entrance, style.frame());
            }
            let standing = self.live.get(&gate.id).is_some_and(|vortex| {
                vortex.style == style && world.get_entity_by_id(vortex.entity).is_some()
            });
            if standing {
                continue;
            }
            self.discard(server, gate.id);
            if let Some(entity) = spawn(world, gate.entrance, style) {
                self.live.insert(
                    gate.id,
                    Vortex {
                        world: gate.world.clone(),
                        entity,
                        style,
                    },
                );
            }
        }
        let gone: Vec<u64> = self
            .live
            .keys()
            .copied()
            .filter(|id| !seen.contains(id))
            .collect();
        for id in gone {
            self.discard(server, id);
        }
    }

    /// Removes the swirl of gate `id`, if it stands.
    fn discard(&mut self, server: &Server, id: u64) {
        let Some(vortex) = self.live.remove(&id) else {
            return;
        };
        if let Some(entity) = server
            .worlds
            .get(&vortex.world)
            .and_then(|world| world.get_entity_by_id(vortex.entity))
        {
            entity.set_removed(RemovalReason::Discarded);
        }
    }
}

/// Spawns the swirl of a gate based at `entrance` and returns its entity id.
fn spawn(world: &Arc<World>, entrance: BlockPos, style: VortexStyle) -> Option<i32> {
    // The model is a plane one block wide and one and a half high, centred
    // on the entity; doubled, it covers the opening from its sill up.
    let position = DVec3::new(
        f64::from(entrance.x()) + 1.0,
        f64::from(entrance.y()) + 2.0,
        f64::from(entrance.z()) + 0.5,
    );
    let display = ItemDisplayEntity::new(
        &vanilla_entities::ITEM_DISPLAY,
        next_entity_id(),
        position,
        Arc::downgrade(world),
    );
    let mut item = ItemStack::new(style.holder());
    model::apply(&mut item, style.model());
    display.set_item(item);
    display.set_transformation(
        Vector3f::ZERO,
        Vector3f::new(2.0, 2.0, 1.0),
        Quaternionf::IDENTITY,
    );
    display.set_brightness(15, 15);
    display.set_persistent(false);
    let id = display.base().id();
    let entity: SharedEntity = Arc::new(display);
    world.try_add_entity(entity).ok().map(|()| id)
}
