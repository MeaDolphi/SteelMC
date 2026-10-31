//! Adventurer Organization contracts on live state, and the board positions.
//!
//! Progress events are queued and applied on the server tick, so the death
//! and dungeon code that reports them never takes the party or session locks
//! while it holds its own.

use std::fs;
use std::io::{self, ErrorKind};
use std::mem;
use std::path::PathBuf;
use std::sync::Arc;

use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};
use steel_core::player::Player;
use steel_core::server::Server;
use steel_utils::{BlockPos, Identifier};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use super::SoloLevelingState;
use crate::daily;
use crate::guild::{Guild, GuildRegistry, GuildRole};
use crate::organization::{
    Contract, Event, GROUP_LIMIT, GroupContract, GuildContract, OBJECTIVES, OrganizationBook,
    OrganizationError, RANKS, Template, guild_rank,
};
use crate::party::PartyError;
use crate::rank::HunterRank;

/// Lecterns registered as Organization boards.
#[derive(Debug, Default)]
pub(super) struct Boards {
    path: PathBuf,
    boards: Vec<Board>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Board {
    world: String,
    pos: [i32; 3],
}

impl Board {
    fn new(world: &Identifier, pos: BlockPos) -> Self {
        Self {
            world: world.to_string(),
            pos: [pos.x(), pos.y(), pos.z()],
        }
    }

    fn is_at(&self, world: &Identifier, pos: BlockPos) -> bool {
        *self == Self::new(world, pos)
    }
}

impl Boards {
    /// Loads the board list kept next to the gameplay player files.
    pub(super) fn load(path: PathBuf) -> io::Result<Self> {
        let boards = match fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents)
                .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?,
            Err(error) if error.kind() == ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        Ok(Self { path, boards })
    }

    fn save(&self) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(&self.boards).map_err(io::Error::other)?;
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, json)?;
        fs::rename(temporary, &self.path)
    }
}

/// What the board shows one hunter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardView {
    /// The hunter's own rank.
    pub rank: HunterRank,
    /// Personal contracts and today's allowance.
    pub book: OrganizationBook,
    /// Today's Moscow day, for allowance checks.
    pub today: u64,
    /// Group contracts of the hunter's party, if any.
    pub group: Vec<GroupContract>,
    /// Whether the hunter leads a party and may manage group contracts.
    pub leader: bool,
    /// The hunter's guild contracts, if they are in a guild.
    pub guild: Option<GuildView>,
    /// Whether the free Association starter kit is still unclaimed.
    pub starter_claimable: bool,
}

/// What the board shows about the hunter's guild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuildView {
    /// Highest contract rank the guild may take.
    pub rank: HunterRank,
    /// Active guild contracts.
    pub contracts: Vec<GuildContract>,
    /// Whether the guild already took a template today.
    pub used: Vec<Template>,
    /// Whether the hunter may manage guild contracts.
    pub officer: bool,
}

/// A group contract that was paid out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupPayout {
    /// The finished contract.
    pub contract: Contract,
    /// Members who received one payment each.
    pub paid: Vec<Uuid>,
}

impl SoloLevelingState {
    /// Returns whether a board stands at `pos` in `world`.
    pub fn is_organization_board(&self, world: &Identifier, pos: BlockPos) -> bool {
        self.organization_boards
            .lock()
            .boards
            .iter()
            .any(|board| board.is_at(world, pos))
    }

    /// Registers a board; `false` when one is already there.
    pub fn add_organization_board(&self, world: &Identifier, pos: BlockPos) -> io::Result<bool> {
        let mut boards = self.organization_boards.lock();
        if boards.boards.iter().any(|board| board.is_at(world, pos)) {
            return Ok(false);
        }
        boards.boards.push(Board::new(world, pos));
        boards.save().map(|()| true)
    }

    /// Unregisters every board within `radius` blocks, returning their positions.
    pub fn remove_organization_boards_near(
        &self,
        world: &Identifier,
        pos: BlockPos,
        radius: i32,
    ) -> io::Result<Vec<BlockPos>> {
        let mut boards = self.organization_boards.lock();
        let world = world.to_string();
        let mut removed = Vec::new();
        boards.boards.retain(|board| {
            let [x, y, z] = board.pos;
            let near = board.world == world
                && (x - pos.x()).abs() <= radius
                && (y - pos.y()).abs() <= radius
                && (z - pos.z()).abs() <= radius;
            if near {
                removed.push(BlockPos::new(x, y, z));
            }
            !near
        });
        if !removed.is_empty() {
            boards.save()?;
        }
        Ok(removed)
    }

