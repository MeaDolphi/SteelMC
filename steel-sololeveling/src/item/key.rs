//! Instant dungeon keys: a System item that opens a gate on the spot.
//!
//! Gates of the world appear on a timer, so a hunter who clears them all has
//! to wait. A key opens a gate of its rank next to the hunter, whenever the
//! hunter chooses. Keys are transferable, so hunters can trade them; the
//! supply is what keeps the shared gates worth walking to.

use steel_registry::{item_stack::ItemStack, vanilla_items};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use super::{named, read_kind, record, tag_string, with_lore, write_tag};
use crate::rank::HunterRank;

/// Kind stored in the record of a dungeon key.
const KIND: &str = "dungeon_key";
/// Tag field holding the rank of the gate the key opens.
const RANK: &str = "rank";
/// Percent chance that a boss drops a key of its gate's rank to a survivor.
pub const BOSS_KEY_PERCENT: u32 = 20;

/// Creates one dungeon key of `rank`.
#[must_use]
pub fn stack(rank: HunterRank) -> ItemStack {
    let mut stack = named(
        ItemStack::new(&vanilla_items::TRIAL_KEY),
        TextComponent::plain(format!("Ключ от подземелья ранга {}", rank.name()))
            .color(Color::LightPurple),
    );
    stack = with_lore(
        stack,
        vec![
            TextComponent::plain("Открывает врата этого ранга рядом с вами.").color(Color::Gray),
            TextComponent::plain("Работает в главном мире; ранг охотника не ниже ранга ключа.")
                .color(Color::Gray),
            TextComponent::plain("ПКМ — открыть врата.").color(Color::Yellow),
        ],
    );
    let mut fields = record(KIND);
    fields.insert(RANK, rank.name().to_owned());
    write_tag(&mut stack, fields);
    super::model::apply(&mut stack, &format!("key/{}", rank.name().to_lowercase()));
    stack
}

/// Kind stored in the record of a Demon Castle key.
const CASTLE_KIND: &str = "castle_key";

/// Creates one Demon Castle key.
#[must_use]
pub fn castle_stack() -> ItemStack {
    let mut stack = named(
        ItemStack::new(&vanilla_items::OMINOUS_TRIAL_KEY),
        TextComponent::plain("Ключ Замка демонов").color(Color::DarkRed),
    );
    stack = with_lore(
        stack,
        vec![
            TextComponent::plain("Открывает ваш следующий этаж Замка демонов.").color(Color::Gray),
            TextComponent::plain("Работает в главном мире.").color(Color::Gray),
            TextComponent::plain("ПКМ — подняться на этаж.").color(Color::Yellow),
        ],
    );
    write_tag(&mut stack, record(CASTLE_KIND));
    super::model::apply(&mut stack, "key/castle");
    stack
}

/// Returns whether `stack` is a Demon Castle key.
#[must_use]
pub fn is_castle_key(stack: &ItemStack) -> bool {
    read_kind(stack, CASTLE_KIND).is_some()
}

/// Returns the rank of a dungeon key, or `None` for other items.
#[must_use]
pub fn rank(stack: &ItemStack) -> Option<HunterRank> {
    let tag = read_kind(stack, KIND)?;
    HunterRank::from_name(&tag_string(&tag, RANK)?)
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;

    use super::{rank, stack};
    use crate::rank::HunterRank;

    #[test]
    fn a_key_keeps_its_rank() {
        init_vanilla_registry();
        for key_rank in [HunterRank::E, HunterRank::B, HunterRank::S] {
            assert_eq!(rank(&stack(key_rank)), Some(key_rank));
        }
    }
}
