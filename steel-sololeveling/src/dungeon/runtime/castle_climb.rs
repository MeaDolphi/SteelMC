//! One party's climb through the Demon Castle world.
//!
//! A floor wakes its packs as the hunters walk its districts. When every
//! living member stands in the throne room its gate is barred and the floor
//! boss appears; its death hands each survivor the key of the floor and opens
//! two gates beside the aisle — up, for a key holder, and home for whoever
//! has had enough. The first hunter through the gate up moves the climb to
//! the next floor; the rest follow through the same gate with their own keys
//! while the floor below stays loaded for them.

use std::mem;
use std::sync::Arc;

use glam::DVec3;
use rustc_hash::{FxHashMap, FxHashSet};
use steel_core::chunk::chunk_request::{ChunkRequest, ChunkRequestHandle, ChunkTicketKind};
use steel_core::chunk::status::ChunkStatus;
use steel_core::entity::{Entity as _, LivingEntity as _, MobEffectInstance, RemovalReason};
use steel_core::player::Player;
use steel_core::player::connection::NetworkConnection as _;
use steel_core::world::World;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_blocks;
use steel_utils::{BlockPos, BlockStateId};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use super::{RunAction, RunContext};
use crate::castle;
use crate::demon_castle::districts;
use crate::demon_castle::districts::prison::bars;
use crate::demon_castle::elites::{EliteAction, EliteSkills};
use crate::demon_castle::encounter::{self, LIVE_CAP, Pack, Spot};
use crate::demon_castle::giga::{self, Phase};
use crate::demon_castle::plan::{Archetype, FloorPlan, Side};
use crate::demon_castle::{CastleSlot, GROUND_Y, MAX_FLOOR};
use crate::dungeon::boss::{BossFight, BossKind, boss_for_gate};
use crate::dungeon::instance::{
    FrameStyle, opening_contains, place_gate_frame, set_fixture, spawn_boss_body, spawn_gate_label,
};
use crate::dungeon::map::{self, ZoneStatus};
use crate::dungeon::mobs::{MobLabel, MobRarity};
use crate::dungeon::skills::{MobSkills, SkillAction, refresh_plates};
use crate::item::key;
use steel_protocol::packets::game::MapDecoration;

/// Ticks one castle floor may take before the hunters are overwhelmed.
pub(super) const FLOOR_TIME_LIMIT_TICKS: u64 = 20 * 60 * 30;
/// Ticks an enraged guardian keeps its rage: longer than any fight.
const RAGE_TICKS: i32 = 20 * 60 * 30;
/// Ticks between two repeated hints in the action bar.
const HINT_INTERVAL_TICKS: u64 = 40;

/// Lifecycle of a floor's boss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FloorBoss {
    /// Nobody has gathered in the throne room yet.
    Waiting,
    /// The boss is alive with this entity id.
    Alive(i32),
    /// The boss died; its rewards are not handed out yet.
    Defeated,
    /// Keys are handed out and the gates are open.
    Cleared,
}

/// Escalation of a giga fight.
struct GigaFight {
    phase: Phase,
    next_eruption: u64,
    /// Floor stands of the arena, for adds and stray eruptions.
    arena: Vec<Spot>,
}

/// The floor below that teammates may still be climbing out of.
struct LowerFloor {
    floor: u32,
    exit: BlockPos,
    home: BlockPos,
    labels: Vec<i32>,
    _ticket: Option<ChunkRequestHandle>,
}

