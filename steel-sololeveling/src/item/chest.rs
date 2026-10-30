//! Reward chests: the categories, rarity, and per-rank drop tables.
//!
//! A chest is not a roulette. A normal or sacred chest hands out exactly one
//! reward the moment it is opened; a choice chest shows three distinct options
//! and the player keeps one. The three options are rolled once and persisted
//! with the player, so reconnecting or restarting cannot reroll them.
//!
//! Bosses roll the chest type once for the whole squad. If the roll produces a
//! chest, every surviving finisher receives their own chest of that type.

use rand::RngExt as _;
use steel_registry::items::ItemRef;
use steel_registry::{item_stack::ItemStack, vanilla_items};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use super::artifact::{self, ArtifactId, Category as ArtifactCategory, Rarity};
use super::potion::BuffPotion;
use super::shadow;
use super::{named, read_kind, read_tag, record, tag_string, with_lore, write_tag};
use crate::rank::HunterRank;

/// Kind stored in the record of a chest item.
const KIND: &str = "chest";
/// Tag field holding the chest type.
const CHEST: &str = "chest";

/// Chest types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChestKind {
    /// One reward, common tier.
    Normal,
    /// One reward, better category and rarity odds.
    Sacred,
    /// Three distinct options, one of which the player keeps.
    Choice,
}

/// Every chest type, for uniform filtering.
pub const ALL_KINDS: [ChestKind; 3] = [ChestKind::Normal, ChestKind::Sacred, ChestKind::Choice];

impl ChestKind {
    /// Returns the stored name of the chest type.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Sacred => "sacred",
            Self::Choice => "choice",
        }
    }

    /// Parses a stored chest name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        ALL_KINDS.into_iter().find(|kind| kind.name() == name)
    }

    /// Returns the Russian display name.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Normal => "Обычный сундук",
            Self::Sacred => "Священный сундук",
            Self::Choice => "Сундук с выбором",
        }
    }

    /// Returns the tooltip color of the chest type.
    #[must_use]
    pub const fn color(self) -> Color {
        match self {
            Self::Normal => Color::Yellow,
            Self::Sacred => Color::Gold,
            Self::Choice => Color::LightPurple,
        }
    }

    fn holder(self) -> ItemRef {
        match self {
            Self::Normal => &vanilla_items::CHEST,
            Self::Sacred => &vanilla_items::ENDER_CHEST,
            Self::Choice => &vanilla_items::TRAPPED_CHEST,
        }
    }

    /// Creates one chest item.
    #[must_use]
    pub fn stack(self) -> ItemStack {
        let hint = match self {
            Self::Normal | Self::Sacred => "Одна награда при открытии.",
            Self::Choice => "Три варианта, один из них забирает игрок.",
        };
        let mut stack = named(
            ItemStack::new(self.holder()),
            TextComponent::plain(self.display_name().to_owned()).color(self.color()),
        );
        stack = with_lore(
            stack,
            vec![
                TextComponent::plain(hint.to_owned()).color(Color::Gray),
                TextComponent::plain("Правый клик — открыть.").color(Color::Yellow),
            ],
        );
        let mut fields = record(KIND);
        fields.insert(CHEST, self.name().to_owned());
        write_tag(&mut stack, fields);
        super::model::apply(&mut stack, &format!("chest/{}", self.name()));
        stack
    }
}

/// Returns the chest type a stack holds, or `None` for an ordinary chest.
#[must_use]
pub fn kind_of(stack: &ItemStack) -> Option<ChestKind> {
    let tag = read_kind(stack, KIND)?;
    ChestKind::from_name(&tag_string(&tag, CHEST)?)
}

/// One possible chest reward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reward {
    /// A themed buff potion.
    Buff(BuffPotion),
    /// An artifact weapon.
    Weapon(ArtifactId),
    /// An artifact armor piece.
    Armor(ArtifactId),
    /// A transferable shadow-summon item.
    Shadow(usize),
}

/// Reward categories, independent of the concrete prize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// A buff potion.
    Buff,
    /// An artifact weapon.
    Weapon,
    /// An artifact armor piece.
    Armor,
    /// A shadow-summon item.
    Shadow,
}

impl Reward {
    /// Returns whether two rewards are the same concrete prize.
    #[must_use]
    pub fn same(self, other: Self) -> bool {
        self == other
    }

