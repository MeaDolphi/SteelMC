//! The Adventurer Organization board menu.
//!
//! The left five columns offer every template by rank and objective. Column
//! six lists the hunter's personal contracts, column seven the party's group
//! contracts and column eight, below the help book, the guild's contracts.
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
use crate::main_menu;
use crate::organization::{
    Contract, GROUP_LIMIT, GUILD_LIMIT, GUILD_SCALE, OBJECTIVES, Objective, OrganizationError,
    PERSONAL_LIMIT, RANKS, Template,
};
use crate::starter_menu;
use crate::state::{BoardView, SoloLevelingState};
use crate::ui;

const COLUMNS: usize = 9;
const SLOTS: usize = 54;
const PERSONAL_COLUMN: usize = 5;
const GROUP_COLUMN: usize = 6;
const GUILD_COLUMN: usize = 7;
const HELP_SLOT: usize = 8;
const BACK_SLOT: usize = 53;
/// Objective columns shown on the board.
const OFFER_COLUMNS: usize = 5;
/// Offer slots of the board: five columns of six rows.
const OFFER_SLOTS: usize = 30;
/// Ticks between board offer rerolls: twenty minutes, and every hunter sees
/// the same board of one bucket.
pub const OFFER_REFRESH_TICKS: u64 = 20 * 60 * 20;

/// Returns the offer bucket of `tick`: the board rerolls when the bucket changes.
#[must_use]
pub const fn offer_bucket(tick: u64) -> u64 {
    tick / OFFER_REFRESH_TICKS
}

/// Returns the offers of `bucket`: a shuffled mix of every objective and rank.
fn rolled_offers(bucket: u64) -> Vec<Template> {
    let mut offers: Vec<Template> = OBJECTIVES
        .into_iter()
        .flat_map(|objective| RANKS.map(|rank| Template { objective, rank }))
        .collect();
    // Seeded Fisher-Yates: one bucket, one board, no player-specific rolls.
    let mut rng = bucket.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    for index in (1..offers.len()).rev() {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let other = usize::try_from(rng % (index as u64 + 1)).unwrap_or(0);
        offers.swap(index, other);
    }
    offers.truncate(OFFER_SLOTS);
    offers
}

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
    let bucket = offer_bucket(state.current_tick());
    let offers = rolled_offers(bucket);
    let view = state.organization_view(player.gameprofile.id());
    let container =
        SimpleContainer::from_items(menu_items(view.as_ref(), at_board, &offers)).into_shared();
    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X6, container_id);
    let buttons = builder.section_all(Arc::clone(&container));
    builder.player_inventory(&player.inventory);
    builder.build(OrganizationMenuKind {
        state,
        buttons,
        container,
        at_board,
        bucket,
        offers,
        view,
    })
}

/// A board slot and what clicking it means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Button {
    Offer(Template),
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

fn button_at(slot: usize, offers: &[Template]) -> Option<Button> {
    let (row, column) = (slot / COLUMNS, slot % COLUMNS);
    if column < OFFER_COLUMNS {
        return offers
            .get(row * OFFER_COLUMNS + column)
            .map(|template| Button::Offer(*template));
    }
    match column {
        PERSONAL_COLUMN if row < PERSONAL_LIMIT => Some(Button::Personal(row)),
        GROUP_COLUMN if row < GROUP_LIMIT => Some(Button::Group(row)),
        GUILD_COLUMN if (1..=GUILD_LIMIT).contains(&row) => Some(Button::Guild(row - 1)),
        _ => None,
    }
}

