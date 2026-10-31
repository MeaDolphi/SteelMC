//! Ephemeral party membership, invitations, and raid contribution accounting.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::organization::GroupContract;

/// Maximum number of hunters in one party.
pub const PARTY_MAX_MEMBERS: usize = 4;
/// Minecraft ticks for which a party invitation remains valid.
pub const PARTY_INVITATION_TICKS: u64 = 20 * 60;

/// An active group of hunters, ordered by join time.
///
/// The first member is the leader. This makes leadership transfer deterministic
/// when the current leader disconnects or leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Party {
    members: Vec<Uuid>,
    /// Organization contracts the party took; they share its lifetime.
    pub(crate) contracts: Vec<GroupContract>,
}

impl Party {
    /// Creates a party led by `leader`.
    #[must_use]
    pub fn new(leader: Uuid) -> Self {
        Self {
            members: vec![leader],
            contracts: Vec::new(),
        }
    }

    /// Returns the current party leader.
    #[must_use]
    pub fn leader(&self) -> Uuid {
        self.members[0]
    }

    /// Returns members in leader and join order.
    #[must_use]
    pub fn members(&self) -> &[Uuid] {
        &self.members
    }

    fn add(&mut self, player: Uuid) -> Result<(), PartyError> {
        if self.members.len() == PARTY_MAX_MEMBERS {
            return Err(PartyError::Full);
        }
        self.members.push(player);
        Ok(())
    }

    fn remove(&mut self, player: Uuid) -> bool {
        let Some(index) = self.members.iter().position(|member| *member == player) else {
            return false;
        };
        self.members.remove(index);
        // A member who leaves forfeits their share; a contract nobody is
        // entitled to any more is cancelled.
        for contract in &mut self.contracts {
            contract
                .participants
                .retain(|participant| *participant != player);
        }
        self.contracts
            .retain(|contract| !contract.participants.is_empty());
        true
    }

    /// Returns the Organization contracts the party holds.
    #[must_use]
    pub fn contracts(&self) -> &[GroupContract] {
        &self.contracts
    }
}

/// What a party looks like after one of its members left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartyExit {
    /// The last member left, so the party no longer exists.
    Disbanded,
    /// The party continues under a new leader.
    Continued {
        /// Member who took the party over.
        new_leader: Uuid,
    },
}

/// A pending invitation to join a party.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Invitation {
    party_leader: Uuid,
    expires_at: u64,
}

/// In-memory party registry.
#[derive(Debug, Default)]
pub struct PartyRuntime {
    parties: FxHashMap<Uuid, Party>,
    membership: FxHashMap<Uuid, Uuid>,
    invitations: FxHashMap<Uuid, Invitation>,
}

/// Failed party operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartyError {
    /// The initiating player has no party.
    NotInParty,
    /// The operation requires the party leader.
    NotLeader,
    /// A party cannot have more than four members.
    Full,
    /// The target is already in this party.
    AlreadyMember,
    /// There is no valid invitation for this player.
    NoInvitation,
}

impl PartyError {
    /// Returns the Russian message shown to the hunter.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::NotInParty => "Игрок не состоит в этой группе.",
            Self::NotLeader => "Это может только лидер группы.",
            Self::Full => "В группе уже четыре участника.",
            Self::AlreadyMember => "Игрок уже состоит в этой группе.",
            Self::NoInvitation => "Активного приглашения в группу нет.",
        }
    }
}

impl PartyRuntime {
    /// Returns the leader whose party invited `target`, while it is valid.
    #[must_use]
    pub fn invitation_for(&self, target: Uuid, tick: u64) -> Option<Uuid> {
        self.invitations
            .get(&target)
            .filter(|invitation| invitation.expires_at >= tick)
            .map(|invitation| invitation.party_leader)
    }

    /// Returns a party snapshot for `player`.
    #[must_use]
    pub fn party_of(&self, player: Uuid) -> Option<&Party> {
        self.membership
            .get(&player)
            .and_then(|leader| self.parties.get(leader))
    }

