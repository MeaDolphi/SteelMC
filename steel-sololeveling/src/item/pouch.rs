//! Won pouches: coin purses dropped by gate mobs and bosses.
//!
//! A pouch is an ordinary, tradeable stack carrying its amount; a right click
//! empties it into the holder's Won balance.

use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_items;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use super::{named, read_kind, record, tag_string, with_lore, write_tag};
use crate::organization::rank_multiplier;
use crate::rank::HunterRank;

/// Themed kind of a pouch.
const KIND: &str = "won_pouch";
/// Tag field holding the amount.
const AMOUNT: &str = "amount";
/// Pack model of a pouch.
const MODEL: &str = "won_pouch";
/// Percent chance that a gate mob drops a pouch.
pub const MOB_POUCH_PERCENT: u32 = 2;

/// Returns the Won a gate mob's pouch holds.
#[must_use]
pub const fn mob_amount(rank: HunterRank) -> u64 {
    10 * rank_multiplier(rank)
}

/// Returns the Won a boss's pouch holds.
#[must_use]
pub const fn boss_amount(rank: HunterRank) -> u64 {
    100 * rank_multiplier(rank)
}

/// Creates a pouch holding `amount` Won.
#[must_use]
pub fn stack(amount: u64) -> ItemStack {
    let mut stack = with_lore(
        named(
            ItemStack::new(&vanilla_items::GOLD_NUGGET),
            TextComponent::plain("Кошель Won").color(Color::Gold),
        ),
        vec![
            TextComponent::plain(format!("Содержит {amount} Won.")).color(Color::Yellow),
            TextComponent::plain("ПКМ — зачислить на счёт.").color(Color::Gray),
        ],
    );
    let mut fields = record(KIND);
    fields.insert(AMOUNT, amount.to_string());
    write_tag(&mut stack, fields);
    super::model::apply(&mut stack, MODEL);
    stack
}

/// Returns the Won a pouch holds, or `None` for anything else.
#[must_use]
pub fn amount(stack: &ItemStack) -> Option<u64> {
    tag_string(&read_kind(stack, KIND)?, AMOUNT)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;

    use super::{amount, boss_amount, mob_amount, stack};
    use crate::rank::HunterRank;

    #[test]
    fn a_pouch_round_trips_its_amount() {
        init_vanilla_registry();
        assert_eq!(amount(&stack(320)), Some(320));
        assert_eq!(mob_amount(HunterRank::D), 20);
        assert_eq!(boss_amount(HunterRank::S), 3_200);
    }
}
