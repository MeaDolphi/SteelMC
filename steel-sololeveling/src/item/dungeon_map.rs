//! The live dungeon map occupying the hotbar slot beside the System book.

use steel_core::inventory::container::Container as _;
use steel_core::player::Player;
use steel_core::player::player_inventory::Prediction;
use steel_registry::data_components::components::MapId;
use steel_registry::data_components::vanilla_components::MAP_ID;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_items;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use super::{named, read_kind, record, with_lore, write_tag};

const KIND: &str = "dungeon_map";
pub(crate) const SLOT: usize = 7;

#[must_use]
pub(crate) fn stack(map_id: MapId) -> ItemStack {
    let mut stack = with_lore(
        named(
            ItemStack::new(&vanilla_items::FILLED_MAP),
            TextComponent::plain("Карта подземелья").color(Color::Aqua),
        ),
        vec![
            TextComponent::plain("Зелёный — зачищено, оранжевый — бой").color(Color::Gray),
            TextComponent::plain("Красный — не зачищено, золотой — страж повержен")
                .color(Color::Gray),
            TextComponent::plain("Метки показывают отряд, босса, тайники и выход")
                .color(Color::Gray),
        ],
    );
    stack.set(MAP_ID, map_id);
    write_tag(&mut stack, record(KIND));
    stack
}

#[must_use]
pub(crate) fn is_dungeon_map(stack: &ItemStack) -> bool {
    read_kind(stack, KIND).is_some()
}

/// Keeps exactly the current run's map in slot 7, or removes every dungeon
/// map when the hunter is outside a mapped run.
pub(crate) fn keep_in_place(player: &Player, current: Option<MapId>) {
    if player.inventory_transaction_active() {
        return;
    }
    let mut displaced = None;
    {
        let mut inventory = player.inventory.lock();
        for slot in 0..inventory.get_container_size() {
            let held = inventory.get_item(slot);
            if !is_dungeon_map(held) {
                continue;
            }
            if slot == SLOT && current == held.get(MAP_ID).copied() {
                continue;
            }
            inventory.set_item(slot, ItemStack::empty());
        }
        if let Some(map_id) = current {
            let held = inventory.get_item(SLOT);
            if !is_dungeon_map(held) || held.get(MAP_ID) != Some(&map_id) {
                if !held.is_empty() && !is_dungeon_map(held) {
                    displaced = Some(held.clone());
                }
                inventory.set_item(SLOT, stack(map_id));
            }
        }
    }
    if let Some(stack) = displaced {
        player.add_item_or_drop(stack, Prediction::ServerOnly);
    }
}
