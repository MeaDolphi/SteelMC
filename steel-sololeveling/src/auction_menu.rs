//! The auction house as chest menus: browsing, buying, bidding and selling.
//!
//! Every action is queued as an [`AuctionRequest`], so the serialized
//! persistence lane checks prices and balances exactly as `/auction` does.
//! Selling lists the main-hand stack: the lane reads the hand when it runs,
//! never inside the click that holds the inventory lock.

use std::sync::{Arc, Weak};

use steel_core::inventory::prelude::*;
use steel_registry::vanilla_items;
use text_components::Modifier as _;
use text_components::TextComponent;
use text_components::format::Color;
use uuid::Uuid;

use crate::auction::{
    COMMISSION_PERCENT, LotKind, MAX_ACTIVE_LOTS, MAX_DURATION_HOURS, MIN_DURATION_HOURS,
};
use crate::item::append_lore;
use crate::state::{AuctionRequest, LotView, SoloLevelingState};
use crate::ui::{self, After};

const LOTS_PER_PAGE: usize = 45;
const PREVIOUS_SLOT: usize = 45;
const SELL_SLOT: usize = 47;
const INFO_SLOT: usize = 49;
const MINE_SLOT: usize = 51;
const NEXT_SLOT: usize = 53;
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

fn open_board(player: &Player, state: Arc<SoloLevelingState>, page: usize, mine: bool) {
    let viewer = player.gameprofile.id();
    let mut page = page;
    let items = board_items(&state, viewer, &mut page, mine);
    ui::open_buttons(
        player,
        TextComponent::plain(if mine {
            "Мои лоты"
        } else {
            "Аукцион охотников"
        })
        .color(Color::Gold),
        6,
        items,
        Box::new(move |player, slot, button| {
            board_click(&state, player, &mut page, mine, slot, button)
        }),
    );
}

fn board_items(
    state: &SoloLevelingState,
    viewer: Uuid,
    page: &mut usize,
    mine: bool,
) -> Vec<ItemStack> {
    let lots = visible_lots(state, viewer, mine);
    let pages = lots.len().div_ceil(LOTS_PER_PAGE).max(1);
    *page = (*page).min(pages - 1);
    let mut items = vec![ItemStack::empty(); 54];
    for (slot, lot) in lots
        .iter()
        .skip(*page * LOTS_PER_PAGE)
        .take(LOTS_PER_PAGE)
        .enumerate()
    {
        items[slot] = lot_item(lot, lot.seller == viewer);
    }
    if lots.is_empty() {
        items[22] = ui::button(
            &vanilla_items::BARREL,
            if mine {
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
        "Выставить предмет из руки",
        Color::Green,
        &[
            "Выставляется стак в главной руке.",
            "Личные предметы выставить нельзя.",
            &format!("Комиссия {COMMISSION_PERCENT}%, до {MAX_ACTIVE_LOTS} лотов."),
        ],
    );
    let (_, owed_won, owed_items) = state.auction_personal(viewer);
    items[MINE_SLOT] = ui::button(
        &vanilla_items::CHEST,
        if mine {
            "Все лоты"
        } else {
            "Мои лоты"
        },
        Color::Gold,
        &[&format!(
            "К получению: {owed_won} Won и предметов: {owed_items}."
        )],
    );
    items[INFO_SLOT] = ui::button(
        &vanilla_items::GOLD_INGOT,
        format!("Страница {} / {pages}", *page + 1),
        Color::Gold,
        &[
            "ЛКМ по лоту — купить или минимальная ставка.",
            "ПКМ по чужому лоту — своя ставка.",
            "ПКМ по своему лоту — снять, если ставок нет.",
            "Клик сюда — обновить.",
        ],
    );
    items
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

fn board_click(
    state: &Arc<SoloLevelingState>,
    player: &Player,
    page: &mut usize,
    mine: bool,
    slot: usize,
    button: MouseButton,
) -> After {
    let viewer = player.gameprofile.id();
    match slot {
        PREVIOUS_SLOT => *page = page.saturating_sub(1),
        NEXT_SLOT => *page = page.saturating_add(1),
        INFO_SLOT => {}
        MINE_SLOT => {
            open_board(player, Arc::clone(state), 0, !mine);
            return After::Replaced;
        }
        SELL_SLOT => {
            open_sell(player, Arc::clone(state));
            return After::Replaced;
        }
        lot_slot if lot_slot < LOTS_PER_PAGE => {
            let lots = visible_lots(state, viewer, mine);
            let Some(lot) = lots.get(*page * LOTS_PER_PAGE + lot_slot).cloned() else {
                return After::Keep;
            };
            let Some(handle) = shared(state, player) else {
                return After::Keep;
            };
            let own = lot.seller == viewer;
            match (button == MouseButton::Right, own, lot.kind) {
                (true, true, _) => submit(
                    state,
                    player,
                    AuctionRequest::Cancel {
                        player: handle,
                        lot: lot.id,
                    },
                ),
                (true, false, LotKind::Bidding) => {
                    let minimum = lot.bid.map_or(lot.price, |bid| bid.saturating_add(1));
                    let balance = state
                        .with_session_mut(viewer, |session| session.won.amount())
                        .unwrap_or(0);
                    if balance < minimum {
                        ui::tell(
                            player,
                            &format!("Для ставки нужно не меньше {minimum} Won."),
                            Color::Red,
                        );
                        return After::Keep;
                    }
                    let bid_state = Arc::clone(state);
                    ui::pick_amount(
                        player,
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
                                    player: Weak::clone(&handle),
                                    lot: lot.id,
                                    amount,
                                },
                            );
                            open(player, Arc::clone(&bid_state));
                        }),
                    );
                    return After::Replaced;
                }
                (false, false, LotKind::FixedPrice) => submit(
                    state,
                    player,
                    AuctionRequest::Buy {
                        player: handle,
                        lot: lot.id,
                    },
                ),
                (false, false, LotKind::Bidding) => submit(
                    state,
                    player,
                    AuctionRequest::Bid {
                        player: handle,
                        lot: lot.id,
                        amount: lot.bid.map_or(lot.price, |bid| bid.saturating_add(1)),
                    },
                ),
                _ => return After::Keep,
            }
        }
        _ => return After::Keep,
    }
    After::Redraw(board_items(state, viewer, page, mine))
}

const FIXED_SLOT: usize = 11;
const BIDDING_SLOT: usize = 15;

/// Asks how to sell, then the price and the duration.
fn open_sell(player: &Player, state: Arc<SoloLevelingState>) {
    let mut items = vec![ItemStack::empty(); 27];
    items[4] = ui::button(
        &vanilla_items::PAPER,
        "Продажа предмета из главной руки",
        Color::Gold,
        &["Возьмите предмет в руку перед подтверждением."],
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
            ui::pick_amount(
                player,
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
                    ui::pick_amount(
                        player,
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
        INFO_SLOT, LOTS_PER_PAGE, MINE_SLOT, NEXT_SLOT, PREVIOUS_SLOT, SELL_SLOT, remaining,
    };

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
}
