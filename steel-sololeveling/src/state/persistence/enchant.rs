//! Enchanting as one inventory/account transaction.
//!
//! The rebuilt artifact and the Won debit commit together: a failed write
//! leaves the old artifact and the old balance, never one without the other.

use std::{cell::Cell, sync::Arc};

use steel_core::player::Player;
use steel_core::player::player_data_storage::transaction::ExtensionStorageWrite;
use steel_core::server::Server;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use super::{Request, account};
use crate::enchant_menu::EnchantMenuKind;
use crate::item::artifact;
use crate::item::binding;
use crate::item::enchantment::{self, EnchantError, Enchantment};
use crate::item::nbt;
use crate::item::serial;
use crate::quest_book::Goal;
use crate::sound::SystemSound;
use crate::state::auction::park_main_hand_stack;
use crate::state::{EnchantTarget, SoloLevelingState};
use steel_core::inventory::container::Container as _;
use steel_registry::item_stack::ItemStack;
use steel_utils::locks::SyncMutex;

fn refuse(player: &Player, message: &str) {
    player.send_message(&TextComponent::plain(message.to_owned()).color(Color::Red));
}

fn enchant_target_info(
    stack: &ItemStack,
) -> Result<
    (
        crate::item::artifact::Rarity,
        Option<crate::item::enchantment::ItemKind>,
        Vec<Enchantment>,
    ),
    EnchantError,
> {
    if let Some(artifact) = artifact::held(stack) {
        return Ok((
            artifact.rarity,
            artifact.id.enchant_kind(),
            artifact::enchantments_of(stack),
        ));
    }
    if let Some(serial) = serial::held(stack) {
        return Ok((
            crate::item::artifact::Rarity::Common,
            serial.id.enchant_kind(),
            serial::enchantments_of(stack),
        ));
    }
    Err(EnchantError::NotArtifact)
}

impl SoloLevelingState {
    /// Queues one enchantment of the main-hand artifact.
    ///
    /// The cheap checks run here so a click gets an answer at once; the
    /// persistence lane checks everything again against the committed balance.
    pub fn request_enchantment(
        &self,
        player: &Player,
        enchantment: Enchantment,
        target: EnchantTarget,
        completion: Arc<SyncMutex<Option<EnchantTarget>>>,
    ) -> Result<(), EnchantError> {
        let uuid = player.gameprofile.id();
        let held = self
            .enchant_target_stack(player, target)
            .ok_or(EnchantError::NotArtifact)?;
        let (rarity, kind, applied) = enchant_target_info(&held)?;
        let balance = self
            .with_session_mut(uuid, |session| session.won.amount())
            .unwrap_or(0);
        enchantment::check(rarity, kind, &applied, enchantment, balance)?;
        if !self.persistence.lock().accepts_writes(uuid) {
            return Err(EnchantError::Storage);
        }
        let accepted = self.sessions.with_mut(uuid, |session| {
            if session.economic_busy {
                return false;
            }
            session.economic_busy = true;
            true
        });
        if accepted != Some(true) {
            return Err(EnchantError::Storage);
        }
        let mut persistence = self.persistence.lock();
        if persistence.quiescing || persistence.failed.contains(&uuid) {
            drop(persistence);
            self.sessions
                .with_mut(uuid, |session| session.economic_busy = false);
            return Err(EnchantError::Storage);
        }
        persistence.requests.push_back(Request::Enchant {
            uuid,
            enchantment,
            target,
            completion,
        });
        Ok(())
    }

    /// Returns the stack an enchantment targets, if it holds anything.
    fn enchant_target_stack(&self, player: &Player, target: EnchantTarget) -> Option<ItemStack> {
        let stack = match target {
            EnchantTarget::Inventory(slot) => player.inventory.lock().get_item(slot).clone(),
            EnchantTarget::Gear(index) => self
                .with_session_mut(player.gameprofile.id(), |session| {
                    session.gear.get(index).cloned()
                })
                .flatten()?,
        };
        (!stack.is_empty()).then_some(stack)
    }