fn menu_items(view: Option<&BoardView>, at_board: bool, offers: &[Template]) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); SLOTS];
    let Some(view) = view else {
        return items;
    };
    for (slot, item) in items.iter_mut().enumerate() {
        *item = match button_at(slot, offers) {
            Some(Button::Offer(_)) if !at_board => ItemStack::empty(),
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
            None if slot == BACK_SLOT => ui::main_menu_button(),
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
            TextComponent::plain("Предложения обновляются каждые 20 минут."),
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
    /// Offer bucket the board was rolled from.
    bucket: u64,
    /// Offers of the current bucket, in board order.
    offers: Vec<Template>,
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
            for (index, item) in menu_items(view.as_ref(), self.at_board, &self.offers)
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
        let bucket = offer_bucket(self.state.current_tick());
        let rerolled = bucket != self.bucket;
        if rerolled {
            self.bucket = bucket;
            self.offers = rolled_offers(bucket);
        }
        let view = self.state.organization_view(player.gameprofile.id());
        if rerolled || self.view != view {
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
        if slot - self.buttons.start() == BACK_SLOT {
            main_menu::open(player, Arc::clone(&self.state));
            return ClickOutcome::Consume;
        }
        if self.at_board && slot - self.buttons.start() == HELP_SLOT {
            starter_menu::open(player, Arc::clone(&self.state));
            return ClickOutcome::Consume;
        }
        let Some(target) = button_at(slot - self.buttons.start(), &self.offers) else {
            return ClickOutcome::Consume;
        };
        let result = if self.at_board {
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
    use super::{Button, OFFER_SLOTS, button_at, offer_bucket, rolled_offers};
    use crate::organization::{OBJECTIVES, RANKS, Template};

    #[test]
    fn offers_are_deterministic_per_bucket_and_reroll_between_buckets() {
        let first = rolled_offers(7);
        assert_eq!(first, rolled_offers(7), "one bucket, one board");
        assert_ne!(first, rolled_offers(8), "the next bucket rerolls");
        assert_eq!(first.len(), OFFER_SLOTS);
    }

    #[test]
    fn offers_are_a_mixed_subset_without_duplicates() {
        for bucket in 0..10 {
            let offers = rolled_offers(bucket);
            let mut unique = offers.clone();
            unique.sort_by_key(|template| (template.objective as u8, template.rank as u8));
            unique.dedup();
            assert_eq!(
                unique.len(),
                OFFER_SLOTS,
                "bucket {bucket} repeats an offer"
            );
            assert!(
                offers
                    .iter()
                    .any(|template| template.objective != offers[0].objective),
                "bucket {bucket} must mix objectives, not page them"
            );
        }
        let total = OBJECTIVES.len() * RANKS.len();
        assert!(
            OFFER_SLOTS < total,
            "the board must be a subset of all offers"
        );
    }

    #[test]
    fn slots_map_to_offers_and_contract_columns() {
        let offers = rolled_offers(3);
        assert_eq!(button_at(0, &offers), Some(Button::Offer(offers[0])));
        assert_eq!(
            button_at(5 * 9 + 4, &offers),
            Some(Button::Offer(offers[29]))
        );
        assert_eq!(button_at(5, &offers), Some(Button::Personal(0)));
        assert_eq!(button_at(2 * 9 + 5, &offers), Some(Button::Personal(2)));
        assert_eq!(button_at(3 * 9 + 5, &offers), None);
        assert_eq!(button_at(9 + 6, &offers), Some(Button::Group(1)));
        assert_eq!(button_at(2 * 9 + 6, &offers), None);
        assert_eq!(button_at(8, &offers), None);
        assert_eq!(button_at(9 + 7, &offers), Some(Button::Guild(0)));
        assert_eq!(button_at(2 * 9 + 7, &offers), Some(Button::Guild(1)));
        assert_eq!(button_at(3 * 9 + 7, &offers), None);
    }

    #[test]
    fn the_bucket_changes_every_twenty_minutes() {
        assert_eq!(
            offer_bucket(0),
            offer_bucket(super::OFFER_REFRESH_TICKS - 1)
        );
        assert_ne!(
            offer_bucket(super::OFFER_REFRESH_TICKS - 1),
            offer_bucket(super::OFFER_REFRESH_TICKS)
        );
    }
}
