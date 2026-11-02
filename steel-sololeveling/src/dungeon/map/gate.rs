//! Ordinary-dungeon map projection fitted to the generated blueprint bounds.

use super::{CORRIDOR, SIZE, WALL, ZoneStatus};
use crate::dungeon::generator::{Blueprint, Part, Room};

/// Shared block-to-canvas transform for the drawing and its decorations.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GateProjection {
    min_x: i32,
    min_z: i32,
    scale: f64,
    offset_x: f64,
    offset_y: f64,
}

impl GateProjection {
    #[must_use]
    pub(crate) fn new(blueprint: &Blueprint) -> Self {
        let ((min_x, _, min_z), (max_x, _, max_z)) = blueprint.bounds;
        let width = f64::from((max_x - min_x + 1).max(1));
        let height = f64::from((max_z - min_z + 1).max(1));
        let scale = 127.0 / width.max(height);
        Self {
            min_x,
            min_z,
            scale,
            offset_x: (127.0 - width * scale) * 0.5,
            offset_y: (127.0 - height * scale) * 0.5,
        }
    }

    #[must_use]
    pub(crate) fn point(self, x: i32, z: i32) -> (f64, f64) {
        (
            (f64::from(x - self.min_x) + 0.5) * self.scale + self.offset_x,
            (f64::from(z - self.min_z) + 0.5) * self.scale + self.offset_y,
        )
    }

    fn pixel(self, x: i32, z: i32) -> Option<(usize, usize)> {
        let (px, py) = self.point(x, z);
        let (px, py) = (px.floor() as i32, py.floor() as i32);
        ((0..128).contains(&px) && (0..128).contains(&py)).then_some((px as usize, py as usize))
    }
}

#[must_use]
pub(crate) fn canvas(blueprint: &Blueprint, statuses: &[ZoneStatus]) -> [u8; SIZE * SIZE] {
    let projection = GateProjection::new(blueprint);
    let mut canvas = [WALL; SIZE * SIZE];
    for placement in &blueprint.blocks {
        if !matches!(
            placement.part,
            Part::Floor | Part::FloorAlt | Part::Platform
        ) {
            continue;
        }
        let (x, _, z) = placement.offset;
        if let Some((px, py)) = projection.pixel(x, z) {
            canvas[px + py * SIZE] = CORRIDOR;
        }
    }
    for (index, room) in blueprint.rooms.iter().enumerate() {
        let color = statuses
            .get(index)
            .copied()
            .unwrap_or(ZoneStatus::Unexplored)
            .color();
        fill_room(&mut canvas, projection, room, color);
    }
    canvas
}

fn fill_room(canvas: &mut [u8; SIZE * SIZE], projection: GateProjection, room: &Room, color: u8) {
    for z in room.center.1 - room.half.1..=room.center.1 + room.half.1 {
        for x in room.center.0 - room.half.0..=room.center.0 + room.half.0 {
            if let Some((px, py)) = projection.pixel(x, z) {
                canvas[px + py * SIZE] = color;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;
    use steel_registry::map_color::{Brightness, MapColor};

    use super::{GateProjection, canvas};
    use crate::dungeon::generator::Blueprint;
    use crate::dungeon::map::{SIZE, ZoneStatus};

    #[test]
    fn room_status_changes_repaint_only_the_canvas() {
        init_vanilla_registry();
        let blueprint = Blueprint::generate(9, 2);
        let mut statuses = vec![ZoneStatus::Unexplored; blueprint.rooms.len()];
        statuses[0] = ZoneStatus::Cleared;
        let canvas = canvas(&blueprint, &statuses);
        let projection = GateProjection::new(&blueprint);
        let room = &blueprint.rooms[0];
        let (x, y) = projection.point(room.center.0, room.center.1);
        assert_eq!(
            canvas[x as usize + y as usize * SIZE],
            MapColor::COLOR_LIGHT_GREEN.packed_id(Brightness::Normal)
        );
    }
}
