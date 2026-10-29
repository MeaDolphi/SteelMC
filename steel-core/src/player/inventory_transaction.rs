//! Live reservations for transactions affecting stored player items.
//!
//! Transactions do not edit equipped slots (including the selected hand).
//! Those slots can still receive armor damage or consume death protection while
//! storage is reserved. This keeps combat outcomes independent of disk latency.

use std::f32::consts::TAU;
use std::{
    io,
    sync::{Arc, atomic::Ordering},
};

use glam::DVec3;
use steel_registry::item_stack::ItemStack;

use crate::{
    entity::entities::ExperienceOrbEntity,
    entity::{Entity, LivingEntity},
    inventory::container::Container,
    player::{
        Player,
        player_data::{PersistentPlayerData, PersistentSlot},
        player_data_storage::transaction::PlayerStorageWrite,
        player_inventory::{MenuRemovalStatus, PlayerInventory},
    },
    world::World,
};

/// A death's inventory consequences, deferred without delaying damage/death events.
pub(super) struct DeferredDeathInventory {
    pub world: Arc<World>,
    pub item_position: DVec3,
    pub experience_position: DVec3,
    pub drop_items: bool,
    pub experience: i32,
}

/// Exclusive ownership of stored slots until an operation is applied or rejected.
///
/// Take this only from a server job safe point, after the menu click has unwound.
/// Dropping an unsubmitted reservation releases it; an in-flight durable job must
/// retain ownership through shutdown and apply its outcome before disconnect saves.
pub struct StoredInventoryReservation {
    pub(crate) player: Arc<Player>,
    before: Vec<PersistentSlot>,
    snapshot: PersistentPlayerData,
    selected: usize,
}

impl Player {
    /// Whether player-driven inventory mutations are temporarily suspended.
    #[must_use]
    pub fn inventory_transaction_active(&self) -> bool {
        self.session
            .inventory_transaction_active
            .load(Ordering::Acquire)
    }

    /// Reserves storage, not equipment. Items to sell/consume must be put away
    /// from the active hand before starting a durable operation.
    pub fn reserve_stored_inventory(self: &Arc<Self>) -> io::Result<StoredInventoryReservation> {
        if self.inventory_transaction_active()
            || self.is_dead_or_dying()
            || self.is_removed()
            || self.is_world_change_pending()
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "player inventory is unavailable",
            ));
        }
        if self.remove_all_menus() != MenuRemovalStatus::Complete {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "menu callback still owns the inventory",
            ));
        }
        self.stop_using_item();
        self.block_breaking
            .lock()
            .cancel_for_inventory_transaction(self, &self.get_world());
        if self
            .session
            .inventory_transaction_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "player inventory is reserved",
            ));
        }
        let selected = {
            let mut inventory = self.inventory.lock();
            inventory.transaction_reserved = true;
            usize::from(inventory.get_selected_slot())
        };
        let snapshot = PersistentPlayerData::from_player(self);
        let before = snapshot.inventory.clone();
        Ok(StoredInventoryReservation {
            player: Arc::clone(self),
            before,
            snapshot,
            selected,
        })
    }

    pub(crate) fn finish_deferred_death_inventory(&self) {
        let pending = self.session.deferred_death_inventory.lock().take();
        let Some(pending) = pending else { return };
        self.finish_death_inventory(pending);
        self.retry_deferred_death_respawn();
    }

    pub(super) fn finish_death_inventory(&self, pending: DeferredDeathInventory) {
        if pending.drop_items {
            let drops = self.inventory.lock().take_death_drops();
            let mut kept = Vec::new();
            for item in drops {
                if !self.get_world().gameplay_hooks.allow_item_drop(self, &item) {
                    kept.push(item);
                    continue;
                }
                let power = rand::random_range(0.0f32..0.5);
                let angle = rand::random_range(0.0f32..TAU);
                let velocity = DVec3::new(
                    f64::from(-angle.sin() * power),
                    0.2,
                    f64::from(angle.cos() * power),
                );
                if let Some(entity) =
                    pending
                        .world
                        .spawn_item_with_velocity(pending.item_position, item, velocity)
                {
                    entity.set_pickup_delay(40);
                }
            }
            if !kept.is_empty() {
                let mut inventory = self.inventory.lock();
                for mut item in kept {
                    let _ = inventory.add(&mut item);
                    if !item.is_empty() {
                        log::error!(
                            "gameplay-protected death item could not be restored for {}",
                            self.gameprofile.id()
                        );
                    }
                }
                drop(inventory);
                self.send_inventory_to_remote();
            }
            if pending.experience > 0 {
                ExperienceOrbEntity::award(
                    &pending.world,
                    pending.experience_position,
                    pending.experience,
                );
            }
        }
    }
}

impl StoredInventoryReservation {
    /// Reads a stored stack. Equipped/selected slots are not transactional stock.
    pub fn item(&self, slot: usize) -> io::Result<ItemStack> {
        self.validate_slot(slot)?;
        Ok(self
            .snapshot
            .inventory
            .iter()
            .find(|item| item.slot as usize == slot)
            .map_or_else(ItemStack::empty, |item| item.item.clone()))
    }

    /// Replaces one staged stored slot without exposing its output before commit.
    pub fn set_item(&mut self, slot: usize, item: ItemStack) -> io::Result<()> {
        self.validate_slot(slot)?;
        self.snapshot
            .inventory
            .retain(|entry| entry.slot as usize != slot);
        if !item.is_empty() {
            self.snapshot.inventory.push(PersistentSlot {
                slot: slot as i8,
                item,
            });
        }
        Ok(())
    }

    fn validate_slot(&self, slot: usize) -> io::Result<()> {
        if slot >= PlayerInventory::INVENTORY_SIZE || slot == self.selected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "equipped slots cannot participate in a stored-item transaction",
            ));
        }
        Ok(())
    }

    pub(crate) fn storage_write(&self) -> PlayerStorageWrite {
        PlayerStorageWrite {
            domain: self.player.get_world().domain().to_owned(),
            uuid: self.player.gameprofile.id(),
            data: self.snapshot.clone(),
        }
    }

    pub(crate) fn install(&mut self) {
        let mut inventory = self.player.inventory.lock();
        for slot in 0..PlayerInventory::INVENTORY_SIZE {
            if slot == self.selected {
                continue;
            }
            let previous = self
                .before
                .iter()
                .find(|entry| entry.slot as usize == slot)
                .map_or_else(ItemStack::empty, |entry| entry.item.clone());
            assert!(
                ItemStack::matches(inventory.get_item(slot), &previous),
                "a reserved inventory was mutated outside its transaction"
            );
            let replacement = self
                .snapshot
                .inventory
                .iter()
                .find(|entry| entry.slot as usize == slot)
                .map_or_else(ItemStack::empty, |entry| entry.item.clone());
            inventory.set_item(slot, replacement);
        }
        inventory.set_changed();
    }
}

impl Drop for StoredInventoryReservation {
    fn drop(&mut self) {
        self.player.inventory.lock().transaction_reserved = false;
        self.player
            .session
            .inventory_transaction_active
            .store(false, Ordering::Release);
        self.player.finish_deferred_death_inventory();
        self.player.send_inventory_to_remote();
    }
}

#[cfg(test)]
mod tests;
