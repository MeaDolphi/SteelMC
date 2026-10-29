//! The System book: a permanent hotbar item that opens the main menu.
//!
//! The book always sits in the last hotbar slot. It cannot be dropped, and
//! any copy found elsewhere in the inventory is removed, so a hunter always
//! has exactly one and never needs a command to reach the menus.

use steel_core::inventory::container::Container as _;
use steel_core::player::Player;
use steel_core::player::player_inventory::Prediction;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_items;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use super::{named, read_kind, record, with_lore, write_tag};

/// Themed kind of the System book.
const KIND: &str = "menu_book";
/// Inventory slot the book lives in: the ninth hotbar slot.
pub const SLOT: usize = 8;

/// Creates the System book.
#[must_use]
pub fn stack() -> ItemStack {
    let mut stack = with_lore(
        named(
            ItemStack::new(&vanilla_items::ENCHANTED_BOOK),
            TextComponent::plain("Система").color(Color::Aqua),
        ),
        vec![TextComponent::plain("Правый клик — главное меню.").color(Color::Gray)],
    );
    write_tag(&mut stack, record(KIND));
    stack
}

/// Returns whether `stack` is the System book.
#[must_use]
pub fn is_menu_book(stack: &ItemStack) -> bool {
    read_kind(stack, KIND).is_some()
}

/// Keeps the book in [`SLOT`] and nowhere else in `player`'s inventory.
///
/// Whatever the hunter put in that slot moves to another free slot, or is
/// dropped when the inventory is full.
pub fn keep_in_place(player: &Player) {
    if player.inventory_transaction_active() {
        return;
    }
    let mut displaced = None;
    {
        let mut inventory = player.inventory.lock();
        for slot in 0..inventory.get_container_size() {
            if slot != SLOT && is_menu_book(inventory.get_item(slot)) {
                inventory.set_item(slot, ItemStack::empty());
            }
        }
        let held = inventory.get_item(SLOT).clone();
        if !is_menu_book(&held) {
            if !held.is_empty() {
                displaced = Some(held);
            }
            inventory.set_item(SLOT, stack());
        }
    }
    if let Some(stack) = displaced {
        player.add_item_or_drop(stack, Prediction::ServerOnly);
    }
}
