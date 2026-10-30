//! Magic crystals: diamonds carrying the rank of the dungeon they were mined in.
//!
//! The Organization is the only buyer, and it pays by crystal rank. A plain
//! vanilla diamond has no rank and cannot be sold, which is why the rank is a
//! persisted tag and not an item name.

use steel_registry::{item_stack::ItemStack, vanilla_items};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use super::{named, read_kind, record, tag_string, with_lore, write_tag};
use crate::rank::HunterRank;

/// Kind stored in the record of a magic crystal.
const KIND: &str = "crystal";
/// Tag field holding the rank the crystal was mined at.
const RANK: &str = "rank";

/// Creates one magic crystal of `rank`.
#[must_use]
pub fn stack(rank: HunterRank) -> ItemStack {
    let mut stack = named(
        ItemStack::new(&vanilla_items::DIAMOND),
        TextComponent::plain(format!("Магический кристалл ранга {}", rank.name()))
            .color(Color::Aqua),
    );
    stack = with_lore(
        stack,
        vec![
            TextComponent::plain("Добыт во вратах и сохраняет их ранг.").color(Color::Gray),
            TextComponent::plain(format!(
                "Организация авантюристов купит его за {} Won.",
                sale_price(rank)
            ))
            .color(Color::Yellow),
        ],
    );
    let mut fields = record(KIND);
    fields.insert(RANK, rank.name().to_owned());
    write_tag(&mut stack, fields);
    super::model::apply(
        &mut stack,
        &format!("crystal/{}", rank.name().to_lowercase()),
    );
    stack
}

/// Returns the rank of a magic crystal, or `None` for an ordinary diamond.
#[must_use]
pub fn rank(stack: &ItemStack) -> Option<HunterRank> {
    let tag = read_kind(stack, KIND)?;
    HunterRank::from_name(&tag_string(&tag, RANK)?)
}

/// Returns what the Organization pays for one crystal of `rank`.
#[must_use]
pub const fn sale_price(rank: HunterRank) -> u64 {
    5 * rank.payment_multiplier()
}

/// One sale of magic crystals to the Adventurer Organization.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CrystalSale {
    /// Number of crystals sold.
    pub count: u32,
    /// Won the Organization paid for them.
    pub total: u64,
}

/// Why the Organization refused to buy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrystalSaleError {
    /// The hunter carries no ranked crystal.
    NoCrystals,
    /// The account store cannot accept the payment right now.
    Storage,
}
