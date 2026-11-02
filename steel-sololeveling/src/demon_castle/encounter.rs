//! Who guards a castle floor: packs in every district, archers on the
//! galleries, hounds on the bridges.
//!
//! A floor holds far more mobs than the server should simulate at once, so
//! packs wait where they were planned and appear only when a hunter comes
//! near, up to a live cap per climb. Packs never return once cleared.

use std::iter;
use std::sync::Arc;

use steel_core::world::World;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::{vanilla_attributes, vanilla_entities};
use steel_utils::BlockStateId;
use steel_utils::axis::Axis;
use text_components::format::Color;

use super::districts::great_hall::GALLERY_LAYER;
use super::districts::{WALL, in_aisle};
use super::plan::{Archetype, Cell, FloorPlan};
use super::theme::CastleTheme;
use super::{CastleSlot, DISTRICT_SIZE, GROUND_Y};
use crate::dungeon::DungeonWave;
use crate::dungeon::generator::Theme;
use crate::dungeon::instance::{SpawnedMob, spawn_archer, spawn_class_mob, spawn_scaled_mob};
use crate::dungeon::mobs::{MobClass, MobLabel, MobRarity};
use crate::rank::HunterRank;
use crate::seeded::{SplitMix, hash};

/// Horizontal distance from a pack at which a hunter wakes it.
pub const ACTIVATION_RADIUS: f64 = 40.0;
/// Live castle mobs one climb keeps before further packs wait.
pub const LIVE_CAP: usize = 72;
/// Health an elite has over an ordinary mob of its class.
const ELITE_HEALTH: f64 = 4.0;
/// Damage an elite deals over an ordinary mob of its class.
const ELITE_DAMAGE: f64 = 1.6;
/// Body scale of an elite.
const ELITE_SCALE: f64 = 1.35;
/// Escorts beside the elite of a treasure cache.
const ELITE_ESCORTS: usize = 3;
/// Classes an elite is drawn from: the fighters, never a caster.
const ELITE_CLASSES: [MobClass; 3] = [MobClass::Warrior, MobClass::Guard, MobClass::Berserker];
/// Least distance between the centres of two packs of one district.
const PACK_SPACING: i32 = 14;
/// Step of the spawn-spot scan across a district.
const SCAN_STEP: i32 = 2;

/// Where a mob stands: hall column and stand layer.
pub type Spot = (i32, i32, i32);

/// One kind of castle mob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Foe {
    /// A zombie-bodied fighter of a dungeon class.
    Class(MobClass),
    /// A larger, far tougher fighter guarding a treasure cache.
    Elite(MobClass),
    /// A skeleton with a bow.
    Archer,
    /// A spider running down the hunters.
    Hound,
}

/// A group of mobs waiting at one spot of a floor.
#[derive(Debug, Clone)]
pub struct Pack {
    /// Index of the district in the 4×4 floor grid.
    pub district: usize,
    /// Hall column of the pack's centre.
    pub center: (i32, i32),
    /// Where each mob appears.
    pub spots: Vec<Spot>,
    /// What appears at each spot.
    pub foes: Vec<Foe>,
}

impl Pack {
    /// Returns whether a hunter at hall column `(lx, lz)` wakes the pack.
    #[must_use]
    pub fn wakes(&self, lx: i32, lz: i32) -> bool {
        let dx = f64::from(lx - self.center.0);
        let dz = f64::from(lz - self.center.1);
        dx.hypot(dz) <= ACTIVATION_RADIUS
    }
}

/// Returns how many mobs one pack of `floor` holds.
#[must_use]
pub fn pack_size(floor: u32) -> usize {
    ((4 + usize::try_from(floor / 12).unwrap_or(usize::MAX).min(3)) * 6).div_ceil(5)
}

