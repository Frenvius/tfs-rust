use std::time::Duration;

use crate::combat::condition::{ConditionEffect, SKILL_FIRST, SKILL_LAST, SPECIALSKILL_FIRST, SPECIALSKILL_LAST, STAT_FIRST, STAT_LAST};
use crate::config::{g_config, IntegerConfig};
use crate::creatures::{CreatureId, Skull};
use crate::game::{
    g_game, EVENT_CHECK_CREATURE_INTERVAL, EVENT_CREATURECOUNT, EVENT_CREATURE_THINK_INTERVAL,
    EVENT_LIGHTINTERVAL, EVENT_WORLDTIMEINTERVAL,
};
use crate::map::tile::TILESTATE_NOLOGOUT;
use crate::net::game_protocol::send_packet_to_player;
use crate::net::output_message::OutputMessage;
use crate::util::get_milliseconds_time;
use crate::world::vocation::g_vocations;

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

pub async fn run_game_tick() {
    let creature_loop = async {
        let mut index: usize = 0;
        loop {
            tokio::time::sleep(Duration::from_millis(EVENT_CHECK_CREATURE_INTERVAL)).await;
            check_creatures(index);
            index = (index + 1) % EVENT_CREATURECOUNT;
        }
    };

    let world_time_loop = async {
        loop {
            tokio::time::sleep(Duration::from_millis(EVENT_WORLDTIMEINTERVAL)).await;
            update_world_time();
        }
    };

    let light_loop = async {
        loop {
            tokio::time::sleep(Duration::from_millis(EVENT_LIGHTINTERVAL)).await;
            check_light();
        }
    };

    let spawn_loop = async {
        loop {
            // Check every 10 seconds for dead spawn blocks that need respawning.
            tokio::time::sleep(Duration::from_millis(10_000)).await;
            check_spawns();
        }
    };

    let global_think_loop = async {
        loop {
            tokio::time::sleep(Duration::from_millis(crate::events::global::SCHEDULER_MINTICKS as u64)).await;
            crate::events::registry::g_script_registry().lock().unwrap().global_events.think();
        }
    };

    let global_timer_loop = async {
        loop {
            tokio::time::sleep(Duration::from_millis(1_000)).await;
            crate::events::registry::g_script_registry().lock().unwrap().global_events.timer();
        }
    };

    // House rent check (C++ Game schedules payHouses; no-op when rent disabled).
    let rent_loop = async {
        loop {
            tokio::time::sleep(Duration::from_millis(60_000)).await;
            let db = crate::db::g_database();
            if let Err(e) = crate::map::serialize::IOMapSerialize::pay_houses(db).await {
                tracing::warn!("payHouses failed: {e}");
            }
        }
    };

    let decay_loop = async {
        loop {
            tokio::time::sleep(Duration::from_millis(crate::game::EVENT_DECAYINTERVAL as u64)).await;
            let mut game = g_game().lock().unwrap();
            game.check_decay();
        }
    };

    tokio::join!(creature_loop, world_time_loop, light_loop, spawn_loop, global_think_loop, global_timer_loop, rent_loop, decay_loop);
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
        if mana_delta != 0 {
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

use crate::creatures::player::{SKILL_AXE, SKILL_CLUB, SKILL_DISTANCE, SKILL_SWORD};

/// Mirrors C++ `Weapon::getMaxWeaponDamage`.
fn get_max_weapon_damage(level: u32, attack_skill: i32, attack_value: i32, attack_factor: f32) -> i32 {
    ((level as f32 / 5.0)
        + ((((attack_skill as f32 / 4.0 + 1.0) * (attack_value as f32 / 3.0)) * 1.03)
            / attack_factor))
        .round() as i32
}

/// Mirrors C++ `Creature::onAttacking` + `Player::doAttacking` + `Weapon::useFist`.
fn on_attacking(creature_id: u32) {
    use crate::combat::{Combat, CombatDamage, CombatOrigin, CombatParams, CombatType};
    use crate::combat::condition::ConditionType;
    use crate::creatures::player::SKILL_FIST;
    use crate::util::{normal_random, otsys_time};

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

    // 3. Player::doAttacking
    let now = otsys_time() as u64;

    let (last_attack, attack_speed, has_pacified, weapon_id, weapon_attack, weapon_skill, weapon_skill_idx, level, attack_factor, add_attack_skill, element_type, element_damage) = {
        use crate::items::g_items;
        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(creature_id) else { return };
        let has_pacified = player.base.conditions.iter().any(|c| c.get_type() == ConditionType::Pacified);
        let level = player.level;
        let attack_factor = player.get_attack_factor();
        let attack_speed = player.get_attack_speed();
        let add_attack_skill = player.add_attack_skill_point;
        let last_attack = player.last_attack;
        let weapon_id = player.get_weapon();
        let (weapon_attack, weapon_skill, weapon_skill_idx, element_type, element_damage) = if let Some(id) = weapon_id {
            let it = g_items().get_item_type(id as usize);
            let atk = it.attack.max(0);
            let wt = it.weapon_type;
            let skill_idx: usize = match wt {
                1 => SKILL_SWORD,
                2 => SKILL_CLUB,
                3 => SKILL_AXE,
                5 => SKILL_DISTANCE,
                _ => SKILL_FIST,
            };
            (atk as i32, player.get_skill_level(skill_idx) as i32, skill_idx, it.element_type, it.element_damage)
        } else {
            (7, player.get_skill_level(SKILL_FIST) as i32, SKILL_FIST, 0u8, 0u16)
        };
        (last_attack, attack_speed, has_pacified, weapon_id, weapon_attack, weapon_skill, weapon_skill_idx, level, attack_factor, add_attack_skill, element_type, element_damage)
    };

    if has_pacified {
        return;
    }

    // Initialize lastAttack to allow first swing.
    let last_attack = if last_attack == 0 {
        let la = now.saturating_sub(attack_speed as u64 + 1);
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(creature_id) {
            player.last_attack = la;
        }
        la
    } else {
        last_attack
    };

    if now.saturating_sub(last_attack) < attack_speed as u64 {
        return;
    }

    // 4. Weapon/fist attack — melee range check (Chebyshev distance ≤ 1 for melee).
    let weapon_type = weapon_id
        .map(|id| crate::items::g_items().get_item_type(id as usize).weapon_type)
        .unwrap_or(0);
    let is_distance = weapon_type == 5;
    if !is_distance {
        let dx = (attacker_pos.x as i32 - target_pos.x as i32).unsigned_abs();
        let dy = (attacker_pos.y as i32 - target_pos.y as i32).unsigned_abs();
        if dx > 1 || dy > 1 {
            return;
        }
    }

    let attack_value = if weapon_attack > 0 { weapon_attack } else { 7 };
    let (max_damage, melee_multiplier) = {
        use crate::world::vocation::g_vocations;
        let mm = g_vocations()
            .get_vocation(
                g_game().lock().unwrap()
                    .get_player(creature_id)
                    .map(|p| p.vocation_id)
                    .unwrap_or(0)
            )
            .map(|v| v.melee_damage_multiplier)
            .unwrap_or(1.0);
        ((get_max_weapon_damage(level, weapon_skill, attack_value, attack_factor) as f64 * mm as f64) as i32, mm)
    };

    // Element (secondary) damage from weapon abilities.
    let secondary_type = CombatType::from_element_index(element_type);
    let secondary_value = if element_type != 0 && element_damage > 0 {
        let elem_max = (get_max_weapon_damage(level, weapon_skill, element_damage as i32, attack_factor) as f64
            * melee_multiplier as f64) as i32;
        -normal_random(0, elem_max)
    } else {
        0
    };

    let params = CombatParams {
        combat_type: CombatType::PhysicalDamage,
        blocked_by_armor: true,
        blocked_by_shield: true,
        ..CombatParams::default()
    };

    let mut damage = CombatDamage {
        origin: if is_distance { CombatOrigin::Ranged } else { CombatOrigin::Melee },
        primary_type: CombatType::PhysicalDamage,
        primary_value: -normal_random(0, max_damage),
        secondary_type,
        secondary_value,
        ..CombatDamage::default()
    };

    Combat::do_target_combat(Some(creature_id), target_id, &mut damage, &params);

    // 5. Block-hit callbacks — mirrors C++ Creature::blockHit callbacks.
    {
        use crate::combat::BlockType;
        use crate::items::{g_items, description::WEAPON_SHIELD};
        use crate::creatures::player::{CONST_SLOT_LEFT, CONST_SLOT_RIGHT};

        let block_type = damage.block_type;

        // attacker: onAttackedCreatureBlockHit — updates add_attack_skill_point + blood_hit_count.
        {
            let mut game = g_game().lock().unwrap();
            if let Some(player) = game.get_player_mut(creature_id) {
                player.last_attack = now;
                match block_type {
                    BlockType::None => {
                        player.add_attack_skill_point = true;
                        player.blood_hit_count = 30;
                        player.shield_block_count = 30; // reset own shield counter on clean hit
                    }
                    BlockType::Defense | BlockType::Armor => {
                        if player.blood_hit_count > 0 {
                            player.add_attack_skill_point = true;
                            player.blood_hit_count -= 1;
                        } else {
                            player.add_attack_skill_point = false;
                        }
                    }
                    _ => {
                        player.add_attack_skill_point = false;
                    }
                }
            }
        }

        // target: onBlockHit — shield skill advance when a player with a shield gets hit.
        if block_type != BlockType::None {
            let (shield_block_count, has_shield) = {
                let game = g_game().lock().unwrap();
                if let Some(p) = game.get_player(target_id) {
                    let items = g_items();
                    let has_shield = [CONST_SLOT_LEFT, CONST_SLOT_RIGHT].iter().any(|&slot| {
                        p.inventory[slot].map(|sid| items.get_item_type(sid as usize).weapon_type == WEAPON_SHIELD).unwrap_or(false)
                    });
                    (p.shield_block_count, has_shield)
                } else {
                    (0, false)
                }
            };
            if shield_block_count > 0 {
                let shield_leveled = {
                    let mut game = g_game().lock().unwrap();
                    if let Some(player) = game.get_player_mut(target_id) {
                        player.shield_block_count -= 1;
                        if has_shield {
                            player.add_skill_advance(crate::creatures::player::SKILL_SHIELD, 1)
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                if shield_leveled {
                    send_packet_to_player(target_id, move |output: &mut OutputMessage| {
                        let msg = "You advanced in shielding.";
                        output.add_byte(0xB4);
                        output.add_byte(0x13);
                        output.add_string(msg.as_bytes());
                    });
                    let new_shield_level = {
                        let game = g_game().lock().unwrap();
                        game.get_player(target_id)
                            .map(|p| p.skills[crate::creatures::player::SKILL_SHIELD].level as u32)
                            .unwrap_or(0u32)
                    };
                    crate::runtime::g_dispatcher().add_task(
                        crate::runtime::dispatcher::Task::new(move || {
                            crate::events::dispatch::execute_creature_event_advance(
                                target_id,
                                crate::creatures::player::SKILL_SHIELD as u8,
                                new_shield_level.saturating_sub(1),
                                new_shield_level,
                            );
                        }),
                    );
                    let game = g_game().lock().unwrap();
                    if let Some(player) = game.get_player(target_id) {
                        let skills_data: Vec<(u8, u8)> = (0..7)
                            .map(|i| (player.get_skill_level(i).min(255) as u8, player.get_skill_percent(i)))
                            .collect();
                        drop(game);
                        send_packet_to_player(target_id, move |output: &mut OutputMessage| {
                            output.add_byte(0xA1);
                            for (level, percent) in &skills_data {
                                output.add_byte(*level);
                                output.add_byte(*percent);
                            }
                        });
                    }
                }
            }
        }
    }

    let skill_to_advance = weapon_skill_idx;
    if add_attack_skill {
        let leveled = {
            let mut game = g_game().lock().unwrap();
            game.get_player_mut(creature_id)
                .map(|p| p.add_skill_advance(skill_to_advance, 1))
                .unwrap_or(false)
        };
        if leveled {
            let skill_name = match skill_to_advance {
                crate::creatures::player::SKILL_FIST => "fist fighting",
                crate::creatures::player::SKILL_CLUB => "club fighting",
                crate::creatures::player::SKILL_SWORD => "sword fighting",
                crate::creatures::player::SKILL_AXE => "axe fighting",
                crate::creatures::player::SKILL_DISTANCE => "distance fighting",
                _ => "fist fighting",
            };
            send_packet_to_player(creature_id, move |output: &mut OutputMessage| {
                let msg = format!("You advanced in {skill_name}.");
                output.add_byte(0xB4);
                output.add_byte(0x13);
                output.add_string(msg.as_bytes());
            });
            let new_level = {
                let game = g_game().lock().unwrap();
                game.get_player(creature_id)
                    .map(|p| p.skills[skill_to_advance].level as u32)
                    .unwrap_or(0u32)
            };
            let skill_u8 = skill_to_advance as u8;
            crate::runtime::g_dispatcher().add_task(
                crate::runtime::dispatcher::Task::new(move || {
                    crate::events::dispatch::execute_creature_event_advance(
                        creature_id,
                        skill_u8,
                        new_level.saturating_sub(1),
                        new_level,
                    );
                }),
            );
        }
        // Always resend skills to update percent bar.
        {
            let game = g_game().lock().unwrap();
            if let Some(player) = game.get_player(creature_id) {
                let skills_data: Vec<(u8, u8)> = (0..7)
                    .map(|i| (player.get_skill_level(i).min(255) as u8, player.get_skill_percent(i)))
                    .collect();
                drop(game);
                send_packet_to_player(creature_id, move |output: &mut OutputMessage| {
                    output.add_byte(0xA1);
                    for (level, percent) in &skills_data {
                        output.add_byte(*level);
                        output.add_byte(*percent);
                    }
                });
            }
        }
    }
}

/// Returns a single-step Direction from `from` toward `to`, preferring diagonals
/// when both axes are non-zero (mirrors C++ Creature::getStepDirection).
fn step_direction_toward(from: crate::map::Position, to: crate::map::Position) -> Option<crate::creatures::Direction> {
    use crate::creatures::Direction;
    let dx = to.x as i32 - from.x as i32;
    let dy = to.y as i32 - from.y as i32;
    if dx == 0 && dy == 0 {
        return None;
    }
    Some(match (dx.signum(), dy.signum()) {
        (0,  -1) => Direction::North,
        (0,   1) => Direction::South,
        (1,   0) => Direction::East,
        (-1,  0) => Direction::West,
        (1,  -1) => Direction::NorthEast,
        (1,   1) => Direction::SouthEast,
        (-1, -1) => Direction::NorthWest,
        (-1,  1) => Direction::SouthWest,
        _        => return None,
    })
}

/// Mirrors C++ Creature::getWalkSpeed / getStepDuration.
/// Returns milliseconds per step for the given base_speed.
fn step_duration_ms(base_speed: u32) -> i64 {
    if base_speed == 0 { return i64::MAX; }
    // groundSpeed=150 (default), formula: floor(1000*groundSpeed / (speed*220/220))
    // simplified: 150_000 / base_speed
    (150_000_u64 / base_speed as u64) as i64
}

fn monster_walk(creature_id: u32) {
    use crate::net::game_protocol::broadcast_creature_move;
    use crate::util::otsys_time;

    // Collect: pos, target pos, base_speed, last_walk_time, fleeing, target_distance.
    let (pos, target_pos, base_speed, last_walk_time, fleeing, target_distance) = {
        let game = g_game().lock().unwrap();
        let Some(creature) = game.get_creature(creature_id) else { return };
        let Some(monster) = creature.as_monster() else { return };
        let Some(target_id) = creature.base().attacked_creature_id else { return };
        let Some(tc) = game.get_creature(target_id) else { return };
        let pos = creature.position();
        let target_pos = tc.position();
        let base_speed = monster.base.base_speed;
        let last_walk_time = monster.last_walk_time;
        let fleeing = monster.is_fleeing();
        let target_distance = monster.mtype_info.target_distance.max(1);
        (pos, target_pos, base_speed, last_walk_time, fleeing, target_distance)
    };

    // Rate-limit based on walk speed.
    let now = otsys_time();
    if now - last_walk_time < step_duration_ms(base_speed) {
        return;
    }

    let dx = (pos.x as i32 - target_pos.x as i32).unsigned_abs();
    let dy = (pos.y as i32 - target_pos.y as i32).unsigned_abs();
    let cur_dist = dx.max(dy) as i32;

    // Decide movement intent (C++ Monster::getDanceStep): flee or be too close
    // for a ranged keep-distance monster => step AWAY; too far => approach;
    // at the desired distance => hold position.
    let want_away = fleeing || (target_distance > 1 && cur_dist < target_distance);

    let new_pos = if want_away {
        // Direction pointing from the target to the monster increases distance.
        let Some(away) = step_direction_toward(target_pos, pos) else { return };
        let primary = pos.offset_direction(away);
        let game = g_game().lock().unwrap();
        [away,
         crate::creatures::Direction::North, crate::creatures::Direction::East,
         crate::creatures::Direction::South, crate::creatures::Direction::West]
            .into_iter()
            .map(|d| pos.offset_direction(d))
            .find(|&np| {
                let ndx = (np.x as i32 - target_pos.x as i32).unsigned_abs();
                let ndy = (np.y as i32 - target_pos.y as i32).unsigned_abs();
                (ndx.max(ndy) as i32) >= cur_dist
                    && game.map.get_tile(np).map(|t| t.is_walkable()).unwrap_or(false)
            })
            .unwrap_or(primary)
    } else {
        // Already at/within desired distance — hold.
        let melee_stop = target_distance <= 1 && dx <= 1 && dy <= 1;
        if melee_stop || cur_dist <= target_distance {
            return;
        }
        let Some(dir) = step_direction_toward(pos, target_pos) else { return };
        pos.offset_direction(dir)
    };

    let dir = step_direction_toward(pos, new_pos).unwrap_or(crate::creatures::Direction::South);

    // Check destination tile walkability.
    let (old_stackpos, walkable) = {
        let game = g_game().lock().unwrap();
        let walkable = game.map.get_tile(new_pos)
            .map(|t| t.is_walkable())
            .unwrap_or(false);
        let old_stackpos = game.map.get_tile(pos)
            .map(|t| t.get_creature_client_stackpos())
            .unwrap_or(0);
        (old_stackpos, walkable)
    };

    if !walkable || new_pos == pos {
        return;
    }

    // Commit: update direction, last_walk_time, and map position.
    {
        let mut game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature_mut(creature_id) {
            creature.base_mut().direction = dir;
        }
        if let Some(m) = game.get_creature_mut(creature_id).and_then(|c| c.as_monster_mut()) {
            m.last_walk_time = now;
        }
        game.move_creature_position(creature_id, pos, new_pos);
    }

    crate::events::dispatch::execute_step_event(creature_id, pos, new_pos, 1);
    crate::events::dispatch::execute_step_event(creature_id, new_pos, pos, 0);

    broadcast_creature_move(creature_id, pos, new_pos, old_stackpos);
}

fn monster_think(creature_id: u32) {
    use crate::map::MAX_VIEWPORT_X;
    use crate::map::MAX_VIEWPORT_Y;

    // Read monster state: is_hostile, current target, position.
    let (is_hostile, has_target, pos) = {
        let game = g_game().lock().unwrap();
        let Some(creature) = game.get_creature(creature_id) else { return };
        let Some(monster) = creature.as_monster() else { return };
        let is_hostile = monster.is_hostile();
        let has_target = creature.base().attacked_creature_id.is_some();
        let pos = creature.position();
        (is_hostile, has_target, pos)
    };

    if !is_hostile {
        return;
    }

    // Update target list: find all players in the viewport and mark them as potential targets.
    let nearby_players: Vec<u32> = {
        let mut game = g_game().lock().unwrap();
        game.map.get_spectators(
            pos,
            true, // multifloor
            true, // only_players
            MAX_VIEWPORT_X,
            MAX_VIEWPORT_X,
            MAX_VIEWPORT_Y,
            MAX_VIEWPORT_Y,
        )
    };

    // Filter to players that aren't the monster itself.
    let opponent_ids: Vec<u32> = nearby_players.into_iter()
        .filter(|&id| id != creature_id)
        .collect();

    // Add new opponents to the target_list.
    {
        let mut game = g_game().lock().unwrap();
        if let Some(monster) = game.get_creature_mut(creature_id).and_then(|c| c.as_monster_mut()) {
            for &opp_id in &opponent_ids {
                if !monster.target_list.contains(&opp_id) {
                    monster.target_list.push_back(opp_id);
                }
            }
        }
    }

    // If no attack target yet and we have opponents, set the first one.
    if !has_target && !opponent_ids.is_empty() {
        let first_target = opponent_ids[0];
        let mut game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature_mut(creature_id) {
            creature.base_mut().attacked_creature_id = Some(first_target);
        }
    }

    // If current target is dead or out of view, clear it.
    {
        let game = g_game().lock().unwrap();
        let current_target = game.get_creature(creature_id)
            .and_then(|c| c.base().attacked_creature_id);
        if let Some(tid) = current_target {
            let target_alive = game.get_creature(tid)
                .map(|t| t.base().health > 0)
                .unwrap_or(false);
            drop(game);

            if !target_alive {
                let mut game = g_game().lock().unwrap();
                if let Some(monster) = game.get_creature_mut(creature_id).and_then(|c| c.as_monster_mut()) {
                    monster.target_list.retain(|&id| id != tid);
                }
                if let Some(creature) = game.get_creature_mut(creature_id) {
                    creature.base_mut().attacked_creature_id = None;
                }
            }
        }
    }

    // Walk toward attack target if out of melee range.
    monster_walk(creature_id);

    // Defense spells and voices fire unconditionally (not gated on has_target in C++).
    monster_do_defense(creature_id);
    monster_do_yell(creature_id);
}

fn monster_do_defense(creature_id: u32) {
    use crate::combat::{Combat, CombatDamage, CombatOrigin, CombatParams, CombatType};
    use crate::util::normal_random;
    use crate::net::game_protocol::{broadcast_magic_effect, broadcast_creature_health};

    let (defense_ticks, spells, pos, is_summon, max_summons, summon_blocks, has_target) = {
        let game = g_game().lock().unwrap();
        let Some(monster) = game.get_creature(creature_id).and_then(|c| c.as_monster()) else {
            return;
        };
        (
            monster.defense_ticks,
            monster.mtype_info.defense_spells.clone(),
            monster.base.position,
            monster.base.is_summon(),
            monster.mtype_info.max_summons,
            monster.mtype_info.summons.clone(),
            monster.base.attacked_creature_id.is_some(),
        )
    };

    let interval = EVENT_CREATURE_THINK_INTERVAL as u32;
    let new_ticks = defense_ticks + interval;
    let mut reset_ticks = true;

    for spell in &spells {
        if spell.speed > new_ticks {
            reset_ticks = false;
            continue;
        }

        if new_ticks % spell.speed < interval {
            // already used this round
        } else {
            continue;
        }

        let chance = spell.chance.min(100);
        if chance < 100 && (normal_random(1, 100) as u32) > chance {
            continue;
        }

        let min_val = spell.min_combat_value;
        let max_val = spell.max_combat_value;
        let damage_val = if min_val == max_val {
            min_val
        } else {
            normal_random(min_val.min(max_val), min_val.max(max_val))
        };

        let area_effect = spell.area_effect;
        let combat_type = spell.combat_type;

        if matches!(combat_type, CombatType::Healing) {
            let heal = damage_val.max(0);
            if heal > 0 {
                let (new_hp, hp_max) = {
                    let mut game = g_game().lock().unwrap();
                    if let Some(c) = game.get_creature_mut(creature_id) {
                        let b = c.base_mut();
                        b.health = (b.health + heal).min(b.health_max);
                        (b.health, b.health_max)
                    } else { (0, 0) }
                };
                broadcast_creature_health(creature_id, pos, new_hp, hp_max, false);
            }
            if area_effect > 0 {
                broadcast_magic_effect(pos, area_effect);
            }
        } else {
            let params = CombatParams {
                combat_type,
                blocked_by_armor: false,
                blocked_by_shield: false,
                ..CombatParams::default()
            };
            let mut damage = CombatDamage {
                origin: CombatOrigin::Spell,
                primary_type: combat_type,
                primary_value: damage_val,
                ..CombatDamage::default()
            };
            Combat::do_target_combat(Some(creature_id), creature_id, &mut damage, &params);
            if area_effect > 0 {
                broadcast_magic_effect(pos, area_effect);
            }
        }
    }

    // Summons (port of Monster::onThinkDefense summon loop). Only non-summon
    // monsters that are actively pursuing a target may summon.
    use crate::creatures::monster::Monster;
    use crate::game::{Creature, CONST_ME_MAGIC_BLUE, CONST_ME_TELEPORT};
    if !is_summon && has_target && !summon_blocks.is_empty() {
        for block in &summon_blocks {
            if block.speed > new_ticks {
                reset_ticks = false;
                continue;
            }
            if new_ticks % block.speed >= interval {
                continue; // already used this round
            }

            // Count current summons + per-name count under the lock.
            let (total_summons, same_name) = {
                let game = g_game().lock().unwrap();
                let Some(monster) = game.get_creature(creature_id).and_then(|c| c.as_monster()) else {
                    break;
                };
                let ids = monster.base.summon_ids.clone();
                let total = ids.len() as u32;
                let same = ids
                    .iter()
                    .filter(|&&sid| {
                        game.get_creature(sid)
                            .and_then(|c| c.as_monster())
                            .map(|m| m.get_name().eq_ignore_ascii_case(&block.name))
                            .unwrap_or(false)
                    })
                    .count() as u32;
                (total, same)
            };

            if total_summons >= max_summons {
                continue;
            }
            if same_name >= block.max {
                continue;
            }
            if block.chance < normal_random(1, 100) as u32 {
                continue;
            }

            let Some(mut summon) = Monster::create_monster(&block.name) else { continue };
            summon.base.position = pos;
            summon.spawn_pos = pos;
            summon.base.master_id = Some(creature_id);
            summon.base.loot_drop = false;
            summon.base.skill_loss = false;
            let summon_id = summon.base.id;

            let placed = {
                let mut game = g_game().lock().unwrap();
                let ok = game.place_creature(Creature::Monster(summon));
                if ok {
                    if let Some(m) = game.get_creature_mut(creature_id).and_then(|c| c.as_monster_mut()) {
                        m.base.summon_ids.push(summon_id);
                    }
                }
                ok
            };

            if placed {
                crate::net::game_protocol::broadcast_creature_appear(summon_id, pos);
                broadcast_magic_effect(pos, CONST_ME_MAGIC_BLUE);
                broadcast_magic_effect(pos, CONST_ME_TELEPORT);
            }
        }
    }

    {
        let mut game = g_game().lock().unwrap();
        if let Some(monster) = game.get_creature_mut(creature_id).and_then(|c| c.as_monster_mut()) {
            monster.defense_ticks = if reset_ticks { 0 } else { new_ticks };
        }
    }
}

fn monster_do_yell(creature_id: u32) {
    use crate::util::normal_random;
    use crate::net::game_protocol::broadcast_creature_say;

    let (yell_speed_ticks, yell_chance, yell_ticks, voices, pos, name) = {
        let game = g_game().lock().unwrap();
        let Some(monster) = game.get_creature(creature_id).and_then(|c| c.as_monster()) else {
            return;
        };
        if monster.mtype_info.yell_speed_ticks == 0 {
            return;
        }
        (
            monster.mtype_info.yell_speed_ticks,
            monster.mtype_info.yell_chance,
            monster.yell_ticks,
            monster.mtype_info.voice_vector.clone(),
            monster.base.position,
            monster.get_name().to_owned(),
        )
    };

    let interval = EVENT_CREATURE_THINK_INTERVAL as u32;
    let new_ticks = yell_ticks + interval;

    if new_ticks < yell_speed_ticks {
        let mut game = g_game().lock().unwrap();
        if let Some(monster) = game.get_creature_mut(creature_id).and_then(|c| c.as_monster_mut()) {
            monster.yell_ticks = new_ticks;
        }
        return;
    }

    {
        let mut game = g_game().lock().unwrap();
        if let Some(monster) = game.get_creature_mut(creature_id).and_then(|c| c.as_monster_mut()) {
            monster.yell_ticks = 0;
        }
    }

    if voices.is_empty() {
        return;
    }
    if yell_chance < (normal_random(1, 100) as u32) {
        return;
    }

    let idx = if voices.len() == 1 { 0 } else { normal_random(0, (voices.len() as i32) - 1) as usize };
    let voice = &voices[idx];

    // TALKTYPE_MONSTER_YELL=20, TALKTYPE_MONSTER_SAY=19
    let speak_type: u8 = if voice.yell_text { 20 } else { 19 };

    broadcast_creature_say(creature_id, pos, &name, 0, speak_type, voice.text.as_bytes());
}

fn monster_do_attacking(creature_id: u32, target_id: u32, attacker_pos: crate::map::Position, target_pos: crate::map::Position) {
    use crate::combat::{Combat, CombatDamage, CombatOrigin, CombatParams, CombatType};
    use crate::util::normal_random;
    use crate::net::game_protocol::{broadcast_magic_effect, broadcast_distance_effect};

    // Collect attack spells and tick state.
    let (attack_ticks, spells) = {
        let game = g_game().lock().unwrap();
        let Some(monster) = game.get_creature(creature_id).and_then(|c| c.as_monster()) else {
            return;
        };
        (monster.attack_ticks, monster.mtype_info.attack_spells.clone())
    };

    let interval = EVENT_CREATURE_THINK_INTERVAL as u32;
    let new_ticks = attack_ticks + interval;
    let mut reset_ticks = false;

    for spell in &spells {
        let range_ok = if spell.is_melee {
            let dx = (attacker_pos.x as i32 - target_pos.x as i32).unsigned_abs();
            let dy = (attacker_pos.y as i32 - target_pos.y as i32).unsigned_abs();
            dx <= 1 && dy <= 1
        } else if spell.range > 0 {
            let dx = (attacker_pos.x as i32 - target_pos.x as i32).unsigned_abs();
            let dy = (attacker_pos.y as i32 - target_pos.y as i32).unsigned_abs();
            dx <= spell.range && dy <= spell.range
        } else {
            true
        };

        if !range_ok { continue; }

        if new_ticks < spell.speed {
            reset_ticks = false;
            continue;
        }

        reset_ticks = true;

        let chance = spell.chance.min(100);
        if chance < 100 && (normal_random(1, 100) as u32) > chance {
            continue;
        }

        let min_val = spell.min_combat_value;
        let max_val = spell.max_combat_value;
        let damage_val = if min_val == max_val {
            min_val
        } else {
            normal_random(min_val.min(max_val), min_val.max(max_val))
        };

        if spell.is_melee {
            let params = CombatParams {
                combat_type: CombatType::PhysicalDamage,
                blocked_by_armor: true,
                blocked_by_shield: true,
                ..CombatParams::default()
            };
            let mut damage = CombatDamage {
                origin: CombatOrigin::Melee,
                primary_type: CombatType::PhysicalDamage,
                primary_value: damage_val,
                ..CombatDamage::default()
            };
            Combat::do_target_combat(Some(creature_id), target_id, &mut damage, &params);
        } else {
            // Non-melee spell attack.
            let combat_type = spell.combat_type;
            let is_healing = matches!(combat_type, CombatType::Healing);
            let area_effect = spell.area_effect;
            let shoot_effect = spell.shoot_effect;

            if shoot_effect > 0 {
                broadcast_distance_effect(attacker_pos, target_pos, shoot_effect);
            }

            if is_healing {
                // Monster heals itself.
                let heal = damage_val.max(0);
                if heal > 0 {
                    let (new_hp, hp_max) = {
                        let mut game = g_game().lock().unwrap();
                        if let Some(c) = game.get_creature_mut(creature_id) {
                            let b = c.base_mut();
                            b.health = (b.health + heal).min(b.health_max);
                            (b.health, b.health_max)
                        } else { (0, 0) }
                    };
                    crate::net::game_protocol::broadcast_creature_health(creature_id, attacker_pos, new_hp, hp_max, false);
                }
                if area_effect > 0 {
                    broadcast_magic_effect(attacker_pos, area_effect);
                }
            } else if matches!(combat_type, CombatType::ManaDrain) {
                let drain = (-damage_val).max(0);
                if drain > 0 {
                    let mut game = g_game().lock().unwrap();
                    if let Some(p) = game.get_player_mut(target_id) {
                        p.mana = p.mana.saturating_sub(drain as u32);
                    }
                    drop(game);
                    crate::net::game_protocol::send_stats_to_player(target_id);
                }
                if area_effect > 0 {
                    broadcast_magic_effect(target_pos, area_effect);
                }
            } else {
                // Damage spell.
                let params = CombatParams {
                    combat_type,
                    blocked_by_armor: false,
                    blocked_by_shield: false,
                    ..CombatParams::default()
                };
                let mut damage = CombatDamage {
                    origin: CombatOrigin::Spell,
                    primary_type: combat_type,
                    primary_value: damage_val,
                    ..CombatDamage::default()
                };
                Combat::do_target_combat(Some(creature_id), target_id, &mut damage, &params);
                if area_effect > 0 {
                    broadcast_magic_effect(target_pos, area_effect);
                }
            }
        }
    }

    // Update attack_ticks.
    {
        let mut game = g_game().lock().unwrap();
        if let Some(monster) = game.get_creature_mut(creature_id).and_then(|c| c.as_monster_mut()) {
            monster.attack_ticks = if reset_ticks { 0 } else { new_ticks };
        }
    }
}

fn player_on_think(creature_id: u32) {
    let (last_ping, last_pong, pz_locked, vocation_id, skull, skull_ticks, idle_time, is_access) = {
        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(creature_id) else { return };
        (
            player.last_ping,
            player.last_pong,
            player.pz_locked,
            player.vocation_id,
            player.base.skull,
            player.skull_ticks,
            player.idle_time,
            player.is_access_player(),
        )
    };

    let now = get_milliseconds_time();

    // Send ping to client every 5 seconds (C++ Player::sendPing).
    if (now - last_ping) >= 5000 {
        {
            let mut game = g_game().lock().unwrap();
            if let Some(player) = game.get_player_mut(creature_id) {
                player.last_ping = now;
            }
        }
        send_packet_to_player(creature_id, |output: &mut OutputMessage| {
            output.add_byte(0x1E);
        });
    }

    let no_pong_time = now - last_pong;

    // Cancel attack on player target after 7s of no pong (C++ Player::sendPing).
    if no_pong_time >= 7000 {
        let attacking_player = {
            let game = g_game().lock().unwrap();
            let Some(player) = game.get_player(creature_id) else { return };
            if let Some(target_id) = player.base.attacked_creature_id {
                game.get_creature(target_id)
                    .map(|c| c.is_player())
                    .unwrap_or(false)
            } else {
                false
            }
        };
        if attacking_player {
            let mut game = g_game().lock().unwrap();
            if let Some(player) = game.get_player_mut(creature_id) {
                player.base.attacked_creature_id = None;
            }
            send_packet_to_player(creature_id, |output: &mut OutputMessage| {
                output.add_byte(0xA3);
                output.add_u32(0);
            });
        }
    }

    // Kick if no pong for vocation-specific time (at least 60s when pzLocked).
    let no_pong_kick_time = {
        let base = g_vocations()
            .get_vocation(vocation_id)
            .map(|v| v.no_pong_kick_time as i64)
            .unwrap_or(60_000);
        if pz_locked && base < 60_000 { 60_000 } else { base }
    };

    if no_pong_time >= no_pong_kick_time {
        let can_kick = {
            let game = g_game().lock().unwrap();
            if let Some(player) = game.get_player(creature_id) {
                let pos = player.base.position;
                if let Some(tile) = game.map.get_tile(pos) {
                    !tile.has_flag(TILESTATE_NOLOGOUT)
                } else {
                    true
                }
            } else {
                false
            }
        };
        if can_kick {
            {
                let mut game = g_game().lock().unwrap();
                game.remove_creature_check(creature_id);
                game.remove_player(creature_id);
            }
            crate::net::game_protocol::unregister_player_connection(creature_id);
        }
        return;
    }

    // Idle time — only tracked for non-access players not on NOLOGOUT tiles.
    if !is_access {
        let kick_after_minutes = g_config().get_number(IntegerConfig::KickAfterMinutes) as i64;
        let idle_threshold = kick_after_minutes * 60_000;
        let warn_threshold = idle_threshold;
        let new_idle = idle_time as i64 + EVENT_CREATURE_THINK_INTERVAL as i64;

        let on_nologout = {
            let game = g_game().lock().unwrap();
            game.get_player(creature_id)
                .and_then(|p| game.map.get_tile(p.base.position))
                .map(|t| t.has_flag(TILESTATE_NOLOGOUT))
                .unwrap_or(false)
        };

        if !on_nologout {
            {
                let mut game = g_game().lock().unwrap();
                if let Some(player) = game.get_player_mut(creature_id) {
                    player.idle_time = new_idle.min(i32::MAX as i64) as i32;
                }
            }

            if new_idle > idle_threshold + 60_000 {
                // Kick for inactivity.
                let mut game = g_game().lock().unwrap();
                game.remove_creature_check(creature_id);
                game.remove_player(creature_id);
                drop(game);
                crate::net::game_protocol::unregister_player_connection(creature_id);
                return;
            } else if new_idle == warn_threshold {
                send_packet_to_player(creature_id, |output: &mut OutputMessage| {
                    let msg = format!(
                        "There was no variation in your behaviour for {} minutes. You will be disconnected in one minute if there is no change in your actions until then.",
                        kick_after_minutes
                    );
                    output.add_byte(0xB4);
                    output.add_byte(0x13); // MESSAGE_STATUS_WARNING
                    output.add_string(msg.as_bytes());
                });
            }
        }
    }

    // Skull ticks — decrement and clear red/black skull when expired.
    if skull == Skull::Red || skull == Skull::Black {
        let new_skull_ticks = skull_ticks - EVENT_CREATURE_THINK_INTERVAL as i64 / 1000;
        {
            let mut game = g_game().lock().unwrap();
            if let Some(player) = game.get_player_mut(creature_id) {
                player.skull_ticks = new_skull_ticks.max(0);
            }
        }
        if new_skull_ticks <= 0 {
            let has_infight = {
                let game = g_game().lock().unwrap();
                game.get_player(creature_id)
                    .map(|p| p.base.conditions.iter().any(|c| {
                        c.get_type() == crate::combat::condition::ConditionType::InFight
                    }))
                    .unwrap_or(false)
            };
            if !has_infight {
                let mut game = g_game().lock().unwrap();
                if let Some(player) = game.get_player_mut(creature_id) {
                    player.base.skull = Skull::None;
                }
            }
        }
    }
}

fn npc_think(creature_id: u32) {
    use crate::creatures::Direction;
    use crate::net::game_protocol::broadcast_creature_move;

    let (pos, walk_interval, walk_timer) = {
        let game = g_game().lock().unwrap();
        let Some(creature) = game.get_creature(creature_id) else { return };
        let Some(npc) = creature.as_npc() else { return };
        (creature.position(), npc.walk_interval, npc.walk_timer)
    };

    if walk_interval == 0 {
        return;
    }

    let interval = EVENT_CREATURE_THINK_INTERVAL as u32;
    let new_timer = walk_timer + interval;
    if new_timer < walk_interval {
        let mut game = g_game().lock().unwrap();
        if let Some(npc) = game.get_creature_mut(creature_id).and_then(|c| c.as_npc_mut()) {
            npc.walk_timer = new_timer;
        }
        return;
    }

    // Reset timer.
    {
        let mut game = g_game().lock().unwrap();
        if let Some(npc) = game.get_creature_mut(creature_id).and_then(|c| c.as_npc_mut()) {
            npc.walk_timer = 0;
        }
    }

    // Pick a random direction and attempt to walk one step.
    let dir_val = crate::util::uniform_random(0, 3) as u8;
    let Some(dir) = Direction::from_u8(dir_val) else { return };
    let new_pos = pos.offset_direction(dir);

    let (old_stackpos, walkable) = {
        let game = g_game().lock().unwrap();
        let walkable = game.map.get_tile(new_pos).map(|t| t.is_walkable()).unwrap_or(false);
        let old_stackpos = game.map.get_tile(pos)
            .map(|t| t.get_creature_client_stackpos()).unwrap_or(0);
        (old_stackpos, walkable)
    };

    if !walkable {
        return;
    }

    {
        let mut game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature_mut(creature_id) {
            creature.base_mut().direction = dir;
        }
        game.move_creature_position(creature_id, pos, new_pos);
    }

    crate::events::dispatch::execute_step_event(creature_id, pos, new_pos, 1);
    crate::events::dispatch::execute_step_event(creature_id, new_pos, pos, 0);

    broadcast_creature_move(creature_id, pos, new_pos, old_stackpos);

    // Fire Lua onThink event for this NPC.
    crate::creatures::npc::fire_npc_think(creature_id);
}

fn check_spawns() {
    use crate::creatures::{Creature, monster::Monster};
    use crate::map::Position;
    use crate::util::otsys_time;
    use crate::world::spawn::MINSPAWN_INTERVAL;

    // Collect spawn zones and their block state without holding the lock.
    struct SpawnWork {
        spawn_idx: usize,
        spawn_id: u32,
        names: Vec<(String, u16)>,
        pos: Position,
        direction: crate::creatures::Direction,
        interval: u32,
    }

    let now = otsys_time();

    let work_items: Vec<SpawnWork> = {
        let game = g_game().lock().unwrap();
        let mut items = Vec::new();
        for (si, spawn) in game.spawns.spawn_list.iter().enumerate() {
            for (&spawn_id, sb) in &spawn.spawn_map {
                if spawn.spawned_map.contains_key(&spawn_id) {
                    // Monster still alive.
                    continue;
                }
                let interval_ms = sb.interval.max(MINSPAWN_INTERVAL as u32);
                if (now - sb.last_spawn) < interval_ms as i64 {
                    continue;
                }
                items.push(SpawnWork {
                    spawn_idx: si,
                    spawn_id,
                    names: sb.monster_names.clone(),
                    pos: sb.pos,
                    direction: sb.direction,
                    interval: interval_ms,
                });
            }
        }
        items
    };

    for w in work_items {
        // Check if any player is near the spawn position.
        let has_nearby_player: bool = {
            let mut game = g_game().lock().unwrap();
            !game.map.get_spectators(w.pos, false, true, 0, 0, 0, 0).is_empty()
        };

        if has_nearby_player {
            // Delay: player is present, try again after interval.
            let mut game = g_game().lock().unwrap();
            if let Some(spawn) = game.spawns.spawn_list.get_mut(w.spawn_idx) {
                if let Some(sb) = spawn.spawn_map.get_mut(&w.spawn_id) {
                    sb.last_spawn = now - w.interval as i64 + MINSPAWN_INTERVAL as i64;
                }
            }
            continue;
        }

        // Try to place a new monster.
        for (name, _weight) in &w.names {
            let Some(mut monster) = Monster::create_monster(name) else { continue };
            monster.base.position = w.pos;
            monster.spawn_pos = w.pos;
            monster.base.direction = w.direction;
            let monster_id = monster.base.id;

            let placed = g_game().lock().unwrap()
                .place_creature(Creature::Monster(monster));

            if placed {
                {
                    let mut game = g_game().lock().unwrap();
                    if let Some(spawn) = game.spawns.spawn_list.get_mut(w.spawn_idx) {
                        spawn.spawned_map.insert(w.spawn_id, monster_id);
                        if let Some(sb) = spawn.spawn_map.get_mut(&w.spawn_id) {
                            sb.last_spawn = now;
                        }
                    }
                }
                // Broadcast appearance after releasing the lock.
                crate::net::game_protocol::broadcast_creature_appear(monster_id, w.pos);
                break;
            }
        }
    }

    // After respawning, clear dead entries from spawned_map.
    {
        let mut game = g_game().lock().unwrap();
        let live_ids: std::collections::HashSet<u32> = game.creatures.keys().copied().collect();
        for spawn in &mut game.spawns.spawn_list {
            spawn.spawned_map.retain(|_, cid| live_ids.contains(cid));
        }
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
