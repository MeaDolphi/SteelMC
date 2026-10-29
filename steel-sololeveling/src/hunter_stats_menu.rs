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

use crate::sidebar;
use crate::state::SoloLevelingState;
use crate::stats::{HunterStat, HunterStats};

const STRENGTH_SLOT: usize = 0;
const VITALITY_SLOT: usize = 1;
const AGILITY_SLOT: usize = 2;
const INTELLIGENCE_SLOT: usize = 3;
const SENSE_SLOT: usize = 4;
/// Opens the hunter-stat allocation menu for an online player.
pub fn open(player: Arc<Player>, state: Arc<SoloLevelingState>) {
    player.open_menu(
        TextComponent::plain("Hunter Stats").color(Color::Gold),
        move |context| build(context.container_id, context.player, Arc::clone(&state)),
    );
}

fn build(container_id: u8, player: &Player, state: Arc<SoloLevelingState>) -> Menu {
    let player_uuid = player.gameprofile.id();
    let hunter_stats = state
        .with_session_mut(player_uuid, |session| session.stats)
        .unwrap_or_default();
    let container = SimpleContainer::from_items(menu_items(&hunter_stats)).into_shared();

    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X1, container_id);
    let buttons = builder.section_all(Arc::clone(&container));
    builder.player_inventory(&player.inventory);
    builder.build(HunterStatsMenuKind {
        state,
        buttons,
        container,
    })
}

fn menu_items(stats: &HunterStats) -> Vec<ItemStack> {
    vec![
        stat_item(
            &vanilla_items::IRON_SWORD,
            "Strength",
            stats.strength,
            "Attack damage: +1% per point",
        ),
        stat_item(
            &vanilla_items::GOLDEN_APPLE,
            "Vitality",
            stats.vitality,
            "Maximum health: +1% per point",
        ),
        stat_item(
            &vanilla_items::FEATHER,
            "Agility",
            stats.agility,
            "Movement and attack speed: +0.5% per point",
        ),
        stat_item(
            &vanilla_items::ENCHANTED_BOOK,
            "Intelligence",
            stats.intelligence,
            "Reserved for future skills",
        ),
        stat_item(
            &vanilla_items::SPYGLASS,
            "Sense",
            stats.sense,
            "Reserved for future gate sensing",
        ),
        ItemStack::empty(),
        points_item(stats.unspent_points),
        ItemStack::empty(),
        ItemStack::empty(),
    ]
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
            TextComponent::plain(format!("Current value: {value}")).color(Color::Aqua),
            TextComponent::plain(effect.to_owned()),
            TextComponent::plain("Left-click: allocate 1 point").color(Color::Yellow),
        ],
    );
    stack
}

fn points_item(points: u32) -> ItemStack {
    let mut stack = ItemStack::new(&vanilla_items::NETHER_STAR);
    stack.set(
        CUSTOM_NAME,
        TextComponent::plain("Unspent points").color(Color::Yellow),
    );
    set_lore(
        &mut stack,
        vec![TextComponent::plain(format!("Available: {points}")).color(Color::Aqua)],
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

        let Some(stat) = stat_for_slot(slot - self.buttons.start()) else {
            return ClickOutcome::Consume;
        };
        let Some(stats) = self.state.allocate_point(player.gameprofile.id(), stat) else {
            return ClickOutcome::Consume;
        };

        stats.apply_to(&mut player.attributes().lock());
        let (won, system_points, quest) = self
            .state
            .with_session_mut(player.gameprofile.id(), |session| {
                (session.won, session.system_points, session.quest)
            })
            .unwrap_or_default();
        sidebar::refresh(player, &stats, won, system_points, &quest);

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
