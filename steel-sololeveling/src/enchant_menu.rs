//! The item maker as chest menus.
//!
//! A picker lists every artifact in the inventory and the System gear; the
//! chosen one opens the item maker. Its top row shows the artifact and its
//! point budget; the second
//! row lists powers and the third side effects that fit it. A click queues the
//! enchantment; the menu works on a snapshot of the artifact, so it marks the
//! queued row instead of reading the inventory its own click has locked.

use std::sync::Arc;

use steel_core::inventory::container::Container as _;
use steel_core::inventory::prelude::*;
use steel_core::player::player_inventory::PlayerInventory;
use steel_registry::{vanilla_items, vanilla_menu_types};
use steel_utils::locks::IntoShared;
use steel_utils::{DowncastType, DowncastTypeKey};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use crate::item::artifact::{self, Artifact};
use crate::item::enchantment::{self, Enchantment};
use crate::item::{append_lore, named, with_lore};
use crate::state::{EnchantTarget, SoloLevelingState};
use crate::ui::{After, open_buttons};

const MENU_SLOTS: usize = 27;
/// Artifacts the picker can list in its largest chest.
const PICKER_LIMIT: usize = 54;
const ITEM_SLOT: usize = 3;
const INFO_SLOT: usize = 5;
const POWER_ROW: usize = 10;
const SIDE_EFFECT_ROW: usize = 19;

/// Opens the picker of artifacts to enchant.
///
/// It reads the inventory, so the caller runs outside any menu callback.
pub fn open_picker(player: &Player, state: Arc<SoloLevelingState>) {
    let mut choices: Vec<(ItemStack, EnchantTarget)> = {
        let inventory = player.inventory.lock();
        (0..PlayerInventory::INVENTORY_SIZE)
            .map(|slot| {
                (
                    inventory.get_item(slot).clone(),
                    EnchantTarget::Inventory(slot),
                )
            })
            .filter(|(stack, _)| artifact::of(stack).is_some())
            .collect()
    };
    let gear = state
        .with_session_mut(player.gameprofile.id(), |session| session.gear.clone())
        .unwrap_or_default();
    choices.extend(
        gear.into_iter()
            .enumerate()
            .filter(|(_, stack)| artifact::of(stack).is_some())
            .map(|(index, stack)| (stack, EnchantTarget::Gear(index))),
    );
    choices.truncate(PICKER_LIMIT);
    if choices.is_empty() {
        player.send_message(
            &TextComponent::plain("Нет артефактов ни в инвентаре, ни в снаряжении.")
                .color(Color::Red),
        );
        return;
    }
    let items = choices
        .iter()
        .map(|(stack, target)| {
            let place = match target {
                EnchantTarget::Inventory(_) => "В инвентаре",
                EnchantTarget::Gear(_) => "В снаряжении",
            };
            append_lore(
                stack.clone(),
                vec![
                    TextComponent::plain(place).color(Color::DarkGray),
                    TextComponent::plain("Клик — зачаровать.").color(Color::Yellow),
                ],
            )
        })
        .collect();
    open_buttons(
        player,
        TextComponent::plain("Выберите артефакт").color(Color::DarkAqua),
        if choices.len() > 27 { 6 } else { 3 },
        items,
        Box::new(move |player, index, _| {
            let Some((stack, target)) = choices.get(index).cloned() else {
                return After::Keep;
            };
            open(player, Arc::clone(&state), stack, target);
            After::Replaced
        }),
    );
}

/// Opens the item maker for the artifact `held` at `target`.
pub fn open(
    player: &Player,
    state: Arc<SoloLevelingState>,
    held: ItemStack,
    target: EnchantTarget,
) {
    let Some(artifact) = artifact::held(&held) else {
        player.send_message(&TextComponent::plain("Этот предмет не артефакт.").color(Color::Red));
        return;
    };
    let applied = artifact::enchantments_of(&held);
    player.open_menu(
        TextComponent::plain("Зачарование").color(Color::DarkAqua),
        move |context| {
            build(
                context.container_id,
                context.player,
                state,
                held,
                target,
                artifact,
                applied,
            )
        },
    );
}

fn build(
    container_id: u8,
    player: &Player,
    state: Arc<SoloLevelingState>,
    held: ItemStack,
    target: EnchantTarget,
    artifact: &'static Artifact,
    applied: Vec<Enchantment>,
) -> Menu {
    let mut kind = EnchantMenuKind {
        state,
        buttons: None,
        container: None,
        held,
        target,
        artifact,
        applied,
        queued: None,
    };
    let items = kind.items(player);
    let container = SimpleContainer::from_items(items).into_shared();
    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X3, container_id);
    kind.buttons = Some(builder.section_all(Arc::clone(&container)));
    kind.container = Some(container);
    builder.player_inventory(&player.inventory);
    builder.build(kind)
}

/// Returns the enchantments a menu offers, each with its menu slot.
fn offers(artifact: &Artifact) -> Vec<(usize, Enchantment)> {
    let fitting = enchantment::ALL
        .into_iter()
        .filter(|enchantment| enchantment.fits(artifact.category));
    let powers = fitting
        .clone()
        .filter(|enchantment| !enchantment.is_side_effect());
    let side_effects = fitting.filter(|enchantment| enchantment.is_side_effect());
    powers
        .enumerate()
        .map(|(index, enchantment)| (POWER_ROW + index, enchantment))
        .chain(
            side_effects
                .enumerate()
                .map(|(index, enchantment)| (SIDE_EFFECT_ROW + index, enchantment)),
        )
        .collect()
}