    /// Invites `target` to the inviting player's party, creating one if needed.
    pub fn invite(&mut self, inviter: Uuid, target: Uuid, tick: u64) -> Result<(), PartyError> {
        if inviter == target {
            return Err(PartyError::AlreadyMember);
        }
        let leader = self.membership.get(&inviter).copied().unwrap_or(inviter);
        if let Some(party) = self.parties.get(&leader) {
            if party.leader() != inviter {
                return Err(PartyError::NotLeader);
            }
            if party.members().contains(&target) {
                return Err(PartyError::AlreadyMember);
            }
            if party.members().len() == PARTY_MAX_MEMBERS {
                return Err(PartyError::Full);
            }
        } else {
            self.parties.insert(leader, Party::new(inviter));
            self.membership.insert(inviter, leader);
        }
        self.invitations.insert(
            target,
            Invitation {
                party_leader: leader,
                expires_at: tick.saturating_add(PARTY_INVITATION_TICKS),
            },
        );
        Ok(())
    }

    /// Accepts the target's unexpired invitation, leaving any old party first.
    pub fn accept(&mut self, target: Uuid, tick: u64) -> Result<Uuid, PartyError> {
        let Some(invitation) = self.invitations.remove(&target) else {
            return Err(PartyError::NoInvitation);
        };
        if tick > invitation.expires_at {
            return Err(PartyError::NoInvitation);
        }
        let Some(party) = self.parties.get(&invitation.party_leader) else {
            return Err(PartyError::NoInvitation);
        };
        if party.members().len() == PARTY_MAX_MEMBERS {
            return Err(PartyError::Full);
        }
        self.leave(target);
        let party = self
            .parties
            .get_mut(&invitation.party_leader)
            .ok_or(PartyError::NoInvitation)?;
        party.add(target)?;
        self.membership.insert(target, invitation.party_leader);
        Ok(invitation.party_leader)
    }

    /// Removes `player` from their party.
    ///
    /// `None` means that `player` was not in any party.
    pub fn leave(&mut self, player: Uuid) -> Option<PartyExit> {
        self.invitations.remove(&player);
        let leader = self.membership.remove(&player)?;
        let mut party = self.parties.remove(&leader)?;
        party.remove(player);
        if party.members().is_empty() {
            return Some(PartyExit::Disbanded);
        }
        let new_leader = party.leader();
        for member in party.members() {
            self.membership.insert(*member, new_leader);
        }
        self.parties.insert(new_leader, party);
        Some(PartyExit::Continued { new_leader })
    }

    /// Removes `target` at the request of the current party leader.
    pub fn kick(&mut self, leader: Uuid, target: Uuid) -> Result<(), PartyError> {
        let Some(party) = self.party_of(leader) else {
            return Err(PartyError::NotInParty);
        };
        if party.leader() != leader {
            return Err(PartyError::NotLeader);
        }
        if !party.members().contains(&target) {
            return Err(PartyError::NotInParty);
        }
        let _ = self.leave(target);
        Ok(())
    }

    /// Returns the party led by `leader`, refusing anyone else.
    pub fn led_party_mut(&mut self, leader: Uuid) -> Result<&mut Party, PartyError> {
        let key = *self.membership.get(&leader).ok_or(PartyError::NotInParty)?;
        let party = self.parties.get_mut(&key).ok_or(PartyError::NotInParty)?;
        if party.leader() != leader {
            return Err(PartyError::NotLeader);
        }
        Ok(party)
    }

    /// Returns every party, for counting shared contract progress.
    pub fn parties_mut(&mut self) -> impl Iterator<Item = &mut Party> {
        self.parties.values_mut()
    }

    /// Expires pending invitations. Party membership itself is not timed.
    pub fn tick(&mut self, tick: u64) {
        self.invitations
            .retain(|_, invitation| tick <= invitation.expires_at);
    }

    /// Captures party rosters and their active group contracts.
    #[must_use]
    pub fn snapshot(&self) -> PartyRegistrySnapshot {
        PartyRegistrySnapshot {
            parties: self
                .parties
                .values()
                .map(|party| PartySnapshot {
                    members: party.members.clone(),
                    contracts: party.contracts.clone(),
                })
                .collect(),
        }
    }

    /// Replaces live parties from a previously published snapshot.
    ///
    /// Pending invitations are intentionally not restored; they are tied to the
    /// previous process's tick counter and would be meaningless after restart.
    pub fn restore(&mut self, snapshot: PartyRegistrySnapshot) {
        self.parties.clear();
        self.membership.clear();
        self.invitations.clear();
        for saved in snapshot.parties {
            if saved.members.is_empty() {
                continue;
            }
            let leader = saved.members[0];
            let mut party = Party::new(leader);
            for member in saved.members.iter().skip(1) {
                if party.add(*member).is_err() {
                    break;
                }
            }
            party.contracts = saved.contracts;
            for member in &party.members {
                self.membership.insert(*member, leader);
            }
            self.parties.insert(leader, party);
        }
    }
}

