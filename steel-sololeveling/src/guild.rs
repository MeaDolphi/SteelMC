//! Persistent guilds and their membership/treasury rules.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use steel_registry::item_stack::ItemStack;
use uuid::Uuid;

use crate::economy::Won;
use crate::item::nbt;
use crate::organization::{Contract, GuildBoard, OrganizationError};

/// Maximum online and offline membership of one guild.
pub const GUILD_MAX_MEMBERS: usize = 50;

/// Authority granted to a guild member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuildRole {
    /// Owns the guild and can manage every guild setting and member.
    Guildmaster,
    /// Can invite, remove lower-ranked members, and operate the guild bank.
    Officer,
    /// Can invite new recruits.
    Member,
    /// Has no guild-management authority.
    Recruit,
}

impl GuildRole {
    /// Returns whether this role may invite a new guild member.
    #[must_use]
    pub const fn can_invite(self) -> bool {
        matches!(self, Self::Guildmaster | Self::Officer | Self::Member)
    }

    /// Returns whether this role may remove `target`.
    #[must_use]
    pub const fn can_remove(self, target: Self) -> bool {
        matches!(self, Self::Guildmaster)
            || matches!(self, Self::Officer) && matches!(target, Self::Member | Self::Recruit)
    }

    /// Returns whether this role may edit the shared item bank.
    #[must_use]
    pub const fn can_manage_bank(self) -> bool {
        matches!(self, Self::Guildmaster | Self::Officer)
    }
}

/// A persistent guild. `bank_items` contains persistent NBT-encoded item stacks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Guild {
    /// Display name chosen by its guildmaster.
    pub name: String,
    /// Guild progression level.
    pub level: u32,
    /// Shared Won balance.
    pub treasury: Won,
    /// Members and their respective authority.
    pub members: FxHashMap<Uuid, GuildRole>,
    /// NBT-encoded persistent bank slots.
    #[serde(default)]
    pub bank_items: Vec<Vec<u8>>,
    /// Adventurer Organization contracts and payments owed to members.
    #[serde(default)]
    pub organization: GuildBoard,
}

impl Guild {
    /// Creates a new level-one guild.
    #[must_use]
    pub fn new(name: String, guildmaster: Uuid) -> Self {
        let mut members = FxHashMap::default();
        members.insert(guildmaster, GuildRole::Guildmaster);
        Self {
            name,
            level: 1,
            treasury: Won::zero(),
            members,
            bank_items: Vec::new(),
            organization: GuildBoard::default(),
        }
    }

    /// Returns the authority held by `player`.
    #[must_use]
    pub fn role_of(&self, player: Uuid) -> Option<GuildRole> {
        self.members.get(&player).copied()
    }

    /// Adds `player` in the recruit role.
    pub fn add_recruit(&mut self, player: Uuid) -> Result<(), GuildError> {
        if self.members.contains_key(&player) {
            return Err(GuildError::AlreadyMember);
        }
        if self.members.len() == GUILD_MAX_MEMBERS {
            return Err(GuildError::Full);
        }
        self.members.insert(player, GuildRole::Recruit);
        Ok(())
    }

    /// Decodes the fixed-size bank inventory from persistent NBT slots.
    pub fn bank_contents(&self, slots: usize) -> io::Result<Vec<ItemStack>> {
        let mut items = self
            .bank_items
            .iter()
            .map(|bytes| nbt::decode(bytes))
            .collect::<io::Result<Vec<_>>>()?;
        items.resize(slots, ItemStack::empty());
        items.truncate(slots);
        Ok(items)
    }

    /// Replaces the persistent bank inventory with encoded slots.
    pub fn set_bank_contents(&mut self, items: &[ItemStack]) -> io::Result<()> {
        self.bank_items = items
            .iter()
            .map(nbt::encode)
            .collect::<io::Result<Vec<_>>>()?;
        Ok(())
    }
}

