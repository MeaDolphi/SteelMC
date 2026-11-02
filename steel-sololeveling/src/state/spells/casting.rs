//! Reading a spell before its release; no game tick waits for a timer.

use super::*;

#[derive(Debug, Default)]
pub(super) struct CastingVisuals {
    active: FxHashMap<Uuid, CastingVisual>,
}

#[derive(Debug)]
struct CastingVisual {
    world: Identifier,
    rune: Option<i32>,
    next_pulse: u64,
}

struct CastCheck {
    spell: SpellId,
    cost: f32,
    wrath: bool,
    rank: HunterRank,
}

pub(super) fn tick(state: &SoloLevelingState, server: &Server, now: u64) {
    let (requests, mut visuals) = {
        let mut runtime = state.spell_runtime.lock();
        (
            mem::take(&mut runtime.casts),
            mem::take(&mut runtime.casting_visuals),
        )
    };
    visuals.active.retain(|owner, visual| {
        if find_player(server, *owner).is_some() {
            return true;
        }
        remove_rune(server, visual);
        false
    });
    for uuid in requests {
        let Some(player) = find_player(server, uuid) else {
            continue;
        };
        begin(state, &player, now, &mut visuals);
    }
    for player in server.get_players() {
        advance(state, &player, server, now, &mut visuals);
    }
    state.spell_runtime.lock().casting_visuals = visuals;
}

fn check(
    state: &SoloLevelingState,
    player: &Arc<Player>,
    gear: SpellGear,
    now: u64,
    starting: bool,
) -> Option<Result<CastCheck, String>> {
    if !holds_grimoire(player) {
        return Some(Err("Гримуар должен быть в руке.".to_owned()));
    }
    state.with_session_mut(player.gameprofile.id(), |session| {
        if session.class.class != Some(HunterClass::Mage) {
            return Err(SpellError::NotAMage.message().to_owned());
        }
        let spell = session
            .spells
            .selected_spell()
            .ok_or_else(|| "Нет подготовленных заклинаний.".to_owned())?;
        let magic = &session.magic;
        if starting && let Some(reading) = magic.casting {
            return Err(format!(
                "Чтение «{}»: ещё {:.1} с.",
                reading.spell.info().name,
                reading.remaining_millis(now) as f64 / 1_000.0
            ));
        }
        if let Some(deadline) = magic.cooldowns.get(&spell)
            && *deadline > now
        {
            return Err(format!(
                "{}: перезарядка {:.1} с.",
                spell.info().name,
                (*deadline - now) as f64 / 1_000.0
            ));
        }
        let wrath = magic.wrath_until > now;
        let free = gear.free_every > 0 && (magic.casts + 1) % gear.free_every == 0;
        let cost = if free {
            0.0
        } else {
            cast_cost(
                gear,
                spell,
                wrath && spell.info().school == School::Destruction,
            )
        };
        let maximum = spell::max_mana(session.stats.intelligence) + gear.mana;
        if magic.mana.unwrap_or(maximum) < cost {
            return Err(format!("Не хватает маны: нужно {}.", cost.round()));
        }
        Ok(CastCheck {
            spell,
            cost,
            wrath,
            rank: session.stats.rank(),
        })
    })
}

fn begin(state: &SoloLevelingState, player: &Arc<Player>, now: u64, visuals: &mut CastingVisuals) {
    if player.is_dead_or_dying() {
        return;
    }
    let uuid = player.gameprofile.id();
    let gear = state.spell_gear(player);
    let Some(check) = check(state, player, gear, now, true) else {
        return;
    };
    let spell = match check {
        Ok(check) => check.spell,
        Err(message) => {
            refuse(player, message);
            return;
        }
    };
    state.with_session_mut(uuid, |session| {
        session.magic.casting = Some(CastProgress::new(spell, now));
    });
    let world = player.get_world();
    let feet = player.position();
    fx::cast(&world, spell, feet);
    let rune = if spell.cast_millis() == 0 {
        None
    } else {
        state.spell_runtime.lock().visuals.spawn(
            &world,
            uuid,
            fx::school_assets(spell).0,
            fx::runtime::Motion::Follow {
                target: uuid,
                offset: DVec3::new(0.0, 0.06, 0.0),
            },
            3.0,
            true,
            2.4,
            now,
            now.saturating_add(spell.cast_millis()),
        )
    };
    if spell.cast_millis() > 0 {
        visuals.active.insert(
            uuid,
            CastingVisual {
                world: world.key.clone(),
                rune,
                next_pulse: now,
            },
        );
    }
}

