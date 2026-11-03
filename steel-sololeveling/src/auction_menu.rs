//! The auction house as chest menus: browsing, buying, bidding and selling.
//!
//! Every action is queued as an [`AuctionRequest`], so the serialized
//! persistence lane checks prices and balances exactly as `/auction` does.
//! Selling starts with a picker of inventory stacks, opened on the next player
//! tick because a click holds the inventory lock; the lane checks that the
//! chosen slot still holds the same stack when it runs.

use std::sync::{Arc, Weak};

use steel_core::inventory::container::Container as _;
use steel_core::inventory::prelude::*;
use steel_core::player::player_inventory::PlayerInventory;
use steel_registry::vanilla_items;
use steel_registry::vanilla_menu_types;
use steel_utils::locks::IntoShared;
use steel_utils::{DowncastType, DowncastTypeKey};
use text_components::Modifier as _;
use text_components::TextComponent;
use text_components::format::Color;
use uuid::Uuid;

use crate::auction::{
    COMMISSION_PERCENT, LotKind, MAX_ACTIVE_LOTS, MAX_DURATION_HOURS, MIN_DURATION_HOURS,
};
use crate::item::{append_lore, binding};
use crate::main_menu;
use crate::state::{AuctionRequest, LotView, SoloLevelingState};
use crate::ui::{self, After};

const LOTS_PER_PAGE: usize = 45;
const PREVIOUS_SLOT: usize = 45;
const SELL_SLOT: usize = 47;
const INFO_SLOT: usize = 49;
const MINE_SLOT: usize = 51;
const NEXT_SLOT: usize = 53;
const BACK_SLOT: usize = 52;
/// Highest price the menu offers; larger prices stay possible by command.
const MAX_MENU_PRICE: u64 = 10_000_000;

/// Opens the auction board.
pub fn open(player: &Player, state: Arc<SoloLevelingState>) {
    open_board(player, state, 0, false);
}

fn visible_lots(state: &SoloLevelingState, viewer: Uuid, mine: bool) -> Vec<LotView> {
    if mine {
        state.auction_personal(viewer).0
    } else {
        state.auction_lots()
    }
}

/// One lot as the board renders it. Equality ignores sub-minute drift of the
/// remaining time, since the board only shows whole minutes.
#[derive(Debug, Clone)]
struct Row(LotView);

impl PartialEq for Row {
    fn eq(&self, other: &Self) -> bool {
        let (a, b) = (&self.0, &other.0);
        a.id == b.id
            && a.seller == b.seller
            && a.seller_name == b.seller_name
            && a.item_name == b.item_name
            && a.count == b.count
            && a.kind == b.kind
            && a.price == b.price
            && a.bid == b.bid
            && a.item == b.item
            && a.seconds_left / 60 == b.seconds_left / 60
    }
}

impl Eq for Row {}

/// Everything the board shows; a change redraws it.
#[derive(Debug, Clone, PartialEq)]
struct View {
    viewer: Uuid,
    lots: Vec<Row>,
    owed_won: u64,
    owed_items: usize,
    mine: bool,
    page: usize,
}

fn view(state: &SoloLevelingState, viewer: Uuid, page: usize, mine: bool) -> Option<View> {
    let lots = visible_lots(state, viewer, mine);
    let (_, owed_won, owed_items) = state.auction_personal(viewer);
    let pages = lots.len().div_ceil(LOTS_PER_PAGE).max(1);
    Some(View {
        viewer,
        lots: lots.into_iter().map(Row).collect(),
        owed_won,
        owed_items,
        mine,
        page: page.min(pages - 1),
    })
}

fn open_board(player: &Player, state: Arc<SoloLevelingState>, page: usize, mine: bool) {
    let viewer = player.gameprofile.id();
    let view = view(&state, viewer, page, mine);
    let container = SimpleContainer::from_items(menu_items(view.as_ref())).into_shared();
    player.open_menu(
        TextComponent::plain(if mine {
            "Мои лоты"
        } else {
            "Аукцион охотников"
        })
        .color(Color::Gold),
        move |context| {
            let mut builder =
                MenuBuilder::new(&vanilla_menu_types::GENERIC_9X6, context.container_id);
            let buttons = builder.section_all(Arc::clone(&container));
            builder.player_inventory(&context.player.inventory);
            builder.build(AuctionMenuKind {
                state,
                buttons,
                container,
                view,
                page,
                mine,
            })
        },
    );
}

