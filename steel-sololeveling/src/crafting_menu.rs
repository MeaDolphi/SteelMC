//! Crafting orders at the city merchant as a chest menu.
//!
//! Each armor line takes one row, helmet to boots; the weapons sit in the
//! column to the right. The top row holds tabs for the three crafted tiers.
//! Every recipe lists its materials with how many the hunter carries, the work
//! fee, and why it cannot be ordered yet.

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
use crate::item::serial::{self, CraftedTier, IMPROVED, SerialId};
use crate::item::{append_lore, named, with_lore};
use crate::merchant_menu;
use crate::rank::HunterRank;
use crate::state::SoloLevelingState;

const MENU_SLOTS: usize = 54;
const INFO_SLOT: usize = 4;
const BACK_SLOT: usize = 49;
/// Tab slots for Improved / Elite / Supreme.
const TAB_SLOTS: [usize; 3] = [0, 1, 2];
/// Slots of the weapons, in catalog order after the armor.
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

fn tier_array(tier: CraftedTier) -> &'static [SerialId] {
    match tier {
        CraftedTier::Improved => &IMPROVED,
        CraftedTier::Elite => &serial::tiered::ELITE,
        CraftedTier::Supreme => &serial::tiered::SUPREME,
    }
}

fn tier_from_tab(slot: usize) -> Option<CraftedTier> {
    match slot {
        0 => Some(CraftedTier::Improved),
        1 => Some(CraftedTier::Elite),
        2 => Some(CraftedTier::Supreme),
        _ => None,
    }
}

/// Menu slot of the `index`-th entry of a tier catalog.
fn slot_of(index: usize) -> Option<usize> {
    if index < 16 {
        Some((1 + index / 4) * 9 + 1 + index % 4)
    } else {
        WEAPON_SLOTS.get(index - 16).copied()
    }
}

fn recipe_at(slot: usize, tier: CraftedTier) -> Option<Recipe> {
    let ids = tier_array(tier);
    let index = (0..ids.len()).find(|index| slot_of(*index) == Some(slot))?;
    crafting::recipe(ids[index])
}

/// Everything the menu shows; a change redraws it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct View {
    won: u64,
    rank: HunterRank,
    tier: CraftedTier,
    /// Carried units of each recipe's materials, in the current tier's order.
    carried: Vec<Vec<u32>>,
}

fn view(
    state: &SoloLevelingState,
    uuid: Uuid,
    inventory: &dyn Container,
    tier: CraftedTier,
) -> Option<View> {
    let (won, rank) =
        state.with_session_mut(uuid, |session| (session.won.amount(), session.stats.rank()))?;
    Some(View {
        won,
        rank,
        tier,
        carried: carried_materials(inventory, tier),
    })
}

fn carried_materials(inventory: &dyn Container, tier: CraftedTier) -> Vec<Vec<u32>> {
    let slots: Vec<ItemStack> = (0..INVENTORY_SLOTS)
        .map(|slot| inventory.get_item(slot).clone())
        .collect();
    crafting::tier_recipes(tier)
        .iter()
        .map(|recipe| crafting::available(recipe, &slots))
        .collect()
}

fn build(container_id: u8, player: &Player, state: Arc<SoloLevelingState>) -> Menu {
    let tier = CraftedTier::Improved;
    let view = view(
        &state,
        player.gameprofile.id(),
        &*player.inventory.lock(),
        tier,
    );
    let container = SimpleContainer::from_items(menu_items(view.as_ref())).into_shared();
    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X6, container_id);
    let buttons = builder.section_all(Arc::clone(&container));
    builder.player_inventory(&player.inventory);
    builder.build(CraftingMenuKind {
        state,
        buttons,
        container,
        view,
        tier,
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
            TextComponent::plain(format!(
                "{} ступень серийных вещей.",
                view.tier.display_name()
            ))
            .color(Color::Gray),
            TextComponent::plain(format!(
                "Материалы: {} (ранг {}).",
                view.tier.quality().display_name(),
                view.tier.rank().name()
            ))
            .color(Color::Gray),
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
    for (index, tier) in [
        CraftedTier::Improved,
        CraftedTier::Elite,
        CraftedTier::Supreme,
    ]
    .into_iter()
    .enumerate()
    {
        items[TAB_SLOTS[index]] = tab_item(view, tier);
    }
    for ((index, recipe), carried) in crafting::tier_recipes(view.tier)
        .into_iter()
        .enumerate()
        .zip(&view.carried)
    {
        if let Some(slot) = slot_of(index) {
            items[slot] = recipe_item(view, &recipe, carried);
        }
    }
    items
}

fn tab_item(view: &View, tier: CraftedTier) -> ItemStack {
    let selected = view.tier == tier;
    let icon = if selected {
        &vanilla_items::ENCHANTED_BOOK
    } else {
        &vanilla_items::PAPER
    };
    with_lore(
        named(
            ItemStack::new(icon),
            TextComponent::plain(format!("{} ступень", tier.display_name())).color(if selected {
                Color::Gold
            } else {
                Color::White
            }),
        ),
        vec![
            TextComponent::plain(format!("Ранг: {}", tier.rank().name())).color(Color::Gray),
            TextComponent::plain(format!("Материалы: {}", tier.quality().display_name()))
                .color(Color::Gray),
            TextComponent::plain("Клик — переключить вкладку.").color(Color::Yellow),
        ],
    )
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
        (
            format!("Заказы принимают с ранга {}.", recipe.rank.name()),
            Color::Red,
        )
    } else if view.won < recipe.work {
        ("Не хватает Won.".to_owned(), Color::Red)
    } else if !enough {
        ("Не хватает материалов.".to_owned(), Color::Red)
    } else {
        ("Клик — заказать.".to_owned(), Color::Yellow)
    };
    lines.push(TextComponent::plain(status).color(color));
    append_lore(serial::stack(recipe.output, None), lines)
}

