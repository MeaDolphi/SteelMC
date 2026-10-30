//! The grimoire as a chest menu.
//!
//! One row per school lists its spells: a click unlocks a locked spell for
//! system points, or prepares an unlocked one and puts it away again. The
//! bottom row shows the prepared set; a click there selects the spell the
//! grimoire casts.

use std::sync::Arc;

use steel_core::inventory::prelude::*;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_items;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use crate::item::{named, with_lore};
use crate::rank::HunterRank;
use crate::spell::{self, School, SpellBook, SpellId};
use crate::state::SoloLevelingState;
use crate::ui::{self, After};

const MENU_SLOTS: usize = 54;
const INFO_SLOT: usize = 4;
/// First slot of the prepared row.
const PREPARED_SLOT: usize = 46;

/// Opens the grimoire.
pub fn open(player: &Player, state: Arc<SoloLevelingState>) {
    let uuid = player.gameprofile.id();
    let items = menu_items(&state, uuid);
    ui::open_buttons(
        player,
        TextComponent::plain("Гримуар Системы").color(Color::DarkPurple),
        6,
        items,
        Box::new(move |player, slot, _| click(&state, player, slot)),
    );
}

/// Returns the menu slot of the school row's `column`-th spell.
const fn school_row(school: School) -> usize {
    match school {
        School::Destruction => 1,
        School::Healing => 2,
        School::Control => 3,
        School::Utility => 4,
    }
}

/// Returns the menu slot of every spell, school by school.
fn slots() -> Vec<(usize, SpellId)> {
    let mut column = [0_usize; 4];
    spell::ALL
        .into_iter()
        .map(|spell| {
            let row = school_row(spell.info().school);
            let index = row - 1;
            column[index] += 1;
            (row * 9 + column[index], spell)
        })
        .collect()
}

fn spell_at(slot: usize) -> Option<SpellId> {
    slots()
        .into_iter()
        .find_map(|(at, spell)| (at == slot).then_some(spell))
}

fn school_icon(school: School) -> ItemRef {
    match school {
        School::Destruction => &vanilla_items::BLAZE_POWDER,
        School::Healing => &vanilla_items::GLISTERING_MELON_SLICE,
        School::Control => &vanilla_items::ENDER_EYE,
        School::Utility => &vanilla_items::FEATHER,
    }
}

/// What the menu reads from the session.
struct View {
    book: SpellBook,
    rank: HunterRank,
    points: u64,
    intelligence: u32,
}

fn view(state: &SoloLevelingState, uuid: Uuid) -> Option<View> {
    state.with_session_mut(uuid, |session| View {
        book: session.spells.clone(),
        rank: session.stats.rank(),
        points: session.system_points.amount(),
        intelligence: session.stats.intelligence,
    })
}

fn line(text: impl Into<String>, color: Color) -> TextComponent {
    TextComponent::plain(text.into()).color(color)
}

fn menu_items(state: &SoloLevelingState, uuid: Uuid) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); MENU_SLOTS];
    let Some(view) = view(state, uuid) else {
        return items;
    };
    let slots_total = spell::prepared_slots(view.rank);
    items[INFO_SLOT] = with_lore(
        named(
            ItemStack::new(&vanilla_items::ENCHANTED_BOOK),
            line("Гримуар Системы", Color::LightPurple),
        ),
        vec![
            line(format!("Очки Системы: {}", view.points), Color::Gold),
            line(
                format!(
                    "Мана: {} (+{:.1} в секунду)",
                    spell::max_mana(view.intelligence),
                    spell::mana_regen(view.intelligence, false)
                ),
                Color::Aqua,
            ),
            line(
                format!("Подготовлено: {} / {slots_total}", view.book.prepared.len()),
                Color::Gray,
            ),
            line("Клик по закрытому — открыть за очки.", Color::Gray),
            line("Клик по открытому — подготовить или убрать.", Color::Gray),
            line("Набор меняется только вне подземелий.", Color::Gray),
        ],
    );
    for school in School::ALL {
        items[school_row(school) * 9] = with_lore(
            named(
                ItemStack::new(school_icon(school)),
                line(school.display_name(), Color::Gold),
            ),
            Vec::new(),
        );
    }
    for (slot, spell) in slots() {
        items[slot] = spell_item(&view, spell);
    }
    for index in 0..slots_total {
        let slot = PREPARED_SLOT + index;
        items[slot] = match view.book.prepared.get(index) {
            Some(spell) => {
                let selected = view.book.selected == index;
                with_lore(
                    named(
                        ItemStack::new(if selected {
                            &vanilla_items::ENCHANTED_BOOK
                        } else {
                            &vanilla_items::BOOK
                        }),
                        line(
                            format!("{}. {}", index + 1, spell.info().name),
                            if selected { Color::Green } else { Color::Aqua },
                        ),
                    ),
                    vec![line(
                        if selected {
                            "Выбрано для гримуара."
                        } else {
                            "Клик — выбрать для гримуара."
                        },
                        Color::Yellow,
                    )],
                )
            }
            None => with_lore(
                named(
                    ItemStack::new(&vanilla_items::GRAY_STAINED_GLASS_PANE),
                    line(format!("{}. Пустая ячейка", index + 1), Color::DarkGray),
                ),
                Vec::new(),
            ),
        };
    }
    items
}