/// One party's climb.
pub(super) struct CastleClimb {
    /// The party's copy of the castle.
    pub(super) slot: CastleSlot,
    /// Floor the climb fights on.
    pub(super) floor: u32,
    world_seed: u64,
    plan: Arc<FloorPlan>,
    /// Packs still waiting for a hunter.
    packs: Vec<Pack>,
    /// Live castle mobs.
    pub(super) remaining_mobs: Vec<i32>,
    /// District each live mob belongs to, for the map's clear states.
    mob_district: FxHashMap<i32, usize>,
    /// Nameplates of the climb's mobs and boss, by entity id.
    pub(super) labels: FxHashMap<i32, MobLabel>,
    skills: MobSkills,
    elite_skills: EliteSkills,
    /// Abilities to apply once the runtime lock is released.
    pub(super) pending_skills: Vec<SkillAction>,
    pub(super) pending_elite_skills: Vec<EliteAction>,
    /// The floor boss.
    pub(super) boss: FloorBoss,
    /// Ability cooldowns and boss-bar state of a boss that appeared.
    pub(super) fight: Option<BossFight>,
    /// Whether the throne room's gate is barred.
    sealed: bool,
    /// The gate up, open once the floor is cleared.
    exit: Option<BlockPos>,
    /// The gate home, open beside the gate up.
    home: Option<BlockPos>,
    gate_labels: Vec<i32>,
    lower: Option<LowerFloor>,
    /// Keeps the current floor generated and its mobs simulated; the entry
    /// request of the first floor.
    ticket: Option<ChunkRequestHandle>,
    /// The next floor, requested while the boss fights so the gate up is
    /// instant.
    next_ticket: Option<ChunkRequestHandle>,
    /// Floor rewards owed to the survivors, delivered outside the lock.
    pub(super) floor_loot: Vec<(Uuid, ItemStack)>,
    /// Set when the climb moved up a floor, so the run restarts its timer.
    pub(super) advanced: bool,
    /// Hunters who stepped through a gate home, to send back once each.
    pub(super) leaving: Vec<Uuid>,
    left: FxHashSet<Uuid>,
    /// Live elites guarding the floor's treasure caches.
    elites: Vec<i32>,
    /// Escalation of the fight while a giga boss lives.
    giga: Option<GigaFight>,
    /// The floor's treasure caches.
    treasures: Vec<BlockPos>,
    /// Caches each hunter already emptied; every hunter opens each once.
    opened: FxHashSet<(BlockPos, Uuid)>,
}

/// What opening a treasure cache gave the hunter.
pub(super) enum TreasureOpen {
    /// The hunter's share of the cache.
    Looted(Vec<ItemStack>),
    /// The hunter already took their share.
    Empty,
}

impl CastleClimb {
    /// Starts a climb on `floor` of `slot`, whose chunks `ticket` holds.
    pub(super) fn new(
        slot: CastleSlot,
        floor: u32,
        world_seed: u64,
        ticket: Option<ChunkRequestHandle>,
    ) -> Self {
        let plan = Arc::new(FloorPlan::generate(world_seed, floor));
        Self {
            slot,
            floor,
            world_seed,
            packs: encounter::floor_packs(&plan, world_seed),
            plan,
            remaining_mobs: Vec::new(),
            mob_district: FxHashMap::default(),
            labels: FxHashMap::default(),
            skills: MobSkills::default(),
            elite_skills: EliteSkills::default(),
            pending_skills: Vec::new(),
            pending_elite_skills: Vec::new(),
            boss: FloorBoss::Waiting,
            fight: None,
            sealed: false,
            exit: None,
            home: None,
            gate_labels: Vec::new(),
            lower: None,
            ticket,
            next_ticket: None,
            floor_loot: Vec::new(),
            advanced: false,
            leaving: Vec::new(),
            left: FxHashSet::default(),
            elites: Vec::new(),
            giga: None,
            treasures: Vec::new(),
            opened: FxHashSet::default(),
        }
        .with_floor_caches()
    }

    /// Records where the current floor's treasure caches stand.
    fn with_floor_caches(mut self) -> Self {
        self.locate_treasures();
        self
    }

    fn locate_treasures(&mut self) {
        let (slot, floor) = (self.slot, self.floor);
        self.treasures = self
            .plan
            .treasures()
            .map(|(_, (lx, lz))| slot.block(floor, lx, lz, GROUND_Y + 1))
            .collect();
        self.opened.clear();
    }

    /// Hands `hunter` their share of the cache at `pos`, or `None` when no
    /// cache of the current floor stands there.
    pub(super) fn open_treasure(&mut self, pos: BlockPos, hunter: Uuid) -> Option<TreasureOpen> {
        if !self.treasures.contains(&pos) {
            return None;
        }
        if !self.opened.insert((pos, hunter)) {
            return Some(TreasureOpen::Empty);
        }
        Some(TreasureOpen::Looted(castle::treasure_loot(
            self.floor,
            || rand::random_range(0..100),
        )))
    }