fn menu_items(view: Option<&View>) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); 54];
    let Some(view) = view else {
        return items;
    };
    let pages = view.lots.len().div_ceil(LOTS_PER_PAGE).max(1);
    for (slot, row) in view
        .lots
        .iter()
        .skip(view.page * LOTS_PER_PAGE)
        .take(LOTS_PER_PAGE)
        .enumerate()
    {
        let lot = &row.0;
        items[slot] = lot_item(lot, lot.seller == view.viewer);
    }
    if view.lots.is_empty() {
        items[22] = ui::button(
            &vanilla_items::BARREL,
            if view.mine {
                "У вас нет лотов"
            } else {
                "Лотов пока нет"
            },
            Color::Gray,
            &[],
        );
    }
    items[PREVIOUS_SLOT] = ui::previous_page();
    items[NEXT_SLOT] = ui::next_page();
    items[SELL_SLOT] = ui::button(
        &vanilla_items::EMERALD,
        "Выставить предмет",
        Color::Green,
        &[
            "Выберите стак из инвентаря.",
            "Личные предметы выставить нельзя.",
            &format!("Комиссия {COMMISSION_PERCENT}%, до {MAX_ACTIVE_LOTS} лотов."),
        ],
    );
    items[MINE_SLOT] = ui::button(
        &vanilla_items::CHEST,
        if view.mine {
            "Все лоты"
        } else {
            "Мои лоты"
        },
        Color::Gold,
        &[&format!(
            "К получению: {} Won и предметов: {}.",
            view.owed_won, view.owed_items
        )],
    );
    items[INFO_SLOT] = ui::button(
        &vanilla_items::GOLD_INGOT,
        format!("Страница {} / {pages}", view.page + 1),
        Color::Gold,
        &[
            "ЛКМ по лоту — купить или минимальная ставка.",
            "ПКМ по чужому лоту — своя ставка.",
            "ПКМ по своему лоту — снять, если ставок нет.",
            "Клик сюда — обновить.",
        ],
    );
    items[BACK_SLOT] = ui::main_menu_button();
    items
}

/// The auction board as a chest menu: clicks queue [`AuctionRequest`]s and
/// every tick the board diff-redraws, so a finished buy or bid never leaves
/// stale lots on display.
pub(crate) struct AuctionMenuKind {
    state: Arc<SoloLevelingState>,
    buttons: Section,
    container: Shared<SimpleContainer>,
    view: Option<View>,
    page: usize,
    mine: bool,
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for AuctionMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/auction");
}

impl AuctionMenuKind {
    /// Recomputes the view and redraws when it changed; the page is adopted
    /// clamped, so lots vanishing under it cannot strand the board.
    fn refresh(&mut self, guard: &mut ContainerLockGuard, player: &Player) {
        let view = view(&self.state, player.gameprofile.id(), self.page, self.mine);
        if let Some(view) = &view {
            self.page = view.page;
        }
        let container_id = ContainerId::from_arc(&self.container);
        if let Some(container) = guard.get_typed_mut::<SimpleContainer>(container_id) {
            for (index, item) in menu_items(view.as_ref()).into_iter().enumerate() {
                container.set_item(index, item);
            }
        }
        self.view = view;
    }
}