    /// Builds the item stack of this reward.
    #[must_use]
    pub fn stack(self, owner: Option<uuid::Uuid>) -> ItemStack {
        let stack = match self {
            Self::Buff(buff) => buff.stack(),
            Self::Weapon(id) | Self::Armor(id) => artifact::stack(id, None),
            Self::Shadow(index) => shadow::catalog_stack(index),
        };
        // A personal chest yields personal loot: otherwise buying one and
        // reselling its content would move a shop purchase between hunters.
        match owner {
            Some(owner) => super::binding::bound(stack, owner),
            None => stack,
        }
    }

    /// Returns the Russian description shown in menus and messages.
    #[must_use]
    pub fn display_name(self) -> String {
        match self {
            Self::Buff(buff) => buff.display_name().to_owned(),
            Self::Weapon(id) | Self::Armor(id) => {
                let artifact = id.artifact();
                format!("{} ({})", artifact.name, artifact.rarity.display_name())
            }
            Self::Shadow(index) => shadow::catalog_name(index),
        }
    }
}

/// Returns the category of a uniform category roll in `0..100`.
#[must_use]
pub const fn category_for(kind: ChestKind, roll: u32) -> Category {
    match kind {
        ChestKind::Normal => match roll {
            0..50 => Category::Buff,
            50..70 => Category::Weapon,
            70..90 => Category::Armor,
            _ => Category::Shadow,
        },
        ChestKind::Sacred | ChestKind::Choice => match roll {
            0..20 => Category::Buff,
            20..50 => Category::Weapon,
            50..80 => Category::Armor,
            _ => Category::Shadow,
        },
    }
}

/// Returns the rarity of a uniform rarity roll in `0..100`.
#[must_use]
pub const fn rarity_for(kind: ChestKind, roll: u32) -> Rarity {
    match kind {
        ChestKind::Normal => match roll {
            0..90 => Rarity::Rare,
            90..99 => Rarity::Epic,
            _ => Rarity::Legendary,
        },
        ChestKind::Sacred | ChestKind::Choice => match roll {
            0..65 => Rarity::Rare,
            65..95 => Rarity::Epic,
            _ => Rarity::Legendary,
        },
    }
}

/// Rolls one reward of `kind`.
#[must_use]
pub fn roll(kind: ChestKind) -> Reward {
    let mut rng = rand::rng();
    reward_from(
        kind,
        rng.random_range(0..100),
        rng.random_range(0..100),
        rng.random_range(0..usize::MAX),
        rng.random_range(0..shadow::CATALOG.len()),
    )
}

/// Resolves one reward from explicit rolls, so the tables stay testable.
#[must_use]
pub fn reward_from(
    kind: ChestKind,
    category_roll: u32,
    rarity_roll: u32,
    item_roll: usize,
    shadow_roll: usize,
) -> Reward {
    let category = category_for(kind, category_roll);
    let rarity = rarity_for(kind, rarity_roll);
    match category {
        Category::Buff => Reward::Buff(super::potion::ALL[item_roll % super::potion::ALL.len()]),
        Category::Weapon => pick_artifact(artifact::weapons_of(rarity), item_roll),
        Category::Armor => pick_artifact(artifact::armors_of(rarity), item_roll),
        Category::Shadow => Reward::Shadow(shadow_roll % shadow::CATALOG.len()),
    }
}

fn pick_artifact(candidates: Vec<ArtifactId>, roll: usize) -> Reward {
    let candidates = if candidates.is_empty() {
        artifact::weapons_of(Rarity::Rare)
    } else {
        candidates
    };
    let fallback = candidates[0];
    let id = candidates
        .get(roll % candidates.len().max(1))
        .copied()
        .unwrap_or(fallback);
    if id.artifact().category == ArtifactCategory::Weapon {
        Reward::Weapon(id)
    } else {
        Reward::Armor(id)
    }
}

/// Rolls the three distinct options of a choice chest.
#[must_use]
pub fn roll_choice() -> Vec<Reward> {
    let mut options: Vec<Reward> = Vec::with_capacity(3);
    let mut attempts = 0;
    while options.len() < 3 && attempts < 64 {
        attempts += 1;
        let reward = roll(ChestKind::Choice);
        if options.contains(&reward) {
            continue;
        }
        options.push(reward);
    }
    while options.len() < 3 {
        match first_unused(&options) {
            Some(reward) => options.push(reward),
            None => break,
        }
    }
    options
}

