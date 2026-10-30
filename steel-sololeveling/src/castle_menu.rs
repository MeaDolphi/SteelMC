//! The Demon Castle page of the System: the hunter's climb, the free entry
//! that returns every ten minutes, and the weekly ranking of the highest
//! cleared floor.

use std::sync::Arc;

use steel_core::inventory::prelude::*;
use steel_registry::vanilla_items;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use crate::castle::{self, CHECKPOINT_EVERY, FREE_ENTRY_SECONDS};
use crate::item::{named, with_lore};
use crate::merchant;
use crate::state::SoloLevelingState;
use crate::ui::{After, open_guarded_buttons};

const PROGRESS_SLOT: usize = 11;
const ENTER_SLOT: usize = 13;
const RANKING_SLOT: usize = 15;

/// Opens the castle page.
pub fn open(player: &Player, state: Arc<SoloLevelingState>) {
    let uuid = player.gameprofile.id();
    let Some((progress, ranking)) = state.castle_view(uuid) else {
        return;
    };
    let wait = progress.free_entry_in(merchant::unix_now());
    let free = wait == 0;
    let mut items = vec![ItemStack::empty(); 27];
    items[PROGRESS_SLOT] = with_lore(
        named(
            ItemStack::new(&vanilla_items::NETHER_BRICK),
            TextComponent::plain("Ваше восхождение").color(Color::DarkRed),
        ),
        vec![
            line(&format!(
                "Следующий этаж: {} (ранг {})",
                progress.next_floor,
                castle::rank_for_floor(progress.next_floor).name()
            )),
            line(&format!("Рубеж: этаж {}", progress.checkpoint)),
            line(&format!("Лучший этаж: {}", progress.best)),
            line(&format!(
                "Рубеж сохраняется каждые {CHECKPOINT_EVERY} этажей; смерть возвращает к нему."
            )),
        ],
    );
    items[ENTER_SLOT] = with_lore(
        named(
            ItemStack::new(if free {
                &vanilla_items::OMINOUS_TRIAL_KEY
            } else {
                &vanilla_items::BARRIER
            }),
            TextComponent::plain(if free {
                "Войти бесплатно".to_owned()
            } else {
                format!("Бесплатный вход через {} мин {:02} с", wait / 60, wait % 60)
            })
            .color(if free { Color::Green } else { Color::Red }),
        ),
        vec![
            line(&format!(
                "Бесплатный вход раз в {} минут.",
                FREE_ENTRY_SECONDS / 60
            )),
            line("Дальше — «Ключ Замка демонов» (ПКМ ключом)."),
            line("Врата этажа откроются рядом с вами в главном мире."),
        ],
    );
    let mut lines: Vec<TextComponent> = ranking
        .iter()
        .enumerate()
        .map(|(place, entry)| {
            line(&format!(
                "{}. {} — этаж {}",
                place + 1,
                entry.name,
                entry.floor
            ))
        })
        .collect();
    if lines.is_empty() {
        lines.push(line("На этой неделе ещё никто не поднимался."));
    }
    items[RANKING_SLOT] = with_lore(
        named(
            ItemStack::new(&vanilla_items::GOLDEN_HELMET),
            TextComponent::plain("Рейтинг недели").color(Color::Gold),
        ),
        lines,
    );
    let state_for_click = Arc::clone(&state);
    open_guarded_buttons(
        player,
        TextComponent::plain("Замок демонов").color(Color::DarkRed),
        3,
        items,
        Box::new(move |player, slot, _| {
            if slot != ENTER_SLOT {
                return After::Keep;
            }
            match state_for_click.request_castle_floor(player.gameprofile.id(), false) {
                Ok(floor) => player.send_message(
                    &TextComponent::plain(format!(
                        "Замок демонов: врата на этаж {floor} открываются рядом с вами…"
                    ))
                    .color(Color::DarkRed),
                ),
                Err(message) => {
                    player.send_message(&TextComponent::plain(message).color(Color::Red));
                }
            }
            After::Keep
        }),
    );
}

fn line(text: &str) -> TextComponent {
    TextComponent::plain(text.to_owned()).color(Color::Gray)
}
