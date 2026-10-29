//! The Adventurer Organization board menu.
//!
//! The left five columns offer every template by rank and objective. Column
//! seven lists the hunter's personal contracts and column eight the party's
//! group contracts. Every click acts on live state and then redraws the menu.

use std::sync::Arc;

use steel_core::inventory::container::Container as _;
use steel_core::inventory::prelude::*;
use steel_registry::items::ItemRef;
use steel_registry::{vanilla_items, vanilla_menu_types};
use steel_utils::locks::IntoShared;
use steel_utils::{DowncastType, DowncastTypeKey};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use crate::item::{named, with_lore};
use crate::organization::{
    Contract, GROUP_LIMIT, OBJECTIVES, Objective, OrganizationError, PERSONAL_LIMIT, RANKS,
    Template,
};
use crate::state::{BoardView, SoloLevelingState};

const COLUMNS: usize = 9;
const SLOTS: usize = 54;
const PERSONAL_COLUMN: usize = 6;
const GROUP_COLUMN: usize = 7;
const HELP_SLOT: usize = 8;

/// Opens the board for an online hunter.
pub fn open(player: Arc<Player>, state: Arc<SoloLevelingState>) {
    player.open_menu(
        TextComponent::plain("Организация авантюристов").color(Color::DarkGreen),
        move |context| build(context.container_id, context.player, Arc::clone(&state)),
    );
}

fn build(container_id: u8, player: &Player, state: Arc<SoloLevelingState>) -> Menu {
    let view = state.organization_view(player.gameprofile.id());
    let container = SimpleContainer::from_items(menu_items(view.as_ref())).into_shared();
    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X6, container_id);
    let buttons = builder.section_all(Arc::clone(&container));
    builder.player_inventory(&player.inventory);
    builder.build(OrganizationMenuKind {
        state,
        buttons,
        container,
    })
}

/// A board slot and what clicking it means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Button {
    Offer(Template),
    Personal(usize),
    Group(usize),
}

fn button_at(slot: usize) -> Option<Button> {
    let (row, column) = (slot / COLUMNS, slot % COLUMNS);
    if let Some(objective) = OBJECTIVES.get(column) {
        let rank = *RANKS.get(row)?;
        return Some(Button::Offer(Template {
            objective: *objective,
            rank,
        }));
    }
    match column {
        PERSONAL_COLUMN if row < PERSONAL_LIMIT => Some(Button::Personal(row)),
        GROUP_COLUMN if row < GROUP_LIMIT => Some(Button::Group(row)),
        _ => None,
    }
}

fn menu_items(view: Option<&BoardView>) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); SLOTS];
    let Some(view) = view else {
        return items;
    };
    for (slot, item) in items.iter_mut().enumerate() {
        *item = match button_at(slot) {
            Some(Button::Offer(template)) => offer_item(view, template),
            Some(Button::Personal(index)) => view.book.contracts.get(index).map_or_else(
                || free_slot("Свободный личный слот"),
                |c| contract_item(*c, false),
            ),
            Some(Button::Group(index)) => view.group.get(index).map_or_else(
                || free_slot("Свободный групповой слот"),
                |group| contract_item(group.contract, true),
            ),
            None if slot == HELP_SLOT => help_item(view),
            None => ItemStack::empty(),
        };
    }
    items
}

fn objective_icon(objective: Objective) -> ItemRef {
    match objective {
        Objective::Mobs => &vanilla_items::IRON_SWORD,
        Objective::Zombies => &vanilla_items::ROTTEN_FLESH,
        Objective::Boss => &vanilla_items::ZOMBIE_HEAD,
        Objective::RankedDungeon => &vanilla_items::COMPASS,
        Objective::Dungeons => &vanilla_items::MAP,
    }
}

fn offer_item(view: &BoardView, template: Template) -> ItemStack {
    if template.rank > view.rank {
        return named(
            ItemStack::new(&vanilla_items::GRAY_STAINED_GLASS_PANE),
            TextComponent::plain(format!("Ранг {} недоступен", template.rank.name()))
                .color(Color::DarkGray),
        );
    }
    let mut lore = vec![
        TextComponent::plain(format!("Оплата: {} Won", template.payment())).color(Color::Gold),
    ];
    if view.book.used_today(template, view.today) {
        lore.push(TextComponent::plain("Уже бралось сегодня.").color(Color::Red));
    } else {
        lore.push(TextComponent::plain("ЛКМ — взять лично.").color(Color::Yellow));
        if view.leader {
            lore.push(TextComponent::plain("ПКМ — взять на группу.").color(Color::Yellow));
        }
    }
    with_lore(
        named(
            ItemStack::new(objective_icon(template.objective)),
            TextComponent::plain(template.title()).color(Color::Green),
        ),
        lore,
    )
}

fn contract_item(contract: Contract, group: bool) -> ItemStack {
    let complete = contract.is_complete();
    let icon = if complete {
        &vanilla_items::EMERALD
    } else {
        &vanilla_items::PAPER
    };
    let prefix = if group { "Группа: " } else { "" };
    let mut lore = vec![
        TextComponent::plain(format!(
            "Прогресс: {}/{}",
            contract.progress,
            contract.template.objective.target()
        ))
        .color(Color::Aqua),
        TextComponent::plain(format!(
            "Оплата: {} Won{}",
            contract.template.payment(),
            if group { " каждому" } else { "" }
        ))
        .color(Color::Gold),
    ];
    if complete {
        lore.push(TextComponent::plain("ЛКМ — сдать.").color(Color::Yellow));
    }
    lore.push(TextComponent::plain("ПКМ — отменить.").color(Color::Red));
    with_lore(
        named(
            ItemStack::new(icon),
            TextComponent::plain(format!("{prefix}{}", contract.template.title()))
                .color(if complete { Color::Green } else { Color::White }),
        ),
        lore,
    )
}

