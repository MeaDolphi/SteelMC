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
use steel_utils::locks::{IntoShared, SyncMutex};
use steel_utils::{DowncastType, DowncastTypeKey};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use crate::item::artifact::{self, Artifact, Rarity};
use crate::item::enchantment::{self, Enchantment};
use crate::item::serial::{self, Serial, SerialId};
use crate::item::{append_lore, named, with_lore};
use crate::state::{EnchantTarget, SoloLevelingState};
use crate::ui::{After, open_buttons};

/// Either a named artifact or a serial item that can be enchanted.
#[derive(Debug, Clone, Copy)]
enum Enchantable {
    Artifact(&'static Artifact),
    Serial(&'static Serial),
}

impl Enchantable {
    fn id(self) -> EitherId {
        match self {
            Self::Artifact(artifact) => EitherId::Artifact(artifact.id),
            Self::Serial(serial) => EitherId::Serial(serial.id),
        }
    }

    fn rarity(self) -> Rarity {
        match self {
            Self::Artifact(artifact) => artifact.rarity,
            Self::Serial(_) => Rarity::Common,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum EitherId {
    Artifact(crate::item::artifact::ArtifactId),
    Serial(SerialId),
}

impl EitherId {
    fn enchant_kind(self) -> Option<crate::item::enchantment::ItemKind> {
        match self {
            Self::Artifact(id) => id.enchant_kind(),
            Self::Serial(id) => id.enchant_kind(),
        }
    }
}

const MENU_SLOTS: usize = 54;
/// Artifacts the picker can list in its largest chest.
const PICKER_LIMIT: usize = 54;
const ITEM_SLOT: usize = 3;
const INFO_SLOT: usize = 5;
/// First slot of the three rows of powers.
const POWER_ROW: usize = 9;
/// First slot of the bottom row of side effects.
const SIDE_EFFECT_ROW: usize = 45;

fn held_enchantable(stack: &ItemStack) -> Option<Enchantable> {
    if let Some(artifact) = artifact::held(stack) {
        return Some(Enchantable::Artifact(artifact));
    }
    serial::held(stack).map(Enchantable::Serial)
}

fn enchantments_of(stack: &ItemStack) -> Vec<Enchantment> {
    if artifact::of(stack).is_some() {
        artifact::enchantments_of(stack)
    } else {
        serial::enchantments_of(stack)
    }
}

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
            .filter(|(stack, _)| held_enchantable(stack).is_some())
            .collect()
    };
    let gear = state
        .with_session_mut(player.gameprofile.id(), |session| session.gear.clone())
        .unwrap_or_default();
    choices.extend(
        gear.into_iter()
            .enumerate()
            .filter(|(_, stack)| held_enchantable(stack).is_some())
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
    let Some(item) = held_enchantable(&held) else {
        player.send_message(
            &TextComponent::plain("Этот предмет нельзя зачаровать.").color(Color::Red),
        );
        return;
    };
    let applied = enchantments_of(&held);
    player.open_menu(
        TextComponent::plain("Зачарование").color(Color::DarkAqua),
        move |context| {
            build(
                context.container_id,
                context.player,
                state,
                held,
                target,
                item,
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
    item: Enchantable,
    applied: Vec<Enchantment>,
) -> Menu {
    let mut kind = EnchantMenuKind {
        state,
        buttons: None,
        container: None,
        held,
        target,
        item,
        applied,
        queued: None,
        completion: None,
    };
    let items = kind.items(player);
    let container = SimpleContainer::from_items(items).into_shared();
    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X6, container_id);
    kind.buttons = Some(builder.section_all(Arc::clone(&container)));
    kind.container = Some(container);
    builder.player_inventory(&player.inventory);
    builder.build(kind)
}

/// Returns the enchantments a menu offers, each with its menu slot.
///
/// Powers fill three rows, simple ones first; side effects take the bottom
/// row. An item without a kind is offered nothing.
fn offers(item: Enchantable) -> Vec<(usize, Enchantment)> {
    let Some(kind) = item.id().enchant_kind() else {
        return Vec::new();
    };
    let mut fitting: Vec<Enchantment> = enchantment::ALL
        .iter()
        .copied()
        .filter(|enchantment| enchantment.fits(kind))
        .collect();
    fitting.sort_by_key(|enchantment| !enchantment.is_simple());
    let fitting = fitting.into_iter();
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

pub(crate) struct EnchantMenuKind {
    state: Arc<SoloLevelingState>,
    buttons: Option<Section>,
    container: Option<Shared<SimpleContainer>>,
    held: ItemStack,
    target: EnchantTarget,
    item: Enchantable,
    applied: Vec<Enchantment>,
    queued: Option<Enchantment>,
    completion: Option<Arc<SyncMutex<Option<EnchantTarget>>>>,
}

impl EnchantMenuKind {
    fn finish(&mut self, target: EnchantTarget, held: ItemStack) -> bool {
        let Some(item) = held_enchantable(&held) else {
            return false;
        };
        self.target = target;
        self.item = item;
        self.applied = enchantments_of(&held);
        self.held = held;
        self.queued = None;
        self.completion = None;
        true
    }

    fn items(&self, player: &Player) -> Vec<ItemStack> {
        let mut items = vec![ItemStack::empty(); MENU_SLOTS];
        let used = enchantment::points_used(&self.applied);
        let budget = enchantment::budget(self.item.rarity());
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
        for (slot, enchantment) in offers(self.item) {
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
                self.item.rarity(),
                self.item.id().enchant_kind(),
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
                TextComponent::plain(if enchantment.is_simple() {
                    "Простое зачарование"
                } else {
                    "Особое зачарование"
                })
                .color(Color::DarkGray),
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
    fn on_tick(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        player: &Player,
    ) {
        let Some(target) = self
            .completion
            .as_ref()
            .and_then(|completion| *completion.lock())
        else {
            return;
        };
        let held = match target {
            EnchantTarget::Inventory(slot) => {
                let id = ContainerId::from_arc(&player.inventory);
                let Some(inventory) = guard.get_typed::<PlayerInventory>(id) else {
                    return;
                };
                inventory.get_item(slot).clone()
            }
            EnchantTarget::Gear(index) => self
                .state
                .with_session_mut(player.gameprofile.id(), |session| {
                    session.gear.get(index).cloned()
                })
                .flatten()
                .unwrap_or_else(ItemStack::empty),
        };
        if !self.finish(target, held) {
            player.close_container();
            return;
        }
        let items = self.items(player);
        if let Some(container) = self.container.as_ref() {
            let id = ContainerId::from_arc(container);
            if let Some(container) = guard.get_typed_mut::<SimpleContainer>(id) {
                for (index, item) in items.into_iter().enumerate() {
                    container.set_item(index, item);
                }
            }
        }
    }

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
        let Some(enchantment) = offers(self.item)
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
        let completion = Arc::new(SyncMutex::new(None));
        self.state.defer_enchantment(
            player.gameprofile.id(),
            enchantment,
            self.target,
            Arc::clone(&completion),
        );
        self.queued = Some(enchantment);
        self.completion = Some(completion);
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
    use std::sync::Arc;

    use steel_utils::locks::SyncMutex;

    use super::{EnchantMenuKind, MENU_SLOTS, POWER_ROW, SIDE_EFFECT_ROW, offers};
    use crate::item::artifact::{self, ALL, ArtifactId};
    use crate::item::enchantment::Enchantment;
    use crate::item::serial::ALL as SERIAL_ALL;
    use crate::state::{EnchantTarget, SoloLevelingState};

    #[test]
    fn completion_refreshes_snapshot_and_allows_another_attempt() {
        let directory = std::env::temp_dir().join(format!("enchant-menu-{}", uuid::Uuid::new_v4()));
        let state = Arc::new(SoloLevelingState::new(directory.clone()).expect("state"));
        let id = ArtifactId::AssassinsShoes;
        let enchantment = Enchantment::Protection;
        let mut menu = EnchantMenuKind {
            state,
            buttons: None,
            container: None,
            held: artifact::stack(id, None),
            target: EnchantTarget::Inventory(0),
            item: super::Enchantable::Artifact(id.artifact()),
            applied: Vec::new(),
            queued: Some(enchantment),
            completion: Some(Arc::new(SyncMutex::new(None))),
        };

        // A failed transaction retains the old artifact but releases the queued row.
        assert!(menu.finish(EnchantTarget::Inventory(4), artifact::stack(id, None)));
        assert_eq!(menu.target, EnchantTarget::Inventory(4));
        assert!(menu.applied.is_empty());
        assert!(menu.queued.is_none());
        assert!(menu.completion.is_none());
        let available = menu.offer_item(enchantment, u64::MAX);

        // A committed transaction must render the newly applied enchantment.
        menu.queued = Some(enchantment);
        assert!(menu.finish(
            EnchantTarget::Inventory(4),
            artifact::enchanted_stack(id, None, &[enchantment]),
        ));
        assert_eq!(menu.applied, [enchantment]);
        assert!(menu.queued.is_none());
        assert_ne!(available, menu.offer_item(enchantment, u64::MAX));
        drop(menu);
        std::fs::remove_dir_all(directory).expect("remove state directory");
    }

    #[test]
    fn every_offer_has_its_own_slot() {
        for id in ALL.into_iter() {
            let offered = offers(super::Enchantable::Artifact(id.artifact()));
            for (slot, enchantment) in &offered {
                let row = if enchantment.is_side_effect() {
                    SIDE_EFFECT_ROW..MENU_SLOTS
                } else {
                    POWER_ROW..SIDE_EFFECT_ROW - 9
                };
                assert!(row.contains(slot), "{id:?} {enchantment:?} at {slot}");
            }
            let mut slots: Vec<usize> = offered.iter().map(|(slot, _)| *slot).collect();
            slots.sort_unstable();
            slots.dedup();
            assert_eq!(slots.len(), offered.len());
        }
        for id in SERIAL_ALL.into_iter() {
            let offered = offers(super::Enchantable::Serial(id.serial()));
            for (slot, enchantment) in &offered {
                let row = if enchantment.is_side_effect() {
                    SIDE_EFFECT_ROW..MENU_SLOTS
                } else {
                    POWER_ROW..SIDE_EFFECT_ROW - 9
                };
                assert!(row.contains(slot), "{id:?} {enchantment:?} at {slot}");
            }
            let mut slots: Vec<usize> = offered.iter().map(|(slot, _)| *slot).collect();
            slots.sort_unstable();
            slots.dedup();
            assert_eq!(slots.len(), offered.len());
        }
    }
}