    /// Captures what the board shows `uuid`.
    pub fn organization_view(&self, uuid: Uuid) -> Option<BoardView> {
        let today = daily::today();
        let (rank, book, starter_claimable) = self.sessions.with_mut(uuid, |session| {
            session.organization.roll_over(today);
            (
                session.stats.rank(),
                session.organization.clone(),
                session.starter.claimable(),
            )
        })?;
        let party = self.party_of(uuid);
        Some(BoardView {
            rank,
            book,
            today,
            starter_claimable,
            leader: party.as_ref().is_some_and(|party| party.leader() == uuid),
            group: party.map_or_else(Vec::new, |party| party.contracts().to_vec()),
            guild: self.guilds.lock().guild_of(uuid).map(|guild| {
                let board = &guild.organization;
                GuildView {
                    rank: guild_rank(guild.level),
                    contracts: board.contracts.clone(),
                    used: RANKS
                        .into_iter()
                        .flat_map(|rank| {
                            OBJECTIVES
                                .into_iter()
                                .map(move |objective| Template { objective, rank })
                        })
                        .filter(|template| board.used_today(*template, today))
                        .collect(),
                    officer: guild.role_of(uuid).is_some_and(GuildRole::can_manage_bank),
                }
            }),
        })
    }

    /// Accepts a guild contract; only the guildmaster and officers may.
    pub fn accept_guild_contract(
        &self,
        actor: Uuid,
        template: Template,
    ) -> Result<(), OrganizationError> {
        self.ensure_guild_mutation_available()
            .map_err(|_| OrganizationError::Storage)?;
        let mut guilds = self.guilds.lock();
        let guild = officer_guild(&mut guilds, actor)?;
        let rank = guild_rank(guild.level);
        guild.organization.accept(template, rank, daily::today())?;
        drop(guilds);
        self.queue_guild_save(Some(actor))
            .map_err(|_| OrganizationError::Storage)
    }

    /// Cancels a guild contract, restoring today's allowance if applicable.
    pub fn cancel_guild_contract(
        &self,
        actor: Uuid,
        index: usize,
    ) -> Result<Contract, OrganizationError> {
        self.ensure_guild_mutation_available()
            .map_err(|_| OrganizationError::Storage)?;
        let mut guilds = self.guilds.lock();
        let contract = officer_guild(&mut guilds, actor)?
            .organization
            .cancel(index, daily::today())?;
        drop(guilds);
        self.queue_guild_save(Some(actor))
            .map_err(|_| OrganizationError::Storage)?;
        Ok(contract)
    }

    /// Settles a finished guild contract.
    ///
    /// Every contributor still in the guild is owed one full payment; it is
    /// saved with the guild first and then paid to whoever is online, so an
    /// offline contributor collects it on the next login.
    pub fn turn_in_guild_contract(
        &self,
        actor: Uuid,
        index: usize,
    ) -> Result<GroupPayout, OrganizationError> {
        self.ensure_guild_mutation_available()
            .map_err(|_| OrganizationError::Storage)?;
        let payout = {
            let mut guilds = self.guilds.lock();
            let guild = officer_guild(&mut guilds, actor)?;
            let members: FxHashSet<_> = guild.members.keys().copied().collect();
            let group = guild
                .organization
                .contracts
                .get_mut(index)
                .ok_or(OrganizationError::Missing)?;
            if !group.contract.is_complete() {
                return Err(OrganizationError::NotComplete);
            }
            if group.pending_turn_in {
                return Err(OrganizationError::Storage);
            }
            group.pending_turn_in = true;
            GroupPayout {
                contract: group.contract,
                paid: group
                    .contributors
                    .iter()
                    .copied()
                    .filter(|hunter| members.contains(hunter))
                    .collect(),
            }
        };
        if self.queue_guild_turn_in(actor, index).is_err() {
            self.clear_guild_turn_in_pending(actor, index);
            return Err(OrganizationError::Storage);
        }
        Ok(payout)
    }

