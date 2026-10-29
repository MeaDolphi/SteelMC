use super::*;
use crate::{
    behavior::init_behaviors,
    entity::damage::DamageSource,
    inventory::equipment::{EntityEquipment, EquipmentSlot},
    test_support::{TestPlayerBuilder, fresh_test_world, insert_ready_full_chunk},
};
use steel_registry::{vanilla_damage_types, vanilla_game_rules, vanilla_items};
use steel_utils::ChunkPos;

#[test]
fn staged_items_are_invisible_and_rejection_releases_the_reservation() {
    let fixture = fresh_test_world("inventory_reservation_reject");
    let player = TestPlayerBuilder::new(Arc::clone(&fixture.world), "Buyer", 1).build();
    player
        .inventory
        .lock()
        .set_item(9, ItemStack::new(&vanilla_items::DIAMOND));
    let mut reservation = player.reserve_stored_inventory().expect("reserve");
    assert!(player.inventory_transaction_active());
    assert!(player.reserve_stored_inventory().is_err());
    assert!(reservation.set_item(0, ItemStack::empty()).is_err());
    assert!(reservation.set_item(38, ItemStack::empty()).is_err());
    reservation
        .set_item(9, ItemStack::new(&vanilla_items::EMERALD))
        .expect("stage");
    assert_eq!(
        player.inventory.lock().get_item(9).item(),
        &*vanilla_items::DIAMOND
    );
    let mut incoming = ItemStack::new(&vanilla_items::GOLD_INGOT);
    assert!(!player.inventory.lock().add(&mut incoming));
    assert_eq!(incoming.count(), 1);
    drop(reservation);
    assert!(!player.inventory_transaction_active());
    assert_eq!(
        player.inventory.lock().get_item(9).item(),
        &*vanilla_items::DIAMOND
    );
    assert!(player.inventory.lock().add(&mut incoming));
}

#[test]
fn combat_equipment_damage_is_not_overwritten_by_a_stored_item_commit() {
    let fixture = fresh_test_world("inventory_reservation_equipment");
    let player = TestPlayerBuilder::new(Arc::clone(&fixture.world), "Buyer", 1).build();
    player.inventory.lock().set(
        EquipmentSlot::Chest,
        ItemStack::new(&vanilla_items::IRON_CHESTPLATE),
    );
    let mut reservation = player.reserve_stored_inventory().expect("reserve");
    reservation
        .set_item(9, ItemStack::new(&vanilla_items::DIAMOND))
        .expect("stage");
    player.hurt_armor(
        &DamageSource::environment(&vanilla_damage_types::GENERIC),
        8.0,
    );
    let damage = player
        .inventory
        .lock()
        .get_ref(EquipmentSlot::Chest)
        .get_damage_value();
    assert!(damage > 0);
    reservation.install();
    drop(reservation);
    assert_eq!(
        player
            .inventory
            .lock()
            .get_ref(EquipmentSlot::Chest)
            .get_damage_value(),
        damage
    );
    assert_eq!(
        player.inventory.lock().get_item(9).item(),
        &*vanilla_items::DIAMOND
    );
}

#[test]
fn death_happens_immediately_but_drops_use_the_final_inventory() {
    let fixture = fresh_test_world("inventory_reservation_death");
    insert_ready_full_chunk(&fixture.world, ChunkPos::new(0, 0));
    assert!(
        fixture
            .world
            .set_game_rule(&vanilla_game_rules::KEEP_INVENTORY, false)
    );
    let player = TestPlayerBuilder::new(Arc::clone(&fixture.world), "Buyer", 1).build();
    player
        .inventory
        .lock()
        .set_item(9, ItemStack::new(&vanilla_items::DIAMOND));
    let mut reservation = player.reserve_stored_inventory().expect("reserve");
    reservation
        .set_item(9, ItemStack::new(&vanilla_items::EMERALD))
        .expect("stage");
    player.set_health(0.0);
    player.die(&DamageSource::environment(
        &vanilla_damage_types::GENERIC_KILL,
    ));
    assert!(player.is_dead_or_dying());
    assert!(
        !player.inventory.lock().is_empty(),
        "death must not consume a reserved input"
    );
    assert!(player.session.deferred_death_inventory.lock().is_some());
    reservation.install();
    drop(reservation);
    assert!(player.inventory.lock().is_empty());
    assert!(player.session.deferred_death_inventory.lock().is_none());
}

#[test]
fn held_death_protection_can_still_be_consumed_while_storage_is_reserved() {
    let fixture = fresh_test_world("inventory_reservation_totem");
    init_behaviors();
    let player = TestPlayerBuilder::new(Arc::clone(&fixture.world), "Buyer", 1).build();
    player.inventory.lock().set(
        EquipmentSlot::OffHand,
        ItemStack::new(&vanilla_items::TOTEM_OF_UNDYING),
    );
    let mut reservation = player.reserve_stored_inventory().expect("reserve");
    reservation
        .set_item(9, ItemStack::new(&vanilla_items::DIAMOND))
        .expect("stage");
    player.set_health(0.0);
    assert!(
        player.check_totem_death_protection(&DamageSource::environment(
            &vanilla_damage_types::GENERIC
        ))
    );
    assert!(player.get_health() > 0.0);
    assert!(
        player
            .inventory
            .lock()
            .get_ref(EquipmentSlot::OffHand)
            .is_empty()
    );
    reservation.install();
    drop(reservation);
    assert!(
        player
            .inventory
            .lock()
            .get_ref(EquipmentSlot::OffHand)
            .is_empty()
    );
    assert_eq!(
        player.inventory.lock().get_item(9).item(),
        &*vanilla_items::DIAMOND
    );
}
