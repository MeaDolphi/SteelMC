//! Chest-style menu for allocating hunter stat points.

use std::sync::Arc;

use steel_core::entity::LivingEntity;
use steel_core::inventory::container::Container as _;
use steel_core::inventory::prelude::*;
use steel_registry::data_components::components::ItemLore;
use steel_registry::data_components::vanilla_components::{CUSTOM_NAME, LORE};
use steel_registry::items::ItemRef;
use steel_registry::{vanilla_items, vanilla_menu_types};
use steel_utils::locks::IntoShared;
use steel_utils::{DowncastType, DowncastTypeKey};
use text_components::Modifier as _;
use text_components::TextComponent;
use text_components::format::Color;

use crate::main_menu;
use crate::state::SoloLevelingState;
use crate::stats::{HunterStat, HunterStats};
use crate::ui;

const STRENGTH_SLOT: usize = 0;
const VITALITY_SLOT: usize = 1;
const AGILITY_SLOT: usize = 2;
const INTELLIGENCE_SLOT: usize = 3;
const SENSE_SLOT: usize = 4;
const RESET_SLOT: usize = 22;
const BACK_SLOT: usize = 26;
/// Opens the hunter-stat allocation menu for an online player.
pub fn open(player: &Player, state: Arc<SoloLevelingState>) {
    player.open_menu(
        TextComponent::plain("Характеристики охотника").color(Color::Gold),
        move |context| build(context.container_id, context.player, Arc::clone(&state)),
    );
}

fn build(container_id: u8, player: &Player, state: Arc<SoloLevelingState>) -> Menu {
    let player_uuid = player.gameprofile.id();
    let hunter_stats = state
        .with_session_mut(player_uuid, |session| session.stats)
        .unwrap_or_default();
    let container = SimpleContainer::from_items(menu_items(&hunter_stats)).into_shared();

    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X3, container_id);
    let buttons = builder.section_all(Arc::clone(&container));
    builder.player_inventory(&player.inventory);
    builder.build(HunterStatsMenuKind {
        state,
        buttons,
        container,
    })
}

fn menu_items(stats: &HunterStats) -> Vec<ItemStack> {
    let mut items = vec![
        stat_item(
            &vanilla_items::IRON_SWORD,
            "Сила",
            stats.strength,
            "Урон атаки: +1% за очко",
        ),
        stat_item(
            &vanilla_items::GOLDEN_APPLE,
            "Выносливость",
            stats.vitality,
            "Максимальное здоровье: +1% за очко",
        ),
        stat_item(
            &vanilla_items::FEATHER,
            "Ловкость",
            stats.agility,
            "Скорость движения и атаки: +0,5% за очко",
        ),
        stat_item(
            &vanilla_items::ENCHANTED_BOOK,
            "Интеллект",
            stats.intelligence,
            "Мана: +5 за очко · восстановление: +0,1/с",
        ),
        stat_item(
            &vanilla_items::SPYGLASS,
            "Чутьё",
            stats.sense,
            "Перезарядка боевых навыков: -1% за очко, до -30%",
        ),
        ItemStack::empty(),
        points_item(stats.unspent_points),
        level_item(stats),
        experience_item(stats),
    ];
    items.resize(27, ItemStack::empty());
    items[RESET_SLOT] = ui::button(
        &vanilla_items::REDSTONE,
        "Сбросить характеристики",
        Color::Red,
        &[
            "Вернуть все распределённые очки.",
            "Уровень и опыт не изменятся.",
            "Только в городе, с подтверждением.",
        ],
    );
    items[BACK_SLOT] = ui::back_button();
    items
}

fn stat_item(item: ItemRef, name: &str, value: u32, effect: &str) -> ItemStack {
    let mut stack = ItemStack::new(item);
    stack.set(
        CUSTOM_NAME,
        TextComponent::plain(name.to_owned()).color(Color::Gold),
    );
    set_lore(
        &mut stack,
        vec![
            TextComponent::plain(format!("Текущее значение: {value}")).color(Color::Aqua),
            TextComponent::plain(effect.to_owned()),
            TextComponent::plain("ЛКМ: добавить 1 очко").color(Color::Yellow),
        ],
    );
    stack
}

fn level_item(stats: &HunterStats) -> ItemStack {
    let mut stack = ItemStack::new(&vanilla_items::BOOK);
    stack.set(
        CUSTOM_NAME,
        TextComponent::plain("Уровень и ранг").color(Color::Gold),
    );
    set_lore(
        &mut stack,
        vec![
            TextComponent::plain(format!(
                "Уровень {} · ранг {}",
                stats.level,
                stats.rank().name()
            ))
            .color(Color::Aqua),
        ],
    );
    stack
}

fn experience_item(stats: &HunterStats) -> ItemStack {
    let mut stack = ItemStack::new(&vanilla_items::EXPERIENCE_BOTTLE);
    stack.set(
        CUSTOM_NAME,
        TextComponent::plain("Опыт").color(Color::Green),
    );
    set_lore(
        &mut stack,
        vec![
            TextComponent::plain(format!(
                "{} / {} XP до следующего уровня",
                stats.experience,
                stats.experience_to_next_level()
            ))
            .color(Color::Aqua),
        ],
    );
    stack
}

