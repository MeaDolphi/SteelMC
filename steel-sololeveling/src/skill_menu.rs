//! Class and general combat skills as a chest menu.

use std::sync::Arc;

use steel_core::player::Player;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_items;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use crate::class::HunterClass;
use crate::combat_skill::{CombatSkill, CombatSkillBook, UpgradeError};
use crate::dungeon::is_instance_world;
use crate::item::{named, with_lore};
use crate::main_menu;
use crate::state::SoloLevelingState;
use crate::stats::HunterStats;
use crate::ui::{self, After};

const INFO_SLOT: usize = 4;
const CLASS_SLOT: usize = 11;
const GENERAL_SLOTS: [usize; 2] = [12, 13];
const RESET_SLOT: usize = 22;
const BACK_SLOT: usize = 26;

#[derive(Clone)]
struct View {
    class: Option<HunterClass>,
    stats: HunterStats,
    book: CombatSkillBook,
}

pub fn open(player: &Player, state: Arc<SoloLevelingState>) {
    ui::open_buttons(
        player,
        Arc::clone(&state),
        TextComponent::plain("Боевые навыки").color(Color::Gold),
        3,
        menu_items(&state, player.gameprofile.id()),
        Box::new(move |player, slot, _| click(&state, player, slot)),
    );
}

fn click(state: &Arc<SoloLevelingState>, player: &Player, slot: usize) -> After {
    if slot == BACK_SLOT {
        main_menu::open(player, Arc::clone(state));
        return After::Replaced;
    }
    if slot == RESET_SLOT {
        let yes = Arc::clone(state);
        let no = Arc::clone(state);
        ui::confirm_with_back(
            player,
            Arc::clone(state),
            "Сбросить все улучшения навыков и вернуть очки?",
            Arc::new(move |player| {
                if let Err(error) = yes.reset_combat_skills(player) {
                    ui::tell(player, error, Color::Red);
                }
                open(player, Arc::clone(&yes));
            }),
            Arc::new(move |player| open(player, Arc::clone(&no))),
        );
        return After::Replaced;
    }
    if is_instance_world(&player.get_world()) {
        ui::tell(
            player,
            "Набор навыков меняется только вне подземелий.",
            Color::Red,
        );
        return After::Keep;
    }
    let result = match slot {
        CLASS_SLOT => state.upgrade_class_skill(player.gameprofile.id()),
        slot if slot == GENERAL_SLOTS[0] => {
            state.upgrade_general_skill(player.gameprofile.id(), CombatSkill::DaggerThrow)
        }
        slot if slot == GENERAL_SLOTS[1] => {
            state.upgrade_general_skill(player.gameprofile.id(), CombatSkill::ShadowExchange)
        }
        _ => return After::Keep,
    };
    if let Err(error) = result {
        ui::tell(player, upgrade_error(error), Color::Red);
    }
    After::Redraw(menu_items(state, player.gameprofile.id()))
}

fn upgrade_error(error: UpgradeError) -> &'static str {
    match error {
        UpgradeError::ClassLocked => "Классовый навык доступен после выбора класса с ранга D.",
        UpgradeError::RankLocked => "Ранг охотника пока не позволяет изучить этот навык.",
        UpgradeError::MaxRank => "Навык уже имеет максимальный уровень III.",
        UpgradeError::NoPoints => "Не хватает очков навыков. Они начисляются за уровни охотника.",
        UpgradeError::NotGeneral => "Этот навык нельзя выбрать как общий.",
    }
}

fn view(state: &SoloLevelingState, uuid: Uuid) -> Option<View> {
    state.with_session_mut(uuid, |session| View {
        class: session.class.class,
        stats: session.stats,
        book: session.combat_skills.clone(),
    })
}

fn skill_icon(skill: CombatSkill) -> ItemRef {
    match skill {
        CombatSkill::Mutilation => &vanilla_items::IRON_SWORD,
        CombatSkill::Taunt => &vanilla_items::SHIELD,
        CombatSkill::Stealth => &vanilla_items::IRON_HOE,
        CombatSkill::VitalShot => &vanilla_items::BOW,
        CombatSkill::RulersAuthority => &vanilla_items::BLAZE_ROD,
        CombatSkill::DaggerThrow => &vanilla_items::IRON_HOE,
        CombatSkill::ShadowExchange => &vanilla_items::ENDER_PEARL,
    }
}

