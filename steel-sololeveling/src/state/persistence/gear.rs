//! Atomic transfers between stored inventory and System equipment.

use std::{io, mem, sync::Arc};

use steel_core::player::Player;
use steel_core::player::player_data_storage::transaction::{
    ExtensionStorageWrite, TransactionCommit,
};
use steel_core::server::Server;
use steel_registry::item_stack::ItemStack;
use text_components::TextComponent;
use uuid::Uuid;

use super::{Request, account, stage_inventory_item};
use crate::gear::{self, GearTransfer};
use crate::item::{binding, nbt};
use crate::player_data::SoloLevelingPlayerData;
use crate::state::SoloLevelingState;

impl SoloLevelingState {
    pub(crate) fn request_gear_transfer(&self, player: &Player, transfer: GearTransfer) {
        let uuid = player.gameprofile.id();
        let accepted = self.sessions.with_mut(uuid, |session| {
            if session.economic_busy {
                return false;
            }
            session.economic_busy = true;
            true
        });
        if accepted != Some(true) {
            player.send_message(&TextComponent::plain("Данные заняты, повторите позже."));
            return;
        }
        let mut persistence = self.persistence.lock();
        if !persistence.accepts_writes(uuid) {
            drop(persistence);
            self.sessions
                .with_mut(uuid, |session| session.economic_busy = false);
            player.send_message(&TextComponent::plain("Хранилище недоступно."));
            return;
        }
        persistence
            .requests
            .push_back(Request::Gear { uuid, transfer });
    }

