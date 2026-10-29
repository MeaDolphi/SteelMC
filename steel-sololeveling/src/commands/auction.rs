//! `/auction`: the player-facing front of the auction house.
//!
//! Commands only parse arguments and queue work: every rule, price, and
//! balance check runs on the serialized persistence lane, so a double click
//! cannot spend the same money twice.

use std::sync::Arc;

use steel_core::command::{
    CommandArgument, CommandContext, CommandError, CommandRegistration, CommandRegistrationError,
    CommandRegistry, argument, literal,
};
use steel_core::player::Player;
use text_components::TextComponent;

use super::SoloLevelingState;
use crate::auction::{LotKind, MAX_ACTIVE_LOTS, MAX_DURATION_HOURS, MIN_DURATION_HOURS};
use crate::identifier::from_sololeveling;
use crate::state::{AuctionRequest, LotView};

/// How many board lines one command prints before summarizing.
const BOARD_LINES: usize = 20;

pub(super) fn register(
    registry: &mut CommandRegistry,
    state: Arc<SoloLevelingState>,
) -> Result<(), CommandRegistrationError> {
    let root = CommandRegistration::new(from_sololeveling("auction"), move || {
        let state = Arc::clone(&state);
        literal("auction")
            .then({
                let state = Arc::clone(&state);
                literal("list").executes(move |context| {
                    let player = player_of(*context)?;
                    let lots = state.auction_lots();
                    player.send_message(&TextComponent::plain(board(&lots)));
                    Ok(1)
                })
            })
            .then({
                let state = Arc::clone(&state);
                literal("sell").then(
                    argument("price", CommandArgument::word()).then(
                        argument("hours", CommandArgument::word())
                            .executes(move |context| place(&state, *context, LotKind::Bidding)),
                    ),
                )
            })
            .then({
                let state = Arc::clone(&state);
                literal("fixed").then(
                    argument("price", CommandArgument::word()).then(
                        argument("hours", CommandArgument::word())
                            .executes(move |context| place(&state, *context, LotKind::FixedPrice)),
                    ),
                )
            })
            .then({
                let state = Arc::clone(&state);
                literal("buy").then(argument("lot", CommandArgument::word()).executes(
                    move |context| {
                        let player = player_of(*context)?;
                        let lot = number(*context, "lot")?;
                        state.request_auction(AuctionRequest::Buy {
                            player: Arc::downgrade(&player),
                            lot,
                        });
                        Ok(1)
                    },
                ))
            })
            .then({
                let state = Arc::clone(&state);
                literal("bid").then(argument("lot", CommandArgument::word()).then(
                    argument("amount", CommandArgument::word()).executes(move |context| {
                        let player = player_of(*context)?;
                        let lot = number(*context, "lot")?;
                        let amount = number(*context, "amount")?;
                        state.request_auction(AuctionRequest::Bid {
                            player: Arc::downgrade(&player),
                            lot,
                            amount,
                        });
                        Ok(1)
                    }),
                ))
            })
            .then({
                let state = Arc::clone(&state);
                literal("cancel").then(argument("lot", CommandArgument::word()).executes(
                    move |context| {
                        let player = player_of(*context)?;
                        let lot = number(*context, "lot")?;
                        state.request_auction(AuctionRequest::Cancel {
                            player: Arc::downgrade(&player),
                            lot,
                        });
                        Ok(1)
                    },
                ))
            })
            .then({
                let state = Arc::clone(&state);
                literal("mine").executes(move |context| {
                    let player = player_of(*context)?;
                    let (lots, won, items) = state.auction_personal(player.gameprofile.id());
                    player.send_message(&TextComponent::plain(personal(&lots, won, items)));
                    Ok(1)
                })
            })
    })
    .default_access();
    registry.register(root)?;
    Ok(())
}

fn player_of(context: CommandContext<'_>) -> Result<Arc<Player>, CommandError> {
    context
        .source()
        .player()
        .cloned()
        .ok_or_else(|| CommandError::from("Команда доступна только игроку."))
}

fn number(context: CommandContext<'_>, name: &str) -> Result<u64, CommandError> {
    context
        .string(name)?
        .parse::<u64>()
        .map_err(|_| CommandError::from("Ожидается целое число."))
}

fn place(
    state: &Arc<SoloLevelingState>,
    context: CommandContext<'_>,
    kind: LotKind,
) -> Result<i32, CommandError> {
    let player = player_of(context)?;
    let price = number(context, "price")?;
    let hours = number(context, "hours")?;
    if price == 0 {
        return Err(CommandError::from("Цена — целое число Won не меньше 1."));
    }
    if !(MIN_DURATION_HOURS..=MAX_DURATION_HOURS).contains(&hours) {
        return Err(CommandError::from("Срок лота — от 1 до 24 часов."));
    }
    state.request_auction(AuctionRequest::List {
        player: Arc::downgrade(&player),
        kind,
        price,
        hours,
    });
    Ok(1)
}

fn board(lots: &[LotView]) -> String {
    if lots.is_empty() {
        return "Аукцион пуст.".to_owned();
    }
    let mut lines = lots
        .iter()
        .take(BOARD_LINES)
        .map(lot_line)
        .collect::<Vec<_>>();
    if lots.len() > BOARD_LINES {
        lines.push(format!("…и ещё {} лотов.", lots.len() - BOARD_LINES));
    }
    lines.join("\n")
}

fn personal(lots: &[LotView], won: u64, items: usize) -> String {
    let mut lines = Vec::with_capacity(lots.len() + 2);
    lines.push(format!("Ваши лоты: {} / {}", lots.len(), MAX_ACTIVE_LOTS));
    lines.extend(lots.iter().map(lot_line));
    lines.push(format!(
        "К получению: {won} Won и предметов: {items}. Выдача — при входе или в течение секунды."
    ));
    lines.join("\n")
}

fn lot_line(view: &LotView) -> String {
    let terms = match view.kind {
        LotKind::FixedPrice => format!("фикс. цена {} Won", view.price),
        LotKind::Bidding => match view.bid {
            Some(bid) => format!("ставка {bid} Won, минимум {}", bid.saturating_add(1)),
            None => format!("торги от {} Won", view.price),
        },
    };
    format!(
        "#{} {} x{} — {} — {} — осталось {}",
        view.id,
        view.item_name,
        view.count,
        view.seller_name,
        terms,
        remaining(view.seconds_left)
    )
}

fn remaining(seconds: u64) -> String {
    let hours = seconds / 3_600;
    let minutes = seconds % 3_600 / 60;
    if hours > 0 {
        format!("{hours} ч {minutes:02} мин")
    } else {
        format!("{} мин", minutes.max(1))
    }
}
