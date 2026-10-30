//! Marks what mobs dropped inside an instance.
//!
//! A hunter who dies in a dungeon or the penalty zone keeps their own gear,
//! food and rewards, and loses only what the run's mobs dropped. The mark is
//! the only way to tell the two apart, so it is put on every mob drop inside
//! the instance world and taken off again once the hunter is back outside,
//! where the stacks merge with unmarked ones as usual.

use simdnbt::owned::NbtCompound;
use steel_core::inventory::container::Container as _;
use steel_core::player::Player;
use steel_registry::data_components::components::CustomData;
use steel_registry::data_components::vanilla_components::CUSTOM_DATA;
use steel_registry::item_stack::ItemStack;

/// `custom_data` key of the mark; kept outside the Solo Leveling record so it
/// never changes how a themed item reads.
const MARK: &str = "sololeveling_run_loot";

/// Marks `stack` as dropped by a mob inside an instance.
pub fn mark(stack: &mut ItemStack) {
    if stack.is_empty() {
        return;
    }
    let mut tag = NbtCompound::new();
    tag.insert(MARK, 1_i8);
    if let Some(data) = CustomData::try_from_compound(tag) {
        stack.set_custom_data(&data);
    }
}

/// Returns whether `stack` was dropped by a mob inside an instance.
#[must_use]
pub fn is_marked(stack: &ItemStack) -> bool {
    stack
        .get(CUSTOM_DATA)
        .is_some_and(|data| data.as_compound().contains(MARK))
}

/// Removes the mark from `stack`; returns whether it had one.
pub fn unmark(stack: &mut ItemStack) -> bool {
    let Some(data) = stack.get(CUSTOM_DATA) else {
        return false;
    };
    let mut tag = data.copy_tag();
    if tag.remove(MARK).is_none() {
        return false;
    }
    match CustomData::try_from_compound(tag) {
        Some(data) if !data.is_empty() => stack.set(CUSTOM_DATA, data),
        _ => stack.remove(CUSTOM_DATA),
    }
    true
}

/// Clears the mark from everything `player` carries.
pub fn unmark_inventory(player: &Player) {
    if player.inventory_transaction_active() {
        return;
    }
    let mut changed = false;
    {
        let mut inventory = player.inventory.lock();
        for slot in 0..inventory.get_container_size() {
            if !is_marked(inventory.get_item(slot)) {
                continue;
            }
            let mut stack = inventory.get_item(slot).clone();
            unmark(&mut stack);
            inventory.set_item(slot, stack);
            changed = true;
        }
    }
    if changed {
        player.send_inventory_to_remote();
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, item_stack::ItemStack, vanilla_items};

    use super::{is_marked, mark, unmark};

    #[test]
    fn marking_round_trips_and_restores_a_plain_stack() {
        init_vanilla_registry();
        let plain = ItemStack::new(&vanilla_items::ROTTEN_FLESH);
        let mut stack = plain.clone();
        assert!(!is_marked(&stack));
        mark(&mut stack);
        assert!(is_marked(&stack));
        assert_ne!(stack, plain);
        assert!(unmark(&mut stack));
        assert!(!is_marked(&stack));
        assert_eq!(stack, plain);
    }
}
