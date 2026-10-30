//! Account-batch rules: what a reward write accepts, and what a commit installs.

use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

use steel_core::{
    config::StorageSelection,
    player::player_data_storage::{
        PlayerDataStorage as CoreStorage, transaction::TransactionCommit,
    },
};
use steel_utils::Identifier;
use uuid::Uuid;

use super::{AccountUpdate, PreparedCommit, load_account};
use crate::combat_reward::HunterReward;
use crate::economy::{SystemPoints, Won};
use crate::guild::GuildRegistry;
use crate::organization::{Event, Objective, OrganizationError, Template};
use crate::player_data::{PlayerDataStorage, SoloLevelingPlayerData};
use crate::rank::HunterRank;
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
fn contract_commit_preserves_concurrent_progress_and_daily_allowance() {
    for committed in [false, true] {
        let (state, directory) = state("organization-race");
        let uuid = Uuid::new_v4();
        begin(&state, uuid, 0);
        let boss = Template {
            objective: Objective::Boss,
            rank: HunterRank::E,
        };
        let dungeon = Template {
            objective: Objective::Dungeons,
            rank: HunterRank::E,
        };
        let extra = Template {
            objective: Objective::Mobs,
            rank: HunterRank::E,
        };
        let today = crate::daily::today();
        state.with_session_mut(uuid, |session| {
            session
                .organization
                .accept(boss, HunterRank::E, today)
                .expect("boss");
            session
                .organization
                .accept(dungeon, HunterRank::E, today)
                .expect("dungeon");
            session
                .organization
                .accept(extra, HunterRank::E, today)
                .expect("extra");
            session.organization.cancel(2, today).expect("cancel extra");
            session.organization.observe(Event::Boss(HunterRank::E));
        });
        let mut snapshot = state
            .with_session_mut(uuid, |session| session.saved_data(uuid))
            .expect("session");
        let contract = snapshot.organization.turn_in(0).expect("complete");
        let prepared = PreparedCommit::prepare(
            &state,
            vec![snapshot],
            vec![AccountUpdate::Organization {
                uuid,
                index: 0,
                contract,
            }],
        )
        .expect("prepare");
        assert_eq!(
            state.cancel_personal_contract(uuid, 0),
            Err(OrganizationError::Storage)
        );
        assert_eq!(
            state.accept_personal_contract(uuid, extra),
            Err(OrganizationError::Storage)
        );
        state.with_session_mut(uuid, |session| {
            session.organization.mark_used(extra, today);
            session.organization.observe(Event::Dungeon(HunterRank::E));
        });
        let result = if committed {
            Ok(TransactionCommit::Committed)
        } else {
            Err(io::Error::other("injected"))
        };
        prepared.complete(&state, &result);
        if committed {
            assert_eq!(
                state.accept_personal_contract(uuid, extra),
                Err(OrganizationError::UsedToday)
            );
        }
        state.with_session_mut(uuid, |session| {
            assert!(session.organization.used_today(extra, today));
            let index = usize::from(!committed);
            assert_eq!(session.organization.contracts[index].template, dungeon);
            assert_eq!(session.organization.contracts[index].progress, 1);
            assert_eq!(
                session.organization.contracts.len(),
                if committed { 1 } else { 2 }
            );
        });
        fs::remove_dir_all(directory).expect("cleanup");
    }
}

#[tokio::test]
async fn sqlite_starts_fresh_without_importing_legacy_account_json() {
    let directory = env::temp_dir().join(format!("sololeveling-sqlite-fresh-{}", Uuid::new_v4()));
    let legacy = PlayerDataStorage::new(directory.join("legacy"));
    legacy.init().expect("initialize legacy storage");
    let uuid = Uuid::new_v4();
    let mut old_account = SoloLevelingPlayerData::new(uuid);
    old_account.won = Won::zero();
    old_account.won.credit(900);
    legacy.save(&old_account).expect("save legacy account");

    let selection = StorageSelection {
        kind: Identifier::from_steel("sqlite"),
        config: None,
    };
    let sqlite = CoreStorage::from_selection(directory.join("database"), &selection)
        .await
        .expect("open fresh SQLite storage");
    let loaded = load_account(&sqlite, legacy.clone(), uuid)
        .await
        .expect("load account");

    assert_eq!(loaded.data.won.amount(), Won::starting_balance().amount());
    assert!(loaded.bytes.is_none());
    assert!(
        legacy.path_for(uuid).exists(),
        "legacy data remains untouched"
    );
    fs::remove_dir_all(directory).expect("cleanup");
}

#[test]
fn a_guild_registry_candidate_is_installed_only_after_its_document_commits() {
    let (state, directory) = state("guild-document-commit");
    let guildmaster = Uuid::new_v4();
    let key = Identifier::new_static("sololeveling", "guilds/registry");
    let mut candidate = GuildRegistry::empty();
    candidate
        .create("Hunters".to_owned(), guildmaster)
        .expect("create candidate guild");
    let bytes = candidate.to_document().expect("encode candidate");

    let failed = PreparedCommit::document(key.clone(), None, bytes.clone())
        .with_guild_registry(candidate.clone());
    assert!(
        !failed
            .complete(&state, &Err(io::Error::other("injected storage failure")))
            .committed
    );
    assert!(state.guilds.lock().guild_of(guildmaster).is_none());

    let committed = PreparedCommit::document(key, None, bytes).with_guild_registry(candidate);
    assert!(
        committed
            .complete(&state, &Ok(TransactionCommit::Committed))
            .confirmed
    );
    assert_eq!(
        state
            .guilds
            .lock()
            .guild_of(guildmaster)
            .map(|guild| guild.name.as_str()),
        Some("Hunters")
    );
    fs::remove_dir_all(directory).expect("cleanup");
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
