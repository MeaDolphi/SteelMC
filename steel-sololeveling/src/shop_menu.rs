//! Random artifacts and permanent consumables of the System shop.

use std::sync::Arc;

use steel_core::inventory::container::Container as _;
use steel_core::inventory::prelude::*;
use steel_registry::{vanilla_items, vanilla_menu_types};
use steel_utils::locks::IntoShared;
use steel_utils::{DowncastType, DowncastTypeKey};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use crate::item::{append_lore, named, with_lore};
use crate::shop::{self, Offer, ShopDay};
use crate::state::SoloLevelingState;

const MENU_SLOTS: usize = 27;
const BALANCE_SLOT: usize = 4;
const RANDOM_TAB_SLOT: usize = 0;
const PERMANENT_TAB_SLOT: usize = 1;
/// Offers fill the middle of rows two and three, five per row.
const FIRST_ROW_START: usize = 11;
const SECOND_ROW_START: usize = 20;
const PER_ROW: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShopTab {
    Random,
    Permanent,
}

/// Opens the random-artifact shop for an online hunter.
pub fn open(player: &Player, state: Arc<SoloLevelingState>) {
    open_tab(player, state, ShopTab::Random);
}

fn open_tab(player: &Player, state: Arc<SoloLevelingState>, tab: ShopTab) {
    player.open_menu(
        TextComponent::plain("Магазин Системы").color(Color::LightPurple),
        move |context| build(context.container_id, context.player, state, tab),
    );
}

fn build(container_id: u8, player: &Player, state: Arc<SoloLevelingState>, tab: ShopTab) -> Menu {
    let uuid = player.gameprofile.id();
    let view = shop_view(&state, uuid);
    let items = render_items(view, uuid, tab);
    let container = SimpleContainer::from_items(items).into_shared();
    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X3, container_id);
    let buttons = builder.section_all(Arc::clone(&container));
    builder.player_inventory(&player.inventory);
    builder.build(ShopMenuKind {
        state,
        buttons,
        container,
        view,
        tab,
    })
}

/// Returns the shop slot shown at a menu slot in either tab.
fn offer_at(slot: usize) -> Option<usize> {
    let row = |start: usize| {
        (start..start + PER_ROW)
            .contains(&slot)
            .then(|| slot - start)
    };
    row(FIRST_ROW_START).or_else(|| row(SECOND_ROW_START).map(|index| index + PER_ROW))
}

fn shop_view(state: &SoloLevelingState, uuid: Uuid) -> Option<(ShopDay, u64)> {
    let day = state.shop_day(uuid)?;
    let points = state.with_session_mut(uuid, |session| session.system_points.amount())?;
    Some((day, points))
}

fn render_items(view: Option<(ShopDay, u64)>, uuid: Uuid, tab: ShopTab) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); MENU_SLOTS];
    let Some((day, points)) = view else {
        return items;
    };
    items[RANDOM_TAB_SLOT] = tab_button(
        &vanilla_items::NETHER_STAR,
        "Случайные артефакты",
        "10 предложений каждые 20 минут; слот покупается один раз.",
        tab == ShopTab::Random,
    );
    items[PERMANENT_TAB_SLOT] = tab_button(
        &vanilla_items::GOLDEN_APPLE,
        "Постоянные товары",
        "Все расходники и еда; покупать можно повторно.",
        tab == ShopTab::Permanent,
    );
    items[BALANCE_SLOT] = with_lore(
        named(
            ItemStack::new(&vanilla_items::AMETHYST_SHARD),
            TextComponent::plain(format!("Очки Системы: {points}")).color(Color::LightPurple),
        ),
        vec![
            TextComponent::plain("Артефакты меняются каждые 20 минут.").color(Color::Gray),
            TextComponent::plain("Расходники всегда доступны повторно.").color(Color::Gray),
            TextComponent::plain("Покупки привязаны к вам.").color(Color::Gray),
        ],
    );
    match tab {
        ShopTab::Random => {
            for (slot, item) in items.iter_mut().enumerate() {
                if let Some(index) = offer_at(slot) {
                    *item = random_offer_item(&day, index, points, uuid);
                }
            }
        }
        ShopTab::Permanent => {
            for (index, offer) in shop::permanent_offers().into_iter().enumerate() {
                if let Some(slot) = offer_slot(index) {
                    items[slot] = offer_item(offer, points, uuid, false);
                }
            }
        }
    }
    items
}

fn offer_slot(index: usize) -> Option<usize> {
    let row = index / PER_ROW;
    let column = index % PER_ROW;
    (row < 2).then(|| FIRST_ROW_START + row * 9 + column)
}

