use std::collections::VecDeque;

use rustc_hash::FxHashSet;
use steel_registry::init_vanilla_registry;

use super::generate;
use crate::dungeon::generator::{Blueprint, RoomRole};

/// Columns a hunter can stand in: ground under the feet and two free blocks
/// above it. Every clearing and trail of the valley is flat.
fn stands(blueprint: &Blueprint) -> FxHashSet<(i32, i32)> {
    let occupied: FxHashSet<(i32, i32, i32)> = blueprint
        .blocks
        .iter()
        .map(|placement| placement.offset)
        .collect();
    occupied
        .iter()
        .filter(|&&(_, y, _)| y == -1)
        .map(|&(x, _, z)| (x, z))
        .filter(|&(x, z)| !occupied.contains(&(x, 0, z)) && !occupied.contains(&(x, 1, z)))
        .collect()
}

fn reachable(stands: &FxHashSet<(i32, i32)>, start: (i32, i32)) -> FxHashSet<(i32, i32)> {
    let mut seen = FxHashSet::default();
    let mut queue = VecDeque::from([start]);
    seen.insert(start);
    while let Some((x, z)) = queue.pop_front() {
        for next in [(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)] {
            if stands.contains(&next) && seen.insert(next) {
                queue.push_back(next);
            }
        }
    }
    seen
}

#[test]
fn every_clearing_the_dungeon_needs_is_reachable_from_the_arrival() {
    init_vanilla_registry();
    for seed in [1_u64, 7, 42, 1_337, 0xDEAD_BEEF, u64::MAX] {
        for camps in 2..=5 {
            let valley = generate(seed, camps);
            let stands = stands(&valley);
            let reached = reachable(&stands, (0, 0));
            for room in &valley.rooms {
                assert!(
                    room.spawns.len() >= 8,
                    "seed {seed}: {:?} clearing has only {} spawn spots",
                    room.role,
                    room.spawns.len()
                );
                for spot in &room.spawns {
                    assert!(
                        reached.contains(spot),
                        "seed {seed}: spawn {spot:?} of the {:?} clearing is cut off",
                        room.role
                    );
                }
            }
            // The boss waits under the throne and the exit opens at the
            // centre: both need open ground the hunters can walk to.
            let arena = valley.boss_room();
            let boss = (arena.center.0, arena.center.1 + arena.half.1 - 3);
            assert!(
                reached.contains(&boss),
                "seed {seed}: the boss spot is cut off"
            );
            let occupied: FxHashSet<(i32, i32, i32)> = valley
                .blocks
                .iter()
                .map(|placement| placement.offset)
                .collect();
            for x in arena.center.0 - 2..=arena.center.0 + 1 {
                for y in 0..=4 {
                    assert!(
                        !occupied.contains(&(x, y, arena.center.1)),
                        "seed {seed}: the exit frame site holds a block at ({x}, {y})"
                    );
                }
            }
            assert_eq!(
                valley
                    .rooms
                    .iter()
                    .filter(|room| room.role == RoomRole::Combat)
                    .count(),
                usize::from(camps)
            );
        }
    }
}

#[test]
fn a_valley_rebuilds_block_for_block_and_stays_affordable_to_build() {
    init_vanilla_registry();
    for seed in [3_u64, 99, 123_456] {
        let valley = generate(seed, 5);
        // A reused region is erased by building the same valley again.
        assert_eq!(valley, generate(seed, 5));
        // The whole valley is written in one tick of the game loop.
        assert!(
            valley.blocks.len() < 120_000,
            "seed {seed}: {} blocks",
            valley.blocks.len()
        );
    }
}
