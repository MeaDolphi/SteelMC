//! Atomic crafting orders: the materials, the work fee and the new item
//! commit together, with overflow going to the System storage.

use std::sync::Arc;

use steel_core::player::Player;
use steel_core::player::connection::NetworkConnection as _;
use steel_core::player::player_data_storage::transaction::{
    ExtensionStorageWrite, TransactionCommit,
};
use steel_core::server::Server;
use steel_registry::item_stack::ItemStack;
use text_components::TextComponent;
use uuid::Uuid;

use super::{Request, account, stage_inventory_item};
use crate::crafting::{self, available};
use crate::crafting_menu::CraftingMenuKind;
use crate::item::artifact::{self, ArtifactId};
use crate::item::{binding, material, nbt};
use crate::quest_book::Goal;
use crate::sound::SystemSound;
use crate::state::SoloLevelingState;

/// Slots of the main inventory and hotbar materials are taken from.
const INVENTORY_SLOTS: usize = 36;

/// Why an order was refused before anything changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CraftingError {
    /// The item is not made to order.
    UnknownRecipe,
    /// The hunter's rank is too low for the recipe.
    RankTooLow,
    /// The hunter has too little Won for the work.
    InsufficientWon,
    /// Another economic operation of this hunter is still running.
    Busy,
    /// The hunter's data cannot be written right now.
    Storage,
}

impl CraftingError {
    /// Returns the Russian message shown to the hunter.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::UnknownRecipe => "Этот предмет не изготавливают на заказ.",
            Self::RankTooLow => "Заказы улучшенной ступени принимают с ранга D.",
            Self::InsufficientWon => "Недостаточно Won для оплаты работы.",
            Self::Busy => "Предыдущая операция ещё обрабатывается.",
            Self::Storage => "Данные охотника сейчас недоступны для записи.",
        }
    }
}