    /// Settles the death of `id` if it was an elite: every member of the
    /// climb gets the elite's loot. Returns whether it was one.
    pub(super) fn on_elite_death(&mut self, id: i32, members: &[Uuid]) -> bool {
        let Some(index) = self.elites.iter().position(|elite| *elite == id) else {
            return false;
        };
        self.elites.swap_remove(index);
        for member in members {
            for stack in castle::elite_loot(self.floor, || rand::random_range(0..100)) {
                self.floor_loot.push((*member, stack));
            }
        }
        true
    }

    /// Returns the chunk request a climb starting on `floor` of `slot` waits for.
    pub(super) fn entry_request(slot: CastleSlot, floor: u32) -> ChunkRequest {
        ChunkRequest {
            status: ChunkStatus::Full,
            positions: slot.floor_chunks(floor),
            ticket_kind: ChunkTicketKind::Portal,
        }
    }

    /// Returns where a hunter arrives on the climb's current floor.
    pub(super) fn entry_position(&self) -> DVec3 {
        let (lx, lz) = self.plan.entry();
        let pos = self.slot.block(self.floor, lx, lz, GROUND_Y + 1);
        DVec3::new(
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()),
            f64::from(pos.z()) + 0.5,
        )
    }

    /// Returns the line announcing the current floor.
    pub(super) fn announcement(&self) -> TextComponent {
        let giga = if self.floor.is_multiple_of(10) {
            " Здесь правит великий страж."
        } else {
            ""
        };
        TextComponent::plain(format!(
            "Замок демонов, этаж {}: {}.{giga} Пробейтесь к тронному залу и убейте стража этажа.",
            self.floor,
            self.plan.theme.display_name()
        ))
        .color(Color::DarkPurple)
    }

    pub(super) fn map_canvas(&self) -> [u8; map::SIZE * map::SIZE] {
        let mut statuses = [ZoneStatus::Cleared; 16];
        for (cell, _) in self.plan.districts() {
            let index = cell.1 * 4 + cell.0;
            statuses[index] = if cell == self.plan.arrival {
                ZoneStatus::Arrival
            } else if cell == self.plan.throne {
                if self.boss == FloorBoss::Cleared {
                    ZoneStatus::ThroneCleared
                } else {
                    ZoneStatus::Throne
                }
            } else {
                let alive = self
                    .remaining_mobs
                    .iter()
                    .any(|id| self.mob_district.get(id) == Some(&index));
                let waiting = self.packs.iter().any(|pack| pack.district == index);
                if alive {
                    ZoneStatus::Combat
                } else if waiting {
                    ZoneStatus::Unexplored
                } else {
                    ZoneStatus::Cleared
                }
            };
        }
        map::castle::canvas(&self.plan, &statuses)
    }

    pub(super) fn map_decorations(
        &self,
        world: &Arc<World>,
        viewer: &Player,
        members: &[Uuid],
    ) -> Vec<MapDecoration> {
        let viewer_id = viewer.gameprofile.id();
        let mut decorations = Vec::new();
        for member in members {
            let Some(hunter) = world.players.get_by_uuid(member) else {
                continue;
            };
            let Some((lx, lz)) = self.slot.local(self.floor, hunter.position()) else {
                continue;
            };
            let (x, y) = map::castle::point(lx, lz);
            let kind = if *member == viewer_id {
                map::PLAYER
            } else {
                map::FRAME
            };
            decorations.push(map::decoration(
                kind,
                x,
                y,
                map::player_rotation(hunter.rotation().0),
                None,
            ));
        }
        if let FloorBoss::Alive(id) = self.boss
            && let Some(boss) = world.get_entity_by_id(id)
            && let Some((lx, lz)) = self.slot.local(self.floor, boss.position())
        {
            let (x, y) = map::castle::point(lx, lz);
            decorations.push(map::decoration(map::RED_MARKER, x, y, 0, Some("Страж")));
        }
        for (gate, kind, name) in [
            (self.home, map::BANNER_LIGHT_BLUE, "Домой"),
            (self.exit, map::BANNER_PURPLE, "Наверх"),
        ] {
            let Some(gate) = gate else {
                continue;
            };
            let position = DVec3::new(
                f64::from(gate.x()) + 0.5,
                f64::from(gate.y()),
                f64::from(gate.z()) + 0.5,
            );
            if let Some((lx, lz)) = self.slot.local(self.floor, position) {
                let (x, y) = map::castle::point(lx, lz);
                decorations.push(map::decoration(kind, x, y, 0, Some(name)));
            }
        }
        for treasure in &self.treasures {
            if self.opened.contains(&(*treasure, viewer_id)) {
                continue;
            }
            let position = DVec3::new(
                f64::from(treasure.x()) + 0.5,
                f64::from(treasure.y()),
                f64::from(treasure.z()) + 0.5,
            );
            if let Some((lx, lz)) = self.slot.local(self.floor, position) {
                let (x, y) = map::castle::point(lx, lz);
                decorations.push(map::decoration(map::RED_X, x, y, 0, Some("Тайник")));
            }
        }
        decorations
    }

    /// Removes every live mob and gate label id, so their entities can be discarded.
    pub(super) fn take_mobs(&mut self) -> Vec<i32> {
        let mut mobs = mem::take(&mut self.remaining_mobs);
        if let FloorBoss::Alive(id) = self.boss {
            mobs.push(id);
        }
        mobs.append(&mut self.gate_labels);
        if let Some(lower) = &mut self.lower {
            mobs.append(&mut lower.labels);
        }
        mobs
    }

    /// Returns whether `id` is one of the climb's ordinary mobs.
    pub(super) fn tracks(&self, id: i32) -> bool {
        self.remaining_mobs.contains(&id) && self.labels.contains_key(&id)
    }

    /// Advances the climb by one tick; returns a boss ability to apply.
    pub(super) fn tick(&mut self, context: &RunContext<'_>, tick: u64) -> Option<RunAction> {
        let world = context.world;
        let hunters: Vec<Arc<Player>> = context
            .members
            .iter()
            .filter_map(|member| world.players.get_by_uuid(member))
            .filter(|hunter| {
                !hunter.is_dead_or_dying() && !self.left.contains(&hunter.gameprofile.id())
            })
            .collect();
        let positions: Vec<DVec3> = hunters.iter().map(|hunter| hunter.position()).collect();
        self.tick_mobs(world, &positions, context.members, tick);
        self.wake_packs(world, &positions, tick);
        self.release_lower_floor(world, &positions);
        let action = self.tick_boss(context, &hunters, tick);
        self.walk_gates(context.world, &hunters, tick);
        action
    }

    fn tick_mobs(&mut self, world: &Arc<World>, hunters: &[DVec3], members: &[Uuid], tick: u64) {
        self.remaining_mobs
            .retain(|id| world.get_entity_by_id(*id).is_some());
        let power = (5.0 + 5.0 * castle::wave(self.floor).damage_multiplier) as f32;
        let out = self
            .skills
            .tick(world, hunters, &self.remaining_mobs, power, tick);
        self.pending_skills.extend(out.actions);
        self.pending_elite_skills
            .extend(self.elite_skills.tick(world, members, power, tick));
        for (id, label) in out.hatched {
            self.remaining_mobs.push(id);
            if let Some(entity) = world.get_entity_by_id(id)
                && let Some((lx, lz)) = self.slot.local(self.floor, entity.position())
                && let Some((cell, _, _)) = FloorPlan::district_at(lx, lz)
            {
                self.mob_district.insert(id, cell.1 * 4 + cell.0);
            }
            self.labels.insert(id, label);
        }
        if tick.is_multiple_of(10) {
            self.labels
                .retain(|id, _| world.get_entity_by_id(*id).is_some());
            refresh_plates(world, hunters, self.labels.keys());
        }
    }

    /// Spawns the packs a hunter came near, up to the live cap.
    fn wake_packs(&mut self, world: &Arc<World>, hunters: &[DVec3], tick: u64) {
        let near: Vec<(i32, i32)> = hunters
            .iter()
            .filter_map(|hunter| self.slot.local(self.floor, *hunter))
            .collect();
        if near.is_empty() {
            return;
        }
        let wave = castle::wave(self.floor);
        let rank = castle::rank_for_floor(self.floor);
        let mut index = 0;
        while index < self.packs.len() {
            if self.remaining_mobs.len() >= LIVE_CAP {
                return;
            }
            if !near
                .iter()
                .any(|(lx, lz)| self.packs[index].wakes(*lx, *lz))
            {
                index += 1;
                continue;
            }
            let pack = self.packs.swap_remove(index);
            for (mob, elite) in
                encounter::spawn_pack(world, self.slot, &self.plan, &pack, wave, rank)
            {
                if elite {
                    self.elites.push(mob.id);
                    self.elite_skills.track(mob.id, mob.class, tick);
                }
                self.remaining_mobs.push(mob.id);
                self.mob_district.insert(mob.id, pack.district);
                self.skills.track(mob.id, mob.class, tick);
                self.labels.insert(mob.id, mob.label);
            }
        }
    }

    /// Lets go of the floor below once no hunter is left on it.
    fn release_lower_floor(&mut self, world: &Arc<World>, hunters: &[DVec3]) {
        let Some(lower) = &self.lower else {
            return;
        };
        if hunters
            .iter()
            .all(|hunter| self.slot.local(lower.floor, *hunter).is_none())
        {
            let Some(lower) = self.lower.take() else {
                return;
            };
            for id in lower.labels {
                if let Some(entity) = world.get_entity_by_id(id) {
                    entity.set_removed(RemovalReason::Discarded);
                }
            }
        }
    }

    fn tick_boss(
        &mut self,
        context: &RunContext<'_>,
        hunters: &[Arc<Player>],
        tick: u64,
    ) -> Option<RunAction> {
        let world = context.world;
        match self.boss {
            FloorBoss::Waiting => {
                let inside = hunters
                    .iter()
                    .filter(|hunter| self.in_throne_room(hunter.position()))
                    .count();
                if inside == 0 {
                    return None;
                }
                if inside < hunters.len() {
                    if tick.is_multiple_of(HINT_INTERVAL_TICKS) {
                        for hunter in hunters {
                            hunter.send_action_bar(
                                TextComponent::plain(format!(
                                    "Тронный зал: {inside}/{} охотников. Страж ждёт весь отряд.",
                                    hunters.len()
                                ))
                                .color(Color::Yellow),
                            );
                        }
                    }
                    return None;
                }
                self.summon_boss(context, tick);
                None
            }
            FloorBoss::Alive(_) => {
                self.tick_giga(context, hunters, tick);
                self.fight
                    .as_mut()
                    .and_then(|fight| fight.tick(world, context.player, context.members, tick))
                    .map(RunAction::BossAbility)
            }
            FloorBoss::Defeated => {
                self.clear_floor(context, hunters);
                None
            }
            FloorBoss::Cleared => None,
        }
    }

    /// Returns whether `position` stands inside the throne room of the
    /// current floor, past its gate.
    fn in_throne_room(&self, position: DVec3) -> bool {
        self.slot
            .local(self.floor, position)
            .and_then(|(lx, lz)| FloorPlan::district_at(lx, lz))
            .is_some_and(|(cell, u, v)| cell == self.plan.throne && !districts::is_wall(u, v))
    }

    /// Bars the throne room and raises the floor boss.
    fn summon_boss(&mut self, context: &RunContext<'_>, tick: u64) {
        let world = context.world;
        self.set_seal(world, true);
        let giga = self.plan.district(self.plan.throne).archetype == Archetype::GigaThrone;
        let (boss, label) = if giga {
            self.giga = Some(GigaFight {
                phase: Phase::Awakened,
                next_eruption: 0,
                arena: encounter::arena_spots(&self.plan),
            });
            let guardian = self.plan.theme.guardian();
            (
                giga::guardian_boss(self.plan.theme, self.floor),
                Self::boss_label(
                    guardian.name.to_owned(),
                    format!("Великий страж, этаж {}", self.floor),
                ),
            )
        } else {
            let rank = castle::rank_for_floor(self.floor);
            let kind = BossKind::from_seed(self.world_seed ^ u64::from(self.floor).rotate_left(40));
            let mut boss = boss_for_gate(kind, rank, false);
            let multiplier = castle::floor_multiplier(self.floor);
            boss.health_multiplier *= multiplier;
            boss.damage_multiplier *= multiplier;
            boss.power *= multiplier;
            (
                boss,
                Self::boss_label(
                    format!("Страж {}-го этажа", self.floor),
                    kind.display_name().to_owned(),
                ),
            )
        };
        let (lx, lz) = self.plan.boss_spawn();
        let position = self.slot.block(self.floor, lx, lz, GROUND_Y + 1);
        let Some(spawned) = spawn_boss_body(world, position, boss, label) else {
            log::error!(
                "sololeveling: the boss of castle floor {} (slot {}) could not be spawned",
                self.floor,
                self.slot.0
            );
            self.set_seal(world, false);
            return;
        };
        self.boss = FloorBoss::Alive(spawned.id);
        self.labels.insert(spawned.id, spawned.label.clone());
        let mut fight = BossFight::new(&spawned.boss, spawned.id, tick);
        if let Some(entity) = world.get_entity_by_id(spawned.id) {
            fight.announce(&entity, context.player, &spawned.name);
        }
        self.fight = Some(fight);
        // The next floor generates while the fight lasts.
        if self.floor < MAX_FLOOR {
            self.next_ticket = Some(
                world
                    .chunk_map
                    .request_chunks(Self::entry_request(self.slot, self.floor + 1)),
            );
        }
        for member in context.members {
            if let Some(hunter) = world.players.get_by_uuid(member) {
                hunter.send_message(
                    &TextComponent::plain(
                        "Врата тронного зала захлопнулись. Победите стража этажа.",
                    )
                    .color(Color::DarkRed),
                );
            }
        }
    }

    const fn boss_label(name: String, title: String) -> MobLabel {
        MobLabel {
            name,
            title,
            color: Color::Red,
            is_tank: false,
            is_broodmother: false,
            rarity: MobRarity::Boss,
        }
    }

    /// Escalates a giga fight with the guardian's health: adds and rage on
    /// each new phase, and eruptions under the hunters once it is furious.
    fn tick_giga(&mut self, context: &RunContext<'_>, hunters: &[Arc<Player>], tick: u64) {
        let world = context.world;
        let in_arena: Vec<DVec3> = hunters
            .iter()
            .map(|hunter| hunter.position())
            .filter(|position| self.in_throne_room(*position))
            .collect();
        let (FloorBoss::Alive(id), Some(giga)) = (self.boss, &mut self.giga) else {
            return;
        };
        let Some(boss) = world.get_entity_by_id(id) else {
            return;
        };
        let Some(living) = boss.as_living_entity() else {
            return;
        };
        let health = living.get_health() / living.get_max_health().max(1.0);
        let phase = Phase::at(health);
        let mut adds = 0;
        if phase > giga.phase {
            giga.phase = phase;
            giga.next_eruption = tick + 40;
            adds = phase.adds();
            for effect in phase.rage() {
                living.add_mob_effect(MobEffectInstance::with_duration(effect, RAGE_TICKS, 0));
            }
            for hunter in hunters {
                hunter.send_title(TextComponent::plain("").color(Color::DarkRed));
                hunter.send_subtitle(
                    TextComponent::plain(phase.announcement()).color(Color::DarkRed),
                );
                hunter.send_title_times(5, 50, 15);
                hunter.send_message(
                    &TextComponent::plain(phase.announcement()).color(Color::DarkRed),
                );
            }
        }
        let mut eruptions = Vec::new();
        if let Some(interval) = phase.eruption_interval()
            && tick >= giga.next_eruption
        {
            giga.next_eruption = tick + interval;
            eruptions.extend(in_arena);
            for _ in 0..phase.stray_eruptions() {
                if giga.arena.is_empty() {
                    break;
                }
                let (lx, lz, layer) = giga.arena[rand::random_range(0..giga.arena.len())];
                let pos = self.slot.block(self.floor, lx, lz, GROUND_Y + layer);
                eruptions.push(DVec3::new(
                    f64::from(pos.x()) + 0.5,
                    f64::from(pos.y()),
                    f64::from(pos.z()) + 0.5,
                ));
            }
        }
        let spots = if adds > 0 {
            giga.arena.clone()
        } else {
            Vec::new()
        };
        if !eruptions.is_empty()
            && let Some(fight) = &mut self.fight
        {
            fight.mark_impacts(&eruptions, tick);
        }
        if adds == 0 {
            return;
        }
        let near = self
            .slot
            .local(self.floor, boss.position())
            .unwrap_or(self.plan.boss_spawn());
        let wave = castle::wave(self.floor);
        let rank = castle::rank_for_floor(self.floor);
        for mob in
            encounter::spawn_adds(world, self.slot, &self.plan, &spots, near, adds, wave, rank)
        {
            self.remaining_mobs.push(mob.id);
            self.mob_district
                .insert(mob.id, self.plan.throne.1 * 4 + self.plan.throne.0);
            self.skills.track(mob.id, mob.class, tick);
            self.labels.insert(mob.id, mob.label);
        }
    }

    /// Bars or frees the throne room's gate.
    fn set_seal(&mut self, world: &Arc<World>, sealed: bool) {
        // The bars join along the gate, or a hunter slips between the posts.
        let along = match self.plan.throne_door() {
            Side::North | Side::South => [Side::East, Side::West],
            Side::East | Side::West => [Side::North, Side::South],
        };
        let block: BlockStateId = if sealed {
            bars(&along)
        } else {
            vanilla_blocks::AIR.default_state()
        };
        let mut failed = 0;
        for (lx, lz, layer) in self.plan.throne_seal() {
            let pos = self.slot.block(self.floor, lx, lz, GROUND_Y + layer);
            set_fixture(world, pos, block, &mut failed);
        }
        if failed > 0 {
            log::warn!(
                "sololeveling: {failed} throne gate block(s) of castle floor {} stayed unwritten",
                self.floor
            );
        }
        self.sealed = sealed;
    }

    /// Hands out keys and floor rewards and opens the gates up and home.
    fn clear_floor(&mut self, context: &RunContext<'_>, hunters: &[Arc<Player>]) {
        let world = context.world;
        if let Some(fight) = self.fight.take() {
            fight.finish(context.player);
        }
        if self.sealed {
            self.set_seal(world, false);
        }
        let survivors: Vec<(Uuid, String)> = hunters
            .iter()
            .filter(|hunter| !hunter.connection.closed())
            .map(|hunter| {
                (
                    hunter.gameprofile.id(),
                    hunter.gameprofile.name().to_owned(),
                )
            })
            .collect();
        let top = self.floor >= MAX_FLOOR;
        for (survivor, _) in &survivors {
            if !top {
                self.floor_loot
                    .push((*survivor, key::floor_stack(self.floor, *survivor)));
            }
            for stack in castle::floor_loot(self.floor, || rand::random_range(0..100)) {
                self.floor_loot.push((*survivor, stack));
            }
            if self.giga.is_some() {
                for stack in castle::giga_loot(self.floor, || rand::random_range(0..100)) {
                    self.floor_loot.push((*survivor, stack));
                }
            }
        }
        self.giga = None;
        context.state.record_castle_clear(&survivors, self.floor);
        let gate = |(lx, lz): (i32, i32)| self.slot.block(self.floor, lx, lz, GROUND_Y + 1);
        let home = gate(self.plan.home_gate());
        if place_gate_frame(world, home, FrameStyle::Normal) {
            self.gate_labels.extend(spawn_gate_label(
                world,
                home,
                TextComponent::plain("Врата домой").color(Color::Aqua),
            ));
        } else {
            log::warn!(
                "sololeveling: home gate of castle floor {} is blocked at {home:?}",
                self.floor
            );
        }
        self.home = Some(home);
        if !top {
            let exit = gate(self.plan.exit_gate());
            if place_gate_frame(world, exit, FrameStyle::Castle) {
                self.gate_labels.extend(spawn_gate_label(
                    world,
                    exit,
                    TextComponent::plain(format!(
                        "Врата наверх — нужен ключ {}-го этажа",
                        self.floor
                    ))
                    .color(Color::Gold),
                ));
            } else {
                log::warn!(
                    "sololeveling: exit gate of castle floor {} is blocked at {exit:?}",
                    self.floor
                );
            }
            self.exit = Some(exit);
        }
        let line = if top {
            "Страж повержен. Выше этого этажа Система не видит ничего; кварцевые врата ведут домой."
                .to_owned()
        } else {
            format!(
                "Страж повержен. Ключ {}-го этажа открывает врата наверх; кварцевые врата ведут домой.",
                self.floor
            )
        };
        for member in context.members {
            if let Some(hunter) = world.players.get_by_uuid(member) {
                hunter.send_title(TextComponent::plain("Страж повержен").color(Color::Gold));
                hunter.send_subtitle(
                    TextComponent::plain("У трона открылись врата: домой и наверх")
                        .color(Color::Aqua),
                );
                hunter.send_title_times(10, 70, 20);
                hunter.send_message(&TextComponent::plain(line.clone()).color(Color::Gold));
            }
        }
        self.boss = FloorBoss::Cleared;
    }

    /// Moves hunters through the gates they step into.
    fn walk_gates(&mut self, world: &Arc<World>, hunters: &[Arc<Player>], tick: u64) {
        for hunter in hunters {
            let uuid = hunter.gameprofile.id();
            let position = hunter.position();
            let homes = [self.home, self.lower.as_ref().map(|lower| lower.home)];
            if homes
                .into_iter()
                .flatten()
                .any(|home| opening_contains(home, position))
            {
                self.left.insert(uuid);
                self.leaving.push(uuid);
                continue;
            }
            let gate_floor = if self
                .exit
                .is_some_and(|exit| opening_contains(exit, position))
            {
                self.floor
            } else if let Some(lower) = &self.lower
                && opening_contains(lower.exit, position)
            {
                lower.floor
            } else {
                continue;
            };
            if !key::take_floor_key(hunter, gate_floor) {
                if tick.is_multiple_of(HINT_INTERVAL_TICKS) {
                    hunter.send_action_bar(
                        TextComponent::plain(format!(
                            "Врата не откроются без ключа {gate_floor}-го этажа."
                        ))
                        .color(Color::Red),
                    );
                }
                continue;
            }
            if gate_floor == self.floor {
                self.advance(world);
            }
            let _ = hunter.teleport(self.entry_position(), 0.0, 0.0);
            hunter.send_message(&self.announcement());
        }
    }

    /// Moves the climb up one floor; the floor below stays loaded for the
    /// teammates still walking to its gate.
    fn advance(&mut self, world: &Arc<World>) {
        let (Some(exit), Some(home)) = (self.exit.take(), self.home.take()) else {
            return;
        };
        let gate_labels = mem::take(&mut self.gate_labels);
        for id in self.take_mobs() {
            if let Some(entity) = world.get_entity_by_id(id) {
                entity.set_removed(RemovalReason::Discarded);
            }
        }
        let next_ticket = self.next_ticket.take().unwrap_or_else(|| {
            world
                .chunk_map
                .request_chunks(Self::entry_request(self.slot, self.floor + 1))
        });
        let ticket = self.ticket.replace(next_ticket);
        self.lower = Some(LowerFloor {
            floor: self.floor,
            exit,
            home,
            labels: gate_labels,
            _ticket: ticket,
        });
        self.floor += 1;
        self.plan = Arc::new(FloorPlan::generate(self.world_seed, self.floor));
        self.packs = encounter::floor_packs(&self.plan, self.world_seed);
        self.mob_district.clear();
        self.labels.clear();
        self.elites.clear();
        self.giga = None;
        self.locate_treasures();
        self.skills = MobSkills::default();
        self.elite_skills = EliteSkills::default();
        self.pending_elite_skills.clear();
        self.boss = FloorBoss::Waiting;
        self.fight = None;
        self.sealed = false;
        self.advanced = true;
    }
}