impl MenuKind for AuctionMenuKind {
    fn on_tick(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        player: &Player,
    ) {
        if player.tick_count() % 20 != 0 {
            return;
        }
        let view = view(&self.state, player.gameprofile.id(), self.page, self.mine);
        if self.view == view {
            return;
        }
        let container_id = ContainerId::from_arc(&self.container);
        if let Some(container) = guard.get_typed_mut::<SimpleContainer>(container_id) {
            for (index, item) in menu_items(view.as_ref()).into_iter().enumerate() {
                container.set_item(index, item);
            }
        }
        if let Some(view) = &view {
            self.page = view.page;
        }
        self.view = view;
    }

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
        let local = slot - self.buttons.start();
        let viewer = player.gameprofile.id();
        match local {
            BACK_SLOT => {
                main_menu::open(player, Arc::clone(&self.state));
                return ClickOutcome::Consume;
            }
            PREVIOUS_SLOT => self.page = self.page.saturating_sub(1),
            NEXT_SLOT => self.page = self.page.saturating_add(1),
            INFO_SLOT => {}
            MINE_SLOT => {
                self.mine = !self.mine;
                self.page = 0;
            }
            // The picker reads the inventory, which this click has locked.
            SELL_SLOT => {
                self.state.defer_auction_sell_menu(viewer);
                return ClickOutcome::Consume;
            }
            lot_slot if lot_slot < LOTS_PER_PAGE => {
                let Some(view) = self.view.as_ref() else {
                    return ClickOutcome::Consume;
                };
                let Some(row) = view.lots.get(view.page * LOTS_PER_PAGE + lot_slot) else {
                    return ClickOutcome::Consume;
                };
                let lot = row.0.clone();
                let Some(handle) = shared(&self.state, player) else {
                    return ClickOutcome::Consume;
                };
                let own = lot.seller == viewer;
                match (button == MouseButton::Right, own, lot.kind) {
                    (true, true, _) => submit(
                        &self.state,
                        player,
                        AuctionRequest::Cancel {
                            player: handle,
                            lot: lot.id,
                        },
                    ),
                    (true, false, LotKind::Bidding) => {
                        let minimum = lot.bid.map_or(lot.price, |bid| bid.saturating_add(1));
                        let balance = self
                            .state
                            .with_session_mut(viewer, |session| session.won.amount())
                            .unwrap_or(0);
                        if balance < minimum {
                            ui::tell(
                                player,
                                &format!("Для ставки нужно не меньше {minimum} Won."),
                                Color::Red,
                            );
                            return ClickOutcome::Consume;
                        }
                        let bid_state = Arc::clone(&self.state);
                        let bid_handle = Weak::clone(&handle);
                        ui::pick_amount(
                            player,
                            Arc::clone(&bid_state),
                            &format!("Ставка на лот #{}", lot.id),
                            "Won",
                            minimum,
                            minimum,
                            balance,
                            Arc::new(move |player, amount| {
                                submit(
                                    &bid_state,
                                    player,
                                    AuctionRequest::Bid {
                                        player: Weak::clone(&bid_handle),
                                        lot: lot.id,
                                        amount,
                                    },
                                );
                                open(player, Arc::clone(&bid_state));
                            }),
                        );
                        return ClickOutcome::Consume;
                    }
                    (false, false, LotKind::FixedPrice) => submit(
                        &self.state,
                        player,
                        AuctionRequest::Buy {
                            player: handle,
                            lot: lot.id,
                        },
                    ),
                    (false, false, LotKind::Bidding) => submit(
                        &self.state,
                        player,
                        AuctionRequest::Bid {
                            player: handle,
                            lot: lot.id,
                            amount: lot.bid.map_or(lot.price, |bid| bid.saturating_add(1)),
                        },
                    ),
                    _ => return ClickOutcome::Consume,
                }
            }
            _ => return ClickOutcome::Consume,
        }
        self.refresh(guard, player);
        ClickOutcome::Consume
    }
}

