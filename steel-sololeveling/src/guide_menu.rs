//! The newcomer's quick start and short reference as a chest menu.

use std::sync::Arc;

use steel_core::player::Player;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_items;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use crate::class_menu;
use crate::guide::{self, GuideChapter};
use crate::hunter_stats_menu;
use crate::item::{named, with_lore};
use crate::party_menu;
use crate::quest_book::{Achievement, PATH};
use crate::quest_menu;
use crate::shadow_menu;
use crate::shop_menu;
use crate::skill_menu;
use crate::starter_menu;
use crate::state::{GateMenuAction, SoloLevelingState};
use crate::ui::{self, After};

const CURRENT_SLOT: usize = 4;
const FIRST_CHAPTER_SLOT: usize = 9;
const PATH_SLOT: usize = 21;
const TOGGLE_SLOT: usize = 23;
const BACK_SLOT: usize = 26;

#[derive(Clone)]
struct View {
    achievements: Vec<Achievement>,
    hidden: bool,
}

/// Opens the guide and refreshes the existing achievements it reads.
pub fn open(player: &Player, state: Arc<SoloLevelingState>) {
    let uuid = player.gameprofile.id();
    state.refresh_path(uuid);
    ui::open_buttons(
        player,
        Arc::clone(&state),
        TextComponent::plain("Профиль и справка").color(Color::Aqua),
        3,
        menu_items(view(&state, uuid)),
        Box::new(move |player, slot, _| click(&state, player, slot)),
    );
}

fn view(state: &SoloLevelingState, uuid: Uuid) -> Option<View> {
    state.with_session_mut(uuid, |session| View {
        achievements: session.quests.achievements.clone(),
        hidden: session.quests.guide_hidden,
    })
}

fn click(state: &Arc<SoloLevelingState>, player: &Player, slot: usize) -> After {
    let uuid = player.gameprofile.id();
    if slot == TOGGLE_SLOT {
        let Some(()) = state.with_session_mut(uuid, |session| {
            session.quests.guide_hidden = !session.quests.guide_hidden;
        }) else {
            return After::Keep;
        };
        if let Err(error) = state.save_session(uuid) {
            log::warn!("sololeveling: could not save guide preference of {uuid}: {error}");
        }
        return After::Redraw(menu_items(view(state, uuid)));
    }
    if slot == PATH_SLOT {
        quest_menu::open_path(player, state);
        return After::Replaced;
    }
    if slot == CURRENT_SLOT {
        let next = view(state, uuid).and_then(|view| guide::next_step(&view.achievements));
        if let Some(step) = next {
            open_step(state, player, step);
        } else {
            quest_menu::open_path(player, state);
        }
        return After::Replaced;
    }
    let Some(index) = slot.checked_sub(FIRST_CHAPTER_SLOT) else {
        return After::Keep;
    };
    let Some(section) = guide::SECTIONS.get(index) else {
        return After::Keep;
    };
    match section.chapter {
        GuideChapter::Controls => skill_menu::open(player, Arc::clone(state)),
        GuideChapter::Equipment => state.defer_gear_menu(uuid),
        GuideChapter::Development => hunter_stats_menu::open(player, Arc::clone(state)),
        GuideChapter::Gates => {
            state.defer_gate_menu_action(uuid, GateMenuAction::OpenPage(0));
        }
        GuideChapter::Economy => shop_menu::open(player, Arc::clone(state)),
        GuideChapter::Shadows => shadow_menu::open(player, Arc::clone(state)),
        GuideChapter::Group => party_menu::open(player, Arc::clone(state)),
    }
    After::Replaced
}

fn open_step(state: &Arc<SoloLevelingState>, player: &Player, step: Achievement) {
    let uuid = player.gameprofile.id();
    match step {
        Achievement::StarterKit => starter_menu::open(player, Arc::clone(state)),
        Achievement::FirstGate | Achievement::FirstBoss => {
            state.defer_gate_menu_action(uuid, GateMenuAction::OpenPage(0));
        }
        Achievement::Level10 => hunter_stats_menu::open(player, Arc::clone(state)),
        Achievement::RankD | Achievement::ClassChosen => {
            class_menu::open(player, Arc::clone(state));
        }
        _ => quest_menu::open_path(player, state),
    }
}

