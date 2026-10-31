//! The Adventurer Organization board menu.
//!
//! The left five columns offer every template by rank and objective. Column
//! seven lists the hunter's personal contracts, column eight the party's group
//! contracts and column nine, below the help book, the guild's contracts.
//! Every click acts on live state and then redraws the menu.

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
    Contract, GROUP_LIMIT, GUILD_LIMIT, GUILD_SCALE, OBJECTIVES, Objective, OrganizationError,
    PERSONAL_LIMIT, RANKS, Template,
};
use crate::starter_menu;
use crate::state::{BoardView, SoloLevelingState};

const COLUMNS: usize = 9;
const SLOTS: usize = 54;
const PERSONAL_COLUMN: usize = 6;
const GROUP_COLUMN: usize = 7;
const GUILD_COLUMN: usize = 8;
const HELP_SLOT: usize = 8;
/// Objective columns shown on one page of the board.
const OFFER_COLUMNS: usize = 6;
/// Button that turns the board to its other page of objectives.
const PAGE_SLOT: usize = 53;

/// Opens the board for a hunter standing at it.
pub fn open(player: &Player, state: Arc<SoloLevelingState>) {
    player.open_menu(
        TextComponent::plain("Организация авантюристов").color(Color::DarkGreen),
        move |context| build(context.container_id, context.player, state, true),
    );
}

/// Opens a read-only list of the hunter's contracts, away from any board.
///
/// Taking and turning in contracts stays at the board, which plays the part
/// of the Organization's NPC.
pub fn open_view(player: &Player, state: Arc<SoloLevelingState>) {
    player.open_menu(
        TextComponent::plain("Мои задания").color(Color::DarkGreen),
        move |context| build(context.container_id, context.player, state, false),
    );
}

fn build(container_id: u8, player: &Player, state: Arc<SoloLevelingState>, at_board: bool) -> Menu {
    let view = state.organization_view(player.gameprofile.id());
    let container =
        SimpleContainer::from_items(menu_items(view.as_ref(), at_board, 0)).into_shared();
    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X6, container_id);
    let buttons = builder.section_all(Arc::clone(&container));
    builder.player_inventory(&player.inventory);
    builder.build(OrganizationMenuKind {
        state,
        buttons,
        container,
        at_board,
        page: 0,
        view,
    })
}

/// A board slot and what clicking it means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Button {
    Offer(Template),
    Page,
    Personal(usize),
    Group(usize),
    Guild(usize),
}

/// How a board slot was clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Left click: take personally, or turn in.
    Primary,
    /// Right click: take for the party, or cancel.
    Secondary,
    /// Shift-click on an offer: take for the guild.
    Guild,
}

fn button_at(slot: usize, page: usize) -> Option<Button> {
    let (row, column) = (slot / COLUMNS, slot % COLUMNS);
    if slot == PAGE_SLOT {
        return Some(Button::Page);
    }
    if column < OFFER_COLUMNS {
        let objective = OBJECTIVES.get(page * OFFER_COLUMNS + column)?;
        let rank = *RANKS.get(row)?;
        return Some(Button::Offer(Template {
            objective: *objective,
            rank,
        }));
    }
    match column {
        PERSONAL_COLUMN if row < PERSONAL_LIMIT => Some(Button::Personal(row)),
        GROUP_COLUMN if row < GROUP_LIMIT => Some(Button::Group(row)),
        GUILD_COLUMN if (1..=GUILD_LIMIT).contains(&row) => Some(Button::Guild(row - 1)),
        _ => None,
    }
}

/// Pages the board needs for every objective.
const PAGES: usize = OBJECTIVES.len().div_ceil(OFFER_COLUMNS);