/// Plans every pack of a floor.
#[must_use]
pub fn floor_packs(plan: &FloorPlan, world_seed: u64) -> Vec<Pack> {
    let mut rng =
        SplitMix::new(world_seed ^ u64::from(plan.floor).wrapping_mul(0xA24B_AED4_963E_E407));
    let size = pack_size(plan.floor);
    let mut packs = Vec::new();
    for (cell, district) in plan.districts() {
        let district_index = cell.1 * 4 + cell.0;
        let (ground, gallery) = match district.archetype {
            Archetype::Courtyard | Archetype::Barracks | Archetype::Prison => (3, 0),
            Archetype::Bridge => (2, 0),
            Archetype::GreatHall => (2, 1),
            Archetype::Arrival | Archetype::Throne | Archetype::GigaThrone => continue,
        };
        if district.door_sides().next().is_none() {
            continue;
        }
        let (floor_spots, gallery_spots) = district_spots(plan, cell);
        let mut centres: Vec<Spot> = Vec::new();
        let first_ground = packs.len();
        for _ in 0..ground {
            if let Some(pack) = pack_at(
                &mut rng,
                &floor_spots,
                &mut centres,
                size,
                district_index,
                |rng| ground_foe(rng, district.archetype),
            ) {
                packs.push(pack);
            }
        }
        enlist_berserker(&mut packs[first_ground..]);
        for _ in 0..gallery {
            if let Some(pack) = pack_at(
                &mut rng,
                &gallery_spots,
                &mut centres,
                size / 2 + 2,
                district_index,
                |_| Foe::Archer,
            ) {
                packs.push(pack);
            }
        }
        if let Some((tu, tv)) = district.treasure_spot() {
            let (lx, lz) = FloorPlan::hall_coords(cell, tu, tv);
            packs.push(guard_pack(
                &mut rng,
                &floor_spots,
                (lx, lz),
                district.archetype,
                district_index,
            ));
        }
    }
    packs
}

/// Makes sure the ground packs of one district hold a berserker: a fighter
/// that rolled another class takes its place, or else the first foe does.
fn enlist_berserker(packs: &mut [Pack]) {
    let berserker = Foe::Class(MobClass::Berserker);
    let foes = packs.iter_mut().flat_map(|pack| pack.foes.iter_mut());
    let mut foes: Vec<&mut Foe> = foes.collect();
    if foes.iter().any(|foe| **foe == berserker) {
        return;
    }
    let slot = foes
        .iter()
        .position(|foe| matches!(foe, Foe::Class(_)))
        .unwrap_or(0);
    if let Some(foe) = foes.get_mut(slot) {
        **foe = berserker;
    }
}

/// The elite and its escorts standing nearest to a treasure cache.
fn guard_pack(
    rng: &mut SplitMix,
    spots: &[Spot],
    (lx, lz): (i32, i32),
    archetype: Archetype,
    district: usize,
) -> Pack {
    let mut near = spots.to_vec();
    near.sort_by_key(|spot| (spot.0 - lx).pow(2) + (spot.1 - lz).pow(2));
    near.truncate(ELITE_ESCORTS + 1);
    let elite = ELITE_CLASSES[rng.index(ELITE_CLASSES.len())];
    let foes = iter::once(Foe::Elite(elite))
        .chain((1..near.len()).map(|_| ground_foe(rng, archetype)))
        .collect();
    Pack {
        district,
        center: (lx, lz),
        spots: near,
        foes,
    }
}

/// Picks a pack centre among `spots` away from the other centres and gathers
/// the nearest spots around it.
fn pack_at(
    rng: &mut SplitMix,
    spots: &[Spot],
    centres: &mut Vec<Spot>,
    size: usize,
    district: usize,
    mut foe: impl FnMut(&mut SplitMix) -> Foe,
) -> Option<Pack> {
    if spots.len() < size {
        return None;
    }
    let far: Vec<Spot> = spots
        .iter()
        .copied()
        .filter(|spot| {
            centres.iter().all(|centre| {
                (spot.0 - centre.0).abs().max((spot.1 - centre.1).abs()) >= PACK_SPACING
            })
        })
        .collect();
    let pool = if far.is_empty() { spots } else { &far };
    let centre = pool[rng.index(pool.len())];
    centres.push(centre);
    let mut around = spots.to_vec();
    around.sort_by_key(|spot| {
        (
            (spot.0 - centre.0).pow(2) + (spot.1 - centre.1).pow(2),
            hash(0, spot.0, spot.2, spot.1),
        )
    });
    around.truncate(size);
    let foes = (0..size).map(|_| foe(rng)).collect();
    Some(Pack {
        district,
        center: (centre.0, centre.1),
        spots: around,
        foes,
    })
}

/// Rolls one mob of a ground pack.
fn ground_foe(rng: &mut SplitMix, archetype: Archetype) -> Foe {
    let roll = rng.next();
    let hounds = if archetype == Archetype::Bridge {
        25
    } else {
        12
    };
    match roll % 100 {
        r if r < hounds => Foe::Hound,
        r if r < hounds + 15 => Foe::Archer,
        _ => Foe::Class(MobClass::from_roll(roll >> 8)),
    }
}