fn chapter_icon(chapter: GuideChapter) -> ItemRef {
    match chapter {
        GuideChapter::Controls => &vanilla_items::ENCHANTED_BOOK,
        GuideChapter::Equipment => &vanilla_items::ARMOR_STAND,
        GuideChapter::Development => &vanilla_items::EXPERIENCE_BOTTLE,
        GuideChapter::Gates => &vanilla_items::ENDER_EYE,
        GuideChapter::Economy => &vanilla_items::EMERALD,
        GuideChapter::Shadows => &vanilla_items::WITHER_SKELETON_SKULL,
        GuideChapter::Group => &vanilla_items::BELL,
    }
}

fn current_item(view: Option<&View>) -> ItemStack {
    let next = view.and_then(|view| guide::next_step(&view.achievements));
    if let Some(step) = next {
        return with_lore(
            named(
                ItemStack::new(&vanilla_items::ENCHANTED_BOOK),
                TextComponent::plain(format!("Далее: {}", step.title())).color(Color::Gold),
            ),
            vec![
                TextComponent::plain(step.description()).color(Color::Gray),
                TextComponent::plain("Клик — перейти к цели.").color(Color::Yellow),
                TextComponent::plain("Награда уже входит в Путь охотника.").color(Color::DarkGray),
            ],
        );
    }
    with_lore(
        named(
            ItemStack::new(&vanilla_items::LIME_DYE),
            TextComponent::plain("Быстрый старт завершён").color(Color::Green),
        ),
        vec![
            TextComponent::plain("Продолжайте развитие через Путь охотника.").color(Color::Gray),
            TextComponent::plain("Клик — все достижения.").color(Color::Yellow),
        ],
    )
}

fn menu_items(view: Option<View>) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); 27];
    items[CURRENT_SLOT] = current_item(view.as_ref());
    for (index, section) in guide::SECTIONS.into_iter().enumerate() {
        let mut lore = section
            .lines
            .iter()
            .map(|line| TextComponent::plain(*line).color(Color::Gray))
            .collect::<Vec<_>>();
        lore.push(TextComponent::plain("Клик — открыть систему.").color(Color::Yellow));
        items[FIRST_CHAPTER_SLOT + index] = with_lore(
            named(
                ItemStack::new(chapter_icon(section.chapter)),
                TextComponent::plain(section.title).color(Color::Aqua),
            ),
            lore,
        );
    }
    items[PATH_SLOT] = with_lore(
        named(
            ItemStack::new(&vanilla_items::FILLED_MAP),
            TextComponent::plain("Путь охотника").color(Color::Gold),
        ),
        vec![
            TextComponent::plain(format!("{} достижений с наградами.", PATH.len()))
                .color(Color::Gray),
            TextComponent::plain("Клик — открыть полный список.").color(Color::Yellow),
        ],
    );
    let hidden = view.is_some_and(|view| view.hidden);
    items[TOGGLE_SLOT] = with_lore(
        named(
            ItemStack::new(if hidden {
                &vanilla_items::BARRIER
            } else {
                &vanilla_items::BELL
            }),
            TextComponent::plain(if hidden {
                "Подсказки скрыты"
            } else {
                "Подсказки включены"
            })
            .color(if hidden { Color::Red } else { Color::Green }),
        ),
        vec![
            TextComponent::plain("Справка останется доступной здесь.").color(Color::Gray),
            TextComponent::plain(if hidden {
                "Клик — показывать подсказки."
            } else {
                "Клик — скрыть подсказки."
            })
            .color(Color::Yellow),
        ],
    );
    items[BACK_SLOT] = ui::main_menu_button();
    items
}

#[cfg(test)]
mod tests {
    use steel_registry::data_components::vanilla_components::{CUSTOM_NAME, LORE};
    use text_components::resolving::NoResolutor;

    use super::{CURRENT_SLOT, View, menu_items};
    use crate::quest_book::Achievement;

    #[test]
    fn guide_shows_the_next_step_and_all_reference_chapters() {
        let items = menu_items(Some(View {
            achievements: vec![Achievement::StarterKit],
            hidden: false,
        }));
        let text = items
            .iter()
            .map(|stack| {
                let mut text = stack
                    .get(CUSTOM_NAME)
                    .map(|name| name.to_plain(&NoResolutor))
                    .unwrap_or_default();
                if let Some(lore) = stack.get(LORE) {
                    for line in lore.lines() {
                        text.push_str(&line.to_plain(&NoResolutor));
                    }
                }
                text
            })
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            "Далее: Первые врата",
            "Управление",
            "Снаряжение и броня",
            "Развитие охотника",
            "Врата и кристаллы",
            "Деньги и магазины",
            "Армия теней",
            "Группа и сообщество",
            "Путь охотника",
        ] {
            assert!(text.contains(expected), "missing {expected:?}");
        }
        assert!(!items[CURRENT_SLOT].is_empty());
    }
}
