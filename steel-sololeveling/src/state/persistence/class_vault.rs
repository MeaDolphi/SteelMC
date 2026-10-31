//! The per-hunter class vault: personal class items freeze here while the
//! hunter's class cannot use them, and come back once a suitable class is
//! chosen.
//!
//! The vault stores raw item NBT in the hunter's account document, like the
//! deferred inventory, but it is class-gated: only the reconcile moves items
//! in, and only a class that may use the item takes them out. Both directions
//! are one durable inventory+account transaction, like a buyback in reverse.
//!
//! The reconcile covers stored inventory slots (0..36) plus a parked main
//! hand; equipped armor is not transactional stock (see
//! `StoredInventoryReservation`), and currently no bound class item can be
//! worn, so nothing class-bound can stay equipped.

use std::io::ErrorKind;
use std::sync::Arc;

use steel_core::inventory::container::Container as _;
use steel_core::player::Player;
use steel_core::player::player_data_storage::transaction::{
    ExtensionStorageWrite, TransactionCommit,
};
use steel_core::player::player_inventory::PlayerInventory;
use steel_core::server::Server;
use steel_registry::item_stack::ItemStack;
use steel_utils::types::InteractionHand;
use text_components::TextComponent;
use text_components::format::Color;
use uuid::Uuid;

use super::{Request, account, stage_inventory_item};
use crate::class::{self, HunterClass};
use crate::dungeon::is_instance_world;
use crate::item::{grimoire, nbt};
use crate::state::SoloLevelingState;
use crate::state::auction::park_main_hand_stack;
use crate::ui;

#[derive(Debug)]
pub(super) enum ClassVaultTransfer {
    Withdraw { index: usize, expected: Vec<u8> },
}

/// The inventory side of one class-vault reconcile.
#[derive(Debug, Default, PartialEq)]
pub(super) struct ReconcilePlan {
    /// Stored inventory slots to clear, with the stack each held.
    frozen: Vec<(usize, ItemStack)>,
    /// Vault entries to hand back, with their vault index.
    returns: Vec<(usize, ItemStack)>,
    /// Vault entries that stay stored: undecodable, still frozen for the
    /// class, or the hunter already carries a grimoire.
    keep: Vec<Vec<u8>>,
}

/// Decides one reconcile without touching the world.
///
/// `inventory` is the stored inventory (36 slots; the selected slot is never
/// transactional stock and reads empty after parking). `held_grimoire` covers
/// the main hand, which the reservation cannot see.
pub(super) fn plan_reconcile(
    inventory: &[ItemStack],
    vault: &[Vec<u8>],
    uuid: Uuid,
    class: Option<HunterClass>,
    held_grimoire: bool,
) -> ReconcilePlan {
    let mut plan = ReconcilePlan::default();
    for (slot, stack) in inventory.iter().enumerate() {
        if !stack.is_empty() && class::is_class_frozen(stack, uuid, class) {
            plan.frozen.push((slot, stack.clone()));
        }
    }
    let mut has_grimoire = held_grimoire || inventory.iter().any(grimoire::is_grimoire);
    for (index, entry) in vault.iter().enumerate() {
        let Ok(stack) = nbt::decode(entry) else {
            plan.keep.push(entry.clone());
            continue;
        };
        if class::is_class_frozen(&stack, uuid, class)
            || (grimoire::is_grimoire(&stack) && has_grimoire)
        {
            plan.keep.push(entry.clone());
            continue;
        }
        has_grimoire |= grimoire::is_grimoire(&stack);
        plan.returns.push((index, stack));
    }
    plan
}