impl GuildError {
    /// Returns how the refusal reads in chat.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::AlreadyInGuild => "Игрок уже состоит в гильдии.",
            Self::AlreadyMember => "Игрок уже состоит в этой гильдии.",
            Self::Full => "В гильдии уже 50 участников.",
            Self::Missing => "Гильдия или её данные не найдены.",
            Self::NameTaken => "Гильдия с таким названием уже существует.",
            Self::InvalidName => "Название: 3–24 символа, латиница, цифры и _.",
            Self::PermissionDenied => "Ваш ранг не даёт этого права.",
            Self::CannotRemoveGuildmaster => "Нельзя исключить гильдмастера.",
            Self::CannotChangeRole => "Менять ранги может только гильдмастер.",
            Self::Storage => "Не удалось сохранить данные гильдии.",
            Self::BankInUse => "Банк гильдии уже открыт другим участником.",
            Self::InsufficientFunds => "Недостаточно Won.",
            Self::TransactionPending => "Дождитесь завершения текущей транзакции.",
            Self::TreasuryOverflow => "В казне недостаточно места для этой суммы.",
        }
    }
}

/// Guild mutation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuildError {
    /// The player already belongs to a guild.
    AlreadyInGuild,
    /// The player already belongs to this guild.
    AlreadyMember,
    /// The guild has reached its membership cap.
    Full,
    /// The referenced guild or member is absent.
    Missing,
    /// Another guild already uses the requested name.
    NameTaken,
    /// The requested guild name violates its format.
    InvalidName,
    /// The acting role lacks the requested authority.
    PermissionDenied,
    /// The guildmaster cannot be removed.
    CannotRemoveGuildmaster,
    /// A role cannot be promoted or demoted by the acting member.
    CannotChangeRole,
    /// Guild data could not be written to disk.
    Storage,
    /// Another member is currently editing this guild bank.
    BankInUse,
    /// The hunter does not have enough Won for the requested treasury deposit.
    InsufficientFunds,
    /// A pending financial transaction owns the hunter's balance.
    TransactionPending,
    /// The requested deposit would overflow the guild treasury.
    TreasuryOverflow,
}

/// Legacy JSON source used only when importing into a non-SQLite storage backend.
#[derive(Debug)]
pub struct GuildStorage {
    path: PathBuf,
}

impl GuildStorage {
    /// Creates storage in the supplied Solo Leveling data directory.
    #[must_use]
    pub fn new(base_dir: impl AsRef<Path>) -> Self {
        Self {
            path: base_dir.as_ref().join("guilds.json"),
        }
    }

    /// Loads every guild keyed by its normalized name.
    pub fn load(&self) -> io::Result<FxHashMap<String, Guild>> {
        if !self.path.exists() {
            return Ok(FxHashMap::default());
        }
        let contents = fs::read_to_string(&self.path)?;
        serde_json::from_str(&contents)
            .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))
    }
}

/// Owns all loaded guild data and enforces cross-guild exclusive membership.
#[derive(Debug, Clone)]
pub struct GuildRegistry {
    guilds: FxHashMap<String, Guild>,
    membership: FxHashMap<Uuid, String>,
}

