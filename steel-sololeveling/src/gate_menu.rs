//! `ChestGUI` navigation for active Solo Leveling gates.

use std::{ops::Range, sync::Arc};

use steel_core::entity::Entity as _;
use steel_core::inventory::prelude::*;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_items;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use crate::dungeon::Gate;
use crate::dungeon::is_instance_world;
use crate::item::{named, with_lore};
use crate::state::{GateMenuAction, SoloLevelingState};
use crate::ui::{After, open_guarded_buttons};

const PAGE_SIZE: usize = 45;

fn page_range(length: usize, page: usize) -> Range<usize> {
    let start = page.saturating_mul(PAGE_SIZE).min(length);
    start..start.saturating_add(PAGE_SIZE).min(length)
}

/// Opens one page of the hunter's active gates.
pub fn open(player: &Player, state: Arc<SoloLevelingState>, page: usize) {
    if player
        .level()
        .is_some_and(|world| is_instance_world(&world))
    {
        player.send_message(
            &TextComponent::plain("Навигация по вратам недоступна в подземелье.").color(Color::Red),
        );
        return;
    }
    let Some(mut gates) = state.active_gates() else {
        player.send_message(
            &TextComponent::plain("Реестр врат сохраняется; повторите позже.").color(Color::Yellow),
        );
        return;
    };
    let world = player.get_world();
    let position = player.position();
    gates.sort_by(|first, second| {
        let first_distance = if first.world == world.key {
            gate_anchor(first).distance_squared(position)
        } else {
            f64::INFINITY
        };
        let second_distance = if second.world == world.key {
            gate_anchor(second).distance_squared(position)
        } else {
            f64::INFINITY
        };
        first_distance
            .total_cmp(&second_distance)
            .then(first.id.cmp(&second.id))
    });
    let page_count = gates.len().div_ceil(PAGE_SIZE).max(1);
    let page = page.min(page_count - 1);
    let range = page_range(gates.len(), page);
    let selected = gates
        .into_iter()
        .skip(range.start)
        .take(range.len())
        .collect::<Vec<_>>();
    let mut contents = vec![ItemStack::empty(); 54];
    for (slot, gate) in selected.iter().enumerate() {
        contents[slot] = gate_item(gate, &world.key, position);
    }
    if page > 0 {
        contents[45] = named(
            ItemStack::new(&vanilla_items::ARROW),
            TextComponent::plain("Предыдущая страница").color(Color::Gold),
        );
    }
    contents[49] = with_lore(
        named(
            ItemStack::new(&vanilla_items::COMPASS),
            TextComponent::plain("Активные врата").color(Color::Aqua),
        ),
        vec![
            TextComponent::plain(format!(
                "Страница {} из {} · выберите врата для переноса поблизости",
                page + 1,
                page_count
            ))
            .color(Color::Gray),
            TextComponent::plain("Вход остаётся у рамки и проверяет ваш ранг.").color(Color::Gray),
        ],
    );
    if page + 1 < page_count {
        contents[53] = named(
            ItemStack::new(&vanilla_items::ARROW),
            TextComponent::plain("Следующая страница").color(Color::Gold),
        );
    }
    open_guarded_buttons(
        player,
        TextComponent::plain("Врата Системы").color(Color::DarkPurple),
        6,
        contents,
        Box::new(move |player, slot, _| {
            if player
                .level()
                .is_some_and(|world| is_instance_world(&world))
            {
                player.send_message(
                    &TextComponent::plain("Навигация по вратам недоступна в подземелье.")
                        .color(Color::Red),
                );
                return After::Keep;
            }
            let uuid = player.gameprofile.id();
            if slot == 45 && page > 0 {
                state.defer_gate_menu_action(uuid, GateMenuAction::OpenPage(page - 1));
            } else if slot == 53 && page + 1 < page_count {
                state.defer_gate_menu_action(uuid, GateMenuAction::OpenPage(page + 1));
            } else if let Some(gate) = selected.get(slot) {
                state.defer_gate_menu_action(uuid, GateMenuAction::Teleport(gate.id));
            }
            After::Keep
        }),
    );
}

fn gate_item(gate: &Gate, current_world: &steel_utils::Identifier, at: glam::DVec3) -> ItemStack {
    let nearby = gate.world == *current_world;
    let distance = if nearby {
        format!(
            "Расстояние: {} блоков",
            gate_anchor(gate).distance(at).round()
        )
    } else {
        format!("Мир: {}:{}", gate.world.namespace, gate.world.path)
    };
    let color = if gate.red { Color::Red } else { Color::Aqua };
    with_lore(
        named(
            ItemStack::new(if gate.red {
                &vanilla_items::RED_STAINED_GLASS
            } else {
                &vanilla_items::END_STONE_BRICKS
            }),
            TextComponent::plain(format!(
                "{}-ранговые врата{}",
                gate.rank.name(),
                if gate.red { " · красные" } else { "" }
            ))
            .color(color),
        ),
        vec![
            TextComponent::plain(format!(
                "Координаты: {}, {}, {}",
                gate.entrance.x(),
                gate.entrance.y(),
                gate.entrance.z()
            ))
            .color(Color::Gray),
            TextComponent::plain(distance).color(Color::Gray),
            TextComponent::plain(if nearby {
                "Нажмите, чтобы перенестись рядом."
            } else {
                "Перенос доступен только в этом мире."
            })
            .color(Color::Yellow),
        ],
    )
}

fn gate_anchor(gate: &Gate) -> glam::DVec3 {
    let (x, y, z) = gate.entrance.get_bottom_center();
    glam::DVec3::new(x + 0.5, y + 1.0, z)
}

#[cfg(test)]
mod tests {
    use super::page_range;

    #[test]
    fn gate_menu_pages_cover_each_gate_once() {
        let first = page_range(91, 0);
        let second = page_range(91, 1);
        let third = page_range(91, 2);
        assert_eq!(first, 0..45);
        assert_eq!(second, 45..90);
        assert_eq!(third, 90..91);
    }
}
