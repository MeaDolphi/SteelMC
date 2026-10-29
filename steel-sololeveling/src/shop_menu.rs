//! The personal System shop as a chest menu.
//!
//! The top row shows the hunter's system points; the two rows below hold the
//! day's ten slots. Clicking an offer buys it, and a bought slot turns grey.

use std::sync::Arc;

use steel_core::inventory::container::Container as _;
use steel_core::inventory::prelude::*;
use steel_core::player::player_inventory::Prediction;
use steel_registry::{vanilla_items, vanilla_menu_types};
use steel_utils::locks::IntoShared;
use steel_utils::{DowncastType, DowncastTypeKey};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use crate::item::{named, with_lore};
use crate::shop::ShopDay;
use crate::state::SoloLevelingState;

const MENU_SLOTS: usize = 27;
const BALANCE_SLOT: usize = 4;
/// Offers fill the middle of rows two and three, five per row.
const FIRST_ROW_START: usize = 11;
const SECOND_ROW_START: usize = 20;
const PER_ROW: usize = 5;

/// Opens the shop for an online hunter.
pub fn open(player: &Player, state: Arc<SoloLevelingState>) {
    player.open_menu(
        TextComponent::plain("Магазин Системы").color(Color::LightPurple),
        move |context| build(context.container_id, context.player, state),
    );
}

fn build(container_id: u8, player: &Player, state: Arc<SoloLevelingState>) -> Menu {
    let items = menu_items(&state, player.gameprofile.id());
    let container = SimpleContainer::from_items(items).into_shared();
    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X3, container_id);
    let buttons = builder.section_all(Arc::clone(&container));
    builder.player_inventory(&player.inventory);
    builder.build(ShopMenuKind {
        state,
        buttons,
        container,
    })
}

/// Returns the shop slot shown at a menu slot.
fn offer_at(slot: usize) -> Option<usize> {
    let row = |start: usize| {
        (start..start + PER_ROW)
            .contains(&slot)
            .then(|| slot - start)
    };
    row(FIRST_ROW_START).or_else(|| row(SECOND_ROW_START).map(|index| index + PER_ROW))
}

fn menu_items(state: &SoloLevelingState, uuid: Uuid) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); MENU_SLOTS];
    let Some(day) = state.shop_day(uuid) else {
        return items;
    };
    let points = state
        .with_session_mut(uuid, |session| session.system_points.amount())
        .unwrap_or(0);
    items[BALANCE_SLOT] = with_lore(
        named(
            ItemStack::new(&vanilla_items::AMETHYST_SHARD),
            TextComponent::plain(format!("Очки Системы: {points}")).color(Color::LightPurple),
        ),
        vec![
            TextComponent::plain("Набор товаров меняется в 00:00 по Москве.").color(Color::Gray),
            TextComponent::plain("Каждый слот покупается раз в день.").color(Color::Gray),
            TextComponent::plain("Покупки привязаны к вам.").color(Color::Gray),
        ],
    );
    for (slot, item) in items.iter_mut().enumerate() {
        if let Some(index) = offer_at(slot) {
            *item = offer_item(&day, index, points, uuid);
        }
    }
    items
}

fn offer_item(day: &ShopDay, index: usize, points: u64, uuid: Uuid) -> ItemStack {
    let Some(offer) = day.offer(index) else {
        return ItemStack::empty();
    };
    if day.bought(index) {
        return named(
            ItemStack::new(&vanilla_items::GRAY_STAINED_GLASS_PANE),
            TextComponent::plain(format!("Куплено: {}", offer.display_name()))
                .color(Color::DarkGray),
        );
    }
    let mut stack = offer.stack(uuid);
    let affordable = points >= offer.price;
    // The offer keeps its own name and lore; the price goes on a renamed copy.
    stack = named(
        stack,
        TextComponent::plain(offer.display_name()).color(if affordable {
            Color::Green
        } else {
            Color::Red
        }),
    );
    with_lore(
        stack,
        vec![
            TextComponent::plain(format!("Цена: {} очков Системы", offer.price)).color(Color::Gold),
            TextComponent::plain(if affordable {
                "Клик — купить."
            } else {
                "Не хватает очков."
            })
            .color(if affordable {
                Color::Yellow
            } else {
                Color::Red
            }),
        ],
    )
}

struct ShopMenuKind {
    state: Arc<SoloLevelingState>,
    buttons: Section,
    container: Shared<SimpleContainer>,
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for ShopMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/shop");
}

impl MenuKind for ShopMenuKind {
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
        let Some(index) = offer_at(slot - self.buttons.start()) else {
            return ClickOutcome::Consume;
        };
        let uuid = player.gameprofile.id();
        match self.state.charge_shop_offer(uuid, index) {
            Ok(offer) => {
                player.add_item_or_drop_with_guard(
                    guard,
                    offer.stack(uuid),
                    Prediction::ServerOnly,
                );
                player.send_message(
                    &TextComponent::plain(format!("Куплено: {}.", offer.display_name()))
                        .color(Color::Green),
                );
            }
            Err(error) => {
                player.send_message(&TextComponent::plain(error.message()).color(Color::Red));
            }
        }
        let container_id = ContainerId::from_arc(&self.container);
        if let Some(container) = guard.get_typed_mut::<SimpleContainer>(container_id) {
            for (index, item) in menu_items(&self.state, uuid).into_iter().enumerate() {
                container.set_item(index, item);
            }
        }
        ClickOutcome::Consume
    }
}

#[cfg(test)]
mod tests {
    use super::offer_at;
    use crate::shop::SLOTS;

    #[test]
    fn every_shop_slot_has_one_menu_slot() {
        let mut seen = [false; SLOTS];
        for slot in 0..27 {
            if let Some(index) = offer_at(slot) {
                assert!(!seen[index], "shop slot {index} shown twice");
                seen[index] = true;
            }
        }
        assert!(seen.iter().all(|shown| *shown));
        assert_eq!(offer_at(4), None);
    }
}