pub(crate) struct CraftingMenuKind {
    state: Arc<SoloLevelingState>,
    buttons: Section,
    container: Shared<SimpleContainer>,
    view: Option<View>,
    tier: CraftedTier,
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for CraftingMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/crafting");
}

impl CraftingMenuKind {
    fn refresh(&mut self, guard: &mut ContainerLockGuard, player: &Player) {
        let inventory_id = ContainerId::from_arc(&player.inventory);
        let Some(inventory) = guard.get_typed::<PlayerInventory>(inventory_id) else {
            return;
        };
        let view = view(&self.state, player.gameprofile.id(), inventory, self.tier);
        let container_id = ContainerId::from_arc(&self.container);
        if let Some(container) = guard.get_typed_mut::<SimpleContainer>(container_id) {
            for (index, item) in menu_items(view.as_ref()).into_iter().enumerate() {
                container.set_item(index, item);
            }
            self.view = view;
        }
    }
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
        self.refresh(guard, player);
    }

    fn on_slot_clicked(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
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
        let local = slot - self.buttons.start();
        if local == BACK_SLOT {
            merchant_menu::open(player, Arc::clone(&self.state));
            return ClickOutcome::Consume;
        }
        if let Some(tier) = tier_from_tab(local) {
            self.tier = tier;
            self.refresh(guard, player);
            return ClickOutcome::Consume;
        }
        let Some(recipe) = recipe_at(local, self.tier) else {
            return ClickOutcome::Consume;
        };
        match self.state.request_crafting(player, recipe.output) {
            Ok(()) => player.send_message(
                &TextComponent::plain(format!(
                    "Заказ «{}» обрабатывается.",
                    recipe.output.serial().name
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

    use super::{
        BACK_SLOT, INFO_SLOT, INVENTORY_SLOTS, MENU_SLOTS, TAB_SLOTS, carried_materials, recipe_at,
        slot_of, tier_from_tab,
    };
    use crate::crafting;
    use crate::item::material;
    use crate::item::serial::{CraftedTier, IMPROVED, tiered};

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

        let counts = carried_materials(locked, CraftedTier::Improved);
        assert_eq!(counts[0][0], 7);
        assert!(
            inventory.try_lock().is_none(),
            "the snapshot must not require releasing the menu's inventory lock"
        );
    }

    #[test]
    fn every_recipe_has_its_own_slot() {
        for tier in [
            CraftedTier::Improved,
            CraftedTier::Elite,
            CraftedTier::Supreme,
        ] {
            let ids = match tier {
                CraftedTier::Improved => &IMPROVED as &[crate::item::serial::SerialId],
                CraftedTier::Elite => &tiered::ELITE,
                CraftedTier::Supreme => &tiered::SUPREME,
            };
            let mut seen = Vec::new();
            for (index, output) in ids.iter().copied().enumerate() {
                let slot = slot_of(index).expect("slot");
                assert!(slot < MENU_SLOTS);
                assert!(slot != INFO_SLOT && slot != BACK_SLOT);
                assert!(!TAB_SLOTS.contains(&slot));
                assert!(!seen.contains(&slot));
                seen.push(slot);
                assert_eq!(
                    recipe_at(slot, tier).map(|recipe| recipe.output),
                    Some(output)
                );
            }
        }
    }

    #[test]
    fn every_tier_is_reachable_from_a_tab() {
        assert_eq!(tier_from_tab(TAB_SLOTS[0]), Some(CraftedTier::Improved));
        assert_eq!(tier_from_tab(TAB_SLOTS[1]), Some(CraftedTier::Elite));
        assert_eq!(tier_from_tab(TAB_SLOTS[2]), Some(CraftedTier::Supreme));
    }
}