fn lot_item(lot: &LotView, own: bool) -> ItemStack {
    let stack = lot
        .item
        .clone()
        .unwrap_or_else(|| ItemStack::new(&vanilla_items::BARRIER));
    let terms = match lot.kind {
        LotKind::FixedPrice => format!("Цена: {} Won", lot.price),
        LotKind::Bidding => match lot.bid {
            Some(bid) => format!("Ставка: {bid} Won, следующая от {}", bid.saturating_add(1)),
            None => format!("Торги от {} Won", lot.price),
        },
    };
    let hint = match (own, lot.kind) {
        (true, _) => "ПКМ — снять лот, если на него нет ставок.",
        (false, LotKind::FixedPrice) => "ЛКМ — купить.",
        (false, LotKind::Bidding) => "ЛКМ — минимальная ставка, ПКМ — своя ставка.",
    };
    append_lore(
        stack,
        vec![
            TextComponent::plain(format!("Лот #{} · ×{}", lot.id, lot.count)).color(Color::Gold),
            TextComponent::plain(format!("Продавец: {}", lot.seller_name)).color(Color::Gray),
            TextComponent::plain(terms).color(Color::Yellow),
            TextComponent::plain(format!("Осталось: {}", remaining(lot.seconds_left)))
                .color(Color::Gray),
            TextComponent::plain(hint).color(Color::Green),
        ],
    )
}

fn remaining(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = seconds % 3600 / 60;
    if hours > 0 {
        format!("{hours} ч {minutes} мин")
    } else {
        format!("{minutes} мин")
    }
}

/// Returns the shared handle the auction lane needs for a hunter.
fn shared(state: &SoloLevelingState, player: &Player) -> Option<Weak<Player>> {
    state
        .online_players()
        .into_iter()
        .find(|online| online.gameprofile.id() == player.gameprofile.id())
        .map(|online| Arc::downgrade(&online))
}

fn submit(state: &SoloLevelingState, player: &Player, request: AuctionRequest) {
    state.request_auction(request);
    ui::tell(player, "Запрос отправлен аукциону.", Color::Gold);
}

const FIXED_SLOT: usize = 11;
const BIDDING_SLOT: usize = 15;

/// Opens the picker of inventory stacks that can be listed.
///
/// It reads the inventory, so the caller runs outside any menu callback.
pub fn open_picker(player: &Player, state: Arc<SoloLevelingState>) {
    let choices: Vec<(usize, ItemStack)> = {
        let inventory = player.inventory.lock();
        (0..PlayerInventory::INVENTORY_SIZE)
            .map(|slot| (slot, inventory.get_item(slot).clone()))
            .filter(|(_, stack)| !stack.is_empty() && binding::bound_owner(stack).is_none())
            .collect()
    };
    if choices.is_empty() {
        ui::tell(
            player,
            "В инвентаре нет предметов, которые можно выставить.",
            Color::Red,
        );
        return;
    }
    let items = choices
        .iter()
        .map(|(_, stack)| {
            append_lore(
                stack.clone(),
                vec![TextComponent::plain("Клик — выставить на аукцион.").color(Color::Yellow)],
            )
        })
        .collect();
    ui::open_buttons(
        player,
        Arc::clone(&state),
        TextComponent::plain("Выберите предмет").color(Color::Gold),
        if choices.len() > 27 { 6 } else { 3 },
        items,
        Box::new(move |player, index, _| {
            let Some(chosen) = choices.get(index).cloned() else {
                return After::Keep;
            };
            open_sell(player, Arc::clone(&state), chosen);
            After::Replaced
        }),
    );
}