impl SoloLevelingState {
    /// Lists the decoded vault entries with their raw bytes, for menus.
    pub(crate) fn class_vault_items(&self, uuid: Uuid) -> Vec<(usize, Vec<u8>, ItemStack)> {
        self.sessions
            .with_mut(uuid, |session| {
                session
                    .class_vault
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

    /// Returns whether a grimoire waits in the class vault.
    pub(crate) fn class_vault_has_grimoire(&self, uuid: Uuid) -> bool {
        self.sessions
            .with_mut(uuid, |session| vault_has_grimoire(&session.class_vault))
            .unwrap_or(false)
    }

    /// Queues the reconcile owed by a class change or join, once the
    /// inventory is free of transactions.
    pub(crate) fn request_class_vault_reconcile(&self, player: &Player) {
        let uuid = player.gameprofile.id();
        let queued = self
            .sessions
            .with_mut(uuid, |session| {
                if !session.class_vault_due || player.inventory_transaction_active() {
                    return false;
                }
                session.class_vault_due = false;
                true
            })
            .unwrap_or(false);
        if !queued {
            return;
        }
        let mut persistence = self.persistence.lock();
        if !persistence.accepts_writes(uuid) {
            drop(persistence);
            self.sessions
                .with_mut(uuid, |session| session.class_vault_due = true);
            return;
        }
        persistence
            .requests
            .push_back(Request::ClassVaultReconcile { uuid });
    }

    /// Queues taking one vault entry back into the inventory.
    pub(crate) fn request_class_vault_withdrawal(
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
        persistence.requests.push_back(Request::ClassVault {
            uuid,
            transfer: ClassVaultTransfer::Withdraw { index, expected },
        });
    }

    #[expect(
        clippy::too_many_lines,
        reason = "keeps the freeze scan, return scan and publication of one reconcile together"
    )]
    pub(super) fn begin_class_vault_reconcile(
        self: &Arc<Self>,
        server: &Arc<Server>,
        player: Arc<Player>,
        uuid: Uuid,
    ) -> bool {
        let retry_later = |state: &SoloLevelingState| {
            state
                .sessions
                .with_mut(uuid, |session| session.class_vault_due = true);
        };
        let Some(class) = self.with_session_mut(uuid, |session| session.class.class) else {
            return false;
        };
        // The main hand is not transactional stock; park it when the reconcile
        // would freeze it, so the freeze can own its slot.
        let held_frozen = {
            let inventory = player.inventory.lock();
            let held = inventory
                .get_item_in_hand(InteractionHand::MainHand)
                .clone();
            class::is_class_frozen(&held, uuid, class)
        };
        if held_frozen && park_main_hand_stack(&player).is_none() {
            retry_later(self);
            return false;
        }
        let Ok(mut reservation) = player.reserve_stored_inventory() else {
            retry_later(self);
            return false;
        };
        let held_grimoire = {
            let inventory = player.inventory.lock();
            grimoire::is_grimoire(inventory.get_item_in_hand(InteractionHand::MainHand))
        };
        let Some(vault) = self
            .sessions
            .with_mut(uuid, |session| session.class_vault.clone())
        else {
            return false;
        };
        let mut inventory = vec![ItemStack::empty(); PlayerInventory::INVENTORY_SIZE];
        for (slot, stack) in inventory.iter_mut().enumerate() {
            if let Ok(item) = reservation.item(slot) {
                *stack = item;
            }
        }
        let plan = plan_reconcile(&inventory, &vault, uuid, class, held_grimoire);
        if plan.frozen.is_empty() && plan.returns.is_empty() {
            return false;
        }
        for (slot, _) in &plan.frozen {
            if reservation.set_item(*slot, ItemStack::empty()).is_err() {
                log::error!("sololeveling: class vault could not clear slot {slot} of {uuid}");
                return false;
            }
        }
        let mut vault_after = plan.keep.clone();
        for (index, stack) in &plan.returns {
            if stage_inventory_item(&mut reservation, stack.clone()).is_err() {
                // No room: the entry stays stored for a manual withdrawal.
                vault_after.push(vault[*index].clone());
            }
        }
        for (_, stack) in &plan.frozen {
            match nbt::encode(stack) {
                Ok(entry) => vault_after.push(entry),
                Err(error) => {
                    log::error!("sololeveling: class vault could not encode a stack: {error}");
                    return false;
                }
            }
        }
        let Some(mut snapshot) = self
            .sessions
            .with_mut(uuid, |session| session.saved_data(uuid))
        else {
            return false;
        };
        snapshot.class_vault.clone_from(&vault_after);
        let Some(expected) = self.persistence.lock().expected.get(&uuid).cloned() else {
            return false;
        };
        let Ok(replacement) = serde_json::to_vec(&snapshot) else {
            return false;
        };
        let committed = replacement.clone();
        let state = Arc::clone(self);
        let completion_player = Arc::clone(&player);
        let frozen_count = plan.frozen.len();
        let returned_count = plan.returns.len();
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
                        state
                            .persistence
                            .lock()
                            .expected
                            .insert(uuid, Some(committed));
                        state.sessions.with_mut(uuid, |session| {
                            session.class_vault = vault_after;
                        });
                        if frozen_count > 0 {
                            ui::tell(
                                &completion_player,
                                &format!(
                                    "Предметы, не подходящие классу, отправлены в Хранилище класса: {frozen_count} шт."
                                ),
                                Color::Yellow,
                            );
                        }
                        if returned_count > 0 {
                            ui::tell(
                                &completion_player,
                                &format!(
                                    "Из Хранилища класса возвращено предметов: {returned_count} шт."
                                ),
                                Color::LightPurple,
                            );
                        }
                        if commit == TransactionCommit::DurabilityUncertain {
                            completion_player.send_message(&TextComponent::plain(
                                "Передача подтверждена не полностью; данные будут восстановлены.",
                            ));
                        }
                    } else {
                        log::error!(
                            "sololeveling: class vault reconcile failed for {uuid}: {result:?}"
                        );
                        let conflicted = result
                            .as_ref()
                            .err()
                            .is_some_and(|error| error.kind() == ErrorKind::WouldBlock);
                        if conflicted {
                            state.sessions.with_mut(uuid, |session| {
                                session.class_vault_due = true;
                            });
                        } else {
                            state.persistence.lock().failed.insert(uuid);
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
            retry_later(self);
            return false;
        }
        true
    }

    #[expect(
        clippy::too_many_lines,
        reason = "keeps the validation and publication of one withdrawal together"
    )]
    pub(super) fn begin_class_vault_withdrawal(
        self: &Arc<Self>,
        server: &Arc<Server>,
        player: Arc<Player>,
        uuid: Uuid,
        index: usize,
        selected: Vec<u8>,
    ) -> bool {
        let fail = |state: &SoloLevelingState, player: &Player, message: &str| {
            state
                .sessions
                .with_mut(uuid, |session| session.economic_busy = false);
            state
                .sessions
                .with_mut(uuid, |session| session.class_vault_menu_due = true);
            player.send_message(&TextComponent::plain(message.to_owned()));
            false
        };
        if is_instance_world(&player.get_world()) {
            return fail(self, &player, "Хранилище недоступно в подземелье.");
        }
        if !self.persistence.lock().accepts_writes(uuid) {
            return fail(self, &player, "Хранилище недоступно.");
        }
        let Some((vault, class)) = self.sessions.with_mut(uuid, |session| {
            (session.class_vault.clone(), session.class.class)
        }) else {
            return fail(self, &player, "Данные охотника не загружены.");
        };
        let Some(expected) = self.persistence.lock().expected.get(&uuid).cloned() else {
            return fail(self, &player, "Данные охотника не загружены.");
        };
        let Some(bytes) = vault.get(index) else {
            return fail(self, &player, "Предмет уже изменился.");
        };
        if bytes != &selected {
            return fail(self, &player, "Предмет уже изменился.");
        }
        let Ok(item) = nbt::decode(bytes) else {
            return fail(
                self,
                &player,
                "Этот предмет не поддерживается текущей версией.",
            );
        };
        if class::is_class_frozen(&item, uuid, class) {
            return fail(self, &player, "Этот предмет не подходит вашему классу.");
        }
        if grimoire::is_grimoire(&item) && {
            let inventory = player.inventory.lock();
            (0..inventory.get_container_size())
                .any(|slot| grimoire::is_grimoire(inventory.get_item(slot)))
        } {
            return fail(self, &player, "Гримуар уже у вас.");
        }
        let Ok(mut reservation) = player.reserve_stored_inventory() else {
            return fail(self, &player, "Инвентарь занят. Повторите позже.");
        };
        if stage_inventory_item(&mut reservation, item).is_err() {
            return fail(self, &player, "В инвентаре недостаточно места.");
        }
        let mut vault = vault;
        vault.remove(index);
        let Some(mut snapshot) = self
            .sessions
            .with_mut(uuid, |session| session.saved_data(uuid))
        else {
            return fail(self, &player, "Данные охотника не загружены.");
        };
        snapshot.class_vault.clone_from(&vault);
        let Ok(replacement) = serde_json::to_vec(&snapshot) else {
            return fail(self, &player, "Не удалось подготовить данные.");
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
                        state
                            .persistence
                            .lock()
                            .expected
                            .insert(uuid, Some(committed));
                        state.sessions.with_mut(uuid, |session| {
                            session.class_vault = vault;
                            session.economic_busy = false;
                            session.class_vault_menu_due = true;
                        });
                        if commit == TransactionCommit::DurabilityUncertain {
                            completion_player.send_message(&TextComponent::plain(
                                "Предмет передан, но подтверждение диска не получено.",
                            ));
                        }
                    } else {
                        log::error!(
                            "sololeveling: class vault withdrawal failed for {uuid}: {result:?}"
                        );
                        let conflicted = result
                            .as_ref()
                            .err()
                            .is_some_and(|error| error.kind() == ErrorKind::WouldBlock);
                        if !conflicted {
                            state.persistence.lock().failed.insert(uuid);
                        }
                        state.sessions.with_mut(uuid, |session| {
                            session.economic_busy = false;
                            session.class_vault_menu_due = true;
                        });
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
            return fail(self, &player, "Не удалось начать сохранение.");
        }
        true
    }
}

