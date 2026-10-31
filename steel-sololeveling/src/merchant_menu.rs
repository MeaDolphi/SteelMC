//! The city merchant as a chest menu.
//!
//! The top row shows the hunter's Won and when each showcase changes. Rows
//! two and three hold the shared showcase: the starter replacements, then
//! the rotating specialization pieces. Row five holds the personal showcase.
//! Clicking an offer buys it; the menu redraws whenever a showcase, the
//! balance or a purchase mark changes.

use std::sync::Arc;

use steel_core::inventory::container::Container as _;
use steel_core::inventory::prelude::*;
use steel_registry::{vanilla_items, vanilla_menu_types};
use steel_utils::locks::IntoShared;
use steel_utils::{DowncastType, DowncastTypeKey};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use crate::crafting_menu;
use crate::item::serial;
use crate::item::{append_lore, named, with_lore};
use crate::merchant::{
    self, MerchantOffer, MerchantRecord, PERSONAL_DISCOUNT_PERCENT, PERSONAL_PERIOD_SECONDS,
    SHARED_PERIOD_SECONDS, Showcase,
};
use crate::rank::HunterRank;
use crate::state::SoloLevelingState;
use crate::ui::{self, After};

const MENU_SLOTS: usize = 54;
const INFO_SLOT: usize = 4;
const SHARED_LABEL_SLOT: usize = 9;
/// Menu slots of the shared showcase, in showcase order.
const SHARED_SLOTS: [usize; 14] = [10, 11, 12, 13, 14, 15, 16, 17, 19, 20, 21, 22, 23, 24];
const PERSONAL_LABEL_SLOT: usize = 36;
/// Menu slots of the personal showcase, in showcase order.
const PERSONAL_SLOTS: [usize; 3] = [38, 40, 42];
/// Button that opens the crafting orders.
const ORDERS_SLOT: usize = 49;
/// Button that opens the buyback of materials and spare artifacts.
const BUYBACK_SLOT: usize = 47;

/// Opens the merchant for a hunter at the board.
pub fn open(player: &Player, state: Arc<SoloLevelingState>) {
    player.open_menu(
        TextComponent::plain("Городской торговец").color(Color::Gold),
        move |context| build(context.container_id, context.player, state),
    );
}

/// Everything the menu shows; a change redraws it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct View {
    shared: Vec<MerchantOffer>,
    personal: Vec<MerchantOffer>,
    won: u64,
    rank: HunterRank,
    record: MerchantRecord,
    shared_minutes: u64,
    personal_minutes: u64,
}

fn view(state: &SoloLevelingState, uuid: Uuid) -> Option<View> {
    let (won, rank, record) = state.with_session_mut(uuid, |session| {
        (session.won.amount(), session.stats.rank(), session.merchant)
    })?;
    let (shared, personal) = state.merchant_offers(uuid);
    let now = merchant::unix_now();
    Some(View {
        shared,
        personal,
        won,
        rank,
        record,
        shared_minutes: merchant::seconds_left(now, SHARED_PERIOD_SECONDS).div_ceil(60),
        personal_minutes: merchant::seconds_left(now, PERSONAL_PERIOD_SECONDS).div_ceil(60),
    })
}

fn build(container_id: u8, player: &Player, state: Arc<SoloLevelingState>) -> Menu {
    let view = view(&state, player.gameprofile.id());
    let container = SimpleContainer::from_items(menu_items(view.as_ref())).into_shared();
    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X6, container_id);
    let buttons = builder.section_all(Arc::clone(&container));
    builder.player_inventory(&player.inventory);
    builder.build(MerchantMenuKind {
        state,
        buttons,
        container,
        view,
    })
}

fn offer_at(view: &View, slot: usize) -> Option<MerchantOffer> {
    if let Some(index) = SHARED_SLOTS.iter().position(|shown| *shown == slot) {
        return view.shared.get(index).copied();
    }
    let index = PERSONAL_SLOTS.iter().position(|shown| *shown == slot)?;
    view.personal.get(index).copied()
}