fn advance(
    state: &SoloLevelingState,
    player: &Arc<Player>,
    server: &Server,
    now: u64,
    visuals: &mut CastingVisuals,
) {
    let uuid = player.gameprofile.id();
    let Some((progress, selected, is_mage)) = state
        .with_session_mut(uuid, |session| {
            Some((
                session.magic.casting?,
                session.spells.selected_spell(),
                session.class.class == Some(HunterClass::Mage),
            ))
        })
        .flatten()
    else {
        return;
    };
    let wrong_world = visuals
        .active
        .get(&uuid)
        .is_some_and(|visual| visual.world != player.get_world().key);
    let refusal = if progress.interrupted {
        Some("Чтение прервано полученным уроном.")
    } else if player.is_dead_or_dying() {
        Some("Чтение прервано смертью.")
    } else if !is_mage {
        Some("Чтение прервано сменой класса.")
    } else if wrong_world {
        Some("Чтение прервано сменой мира.")
    } else if selected != Some(progress.spell) {
        Some("Чтение прервано сменой заклинания.")
    } else if !holds_grimoire(player) {
        Some("Чтение прервано: гримуар больше не в руке.")
    } else {
        None
    };
    if let Some(message) = refusal {
        state.with_session_mut(uuid, |session| session.magic.casting = None);
        clear_visual(server, uuid, visuals);
        tell(player, message, Color::Red);
        return;
    }
    if now < progress.ready_at {
        pulse(player, progress, now, visuals);
        if player.tick_count() % 5 == 0 {
            player.send_action_bar(
                TextComponent::plain(format!(
                    "Чтение «{}» · {:.1} с",
                    progress.spell.info().name,
                    progress.remaining_millis(now) as f64 / 1_000.0
                ))
                .color(Color::Aqua),
            );
        }
        return;
    }
    state.with_session_mut(uuid, |session| session.magic.casting = None);
    clear_visual(server, uuid, visuals);
    release(state, player, now, progress);
}

fn release(state: &SoloLevelingState, player: &Arc<Player>, now: u64, progress: CastProgress) {
    if progress.interrupted || now < progress.ready_at {
        return;
    }
    let uuid = player.gameprofile.id();
    let gear = state.spell_gear(player);
    let Some(check) = check(state, player, gear, now, false) else {
        return;
    };
    let CastCheck {
        spell,
        cost,
        wrath,
        rank,
    } = match check {
        Ok(check) => check,
        Err(message) => {
            refuse(player, message);
            return;
        }
    };
    let caster = Caster {
        state,
        player,
        eye: DVec3::new(player.position().x, player.get_eye_y(), player.position().z),
        look: player.look_angle(),
        world: player.get_world(),
        scale: class::rank_scale(rank),
        healing: 1.0 + gear.healing,
        barrier: gear.barrier,
        burn: if gear.burning { 2 } else { 1 },
        power: if wrath && spell.info().school == School::Destruction {
            WRATH_DAMAGE
        } else {
            1.0
        },
        now,
    };
    if let Err(message) = state.perform(&caster, spell, rank) {
        refuse(player, message.to_owned());
        return;
    }
    let echo = spell.info().school == School::Destruction
        && spell != SpellId::MonarchsWrath
        && rand::random_range(0..100) < gear.echo_percent;
    if echo {
        let _ = state.perform(&caster, spell, rank);
    }
    state.with_session_mut(uuid, |session| {
        let maximum = spell::max_mana(session.stats.intelligence) + gear.mana;
        let magic = &mut session.magic;
        magic.mana = Some((magic.mana.unwrap_or(maximum) - cost).max(0.0));
        magic.casts = magic.casts.wrapping_add(1);
        magic.cooldowns.insert(spell, now + gear.cooldown_of(spell));
        magic.cooldowns.retain(|_, deadline| *deadline > now);
    });
    player.send_action_bar(
        TextComponent::plain(spell.info().name.to_owned()).color(Color::LightPurple),
    );
    state.record_goal(uuid, Goal::SpellCast, 1);
}

fn refuse(player: &Player, message: String) {
    player.send_action_bar(TextComponent::plain(message).color(Color::Red));
}

fn pulse(player: &Arc<Player>, progress: CastProgress, now: u64, visuals: &mut CastingVisuals) {
    let Some(visual) = visuals.active.get_mut(&player.gameprofile.id()) else {
        return;
    };
    if now < visual.next_pulse {
        return;
    }
    visual.next_pulse = now.saturating_add(200);
    let world = player.get_world();
    let feet = player.position();
    let theme = fx::theme(progress.spell);
    fx::spiral(&world, theme.secondary, feet, 0.8, 1.6, 2.0, 12);
    fx::ring_dust(
        &world,
        theme.tint,
        feet + DVec3::new(0.0, 0.08, 0.0),
        1.3,
        16,
    );
}