fn points_item(points: u32) -> ItemStack {
    let mut stack = ItemStack::new(&vanilla_items::NETHER_STAR);
    stack.set(
        CUSTOM_NAME,
        TextComponent::plain("Свободные очки характеристик").color(Color::Yellow),
    );
    set_lore(
        &mut stack,
        vec![TextComponent::plain(format!("Доступно: {points}")).color(Color::Aqua)],
    );
    stack
}

fn set_lore(stack: &mut ItemStack, lines: Vec<TextComponent>) {
    let Ok(lore) = ItemLore::new(lines) else {
        return;
    };
    stack.set(LORE, lore);
}

struct HunterStatsMenuKind {
    state: Arc<SoloLevelingState>,
    buttons: Section,
    container: Shared<SimpleContainer>,
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for HunterStatsMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/hunter_stats");
}

impl MenuKind for HunterStatsMenuKind {
    fn on_slot_clicked(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        click: Click,
        player: &Player,
    ) -> ClickOutcome {
        let Click::Pickup {
            slot,
            button: MouseButton::Left,
        } = click
        else {
            return if click.slot().is_some_and(|slot| self.buttons.contains(slot)) {
                ClickOutcome::Consume
            } else {
                ClickOutcome::Fallthrough
            };
        };
        if !self.buttons.contains(slot) {
            return ClickOutcome::Fallthrough;
        }
        let slot = slot - self.buttons.start();
        if slot == BACK_SLOT {
            main_menu::open(player, Arc::clone(&self.state));
            return ClickOutcome::Consume;
        }
        if slot == RESET_SLOT {
            let yes = Arc::clone(&self.state);
            let no = Arc::clone(&self.state);
            ui::confirm_with_back(
                player,
                Arc::clone(&self.state),
                "Вернуть все распределённые очки характеристик?",
                Arc::new(move |player| {
                    if let Err(error) = yes.reset_hunter_stats(player) {
                        ui::tell(player, error, Color::Red);
                    }
                    open(player, Arc::clone(&yes));
                }),
                Arc::new(move |player| open(player, Arc::clone(&no))),
            );
            return ClickOutcome::Consume;
        }

        let Some(stat) = stat_for_slot(slot) else {
            return ClickOutcome::Consume;
        };
        let Some(stats) = self.state.allocate_point(player.gameprofile.id(), stat) else {
            return ClickOutcome::Consume;
        };

        // The sidebar picks the new stats up on its next periodic refresh.
        stats.apply_to(&mut player.attributes().lock());

        let container_id = ContainerId::from_arc(&self.container);
        let Some(container) = guard.get_typed_mut::<SimpleContainer>(container_id) else {
            return ClickOutcome::Consume;
        };
        for (index, item) in menu_items(&stats).into_iter().enumerate() {
            container.set_item(index, item);
        }
        ClickOutcome::Consume
    }
}

const fn stat_for_slot(slot: usize) -> Option<HunterStat> {
    match slot {
        STRENGTH_SLOT => Some(HunterStat::Strength),
        VITALITY_SLOT => Some(HunterStat::Vitality),
        AGILITY_SLOT => Some(HunterStat::Agility),
        INTELLIGENCE_SLOT => Some(HunterStat::Intelligence),
        SENSE_SLOT => Some(HunterStat::Sense),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::data_components::vanilla_components::{CUSTOM_NAME, LORE};
    use steel_registry::item_stack::ItemStack;
    use text_components::resolving::NoResolutor;

    use super::{HunterStat, HunterStats, menu_items, stat_for_slot};

    fn item_text(stack: &ItemStack) -> String {
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
    }

    #[test]
    fn menu_is_russian_and_shows_current_progress() {
        let stats = HunterStats {
            level: 12,
            experience: 50,
            unspent_points: 6,
            strength: 3,
            intelligence: 4,
            ..HunterStats::new()
        };
        let items = menu_items(&stats);
        assert_eq!(items.len(), 27);
        let text = items.iter().map(item_text).collect::<Vec<_>>().join("\n");
        for expected in [
            "Сила",
            "Выносливость",
            "Ловкость",
            "Интеллект",
            "Чутьё",
            "Текущее значение: 3",
            "Мана: +5 за очко · восстановление: +0,1/с",
            "Перезарядка боевых навыков: -1% за очко, до -30%",
            "Сбросить характеристики",
            "Назад",
            "Доступно: 6",
            "Уровень 12 · ранг D",
            &format!("50 / {} XP", stats.experience_to_next_level()),
            "ЛКМ: добавить 1 очко",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
        for outdated in ["Strength", "Intelligence", "Reserved", "Current value"] {
            assert!(!text.contains(outdated), "outdated {outdated:?} in {text}");
        }
    }

    #[test]
    fn allocation_slots_are_unchanged() {
        assert_eq!(stat_for_slot(0), Some(HunterStat::Strength));
        assert_eq!(stat_for_slot(1), Some(HunterStat::Vitality));
        assert_eq!(stat_for_slot(2), Some(HunterStat::Agility));
        assert_eq!(stat_for_slot(3), Some(HunterStat::Intelligence));
        assert_eq!(stat_for_slot(4), Some(HunterStat::Sense));
        assert_eq!(stat_for_slot(5), None);
    }
}
