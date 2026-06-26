use std::sync::atomic::{AtomicUsize, Ordering};

use crate::combat::condition::{ConditionEffect, SKILL_FIRST, SKILL_LAST, SPECIALSKILL_FIRST, SPECIALSKILL_LAST, STAT_FIRST, STAT_LAST};
use crate::creatures::CreatureId;
use crate::game::{
    g_game, EVENT_CHECK_CREATURE_INTERVAL, EVENT_CREATURECOUNT, EVENT_CREATURE_THINK_INTERVAL,
    EVENT_LIGHTINTERVAL, EVENT_WORLDTIMEINTERVAL,
};
use crate::net::game_protocol::send_packet_to_player;
use crate::net::output_message::OutputMessage;
use crate::runtime::scheduler::SchedulerTask;
use crate::runtime::g_scheduler;
use crate::world::vocation::g_vocations;

static CREATURE_CHECK_INDEX: AtomicUsize = AtomicUsize::new(0);

pub fn apply_condition_effects(creature_id: CreatureId, effects: &[ConditionEffect]) {
    let mut need_speed_broadcast = false;
    let mut need_light_broadcast = false;
    let mut need_outfit_broadcast = false;
    let mut need_visible_broadcast: Option<bool> = None;
    let mut need_send_stats = false;
    let mut need_send_skills = false;
    let mut need_send_icons = false;

    {
        let mut game = g_game().lock().unwrap();
        let Some(creature) = game.get_creature_mut(creature_id) else { return };
        for effect in effects {
            match effect {
                ConditionEffect::ChangeSpeed(delta) => {
                    creature.base_mut().var_speed += delta;
                    need_speed_broadcast = true;
                }
                ConditionEffect::SetDrunkenness(d) => {
                    creature.base_mut().drunkenness = *d;
                }
                ConditionEffect::SetCreatureLight(info) => {
                    creature.base_mut().internal_light = crate::creatures::LightInfo {
                        level: info.level,
                        color: info.color,
                    };
                    need_light_broadcast = true;
                }
                ConditionEffect::RevertCreatureLight => {
                    creature.base_mut().internal_light = crate::creatures::LightInfo::default();
                    need_light_broadcast = true;
                }
                ConditionEffect::ChangeOutfit(oi) => {
                    creature.base_mut().current_outfit = crate::creatures::Outfit {
                        look_type: oi.look_type,
                        look_type_ex: oi.look_type_ex,
                        look_head: oi.look_head,
                        look_body: oi.look_body,
                        look_legs: oi.look_legs,
                        look_feet: oi.look_feet,
                        look_addons: oi.look_addons,
                        look_mount: 0,
                    };
                    need_outfit_broadcast = true;
                }
                ConditionEffect::RevertOutfit => {
                    let default_outfit = creature.base().default_outfit;
                    creature.base_mut().current_outfit = default_outfit;
                    need_outfit_broadcast = true;
                }
                ConditionEffect::SetVisible(visible) => {
                    need_visible_broadcast = Some(*visible);
                }
                ConditionEffect::SetUseDefense(b) => {
                    creature.base_mut().can_use_defense = *b;
                }
                ConditionEffect::SendStats => { need_send_stats = true; }
                ConditionEffect::SendSkills => { need_send_skills = true; }
                ConditionEffect::SendIcons => { need_send_icons = true; }
                _ => {}
            }
        }
    }

    {
        let mut game = g_game().lock().unwrap();
        let is_player = game.get_creature(creature_id).map(|c| c.is_player()).unwrap_or(false);
        if is_player {
            for effect in effects {
                match effect {
                    ConditionEffect::AddSkills(sk) => {
                        if let Some(player) = game.get_player_mut(creature_id) {
                            for (i, &val) in sk.iter().enumerate().take(SKILL_LAST + 1).skip(SKILL_FIRST) {
                                if val != 0 {
                                    player.var_skills[i] += val;
                                    need_send_skills = true;
                                }
                            }
                        }
                    }
                    ConditionEffect::AddSpecialSkills(ss) => {
                        if let Some(player) = game.get_player_mut(creature_id) {
                            for (i, &val) in ss.iter().enumerate().take(SPECIALSKILL_LAST + 1).skip(SPECIALSKILL_FIRST) {
                                if val != 0 {
                                    player.var_special_skills[i] += val;
                                    need_send_skills = true;
                                }
                            }
                        }
                    }
                    ConditionEffect::AddStats(st) => {
                        if let Some(player) = game.get_player_mut(creature_id) {
                            for (i, &val) in st.iter().enumerate().take(STAT_LAST + 1).skip(STAT_FIRST) {
                                if val != 0 {
                                    player.set_var_stats(i, val);
                                    need_send_stats = true;
                                }
                            }
                        }
                    }
                    ConditionEffect::ChangeSoul(delta) => {
                        if let Some(player) = game.get_player_mut(creature_id) {
                            if *delta > 0 {
                                let voc_id = player.vocation_id;
                                let soul_max = g_vocations()
                                    .get_vocation(voc_id)
                                    .map(|v| v.soul_max)
                                    .unwrap_or(100) as i32;
                                let gain = (*delta).min(soul_max - player.soul as i32);
                                if gain > 0 {
                                    player.soul = (player.soul as i32 + gain) as u8;
                                }
                            } else {
                                player.soul = (player.soul as i32 + *delta).max(0) as u8;
                            }
                            need_send_stats = true;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    if need_speed_broadcast {
        let (pos, speed) = {
            let game = g_game().lock().unwrap();
            if let Some(player) = game.get_player(creature_id) {
                (player.base.position, player.get_step_speed() as u32)
            } else {
                match game.get_creature(creature_id) {
                    Some(c) => {
                        let spd = c.base().get_speed().max(0) as u32;
                        (c.base().position, spd)
                    }
                    None => return,
                }
            }
        };
        crate::net::game_protocol::broadcast_change_speed(creature_id, pos, speed);
    }

    if need_light_broadcast {
        let (pos, light) = {
            let game = g_game().lock().unwrap();
            match game.get_creature(creature_id) {
                Some(c) => (c.base().position, c.base().internal_light),
                None => return,
            }
        };
        crate::net::game_protocol::broadcast_creature_light(creature_id, pos, light);
    }

    if need_outfit_broadcast {
        let (pos, outfit) = {
            let game = g_game().lock().unwrap();
            match game.get_creature(creature_id) {
                Some(c) => (c.base().position, c.base().current_outfit),
                None => return,
            }
        };
        crate::net::game_protocol::broadcast_creature_outfit(creature_id, pos, outfit);
    }

    if let Some(visible) = need_visible_broadcast {
        let (pos, is_player_creature, outfit) = {
            let game = g_game().lock().unwrap();
            match game.get_creature(creature_id) {
                Some(c) => (c.base().position, c.is_player(), c.base().current_outfit),
                None => return,
            }
        };
        crate::net::game_protocol::broadcast_creature_visible(creature_id, pos, visible, is_player_creature, outfit);
    }

    let is_player = {
        let game = g_game().lock().unwrap();
        game.get_creature(creature_id).map(|c| c.is_player()).unwrap_or(false)
    };

    for effect in effects {
        match effect {
            ConditionEffect::ConditionDamage { owner, damage, combat_type, field } => {
                use crate::combat::{Combat, CombatDamage, CombatOrigin, CombatParams};
                let mut cd = CombatDamage {
                    origin: CombatOrigin::Condition,
                    primary_type: *combat_type,
                    primary_value: *damage,
                    ..CombatDamage::default()
                };
                if *field && *owner != 0 {
                    let game = g_game().lock().unwrap();
                    let target_is_player = game.get_player(creature_id).is_some();
                    let owner_is_player = game.get_player(*owner).is_some();
                    drop(game);
                    if target_is_player && owner_is_player {
                        cd.primary_value = (cd.primary_value as f64 / 2.0).round() as i32;
                    }
                }
                let params = CombatParams::default();
                Combat::do_target_combat(if *owner != 0 { Some(*owner) } else { None }, creature_id, &mut cd, &params);
            }
            ConditionEffect::SendSpellCooldown { spell_id, ticks } => {
                crate::net::game_protocol::send_spell_cooldown_to_player(creature_id, *spell_id as u8, *ticks as u32);
            }
            ConditionEffect::SendSpellGroupCooldown { group_id, ticks } => {
                crate::net::game_protocol::send_spell_group_cooldown_to_player(creature_id, *group_id, *ticks as u32);
            }
            _ => {}
        }
    }

    if is_player {
        if need_send_stats {
            crate::net::game_protocol::send_stats_to_player(creature_id);
        }
        if need_send_skills {
            crate::net::game_protocol::send_skills_to_player(creature_id);
        }
        if need_send_icons {
            crate::net::game_protocol::send_icons_to_player(creature_id);
        }
    }
}

pub fn schedule_tick_events() {
    schedule_creature_check();
    schedule_world_time();
    schedule_light();
    schedule_spawns();
    schedule_global_think();
    schedule_global_timer();
    schedule_decay();
}

fn schedule_creature_check() {
    g_scheduler().add_event(SchedulerTask::new(EVENT_CHECK_CREATURE_INTERVAL as u32, || {
        let index = CREATURE_CHECK_INDEX.fetch_add(1, Ordering::Relaxed) % EVENT_CREATURECOUNT;
        check_creatures(index);
        schedule_creature_check();
    }));
}

fn schedule_world_time() {
    g_scheduler().add_event(SchedulerTask::new(EVENT_WORLDTIMEINTERVAL as u32, || {
        update_world_time();
        schedule_world_time();
    }));
}

fn schedule_light() {
    g_scheduler().add_event(SchedulerTask::new(EVENT_LIGHTINTERVAL as u32, || {
        check_light();
        schedule_light();
    }));
}

fn schedule_spawns() {
    g_scheduler().add_event(SchedulerTask::new(10_000, || {
        check_spawns();
        schedule_spawns();
    }));
}

fn schedule_global_think() {
    g_scheduler().add_event(SchedulerTask::new(crate::events::global::SCHEDULER_MINTICKS as u32, || {
        let due = crate::events::registry::g_script_registry().lock().unwrap()
            .global_events.collect_due_think_events();
        crate::events::global::execute_collected_events(due);
        schedule_global_think();
    }));
}

fn schedule_global_timer() {
    g_scheduler().add_event(SchedulerTask::new(1_000, || {
        let due = crate::events::registry::g_script_registry().lock().unwrap()
            .global_events.collect_due_timer_events();
        crate::events::global::execute_collected_events(due);
        schedule_global_timer();
    }));
}

fn schedule_decay() {
    g_scheduler().add_event(SchedulerTask::new(crate::game::EVENT_DECAYINTERVAL as u32, || {
        let mut game = g_game().lock().unwrap();
        game.check_decay();
        drop(game);
        schedule_decay();
    }));
}

pub fn schedule_rent_loop() {
    tokio::spawn(async {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(60_000)).await;
            let db = crate::db::g_database();
            if let Err(e) = crate::map::serialize::IOMapSerialize::pay_houses(db).await {
                tracing::warn!("payHouses failed: {e}");
            }
        }
    });
}

fn check_creatures(index: usize) {
    let creature_ids: Vec<u32> = {
        let mut game = g_game().lock().unwrap();
        let list = game.get_check_creature_list(index).clone();

        // Remove stale (creature_check = false) entries.
        game.drain_dead_from_check_list(index);
        list
    };

    for creature_id in creature_ids {
        let (is_player, still_alive) = {
            let game = g_game().lock().unwrap();
            let Some(creature) = game.get_creature(creature_id) else { continue };
            (creature.is_player(), creature.base().creature_check)
        };

        if !still_alive {
            continue;
        }

        let is_npc = {
            let game = g_game().lock().unwrap();
            game.get_creature(creature_id).map(|c| c.is_npc()).unwrap_or(false)
        };

        if is_player {
            player_on_think(creature_id);
        } else if is_npc {
            npc_think(creature_id);
        } else {
            monster_think(creature_id);
        }

        // Fire creature script onThink events.
        crate::events::dispatch::execute_creature_event_think(
            creature_id,
            crate::creatures::EVENT_CREATURE_THINK_INTERVAL as u32,
        );

        // Tick active conditions and remove expired ones.
        execute_conditions(creature_id);

        // Re-read attack target (monster_think may have set one).
        let has_attack_target = {
            let game = g_game().lock().unwrap();
            game.get_creature(creature_id)
                .map(|c| c.base().attacked_creature_id.is_some())
                .unwrap_or(false)
        };

        // Auto-attack tick (sight-clear check + doAttacking for player/monster).
        if has_attack_target {
            on_attacking(creature_id);
        }
    }
}

fn execute_conditions(creature_id: u32) {
    use crate::combat::condition::ConditionEffect;

    let (to_remove, health_delta, mana_delta, mut all_effects) = {
        let mut game = g_game().lock().unwrap();
        let Some(creature) = game.get_creature_mut(creature_id) else { return };
        let conditions = &mut creature.base_mut().conditions;
        let interval = EVENT_CREATURE_THINK_INTERVAL as i32;
        let mut expired = Vec::new();
        let mut hdelta: i32 = 0;
        let mut mdelta: i32 = 0;
        let mut effects: Vec<ConditionEffect> = Vec::new();
        for (i, cond) in conditions.iter_mut().enumerate() {
            let (h, m) = cond.tick_regen(interval);
            hdelta += h;
            mdelta += m;
            effects.extend(cond.on_tick(interval));
            if !cond.execute_condition(interval) {
                expired.push(i);
            }
        }
        (expired, hdelta, mdelta, effects)
    };

    if health_delta != 0 || mana_delta != 0 {
        let mut game = g_game().lock().unwrap();
        let is_player = game.get_creature(creature_id).map(|c| c.is_player()).unwrap_or(false);
        if let Some(creature) = game.get_creature_mut(creature_id) {
            if health_delta != 0 {
                let base = creature.base_mut();
                base.health = (base.health + health_delta).clamp(0, base.health_max);
            }
        }
        let no_gain_mana = game.get_player(creature_id)
            .map(|p| p.has_flag(crate::creatures::player::PLAYER_FLAG_NOT_GAIN_MANA))
            .unwrap_or(false);
        if mana_delta != 0 && !no_gain_mana {
            if let Some(player) = game.get_player_mut(creature_id) {
                let new_mana = (player.mana as i64 + mana_delta as i64).clamp(0, player.mana_max as i64) as u32;
                player.mana = new_mana;
            }
        }
        drop(game);
        if is_player {
            crate::net::game_protocol::send_stats_to_player(creature_id);
        }
    }

    if !to_remove.is_empty() {
        let mut game = g_game().lock().unwrap();
        let Some(creature) = game.get_creature_mut(creature_id) else { return };
        let conditions = &mut creature.base_mut().conditions;
        for &i in to_remove.iter().rev() {
            if i < conditions.len() {
                let mut cond = conditions.remove(i);
                cond.end_condition();
                all_effects.extend(cond.on_end());
            }
        }
    }

    if !all_effects.is_empty() {
        apply_condition_effects(creature_id, &all_effects);
    }
}

fn on_attacking(creature_id: u32) {

    // 1. Collect read-only state: target, positions, player fields.
    let (target_id, is_player, attacker_pos, target_pos) = {
        let game = g_game().lock().unwrap();
        let Some(creature) = game.get_creature(creature_id) else { return };
        let Some(tid) = creature.base().attacked_creature_id else { return };
        let attacker_pos = creature.position();
        let target_pos = game.get_creature(tid).map(|c| c.position());
        let Some(target_pos) = target_pos else { return };
        (tid, creature.is_player(), attacker_pos, target_pos)
    };

    // 2. Sight-clear check: same floor and within map range. Simplified: check z match.
    if attacker_pos.z != target_pos.z {
        return;
    }

    if !is_player {
        monster_do_attacking(creature_id, target_id, attacker_pos, target_pos);
        return;
    }

    crate::creatures::player::player_do_attacking(creature_id, target_id, attacker_pos, target_pos);
}


fn monster_think(creature_id: u32) {
    crate::creatures::monster::monster_think(creature_id);
}

fn monster_do_attacking(creature_id: u32, target_id: u32, attacker_pos: crate::map::Position, target_pos: crate::map::Position) {
    crate::creatures::monster::monster_do_attacking(creature_id, target_id, attacker_pos, target_pos);
}

fn player_on_think(creature_id: u32) {
    crate::creatures::player::player_on_think(creature_id);
}

fn npc_think(creature_id: u32) {
    crate::creatures::npc::npc_think_tick(creature_id);
}

fn check_spawns() {
    let spawned = {
        let mut game = g_game().lock().unwrap();
        let mut spawns = std::mem::take(&mut game.spawns);
        let result = spawns.check_spawns(&mut game);
        game.spawns = spawns;
        result
    };
    for (creature_id, pos) in spawned {
        crate::net::game_protocol::broadcast_creature_appear(creature_id, pos);
    }
}

fn update_world_time() {
    use std::time::{SystemTime, UNIX_EPOCH};
    let os_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        % 86400;
    let world_time = (os_secs as f32 / 2.5) as i16;
    g_game().lock().unwrap().set_world_time(world_time);
}

fn check_light() {
    let (prev_level, player_ids) = {
        let game = g_game().lock().unwrap();
        (game.light_level, game.get_all_players())
    };

    {
        let mut game = g_game().lock().unwrap();
        game.update_world_light_level();
    }

    let (new_level, new_color) = {
        let game = g_game().lock().unwrap();
        game.get_world_light_info()
    };

    if prev_level != new_level {
        for player_id in player_ids {
            send_packet_to_player(player_id, |output: &mut OutputMessage| {
                output.add_byte(0x82);
                output.add_byte(new_level);
                output.add_byte(new_color);
            });
        }
    }
}
