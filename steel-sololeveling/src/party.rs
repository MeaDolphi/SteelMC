//! Ephemeral party membership, invitations, and raid contribution accounting.

use rustc_hash::FxHashMap;
use uuid::Uuid;

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
}

impl Party {
    /// Creates a party led by `leader`.
    #[must_use]
    pub fn new(leader: Uuid) -> Self {
        Self {
            members: vec![leader],
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
        true
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

impl PartyRuntime {
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

    /// Expires pending invitations. Party membership itself is not timed.
    pub fn tick(&mut self, tick: u64) {
        self.invitations
            .retain(|_, invitation| tick <= invitation.expires_at);
    }
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
}