fn menu_items(view: Option<&BoardView>, at_board: bool, page: usize) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); SLOTS];
    let Some(view) = view else {
        return items;
    };
    for (slot, item) in items.iter_mut().enumerate() {
        *item = match button_at(slot, page) {
            Some(Button::Offer(_) | Button::Page) if !at_board => ItemStack::empty(),
            Some(Button::Page) => with_lore(
                named(
                    ItemStack::new(&vanilla_items::ARROW),
                    TextComponent::plain(format!("Страница {} / {PAGES}", page + 1))
                        .color(Color::Yellow),
                ),
                vec![TextComponent::plain("Клик — другие задания.").color(Color::Gray)],
            ),
            Some(Button::Offer(template)) => offer_item(view, template),
            Some(Button::Personal(index)) => view.book.contracts.get(index).map_or_else(
                || free_slot("Свободный личный слот"),
                |c| contract_item(*c, ""),
            ),
            Some(Button::Group(index)) => view.group.get(index).map_or_else(
                || free_slot("Свободный групповой слот"),
                |group| contract_item(group.contract, "Группа: "),
            ),
            Some(Button::Guild(index)) => match &view.guild {
                Some(guild) => guild.contracts.get(index).map_or_else(
                    || free_slot("Свободный гильдийный слот"),
                    |group| contract_item(group.contract, "Гильдия: "),
                ),
                None => ItemStack::empty(),
            },
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
        Objective::RedGate => &vanilla_items::REDSTONE,
        Objective::CleanGate => &vanilla_items::TOTEM_OF_UNDYING,
        Objective::FastGate => &vanilla_items::CLOCK,
        Objective::CrystalHaul => &vanilla_items::AMETHYST_SHARD,
        Objective::TrophyHaul => &vanilla_items::LEATHER,
        Objective::Nests => &vanilla_items::SPIDER_EYE,
        Objective::Castle => &vanilla_items::NETHER_BRICK,
        Objective::CityDefense => &vanilla_items::SHIELD,
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
    if let Some(guild) = view.guild.as_ref().filter(|guild| guild.officer) {
        if template.rank > guild.rank {
            lore.push(
                TextComponent::plain("Гильдии этот ранг пока недоступен.").color(Color::DarkGray),
            );
        } else if guild.used.contains(&template) {
            lore.push(
                TextComponent::plain("Гильдия уже брала его сегодня.").color(Color::DarkGray),
            );
        } else {
            lore.push(
                TextComponent::plain(format!(
                    "Shift+клик — на гильдию: ×{GUILD_SCALE}, {} Won каждому вкладчику.",
                    template.payment() * u64::from(GUILD_SCALE)
                ))
                .color(Color::Aqua),
            );
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

fn contract_item(contract: Contract, prefix: &str) -> ItemStack {
    let complete = contract.is_complete();
    let icon = if complete {
        &vanilla_items::EMERALD
    } else {
        &vanilla_items::PAPER
    };
    let mut lore = vec![
        TextComponent::plain(format!(
            "Прогресс: {}/{}",
            contract.progress,
            contract.target()
        ))
        .color(Color::Aqua),
        TextComponent::plain(format!(
            "Оплата: {} Won{}",
            contract.payment(),
            if prefix.is_empty() {
                ""
            } else {
                " каждому"
            }
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
            TextComponent::plain(format!(
                "Гильдии: до {GUILD_LIMIT} заданий, цель и оплата ×{GUILD_SCALE}."
            )),
            TextComponent::plain("Гильдийными управляют мастер и офицеры."),
            TextComponent::plain(if view.starter_claimable {
                "Клик — получить стартовый набор Ассоциации!"
            } else {
                "Клик — помощь Ассоциации и городской торговец."
            })
            .color(if view.starter_claimable {
                Color::Green
            } else {
                Color::Yellow
            }),
        ],
    )
}

struct OrganizationMenuKind {
    state: Arc<SoloLevelingState>,
    buttons: Section,
    container: Shared<SimpleContainer>,
    /// Whether the hunter opened the board itself, rather than the read-only list.
    at_board: bool,
    /// Page of objectives the board shows.
    page: usize,
    /// Last state drawn into the container.
    view: Option<BoardView>,
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for OrganizationMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/organization");
}

impl OrganizationMenuKind {
    fn redraw(&mut self, guard: &mut ContainerLockGuard, view: Option<BoardView>) {
        let container_id = ContainerId::from_arc(&self.container);
        if let Some(container) = guard.get_typed_mut::<SimpleContainer>(container_id) {
            for (index, item) in menu_items(view.as_ref(), self.at_board, self.page)
                .into_iter()
                .enumerate()
            {
                container.set_item(index, item);
            }
            self.view = view;
        }
    }

    fn act(
        &self,
        button: Button,
        action: Action,
        player: &Player,
    ) -> Result<String, OrganizationError> {
        let uuid = player.gameprofile.id();
        let state = &self.state;
        Ok(match (button, action) {
            (Button::Offer(template), Action::Guild) => {
                state.accept_guild_contract(uuid, template)?;
                format!(
                    "Принятие гильдийного задания «{}» поставлено в очередь сохранения.",
                    template.title()
                )
            }
            (Button::Guild(index), Action::Primary) => {
                let payout = state.turn_in_guild_contract(uuid, index)?;
                format!(
                    "Сдача гильдийного задания «{}» поставлена в очередь сохранения.",
                    payout.contract.template.title(),
                )
            }
            (Button::Guild(index), _) => {
                let contract = state.cancel_guild_contract(uuid, index)?;
                format!(
                    "Отмена гильдийного задания «{}» поставлена в очередь сохранения.",
                    contract.template.title()
                )
            }
            (_, Action::Guild) => return Err(OrganizationError::Missing),
            (Button::Offer(template), Action::Primary) => {
                state.accept_personal_contract(uuid, template)?;
                format!("Задание взято: {}.", template.title())
            }
            (Button::Offer(template), Action::Secondary) => {
                state.accept_group_contract(uuid, template)?;
                format!("Групповое задание взято: {}.", template.title())
            }
            (Button::Personal(index), Action::Primary) => {
                let contract = state.turn_in_personal_contract(uuid, index)?;
                format!(
                    "Сдача задания «{}» отправлена. Оплата — {} Won.",
                    contract.template.title(),
                    contract.payment()
                )
            }
            (Button::Personal(index), Action::Secondary) => {
                let contract = state.cancel_personal_contract(uuid, index)?;
                format!("Задание отменено: {}.", contract.template.title())
            }
            (Button::Group(index), Action::Primary) => {
                let payout = state.turn_in_group_contract(uuid, index)?;
                format!(
                    "Сдача группового задания «{}» отправлена. По {} Won: {} участн.",
                    payout.contract.template.title(),
                    payout.contract.payment(),
                    payout.paid.len()
                )
            }
            (Button::Group(index), Action::Secondary) => {
                let contract = state.cancel_group_contract(uuid, index)?;
                format!("Групповое задание отменено: {}.", contract.template.title())
            }
            // Turning the page happens before any action reaches here.
            (Button::Page, _) => String::new(),
        })
    }
}

impl MenuKind for OrganizationMenuKind {
    fn on_tick(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        player: &Player,
    ) {
        let view = self.state.organization_view(player.gameprofile.id());
        if self.view != view {
            self.redraw(guard, view);
        }
    }

    fn on_slot_clicked(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        click: Click,
        player: &Player,
    ) -> ClickOutcome {
        let (slot, action) = match click {
            Click::Pickup { slot, button } => (
                slot,
                if button == MouseButton::Left {
                    Action::Primary
                } else {
                    Action::Secondary
                },
            ),
            Click::QuickMove { slot } => (slot, Action::Guild),
            _ => {
                return if click.slot().is_some_and(|slot| self.buttons.contains(slot)) {
                    ClickOutcome::Consume
                } else {
                    ClickOutcome::Fallthrough
                };
            }
        };
        if !self.buttons.contains(slot) {
            return ClickOutcome::Fallthrough;
        }
        if self.at_board && slot - self.buttons.start() == HELP_SLOT {
            starter_menu::open(player, Arc::clone(&self.state));
            return ClickOutcome::Consume;
        }
        let Some(target) = button_at(slot - self.buttons.start(), self.page) else {
            return ClickOutcome::Consume;
        };
        let result = if target == Button::Page {
            self.page = (self.page + 1) % PAGES;
            Ok(String::new())
        } else if self.at_board {
            self.act(target, action, player)
        } else {
            Err(OrganizationError::NotAtBoard)
        };
        match result {
            Ok(message) if message.is_empty() => {}
            Ok(message) => {
                player.send_message(&TextComponent::plain(message).color(Color::Green));
            }
            Err(error) => {
                player.send_message(&TextComponent::plain(error.message()).color(Color::Red));
            }
        }

        let view = self.state.organization_view(player.gameprofile.id());
        self.redraw(guard, view);
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
            button_at(0, 0),
            Some(Button::Offer(Template {
                objective: Objective::Mobs,
                rank: HunterRank::E,
            }))
        );
        assert_eq!(
            button_at(5 * 9 + 4, 0),
            Some(Button::Offer(Template {
                objective: Objective::Dungeons,
                rank: HunterRank::S,
            }))
        );
        assert_eq!(button_at(6, 0), Some(Button::Personal(0)));
        assert_eq!(button_at(2 * 9 + 6, 0), Some(Button::Personal(2)));
        assert_eq!(button_at(3 * 9 + 6, 0), None);
        assert_eq!(button_at(9 + 7, 0), Some(Button::Group(1)));
        assert_eq!(button_at(2 * 9 + 7, 0), None);
        assert_eq!(
            button_at(5, 0),
            Some(Button::Offer(Template {
                objective: Objective::RedGate,
                rank: HunterRank::E,
            }))
        );
        assert_eq!(
            button_at(5, 1),
            Some(Button::Offer(Template {
                objective: Objective::Castle,
                rank: HunterRank::E,
            }))
        );
        assert_eq!(button_at(53, 0), Some(Button::Page));
        assert_eq!(button_at(8, 0), None);
        assert_eq!(button_at(9 + 8, 0), Some(Button::Guild(0)));
        assert_eq!(button_at(2 * 9 + 8, 0), Some(Button::Guild(1)));
        assert_eq!(button_at(3 * 9 + 8, 0), None);
    }
}