/// Serialized party registry, published as one gameplay document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PartyRegistrySnapshot {
    /// Active parties; the leader is the first member of each party.
    pub parties: Vec<PartySnapshot>,
}

/// Serialized party: roster and active group contracts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartySnapshot {
    /// Members in leader and join order.
    pub members: Vec<Uuid>,
    /// Active group contracts sharing the party's lifetime.
    pub contracts: Vec<GroupContract>,
}

/// Records one raid member's contribution.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RaidContribution {
    /// Damage that reduced the boss's health.
    pub boss_damage: u64,
    /// Dungeon mobs killed by the member or one of their shadows.
    pub mob_kills: u32,
}

impl RaidContribution {
    /// Returns the weighted contribution score used for loot distribution.
    ///
    /// A mob kill is deliberately worth 20 score: clearing support mobs matters
    /// without outweighing sustained boss damage.
    #[must_use]
    pub const fn score(self) -> u64 {
        self.boss_damage
            .saturating_add((self.mob_kills as u64).saturating_mul(20))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepting_an_invite_leaves_the_old_party_and_preserves_join_order() {
        let leader_a = Uuid::new_v4();
        let leader_b = Uuid::new_v4();
        let member = Uuid::new_v4();
        let mut parties = PartyRuntime::default();

        parties
            .invite(leader_a, member, 0)
            .expect("invite succeeds");
        parties.accept(member, 1).expect("accept succeeds");
        parties
            .invite(leader_b, member, 2)
            .expect("invite succeeds");
        parties.accept(member, 3).expect("accept succeeds");

        assert_eq!(parties.party_of(member).map(Party::leader), Some(leader_b));
        assert!(
            parties
                .party_of(leader_a)
                .is_some_and(|party| party.members().len() == 1)
        );
    }

    #[test]
    fn leader_transfer_follows_member_join_order() {
        let leader = Uuid::new_v4();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let mut parties = PartyRuntime::default();
        for (tick, member) in [(0, first), (1, second)] {
            parties
                .invite(leader, member, tick)
                .expect("invite succeeds");
            parties.accept(member, tick).expect("accept succeeds");
        }

        assert_eq!(
            parties.leave(leader),
            Some(PartyExit::Continued { new_leader: first })
        );
        assert_eq!(parties.party_of(second).map(Party::leader), Some(first));
    }

    #[test]
    fn party_registry_round_trips_rosters_and_group_contracts() {
        use crate::organization::{Contract, GroupContract, Objective, Template};
        use crate::rank::HunterRank;

        let leader = Uuid::new_v4();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let mut parties = PartyRuntime::default();
        parties.invite(leader, first, 0).expect("invite first");
        parties.accept(first, 0).expect("accept first");
        parties.invite(leader, second, 1).expect("invite second");
        parties.accept(second, 1).expect("accept second");

        let template = Template {
            objective: Objective::Boss,
            rank: HunterRank::C,
        };
        let contract = Contract::accepted(template, 1, 7);
        parties
            .parties_mut()
            .next()
            .expect("one party")
            .contracts
            .push(GroupContract {
                contract,
                participants: vec![leader, first, second],
                pending_turn_in: true,
            });

        let bytes = serde_json::to_vec(&parties.snapshot()).expect("snapshot should serialize");
        let snapshot: PartyRegistrySnapshot =
            serde_json::from_slice(&bytes).expect("snapshot should load");

        let mut restored = PartyRuntime::default();
        restored.restore(snapshot);

        assert!(
            restored.invitations.is_empty(),
            "invitations are not restored"
        );
        let party = restored.party_of(leader).expect("leader has a party");
        assert_eq!(party.leader(), leader);
        assert_eq!(party.members(), &[leader, first, second]);
        assert_eq!(party.contracts.len(), 1);
        assert_eq!(party.contracts[0].contract, contract);
        assert_eq!(party.contracts[0].participants, [leader, first, second]);
        assert!(
            !party.contracts[0].pending_turn_in,
            "in-flight turn-in flag is not restored"
        );
    }
}