    /// Queues an atomic collection of a hunter's stored guild payouts.
    pub fn collect_guild_payout(&self, hunter: Uuid) -> bool {
        if self.ensure_guild_mutation_available().is_err()
            || !self.persistence.lock().accepts_writes(hunter)
            || !self.guilds.lock().guild_of(hunter).is_some_and(|guild| {
                guild
                    .organization
                    .owed
                    .get(&hunter)
                    .is_some_and(|amount| *amount > 0)
            })
        {
            return false;
        }
        let Some(accepted) = self.sessions.with_mut(hunter, |session| {
            if session.economic_busy {
                return false;
            }
            session.economic_busy = true;
            true
        }) else {
            return false;
        };
        if !accepted {
            return false;
        }
        if let Err(error) = self.queue_guild_collection(hunter) {
            self.sessions
                .with_mut(hunter, |session| session.economic_busy = false);
            log::warn!("sololeveling: could not queue guild payout for {hunter}: {error}");
            return false;
        }
        true
    }

    /// Retries due online guild payouts that could not queue on join.
    pub fn tick_guild_payouts(&self) {
        if self.ensure_guild_mutation_available().is_err() {
            return;
        }
        for hunter in self.sessions.player_ids() {
            let _ = self.collect_guild_payout(hunter);
        }
    }

    /// Accepts a personal contract.
    pub fn accept_personal_contract(
        &self,
        uuid: Uuid,
        template: Template,
    ) -> Result<(), OrganizationError> {
        let today = daily::today();
        self.sessions
            .with_mut(uuid, |session| {
                if session.economic_busy {
                    return Err(OrganizationError::Storage);
                }
                let rank = session.stats.rank();
                session.organization.accept(template, rank, today)
            })
            .ok_or(OrganizationError::NoSession)??;
        self.queue_organization_save(uuid);
        Ok(())
    }

    /// Cancels a personal contract, restoring today's allowance if applicable.
    pub fn cancel_personal_contract(
        &self,
        uuid: Uuid,
        index: usize,
    ) -> Result<Contract, OrganizationError> {
        let contract = self
            .sessions
            .with_mut(uuid, |session| {
                if session.economic_busy {
                    return Err(OrganizationError::Storage);
                }
                session.organization.cancel(index, daily::today())
            })
            .ok_or(OrganizationError::NoSession)??;
        self.queue_organization_save(uuid);
        Ok(contract)
    }

    /// Accepts a group contract on behalf of the leader's whole party.
    ///
    /// The members are fixed now, the rank is capped by the weakest of them,
    /// and every member spends today's allowance of the template.
    pub fn accept_group_contract(
        &self,
        leader: Uuid,
        template: Template,
    ) -> Result<(), OrganizationError> {
        let today = daily::today();
        let mut parties = self.party_runtime.lock();
        let party = parties.led_party_mut(leader).map_err(party_error)?;
        if party.contracts.len() >= GROUP_LIMIT {
            return Err(OrganizationError::TooManyContracts);
        }
        let members = party.members().to_vec();
        let mut weakest = HunterRank::S;
        for member in &members {
            let (rank, used) = self
                .sessions
                .with_mut(*member, |session| {
                    (
                        session.stats.rank(),
                        session.organization.used_today(template, today),
                    )
                })
                .ok_or(OrganizationError::NoSession)?;
            if used {
                return Err(if *member == leader {
                    OrganizationError::UsedToday
                } else {
                    OrganizationError::MemberUsedToday
                });
            }
            weakest = weakest.min(rank);
        }
        if template.rank > weakest {
            return Err(OrganizationError::RankTooHigh);
        }
        for member in &members {
            self.sessions.with_mut(*member, |session| {
                session.organization.mark_used(template, today);
            });
        }
        party.contracts.push(GroupContract {
            contract: Contract::accepted(template, 1, today),
            participants: members.clone(),
            pending_turn_in: false,
        });
        drop(parties);
        for member in members {
            self.queue_organization_save(member);
        }
        if let Err(error) = self.queue_party_save() {
            log::warn!(
                "sololeveling: could not queue party save after group contract accept: {error}"
            );
        }
        Ok(())
    }