fn skill_item(_view: &View, skill: CombatSkill, rank: u8, selected: bool) -> ItemStack {
    let mut lore = vec![
        TextComponent::plain(skill.description()).color(Color::Gray),
        TextComponent::plain(format!("Уровень: {rank} / 3")).color(Color::Aqua),
        TextComponent::plain(format!("Перезарядка: {} с", skill.cooldown_seconds()))
            .color(Color::Gray),
    ];
    if rank < 3 {
        lore.push(
            TextComponent::plain(format!(
                "Следующий уровень: {} очков навыков",
                skill.next_rank_cost(rank)
            ))
            .color(Color::Gold),
        );
        lore.push(TextComponent::plain("Клик — изучить или улучшить.").color(Color::Yellow));
    } else {
        lore.push(TextComponent::plain("Максимальный уровень.").color(Color::Green));
    }
    if selected {
        lore.push(TextComponent::plain("Выбрано для Shift + ПКМ.").color(Color::Green));
    }
    with_lore(
        named(
            ItemStack::new(skill_icon(skill)),
            TextComponent::plain(skill.title()).color(if selected {
                Color::Green
            } else {
                Color::Gold
            }),
        ),
        lore,
    )
}

fn menu_items(state: &SoloLevelingState, uuid: Uuid) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); 27];
    let Some(view) = view(state, uuid) else {
        items[BACK_SLOT] = ui::main_menu_button();
        return items;
    };
    let class_skill = view.class.map(CombatSkill::for_class);
    items[INFO_SLOT] = with_lore(
        named(
            ItemStack::new(&vanilla_items::ENCHANTED_BOOK),
            TextComponent::plain("Боевые навыки").color(Color::Gold),
        ),
        vec![
            TextComponent::plain(format!(
                "Свободные очки: {}",
                view.book.available_points(view.stats.level)
            ))
            .color(Color::Aqua),
            TextComponent::plain("ПКМ — классовый навык.").color(Color::Gray),
            TextComponent::plain("Shift + ПКМ — общий навык.").color(Color::Gray),
            TextComponent::plain("Набор меняется только вне подземелий.").color(Color::Gray),
        ],
    );
    if let Some(skill) = class_skill {
        items[CLASS_SLOT] = skill_item(
            &view,
            skill,
            view.class.map_or(0, |class| view.book.class_rank(class)),
            false,
        );
    } else {
        items[CLASS_SLOT] = with_lore(
            named(
                ItemStack::new(&vanilla_items::GRAY_DYE),
                TextComponent::plain("Классовый навык не выбран").color(Color::DarkGray),
            ),
            vec![TextComponent::plain("Выберите класс с ранга D.").color(Color::Gray)],
        );
    }
    for (slot, skill) in GENERAL_SLOTS.into_iter().zip(CombatSkill::GENERAL) {
        items[slot] = skill_item(
            &view,
            skill,
            view.book.general_rank(skill),
            view.book.selected_general == Some(skill),
        );
    }
    items[RESET_SLOT] = with_lore(
        named(
            ItemStack::new(&vanilla_items::REDSTONE),
            TextComponent::plain("Сбросить навыки").color(Color::Red),
        ),
        vec![
            TextComponent::plain("Вернуть потраченные очки навыков.").color(Color::Gray),
            TextComponent::plain("Базовый классовый навык сохранится.").color(Color::Gray),
            TextComponent::plain("Только в городе, с подтверждением.").color(Color::Gray),
        ],
    );
    items[BACK_SLOT] = ui::main_menu_button();
    items
}

#[cfg(test)]
mod tests {
    use steel_registry::data_components::vanilla_components::{CUSTOM_NAME, LORE};
    use text_components::resolving::NoResolutor;
    use uuid::Uuid;

    use super::menu_items;
    use crate::class::HunterClass;
    use crate::player_data::SoloLevelingPlayerData;
    use crate::state::SoloLevelingState;

    #[test]
    fn menu_shows_skills_reset_and_main_return() {
        let directory = std::env::temp_dir().join(format!("skill-menu-test-{}", Uuid::new_v4()));
        let state = SoloLevelingState::new(directory.clone()).expect("state");
        let uuid = Uuid::new_v4();
        let mut data = SoloLevelingPlayerData::new(uuid);
        data.stats.level = 10;
        data.class.class = Some(HunterClass::Fighter);
        state.begin_session(uuid, data);
        let text = menu_items(&state, uuid)
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
            "Боевые навыки",
            "Рассечение",
            "Бросок кинжала",
            "Обмен тенью",
            "Сбросить навыки",
            "Главное меню",
        ] {
            assert!(text.contains(expected), "missing {expected:?}");
        }
        std::fs::remove_dir_all(directory).expect("cleanup");
    }
}
