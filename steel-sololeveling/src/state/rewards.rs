//! Gate rewards and the hunter's quest book applied to live hunters.
//!
//! The Association pays a bounty for every cleared gate and a daily salary
//! to hunters who cleared one that day. Gameplay events are counted as
//! [`Goal`]s toward the System errands, the weekly trials and the Hunter's
//! Path; whatever they finish is paid at once.

use steel_core::entity::Entity as _;
use steel_core::player::Player;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use super::SoloLevelingState;
use crate::castle;
use crate::combat_reward::HunterReward;
use crate::daily;
use crate::item::chest::ChestKind;
use crate::organization::{Event, rank_multiplier};
use crate::quest_book::{
    self, Achievement, Finished, Goal, Standing, errand_experience, errand_points, trial_won,
};
use crate::rank::HunterRank;

/// Association bounty for an E-rank gate, shared by the survivors.
const GATE_BOUNTY: u64 = 40;
/// Association salary of an E-rank hunter.
const SALARY: u64 = 100;
/// Longest move between two counts that still counts as running.
const RUN_STEP_LIMIT: f64 = 12.0;
/// Blocks counted together toward running errands.
const RUN_BATCH: f64 = 10.0;

/// A cleared gate, as the dungeon runtime reports it.
#[derive(Debug, Clone)]
pub struct GateClear {
    /// Hunters alive at the exit.
    pub survivors: Vec<Uuid>,
    /// Raid contribution of each survivor, in the same order.
    pub scores: Vec<u64>,
    /// Effective rank of the gate.
    pub rank: HunterRank,
    /// Whether it was a red gate.
    pub red: bool,
    /// Demon Castle floor, if the gate was one.
    pub floor: Option<u32>,
    /// Seconds from the entry to the exit.
    pub seconds: u64,
    /// Hunters who died inside.
    pub deaths: u32,
    /// Hunters who entered.
    pub party: usize,
}

/// Splits `pool` by `scores`, evenly when nobody scored.
fn shares(pool: u64, scores: &[u64]) -> Vec<u64> {
    let total: u64 = scores.iter().sum();
    let count = scores.len().max(1) as u64;
    scores
        .iter()
        .map(|score| {
            pool.saturating_mul(*score)
                .checked_div(total)
                .unwrap_or(pool / count)
        })
        .collect()
}

impl SoloLevelingState {
    /// Pays the bounty and salaries of a cleared gate and counts it.
    pub fn on_gate_cleared(&self, clear: &GateClear) {
        let mut pool = GATE_BOUNTY * rank_multiplier(clear.rank);
        if clear.red {
            pool = pool * 3 / 2;
        }
        let today = daily::today();
        self.record_organization_event(
            clear.survivors.clone(),
            Event::Gate {
                rank: clear.rank,
                red: clear.red,
                clean: clear.deaths == 0,
                fast: clear.seconds < quest_book::FAST_GATE_SECONDS,
            },
        );
        if clear.floor.is_some() {
            self.record_organization_event(clear.survivors.clone(), Event::CastleFloor);
        }
        for (uuid, share) in clear.survivors.iter().zip(shares(pool, &clear.scores)) {
            if share > 0 {
                self.credit_won(*uuid, share);
                self.notify(*uuid, &format!("Премия Ассоциации за врата: +{share} Won."));
            }
            let salary = self
                .with_session_mut(*uuid, |session| {
                    if session.quests.salary_day == today {
                        return None;
                    }
                    session.quests.salary_day = today;
                    Some(SALARY * rank_multiplier(session.stats.rank()))
                })
                .flatten();
            if let Some(salary) = salary {
                self.credit_won(*uuid, salary);
                self.notify(
                    *uuid,
                    &format!("Жалование Ассоциации за день: +{salary} Won."),
                );
            }
            let own = self.hunter_rank(*uuid) == Some(clear.rank);
            let goals = [
                (Goal::GateCleared, true),
                (Goal::BossKill, true),
                (Goal::OwnRankGate, own),
                (Goal::CleanGate, clear.deaths == 0),
                (
                    Goal::FastGate,
                    clear.seconds < quest_book::FAST_GATE_SECONDS,
                ),
                (Goal::PartyGate, clear.party >= 2),
                (Goal::RedGate, clear.red),
                (Goal::CastleFloor, clear.floor.is_some()),
            ];
            for (goal, happened) in goals {
                if happened {
                    self.record_goal(*uuid, goal, 1);
                }
            }
            self.roll_temple_key(*uuid, clear);
        }
    }

    /// Counts `amount` of `goal` for a hunter and pays what it finished.
    pub fn record_goal(&self, uuid: Uuid, goal: Goal, amount: u32) {
        let now_day = daily::today();
        let week = castle::week_of(now_day);
        let Some((finished, rank, standing)) = self.with_session_mut(uuid, |session| {
            let rank = session.stats.rank();
            let class = session.class.class;
            session
                .quests
                .roll_over(now_day, week, rank, class, rand::random);
            let finished = session.quests.record(goal, amount);
            let standing = Standing {
                rank,
                level: session.stats.level,
                castle_best: session.castle.best,
            };
            (finished, rank, standing)
        }) else {
            return;
        };
        self.pay_finished(uuid, rank, &finished);
        let reached = self.check_path(uuid, standing);
        if (finished != Finished::default() || !reached.is_empty())
            && let Err(error) = self.save_session(uuid)
        {
            log::warn!("sololeveling: failed to persist quests of {uuid}: {error}");
        }
    }