fn vault_has_grimoire(entries: &[Vec<u8>]) -> bool {
    entries
        .iter()
        .any(|entry| nbt::decode(entry).is_ok_and(|stack| grimoire::is_grimoire(&stack)))
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;

    use super::*;
    use crate::item::{binding, serial};

    fn stored_slots(plan: &ReconcilePlan) -> Vec<usize> {
        plan.frozen.iter().map(|(slot, _)| *slot).collect()
    }

    fn empty_inventory() -> Vec<ItemStack> {
        vec![ItemStack::empty(); 36]
    }

    #[test]
    fn changing_away_from_the_mage_freezes_only_personal_class_items() {
        init_vanilla_registry();
        let owner = Uuid::new_v4();
        let grimoire = grimoire::stack(owner);
        let foreign_tradable = serial::stack(serial::SerialId::RobeChestplate, None);
        let own_starter = serial::stack(serial::SerialId::HunterSword, Some(owner));
        let inventory = vec![grimoire, foreign_tradable, own_starter];

        let plan = plan_reconcile(&inventory, &[], owner, Some(HunterClass::Fighter), false);

        assert_eq!(stored_slots(&plan), vec![0]);
        assert!(plan.returns.is_empty());
        assert!(plan.keep.is_empty());
    }

    #[test]
    fn changing_back_to_the_mage_returns_the_grimoire() {
        init_vanilla_registry();
        let owner = Uuid::new_v4();
        let grimoire = grimoire::stack(owner);
        let entry = nbt::encode(&grimoire).expect("grimoire should encode");
        let vault = vec![entry];

        let plan = plan_reconcile(
            &empty_inventory(),
            &vault,
            owner,
            Some(HunterClass::Mage),
            false,
        );

        assert!(plan.frozen.is_empty());
        assert_eq!(plan.returns.len(), 1);
        assert_eq!(plan.returns[0].1, grimoire);
        assert!(plan.keep.is_empty());
    }

    #[test]
    fn a_third_class_keeps_the_grimoire_frozen_and_legacy_saves_freeze_it() {
        init_vanilla_registry();
        let owner = Uuid::new_v4();
        let entry = nbt::encode(&grimoire::stack(owner)).expect("grimoire should encode");
        for class in [Some(HunterClass::Tanker), None] {
            let plan = plan_reconcile(&empty_inventory(), &[entry.clone()], owner, class, false);
            assert!(plan.returns.is_empty());
            assert_eq!(plan.keep, vec![entry.clone()]);
        }
    }

    #[test]
    fn a_hunter_already_holding_a_grimoire_gets_no_second_one() {
        init_vanilla_registry();
        let owner = Uuid::new_v4();
        let held = grimoire::stack(owner);
        let stored_entry = nbt::encode(&grimoire::stack(owner)).expect("grimoire should encode");
        let mut inventory = vec![ItemStack::empty(); 36];
        inventory[3] = held;

        let plan = plan_reconcile(
            &inventory,
            &[stored_entry.clone()],
            owner,
            Some(HunterClass::Mage),
            false,
        );

        assert!(plan.returns.is_empty());
        assert_eq!(plan.keep, vec![stored_entry]);
    }

    #[test]
    fn undecodable_entries_survive_the_reconcile() {
        init_vanilla_registry();
        let owner = Uuid::new_v4();
        let broken = vec![0xff, 0x00, 0x7f];
        let plan = plan_reconcile(
            &empty_inventory(),
            &[broken.clone()],
            owner,
            Some(HunterClass::Mage),
            false,
        );
        assert_eq!(plan.keep, vec![broken]);
    }

    #[test]
    fn bound_class_armor_freezes_and_tradable_copies_stay() {
        init_vanilla_registry();
        let owner = Uuid::new_v4();
        let personal = serial::stack(serial::SerialId::ImprovedRobeChestplate, Some(owner));
        assert!(binding::bound_owner(&personal).is_some());
        let tradable = serial::stack(serial::SerialId::ImprovedRobeChestplate, None);
        let inventory = vec![personal.clone(), tradable];

        let plan = plan_reconcile(&inventory, &[], owner, Some(HunterClass::Assassin), false);

        assert_eq!(stored_slots(&plan), vec![0]);
        // Back to the Mage the same entry would come home.
        let entry = nbt::encode(&personal).expect("robe should encode");
        let back = plan_reconcile(
            &empty_inventory(),
            &[entry],
            owner,
            Some(HunterClass::Mage),
            false,
        );
        assert_eq!(back.returns.len(), 1);
    }

    #[test]
    fn vault_grimoire_detection_decodes_entries() {
        init_vanilla_registry();
        let owner = Uuid::new_v4();
        assert!(!vault_has_grimoire(&[]));
        let entry = nbt::encode(&grimoire::stack(owner)).expect("grimoire should encode");
        assert!(vault_has_grimoire(&[entry]));
        assert!(!vault_has_grimoire(&[vec![0xff]]));
    }
}
