//! The Demon Castle: an endless climb of dungeon floors.
//!
//! Every floor is a short dungeon opened next to the hunter, ending in a boss.
//! The rank of a floor grows every ten floors from E to S, and inside a rank
//! every floor is a little harder; past floor 60 there is no ceiling, health
//! and damage keep growing by [`OVER_S_STEP`] per floor.
//!
//! Progress is personal and kept at checkpoints every five floors. Entering a
//! floor already moves the hunter's next floor back to the checkpoint, and
//! clearing it moves it forward again, so a death or a lost run costs only the
//! floors since the last checkpoint. Hunters enter with a castle key or with
//! a free entry every [`FREE_ENTRY_SECONDS`]. The highest floor cleared this
//! week is ranked.

use std::cmp::Reverse;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use steel_registry::item_stack::ItemStack;
use uuid::Uuid;

use crate::dungeon::{DungeonWave, wave_for_rank};
use crate::item::artifact::{self, Rarity};
use crate::item::material::{self, Material, Quality};
use crate::rank::HunterRank;

/// Floors between two checkpoints.
pub const CHECKPOINT_EVERY: u32 = 5;
/// Floors of one rank.
const FLOORS_PER_RANK: u32 = 10;
/// Extra health and damage of every floor inside its rank.
const IN_RANK_STEP: f64 = 0.05;
/// Extra health and damage of every floor above the last S floor.
pub const OVER_S_STEP: f64 = 0.08;
/// Last floor whose difficulty comes from the rank table alone.
const LAST_RANKED_FLOOR: u32 = 6 * FLOORS_PER_RANK;
/// Percent chance of a legendary artifact on every tenth floor.
const LEGENDARY_PERCENT: u32 = 25;
/// Percent chance that a boss of a B-rank or higher gate drops a castle key.
pub const CASTLE_KEY_PERCENT: u32 = 10;
/// Seconds between two free entries.
pub const FREE_ENTRY_SECONDS: u64 = 10 * 60;
/// Won the top three of a finished week receive.
pub const WEEKLY_PRIZES: [u64; 3] = [20_000, 10_000, 5_000];
/// Entries shown in the weekly ranking.
pub const RANKING_SIZE: usize = 10;

/// One hunter's climb.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CastleProgress {
    /// Floor the next entry opens.
    #[serde(default = "first_floor")]
    pub next_floor: u32,
    /// Highest checkpoint reached; the climb resumes above it.
    #[serde(default)]
    pub checkpoint: u32,
    /// Highest floor ever cleared.
    #[serde(default)]
    pub best: u32,
    /// Unix second the free entry was last used.
    #[serde(default)]
    pub free_at: u64,
}

const fn first_floor() -> u32 {
    1
}

impl Default for CastleProgress {
    fn default() -> Self {
        Self {
            next_floor: 1,
            checkpoint: 0,
            best: 0,
            free_at: 0,
        }
    }
}

impl CastleProgress {
    /// Starts a floor: returns it and falls back to the checkpoint until the
    /// floor is cleared.
    pub fn enter(&mut self) -> u32 {
        let floor = self.next_floor.max(1);
        self.next_floor = self.checkpoint + 1;
        floor
    }

    /// Records a cleared floor.
    pub fn clear(&mut self, floor: u32) {
        self.next_floor = self.next_floor.max(floor + 1);
        self.best = self.best.max(floor);
        if floor.is_multiple_of(CHECKPOINT_EVERY) {
            self.checkpoint = self.checkpoint.max(floor);
        }
    }

    /// Returns the seconds until the next free entry, zero when it is ready.
    #[must_use]
    pub const fn free_entry_in(self, now: u64) -> u64 {
        self.free_at
            .saturating_add(FREE_ENTRY_SECONDS)
            .saturating_sub(now)
    }
}

