//! `ChestGUI` for account-backed System loot that did not fit in inventory.

use std::ops::Range;
use std::sync::Arc;

use steel_core::entity::Entity;
use steel_core::inventory::prelude::*;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_items;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use crate::dungeon::is_instance_world;
use crate::item::{named, with_lore};
use crate::sound::SystemSound;
use crate::state::SoloLevelingState;
use crate::ui::{After, open_guarded_buttons};

const PAGE_SIZE: usize = 45;

fn page_range(length: usize, page: usize) -> Range<usize> {
    let start = page.saturating_mul(PAGE_SIZE).min(length);
    start..start.saturating_add(PAGE_SIZE).min(length)
}

/// Opens a page of the System's deferred item storage.
pub fn open(player: &Player, state: Arc<SoloLevelingState>, page: usize) {
    if player
        .level()
        .is_some_and(|world| is_instance_world(&world))
    {
        player.send_message(
            &TextComponent::plain("Хранилище недоступно в подземелье.").color(Color::Red),
        );
        return;
    }
    SystemSound::Open.play(player);
    let uuid = player.gameprofile.id();
    let entries = state.deferred_items(uuid);
    let page_count = entries.len().div_ceil(PAGE_SIZE).max(1);
    let page = page.min(page_count - 1);
    let range = page_range(entries.len(), page);
    let mut contents = vec![ItemStack::empty(); 54];
    for (slot, (_, _, item)) in entries[range.clone()].iter().enumerate() {
        contents[slot] = item.clone();
    }
    if page > 0 {
        contents[45] = named(
            ItemStack::new(&vanilla_items::ARROW),
            TextComponent::plain("Предыдущая страница").color(Color::Gold),
        );
    }
    if page + 1 < page_count {
        contents[53] = named(
            ItemStack::new(&vanilla_items::ARROW),
            TextComponent::plain("Следующая страница").color(Color::Gold),
        );
    }
    contents[49] = with_lore(
        named(
            ItemStack::new(&vanilla_items::CHEST),
            TextComponent::plain("Отложенные предметы").color(Color::Aqua),
        ),
        vec![
            TextComponent::plain(format!(
                "Страница {} из {} · нажмите предмет, чтобы забрать",
                page + 1,
                page_count
            ))
            .color(Color::Gray),
        ],
    );
    let selected: Vec<_> = entries
        .into_iter()
        .skip(range.start)
        .take(range.len())
        .collect();
    open_guarded_buttons(
        player,
        TextComponent::plain("Хранилище Системы").color(Color::Aqua),
        6,
        contents,
        Box::new(move |player, slot, _| {
            if player
                .level()
                .is_some_and(|world| is_instance_world(&world))
            {
                player.send_message(
                    &TextComponent::plain("Хранилище недоступно в подземелье.").color(Color::Red),
                );
                return After::Keep;
            }
            if slot == 45 && page > 0 {
                state.defer_deferred_menu_open(player.gameprofile.id(), page - 1);
            } else if slot == 53 && page + 1 < page_count {
                state.defer_deferred_menu_open(player.gameprofile.id(), page + 1);
            } else if let Some((index, bytes, _)) = selected.get(slot) {
                state.request_deferred_withdrawal(player, *index, bytes.clone());
            }
            After::Keep
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::page_range;

    #[test]
    fn deferred_menu_pages_select_each_stored_index_once() {
        let first = page_range(91, 0);
        let second = page_range(91, 1);
        let third = page_range(91, 2);
        assert_eq!(first, 0..45);
        assert_eq!(second, 45..90);
        assert_eq!(third, 90..91);
        assert!(first.end <= second.start && second.end <= third.start);
    }
}