impl GuildRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            guilds: FxHashMap::default(),
            membership: FxHashMap::default(),
        }
    }

    /// Imports old `guilds.json` data; canonical storage is the core document.
    pub fn load_legacy(base_dir: impl AsRef<Path>) -> io::Result<Self> {
        let guilds = GuildStorage::new(base_dir).load()?;
        Self::from_guilds(guilds)
    }

    /// Decodes the canonical transactionally stored guild document.
    pub fn from_document(bytes: &[u8]) -> io::Result<Self> {
        let guilds = serde_json::from_slice(bytes)
            .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?;
        Self::from_guilds(guilds)
    }

    /// Encodes the guild collection as one opaque gameplay document.
    pub fn to_document(&self) -> io::Result<Vec<u8>> {
        serde_json::to_vec(&self.guilds)
            .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))
    }

    fn from_guilds(guilds: FxHashMap<String, Guild>) -> io::Result<Self> {
        let mut membership = FxHashMap::default();
        for (key, guild) in &guilds {
            for member in guild.members.keys() {
                if membership.insert(*member, key.clone()).is_some() {
                    return Err(io::Error::new(
                        ErrorKind::InvalidData,
                        format!("player {member} belongs to multiple guilds"),
                    ));
                }
            }
        }
        Ok(Self { guilds, membership })
    }

    /// Replaces the registry with a freshly loaded document.
    pub fn replace_from_document(&mut self, bytes: &[u8]) -> io::Result<()> {
        *self = Self::from_document(bytes)?;
        Ok(())
    }

    #[must_use]
    /// Returns the guild of `player`.
    pub fn guild_of(&self, player: Uuid) -> Option<&Guild> {
        self.membership
            .get(&player)
            .and_then(|key| self.guilds.get(key))
    }

    /// Returns the guild of `player` for mutation.
    pub fn guild_of_mut(&mut self, player: Uuid) -> Option<&mut Guild> {
        let key = self.membership.get(&player)?;
        self.guilds.get_mut(key)
    }

    /// Returns every guild, for counting shared contract progress.
    pub fn guilds_mut(&mut self) -> impl Iterator<Item = &mut Guild> {
        self.guilds.values_mut()
    }

    /// Creates a guild and assigns its guildmaster.
    pub fn create(&mut self, name: String, guildmaster: Uuid) -> Result<(), GuildError> {
        if !valid_name(&name) {
            return Err(GuildError::InvalidName);
        }
        if self.membership.contains_key(&guildmaster) {
            return Err(GuildError::AlreadyInGuild);
        }
        let key = normalize_name(&name);
        if self.guilds.contains_key(&key) {
            return Err(GuildError::NameTaken);
        }
        self.guilds
            .insert(key.clone(), Guild::new(name, guildmaster));
        self.membership.insert(guildmaster, key);
        Ok(())
    }

    /// Adds `recruit` to the actor's guild with the recruit role.
    pub fn add_recruit(&mut self, guildmaster: Uuid, recruit: Uuid) -> Result<(), GuildError> {
        if self.membership.contains_key(&recruit) {
            return Err(GuildError::AlreadyInGuild);
        }
        let key = self
            .membership
            .get(&guildmaster)
            .cloned()
            .ok_or(GuildError::Missing)?;
        let guild = self.guilds.get_mut(&key).ok_or(GuildError::Missing)?;
        if !guild
            .role_of(guildmaster)
            .is_some_and(GuildRole::can_invite)
        {
            return Err(GuildError::PermissionDenied);
        }
        guild.add_recruit(recruit)?;
        self.membership.insert(recruit, key);
        Ok(())
    }

    /// Removes `target` when `actor` has authority over their role.
    pub fn remove(&mut self, actor: Uuid, target: Uuid) -> Result<(), GuildError> {
        let key = self
            .membership
            .get(&actor)
            .cloned()
            .ok_or(GuildError::Missing)?;
        if self.membership.get(&target) != Some(&key) {
            return Err(GuildError::Missing);
        }
        let guild = self.guilds.get_mut(&key).ok_or(GuildError::Missing)?;
        let actor_role = guild.role_of(actor).ok_or(GuildError::Missing)?;
        let target_role = guild.role_of(target).ok_or(GuildError::Missing)?;
        if target_role == GuildRole::Guildmaster {
            return Err(GuildError::CannotRemoveGuildmaster);
        }
        if !actor_role.can_remove(target_role) {
            return Err(GuildError::PermissionDenied);
        }
        guild.members.remove(&target);
        self.membership.remove(&target);
        Ok(())
    }

    /// Changes a lower-ranked member's role.
    pub fn set_role(
        &mut self,
        actor: Uuid,
        target: Uuid,
        role: GuildRole,
    ) -> Result<(), GuildError> {
        let key = self
            .membership
            .get(&actor)
            .cloned()
            .ok_or(GuildError::Missing)?;
        if self.membership.get(&target) != Some(&key) {
            return Err(GuildError::Missing);
        }
        let guild = self.guilds.get_mut(&key).ok_or(GuildError::Missing)?;
        let actor_role = guild.role_of(actor).ok_or(GuildError::Missing)?;
        let target_role = guild.role_of(target).ok_or(GuildError::Missing)?;
        if actor_role != GuildRole::Guildmaster || target_role == GuildRole::Guildmaster {
            return Err(GuildError::CannotChangeRole);
        }
        if role == GuildRole::Guildmaster {
            return Err(GuildError::CannotChangeRole);
        }
        guild.members.insert(target, role);
        Ok(())
    }

    /// Lets a non-guildmaster leave their guild.
    pub fn leave(&mut self, player: Uuid) -> Result<(), GuildError> {
        let key = self
            .membership
            .get(&player)
            .cloned()
            .ok_or(GuildError::Missing)?;
        let guild = self.guilds.get_mut(&key).ok_or(GuildError::Missing)?;
        if guild.role_of(player) == Some(GuildRole::Guildmaster) {
            return Err(GuildError::CannotRemoveGuildmaster);
        }
        guild.members.remove(&player);
        self.membership.remove(&player);
        Ok(())
    }

    /// Replaces the persisted bank slots when an authorized viewer closes it.
    pub fn set_bank_items(&mut self, player: Uuid, items: &[ItemStack]) -> Result<(), GuildError> {
        let key = self
            .membership
            .get(&player)
            .cloned()
            .ok_or(GuildError::Missing)?;
        let guild = self.guilds.get_mut(&key).ok_or(GuildError::Missing)?;
        if !guild
            .role_of(player)
            .is_some_and(GuildRole::can_manage_bank)
        {
            return Err(GuildError::PermissionDenied);
        }
        guild
            .set_bank_contents(items)
            .map_err(|_| GuildError::Storage)?;
        Ok(())
    }

    /// Deposits Won into the actor's guild treasury.
    pub fn deposit_treasury(&mut self, player: Uuid, amount: u64) -> Result<(), GuildError> {
        let key = self
            .membership
            .get(&player)
            .cloned()
            .ok_or(GuildError::Missing)?;
        let guild = self.guilds.get_mut(&key).ok_or(GuildError::Missing)?;
        if !guild
            .role_of(player)
            .is_some_and(GuildRole::can_manage_bank)
        {
            return Err(GuildError::PermissionDenied);
        }
        if !guild.treasury.try_credit(amount) {
            return Err(GuildError::TreasuryOverflow);
        }
        Ok(())
    }

    /// Stages one completed guild contract settlement in this registry copy.
    pub fn settle_contract(
        &mut self,
        actor: Uuid,
        index: usize,
    ) -> Result<(Contract, Vec<Uuid>), OrganizationError> {
        let guild = self
            .guild_of_mut(actor)
            .ok_or(OrganizationError::NotInGuild)?;
        let members: Vec<_> = guild.members.keys().copied().collect();
        let group = guild
            .organization
            .contracts
            .get_mut(index)
            .ok_or(OrganizationError::Missing)?;
        group.pending_turn_in = false;
        guild
            .organization
            .turn_in(index, |hunter| members.contains(&hunter))
    }

    /// Returns the normalized guild key and its bank contents for an authorized member.
    pub fn bank_contents_for(
        &self,
        player: Uuid,
        slots: usize,
    ) -> Result<(String, Vec<ItemStack>), GuildError> {
        let key = self
            .membership
            .get(&player)
            .cloned()
            .ok_or(GuildError::Missing)?;
        let guild = self.guilds.get(&key).ok_or(GuildError::Missing)?;
        if !guild
            .role_of(player)
            .is_some_and(GuildRole::can_manage_bank)
        {
            return Err(GuildError::PermissionDenied);
        }
        let items = guild
            .bank_contents(slots)
            .map_err(|_| GuildError::Storage)?;
        Ok((key, items))
    }
}

fn normalize_name(name: &str) -> String {
    name.to_lowercase()
}

fn valid_name(name: &str) -> bool {
    let length = name.chars().count();
    (3..=24).contains(&length)
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guild_membership_is_exclusive_and_round_trips_through_the_gameplay_document() {
        let guildmaster = Uuid::new_v4();
        let recruit = Uuid::new_v4();
        let mut guilds = GuildRegistry::empty();
        guilds
            .create("Hunters".to_owned(), guildmaster)
            .expect("create guild");
        guilds
            .add_recruit(guildmaster, recruit)
            .expect("add recruit");
        let loaded = GuildRegistry::from_document(&guilds.to_document().expect("serialize guilds"))
            .expect("load committed registry");
        assert_eq!(
            loaded.guild_of(recruit).map(|guild| guild.name.as_str()),
            Some("Hunters")
        );
    }
}