    #[expect(
        clippy::too_many_lines,
        reason = "keeps staged gear validation and its single atomic commit together"
    )]
    pub(super) fn begin_gear_transfer(
        self: &Arc<Self>,
        server: &Arc<Server>,
        player: Arc<Player>,
        uuid: Uuid,
        transfer: GearTransfer,
    ) -> bool {
        let fail = |message: &str| {
            self.release_shop_purchase(uuid);
            self.defer_gear_menu(uuid);
            player.send_message(&TextComponent::plain(message.to_owned()));
            false
        };
        if !self.persistence.lock().accepts_writes(uuid) {
            return fail("Хранилище недоступно.");
        }
        let Some((mut snapshot, mut items)) = self.sessions.with_mut(uuid, |session| {
            (session.saved_data(uuid), session.gear.clone())
        }) else {
            return fail("Данные охотника не загружены.");
        };
        items.resize(gear::GEAR_SLOTS, ItemStack::empty());
        let expected = self.persistence.lock().expected.get(&uuid).cloned();
        let Some(expected) = expected else {
            return fail("Данные охотника не загружены.");
        };
        if let Some(bytes) = expected.as_deref() {
            let Ok(stored) = serde_json::from_slice::<SoloLevelingPlayerData>(bytes) else {
                return fail("Не удалось проверить сохранённое снаряжение.");
            };
            for (index, slot) in stored.gear.slots.iter().enumerate() {
                if let Some(bytes) = slot {
                    let Ok(item) = nbt::decode(bytes) else {
                        return fail("Не удалось прочитать сохранённое снаряжение.");
                    };
                    if items
                        .get(index)
                        .is_none_or(|current| !ItemStack::matches(current, &item))
                    {
                        return fail("Снаряжение изменилось; повторите позже.");
                    }
                } else if items.get(index).is_some_and(|item| !item.is_empty()) {
                    return fail("Снаряжение изменилось; повторите позже.");
                }
            }
            if items
                .iter()
                .skip(stored.gear.slots.len())
                .any(|item| !item.is_empty())
            {
                return fail("Снаряжение изменилось; повторите позже.");
            }
        }
        let Ok(mut reservation) = player.reserve_stored_inventory() else {
            return fail("Инвентарь занят. Повторите позже.");
        };
        match transfer {
            GearTransfer::Equip { slot, expected } => {
                let Ok(item) = reservation.item(slot) else {
                    return fail("Переключите активный слот руки и повторите.");
                };
                if !ItemStack::matches(&item, &expected) || binding::bound_to_other(&item, uuid) {
                    return fail("Предмет изменился или принадлежит другому охотнику.");
                }
                if gear::kind_of(&item).is_none() {
                    return fail("Этот предмет нельзя экипировать.");
                }
                if !gear::wearable(&item, self.class_of(uuid)) {
                    return fail("Эту броню может носить только указанный в описании класс.");
                }
                let Some(index) = gear::free_slot(&items, &item) else {
                    return fail("Нет свободного подходящего слота снаряжения.");
                };
                if reservation.set_item(slot, ItemStack::empty()).is_err() {
                    return fail("Не удалось подготовить инвентарь.");
                }
                items[index] = item;
            }
            GearTransfer::Unequip { index, expected } => {
                let Some(item) = items.get(index) else {
                    return fail("Слот снаряжения недоступен.");
                };
                if item.is_empty()
                    || !ItemStack::matches(item, &expected)
                    || binding::bound_to_other(item, uuid)
                {
                    return fail("Предмет изменился или принадлежит другому охотнику.");
                }
                if stage_inventory_item(&mut reservation, item.clone()).is_err() {
                    return fail("В инвентаре недостаточно места.");
                }
                items[index] = ItemStack::empty();
            }
        }
        let mut slots = Vec::with_capacity(items.len());
        for item in &items {
            if item.is_empty() {
                slots.push(None);
            } else if let Ok(bytes) = nbt::encode(item) {
                slots.push(Some(bytes));
            } else {
                return fail("Не удалось подготовить снаряжение к сохранению.");
            }
        }
        snapshot.gear.slots = slots;
        let Ok(replacement) = serde_json::to_vec(&snapshot) else {
            return fail("Не удалось подготовить данные.");
        };
        let committed_bytes = replacement.clone();
        let committed_gear = mem::take(&mut snapshot.gear);
        let state = Arc::clone(self);
        let completion_player = Arc::clone(&player);
        let result = server.submit_stored_inventory_transaction(
            vec![reservation],
            vec![ExtensionStorageWrite {
                key: account::key(uuid),
                expected,
                replacement,
            }],
            move |result| {
                if let Ok(commit) = result {
                    state
                        .persistence
                        .lock()
                        .expected
                        .insert(uuid, Some(committed_bytes));
                    state.sessions.with_mut(uuid, |session| {
                        session.gear = items;
                        session.saved_gear = committed_gear;
                        session.gear_dirty = true;
                        session.economic_busy = false;
                    });
                    state.defer_gear_menu(uuid);
                    if let Err(error) = state.save_session(uuid) {
                        log::warn!("sololeveling: gear follow-up save failed: {error}");
                    }
                    let message = if commit == TransactionCommit::DurabilityUncertain {
                        "Снаряжение изменено, но подтверждение диска не получено."
                    } else {
                        "Снаряжение изменено."
                    };
                    completion_player.send_message(&TextComponent::plain(message));
                } else {
                    log::error!("sololeveling: gear persistence failed for {uuid}: {result:?}");
                    let conflicted = result
                        .as_ref()
                        .err()
                        .is_some_and(|error| error.kind() == io::ErrorKind::WouldBlock);
                    if !conflicted {
                        state.persistence.lock().failed.insert(uuid);
                    }
                    state
                        .sessions
                        .with_mut(uuid, |session| session.economic_busy = false);
                    state.defer_gear_menu(uuid);
                    completion_player.send_message(&TextComponent::plain(if conflicted {
                        "Данные изменились; повторите попытку позже."
                    } else {
                        "Перенос не сохранён; предметы не изменены."
                    }));
                }
                state.persistence.lock().busy = false;
            },
        );
        if result.is_err() {
            return fail("Не удалось начать сохранение.");
        }
        true
    }
}