fn first_unused(used: &[Reward]) -> Option<Reward> {
    for buff in super::potion::ALL {
        let reward = Reward::Buff(buff);
        if !used.contains(&reward) {
            return Some(reward);
        }
    }
    for id in artifact::ALL {
        let reward = if id.artifact().category == ArtifactCategory::Weapon {
            Reward::Weapon(id)
        } else {
            Reward::Armor(id)
        };
        if !used.contains(&reward) {
            return Some(reward);
        }
    }
    (0..shadow::CATALOG.len())
        .map(Reward::Shadow)
        .find(|reward| !used.contains(reward))
}

/// Returns the options a choice chest already rolled.
///
/// The options live on the chest stack, so closing the menu, reconnecting, or
/// restarting the server can never reroll them.
#[must_use]
pub fn stored_options(stack: &ItemStack) -> Option<[Reward; 3]> {
    let tag = read_kind(stack, KIND)?;
    let mut options = [Reward::Shadow(0); 3];
    for (index, slot) in options.iter_mut().enumerate() {
        *slot = decode_option(&tag_string(&tag, &option_field(index))?)?;
    }
    Some(options)
}

/// Stores the rolled options on a choice chest stack.
///
/// The record is rebuilt from the stack's own tag so a personal binding and any
/// other Solo Leveling field survive the rewrite.
pub fn store_options(stack: &mut ItemStack, options: &[Reward; 3]) {
    let mut fields = read_tag(stack).unwrap_or_else(|| record(KIND));
    fields.insert(KIND, KIND.to_owned());
    fields.insert(CHEST, ChestKind::Choice.name().to_owned());
    for (index, reward) in options.iter().enumerate() {
        fields.insert(option_field(index), encode_option(*reward));
    }
    write_tag(stack, fields);
}

fn option_field(index: usize) -> String {
    format!("option{index}")
}

fn encode_option(reward: Reward) -> String {
    match reward {
        Reward::Buff(buff) => format!("buff:{}", buff.name()),
        Reward::Weapon(id) => format!("weapon:{}", id.name()),
        Reward::Armor(id) => format!("armor:{}", id.name()),
        Reward::Shadow(index) => format!("shadow:{}", index % shadow::CATALOG.len()),
    }
}

fn decode_option(text: &str) -> Option<Reward> {
    let (category, name) = text.split_once(':')?;
    match category {
        "buff" => Some(Reward::Buff(BuffPotion::from_name(name)?)),
        "weapon" => {
            let id = ArtifactId::from_name(name)?;
            (id.artifact().category == ArtifactCategory::Weapon).then_some(Reward::Weapon(id))
        }
        "armor" => {
            let id = ArtifactId::from_name(name)?;
            (id.artifact().category == ArtifactCategory::Armor).then_some(Reward::Armor(id))
        }
        "shadow" => {
            let index = name.parse::<usize>().ok()?;
            Some(Reward::Shadow(index % shadow::CATALOG.len()))
        }
        _ => None,
    }
}