/// Returns the rank a floor is fought and paid at.
#[must_use]
pub const fn rank_for_floor(floor: u32) -> HunterRank {
    match floor.saturating_sub(1) / FLOORS_PER_RANK {
        0 => HunterRank::E,
        1 => HunterRank::D,
        2 => HunterRank::C,
        3 => HunterRank::B,
        4 => HunterRank::A,
        _ => HunterRank::S,
    }
}

/// Returns the health and damage multiplier a floor adds to its rank.
#[must_use]
pub fn floor_multiplier(floor: u32) -> f64 {
    let floor = floor.max(1);
    if floor > LAST_RANKED_FLOOR {
        let top = 1.0 + IN_RANK_STEP * f64::from(FLOORS_PER_RANK - 1);
        return top * (1.0 + OVER_S_STEP * f64::from(floor - LAST_RANKED_FLOOR));
    }
    1.0 + IN_RANK_STEP * f64::from((floor - 1) % FLOORS_PER_RANK)
}

/// Returns the wave of a floor.
#[must_use]
pub fn wave(floor: u32) -> DungeonWave {
    let base = wave_for_rank(rank_for_floor(floor));
    let multiplier = floor_multiplier(floor);
    DungeonWave {
        mob_count: base.mob_count,
        health_multiplier: base.health_multiplier * multiplier,
        damage_multiplier: base.damage_multiplier * multiplier,
    }
}

/// Returns the extra floor rewards of one survivor.
#[must_use]
pub fn floor_loot(floor: u32, mut roll: impl FnMut() -> u32) -> Vec<ItemStack> {
    let quality = Quality::for_rank(rank_for_floor(floor));
    let mut loot = vec![
        material::stack(Material::StableEssence, quality, 1),
        material::stack(
            [
                Material::MagicMetal,
                Material::ToughHide,
                Material::ChitinPlate,
                Material::ManaFiber,
            ][(roll() % 4) as usize],
            quality,
            2,
        ),
    ];
    if floor.is_multiple_of(10) && roll() < LEGENDARY_PERCENT {
        let pool: Vec<_> = artifact::ALL
            .into_iter()
            .filter(|id| id.artifact().rarity == Rarity::Legendary)
            .collect();
        if !pool.is_empty() {
            let id = pool[(roll() as usize) % pool.len()];
            loot.push(artifact::stack(id, None));
        }
    }
    loot
}

/// Returns the Monday-based week of a Moscow day.
#[must_use]
pub const fn week_of(day: u64) -> u64 {
    // Day zero of the Unix epoch was a Thursday.
    (day + 3) / 7
}

/// One line of the weekly ranking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RankingEntry {
    /// The hunter.
    pub uuid: Uuid,
    /// The hunter's name when the floor was cleared.
    pub name: String,
    /// Highest floor cleared this week.
    pub floor: u32,
}

/// The weekly ranking of the highest cleared floor, kept in a small file.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ranking {
    /// Week the entries belong to.
    pub week: u64,
    /// Best floors, highest first.
    pub entries: Vec<RankingEntry>,
    /// Prizes of finished weeks not yet paid out.
    #[serde(default)]
    pub owed: Vec<(Uuid, u64)>,
}

impl Ranking {
    /// Records `floor` for `uuid`, rolling over to a new week first.
    pub fn record(&mut self, week: u64, uuid: Uuid, name: &str, floor: u32) {
        self.roll_week(week);
        match self.entries.iter_mut().find(|entry| entry.uuid == uuid) {
            Some(entry) if entry.floor >= floor => return,
            Some(entry) => {
                entry.floor = floor;
                name.clone_into(&mut entry.name);
            }
            None => self.entries.push(RankingEntry {
                uuid,
                name: name.to_owned(),
                floor,
            }),
        }
        self.entries.sort_by_key(|entry| Reverse(entry.floor));
    }

    /// Starts `week` if it is new, owing the prizes of the finished one;
    /// returns whether it rolled over.
    pub fn roll_week(&mut self, week: u64) -> bool {
        if self.week == week {
            return false;
        }
        for (entry, prize) in self.entries.iter().zip(WEEKLY_PRIZES) {
            self.owed.push((entry.uuid, prize));
        }
        self.week = week;
        self.entries.clear();
        true
    }