fn menu_items(view: Option<&View>) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); MENU_SLOTS];
    let Some(view) = view else {
        return items;
    };
    items[INFO_SLOT] = with_lore(
        named(
            ItemStack::new(&vanilla_items::GOLD_INGOT),
            TextComponent::plain(format!("Ваши Won: {}", view.won)).color(Color::Gold),
        ),
        vec![
            TextComponent::plain("Покупки можно передавать и продавать.").color(Color::Gray),
            TextComponent::plain("Купленные вещи изнашиваются как обычные.").color(Color::Gray),
            TextComponent::plain("Броню надевают в снаряжении Системы.").color(Color::Gray),
        ],
    );
    items[SHARED_LABEL_SLOT] = with_lore(
        named(
            ItemStack::new(&vanilla_items::CHEST),
            TextComponent::plain("Общая витрина").color(Color::Yellow),
        ),
        vec![
            TextComponent::plain("Одна для всех охотников, меняется раз в час.").color(Color::Gray),
            TextComponent::plain(format!("До обновления: {} мин.", view.shared_minutes))
                .color(Color::Aqua),
        ],
    );
    items[PERSONAL_LABEL_SLOT] = with_lore(
        named(
            ItemStack::new(&vanilla_items::ENDER_CHEST),
            TextComponent::plain("Персональная витрина").color(Color::LightPurple),
        ),
        vec![
            TextComponent::plain("Броня вашего класса, меняется каждые 20 минут.")
                .color(Color::Gray),
            TextComponent::plain(format!(
                "Скидка {PERSONAL_DISCOUNT_PERCENT}%, каждое предложение — один раз."
            ))
            .color(Color::Gray),
            TextComponent::plain(format!("До обновления: {} мин.", view.personal_minutes))
                .color(Color::Aqua),
        ],
    );
    items[ORDERS_SLOT] = with_lore(
        named(
            ItemStack::new(&vanilla_items::SMITHING_TABLE),
            TextComponent::plain("Заказы у мастера").color(Color::Gold),
        ),
        vec![
            TextComponent::plain("Улучшенные вещи из материалов данжей.").color(Color::Gray),
            TextComponent::plain("Клик — открыть заказы.").color(Color::Yellow),
        ],
    );
    items[BUYBACK_SLOT] = with_lore(
        named(
            ItemStack::new(&vanilla_items::GOLD_INGOT),
            TextComponent::plain("Скупка").color(Color::Gold),
        ),
        vec![
            TextComponent::plain("Торговец покупает материалы, эссенции").color(Color::Gray),
            TextComponent::plain("и ненужные артефакты без привязки.").color(Color::Gray),
            TextComponent::plain("Клик — выбрать, что продать.").color(Color::Yellow),
        ],
    );
    for (slot, offer) in SHARED_SLOTS.into_iter().zip(&view.shared) {
        items[slot] = offer_item(view, *offer);
    }
    for (slot, offer) in PERSONAL_SLOTS.into_iter().zip(&view.personal) {
        items[slot] = offer_item(view, *offer);
    }
    items
}

fn offer_item(view: &View, offer: MerchantOffer) -> ItemStack {
    if offer.showcase == Showcase::Personal && view.record.bought(offer.period, offer.index) {
        return named(
            ItemStack::new(&vanilla_items::GRAY_STAINED_GLASS_PANE),
            TextComponent::plain(format!("Куплено: {}", offer.item.serial().name))
                .color(Color::DarkGray),
        );
    }
    let (status, color) = if !merchant::may_buy(offer.item, view.rank) {
        ("Продаётся охотникам с ранга D.", Color::Red)
    } else if view.won < offer.price {
        ("Не хватает Won.", Color::Red)
    } else {
        ("Клик — купить.", Color::Yellow)
    };
    append_lore(
        serial::stack(offer.item, None),
        vec![
            TextComponent::plain(format!("Цена: {} Won", offer.price)).color(Color::Gold),
            TextComponent::plain(status).color(color),
        ],
    )
}

pub(crate) struct MerchantMenuKind {
    state: Arc<SoloLevelingState>,
    buttons: Section,
    container: Shared<SimpleContainer>,
    view: Option<View>,
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for MerchantMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/merchant");
}

