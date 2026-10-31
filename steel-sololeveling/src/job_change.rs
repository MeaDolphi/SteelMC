//! Second Awakening: the class evolution trial against Knight Igris.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use steel_core::player::Player;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use crate::dungeon::boss::{BossGear, BossKind, DungeonBoss, ZOMBIE_ATTACK_DAMAGE};
use crate::rank::HunterRank;
use crate::scenario::{self, ScenarioKind};
use crate::state::SoloLevelingState;

/// Hunter level at which the Second Awakening becomes available.
pub const JOB_CHANGE_LEVEL: u32 = 40;

/// Progress of the Second Awakening quest chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum JobChangeProgress {
    /// Not yet started.
    #[default]
    None,
    /// The trial is available but has not been started.
    Available,
    /// The hunter has entered the arena.
    Started,
    /// Igris has been defeated and the class evolved.
    Completed,
}

impl SoloLevelingState {
    /// Returns whether `player` can begin the Second Awakening right now.
    pub fn job_change_available(&self, uuid: Uuid) -> bool {
        self.with_session_mut(uuid, |session| {
            let level = session.stats.level;
            let class = session.class.class;
            let evolved = session.class.evolved;
            let progress = session.quests.job_change;
            level >= JOB_CHANGE_LEVEL
                && class.is_some()
                && !evolved
                && progress != JobChangeProgress::Completed
        })
        .unwrap_or(false)
    }

    /// Starts the Second Awakening scenario for `player` if it is available.
    pub fn start_job_change(self: &Arc<Self>, player: &Player) -> Result<(), String> {
        let uuid = player.gameprofile.id();
        if !self.job_change_available(uuid) {
            return Err("Второе пробуждение недоступно. Требуется 40 уровень, выбранный класс и непробуждённый класс.".to_owned());
        }
        let rank = self.hunter_rank(uuid).unwrap_or(HunterRank::E);
        let boss = igris_boss(rank);
        let name = TextComponent::plain("Рыцарь Игрис")
            .color(Color::DarkRed)
            .bold(true);
        self.with_session_mut(uuid, |session| {
            session.quests.job_change = JobChangeProgress::Started
        });
        scenario::start(
            Arc::clone(self),
            player,
            ScenarioKind::JobChange,
            rank,
            boss,
            name,
        )
    }
}

/// Returns the boss data for Knight Igris at `rank`.
#[must_use]
pub fn igris_boss(rank: HunterRank) -> DungeonBoss {
    let gear = BossGear::for_rank(rank);
    let damage_multiplier = 6.0;
    DungeonBoss {
        kind: BossKind::Warlord,
        scale: 2.0,
        health_multiplier: 40.0,
        damage_multiplier,
        speed_multiplier: 1.0,
        power: ZOMBIE_ATTACK_DAMAGE * damage_multiplier,
        armor: 10.0,
        armor_toughness: 8.0,
        knockback_resistance: 0.8,
        attack_knockback: 1.5,
        gear,
        abilities: BossKind::Warlord.abilities(rank),
    }
}
