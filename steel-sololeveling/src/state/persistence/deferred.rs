//! Atomic System loot delivery and deferred-inventory withdrawals.

use std::{io, sync::Arc};

use steel_core::player::Player;
use steel_core::player::player_data_storage::transaction::{
    ExtensionStorageWrite, PlayerStorageTransaction, TransactionCommit,
};
use steel_core::server::Server;
use steel_registry::item_stack::ItemStack;
use text_components::TextComponent;
use uuid::Uuid;

use super::{Request, account, load_account, stage_inventory_item};
use crate::dungeon::is_instance_world;
use crate::item::nbt;
use crate::state::SoloLevelingState;

#[derive(Debug)]
pub(super) enum DeferredTransfer {
    Deliver(ItemStack),
    Withdraw { index: usize, expected: Vec<u8> },
}

impl SoloLevelingState {
    pub(crate) fn request_system_loot(&self, player: &Player, stack: ItemStack) {
        self.queue_system_loot(player.gameprofile.id(), stack);
    }

    pub(crate) fn queue_system_loot(&self, uuid: Uuid, stack: ItemStack) {
        if stack.is_empty() {
            return;
        }
        let mut persistence = self.persistence.lock();
        if !persistence.accepts_writes(uuid) {
            log::error!("sololeveling: deferred loot delivery fenced for {uuid}");
            return;
        }
        persistence.requests.push_back(Request::Deferred {
            uuid,
            transfer: DeferredTransfer::Deliver(stack),
        });
    }