    /// Checks the Hunter's Path for rank, level and castle steps.
    pub fn refresh_path(&self, uuid: Uuid) {
        let Some(standing) = self.with_session_mut(uuid, |session| Standing {
            rank: session.stats.rank(),
            level: session.stats.level,
            castle_best: session.castle.best,
        }) else {
            return;
        };
        if !self.check_path(uuid, standing).is_empty()
            && let Err(error) = self.save_session(uuid)
        {
            log::warn!("sololeveling: failed to persist the path of {uuid}: {error}");
        }
    }

    fn check_path(&self, uuid: Uuid, standing: Standing) -> Vec<Achievement> {
        let reached = self
            .with_session_mut(uuid, |session| session.quests.check_path(standing))
            .unwrap_or_default();
        for step in &reached {
            let (won, points) = step.reward();
            self.credit_won(uuid, won);
            self.award(
                uuid,
                HunterReward {
                    experience: 0,
                    system_points: points,
                },
            );
            self.notify(
                uuid,
                &format!(
                    "Путь охотника: «{}» — +{won} Won, +{points} очков Системы.",
                    step.title()
                ),
            );
        }
        reached
    }

    fn pay_finished(&self, uuid: Uuid, rank: HunterRank, finished: &Finished) {
        for errand in &finished.errands {
            self.award(
                uuid,
                HunterReward {
                    experience: errand_experience(rank),
                    system_points: errand_points(rank),
                },
            );
            self.notify(
                uuid,
                &format!(
                    "Поручение выполнено: {} — +{} очков Системы.",
                    quest_book::errand_title(errand.goal, errand.target),
                    errand_points(rank)
                ),
            );
        }
        if finished.errand_chest {
            self.queue_system_loot(uuid, ChestKind::Normal.stack());
            self.notify(
                uuid,
                "Все поручения дня выполнены: Сундук Системы в награду.",
            );
        }
        for trial in &finished.trials {
            self.credit_won(uuid, trial_won(rank));
            self.notify(
                uuid,
                &format!(
                    "Испытание недели пройдено: {} — +{} Won.",
                    quest_book::errand_title(trial.goal, trial.target),
                    trial_won(rank)
                ),
            );
        }
        if finished.trial_chest {
            self.queue_system_loot(uuid, ChestKind::Sacred.stack());
            self.notify(
                uuid,
                "Все испытания недели пройдены: Священный сундук в награду.",
            );
        }
    }

    /// Counts running, and now and then checks the path and pays castle
    /// prizes; runs on the player tick.
    pub fn tick_quests(&self, player: &Player) {
        let tick = player.tick_count();
        if tick % 20 != 0 {
            return;
        }
        let uuid = player.gameprofile.id();
        let position = player.position();
        let ran = self
            .with_session_mut(uuid, |session| {
                if let Some(last) = session.run_position.replace(position) {
                    let dx = position.x - last.x;
                    let dz = position.z - last.z;
                    // Teleports and world changes do not count.
                    let step = (dx * dx + dz * dz).sqrt();
                    if step < RUN_STEP_LIMIT {
                        session.run_carry += step;
                    }
                }
                let whole = session.run_carry.floor();
                if whole >= RUN_BATCH {
                    session.run_carry -= whole;
                    return whole as u32;
                }
                0
            })
            .unwrap_or(0);
        if ran > 0 {
            self.record_goal(uuid, Goal::Distance, ran);
        }
        if tick % 200 == 0 {
            self.refresh_path(uuid);
            self.pay_castle_prizes(uuid);
            self.roll_quests(uuid);
        }
    }

    /// Pays the Demon Castle prizes of finished weeks owed to a hunter.
    fn pay_castle_prizes(&self, uuid: Uuid) {
        let week = castle::week_of(daily::today());
        let prize = {
            let mut file = self.castle_ranking.lock();
            let rolled = file.ranking.roll_week(week);
            let prize = file.ranking.take_owed(uuid);
            if (rolled || prize > 0)
                && let Err(error) = file.save()
            {
                log::warn!("sololeveling: could not save the castle ranking: {error}");
            }
            prize
        };
        if prize > 0 {
            self.credit_won(uuid, prize);
            self.notify(uuid, &format!("Приз недели Замка демонов: +{prize} Won."));
        }
    }

    /// Rolls today's errands and this week's trials if they are stale.
    pub fn roll_quests(&self, uuid: Uuid) {
        let day = daily::today();
        let week = castle::week_of(day);
        let rolled = self
            .with_session_mut(uuid, |session| {
                let rank = session.stats.rank();
                let class = session.class.class;
                session
                    .quests
                    .roll_over(day, week, rank, class, rand::random)
            })
            .unwrap_or(false);
        if rolled && let Err(error) = self.save_session(uuid) {
            log::warn!("sololeveling: failed to persist quests of {uuid}: {error}");
        }
    }

    /// Sends a System line to an online hunter.
    pub(crate) fn notify(&self, uuid: Uuid, text: &str) {
        if let Some(player) = self
            .online_players()
            .into_iter()
            .find(|player| player.gameprofile.id() == uuid)
        {
            player.send_message(&TextComponent::plain(text.to_owned()).color(Color::Aqua));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::shares;

    #[test]
    fn the_bounty_follows_contribution_or_splits_evenly() {
        assert_eq!(shares(100, &[3, 1]), vec![75, 25]);
        assert_eq!(shares(90, &[0, 0, 0]), vec![30, 30, 30]);
        assert_eq!(shares(80, &[5]), vec![80]);
    }
}