    /// Cancels a group contract at the leader's request.
    pub fn cancel_group_contract(
        &self,
        leader: Uuid,
        index: usize,
    ) -> Result<Contract, OrganizationError> {
        let mut parties = self.party_runtime.lock();
        let party = parties.led_party_mut(leader).map_err(party_error)?;
        if index >= party.contracts.len() {
            return Err(OrganizationError::Missing);
        }
        if party.contracts[index].pending_turn_in {
            return Err(OrganizationError::Storage);
        }
        let group = party.contracts.remove(index);
        drop(parties);
        let today = daily::today();
        for participant in group.participants {
            self.sessions.with_mut(participant, |session| {
                session
                    .organization
                    .release_cancelled(group.contract, today);
            });
            self.queue_organization_save(participant);
        }
        if let Err(error) = self.queue_party_save() {
            log::warn!(
                "sololeveling: could not queue party save after group contract cancel: {error}"
            );
        }
        Ok(group.contract)
    }

    /// Queues an event that personal and group contracts may count.
    pub fn record_organization_event(&self, hunters: Vec<Uuid>, event: Event) {
        if hunters.is_empty() {
            return;
        }
        self.organization_events.lock().push((hunters, event));
    }

    /// Applies queued contract events and tells hunters what they finished.
    pub fn tick_organization(&self, server: &Server) {
        if self.ensure_guild_mutation_available().is_err() {
            return;
        }
        let finished = self.apply_organization_events();
        if finished.is_empty() {
            return;
        }
        for player in server.get_players() {
            let uuid = player.gameprofile.id();
            for (_, contract) in finished.iter().filter(|(hunter, _)| *hunter == uuid) {
                announce_finished(&player, *contract);
            }
        }
    }

    /// Counts queued events, returning who finished which contract.
    fn apply_organization_events(&self) -> Vec<(Uuid, Contract)> {
        let events = mem::take(&mut *self.organization_events.lock());
        let mut finished: Vec<(Uuid, Contract)> = Vec::new();
        let mut guild_notices = FxHashSet::default();
        for (hunters, event) in &events {
            for hunter in hunters {
                let observed = self.sessions.with_mut(*hunter, |session| {
                    let before = session.organization.contracts.clone();
                    let done = session.organization.observe(*event);
                    (before != session.organization.contracts, done)
                });
                let Some((changed, done)) = observed else {
                    continue;
                };
                if changed {
                    self.queue_organization_save(*hunter);
                }
                finished.extend(done.into_iter().map(|contract| (*hunter, contract)));
            }
            // A shared goal counts once per event, however many participants
            // took part in it.
            let mut parties = self.party_runtime.lock();
            for party in parties.parties_mut() {
                for group in &mut party.contracts {
                    if group.pending_turn_in {
                        continue;
                    }
                    if group
                        .participants
                        .iter()
                        .any(|participant| hunters.contains(participant))
                        && group.contract.observe(*event)
                    {
                        finished.extend(
                            group
                                .participants
                                .iter()
                                .map(|participant| (*participant, group.contract)),
                        );
                    }
                }
            }
            drop(parties);

            let mut guilds = self.guilds.lock();
            let mut changed = false;
            for guild in guilds.guilds_mut() {
                let members: Vec<Uuid> = hunters
                    .iter()
                    .copied()
                    .filter(|hunter| guild.members.contains_key(hunter))
                    .collect();
                if members.is_empty() {
                    continue;
                }
                let (counted, done) = guild.organization.observe(&members, *event);
                changed |= counted;
                if counted {
                    guild_notices.extend(members);
                }
                for contract in done {
                    finished.extend(guild.members.keys().map(|member| (*member, contract)));
                }
            }
            drop(guilds);
            if changed
                && guild_notices.is_empty()
                && let Some(player) = hunters.first()
            {
                guild_notices.insert(*player);
            }
        }
        for player in guild_notices {
            if let Err(error) = self.queue_guild_save(Some(player)) {
                log::warn!("sololeveling: guild contract progress was not saved: {error}");
                break;
            }
        }
        finished
    }

    fn queue_organization_save(&self, uuid: Uuid) {
        if let Err(error) = self.save_session(uuid) {
            log::warn!("sololeveling: failed to persist contracts of {uuid}: {error}");
        }
    }
}

fn announce_finished(player: &Arc<Player>, contract: Contract) {
    player.send_message(
        &TextComponent::plain(format!(
            "Задание выполнено: {}. Сдайте его на доске Организации.",
            contract.template.title()
        ))
        .color(Color::Gold),
    );
}

