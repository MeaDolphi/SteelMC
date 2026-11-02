//! Instant dungeon keys: a System item that opens a gate on the spot.
//!
//! Gates of the world appear on a timer, so a hunter who clears them all has
//! to wait. A key opens a gate of its rank next to the hunter, whenever the
//! hunter chooses. Keys are transferable, so hunters can trade them; the
//! supply is what keeps the shared gates worth walking to.

use steel_core::inventory::container::Container as _;
use steel_core::player::Player;
use steel_registry::{item_stack::ItemStack, vanilla_items};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use uuid::Uuid;

use super::{binding, named, read_kind, record, tag_string, with_lore, write_tag};
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

/// Kind stored in the record of the key a castle floor's boss hands out.
const FLOOR_KIND: &str = "castle_floor_key";
/// Tag field holding the floor whose exit gate the key opens.
const FLOOR: &str = "floor";

/// Creates the key of castle `floor`, bound to the hunter it was handed to.
#[must_use]
pub fn floor_stack(floor: u32, owner: Uuid) -> ItemStack {
    let mut stack = named(
        ItemStack::new(&vanilla_items::OMINOUS_TRIAL_KEY),
        TextComponent::plain(format!("Ключ {floor}-го этажа")).color(Color::Gold),
    );
    stack = with_lore(
        stack,
        vec![
            TextComponent::plain("Трофей стража этажа Замка демонов.").color(Color::Gray),
            TextComponent::plain("Войдите во врата за троном, чтобы подняться выше.")
                .color(Color::Yellow),
        ],
    );
    let mut fields = record(FLOOR_KIND);
    fields.insert(FLOOR, floor.to_string());
    write_tag(&mut stack, fields);
    super::model::apply(&mut stack, "key/castle");
    binding::bound(stack, owner)
}

/// Returns the floor a castle floor key opens, or `None` for other items.
#[must_use]
pub fn key_floor(stack: &ItemStack) -> Option<u32> {
    tag_string(&read_kind(stack, FLOOR_KIND)?, FLOOR)?
        .parse()
        .ok()
}

/// Takes one key of `floor` belonging to `player` from their inventory and
/// returns whether there was one.
pub fn take_floor_key(player: &Player, floor: u32) -> bool {
    let owner = player.gameprofile.id();
    let mut inventory = player.inventory.lock();
    let Some(slot) = (0..inventory.get_container_size()).find(|slot| {
        let stack = inventory.get_item(*slot);
        key_floor(stack) == Some(floor) && binding::bound_owner(stack) == Some(owner)
    }) else {
        return false;
    };
    let _ = inventory.remove_item(slot, 1);
    true
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

    use uuid::Uuid;

    use super::{floor_stack, key_floor, rank, stack};
    use crate::item::binding;
    use crate::rank::HunterRank;

    #[test]
    fn a_key_keeps_its_rank() {
        init_vanilla_registry();
        for key_rank in [HunterRank::E, HunterRank::B, HunterRank::S] {
            assert_eq!(rank(&stack(key_rank)), Some(key_rank));
        }
    }

    #[test]
    fn a_floor_key_names_its_floor_and_its_owner() {
        init_vanilla_registry();
        let owner = Uuid::new_v4();
        let key = floor_stack(37, owner);
        assert_eq!(key_floor(&key), Some(37));
        assert_eq!(binding::bound_owner(&key), Some(owner));
        // A floor key is no dungeon key of any rank.
        assert_eq!(rank(&key), None);
    }
}
