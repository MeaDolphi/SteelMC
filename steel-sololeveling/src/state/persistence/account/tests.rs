//! Account-batch rules: what a reward write accepts, and what a commit installs.

use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

use steel_core::player::player_data_storage::transaction::TransactionCommit;
use uuid::Uuid;

use super::{AccountUpdate, PreparedCommit};
use crate::combat_reward::HunterReward;
use crate::economy::SystemPoints;
use crate::player_data::SoloLevelingPlayerData;
use crate::state::SoloLevelingState;

fn state(name: &str) -> (SoloLevelingState, PathBuf) {
    let directory = env::temp_dir().join(format!("sololeveling-{name}-{}", Uuid::new_v4()));
    let state = SoloLevelingState::new(directory.clone()).expect("state should initialize");
    (state, directory)
}

/// Starts a session whose account is already loaded from core storage.
fn begin(state: &SoloLevelingState, uuid: Uuid, points: u64) {
    let mut data = SoloLevelingPlayerData::new(uuid);
    let mut system_points = SystemPoints::zero();
    assert!(system_points.credit(points));
    data.system_points = system_points;
    state.begin_session(uuid, data);
    state
        .persistence
        .lock()
        .expected
        .insert(uuid, Some(b"{}".to_vec()));
}

#[test]
fn a_reward_that_overflows_system_points_is_refused() {
    let (state, directory) = state("account-overflow");
    let uuid = Uuid::new_v4();
    let mut points = SystemPoints::zero();
    assert!(points.credit(u64::MAX));
    let mut data = SoloLevelingPlayerData::new(uuid);
    data.system_points = points;
    state.begin_session(uuid, data);
    state
        .persistence
        .lock()
        .expected
        .insert(uuid, Some(b"{}".to_vec()));

    let reward = HunterReward {
        experience: 1,
        system_points: 1,
    };
    assert!(PreparedCommit::rewards(&state, vec![(uuid, reward)]).is_err());
    fs::remove_dir_all(directory).expect("cleanup");
}

#[test]
fn a_reward_needs_an_account_loaded_from_core_storage() {
    let (state, directory) = state("account-unloaded");
    let uuid = Uuid::new_v4();
    state.begin_session(uuid, SoloLevelingPlayerData::new(uuid));
    let reward = HunterReward {
        experience: 10,
        system_points: 2,
    };
    assert!(PreparedCommit::rewards(&state, vec![(uuid, reward)]).is_err());
    fs::remove_dir_all(directory).expect("cleanup");
}

#[test]
fn a_confirmed_commit_installs_the_reward_and_releases_the_balance() {
    let (state, directory) = state("account-commit");
    let uuid = Uuid::new_v4();
    begin(&state, uuid, 0);
    let reward = HunterReward {
        experience: 40,
        system_points: 8,
    };
    let prepared = PreparedCommit::rewards(&state, vec![(uuid, reward)]).expect("prepared");
    assert!(
        state
            .with_session_mut(uuid, |session| session.economic_busy)
            .unwrap_or(false)
    );

    assert!(
        prepared
            .complete(&state, &Ok(TransactionCommit::Committed))
            .confirmed
    );

    let (experience, points, busy) = state
        .with_session_mut(uuid, |session| {
            (
                session.stats.experience,
                session.system_points.amount(),
                session.economic_busy,
            )
        })
        .expect("session");
    assert_eq!(experience, 40);
    assert_eq!(points, 8);
    assert!(!busy);
    fs::remove_dir_all(directory).expect("cleanup");
}

#[test]
fn a_failed_commit_marks_the_account_unusable_without_crediting_it() {
    let (state, directory) = state("account-failure");
    let uuid = Uuid::new_v4();
    begin(&state, uuid, 0);
    let reward = HunterReward {
        experience: 40,
        system_points: 8,
    };
    let prepared = PreparedCommit::rewards(&state, vec![(uuid, reward)]).expect("prepared");

    assert!(
        !prepared
            .complete(&state, &Err(io::Error::other("storage unavailable")))
            .committed
    );

    let (experience, points, busy) = state
        .with_session_mut(uuid, |session| {
            (
                session.stats.experience,
                session.system_points.amount(),
                session.economic_busy,
            )
        })
        .expect("session");
    assert_eq!(experience, 0);
    assert_eq!(points, 0);
    assert!(!busy);
    assert!(state.persistence.lock().failed.contains(&uuid));
    fs::remove_dir_all(directory).expect("cleanup");
}

#[test]
fn two_updates_for_one_account_are_refused_as_one_batch() {
    let (state, directory) = state("account-duplicate");
    let uuid = Uuid::new_v4();
    begin(&state, uuid, 0);
    let snapshot = state
        .with_session_mut(uuid, |session| session.saved_data(uuid))
        .expect("session");
    let prepared = PreparedCommit::prepare(&state, vec![snapshot.clone(), snapshot], Vec::new());
    assert!(prepared.is_err());

    let prepared = PreparedCommit::prepare(&state, Vec::new(), Vec::new());
    assert!(prepared.is_err());
    fs::remove_dir_all(directory).expect("cleanup");
}

#[test]
fn won_updates_and_progress_updates_share_one_commit() {
    let (state, directory) = state("account-mixed");
    let sender = Uuid::new_v4();
    let recipient = Uuid::new_v4();
    begin(&state, sender, 0);
    begin(&state, recipient, 0);
    let mut sender_data = state
        .with_session_mut(sender, |session| session.saved_data(sender))
        .expect("sender");
    sender_data.won.credit(100);
    let mut recipient_data = state
        .with_session_mut(recipient, |session| session.saved_data(recipient))
        .expect("recipient");
    let (sender_balance, recipient_balance) = sender_data
        .won
        .payment(recipient_data.won, 30)
        .expect("payment");
    sender_data.won = sender_balance;
    recipient_data.won = recipient_balance;

    let prepared = PreparedCommit::prepare(
        &state,
        vec![sender_data, recipient_data],
        vec![
            AccountUpdate::Won {
                uuid: sender,
                balance: sender_balance,
            },
            AccountUpdate::Won {
                uuid: recipient,
                balance: recipient_balance,
            },
        ],
    )
    .expect("prepared");
    assert!(
        prepared
            .complete(&state, &Ok(TransactionCommit::Committed))
            .confirmed
    );

    assert_eq!(
        state.with_session_mut(sender, |session| session.won.amount()),
        Some(70)
    );
    assert_eq!(
        state.with_session_mut(recipient, |session| session.won.amount()),
        Some(30)
    );
    fs::remove_dir_all(directory).expect("cleanup");
}
