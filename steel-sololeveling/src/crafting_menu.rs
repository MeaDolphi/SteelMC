//! Crafting orders at the city merchant as a chest menu.
//!
//! Each armor line takes one row, helmet to boots; the improved weapons sit
//! in the column to the right. Every recipe lists its materials with how
//! many the hunter carries, the work fee, and why it cannot be ordered yet.

use std::sync::Arc;

use steel_core::inventory::container::Container as _;
use steel_core::inventory::prelude::*;
use steel_core::player::player_inventory::PlayerInventory;
use steel_registry::{vanilla_items, vanilla_menu_types};
use steel_utils::locks::IntoShared;
use steel_utils::{DowncastType, DowncastTypeKey};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use crate::crafting::{self, Recipe};
use crate::item::artifact::{self, IMPROVED};
use crate::item::{append_lore, named, with_lore};
use crate::merchant_menu;
use crate::rank::HunterRank;
use crate::state::SoloLevelingState;

const MENU_SLOTS: usize = 54;
const INFO_SLOT: usize = 4;
const BACK_SLOT: usize = 49;
/// Slots of the improved weapons, in [`IMPROVED`] order after the armor.
const WEAPON_SLOTS: [usize; 3] = [15, 24, 33];
/// Main-inventory slots materials are counted in.
const INVENTORY_SLOTS: usize = 36;

/// Opens the order desk.
pub fn open(player: &Player, state: Arc<SoloLevelingState>) {
    player.open_menu(
        TextComponent::plain("Заказы у мастера").color(Color::Gold),
        move |context| build(context.container_id, context.player, state),
    );
}

/// Menu slot of the `index`-th entry of [`IMPROVED`].
fn slot_of(index: usize) -> Option<usize> {
    if index < 16 {
        Some((1 + index / 4) * 9 + 1 + index % 4)
    } else {
        WEAPON_SLOTS.get(index - 16).copied()
    }
}

fn recipe_at(slot: usize) -> Option<Recipe> {
    let index = (0..IMPROVED.len()).find(|index| slot_of(*index) == Some(slot))?;
    crafting::recipe(IMPROVED[index])
}

/// Everything the menu shows; a change redraws it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct View {
    won: u64,
    rank: HunterRank,
    /// Carried units of each recipe's materials, in [`IMPROVED`] order.
    carried: Vec<Vec<u32>>,
}

fn view(state: &SoloLevelingState, uuid: Uuid, inventory: &dyn Container) -> Option<View> {
    let (won, rank) =
        state.with_session_mut(uuid, |session| (session.won.amount(), session.stats.rank()))?;
    Some(View {
        won,
        rank,
        carried: carried_materials(inventory),
    })
}

fn carried_materials(inventory: &dyn Container) -> Vec<Vec<u32>> {
    let slots: Vec<ItemStack> = (0..INVENTORY_SLOTS)
        .map(|slot| inventory.get_item(slot).clone())
        .collect();
    crafting::all()
        .iter()
        .map(|recipe| crafting::available(recipe, &slots))
        .collect()
}

fn build(container_id: u8, player: &Player, state: Arc<SoloLevelingState>) -> Menu {
    let view = view(&state, player.gameprofile.id(), &*player.inventory.lock());
    let container = SimpleContainer::from_items(menu_items(view.as_ref())).into_shared();
    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X6, container_id);
    let buttons = builder.section_all(Arc::clone(&container));
    builder.player_inventory(&player.inventory);
    builder.build(CraftingMenuKind {
        state,
        buttons,
        container,
        view,
    })
}

fn menu_items(view: Option<&View>) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); MENU_SLOTS];
    let Some(view) = view else {
        return items;
    };
    items[INFO_SLOT] = with_lore(
        named(
            ItemStack::new(&vanilla_items::SMITHING_TABLE),
            TextComponent::plain(format!("Ваши Won: {}", view.won)).color(Color::Gold),
        ),
        vec![
            TextComponent::plain("Улучшенная ступень серийных вещей.").color(Color::Gray),
            TextComponent::plain("Материалы — из инвентаря, работа оплачивается в Won.")
                .color(Color::Gray),
            TextComponent::plain("Без таймера и без риска провала.").color(Color::Gray),
            TextComponent::plain("Изделия можно передавать и продавать.").color(Color::Gray),
        ],
    );
    items[BACK_SLOT] = with_lore(
        named(
            ItemStack::new(&vanilla_items::ARROW),
            TextComponent::plain("К витринам торговца").color(Color::Yellow),
        ),
        Vec::new(),
    );
    for ((index, recipe), carried) in crafting::all().into_iter().enumerate().zip(&view.carried) {
        if let Some(slot) = slot_of(index) {
            items[slot] = recipe_item(view, &recipe, carried);
        }
    }
    items
}