    pub(crate) fn request_deferred_withdrawal(
        &self,
        player: &Player,
        index: usize,
        expected: Vec<u8>,
    ) {
        let uuid = player.gameprofile.id();
        if self.sessions.with_mut(uuid, |session| {
            if session.economic_busy {
                return false;
            }
            session.economic_busy = true;
            true
        }) != Some(true)
        {
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
        persistence.requests.push_back(Request::Deferred {
            uuid,
            transfer: DeferredTransfer::Withdraw { index, expected },
        });
    }

    #[expect(
        clippy::too_many_lines,
        reason = "keeps validation and publication of one atomic inventory/account transaction together"
    )]
    pub(super) fn begin_deferred_transfer(
        self: &Arc<Self>,
        server: &Arc<Server>,
        player: Arc<Player>,
        uuid: Uuid,
        transfer: DeferredTransfer,
    ) -> bool {
        let reopen_menu = matches!(&transfer, DeferredTransfer::Withdraw { .. });
        let fail = |message: &str| {
            if reopen_menu {
                self.sessions
                    .with_mut(uuid, |session| session.economic_busy = false);
            }
            if reopen_menu {
                self.defer_deferred_menu(uuid);
            }
            player.send_message(&TextComponent::plain(message.to_owned()));
            false
        };
        if reopen_menu && is_instance_world(&player.get_world()) {
            return fail("Хранилище недоступно в подземелье.");
        }
        if !self.persistence.lock().accepts_writes(uuid) {
            return fail("Хранилище недоступно.");
        }
        let Some((mut snapshot, mut storage)) = self.sessions.with_mut(uuid, |session| {
            (session.saved_data(uuid), session.deferred_inventory.clone())
        }) else {
            return fail("Данные охотника не загружены.");
        };
        let expected = self.persistence.lock().expected.get(&uuid).cloned();
        let Some(expected) = expected else {
            return fail("Данные охотника не загружены.");
        };
        let Ok(mut reservation) = player.reserve_stored_inventory() else {
            return match transfer {
                DeferredTransfer::Deliver(item) => {
                    self.begin_offline_deferred_delivery(server, uuid, item)
                }
                DeferredTransfer::Withdraw { .. } => fail("Инвентарь занят. Повторите позже."),
            };
        };
        let mut loot_notification = None;
        match transfer {
            DeferredTransfer::Deliver(item) => {
                loot_notification = Some(item.clone());
                if stage_inventory_item(&mut reservation, item.clone()).is_err()
                    && defer_stack(item, &mut storage).is_err()
                {
                    return fail("Не удалось сохранить предмет.");
                }
            }
            DeferredTransfer::Withdraw {
                index,
                expected: selected,
            } => {
                let Some(bytes) = storage.get(index) else {
                    return fail("Предмет уже изменился.");
                };
                if bytes != &selected {
                    return fail("Предмет уже изменился.");
                }
                let Ok(item) = nbt::decode(bytes) else {
                    return fail("Этот предмет не поддерживается текущей версией.");
                };
                if stage_inventory_item(&mut reservation, item).is_err() {
                    return fail("В инвентаре недостаточно места.");
                }
                storage.remove(index);
            }
        }
        snapshot.deferred_inventory.clone_from(&storage);
        let Ok(replacement) = serde_json::to_vec(&snapshot) else {
            return fail("Не удалось подготовить данные.");
        };
        let committed = replacement.clone();
        let state = Arc::clone(self);
        let completion_player = Arc::clone(&player);
        if server
            .submit_stored_inventory_transaction(
                vec![reservation],
                vec![ExtensionStorageWrite {
                    key: account::key(uuid),
                    expected,
                    replacement,
                }],
                move |result| {
                    if let Ok(commit) = result {
                        if let Some(stack) = &loot_notification {
                            state.notify_loot(uuid, stack);
                        }
                        state
                            .persistence
                            .lock()
                            .expected
                            .insert(uuid, Some(committed));
                        state.sessions.with_mut(uuid, |session| {
                            session.deferred_inventory = storage;
                            if reopen_menu {
                                session.economic_busy = false;
                            }
                        });
                        if reopen_menu {
                            state.defer_deferred_menu(uuid);
                        }
                        if commit == TransactionCommit::DurabilityUncertain {
                            completion_player.send_message(&TextComponent::plain(
                                "Предмет передан, но подтверждение диска не получено.",
                            ));
                        }
                    } else {
                        log::error!(
                            "sololeveling: deferred inventory commit failed for {uuid}: {result:?}"
                        );
                        let conflicted = result
                            .as_ref()
                            .err()
                            .is_some_and(|error| error.kind() == io::ErrorKind::WouldBlock);
                        if !conflicted {
                            state.persistence.lock().failed.insert(uuid);
                        }
                        if reopen_menu {
                            state
                                .sessions
                                .with_mut(uuid, |session| session.economic_busy = false);
                        }
                        if reopen_menu {
                            state.defer_deferred_menu(uuid);
                        }
                        completion_player.send_message(&TextComponent::plain(if conflicted {
                            "Данные изменились; повторите попытку позже."
                        } else {
                            "Передача не сохранена; предметы не изменены."
                        }));
                    }
                    state.persistence.lock().busy = false;
                },
            )
            .is_err()
        {
            return fail("Не удалось начать сохранение.");
        }
        true
    }

    pub(super) fn begin_offline_deferred_delivery(
        self: &Arc<Self>,
        server: &Arc<Server>,
        uuid: Uuid,
        item: ItemStack,
    ) -> bool {
        if !self.persistence.lock().accepts_writes(uuid) {
            log::error!("sololeveling: offline deferred loot delivery fenced for {uuid}");
            return false;
        }
        let state = Arc::clone(self);
        let server_task = Arc::clone(server);
        let completion_server = Arc::clone(&server_task);
        let legacy = self.storage.clone();
        let session_snapshot = self
            .sessions
            .with_mut(uuid, |session| session.saved_data(uuid));
        let session_expected = self.persistence.lock().expected.get(&uuid).cloned();
        server.jobs.spawn_io(
            async move {
                let (mut snapshot, expected) =
                    if let (Some(snapshot), Some(expected)) = (session_snapshot, session_expected)
                    {
                        (snapshot, expected)
                    } else {
                        let loaded =
                            load_account(&server_task.player_data_storage, legacy, uuid).await?;
                        (loaded.data, loaded.bytes)
                    };
                snapshot.deferred_inventory.push(nbt::encode(&item)?);
                let deferred_inventory = snapshot.deferred_inventory.clone();
                let replacement = serde_json::to_vec(&snapshot).map_err(io::Error::other)?;
                let mut transaction = PlayerStorageTransaction {
                    players: Vec::new(),
                    extensions: vec![ExtensionStorageWrite {
                        key: account::key(uuid),
                        expected,
                        replacement: replacement.clone(),
                    }],
                };
                let commit = account::commit_with_conflict_retry(
                    &server_task.player_data_storage,
                    &mut transaction,
                )
                .await?;
                Ok::<_, io::Error>((replacement, deferred_inventory, commit))
            },
            move |result| {
                let mut persistence = state.persistence.lock();
                match result {
                    Ok(Ok((replacement, deferred_inventory, _commit))) => {
                        persistence.expected.insert(uuid, Some(replacement));
                        persistence.busy = false;
                        drop(persistence);
                        state.sessions.with_mut(uuid, |session| {
                            session.deferred_inventory = deferred_inventory;
                        });
                    }
                    Ok(Err(error)) => {
                        log::error!(
                            "sololeveling: offline deferred loot delivery failed for {uuid}: {error}"
                        );
                        persistence.failed.insert(uuid);
                        persistence.busy = false;
                    }
                    Err(error) => {
                        log::error!(
                            "sololeveling: offline deferred loot task failed for {uuid}: {error}"
                        );
                        persistence.failed.insert(uuid);
                        persistence.busy = false;
                    }
                }
                if let Some(player) = completion_server
                    .get_players()
                    .into_iter()
                    .find(|player| player.gameprofile.id() == uuid)
                {
                    player.send_message(&TextComponent::plain(
                        "Системная награда сохранена в Хранилище.",
                    ));
                }
            },
        );
        true
    }

    pub(crate) fn deferred_items(&self, uuid: Uuid) -> Vec<(usize, Vec<u8>, ItemStack)> {
        self.sessions
            .with_mut(uuid, |session| {
                session
                    .deferred_inventory
                    .iter()
                    .enumerate()
                    .filter_map(|(index, bytes)| {
                        nbt::decode(bytes)
                            .ok()
                            .map(|item| (index, bytes.clone(), item))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn defer_deferred_menu(&self, uuid: Uuid) {
        self.defer_deferred_menu_open(uuid, 0);
    }
}

fn defer_stack(item: ItemStack, storage: &mut Vec<Vec<u8>>) -> io::Result<()> {
    storage.push(nbt::encode(&item)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_items};

    use super::defer_stack;
    use crate::item::{binding, nbt};
    use steel_registry::item_stack::ItemStack;
    use uuid::Uuid;

    #[test]
    fn deferred_overflow_keeps_the_complete_bound_stack() {
        init_vanilla_registry();
        let owner = Uuid::new_v4();
        let mut stack = ItemStack::new(&vanilla_items::DIAMOND);
        stack.count = 17;
        binding::bind(&mut stack, owner);

        let mut storage = Vec::new();
        defer_stack(stack.clone(), &mut storage).expect("stack should encode");
        let restored = nbt::decode(&storage[0]).expect("stored stack should decode");
        assert!(ItemStack::matches(&restored, &stack));
        assert_eq!(binding::bound_owner(&restored), Some(owner));
        assert_eq!(restored.count, 17);
    }
}