fn clear_visual(server: &Server, owner: Uuid, visuals: &mut CastingVisuals) {
    if let Some(visual) = visuals.active.remove(&owner) {
        remove_rune(server, &visual);
    }
}

fn remove_rune(server: &Server, visual: &CastingVisual) {
    let Some(world) = server.worlds.get(&visual.world) else {
        return;
    };
    if let Some(entity) = visual.rune.and_then(|id| world.get_entity_by_id(id)) {
        entity.set_removed(RemovalReason::Discarded);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use steel_core::inventory::container::Container as _;
    use steel_registry::vanilla_damage_types;

    use super::*;
    use crate::hooks::SoloLevelingHooks;
    use crate::item::grimoire;
    use crate::player_data::SoloLevelingPlayerData;
    use crate::test_support::{self, TestWorld};

    struct Fixture {
        state: Arc<SoloLevelingState>,
        player: Arc<Player>,
        visuals: CastingVisuals,
        directory: PathBuf,
        _world: TestWorld,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = std::env::temp_dir().join(format!("spell-cast-{}", Uuid::new_v4()));
            let state = Arc::new(SoloLevelingState::new(directory.clone()).expect("test state"));
            let world = test_support::world(Arc::new(SoloLevelingHooks::new(Arc::clone(&state))));
            let player = test_support::player(&world.world, "Mage", 1);
            let uuid = player.gameprofile.id();
            let mut data = SoloLevelingPlayerData::new(uuid);
            data.stats.level = 10;
            data.class.class = Some(HunterClass::Mage);
            data.spells.learn_starting();
            state.begin_session(uuid, data);
            player.inventory.lock().set_item(0, grimoire::stack(uuid));
            state.with_session_mut(uuid, |session| session.magic.mana = Some(100.0));
            Self {
                state,
                player,
                visuals: CastingVisuals::default(),
                directory,
                _world: world,
            }
        }

        fn begin(&mut self, now: u64) {
            begin(&self.state, &self.player, now, &mut self.visuals);
        }

        fn progress(&self) -> CastProgress {
            self.state
                .with_session_mut(self.player.gameprofile.id(), |session| {
                    session.magic.casting
                })
                .flatten()
                .expect("reading should be active")
        }

        fn finish_reading(&self) {
            self.state
                .with_session_mut(self.player.gameprofile.id(), |session| {
                    session.magic.casting = None;
                });
        }

        fn resources(&self) -> (f32, Option<u64>) {
            self.state
                .with_session_mut(self.player.gameprofile.id(), |session| {
                    (
                        session.magic.mana.unwrap_or_default(),
                        session.magic.cooldowns.get(&SpellId::MagicArrow).copied(),
                    )
                })
                .expect("mage session")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn an_interrupted_reading_never_spends_mana() {
        let mut fixture = Fixture::new();
        fixture.begin(0);
        let progress = fixture.progress();
        assert_eq!(progress.ready_at, 600);
        fixture.begin(100);
        assert_eq!(fixture.progress().started, 0);
        release(&fixture.state, &fixture.player, 599, progress);
        assert_eq!(fixture.resources(), (100.0, None));

        assert!(fixture.player.hurt(
            &fixture.player.get_world(),
            &DamageSource::environment(&vanilla_damage_types::ON_FIRE),
            1.0,
        ));
        let interrupted = fixture.progress();
        assert!(interrupted.interrupted);
        release(&fixture.state, &fixture.player, 600, interrupted);
        assert_eq!(fixture.resources(), (100.0, None));
        assert!(fixture.state.spell_runtime.lock().projectiles.is_empty());
    }

    #[test]
    fn a_successful_release_spends_mana_and_starts_cooldown_once() {
        let mut fixture = Fixture::new();
        fixture.begin(1_000);
        let progress = fixture.progress();
        fixture.finish_reading();
        release(&fixture.state, &fixture.player, 1_600, progress);
        assert_eq!(fixture.resources(), (88.0, Some(4_600)));
        assert_eq!(fixture.state.spell_runtime.lock().projectiles.len(), 1);

        release(&fixture.state, &fixture.player, 1_600, progress);
        assert_eq!(fixture.resources(), (88.0, Some(4_600)));
        assert_eq!(fixture.state.spell_runtime.lock().projectiles.len(), 1);
    }
}
