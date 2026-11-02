//! Castle map projection: one 32×32 tile per 80-block district.

use steel_registry::map_color::{Brightness, MapColor};

use super::{SIZE, WALL, ZoneStatus};
use crate::demon_castle::districts;
use crate::demon_castle::plan::FloorPlan;

/// Renders all sixteen districts, leaving their door openings in the status
/// color and every other wall column black.
#[must_use]
pub(crate) fn canvas(plan: &FloorPlan, statuses: &[ZoneStatus; 16]) -> [u8; SIZE * SIZE] {
    let mut canvas = [WALL; SIZE * SIZE];
    for (cell, _) in plan.districts() {
        let index = cell.1 * 4 + cell.0;
        let color = statuses[index].color();
        for py in 0..32 {
            for px in 0..32 {
                let u = (px * 80 / 32) as i32;
                let v = (py * 80 / 32) as i32;
                let (lx, lz) = FloorPlan::hall_coords(cell, u, v);
                let solid_wall = districts::is_wall(u, v) && plan.block(lx, lz, 1).is_some();
                let x = cell.0 * 32 + px;
                let y = cell.1 * 32 + py;
                canvas[x + y * SIZE] = if solid_wall {
                    MapColor::COLOR_BLACK.packed_id(Brightness::High)
                } else {
                    color
                };
            }
        }
    }
    canvas
}

/// Projects hall coordinates onto the 4×4 district grid.
#[must_use]
pub(crate) fn point(lx: i32, lz: i32) -> (f64, f64) {
    (f64::from(lx + 160) * 0.4, f64::from(lz + 160) * 0.4)
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;
    use steel_registry::map_color::{Brightness, MapColor};

    use super::canvas;
    use crate::demon_castle::plan::FloorPlan;
    use crate::dungeon::map::ZoneStatus;

    #[test]
    fn cleared_district_is_green_and_cleared_throne_is_gold() {
        init_vanilla_registry();
        let plan = FloorPlan::generate(7, 1);
        let mut statuses = [ZoneStatus::Unexplored; 16];
        let cleared = (2, 1);
        statuses[cleared.1 * 4 + cleared.0] = ZoneStatus::Cleared;
        statuses[plan.throne.1 * 4 + plan.throne.0] = ZoneStatus::ThroneCleared;
        let canvas = canvas(&plan, &statuses);
        let center = |cell: (usize, usize)| canvas[(cell.0 * 32 + 16) + (cell.1 * 32 + 16) * 128];
        assert_eq!(
            center(cleared),
            MapColor::COLOR_LIGHT_GREEN.packed_id(Brightness::Normal)
        );
        assert_eq!(
            center(plan.throne),
            MapColor::GOLD.packed_id(Brightness::Normal)
        );
    }
}
