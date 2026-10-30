//! System errands, weekly trials and the Hunter's Path as chest menus.
//!
//! Both pages only show progress: errands and trials count by themselves
//! and pay the moment they finish.

use std::sync::Arc;

use steel_core::inventory::prelude::*;
use steel_registry::vanilla_items;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use crate::item::{named, with_lore};
use crate::quest_book::{self, Errand, PATH, errand_experience, errand_points, trial_won};
use crate::state::SoloLevelingState;
use crate::ui::{self, After};

/// Slots of the three errands of the day.
const ERRAND_SLOTS: [usize; 3] = [10, 12, 14];
/// Slots of the three trials of the week.
const TRIAL_SLOTS: [usize; 3] = [19, 21, 23];

fn line(text: impl Into<String>, color: Color) -> TextComponent {
    TextComponent::plain(text.into()).color(color)
}

fn errand_item(errand: Errand, reward: String) -> ItemStack {
    let done = errand.done();
    with_lore(
        named(
            ItemStack::new(if done {
                &vanilla_items::LIME_DYE
            } else {
                &vanilla_items::PAPER
            }),
            line(
                quest_book::errand_title(errand.goal, errand.target),
                if done { Color::Green } else { Color::Yellow },
            ),
        ),
        vec![
            line(
                format!("Прогресс: {} / {}", errand.progress, errand.target),
                Color::Aqua,
            ),
            line(reward, Color::Gold),
            line(
                if done {
                    "Выполнено."
                } else {
                    "Засчитывается само."
                },
                Color::Gray,
            ),
        ],
    )
}

/// Opens today's errands and this week's trials.
pub fn open_errands(player: &Player, state: &Arc<SoloLevelingState>) {
    let uuid = player.gameprofile.id();
    state.roll_quests(uuid);
    let Some((book, rank)) = state.with_session_mut(uuid, |session| {
        (session.quests.clone(), session.stats.rank())
    }) else {
        return;
    };
    let mut items = vec![ItemStack::empty(); 27];
    items[4] = with_lore(
        named(
            ItemStack::new(&vanilla_items::WRITABLE_BOOK),
            line("Поручения Системы", Color::Aqua),
        ),
        vec![
            line(
                "Три поручения в день и три испытания в неделю.",
                Color::Gray,
            ),
            line("Все поручения дня — Сундук Системы.", Color::Gray),
            line("Все испытания недели — Священный сундук.", Color::Gray),
            line(
                if book.errand_chest {
                    "Сундук дня уже получен."
                } else {
                    "Сундук дня ещё впереди."
                },
                Color::Yellow,
            ),
        ],
    );
    items[9] = named(
        ItemStack::new(&vanilla_items::SUNFLOWER),
        line("Сегодня", Color::Gold),
    );
    items[18] = named(
        ItemStack::new(&vanilla_items::CLOCK),
        line("Эта неделя", Color::Gold),
    );
    for (slot, errand) in ERRAND_SLOTS.into_iter().zip(&book.errands) {
        items[slot] = errand_item(
            *errand,
            format!(
                "Награда: {} очков Системы, {} опыта",
                errand_points(rank),
                errand_experience(rank)
            ),
        );
    }
    for (slot, trial) in TRIAL_SLOTS.into_iter().zip(&book.trials) {
        items[slot] = errand_item(*trial, format!("Награда: {} Won", trial_won(rank)));
    }
    ui::open_buttons(
        player,
        line("Поручения Системы", Color::Aqua),
        3,
        items,
        Box::new(|_, _, _| After::Keep),
    );
}

/// Opens the Hunter's Path.
pub fn open_path(player: &Player, state: &Arc<SoloLevelingState>) {
    let uuid = player.gameprofile.id();
    state.refresh_path(uuid);
    let Some(done) = state.with_session_mut(uuid, |session| session.quests.achievements.clone())
    else {
        return;
    };
    let mut items = vec![ItemStack::empty(); 54];
    items[4] = with_lore(
        named(
            ItemStack::new(&vanilla_items::FILLED_MAP),
            line("Путь охотника", Color::Gold),
        ),
        vec![
            line(
                format!("Пройдено: {} / {}", done.len(), PATH.len()),
                Color::Aqua,
            ),
            line("Каждый шаг платит Won и очки Системы.", Color::Gray),
        ],
    );
    for (index, step) in PATH.into_iter().enumerate() {
        let finished = done.contains(&step);
        let (won, points) = step.reward();
        items[9 + index] = with_lore(
            named(
                ItemStack::new(if finished {
                    &vanilla_items::LIME_DYE
                } else {
                    &vanilla_items::GRAY_DYE
                }),
                line(
                    step.title(),
                    if finished {
                        Color::Green
                    } else {
                        Color::Yellow
                    },
                ),
            ),
            vec![
                line(step.description(), Color::Gray),
                line(format!("Награда: {won} Won, {points} очков"), Color::Gold),
                line(
                    if finished {
                        "Пройдено."
                    } else {
                        "Ещё впереди."
                    },
                    if finished {
                        Color::Green
                    } else {
                        Color::DarkGray
                    },
                ),
            ],
        );
    }
    ui::open_buttons(
        player,
        line("Путь охотника", Color::Gold),
        6,
        items,
        Box::new(|_, _, _| After::Keep),
    );
}

#[cfg(test)]
mod tests {
    use super::{ERRAND_SLOTS, TRIAL_SLOTS};
    use crate::quest_book::{DAILY_ERRANDS, PATH, WEEKLY_TRIALS};

    #[test]
    fn every_task_has_a_slot() {
        assert_eq!(ERRAND_SLOTS.len(), DAILY_ERRANDS);
        assert_eq!(TRIAL_SLOTS.len(), WEEKLY_TRIALS);
        assert!(9 + PATH.len() <= 54);
    }
}