/// Asks how to sell the chosen stack, then the price and the duration.
fn open_sell(player: &Player, state: Arc<SoloLevelingState>, chosen: (usize, ItemStack)) {
    let mut items = vec![ItemStack::empty(); 27];
    items[4] = append_lore(
        chosen.1.clone(),
        vec![
            TextComponent::plain("Этот предмет будет выставлен.").color(Color::Gold),
            TextComponent::plain("Не перемещайте его до подтверждения.").color(Color::Gray),
        ],
    );
    items[FIXED_SLOT] = ui::button(
        &vanilla_items::GOLD_NUGGET,
        "Фиксированная цена",
        Color::Yellow,
        &["Первый покупатель забирает лот."],
    );
    items[BIDDING_SLOT] = ui::button(
        &vanilla_items::GOLD_BLOCK,
        "Торги",
        Color::Yellow,
        &["Лот уходит лучшей ставке по истечении срока."],
    );
    ui::open_buttons(
        player,
        Arc::clone(&state),
        TextComponent::plain("Выставить лот").color(Color::Gold),
        3,
        items,
        Box::new(move |player, slot, _| {
            let kind = match slot {
                FIXED_SLOT => LotKind::FixedPrice,
                BIDDING_SLOT => LotKind::Bidding,
                _ => return After::Keep,
            };
            let price_state = Arc::clone(&state);
            let chosen = chosen.clone();
            ui::pick_amount(
                player,
                Arc::clone(&price_state),
                if kind == LotKind::FixedPrice {
                    "Цена лота"
                } else {
                    "Начальная ставка"
                },
                "Won",
                100,
                1,
                MAX_MENU_PRICE,
                Arc::new(move |player, price| {
                    let hours_state = Arc::clone(&price_state);
                    let chosen = chosen.clone();
                    ui::pick_amount(
                        player,
                        Arc::clone(&hours_state),
                        "Срок лота",
                        "ч",
                        12,
                        MIN_DURATION_HOURS,
                        MAX_DURATION_HOURS,
                        Arc::new(move |player, hours| {
                            if let Some(handle) = shared(&hours_state, player) {
                                submit(
                                    &hours_state,
                                    player,
                                    AuctionRequest::List {
                                        player: handle,
                                        chosen: Some(chosen.clone()),
                                        kind,
                                        price,
                                        hours,
                                    },
                                );
                            }
                            open_board(player, Arc::clone(&hours_state), 0, true);
                        }),
                    );
                }),
            );
            After::Replaced
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::{
        INFO_SLOT, LOTS_PER_PAGE, MINE_SLOT, NEXT_SLOT, PREVIOUS_SLOT, Row, SELL_SLOT, View,
        menu_items, remaining,
    };
    use crate::auction::LotKind;
    use crate::state::LotView;
    use steel_registry::init_vanilla_registry;
    use uuid::Uuid;

    fn lot_view(id: u64, bid: Option<u64>, seconds_left: u64) -> LotView {
        LotView {
            id,
            seller: Uuid::from_u128(id as u128),
            seller_name: "seller".to_owned(),
            item_name: "item".to_owned(),
            item: None,
            count: 1,
            kind: LotKind::Bidding,
            price: 100,
            bid,
            seconds_left,
        }
    }

    fn view(lots: Vec<Row>, page: usize) -> View {
        View {
            viewer: Uuid::new_v4(),
            lots,
            owed_won: 0,
            owed_items: 0,
            mine: false,
            page,
        }
    }

    #[test]
    fn board_buttons_sit_below_the_lots() {
        for slot in [PREVIOUS_SLOT, SELL_SLOT, INFO_SLOT, MINE_SLOT, NEXT_SLOT] {
            assert!((LOTS_PER_PAGE..54).contains(&slot));
        }
    }

    #[test]
    fn remaining_time_reads_in_hours_and_minutes() {
        assert_eq!(remaining(3 * 3600 + 120), "3 ч 2 мин");
        assert_eq!(remaining(59), "0 мин");
    }

    #[test]
    fn rows_ignore_sub_minute_expiry_drift_but_not_a_new_bid() {
        let listed = Row(lot_view(7, None, 595));
        assert_eq!(listed, Row(lot_view(7, None, 580)), "drift under a minute");
        assert_ne!(
            listed,
            Row(lot_view(7, None, 539)),
            "a crossed minute changes the board"
        );
        assert_ne!(listed, Row(lot_view(7, Some(150), 600)), "a new bid");
        assert_ne!(listed, Row(lot_view(8, None, 600)), "another lot");
    }

    #[test]
    fn the_board_renders_from_its_view() {
        init_vanilla_registry();
        let empty = menu_items(Some(&view(Vec::new(), 0)));
        assert!(!empty[22].is_empty(), "an empty board explains itself");

        let lots: Vec<Row> = (0..2).map(|id| Row(lot_view(id, None, 3600))).collect();
        let items = menu_items(Some(&view(lots, 0)));
        assert!(!items[0].is_empty());
        assert!(!items[1].is_empty());
        let info = format!("{:?}", items[INFO_SLOT]);
        assert!(info.contains("Страница 1 / 1"), "{info}");
    }
}