fn officer_guild(guilds: &mut GuildRegistry, actor: Uuid) -> Result<&mut Guild, OrganizationError> {
    let guild = guilds
        .guild_of_mut(actor)
        .ok_or(OrganizationError::NotInGuild)?;
    if !guild.role_of(actor).is_some_and(GuildRole::can_manage_bank) {
        return Err(OrganizationError::NotOfficer);
    }
    Ok(guild)
}

pub(super) const fn party_error(error: PartyError) -> OrganizationError {
    match error {
        PartyError::NotLeader => OrganizationError::NotLeader,
        _ => OrganizationError::NotInParty,
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::path::PathBuf;

    use uuid::Uuid;

    use crate::guild::GuildRole;
    use crate::organization::{Event, Objective, OrganizationError, Template};
    use crate::player_data::SoloLevelingPlayerData;
    use crate::rank::HunterRank;
    use crate::state::SoloLevelingState;

    fn state() -> (SoloLevelingState, PathBuf) {
        let directory = env::temp_dir().join(format!("sololeveling-org-{}", Uuid::new_v4()));
        let state = SoloLevelingState::new(directory.clone()).expect("state should initialize");
        (state, directory)
    }

    fn hunter(state: &SoloLevelingState, level: u32) -> Uuid {
        let uuid = Uuid::new_v4();
        let mut data = SoloLevelingPlayerData::new(uuid);
        data.stats.level = level;
        state.begin_session(uuid, data);
        uuid
    }

    fn party(state: &SoloLevelingState, leader: Uuid, members: &[Uuid]) {
        for member in members {
            state.invite_to_party(leader, *member, 0).expect("invite");
            state.accept_party_invitation(*member, 0).expect("accept");
        }
    }

    const BOSS_E: Template = Template {
        objective: Objective::Boss,
        rank: HunterRank::E,
    };

    #[test]
    fn group_contract_is_capped_by_the_weakest_member_and_spends_everyones_allowance() {
        let (state, directory) = state();
        let leader = hunter(&state, 30);
        let weak = hunter(&state, 1);
        party(&state, leader, &[weak]);

        let ranked = Template {
            objective: Objective::Mobs,
            rank: HunterRank::C,
        };
        assert_eq!(
            state.accept_group_contract(leader, ranked),
            Err(OrganizationError::RankTooHigh)
        );
        assert_eq!(
            state.accept_group_contract(weak, BOSS_E),
            Err(OrganizationError::NotLeader)
        );
        state
            .accept_group_contract(leader, BOSS_E)
            .expect("leader takes the contract");
        assert_eq!(
            state.accept_personal_contract(weak, BOSS_E),
            Err(OrganizationError::UsedToday)
        );
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn cancelled_group_contract_releases_each_participants_allowance() {
        let (state, directory) = state();
        let leader = hunter(&state, 1);
        let member = hunter(&state, 1);
        party(&state, leader, &[member]);
        state.accept_group_contract(leader, BOSS_E).expect("accept");
        state.cancel_group_contract(leader, 0).expect("cancel");
        state
            .accept_personal_contract(member, BOSS_E)
            .expect("member retakes");
        state
            .accept_personal_contract(leader, BOSS_E)
            .expect("leader retakes");
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn a_shared_event_counts_once_and_leavers_forfeit_their_share() {
        let (state, directory) = state();
        let leader = hunter(&state, 1);
        let first = hunter(&state, 1);
        let second = hunter(&state, 1);
        party(&state, leader, &[first, second]);
        let dungeons = Template {
            objective: Objective::Dungeons,
            rank: HunterRank::E,
        };
        state
            .accept_group_contract(leader, dungeons)
            .expect("accept");

        state.record_organization_event(vec![leader, first, second], Event::Dungeon(HunterRank::E));
        state.apply_organization_events();
        let progress = state.party_of(leader).expect("party").contracts()[0]
            .contract
            .progress;
        assert_eq!(progress, 1);

        state.leave_party(second);
        state.record_organization_event(vec![second], Event::Dungeon(HunterRank::E));
        state.record_organization_event(vec![first], Event::Dungeon(HunterRank::E));
        state.record_organization_event(vec![leader], Event::Dungeon(HunterRank::E));
        let finished = state.apply_organization_events();
        assert_eq!(finished.len(), 2);

        let payout = state
            .turn_in_group_contract(leader, 0)
            .expect("finished contract is queued");
        assert_eq!(payout.paid, vec![leader, first]);
        assert_eq!(
            state.with_session_mut(leader, |session| session.won.amount()),
            Some(0)
        );
        assert_eq!(state.party_of(leader).expect("party").contracts().len(), 1);
        assert_eq!(
            state.turn_in_group_contract(leader, 0),
            Err(OrganizationError::Storage)
        );
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn personal_contract_is_queued_without_early_payment_or_removal() {
        let (state, directory) = state();
        let uuid = hunter(&state, 1);
        state
            .accept_personal_contract(uuid, BOSS_E)
            .expect("accept");
        assert_eq!(
            state.turn_in_personal_contract(uuid, 0),
            Err(OrganizationError::NotComplete)
        );
        state.record_organization_event(vec![uuid], Event::Boss(HunterRank::E));
        assert_eq!(state.apply_organization_events().len(), 1);
        let queued = state
            .turn_in_personal_contract(uuid, 0)
            .expect("queue turn-in");
        assert_eq!(queued.template, BOSS_E);
        assert_eq!(
            state.with_session_mut(uuid, |session| (
                session.won.amount(),
                session.organization.contracts.len()
            )),
            Some((0, 1))
        );
        assert_eq!(
            state.turn_in_personal_contract(uuid, 0),
            Err(OrganizationError::Storage)
        );
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn guild_contract_settlement_stages_offline_and_online_payouts_without_early_mutation() {
        let (state, directory) = state();
        let master = hunter(&state, 1);
        let officer = hunter(&state, 1);
        let recruit = hunter(&state, 1);
        state
            .create_guild("Hunters".to_owned(), master)
            .expect("create guild");
        for member in [officer, recruit] {
            state.add_guild_recruit(master, member).expect("recruit");
        }
        state
            .set_guild_role(master, officer, GuildRole::Officer)
            .expect("promote");

        assert_eq!(
            state.accept_guild_contract(recruit, BOSS_E),
            Err(OrganizationError::NotOfficer)
        );
        state
            .accept_guild_contract(officer, BOSS_E)
            .expect("officer takes the contract");
        // A guild contract does not spend anyone's personal allowance.
        state
            .accept_personal_contract(recruit, BOSS_E)
            .expect("personal allowance is separate");

        for hunters in [
            vec![master, recruit],
            vec![recruit],
            vec![recruit],
            vec![recruit],
        ] {
            state.record_organization_event(hunters, Event::Boss(HunterRank::E));
        }
        state.apply_organization_events();
        assert_eq!(
            state.turn_in_guild_contract(officer, 0),
            Err(OrganizationError::NotComplete)
        );
        state.record_organization_event(vec![recruit], Event::Boss(HunterRank::E));
        state.apply_organization_events();

        // The recruit goes offline before the officer turns the contract in.
        let _ = state.sessions.remove(recruit);
        let payout = state
            .turn_in_guild_contract(officer, 0)
            .expect("finished contract");
        assert_eq!(payout.contract.payment(), 750);
        assert_eq!(payout.paid.len(), 2);
        assert!(
            !payout.paid.contains(&officer),
            "the officer did not contribute"
        );

        assert!(
            state
                .guilds
                .lock()
                .guild_of(officer)
                .expect("guild")
                .organization
                .contracts[0]
                .pending_turn_in
        );
        assert_eq!(
            state.with_session_mut(master, |session| session.won.amount()),
            Some(0),
            "online balances wait for the durable transaction"
        );
        let mut staged = state.guilds.lock().clone();
        let (_, contributors) = staged
            .settle_contract(officer, 0)
            .expect("stage guild settlement");
        assert_eq!(contributors, vec![master, recruit]);
        for contributor in contributors {
            assert_eq!(
                staged
                    .guild_of(contributor)
                    .and_then(|guild| guild.organization.owed.get(&contributor))
                    .copied(),
                Some(750)
            );
        }
        assert_eq!(
            state
                .guilds
                .lock()
                .guild_of(officer)
                .expect("unchanged live guild")
                .organization
                .contracts
                .len(),
            1,
            "candidate settlement does not mutate the live registry"
        );
        fs::remove_dir_all(directory).expect("cleanup");
    }
}