fn spell_item(view: &View, spell: SpellId) -> ItemStack {
    let info = spell.info();
    let known = view.book.knows(spell);
    let prepared = view.book.prepared.contains(&spell);
    let mut lore = vec![
        line(info.description, Color::Gray),
        line(
            format!(
                "Мана: {} · Перезарядка: {} с",
                info.mana,
                info.cooldown_millis / 1_000
            ),
            Color::Aqua,
        ),
    ];
    let (status, color) = if prepared {
        ("Подготовлено. Клик — убрать.".to_owned(), Color::Green)
    } else if known {
        ("Открыто. Клик — подготовить.".to_owned(), Color::Yellow)
    } else {
        lore.push(line(
            format!(
                "Цена: {} очков Системы · ранг {}",
                info.price,
                info.rank.name()
            ),
            Color::Gold,
        ));
        if view.rank < info.rank {
            (format!("Нужен ранг {}.", info.rank.name()), Color::Red)
        } else if view.points < info.price {
            ("Не хватает очков Системы.".to_owned(), Color::Red)
        } else {
            ("Клик — открыть.".to_owned(), Color::Yellow)
        }
    };
    lore.push(line(status, color));
    with_lore(
        named(
            ItemStack::new(if known {
                school_icon(info.school)
            } else {
                &vanilla_items::GRAY_DYE
            }),
            line(
                info.name,
                if prepared {
                    Color::Green
                } else if known {
                    Color::Aqua
                } else {
                    Color::DarkGray
                },
            ),
        ),
        lore,
    )
}

fn click(state: &Arc<SoloLevelingState>, player: &Player, slot: usize) -> After {
    let uuid = player.gameprofile.id();
    if (PREPARED_SLOT..PREPARED_SLOT + 6).contains(&slot) {
        state.select_spell(uuid, slot - PREPARED_SLOT);
        return After::Redraw(menu_items(state, uuid));
    }
    let Some(spell) = spell_at(slot) else {
        return After::Keep;
    };
    let known = view(state, uuid).is_some_and(|view| view.book.knows(spell));
    let result = if known {
        state.toggle_spell(player, spell).map(|()| None)
    } else {
        state.unlock_spell(player, spell).map(Some)
    };
    match result {
        Ok(Some(price)) => ui::tell(
            player,
            &format!(
                "Открыто заклинание «{}» за {price} очков.",
                spell.info().name
            ),
            Color::LightPurple,
        ),
        Ok(None) => {}
        Err(error) => ui::tell(player, error.message(), Color::Red),
    }
    After::Redraw(menu_items(state, uuid))
}

#[cfg(test)]
mod tests {
    use super::{INFO_SLOT, MENU_SLOTS, PREPARED_SLOT, slots, spell_at};
    use crate::spell;

    #[test]
    fn every_spell_has_its_own_slot_clear_of_the_buttons() {
        let all = slots();
        assert_eq!(all.len(), spell::ALL.len());
        for (slot, spell) in &all {
            assert!(*slot < PREPARED_SLOT - 1 && *slot != INFO_SLOT && *slot % 9 != 0);
            assert_eq!(spell_at(*slot), Some(*spell));
        }
        let mut unique: Vec<usize> = all.iter().map(|(slot, _)| *slot).collect();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), all.len());
        const { assert!(PREPARED_SLOT + 6 <= MENU_SLOTS) };
    }
}