struct EnchantMenuKind {
    state: Arc<SoloLevelingState>,
    buttons: Option<Section>,
    container: Option<Shared<SimpleContainer>>,
    held: ItemStack,
    target: EnchantTarget,
    artifact: &'static Artifact,
    applied: Vec<Enchantment>,
    queued: Option<Enchantment>,
}

impl EnchantMenuKind {
    fn items(&self, player: &Player) -> Vec<ItemStack> {
        let mut items = vec![ItemStack::empty(); MENU_SLOTS];
        let used = enchantment::points_used(&self.applied);
        let budget = enchantment::budget(self.artifact.rarity);
        items[ITEM_SLOT] = self.held.clone();
        items[INFO_SLOT] = with_lore(
            named(
                ItemStack::new(&vanilla_items::EXPERIENCE_BOTTLE),
                TextComponent::plain("Бюджет предмета").color(Color::DarkAqua),
            ),
            vec![
                TextComponent::plain(format!("Очки зачарования: {used} / {budget}"))
                    .color(Color::DarkAqua),
                TextComponent::plain(format!(
                    "Зачарований: {} / {}",
                    self.applied.len(),
                    enchantment::MAX_ENCHANTMENTS
                ))
                .color(Color::Gray),
                TextComponent::plain("Недостатки дают очки, но не снижают цену.")
                    .color(Color::Gray),
            ],
        );
        let balance = self
            .state
            .with_session_mut(player.gameprofile.id(), |session| session.won.amount())
            .unwrap_or(0);
        for (slot, enchantment) in offers(self.artifact) {
            items[slot] = self.offer_item(enchantment, balance);
        }
        items
    }

    fn offer_item(&self, enchantment: Enchantment, balance: u64) -> ItemStack {
        let side = enchantment.is_side_effect();
        let icon = if self.applied.contains(&enchantment) {
            &vanilla_items::GRAY_STAINED_GLASS_PANE
        } else if side {
            &vanilla_items::FERMENTED_SPIDER_EYE
        } else {
            &vanilla_items::ENCHANTED_BOOK
        };
        let status = if self.queued == Some(enchantment) {
            ("Обрабатывается…".to_owned(), Color::Gold)
        } else {
            match enchantment::check(
                self.artifact.rarity,
                self.artifact.category,
                &self.applied,
                enchantment,
                balance,
            ) {
                Ok(()) => ("Клик — наложить.".to_owned(), Color::Yellow),
                Err(error) => (error.message().to_owned(), Color::Red),
            }
        };
        with_lore(
            named(
                ItemStack::new(icon),
                TextComponent::plain(enchantment.display_name()).color(if side {
                    Color::Red
                } else {
                    Color::Aqua
                }),
            ),
            vec![
                TextComponent::plain(enchantment.describe()).color(Color::Gray),
                TextComponent::plain(format!("Очки: {:+}", enchantment.cost()))
                    .color(Color::DarkAqua),
                TextComponent::plain(format!("Цена: {} Won", enchantment.price()))
                    .color(Color::Gold),
                TextComponent::plain(status.0).color(status.1),
            ],
        )
    }
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for EnchantMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/enchant");
}

impl MenuKind for EnchantMenuKind {
    fn on_slot_clicked(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        click: Click,
        player: &Player,
    ) -> ClickOutcome {
        let Some(buttons) = self.buttons.as_ref() else {
            return ClickOutcome::Consume;
        };
        let Click::Pickup { slot, .. } = click else {
            return if click.slot().is_some_and(|slot| buttons.contains(slot)) {
                ClickOutcome::Consume
            } else {
                ClickOutcome::Fallthrough
            };
        };
        if !buttons.contains(slot) {
            return ClickOutcome::Fallthrough;
        }
        let local = slot - buttons.start();
        let Some(enchantment) = offers(self.artifact)
            .into_iter()
            .find_map(|(at, enchantment)| (at == local).then_some(enchantment))
        else {
            return ClickOutcome::Consume;
        };
        if self.queued.is_some() {
            player.send_message(
                &TextComponent::plain("Предыдущее зачарование ещё обрабатывается.")
                    .color(Color::Red),
            );
            return ClickOutcome::Consume;
        }
        // The click owns the inventory lock, so the request runs on the next
        // player tick, which reads the real artifact again.
        self.state
            .defer_enchantment(player.gameprofile.id(), enchantment, self.target);
        self.queued = Some(enchantment);
        if let Some(container) = self.container.as_ref() {
            let id = ContainerId::from_arc(container);
            let items = self.items(player);
            if let Some(container) = guard.get_typed_mut::<SimpleContainer>(id) {
                for (index, item) in items.into_iter().enumerate() {
                    container.set_item(index, item);
                }
            }
        }
        ClickOutcome::Consume
    }
}

#[cfg(test)]
mod tests {
    use super::{MENU_SLOTS, offers};
    use crate::item::artifact::ALL;

    #[test]
    fn every_offer_has_its_own_slot() {
        for id in ALL {
            let slots: Vec<usize> = offers(id.artifact())
                .iter()
                .map(|(slot, _)| *slot)
                .collect();
            assert!(slots.iter().all(|slot| *slot < MENU_SLOTS));
            let mut unique = slots.clone();
            unique.dedup();
            assert_eq!(unique.len(), slots.len());
        }
    }
}