impl SoloLevelingState {
    /// Queues a crafting order for `output`.
    pub fn request_crafting(
        &self,
        player: &Player,
        output: ArtifactId,
    ) -> Result<(), CraftingError> {
        let uuid = player.gameprofile.id();
        let recipe = crafting::recipe(output).ok_or(CraftingError::UnknownRecipe)?;
        let accepted = self
            .sessions
            .with_mut(uuid, |session| {
                if session.economic_busy {
                    return Err(CraftingError::Busy);
                }
                if session.stats.rank() < recipe.rank {
                    return Err(CraftingError::RankTooLow);
                }
                if session.won.amount() < recipe.work {
                    return Err(CraftingError::InsufficientWon);
                }
                session.economic_busy = true;
                Ok(())
            })
            .ok_or(CraftingError::Storage)?;
        accepted?;
        let mut persistence = self.persistence.lock();
        if persistence.quiescing || persistence.failed.contains(&uuid) {
            drop(persistence);
            self.sessions
                .with_mut(uuid, |session| session.economic_busy = false);
            return Err(CraftingError::Storage);
        }
        persistence
            .requests
            .push_back(Request::Crafting { uuid, output });
        Ok(())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "materials, the work fee and the new item must commit as one transaction"
    )]
    pub(super) fn begin_crafting(
        self: &Arc<Self>,
        server: &Arc<Server>,
        player: Arc<Player>,
        uuid: Uuid,
        output: ArtifactId,
    ) -> bool {
        let fail = |message: &'static str| {
            self.release_shop_purchase(uuid);
            player.send_message(&TextComponent::plain(message));
            false
        };
        if player.connection.closed() {
            self.release_shop_purchase(uuid);
            return false;
        }
        let Some(recipe) = crafting::recipe(output) else {
            return fail(CraftingError::UnknownRecipe.message());
        };
        let Ok(mut reservation) = player.reserve_stored_inventory_with_menu::<CraftingMenuKind>()
        else {
            return fail(
                "Заказ не удалось начать: инвентарь занят или предмет находится на курсоре или в сетке крафта.",
            );
        };
        let Some(mut snapshot) = self
            .sessions
            .with_mut(uuid, |session| session.saved_data(uuid))
        else {
            return fail("Сессия охотника не найдена.");
        };
        if snapshot.stats.rank() < recipe.rank {
            return fail(CraftingError::RankTooLow.message());
        }
        if !snapshot.won.debit(recipe.work) {
            return fail(CraftingError::InsufficientWon.message());
        }
        let mut slots: Vec<ItemStack> = (0..INVENTORY_SLOTS)
            .map(|slot| {
                reservation
                    .item(slot)
                    .unwrap_or_else(|_| ItemStack::empty())
            })
            .collect();
        let have = available(&recipe, &slots);
        if recipe
            .materials
            .iter()
            .zip(&have)
            .any(|((_, needed), have)| have < needed)
        {
            return fail("Не хватает материалов для заказа.");
        }
        for (wanted, needed) in &recipe.materials {
            let mut left = i32::try_from(*needed).unwrap_or(i32::MAX);
            for (slot, stack) in slots.iter_mut().enumerate() {
                if left == 0 {
                    break;
                }
                if binding::bound_owner(stack).is_some()
                    || material::of(stack) != Some((*wanted, recipe.quality))
                {
                    continue;
                }
                let taken = stack.count.min(left);
                left -= taken;
                let remaining = stack.count - taken;
                let replacement = if remaining > 0 {
                    stack.copy_with_count(remaining)
                } else {
                    ItemStack::empty()
                };
                if reservation.set_item(slot, replacement.clone()).is_err() {
                    return fail("Не удалось подготовить инвентарь к заказу.");
                }
                *stack = replacement;
            }
        }
        let item = artifact::stack(output, None);
        let mut stored = false;
        if stage_inventory_item(&mut reservation, item.clone()).is_err() {
            match nbt::encode(&item) {
                Ok(bytes) => {
                    snapshot.deferred_inventory.push(bytes);
                    stored = true;
                }
                Err(error) => {
                    log::error!(
                        "sololeveling: could not encode a crafted item for {uuid}: {error}"
                    );
                    return fail("Не удалось подготовить заказ.");
                }
            }
        }
        let expected = self.persistence.lock().expected.get(&uuid).cloned();
        let Some(expected) = expected else {
            return fail("Данные охотника не загружены.");
        };
        let replacement = match serde_json::to_vec(&snapshot) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::error!("sololeveling: could not encode crafting order for {uuid}: {error}");
                return fail("Не удалось сохранить заказ.");
            }
        };
        let won = snapshot.won;
        let deferred_inventory = snapshot.deferred_inventory;
        let state = Arc::clone(self);
        let completion_player = Arc::clone(&player);
        let committed_bytes = replacement.clone();
        let transaction = server.submit_stored_inventory_transaction(
            vec![reservation],
            vec![ExtensionStorageWrite {
                key: account::key(uuid),
                expected,
                replacement,
            }],
            move |result| {
                match result {
                    Ok(commit) => {
                        state.record_goal(uuid, Goal::CraftOrder, 1);
                        state
                            .persistence
                            .lock()
                            .expected
                            .insert(uuid, Some(committed_bytes));
                        state.sessions.with_mut(uuid, |session| {
                            session.won = won;
                            session.deferred_inventory = deferred_inventory;
                            session.economic_busy = false;
                        });
                        SystemSound::Reward.play(&completion_player);
                        let name = output.artifact().name;
                        completion_player.send_message(&TextComponent::plain(if stored {
                            format!("Изготовлено: {name}. Инвентарь полон — предмет в Хранилище.")
                        } else {
                            format!("Изготовлено: {name}.")
                        }));
                        if commit == TransactionCommit::DurabilityUncertain {
                            completion_player.send_message(&TextComponent::plain(
                                "Заказ выполнен, но подтверждение диска не получено.",
                            ));
                        }
                    }
                    Err(error) => {
                        log::error!("sololeveling: crafting order failed: {error:?}");
                        state
                            .sessions
                            .with_mut(uuid, |session| session.economic_busy = false);
                        completion_player.send_message(&TextComponent::plain(
                            "Заказ не сохранён; Won и материалы не изменены.",
                        ));
                    }
                }
                state.persistence.lock().busy = false;
            },
        );
        if let Err(error) = transaction {
            log::error!("sololeveling: could not start crafting order: {error}");
            return fail("Заказ не удалось сохранить.");
        }
        true
    }
}