/// Returns the floor stands of the throne room, where a giga boss's adds
/// appear and its eruptions strike.
#[must_use]
pub fn arena_spots(plan: &FloorPlan) -> Vec<Spot> {
    district_spots(plan, plan.throne).0
}

/// Spawns `count` class fighters on `spots` of `plan`'s floor, the nearest
/// ones to `near` left free for the boss.
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "adds need the floor, where they gather and how strong they are"
)]
pub fn spawn_adds(
    world: &Arc<World>,
    slot: CastleSlot,
    plan: &FloorPlan,
    spots: &[Spot],
    near: (i32, i32),
    count: usize,
    wave: DungeonWave,
    rank: HunterRank,
) -> Vec<SpawnedMob> {
    let mut around = spots.to_vec();
    around.sort_by_key(|spot| (spot.0 - near.0).pow(2) + (spot.1 - near.1).pow(2));
    around
        .iter()
        .skip(4)
        .step_by(2)
        .take(count)
        .filter_map(|&(lx, lz, layer)| {
            let class = MobClass::from_roll(hash(plan.floor.into(), lx, layer, lz));
            let position = slot.block(plan.floor, lx, lz, GROUND_Y + layer);
            spawn_class_mob(world, skin(plan.theme), position, class, wave, rank)
        })
        .collect()
}

/// Returns the open stands of a district: floor stands outside the aisles
/// and the stands on its gallery.
fn district_spots(plan: &FloorPlan, cell: Cell) -> (Vec<Spot>, Vec<Spot>) {
    let district = plan.district(cell);
    let mut floor = Vec::new();
    let mut gallery = Vec::new();
    for u in (WALL..DISTRICT_SIZE - WALL).step_by(SCAN_STEP as usize) {
        for v in (WALL..DISTRICT_SIZE - WALL).step_by(SCAN_STEP as usize) {
            let (lx, lz) = FloorPlan::hall_coords(cell, u, v);
            for layer in [1, GALLERY_LAYER + 1] {
                if !stands(plan, lx, lz, layer) {
                    continue;
                }
                if layer == 1 && !in_aisle(district, u, v) {
                    floor.push((lx, lz, layer));
                } else if layer > 1 {
                    gallery.push((lx, lz, layer));
                }
            }
        }
    }
    (floor, gallery)
}

/// Returns whether a mob can stand at `layer`: a full block under its feet
/// and two layers of air above.
fn stands(plan: &FloorPlan, lx: i32, lz: i32, layer: i32) -> bool {
    plan.block(lx, lz, layer).is_none()
        && plan.block(lx, lz, layer + 1).is_none()
        && plan.block(lx, lz, layer - 1).is_some_and(full_top)
}

/// Returns whether a block offers a whole flat top to stand on.
fn full_top(block: BlockStateId) -> bool {
    block.get_static_collision_shape().iter().any(|aabb| {
        aabb.max(Axis::Y) >= 1.0 - 1e-6
            && aabb.min(Axis::X) <= 1e-6
            && aabb.max(Axis::X) >= 1.0 - 1e-6
            && aabb.min(Axis::Z) <= 1e-6
            && aabb.max(Axis::Z) >= 1.0 - 1e-6
    })
}

/// Returns the dungeon look castle zombies wear on floors of `theme`.
const fn skin(theme: CastleTheme) -> Theme {
    match theme {
        CastleTheme::Fortress | CastleTheme::Inferno => Theme::Nether,
        CastleTheme::FrozenCitadel => Theme::Ice,
        CastleTheme::DemonCity => Theme::DeepDark,
        CastleTheme::Catacombs => Theme::Crypt,
    }
}

/// Spawns `pack` on `floor` of `slot`; each mob comes with whether it is an
/// elite.
#[must_use]
pub fn spawn_pack(
    world: &Arc<World>,
    slot: CastleSlot,
    plan: &FloorPlan,
    pack: &Pack,
    wave: DungeonWave,
    rank: HunterRank,
) -> Vec<(SpawnedMob, bool)> {
    let theme = skin(plan.theme);
    pack.spots
        .iter()
        .zip(&pack.foes)
        .filter_map(|(&(lx, lz, layer), &foe)| {
            let position = slot.block(plan.floor, lx, lz, GROUND_Y + layer);
            let elite = matches!(foe, Foe::Elite(_));
            let mob = match foe {
                Foe::Class(class) => spawn_class_mob(world, theme, position, class, wave, rank),
                Foe::Elite(class) => spawn_elite(world, theme, position, class, wave, rank),
                Foe::Archer | Foe::Hound => spawn_beast(world, theme, position, foe, wave),
            };
            mob.map(|mob| (mob, elite))
        })
        .collect()
}