    /// Rebuilds the main-hand artifact and debits its price in one commit.
    ///
    /// Returns `false` when nothing was submitted, so the lane is released.
    #[expect(
        clippy::too_many_lines,
        reason = "the artifact swap and the Won debit are staged and committed as one transaction"
    )]
    pub(super) fn begin_enchantment(
        self: &Arc<Self>,
        server: &Arc<Server>,
        player: Arc<Player>,
        uuid: Uuid,
        enchantment: Enchantment,
        target: EnchantTarget,
        completion: Arc<SyncMutex<Option<EnchantTarget>>>,
    ) -> bool {
        let current_target = Cell::new(target);
        let fail = |message: &str| {
            self.release_shop_purchase(uuid);
            refuse(&player, message);
            *completion.lock() = Some(current_target.get());
            false
        };
        let Some(held) = self.enchant_target_stack(&player, target) else {
            return fail(EnchantError::NotArtifact.message());
        };
        let Some(mut snapshot) = self
            .sessions
            .with_mut(uuid, |session| session.saved_data(uuid))
        else {
            return fail("Данные охотника не загружены.");
        };
        let mut applied = match enchant_target_info(&held) {
            Ok((rarity, kind, applied)) => {
                if let Err(error) =
                    enchantment::check(rarity, kind, &applied, enchantment, snapshot.won.amount())
                {
                    return fail(error.message());
                }
                applied
            }
            Err(_) => return fail(EnchantError::NotArtifact.message()),
        };
        if !snapshot.won.debit(enchantment.price()) {
            return fail(EnchantError::InsufficientFunds.message());
        }
        applied.push(enchantment);
        let mut enchanted = if let Some(id) = artifact::of(&held) {
            artifact::enchanted_stack(id, binding::bound_owner(&held), &applied)
        } else if let Some(id) = serial::of(&held) {
            serial::enchanted_stack(id, binding::bound_owner(&held), &applied)
        } else {
            return fail(EnchantError::NotArtifact.message());
        };
        enchanted.count = held.count;
        let balance = snapshot.won;
        if let EnchantTarget::Gear(index) = target {
            // Gear lives in the account record, so the rebuilt artifact is
            // committed with the debit as part of the same write.
            let Ok(bytes) = nbt::encode(&enchanted) else {
                return fail("Зачарование не удалось подготовить.");
            };
            if snapshot.gear.slots.len() <= index {
                snapshot.gear.slots.resize(index + 1, None);
            }
            snapshot.gear.slots[index] = Some(bytes);
        }
        let Some(expected) = self.persistence.lock().expected.get(&uuid).cloned() else {
            return fail("Данные охотника не загружены.");
        };
        let Ok(replacement) = serde_json::to_vec(&snapshot) else {
            return fail("Зачарование не удалось подготовить.");
        };
        let inventory_slot = match target {
            // A durable transaction cannot own the equipped hand, so a held
            // artifact is parked in storage first; the move alone never
            // changes ownership.
            EnchantTarget::Inventory(slot)
                if slot == usize::from(player.inventory.lock().get_selected_slot()) =>
            {
                let Some(slot) = park_main_hand_stack(&player) else {
                    return fail("Освободите слот в инвентаре для зачарования.");
                };
                current_target.set(EnchantTarget::Inventory(slot));
                Some(slot)
            }
            EnchantTarget::Inventory(slot) => Some(slot),
            EnchantTarget::Gear(_) => None,
        };
        // Gear changes no inventory slot, but the transaction still owns the
        // inventory for the length of the account write.
        let Ok(mut reservation) = player.reserve_stored_inventory_with_menu::<EnchantMenuKind>()
        else {
            return fail("Инвентарь занят: повторите через мгновение.");
        };
        if let Some(slot) = inventory_slot
            && reservation.set_item(slot, enchanted.clone()).is_err()
        {
            return fail("Не удалось подготовить предмет.");
        }
        let committed_bytes = replacement.clone();
        let state = Arc::clone(self);
        let completion_player = Arc::clone(&player);
        let finished = Arc::clone(&completion);
        let finished_target = current_target.get();
        let transaction = server.submit_stored_inventory_transaction(
            vec![reservation],
            vec![ExtensionStorageWrite {
                key: account::key(uuid),
                expected,
                replacement,
            }],
            move |result| {
                if result.is_ok() {
                    state.record_goal(uuid, Goal::Enchant, 1);
                    state
                        .persistence
                        .lock()
                        .expected
                        .insert(uuid, Some(committed_bytes));
                    state.sessions.with_mut(uuid, |session| {
                        session.won = balance;
                        session.economic_busy = false;
                        if let EnchantTarget::Gear(index) = target
                            && let Some(slot) = session.gear.get_mut(index)
                        {
                            *slot = enchanted.clone();
                            session.gear_dirty = true;
                        }
                    });
                    if let Err(error) = state.save_session(uuid) {
                        log::warn!("sololeveling: could not queue enchant follow-up save: {error}");
                    }
                    SystemSound::Reward.play(&completion_player);
                    completion_player.send_message(
                        &TextComponent::plain(format!(
                            "Наложено «{}» за {} Won.",
                            enchantment.display_name(),
                            enchantment.price()
                        ))
                        .color(Color::Aqua),
                    );
                } else {
                    log::error!("sololeveling: enchantment transaction failed: {result:?}");
                    state
                        .sessions
                        .with_mut(uuid, |session| session.economic_busy = false);
                    refuse(
                        &completion_player,
                        "Зачарование не сохранено; предмет и баланс не изменены.",
                    );
                }
                state.persistence.lock().busy = false;
                *finished.lock() = Some(finished_target);
            },
        );
        if let Err(error) = transaction {
            log::error!("sololeveling: could not start enchantment transaction: {error}");
            return fail("Зачарование не удалось сохранить.");
        }
        true
    }
}