impl MenuKind for MerchantMenuKind {
    fn on_tick(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        player: &Player,
    ) {
        if player.tick_count() % 20 != 0 {
            return;
        }
        let view = view(&self.state, player.gameprofile.id());
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
        if slot - self.buttons.start() == ORDERS_SLOT {
            crafting_menu::open(player, Arc::clone(&self.state));
            return ClickOutcome::Consume;
        }
        // The picker reads the inventory, which this click has locked.
        if slot - self.buttons.start() == BUYBACK_SLOT {
            self.state.defer_buyback_menu(player.gameprofile.id());
            return ClickOutcome::Consume;
        }
        let Some(offer) = self
            .view
            .as_ref()
            .and_then(|view| offer_at(view, slot - self.buttons.start()))
        else {
            return ClickOutcome::Consume;
        };
        match self.state.request_merchant_purchase(player, offer) {
            Ok(()) => player.send_message(
                &TextComponent::plain(format!(
                    "Покупка «{}» обрабатывается.",
                    offer.item.serial().name
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

/// Opens the buyback: every stack the merchant would buy, with its price.
///
/// It reads the inventory, so the caller runs outside any menu callback.
pub fn open_buyback(player: &Player, state: Arc<SoloLevelingState>) {
    let mut choices: Vec<(usize, ItemStack)> = {
        let inventory = player.inventory.lock();
        (0..36)
            .map(|slot| (slot, inventory.get_item(slot).clone()))
            .filter(|(_, stack)| merchant::buyback_price(stack).is_some())
            .collect()
    };
    if choices.is_empty() {
        ui::tell(
            player,
            "Торговцу нечего продать: материалы, эссенции и артефакты без привязки.",
            Color::Red,
        );
        return;
    }
    let items = buyback_items(&choices);
    ui::open_buttons(
        player,
        TextComponent::plain("Скупка").color(Color::Gold),
        if choices.len() > 27 { 6 } else { 3 },
        items,
        Box::new(move |player, index, _| {
            if index >= choices.len() {
                return After::Keep;
            }
            let (slot, stack) = choices[index].clone();
            match state.request_buyback(player, slot, stack) {
                Ok(_) => {
                    choices.remove(index);
                }
                Err(message) => ui::tell(player, message, Color::Red),
            }
            let mut items = buyback_items(&choices);
            items.resize(if choices.len() > 27 { 54 } else { 27 }, ItemStack::empty());
            After::Redraw(items)
        }),
    );
}

fn buyback_items(choices: &[(usize, ItemStack)]) -> Vec<ItemStack> {
    choices
        .iter()
        .map(|(_, stack)| {
            append_lore(
                stack.clone(),
                vec![
                    TextComponent::plain(format!(
                        "Торговец даст: {} Won",
                        merchant::buyback_price(stack).unwrap_or(0)
                    ))
                    .color(Color::Gold),
                    TextComponent::plain("Клик — продать весь стак.").color(Color::Yellow),
                ],
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;
    use uuid::Uuid;

    use super::{PERSONAL_SLOTS, SHARED_SLOTS, View, menu_items, offer_at};
    use crate::class::HunterClass;
    use crate::item::serial;
    use crate::merchant::{self, MerchantRecord};
    use crate::rank::HunterRank;

    fn sample() -> View {
        View {
            shared: merchant::shared_showcase(3),
            personal: merchant::personal_showcase(Uuid::new_v4(), Some(HunterClass::Tanker), 9),
            won: 1_000,
            rank: HunterRank::D,
            record: MerchantRecord::default(),
            shared_minutes: 10,
            personal_minutes: 5,
        }
    }

    #[test]
    fn every_offer_has_one_menu_slot() {
        let view = sample();
        assert_eq!(view.shared.len(), SHARED_SLOTS.len());
        for (slot, offer) in SHARED_SLOTS.into_iter().zip(&view.shared) {
            assert_eq!(offer_at(&view, slot), Some(*offer));
        }
        for (slot, offer) in PERSONAL_SLOTS.into_iter().zip(&view.personal) {
            assert_eq!(offer_at(&view, slot), Some(*offer));
        }
        assert_eq!(offer_at(&view, 4), None);
    }

    #[test]
    fn a_bought_personal_offer_is_greyed_out() {
        init_vanilla_registry();
        let mut view = sample();
        let first = view.personal[0];
        assert_eq!(
            serial::of(&menu_items(Some(&view))[PERSONAL_SLOTS[0]]),
            Some(first.item)
        );
        view.record.mark(first.period, first.index);
        assert_eq!(
            serial::of(&menu_items(Some(&view))[PERSONAL_SLOTS[0]]),
            None
        );
    }
}