fn recipe_item(view: &View, recipe: &Recipe, carried: &[u32]) -> ItemStack {
    let mut lines: Vec<TextComponent> = recipe
        .materials
        .iter()
        .zip(carried)
        .map(|((material, needed), have)| {
            TextComponent::plain(format!(
                "{} ({}) ×{needed} — есть {have}",
                material.display_name(),
                recipe.quality.display_name()
            ))
            .color(if have >= needed {
                Color::Green
            } else {
                Color::Red
            })
        })
        .collect();
    lines.push(
        TextComponent::plain(format!("Работа мастера: {} Won", recipe.work)).color(Color::Gold),
    );
    let enough = recipe
        .materials
        .iter()
        .zip(carried)
        .all(|((_, needed), have)| have >= needed);
    let (status, color) = if view.rank < recipe.rank {
        ("Заказы принимают с ранга D.", Color::Red)
    } else if view.won < recipe.work {
        ("Не хватает Won.", Color::Red)
    } else if !enough {
        ("Не хватает материалов.", Color::Red)
    } else {
        ("Клик — заказать.", Color::Yellow)
    };
    lines.push(TextComponent::plain(status).color(color));
    append_lore(artifact::stack(recipe.output, None), lines)
}

pub(crate) struct CraftingMenuKind {
    state: Arc<SoloLevelingState>,
    buttons: Section,
    container: Shared<SimpleContainer>,
    view: Option<View>,
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for CraftingMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/crafting");
}

impl MenuKind for CraftingMenuKind {
    fn on_tick(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        player: &Player,
    ) {
        if player.tick_count() % 20 != 0 {
            return;
        }
        let inventory_id = ContainerId::from_arc(&player.inventory);
        let Some(inventory) = guard.get_typed::<PlayerInventory>(inventory_id) else {
            return;
        };
        let view = view(&self.state, player.gameprofile.id(), inventory);
        if self.view == view {
            return;
        }
        let container_id = ContainerId::from_arc(&self.container);
        if let Some(container) = guard.get_typed_mut::<SimpleContainer>(container_id) {
            for (index, item) in menu_items(view.as_ref()).into_iter().enumerate() {
                container.set_item(index, item);
            }
            self.view = view;
        }
    }

    fn on_slot_clicked(
        &mut self,
        _behavior: &mut MenuBehavior,
        _guard: &mut ContainerLockGuard,
        click: Click,
        player: &Player,
    ) -> ClickOutcome {
        let Click::Pickup { slot, .. } = click else {
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
            merchant_menu::open(player, Arc::clone(&self.state));
            return ClickOutcome::Consume;
        }
        let Some(recipe) = recipe_at(slot) else {
            return ClickOutcome::Consume;
        };
        match self.state.request_crafting(player, recipe.output) {
            Ok(()) => player.send_message(
                &TextComponent::plain(format!(
                    "Заказ «{}» обрабатывается.",
                    recipe.output.artifact().name
                ))
                .color(Color::Gold),
            ),
            Err(error) => {
                player.send_message(&TextComponent::plain(error.message()).color(Color::Red));
            }
        }
        ClickOutcome::Consume
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use steel_core::inventory::prelude::*;
    use steel_registry::init_vanilla_registry;
    use steel_utils::locks::IntoShared;

    use super::{BACK_SLOT, INFO_SLOT, MENU_SLOTS, recipe_at, slot_of};
    use super::{INVENTORY_SLOTS, carried_materials};
    use crate::crafting;
    use crate::item::artifact::IMPROVED;
    use crate::item::material;

    #[test]
    fn material_counts_use_the_already_locked_container() {
        init_vanilla_registry();
        let recipe = crafting::all().remove(0);
        let ingredient = recipe.materials[0].0;
        let mut slots = vec![ItemStack::empty(); INVENTORY_SLOTS];
        slots[0] = material::stack(ingredient, recipe.quality, 3);
        slots[35] = material::stack(ingredient, recipe.quality, 4);
        let inventory = SimpleContainer::from_items(slots).into_shared();
        let guard = ContainerLockGuard::lock_all(&[ContainerRef::from(Arc::clone(&inventory))]);
        let locked = guard
            .get_typed::<SimpleContainer>(ContainerId::from_arc(&inventory))
            .expect("inventory participates in the menu lock");

        let counts = carried_materials(locked);
        assert_eq!(counts[0][0], 7);
        assert!(
            inventory.try_lock().is_none(),
            "the snapshot must not require releasing the menu's inventory lock"
        );
    }

    #[test]
    fn every_recipe_has_its_own_slot() {
        let mut seen = Vec::new();
        for (index, output) in IMPROVED.into_iter().enumerate() {
            let slot = slot_of(index).expect("slot");
            assert!(slot < MENU_SLOTS);
            assert!(slot != INFO_SLOT && slot != BACK_SLOT);
            assert!(!seen.contains(&slot));
            seen.push(slot);
            assert_eq!(recipe_at(slot).map(|recipe| recipe.output), Some(output));
        }
    }
}
