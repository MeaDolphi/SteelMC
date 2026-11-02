//! Where a gate may appear.
//!
//! A gate frame needs open, level ground: never a roof, a lamp post or the top
//! of another frame. The block checks run once the chunks are loaded
//! ([`super::instance::gate_ground_is_level`]); this module picks candidate
//! columns before that, so a gate requested next to a hunter starts on street
//! level instead of wherever a random point fell.

use steel_utils::BlockPos;

use super::instance::gate_footprint;

/// Least horizontal distance between a gate opened by a key or for a test and
/// any other frame: the four-block frame plus a two-block gap.
pub const NEARBY_GATE_MIN_DISTANCE: f64 = 6.0;
/// Nearest a gate requested next to a hunter appears, so it never lands on
/// the hunter.
const NEARBY_MIN_RADIUS: i32 = 4;
/// Farthest a gate requested next to a hunter appears.
const NEARBY_MAX_RADIUS: i32 = 24;

/// Returns whether the frame based at column `(x, z)` stands farther than
/// `min_distance` from every frame in `frames`, measured horizontally.
#[must_use]
pub fn is_free(x: i32, z: i32, frames: &[BlockPos], min_distance: f64) -> bool {
    frames.iter().all(|frame| {
        let dx = f64::from(frame.x() - x);
        let dz = f64::from(frame.z() - z);
        dx * dx + dz * dz >= min_distance * min_distance
    })
}

/// Returns the `attempt`-th site around `around`, nearest first, whose whole
/// footprint `ground` accepts and that keeps its distance from `frames`.
///
/// Each failed placement asks for the next site, so retries walk outwards
/// instead of rolling the same rejected column again.
#[must_use]
pub fn nearby_site(
    around: (i32, i32),
    attempt: usize,
    ground: impl Fn(i32, i32) -> bool,
    frames: &[BlockPos],
) -> Option<(i32, i32)> {
    let mut sites = Vec::new();
    for dx in -NEARBY_MAX_RADIUS..=NEARBY_MAX_RADIUS {
        for dz in -NEARBY_MAX_RADIUS..=NEARBY_MAX_RADIUS {
            let distance = dx * dx + dz * dz;
            let ring =
                NEARBY_MIN_RADIUS * NEARBY_MIN_RADIUS..=NEARBY_MAX_RADIUS * NEARBY_MAX_RADIUS;
            if !ring.contains(&distance) {
                continue;
            }
            sites.push((distance, around.0 + dx, around.1 + dz));
        }
    }
    sites.sort_unstable();
    sites
        .into_iter()
        .filter(|&(_, x, z)| {
            gate_footprint(BlockPos::new(x, 0, z))
                .into_iter()
                .all(|(cx, cz)| ground(cx, cz))
                && is_free(x, z, frames, NEARBY_GATE_MIN_DISTANCE)
        })
        .nth(attempt)
        .map(|(_, x, z)| (x, z))
}

#[cfg(test)]
mod tests {
    use steel_utils::BlockPos;

    use super::{NEARBY_GATE_MIN_DISTANCE, is_free, nearby_site};
    use crate::city::is_gate_ground;
    use crate::dungeon::instance::gate_footprint;

    #[test]
    fn a_gate_beside_a_building_stays_on_the_street() {
        // A hunter standing on the sidewalk in front of the Association: most
        // columns within reach are inside buildings, whose roofs the block
        // check alone would take for ground.
        let hunter = (0, 20);
        let mut frames = Vec::new();
        for attempt in 0..6 {
            let (x, z) = nearby_site(hunter, attempt, is_gate_ground, &frames)
                .expect("a street site near the plaza");
            for (cx, cz) in gate_footprint(BlockPos::new(x, 0, z)) {
                assert!(is_gate_ground(cx, cz), "site ({x}, {z}) uses ({cx}, {cz})");
            }
            frames.push(BlockPos::new(x, 4, z));
        }
        // Gates opened one after another keep their distance.
        for (index, frame) in frames.iter().enumerate() {
            let others: Vec<BlockPos> = frames
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != index)
                .map(|(_, frame)| *frame)
                .collect();
            assert!(is_free(
                frame.x(),
                frame.z(),
                &others,
                NEARBY_GATE_MIN_DISTANCE
            ));
        }
    }

    #[test]
    fn the_next_attempt_never_retries_a_rejected_site() {
        let first = nearby_site((300, 300), 0, |_, _| true, &[]);
        let second = nearby_site((300, 300), 1, |_, _| true, &[]);
        assert!(first.is_some() && second.is_some());
        assert_ne!(first, second);
    }
}