    /// Takes the prizes owed to `uuid`.
    pub fn take_owed(&mut self, uuid: Uuid) -> u64 {
        let mut total = 0;
        self.owed.retain(|(owner, prize)| {
            if *owner == uuid {
                total += prize;
                false
            } else {
                true
            }
        });
        total
    }

    /// Returns the top of the ranking for `week`.
    #[must_use]
    pub fn top(&self, week: u64) -> &[RankingEntry] {
        if self.week != week {
            return &[];
        }
        &self.entries[..self.entries.len().min(RANKING_SIZE)]
    }
}

/// Loads and saves the ranking next to the hunters' data.
#[derive(Debug)]
pub struct RankingFile {
    path: PathBuf,
    /// The ranking as last loaded or saved.
    pub ranking: Ranking,
}

impl RankingFile {
    /// Loads the ranking at `path`, starting empty when there is none.
    pub fn load(path: &Path) -> io::Result<Self> {
        let ranking = match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
            Err(error) if error.kind() == ErrorKind::NotFound => Ranking::default(),
            Err(error) => return Err(error),
        };
        Ok(Self {
            path: path.to_owned(),
            ranking,
        })
    }

    /// Writes the ranking through a temporary file.
    pub fn save(&self) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(&self.ranking).map_err(io::Error::other)?;
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{
        CastleProgress, FREE_ENTRY_SECONDS, Ranking, WEEKLY_PRIZES, floor_multiplier,
        rank_for_floor, wave, week_of,
    };
    use crate::rank::HunterRank;

    #[test]
    fn floors_climb_through_the_ranks_without_a_ceiling() {
        assert_eq!(rank_for_floor(1), HunterRank::E);
        assert_eq!(rank_for_floor(10), HunterRank::E);
        assert_eq!(rank_for_floor(11), HunterRank::D);
        assert_eq!(rank_for_floor(60), HunterRank::S);
        assert_eq!(rank_for_floor(500), HunterRank::S);
        assert!(wave(12).health_multiplier > wave(11).health_multiplier);
        assert!(floor_multiplier(61) > floor_multiplier(60));
        assert!(floor_multiplier(120) > floor_multiplier(61));
    }

    #[test]
    fn a_failed_floor_falls_back_to_the_checkpoint() {
        let mut progress = CastleProgress::default();
        for floor in 1..=7 {
            assert_eq!(progress.enter(), floor);
            progress.clear(floor);
        }
        assert_eq!(progress.checkpoint, 5);
        assert_eq!(progress.enter(), 8);
        // The hunter dies on floor 8 and never clears it.
        assert_eq!(progress.enter(), 6);
        assert_eq!(progress.best, 7);
    }

    #[test]
    fn the_free_entry_returns_every_ten_minutes() {
        let mut progress = CastleProgress::default();
        let now = 1_800_000_000;
        assert_eq!(progress.free_entry_in(now), 0);
        progress.free_at = now;
        assert_eq!(progress.free_entry_in(now + 1), FREE_ENTRY_SECONDS - 1);
        assert_eq!(progress.free_entry_in(now + FREE_ENTRY_SECONDS), 0);
    }

    #[test]
    fn the_ranking_keeps_each_hunters_best_and_resets_weekly() {
        let mut ranking = Ranking::default();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        ranking.record(5, a, "A", 12);
        ranking.record(5, b, "B", 20);
        ranking.record(5, a, "A", 9);
        assert_eq!(ranking.top(5)[0].floor, 20);
        assert_eq!(ranking.top(5)[1].floor, 12);
        ranking.record(6, a, "A", 3);
        assert_eq!(ranking.top(6).len(), 1);
        assert_eq!(ranking.take_owed(b), WEEKLY_PRIZES[0]);
        assert_eq!(ranking.take_owed(a), WEEKLY_PRIZES[1]);
        assert_eq!(ranking.take_owed(a), 0);
        assert_eq!(week_of(7), week_of(10));
    }
}
