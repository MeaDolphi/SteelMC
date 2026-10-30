//! Selling one stack to the city merchant: the stack leaves the inventory
//! and the Won lands on the account in one transaction.

use std::sync::Arc;

use steel_core::inventory::container::Container as _;
use steel_core::player::Player;
use steel_core::player::player_data_storage::transaction::ExtensionStorageWrite;
use steel_core::server::Server;
use steel_registry::item_stack::ItemStack;
use text_components::TextComponent;
use uuid::Uuid;

use super::{Request, account};
use crate::merchant;
use crate::state::SoloLevelingState;
use crate::state::auction::park_main_hand_stack;

impl SoloLevelingState {
    /// Queues the sale of the stack `seen` in inventory `slot`.
    pub fn request_buyback(
        &self,
        player: &Player,
        slot: usize,
        seen: ItemStack,
    ) -> Result<u64, &'static str> {
        let uuid = player.gameprofile.id();
        let price = merchant::buyback_price(&seen).ok_or("Торговец это не покупает.")?;
        if !self.persistence.lock().accepts_writes(uuid) {
            return Err("Данные охотника сейчас недоступны для записи.");
        }
        let accepted = self
            .sessions
            .with_mut(uuid, |session| {
                if session.economic_busy {
                    return false;
                }
                session.economic_busy = true;
                true
            })
            .unwrap_or(false);
        if !accepted {
            return Err("Предыдущая операция ещё обрабатывается.");
        }
        self.persistence
            .lock()
            .requests
            .push_back(Request::Buyback { uuid, slot, seen });
        Ok(price)
    }

    pub(super) fn begin_buyback(
        self: &Arc<Self>,
        server: &Arc<Server>,
        player: Arc<Player>,
        uuid: Uuid,
        slot: usize,
        seen: &ItemStack,
    ) -> bool {
        let fail = |message: &'static str| {
            self.release_shop_purchase(uuid);
            player.send_message(&TextComponent::plain(message));
            false
        };
        let (current, selected) = {
            let inventory = player.inventory.lock();
            (
                inventory.get_item(slot).clone(),
                usize::from(inventory.get_selected_slot()),
            )
        };
        if current != *seen {
            return fail("Предмет сдвинулся: выберите его заново.");
        }
        let Some(price) = merchant::buyback_price(&current) else {
            return fail("Торговец это не покупает.");
        };
        // A durable transaction cannot own the equipped hand.
        let slot = if slot == selected {
            match park_main_hand_stack(&player) {
                Some(slot) => slot,
                None => return fail("Освободите место в инвентаре для продажи."),
            }
        } else {
            slot
        };
        let Ok(mut reservation) = player.reserve_stored_inventory() else {
            return fail("Инвентарь занят: повторите через мгновение.");
        };
        if reservation.set_item(slot, ItemStack::empty()).is_err() {
            return fail("Не удалось подготовить предмет к продаже.");
        }
        let Some(mut snapshot) = self
            .sessions
            .with_mut(uuid, |session| session.saved_data(uuid))
        else {
            return fail("Данные охотника не загружены.");
        };
        if !snapshot.won.try_credit(price) {
            return fail("Баланс Won переполнен; продажа отменена.");
        }
        let balance = snapshot.won;
        let Some(expected) = self.persistence.lock().expected.get(&uuid).cloned() else {
            return fail("Данные охотника не загружены.");
        };
        let Ok(replacement) = serde_json::to_vec(&snapshot) else {
            return fail("Продажу не удалось подготовить.");
        };
        let committed_bytes = replacement.clone();
        let state = Arc::clone(self);
        let completion_player = Arc::clone(&player);
        let transaction = server.submit_stored_inventory_transaction(
            vec![reservation],
            vec![ExtensionStorageWrite {
                key: account::key(uuid),
                expected,
                replacement,
            }],
            move |result| {
                if result.is_ok() {
                    state
                        .persistence
                        .lock()
                        .expected
                        .insert(uuid, Some(committed_bytes));
                    state.sessions.with_mut(uuid, |session| {
                        session.won = balance;
                        session.economic_busy = false;
                    });
                    completion_player.send_message(&TextComponent::plain(format!(
                        "Торговец заплатил {price} Won."
                    )));
                } else {
                    log::error!("sololeveling: merchant buyback failed: {result:?}");
                    state
                        .sessions
                        .with_mut(uuid, |session| session.economic_busy = false);
                    completion_player.send_message(&TextComponent::plain(
                        "Продажа не сохранена; предмет и баланс не изменены.",
                    ));
                }
                state.persistence.lock().busy = false;
            },
        );
        if let Err(error) = transaction {
            log::error!("sololeveling: could not start merchant buyback: {error}");
            return fail("Продажу не удалось сохранить.");
        }
        true
    }
}