fn free_slot(name: &str) -> ItemStack {
    named(
        ItemStack::new(&vanilla_items::LIGHT_GRAY_STAINED_GLASS_PANE),
        TextComponent::plain(name.to_owned()).color(Color::Gray),
    )
}

fn help_item(view: &BoardView) -> ItemStack {
    with_lore(
        named(
            ItemStack::new(&vanilla_items::BOOK),
            TextComponent::plain("Задания Организации").color(Color::Gold),
        ),
        vec![
            TextComponent::plain(format!("Ваш ранг: {}", view.rank.name())).color(Color::Aqua),
            TextComponent::plain(format!(
                "До {PERSONAL_LIMIT} личных и {GROUP_LIMIT} групповых заданий."
            )),
            TextComponent::plain("Каждое задание — раз в день."),
            TextComponent::plain("Групповыми заданиями управляет лидер."),
        ],
    )
}

struct OrganizationMenuKind {
    state: Arc<SoloLevelingState>,
    buttons: Section,
    container: Shared<SimpleContainer>,
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for OrganizationMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/organization");
}

impl OrganizationMenuKind {
    fn act(
        &self,
        button: Button,
        primary: bool,
        player: &Player,
    ) -> Result<String, OrganizationError> {
        let uuid = player.gameprofile.id();
        let state = &self.state;
        Ok(match (button, primary) {
            (Button::Offer(template), true) => {
                state.accept_personal_contract(uuid, template)?;
                format!("Задание взято: {}.", template.title())
            }
            (Button::Offer(template), false) => {
                state.accept_group_contract(uuid, template)?;
                format!("Групповое задание взято: {}.", template.title())
            }
            (Button::Personal(index), true) => {
                let contract = state.turn_in_personal_contract(uuid, index)?;
                format!(
                    "Задание сдано: {}. Начислено {} Won.",
                    contract.template.title(),
                    contract.template.payment()
                )
            }
            (Button::Personal(index), false) => {
                let contract = state.cancel_personal_contract(uuid, index)?;
                format!("Задание отменено: {}.", contract.template.title())
            }
            (Button::Group(index), true) => {
                let payout = state.turn_in_group_contract(uuid, index)?;
                format!(
                    "Групповое задание сдано: {}. По {} Won получили {} участн.",
                    payout.contract.template.title(),
                    payout.contract.template.payment(),
                    payout.paid.len()
                )
            }
            (Button::Group(index), false) => {
                let contract = state.cancel_group_contract(uuid, index)?;
                format!("Групповое задание отменено: {}.", contract.template.title())
            }
        })
    }
}

impl MenuKind for OrganizationMenuKind {
    fn on_slot_clicked(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        click: Click,
        player: &Player,
    ) -> ClickOutcome {
        let Click::Pickup { slot, button } = click else {
            return if click.slot().is_some_and(|slot| self.buttons.contains(slot)) {
                ClickOutcome::Consume
            } else {
                ClickOutcome::Fallthrough
            };
        };
        if !self.buttons.contains(slot) {
            return ClickOutcome::Fallthrough;
        }
        let Some(target) = button_at(slot - self.buttons.start()) else {
            return ClickOutcome::Consume;
        };
        let primary = button == MouseButton::Left;
        match self.act(target, primary, player) {
            Ok(message) => {
                player.send_message(&TextComponent::plain(message).color(Color::Green));
            }
            Err(error) => {
                player.send_message(&TextComponent::plain(error.message()).color(Color::Red));
            }
        }

        let view = self.state.organization_view(player.gameprofile.id());
        let container_id = ContainerId::from_arc(&self.container);
        if let Some(container) = guard.get_typed_mut::<SimpleContainer>(container_id) {
            for (index, item) in menu_items(view.as_ref()).into_iter().enumerate() {
                container.set_item(index, item);
            }
        }
        ClickOutcome::Consume
    }
}

#[cfg(test)]
mod tests {
    use super::{Button, button_at};
    use crate::organization::{Objective, Template};
    use crate::rank::HunterRank;

    #[test]
    fn slots_map_to_offers_and_contract_columns() {
        assert_eq!(
            button_at(0),
            Some(Button::Offer(Template {
                objective: Objective::Mobs,
                rank: HunterRank::E,
            }))
        );
        assert_eq!(
            button_at(5 * 9 + 4),
            Some(Button::Offer(Template {
                objective: Objective::Dungeons,
                rank: HunterRank::S,
            }))
        );
        assert_eq!(button_at(6), Some(Button::Personal(0)));
        assert_eq!(button_at(2 * 9 + 6), Some(Button::Personal(2)));
        assert_eq!(button_at(3 * 9 + 6), None);
        assert_eq!(button_at(9 + 7), Some(Button::Group(1)));
        assert_eq!(button_at(2 * 9 + 7), None);
        assert_eq!(button_at(5), None);
    }
}