/// Returns the chest type a boss of `rank` drops, or `None` when it drops none.
#[must_use]
pub const fn boss_chest_for(rank: HunterRank, roll: u32) -> Option<ChestKind> {
    let (none, normal, sacred) = match rank {
        HunterRank::E => (30, 65, 5),
        HunterRank::D => (20, 70, 9),
        HunterRank::C => (10, 70, 17),
        HunterRank::B => (5, 60, 30),
        HunterRank::A => (0, 45, 45),
        HunterRank::S => (0, 25, 60),
    };
    if roll < none {
        None
    } else if roll < none + normal {
        Some(ChestKind::Normal)
    } else if roll < none + normal + sacred {
        Some(ChestKind::Sacred)
    } else {
        Some(ChestKind::Choice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_weights_follow_the_confirmed_table() {
        assert_eq!(category_for(ChestKind::Normal, 0), Category::Buff);
        assert_eq!(category_for(ChestKind::Normal, 49), Category::Buff);
        assert_eq!(category_for(ChestKind::Normal, 50), Category::Weapon);
        assert_eq!(category_for(ChestKind::Normal, 69), Category::Weapon);
        assert_eq!(category_for(ChestKind::Normal, 70), Category::Armor);
        assert_eq!(category_for(ChestKind::Normal, 89), Category::Armor);
        assert_eq!(category_for(ChestKind::Normal, 90), Category::Shadow);
        assert_eq!(category_for(ChestKind::Normal, 99), Category::Shadow);

        assert_eq!(category_for(ChestKind::Sacred, 19), Category::Buff);
        assert_eq!(category_for(ChestKind::Sacred, 20), Category::Weapon);
        assert_eq!(category_for(ChestKind::Sacred, 79), Category::Armor);
        assert_eq!(category_for(ChestKind::Sacred, 80), Category::Shadow);
    }

    #[test]
    fn rarity_weights_follow_the_confirmed_table() {
        assert_eq!(rarity_for(ChestKind::Normal, 89), Rarity::Rare);
        assert_eq!(rarity_for(ChestKind::Normal, 90), Rarity::Epic);
        assert_eq!(rarity_for(ChestKind::Normal, 98), Rarity::Epic);
        assert_eq!(rarity_for(ChestKind::Normal, 99), Rarity::Legendary);

        assert_eq!(rarity_for(ChestKind::Sacred, 64), Rarity::Rare);
        assert_eq!(rarity_for(ChestKind::Sacred, 65), Rarity::Epic);
        assert_eq!(rarity_for(ChestKind::Sacred, 94), Rarity::Epic);
        assert_eq!(rarity_for(ChestKind::Sacred, 95), Rarity::Legendary);
    }

    #[test]
    fn boss_chest_table_sums_to_one_interval_per_rank() {
        for rank in [
            HunterRank::E,
            HunterRank::D,
            HunterRank::C,
            HunterRank::B,
            HunterRank::A,
            HunterRank::S,
        ] {
            let none = (0..100)
                .filter(|roll| boss_chest_for(rank, *roll).is_none())
                .count();
            let total = (0..100)
                .filter(|roll| boss_chest_for(rank, *roll).is_some())
                .count();
            assert_eq!(none + total, 100, "{rank:?}");
        }
        assert_eq!(boss_chest_for(HunterRank::E, 29), None);
        assert_eq!(boss_chest_for(HunterRank::E, 30), Some(ChestKind::Normal));
        assert_eq!(boss_chest_for(HunterRank::S, 99), Some(ChestKind::Choice));
    }

    #[test]
    fn choice_options_are_always_three_distinct_rewards() {
        for _ in 0..200 {
            let options = roll_choice();
            assert_eq!(options.len(), 3);
            assert_ne!(options[0], options[1]);
            assert_ne!(options[1], options[2]);
            assert_ne!(options[0], options[2]);
        }
    }

    #[test]
    fn a_choice_chest_keeps_its_options_and_its_binding() {
        let owner = uuid::Uuid::new_v4();
        let mut stack = ChestKind::Choice.stack();
        super::super::binding::bind(&mut stack, owner);
        let options = [
            Reward::Buff(BuffPotion::Speed),
            Reward::Weapon(ArtifactId::KnightsKiller),
            Reward::Shadow(1),
        ];
        store_options(&mut stack, &options);

        assert_eq!(stored_options(&stack), Some(options));
        assert_eq!(kind_of(&stack), Some(ChestKind::Choice));
        assert_eq!(super::super::binding::bound_owner(&stack), Some(owner));
    }

    #[test]
    fn an_unopened_chest_has_no_stored_options() {
        assert_eq!(stored_options(&ChestKind::Choice.stack()), None);
        assert_eq!(stored_options(&ChestKind::Normal.stack()), None);
        assert_eq!(stored_options(&ItemStack::empty()), None);
    }

    #[test]
    fn chest_and_reward_stacks_round_trip() {
        for kind in ALL_KINDS {
            let stack = kind.stack();
            assert_eq!(kind_of(&stack), Some(kind));
        }
        for reward in [
            Reward::Buff(BuffPotion::Strength),
            Reward::Weapon(ArtifactId::KnightsKiller),
            Reward::Armor(ArtifactId::RedKnightsHelmet),
            Reward::Shadow(0),
        ] {
            assert!(!reward.stack(None).is_empty());
        }
    }
}