/// Spawns an elite: a class fighter grown larger and far tougher, under a
/// golden plate.
fn spawn_elite(
    world: &Arc<World>,
    theme: Theme,
    position: steel_utils::BlockPos,
    class: MobClass,
    wave: DungeonWave,
    rank: HunterRank,
) -> Option<SpawnedMob> {
    let boosted = DungeonWave {
        mob_count: wave.mob_count,
        health_multiplier: wave.health_multiplier * ELITE_HEALTH,
        damage_multiplier: wave.damage_multiplier * ELITE_DAMAGE,
    };
    let mut mob = spawn_class_mob(world, theme, position, class, boosted, rank)?;
    let entity = world.get_entity_by_id(mob.id)?;
    let living = entity.as_living_entity()?;
    living.set_base_attribute_value(vanilla_attributes::SCALE, ELITE_SCALE);
    entity.refresh_dimensions();
    mob.label = MobLabel {
        name: format!("Элита · {}", class.display_name().to_lowercase()),
        title: "Хранитель тайника".to_owned(),
        color: Color::Gold,
        is_tank: mob.label.is_tank,
        is_broodmother: false,
        rarity: MobRarity::Rare,
    };
    mob.label.show(world, living);
    Some(mob)
}

/// Spawns an archer or a hound with its nameplate.
fn spawn_beast(
    world: &Arc<World>,
    theme: Theme,
    position: steel_utils::BlockPos,
    foe: Foe,
    wave: DungeonWave,
) -> Option<SpawnedMob> {
    let common = |name: &str, title: &str, color| MobLabel {
        name: name.to_owned(),
        title: title.to_owned(),
        color,
        is_tank: false,
        is_broodmother: false,
        rarity: MobRarity::Common,
    };
    if foe == Foe::Archer {
        let label = common("Демон-лучник", "Лучник", Color::Yellow);
        return spawn_archer(world, theme, position, wave, label);
    }
    if foe != Foe::Hound {
        return None;
    }
    let entity = spawn_scaled_mob(
        world,
        &vanilla_entities::SPIDER,
        position,
        wave.health_multiplier * 0.9,
        wave.damage_multiplier * 1.1,
    )?;
    let living = entity.as_living_entity()?;
    let label = common("Адская гончая", "Гончая", Color::DarkRed);
    label.show(world, living);
    Some(SpawnedMob {
        id: entity.id(),
        class: MobClass::Warrior,
        label,
    })
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;

    use super::{Foe, floor_packs, pack_size};
    use crate::demon_castle::plan::FloorPlan;
    use crate::dungeon::mobs::MobClass;

    #[test]
    fn a_floor_is_crowded_and_grows_with_the_climb() {
        init_vanilla_registry();
        for (seed, floor) in [(7_u64, 1_u32), (42, 20), (1_337, 40)] {
            let plan = FloorPlan::generate(seed, floor);
            let packs = floor_packs(&plan, seed);
            let mobs: usize = packs.iter().map(|pack| pack.foes.len()).sum();
            assert!(
                (145..=360).contains(&mobs),
                "seed {seed} floor {floor}: {mobs} mobs on the floor"
            );
            for pack in &packs {
                assert_eq!(pack.spots.len(), pack.foes.len());
            }
            // Every district with packs fields at least one berserker.
            let mut districts: Vec<usize> = packs.iter().map(|pack| pack.district).collect();
            districts.dedup();
            for district in districts {
                assert!(
                    packs
                        .iter()
                        .filter(|pack| pack.district == district)
                        .flat_map(|pack| &pack.foes)
                        .any(|foe| *foe == Foe::Class(MobClass::Berserker)),
                    "seed {seed} floor {floor}: district {district} has no berserker"
                );
            }
            // Every cache has its elite.
            let elites = packs
                .iter()
                .flat_map(|pack| &pack.foes)
                .filter(|foe| matches!(foe, Foe::Elite(_)))
                .count();
            assert_eq!(
                elites,
                plan.treasures().count(),
                "seed {seed} floor {floor}"
            );
            assert!(elites >= 2, "seed {seed} floor {floor}: too few caches");
        }
        assert!(pack_size(40) > pack_size(1));
    }
}