fn tab_button(
    item: steel_registry::items::ItemRef,
    name: &str,
    description: &str,
    selected: bool,
) -> ItemStack {
    with_lore(
        named(
            ItemStack::new(item),
            TextComponent::plain(name.to_owned()).color(if selected {
                Color::Green
            } else {
                Color::Yellow
            }),
        ),
        vec![
            TextComponent::plain(description.to_owned()).color(Color::Gray),
            TextComponent::plain(if selected {
                "Эта вкладка открыта."
            } else {
                "Клик — переключиться."
            })
            .color(Color::Aqua),
        ],
    )
}

fn random_offer_item(day: &ShopDay, index: usize, points: u64, uuid: Uuid) -> ItemStack {
    let Some(offer) = day.offer(index) else {
        return ItemStack::empty();
    };
    offer_item(offer, points, uuid, day.bought(index))
}

fn offer_item(offer: Offer, points: u64, uuid: Uuid, bought: bool) -> ItemStack {
    if bought {
        return named(
            ItemStack::new(&vanilla_items::GRAY_STAINED_GLASS_PANE),
            TextComponent::plain(format!("Куплено: {}", offer.display_name()))
                .color(Color::DarkGray),
        );
    }
    let mut stack = offer.stack(uuid);
    let affordable = points >= offer.price;
    stack = named(
        stack,
        TextComponent::plain(offer.display_name()).color(if affordable {
            Color::Green
        } else {
            Color::Red
        }),
    );
    append_lore(
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

pub(crate) struct ShopMenuKind {
    state: Arc<SoloLevelingState>,
    buttons: Section,
    container: Shared<SimpleContainer>,
    view: Option<(ShopDay, u64)>,
    tab: ShopTab,
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for ShopMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/shop");
}

impl MenuKind for ShopMenuKind {
    fn on_tick(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        player: &Player,
    ) {
        let uuid = player.gameprofile.id();
        let view = shop_view(&self.state, uuid);
        if self.view == view {
            return;
        }
        let container_id = ContainerId::from_arc(&self.container);
        if let Some(container) = guard.get_typed_mut::<SimpleContainer>(container_id) {
            for (index, item) in render_items(view, uuid, self.tab).into_iter().enumerate() {
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
        if slot == RANDOM_TAB_SLOT {
            open_tab(player, Arc::clone(&self.state), ShopTab::Random);
            return ClickOutcome::Consume;
        }
        if slot == PERMANENT_TAB_SLOT {
            open_tab(player, Arc::clone(&self.state), ShopTab::Permanent);
            return ClickOutcome::Consume;
        }
        let Some(index) = offer_at(slot) else {
            return ClickOutcome::Consume;
        };
        let purchase = match self.tab {
            ShopTab::Random => {
                let Some((displayed, _)) = self.view else {
                    return ClickOutcome::Consume;
                };
                self.state
                    .request_shop_purchase(player, index, true, Some(displayed))
            }
            ShopTab::Permanent => {
                let Some(offer) = shop::permanent_offers().get(index).copied() else {
                    return ClickOutcome::Consume;
                };
                self.state.request_permanent_purchase(player, offer, true)
            }
        };
        match purchase {
            Ok(offer) => {
                player.send_message(
                    &TextComponent::plain(format!(
                        "Покупка «{}» обрабатывается.",
                        offer.display_name()
                    ))
                    .color(Color::Gold),
                );
            }
            Err(error) => {
                player.send_message(&TextComponent::plain(error.message()).color(Color::Red));
            }
        }
        ClickOutcome::Consume
    }
}

#[cfg(test)]
mod tests {
    use super::{FIRST_ROW_START, ShopTab, offer_at, render_items};
    use crate::shop::{self, SLOTS, ShopDay};
    use steel_registry::{init_vanilla_registry, vanilla_items};
    use uuid::Uuid;

    #[test]
    fn every_random_shop_slot_has_one_menu_slot() {
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

    #[test]
    fn committed_purchase_updates_only_its_offer_in_the_menu() {
        init_vanilla_registry();
        let mut day = ShopDay::unset();
        day.roll_over(1);
        let owner = Uuid::new_v4();
        let before = render_items(Some((day, 1000)), owner, ShopTab::Random);
        assert!(day.mark(0));
        let after = render_items(Some((day, 1000)), owner, ShopTab::Random);
        assert!(
            after[FIRST_ROW_START].is(&vanilla_items::GRAY_STAINED_GLASS_PANE),
            "the committed offer is shown as purchased"
        );
        assert_eq!(before[FIRST_ROW_START + 1], after[FIRST_ROW_START + 1]);
    }

    #[test]
    fn permanent_tab_shows_every_consumable_without_purchase_marks() {
        init_vanilla_registry();
        let owner = Uuid::new_v4();
        let items = render_items(Some((ShopDay::unset(), 1_000)), owner, ShopTab::Permanent);
        for (index, offer) in shop::permanent_offers().into_iter().enumerate() {
            let slot = super::offer_slot(index).expect("permanent offer slot");
            assert!(!items[slot].is_empty(), "missing {}", offer.display_name());
        }
    }
}
