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

use serde::{Deserialize, Serialize};
use steel_core::player::Player;
use steel_core::server::Server;
use steel_utils::{BlockPos, Identifier};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use super::SoloLevelingState;
use crate::daily;
use crate::organization::{
    Contract, Event, GROUP_LIMIT, GroupContract, OrganizationBook, OrganizationError, Template,
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
#[derive(Debug, Clone)]
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
        let (rank, book) = self.sessions.with_mut(uuid, |session| {
            session.organization.roll_over(today);
            (session.stats.rank(), session.organization.clone())
        })?;
        let party = self.party_of(uuid);
        Some(BoardView {
            rank,
            book,
            today,
            leader: party.as_ref().is_some_and(|party| party.leader() == uuid),
            group: party.map_or_else(Vec::new, |party| party.contracts().to_vec()),
        })
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
                let rank = session.stats.rank();
                session.organization.accept(template, rank, today)
            })
            .ok_or(OrganizationError::NoSession)??;
        self.queue_organization_save(uuid);
        Ok(())
    }

    /// Cancels a personal contract without restoring today's allowance.
    pub fn cancel_personal_contract(
        &self,
        uuid: Uuid,
        index: usize,
    ) -> Result<Contract, OrganizationError> {
        let contract = self
            .sessions
            .with_mut(uuid, |session| session.organization.cancel(index))
            .ok_or(OrganizationError::NoSession)??;
        self.queue_organization_save(uuid);
        Ok(contract)
    }

    /// Pays a finished personal contract exactly once.
    ///
    /// The contract leaves the book before the Won credit is queued, and the
    /// credit commits the book together with the balance.
    pub fn turn_in_personal_contract(
        &self,
        uuid: Uuid,
        index: usize,
    ) -> Result<Contract, OrganizationError> {
        if !self.persistence.lock().accepts_writes(uuid) {
            return Err(OrganizationError::Storage);
        }
        let contract = self
            .sessions
            .with_mut(uuid, |session| session.organization.turn_in(index))
            .ok_or(OrganizationError::NoSession)??;
        self.credit_won(uuid, contract.template.payment());
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
            contract: Contract::new(template),
            participants: members.clone(),
        });
        drop(parties);
        for member in members {
            self.queue_organization_save(member);
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
        Ok(party.contracts.remove(index).contract)
    }

    /// Pays every remaining participant of a finished group contract once.
    ///
    /// Shares of members who left are not redistributed.
    pub fn turn_in_group_contract(
        &self,
        leader: Uuid,
        index: usize,
    ) -> Result<GroupPayout, OrganizationError> {
        let mut parties = self.party_runtime.lock();
        let party = parties.led_party_mut(leader).map_err(party_error)?;
        let group = party
            .contracts
            .get(index)
            .ok_or(OrganizationError::Missing)?;
        if !group.contract.is_complete() {
            return Err(OrganizationError::NotComplete);
        }
        {
            let persistence = self.persistence.lock();
            if group
                .participants
                .iter()
                .any(|participant| !persistence.accepts_writes(*participant))
            {
                return Err(OrganizationError::Storage);
            }
        }
        let group = party.contracts.remove(index);
        drop(parties);
        let payment = group.contract.template.payment();
        for participant in &group.participants {
            self.credit_won(*participant, payment);
        }
        Ok(GroupPayout {
            contract: group.contract,
            paid: group.participants,
        })
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

const fn party_error(error: PartyError) -> OrganizationError {
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
            .expect("finished contract is paid");
        assert_eq!(payout.paid, vec![leader, first]);
        assert_eq!(
            state.turn_in_group_contract(leader, 0),
            Err(OrganizationError::Missing)
        );
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn personal_contract_is_paid_once_after_completion() {
        let (state, directory) = state();
        let uuid = hunter(&state, 1);
        state
            .accept_personal_contract(uuid, BOSS_E)
            .expect("accept");
        assert_eq!(
            state.turn_in_personal_contract(uuid, 0),
            Err(OrganizationError::NotComplete)
        );
        state.record_organization_event(vec![uuid], Event::Boss);
        assert_eq!(state.apply_organization_events().len(), 1);
        let paid = state.turn_in_personal_contract(uuid, 0).expect("paid");
        assert_eq!(paid.template, BOSS_E);
        assert_eq!(
            state.turn_in_personal_contract(uuid, 0),
            Err(OrganizationError::Missing)
        );
        fs::remove_dir_all(directory).expect("cleanup");
    }
}
