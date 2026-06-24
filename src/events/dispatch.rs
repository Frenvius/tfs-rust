use crate::combat::{CombatDamage, CombatType};
use crate::creatures::CreatureId;
use crate::events::registry::g_script_registry;
use crate::game::g_game;
use crate::lua::script::{g_lua, ScriptEnvironment};
use crate::map::Position;

use mlua::prelude::*;

fn push_position(lua: &Lua, pos: Position) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;
    t.set("x", pos.x as i64)?;
    t.set("y", pos.y as i64)?;
    t.set("z", pos.z as i64)?;
    if let Ok(mt) = lua.named_registry_value::<LuaTable>("Position") {
        let _ = t.set_metatable(Some(mt));
    }
    Ok(t)
}

fn push_player(lua: &Lua, creature_id: CreatureId) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;
    t.raw_set(1, creature_id)?;
    if let Ok(mt) = lua.named_registry_value::<LuaTable>("Player") {
        let _ = t.set_metatable(Some(mt));
    }
    Ok(t)
}

fn push_item(lua: &Lua, server_id: u16, pos: Position, index: i32) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;
    t.raw_set(1, server_id as i64)?;
    t.raw_set("itemid", server_id as i64)?;
    t.raw_set("_pos_x", pos.x)?;
    t.raw_set("_pos_y", pos.y)?;
    t.raw_set("_pos_z", pos.z)?;
    t.raw_set("_idx", index)?;

    let game = g_game().lock().unwrap();
    if let Some(tile) = game.map.get_tile(pos) {
        let item = if index == -1 {
            tile.ground.as_ref()
        } else {
            tile.items.get(index as usize)
        };
        if let Some(item) = item {
            t.raw_set("uid", item.unique_id as i64)?;
            if item.action_id > 0 {
                t.raw_set("actionid", item.action_id as i64)?;
            }
        }
    }
    drop(game);

    if let Ok(mt) = lua.named_registry_value::<LuaTable>("Item") {
        let _ = t.set_metatable(Some(mt));
    }
    Ok(t)
}

pub async fn execute_action_use_async(
    player_id: CreatureId,
    item_pos: Position,
    item_server_id: u16,
    item_index: i32,
    item_action_id: u16,
    item_unique_id: u16,
    is_hotkey: bool,
) -> bool {
    let (tx, rx) = tokio::sync::oneshot::channel();
    crate::runtime::g_dispatcher().add_task(
        crate::runtime::dispatcher::Task::new(move || {
            let result = execute_action_use(
                player_id, item_pos, item_server_id, item_index,
                item_action_id, item_unique_id, is_hotkey,
            );
            let _ = tx.send(result);
        }),
    );
    rx.await.unwrap_or(false)
}

pub async fn execute_action_use_ex_async(
    player_id: CreatureId,
    from_pos: Position,
    item_server_id: u16,
    item_index: i32,
    item_action_id: u16,
    item_unique_id: u16,
    to_pos: Position,
    to_stackpos: u8,
    is_hotkey: bool,
) -> bool {
    let (tx, rx) = tokio::sync::oneshot::channel();
    crate::runtime::g_dispatcher().add_task(
        crate::runtime::dispatcher::Task::new(move || {
            let result = execute_action_use_ex(
                player_id, from_pos, item_server_id, item_index,
                item_action_id, item_unique_id, to_pos, to_stackpos, is_hotkey,
            );
            let _ = tx.send(result);
        }),
    );
    rx.await.unwrap_or(false)
}

pub fn execute_action_use(
    player_id: CreatureId,
    item_pos: Position,
    item_server_id: u16,
    item_index: i32,
    item_action_id: u16,
    item_unique_id: u16,
    is_hotkey: bool,
) -> bool {
    crate::runtime::assert_dispatcher_thread!();
    let (script_id, from_lua) = {
        let registry = g_script_registry().lock().unwrap();
        let action = registry.actions.get_action(item_server_id, item_unique_id, item_action_id);
        match action {
            Some(a) if a.scripted => (a.script_id, a.from_lua),
            _ => return false,
        }
    };

    let lua = g_lua();

    if !ScriptEnvironment::reserve() {
        tracing::error!("Action::executeUse - Call stack overflow");
        return false;
    }
    ScriptEnvironment::set_script_id(script_id, "Action Interface");

    let result = (|| -> LuaResult<bool> {
        let func = if from_lua {
            let registry = g_script_registry().lock().unwrap();
            registry.get_callback_function(lua, script_id)
        } else {
            let registry = g_script_registry().lock().unwrap();
            registry.actions.get_script_function(script_id)
        };

        let Some(func) = func else {
            return Ok(false);
        };

        let player_tbl = push_player(lua, player_id)?;
        let item_tbl = push_item(lua, item_server_id, item_pos, item_index)?;
        let from_pos_tbl = push_position(lua, item_pos)?;
        let target_tbl = push_item(lua, item_server_id, item_pos, item_index)?;
        let to_pos_tbl = push_position(lua, item_pos)?;

        let r: mlua::Value = func.call((player_tbl, item_tbl, from_pos_tbl, target_tbl, to_pos_tbl, is_hotkey))?;
        Ok(matches!(r, mlua::Value::Boolean(true)))
    })();

    ScriptEnvironment::reset();

    match result {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(%e, "execute_action_use Lua error");
            false
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn execute_action_use_ex(
    player_id: CreatureId,
    from_pos: Position,
    item_server_id: u16,
    item_index: i32,
    item_action_id: u16,
    item_unique_id: u16,
    to_pos: Position,
    to_stackpos: u8,
    is_hotkey: bool,
) -> bool {
    let (script_id, from_lua) = {
        let registry = g_script_registry().lock().unwrap();
        let action = registry.actions.get_action(item_server_id, item_unique_id, item_action_id);
        match action {
            Some(a) if a.scripted => (a.script_id, a.from_lua),
            _ => return false,
        }
    };

    let lua = g_lua();

    if !ScriptEnvironment::reserve() {
        tracing::error!("Action::executeUse - Call stack overflow");
        return false;
    }
    ScriptEnvironment::set_script_id(script_id, "Action Interface");

    let result = (|| -> LuaResult<bool> {
        let func = if from_lua {
            let registry = g_script_registry().lock().unwrap();
            registry.get_callback_function(lua, script_id)
        } else {
            let registry = g_script_registry().lock().unwrap();
            registry.actions.get_script_function(script_id)
        };

        let Some(func) = func else {
            return Ok(false);
        };

        let player_tbl = push_player(lua, player_id)?;
        let item_tbl = push_item(lua, item_server_id, from_pos, item_index)?;
        let from_pos_tbl = push_position(lua, from_pos)?;

        let game = g_game().lock().unwrap();
        let target: LuaValue = if let Some(tile) = game.map.get_tile(to_pos) {
            if !tile.creature_ids.is_empty() {
                let cid = tile.creature_ids[0];
                let class_name = match game.get_creature(cid) {
                    Some(crate::creatures::Creature::Player(_)) => "Player",
                    Some(crate::creatures::Creature::Monster(_)) => "Monster",
                    Some(crate::creatures::Creature::Npc(_)) => "Npc",
                    None => "Creature",
                };
                drop(game);
                let t = lua.create_table()?;
                t.raw_set(1, cid)?;
                if let Ok(mt) = lua.named_registry_value::<LuaTable>(class_name) {
                    let _ = t.set_metatable(Some(mt));
                }
                LuaValue::Table(t)
            } else if to_stackpos < 255 {
                let idx = to_stackpos as i32;
                let sid = if idx == 0 && tile.ground.is_some() {
                    tile.ground.as_ref().map(|g| g.server_id).unwrap_or(0)
                } else {
                    tile.items.get(idx.saturating_sub(1) as usize).map(|i| i.server_id).unwrap_or(0)
                };
                drop(game);
                if sid > 0 {
                    LuaValue::Table(push_item(lua, sid, to_pos, idx - 1)?)
                } else {
                    LuaValue::Nil
                }
            } else {
                drop(game);
                LuaValue::Nil
            }
        } else {
            drop(game);
            LuaValue::Nil
        };

        let to_pos_tbl = push_position(lua, to_pos)?;

        match func.call::<bool>((player_tbl, item_tbl, from_pos_tbl, target, to_pos_tbl, is_hotkey)) {
            Ok(v) => Ok(v),
            Err(e) => {
                tracing::error!("Lua action error: {e}");
                Ok(false)
            }
        }
    })();

    ScriptEnvironment::reset();

    result.unwrap_or(false)
}

pub fn execute_spell_say(player_id: CreatureId, text: &str) -> bool {
    crate::runtime::assert_dispatcher_thread!();
    let words_lower = text.to_lowercase();
    let (spell_words, param) = {
        let registry = g_script_registry().lock().unwrap();
        // Mirror C++ Spells::getInstantSpell:
        //   - no-param spells: exact match only
        //   - param spells: longest prefix match (so "exura gran" beats "exura")
        let mut found: Option<(crate::events::registry::SpellEntry, String)> = None;
        for (key, entry) in &registry.spells {
            if entry.has_params || entry.has_player_name_param {
                if words_lower.starts_with(key.as_str())
                    && (words_lower.len() == key.len()
                        || words_lower.as_bytes().get(key.len()) == Some(&b' '))
                {
                    let rest = words_lower[key.len()..].trim_start().to_string();
                    if found.as_ref().map_or(true, |(e, _)| key.len() > e.words.len()) {
                        found = Some((entry.clone(), rest));
                    }
                }
            } else if words_lower == key.as_str() {
                found = Some((entry.clone(), String::new()));
                break;
            }
        }
        match found {
            Some(f) => f,
            None => return false,
        }
    };

    let (level, mag_level, mana, mana_max, group_flags) = {
        let game = g_game().lock().unwrap();
        match game.get_player(player_id) {
            Some(p) => (p.level, p.mag_level, p.mana, p.mana_max, p.group_flags),
            None => return false,
        }
    };

    if !spell_words.enabled {
        return true;
    }

    // Player flag gates, mirroring C++ Spell::playerSpellCheck.
    use crate::creatures::player::{
        PLAYER_FLAG_CANNOT_USE_SPELLS, PLAYER_FLAG_IGNORE_SPELL_CHECK, PLAYER_FLAG_HAS_INFINITE_MANA,
    };
    if group_flags & PLAYER_FLAG_CANNOT_USE_SPELLS != 0 {
        return true; // cannot cast; treat as handled (no normal talk)
    }
    let ignore_checks = group_flags & PLAYER_FLAG_IGNORE_SPELL_CHECK != 0;

    if !ignore_checks {
        if level < spell_words.level {
            let msg = "You do not have enough level.".to_string();
            let mut game = g_game().lock().unwrap();
            game.send_text_message(player_id, crate::game::MESSAGE_INFO_DESCR, msg);
            return true;
        }

        if mag_level < spell_words.magic_level {
            let msg = "You do not have enough magic level.".to_string();
            let mut game = g_game().lock().unwrap();
            game.send_text_message(player_id, crate::game::MESSAGE_INFO_DESCR, msg);
            return true;
        }
    }

    let required_mana = if spell_words.mana_percent > 0 {
        (mana_max as u64 * spell_words.mana_percent as u64 / 100) as u32
    } else {
        spell_words.mana
    };

    // `required_mana` stays the real cost (C++ getManaCost ignores the infinite
    // flag); the flag only bypasses the availability check below and the actual
    // deduction in postCastSpell (changeMana still runs → sends stats).
    let infinite_mana = ignore_checks || (group_flags & PLAYER_FLAG_HAS_INFINITE_MANA != 0);

    if !ignore_checks && !infinite_mana && mana < required_mana {
        let msg = "You do not have enough mana.".to_string();
        let mut game = g_game().lock().unwrap();
        game.send_text_message(player_id, crate::game::MESSAGE_INFO_DESCR, msg);
        return true;
    }

    let script_id = spell_words.script_id;
    if script_id == 0 {
        return true;
    }

    let lua = g_lua();

    if !ScriptEnvironment::reserve() {
        tracing::error!("Spell::castSpell - Call stack overflow");
        return true;
    }
    ScriptEnvironment::set_script_id(script_id, "Spell Interface");

    // Bundle the caster's cast output (combat effect + post-cast mana/stats)
    // into one XTEA frame, matching C++ which runs the cast (effect) then
    // postCastSpell (mana deduction + stats) within a single dispatch flush.
    crate::net::game_protocol::begin_player_bundle_empty(player_id);

    let result = (|| -> LuaResult<bool> {
        let registry = g_script_registry().lock().unwrap();
        let func = registry.get_callback_function(lua, script_id);
        drop(registry);

        let Some(func) = func else {
            return Ok(true);
        };

        let player_tbl = push_player(lua, player_id)?;
        // Build variant table matching C++ LuaScriptInterface::pushVariant.
        let variant: LuaValue = {
            let vt = lua.create_table()?;
            if spell_words.need_target {
                // VARIANT_NUMBER — use battle target
                let target_id = {
                    let game = g_game().lock().unwrap();
                    game.get_creature(player_id)
                        .and_then(|c| c.base().attacked_creature_id)
                        .unwrap_or(0)
                };
                if target_id != 0 {
                    vt.set("type", 1i64)?; // VARIANT_NUMBER
                    vt.set("number", target_id as i64)?;
                } else {
                    // No target — cancel silently
                    vt.set("type", 0i64)?;
                    // In C++ this sends "You cannot cast a spell on others yet."
                    let game = g_game().lock().unwrap();
                    drop(game);
                }
            } else if !param.is_empty() {
                vt.set("type", 2i64)?; // VARIANT_STRING
                vt.set("string", param.as_str())?;
            } else {
                vt.set("type", 0i64)?; // VARIANT_NONE
            }
            LuaValue::Table(vt)
        };

        match func.call::<bool>((player_tbl, variant)) {
            Ok(v) => Ok(v),
            Err(e) => {
                tracing::error!("Lua spell error: {e}");
                Ok(true)
            }
        }
    })();

    ScriptEnvironment::reset();

    // postCastSpell: deduct mana, advance magic level, and send stats — after
    // the cast effect, matching C++ order. These flow into the open bundle.
    let new_magic_level = if required_mana > 0 {
        let mut game = g_game().lock().unwrap();
        if let Some(p) = game.get_player_mut(player_id) {
            // C++ postCastSpell: addManaSpent always (magic-level progress);
            // changeMana deducts only for non-infinite-mana players.
            if !infinite_mana {
                p.mana = p.mana.saturating_sub(required_mana);
            }
            p.add_mana_spent(required_mana as u64)
        } else {
            None
        }
    } else {
        None
    };

    if let Some(new_ml) = new_magic_level {
        let msg = format!("You advanced to magic level {new_ml}.");
        let mut game = g_game().lock().unwrap();
        game.send_text_message(player_id, crate::world::raids::MESSAGE_EVENT_ADVANCE, msg);
        drop(game);
        let prev_ml = new_ml.saturating_sub(1);
        crate::runtime::g_dispatcher().add_task(
            crate::runtime::dispatcher::Task::new(move || {
                execute_creature_event_advance(player_id, 7, prev_ml, new_ml);
            }),
        );
    }
    if required_mana > 0 {
        crate::net::game_protocol::send_stats_to_player(player_id);
    }

    // Echo the spell words as a SAY (0xAA), matching C++ playerSaySpell →
    // internalCreatureSay(player, TALKTYPE_SAY, words) for a cast spell
    // (EMOTE_SPELLS defaults off). Flows into the open bundle for the caster.
    {
        let (name, level, pos) = {
            let game = g_game().lock().unwrap();
            match game.get_player(player_id) {
                Some(p) => (p.name.clone(), p.level as u16, p.base.position),
                None => (String::new(), 0, crate::map::Position::default()),
            }
        };
        if !name.is_empty() {
            crate::net::game_protocol::broadcast_creature_say(
                player_id, pos, &name, level, 1, text.as_bytes(),
            );
        }
    }

    crate::net::game_protocol::flush_player_bundle(player_id);

    result.unwrap_or(true)
}

fn push_creature(lua: &Lua, creature_id: CreatureId) -> LuaResult<LuaTable> {
    let class_name = {
        let game = g_game().lock().unwrap();
        match game.get_creature(creature_id) {
            Some(crate::creatures::Creature::Player(_)) => "Player",
            Some(crate::creatures::Creature::Monster(_)) => "Monster",
            Some(crate::creatures::Creature::Npc(_)) => "Npc",
            None => "Creature",
        }
    };
    let t = lua.create_table()?;
    t.raw_set(1, creature_id)?;
    if let Ok(mt) = lua.named_registry_value::<LuaTable>(class_name) {
        let _ = t.set_metatable(Some(mt));
    }
    Ok(t)
}

/// Fire all `onLogin` creature events for a player. Mirrors `CreatureEvents::playerLogin`.
pub fn execute_creature_event_login(player_id: CreatureId) -> bool {
    crate::runtime::assert_dispatcher_thread!();
    use crate::events::creature::CreatureEventType;
    use crate::events::registry::g_script_registry;

    let event_ids: Vec<(i32, bool)> = {
        let registry = g_script_registry().lock().unwrap();
        registry.creature_events.iter()
            .filter(|(_, ev)| ev.event_type == CreatureEventType::Login && ev.scripted)
            .map(|(_, ev)| (ev.script_id, ev.from_lua))
            .collect()
    };

    let lua = g_lua();
    for (script_id, from_lua) in event_ids {
        if !ScriptEnvironment::reserve() {
            tracing::error!("CreatureEvent::executeOnLogin - Call stack overflow");
            return false;
        }
        ScriptEnvironment::set_script_id(script_id, "CreatureScript Interface");
        let result = (|| -> LuaResult<bool> {
            let func = get_creature_event_func(lua, script_id, from_lua);
            let Some(func) = func else { return Ok(true); };
            let player_tbl = push_player(lua, player_id)?;
            match func.call::<bool>(player_tbl) {
                Ok(v) => Ok(v),
                Err(e) => { tracing::error!("Lua onLogin error: {e}"); Ok(true) }
            }
        })();
        ScriptEnvironment::reset();
        if !result.unwrap_or(true) {
            return false;
        }
    }
    true
}

/// Fire all `onLogout` creature events for a player. Mirrors `CreatureEvents::playerLogout`.
pub fn execute_creature_event_logout(player_id: CreatureId) -> bool {
    use crate::events::creature::CreatureEventType;
    use crate::events::registry::g_script_registry;

    let event_ids: Vec<(i32, bool)> = {
        let registry = g_script_registry().lock().unwrap();
        registry.creature_events.iter()
            .filter(|(_, ev)| ev.event_type == CreatureEventType::Logout && ev.scripted)
            .map(|(_, ev)| (ev.script_id, ev.from_lua))
            .collect()
    };

    let lua = g_lua();
    for (script_id, from_lua) in event_ids {
        if !ScriptEnvironment::reserve() {
            tracing::error!("CreatureEvent::executeOnLogout - Call stack overflow");
            return false;
        }
        ScriptEnvironment::set_script_id(script_id, "CreatureScript Interface");
        let result = (|| -> LuaResult<bool> {
            let func = get_creature_event_func(lua, script_id, from_lua);
            let Some(func) = func else { return Ok(true); };
            let player_tbl = push_player(lua, player_id)?;
            match func.call::<bool>(player_tbl) {
                Ok(v) => Ok(v),
                Err(e) => { tracing::error!("Lua onLogout error: {e}"); Ok(true) }
            }
        })();
        ScriptEnvironment::reset();
        if !result.unwrap_or(true) {
            return false;
        }
    }
    true
}

/// Fire per-creature `onDeath` events. Mirrors `CreatureEvent::executeOnDeath`.
/// `corpse_id` is 0 if no corpse was dropped.
pub fn execute_creature_event_death(
    creature_id: CreatureId,
    last_hit_id: CreatureId,
    most_damage_id: CreatureId,
    last_hit_unjustified: bool,
    most_damage_unjustified: bool,
) {
    use crate::events::creature::CreatureEventType;

    let event_names: Vec<String> = {
        let game = g_game().lock().unwrap();
        match game.get_creature(creature_id) {
            Some(c) => c.base().get_creature_event_names(CreatureEventType::Death),
            None => return,
        }
    };

    if event_names.is_empty() {
        return;
    }

    use crate::events::registry::g_script_registry;
    let event_ids: Vec<(i32, bool)> = {
        let registry = g_script_registry().lock().unwrap();
        event_names.iter()
            .filter_map(|n| registry.creature_events.get_event_by_name(n, true).map(|e| (e.script_id, e.from_lua)))
            .collect()
    };

    let lua = g_lua();
    for (script_id, from_lua) in event_ids {
        if !ScriptEnvironment::reserve() {
            tracing::error!("CreatureEvent::executeOnDeath - Call stack overflow");
            return;
        }
        ScriptEnvironment::set_script_id(script_id, "CreatureScript Interface");
        let _ = (|| -> LuaResult<()> {
            let func = get_creature_event_func(lua, script_id, from_lua);
            let Some(func) = func else { return Ok(()); };
            let creature_tbl = push_creature(lua, creature_id)?;
            let killer_val: LuaValue = if last_hit_id != 0 {
                LuaValue::Table(push_creature(lua, last_hit_id)?)
            } else {
                LuaValue::Nil
            };
            let most_dmg_val: LuaValue = if most_damage_id != 0 {
                LuaValue::Table(push_creature(lua, most_damage_id)?)
            } else {
                LuaValue::Nil
            };
            match func.call::<bool>((creature_tbl, LuaValue::Nil, killer_val, most_dmg_val, last_hit_unjustified, most_damage_unjustified)) {
                Ok(_) => Ok(()),
                Err(e) => { tracing::error!("Lua onDeath error: {e}"); Ok(()) }
            }
        })();
        ScriptEnvironment::reset();
    }
}

/// Fire per-creature `onKill` events on the killer. Mirrors `Creature::onKilledCreature`.
pub fn execute_creature_event_kill(killer_id: CreatureId, target_id: CreatureId) {
    use crate::events::creature::CreatureEventType;

    let event_names: Vec<String> = {
        let game = g_game().lock().unwrap();
        match game.get_creature(killer_id) {
            Some(c) => c.base().get_creature_event_names(CreatureEventType::Kill),
            None => return,
        }
    };

    if event_names.is_empty() {
        return;
    }

    use crate::events::registry::g_script_registry;
    let event_ids: Vec<(i32, bool)> = {
        let registry = g_script_registry().lock().unwrap();
        event_names.iter()
            .filter_map(|n| registry.creature_events.get_event_by_name(n, true).map(|e| (e.script_id, e.from_lua)))
            .collect()
    };

    let lua = g_lua();
    for (script_id, from_lua) in event_ids {
        if !ScriptEnvironment::reserve() {
            tracing::error!("CreatureEvent::executeOnKill - Call stack overflow");
            return;
        }
        ScriptEnvironment::set_script_id(script_id, "CreatureScript Interface");
        let _ = (|| -> LuaResult<()> {
            let func = get_creature_event_func(lua, script_id, from_lua);
            let Some(func) = func else { return Ok(()); };
            let killer_tbl = push_creature(lua, killer_id)?;
            let target_tbl = push_creature(lua, target_id)?;
            match func.call::<()>((killer_tbl, target_tbl)) {
                Ok(_) => Ok(()),
                Err(e) => { tracing::error!("Lua onKill error: {e}"); Ok(()) }
            }
        })();
        ScriptEnvironment::reset();
    }
}

/// Fire all `onAdvance(player, skill, oldLevel, newLevel)` events globally.
/// Mirrors `CreatureEvents::playerAdvance`. `skill` is `skills_t` numeric value.
pub fn execute_creature_event_advance(player_id: CreatureId, skill: u8, old_level: u32, new_level: u32) {
    use crate::events::creature::CreatureEventType;
    use crate::events::registry::g_script_registry;

    let event_ids: Vec<(i32, bool)> = {
        let registry = g_script_registry().lock().unwrap();
        registry.creature_events.iter()
            .filter(|(_, ev)| ev.event_type == CreatureEventType::Advance && ev.scripted)
            .map(|(_, ev)| (ev.script_id, ev.from_lua))
            .collect()
    };

    if event_ids.is_empty() {
        return;
    }

    let lua = g_lua();
    for (script_id, from_lua) in event_ids {
        if !ScriptEnvironment::reserve() {
            tracing::error!("CreatureEvent::executeAdvance - Call stack overflow");
            return;
        }
        ScriptEnvironment::set_script_id(script_id, "CreatureScript Interface");
        let result = (|| -> LuaResult<bool> {
            let func = get_creature_event_func(lua, script_id, from_lua);
            let Some(func) = func else { return Ok(true); };
            let player_tbl = push_player(lua, player_id)?;
            match func.call::<bool>((player_tbl, skill as i64, old_level as i64, new_level as i64)) {
                Ok(v) => Ok(v),
                Err(e) => { tracing::error!("Lua onAdvance error: {e}"); Ok(true) }
            }
        })();
        ScriptEnvironment::reset();
        if !result.unwrap_or(true) {
            return;
        }
    }
}

/// Fire per-creature `onThink(creature, interval)` creature script events.
/// Mirrors `Creature::onThink` scripting portion.
pub fn execute_creature_event_think(creature_id: CreatureId, interval: u32) {
    crate::runtime::assert_dispatcher_thread!();
    use crate::events::creature::CreatureEventType;
    use crate::events::registry::g_script_registry;

    let names: Vec<String> = {
        let game = g_game().lock().unwrap();
        match game.get_creature(creature_id) {
            Some(c) => c.base().get_creature_event_names(CreatureEventType::Think),
            None => return,
        }
    };
    if names.is_empty() { return; }
    let script_ids: Vec<(i32, bool)> = {
        let registry = g_script_registry().lock().unwrap();
        names.iter()
            .filter_map(|n| registry.creature_events.get_event_by_name(n, true).map(|e| (e.script_id, e.from_lua)))
            .collect()
    };

    let lua = g_lua();
    for (script_id, from_lua) in script_ids {
        if !ScriptEnvironment::reserve() {
            tracing::error!("CreatureEvent::executeOnThink - Call stack overflow");
            return;
        }
        ScriptEnvironment::set_script_id(script_id, "CreatureScript Interface");
        let _ = (|| -> LuaResult<()> {
            let func = get_creature_event_func(lua, script_id, from_lua);
            let Some(func) = func else { return Ok(()); };
            let creature_tbl = push_creature(lua, creature_id)?;
            match func.call::<bool>((creature_tbl, interval as i64)) {
                Ok(_) => Ok(()),
                Err(e) => { tracing::error!("Lua onThink error: {e}"); Ok(()) }
            }
        })();
        ScriptEnvironment::reset();
    }
}

fn get_creature_event_func(lua: &mlua::Lua, script_id: i32, from_lua: bool) -> Option<mlua::Function> {
    if from_lua {
        let registry = g_script_registry().lock().unwrap();
        registry.get_callback_function(lua, script_id)
    } else {
        let registry = g_script_registry().lock().unwrap();
        registry.creature_events.get_script_function(script_id)
    }
}

/// Fire a single `onKill(creature, target)` script. `from_lua` selects the function source.
pub fn fire_kill_script(script_id: i32, from_lua: bool, killer_id: CreatureId, target_id: CreatureId) {
    let lua = g_lua();
    if !ScriptEnvironment::reserve() {
        tracing::error!("CreatureEvent::executeOnKill - Call stack overflow");
        return;
    }
    ScriptEnvironment::set_script_id(script_id, "CreatureScript Interface");
    let _ = (|| -> LuaResult<()> {
        let func = get_creature_event_func(lua, script_id, from_lua);
        let Some(func) = func else { return Ok(()); };
        let killer_tbl = push_creature(lua, killer_id)?;
        let target_tbl = push_creature(lua, target_id)?;
        match func.call::<()>((killer_tbl, target_tbl)) {
            Ok(_) => Ok(()),
            Err(e) => { tracing::error!("Lua onKill error: {e}"); Ok(()) }
        }
    })();
    ScriptEnvironment::reset();
}

#[allow(clippy::too_many_arguments)]
pub fn fire_death_script(
    script_id: i32,
    from_lua: bool,
    creature_id: CreatureId,
    last_hit_id: CreatureId,
    most_damage_id: CreatureId,
    corpse_server_id: u16,
    corpse_pos: Position,
    corpse_tile_idx: i32,
) {
    let lua = g_lua();
    if !ScriptEnvironment::reserve() {
        tracing::error!("CreatureEvent::executeOnDeath - Call stack overflow");
        return;
    }
    ScriptEnvironment::set_script_id(script_id, "CreatureScript Interface");
    let _ = (|| -> LuaResult<()> {
        let func = get_creature_event_func(lua, script_id, from_lua);
        let Some(func) = func else { return Ok(()); };
        let creature_tbl = push_creature(lua, creature_id)?;
        let corpse_val: LuaValue = if corpse_server_id > 0 && corpse_tile_idx >= 0 {
            let t = push_item(lua, corpse_server_id, corpse_pos, corpse_tile_idx)?;
            if let Ok(mt) = lua.named_registry_value::<LuaTable>("Container") {
                let _ = t.set_metatable(Some(mt));
            }
            LuaValue::Table(t)
        } else {
            LuaValue::Nil
        };
        let killer_val: LuaValue = if last_hit_id != 0 {
            LuaValue::Table(push_creature(lua, last_hit_id)?)
        } else {
            LuaValue::Nil
        };
        let most_dmg_val: LuaValue = if most_damage_id != 0 {
            LuaValue::Table(push_creature(lua, most_damage_id)?)
        } else {
            LuaValue::Nil
        };
        match func.call::<bool>((creature_tbl, corpse_val, killer_val, most_dmg_val, false, false)) {
            Ok(_) => Ok(()),
            Err(e) => { tracing::error!("Lua onDeath error: {e}"); Ok(()) }
        }
    })();
    ScriptEnvironment::reset();
}

/// Fire HEALTHCHANGE creature events on a target. Mirrors C++ `CreatureEvent::executeHealthChange`.
///
/// Returns `true` if any events were registered and fired (meaning the caller
/// should set `origin = ORIGIN_NONE` and re-call `combat_change_health`).
pub fn fire_health_change_events(
    target_id: CreatureId,
    attacker_id: Option<CreatureId>,
    damage: &mut CombatDamage,
) -> bool {
    use crate::events::creature::CreatureEventType;

    let script_ids: Vec<(i32, bool)> = {
        let game = g_game().lock().unwrap();
        let names = match game.get_creature(target_id) {
            Some(c) => c.base().get_creature_event_names(CreatureEventType::HealthChange),
            None => return false,
        };
        if names.is_empty() { return false; }
        let registry = g_script_registry().lock().unwrap();
        names.iter()
            .filter_map(|n| registry.creature_events.get_event_by_name(n, true).map(|e| (e.script_id, e.from_lua)))
            .collect()
    };

    if script_ids.is_empty() { return false; }

    let lua = g_lua();
    for (script_id, from_lua) in &script_ids {
        if !ScriptEnvironment::reserve() {
            tracing::error!("CreatureEvent::executeHealthChange - Call stack overflow");
            return true;
        }
        ScriptEnvironment::set_script_id(*script_id, "CreatureScript Interface");
        let _ = (|| -> LuaResult<()> {
            let func = get_creature_event_func(lua, *script_id, *from_lua);
            let Some(func) = func else { return Ok(()); };

            let creature_tbl = push_creature(lua, target_id)?;
            let attacker_val: LuaValue = match attacker_id {
                Some(aid) => LuaValue::Table(push_creature(lua, aid)?),
                None => LuaValue::Nil,
            };

            let r: LuaMultiValue = func.call((
                creature_tbl,
                attacker_val,
                damage.primary_value as i64,
                damage.primary_type as u32 as i64,
                damage.secondary_value as i64,
                damage.secondary_type as u32 as i64,
                damage.origin as u32 as i64,
            ))?;

            let vals: Vec<LuaValue> = r.into_vec();
            if vals.len() >= 4 {
                let pv = match &vals[0] { LuaValue::Integer(n) => *n as i32, LuaValue::Number(n) => *n as i32, _ => damage.primary_value };
                let pt = match &vals[1] { LuaValue::Integer(n) => *n as u32, LuaValue::Number(n) => *n as u32, _ => damage.primary_type as u32 };
                let sv = match &vals[2] { LuaValue::Integer(n) => *n as i32, LuaValue::Number(n) => *n as i32, _ => damage.secondary_value };
                let st = match &vals[3] { LuaValue::Integer(n) => *n as u32, LuaValue::Number(n) => *n as u32, _ => damage.secondary_type as u32 };

                damage.primary_value = pv.unsigned_abs() as i32;
                damage.primary_type = CombatType::from_u32(pt);
                damage.secondary_value = sv.unsigned_abs() as i32;
                damage.secondary_type = CombatType::from_u32(st);

                if damage.primary_type != CombatType::Healing {
                    damage.primary_value = -(damage.primary_value);
                    damage.secondary_value = -(damage.secondary_value);
                }
            }

            Ok(())
        })();
        ScriptEnvironment::reset();
    }
    true
}

/// Fire MANACHANGE creature events on a target. Mirrors C++ `CreatureEvent::executeManaChange`.
///
/// Returns `true` if any events were registered and fired.
pub fn fire_mana_change_events(
    target_id: CreatureId,
    attacker_id: Option<CreatureId>,
    damage: &mut CombatDamage,
) -> bool {
    use crate::events::creature::CreatureEventType;

    let script_ids: Vec<(i32, bool)> = {
        let game = g_game().lock().unwrap();
        let names = match game.get_creature(target_id) {
            Some(c) => c.base().get_creature_event_names(CreatureEventType::ManaChange),
            None => return false,
        };
        if names.is_empty() { return false; }
        let registry = g_script_registry().lock().unwrap();
        names.iter()
            .filter_map(|n| registry.creature_events.get_event_by_name(n, true).map(|e| (e.script_id, e.from_lua)))
            .collect()
    };

    if script_ids.is_empty() { return false; }

    let lua = g_lua();
    for (script_id, from_lua) in &script_ids {
        if !ScriptEnvironment::reserve() {
            tracing::error!("CreatureEvent::executeManaChange - Call stack overflow");
            return true;
        }
        ScriptEnvironment::set_script_id(*script_id, "CreatureScript Interface");
        let _ = (|| -> LuaResult<()> {
            let func = get_creature_event_func(lua, *script_id, *from_lua);
            let Some(func) = func else { return Ok(()); };

            let creature_tbl = push_creature(lua, target_id)?;
            let attacker_val: LuaValue = match attacker_id {
                Some(aid) => LuaValue::Table(push_creature(lua, aid)?),
                None => LuaValue::Nil,
            };

            let r: LuaMultiValue = func.call((
                creature_tbl,
                attacker_val,
                damage.primary_value as i64,
                damage.primary_type as u32 as i64,
                damage.secondary_value as i64,
                damage.secondary_type as u32 as i64,
                damage.origin as u32 as i64,
            ))?;

            let vals: Vec<LuaValue> = r.into_vec();
            if vals.len() >= 4 {
                let pv = match &vals[0] { LuaValue::Integer(n) => *n as i32, LuaValue::Number(n) => *n as i32, _ => damage.primary_value };
                let pt = match &vals[1] { LuaValue::Integer(n) => *n as u32, LuaValue::Number(n) => *n as u32, _ => damage.primary_type as u32 };
                let sv = match &vals[2] { LuaValue::Integer(n) => *n as i32, LuaValue::Number(n) => *n as i32, _ => damage.secondary_value };
                let st = match &vals[3] { LuaValue::Integer(n) => *n as u32, LuaValue::Number(n) => *n as u32, _ => damage.secondary_type as u32 };

                damage.primary_value = pv;
                damage.primary_type = CombatType::from_u32(pt);
                damage.secondary_value = sv;
                damage.secondary_type = CombatType::from_u32(st);
            }

            Ok(())
        })();
        ScriptEnvironment::reset();
    }
    true
}

/// Fire PREPAREDEATH creature events on a target. Mirrors C++ `Creature::onPrepareDeath`.
///
/// Returns `false` if any callback returned `false` (preventing death).
pub fn fire_prepare_death_events(
    target_id: CreatureId,
    attacker_id: Option<CreatureId>,
) -> bool {
    use crate::events::creature::CreatureEventType;

    let script_ids: Vec<(i32, bool)> = {
        let game = g_game().lock().unwrap();
        let names = match game.get_creature(target_id) {
            Some(c) => c.base().get_creature_event_names(CreatureEventType::PrepareDeath),
            None => return true,
        };
        if names.is_empty() { return true; }
        let registry = g_script_registry().lock().unwrap();
        names.iter()
            .filter_map(|n| registry.creature_events.get_event_by_name(n, true).map(|e| (e.script_id, e.from_lua)))
            .collect()
    };

    if script_ids.is_empty() { return true; }

    let lua = g_lua();
    for (script_id, from_lua) in script_ids {
        if !ScriptEnvironment::reserve() {
            tracing::error!("CreatureEvent::executeOnPrepareDeath - Call stack overflow");
            return false;
        }
        ScriptEnvironment::set_script_id(script_id, "CreatureScript Interface");
        let result = (|| -> LuaResult<bool> {
            let func = get_creature_event_func(lua, script_id, from_lua);
            let Some(func) = func else { return Ok(true); };

            let creature_tbl = push_creature(lua, target_id)?;
            let killer_val: LuaValue = match attacker_id {
                Some(aid) => LuaValue::Table(push_creature(lua, aid)?),
                None => LuaValue::Nil,
            };

            match func.call::<bool>((creature_tbl, killer_val)) {
                Ok(v) => Ok(v),
                Err(e) => { tracing::error!("Lua onPrepareDeath error: {e}"); Ok(true) }
            }
        })();
        ScriptEnvironment::reset();
        if !result.unwrap_or(true) {
            return false;
        }
    }
    true
}

pub fn execute_talk_action(
    player_id: CreatureId,
    words: &str,
    param: &str,
    talk_type: u8,
) -> bool {
    let (script_id, from_lua) = {
        let registry = g_script_registry().lock().unwrap();
        match registry.talk_actions.get_talk_action(words) {
            Some(ta) if ta.scripted => (ta.script_id, ta.from_lua),
            _ => return false,
        }
    };

    let lua = g_lua();

    if !ScriptEnvironment::reserve() {
        tracing::error!("TalkAction::executeSay - Call stack overflow");
        return false;
    }
    ScriptEnvironment::set_script_id(script_id, "TalkAction Interface");

    let result = (|| -> LuaResult<bool> {
        let func = if from_lua {
            let registry = g_script_registry().lock().unwrap();
            registry.get_callback_function(lua, script_id)
        } else {
            let registry = g_script_registry().lock().unwrap();
            registry.talk_actions.get_script_function(script_id)
        };

        let Some(func) = func else {
            return Ok(false);
        };

        let player_tbl = push_player(lua, player_id)?;

        match func.call::<bool>((player_tbl, words.to_string(), param.to_string(), talk_type)) {
            Ok(v) => Ok(v),
            Err(e) => {
                tracing::error!("Lua talkaction error: {e}");
                Ok(false)
            }
        }
    })();

    ScriptEnvironment::reset();

    result.unwrap_or(false)
}

/// Fire `onStepIn` or `onStepOut` events for a creature moving to/from `tile_pos`.
/// `event_type`: 0 = StepIn, 1 = StepOut.
/// Mirrors `MoveEvents::onCreatureMove`.
pub fn execute_step_event(
    creature_id: CreatureId,
    tile_pos: Position,
    from_pos: Position,
    event_type: u8,
) {
    use crate::events::movement::MoveEventType;

    let etype = if event_type == 0 { MoveEventType::StepIn } else { MoveEventType::StepOut };

    let tile_items: Vec<(u16, u16, u16)> = {
        let game = g_game().lock().unwrap();
        match game.map.get_tile(tile_pos) {
            Some(tile) => {
                let mut items = Vec::new();
                if let Some(g) = &tile.ground {
                    items.push((g.server_id, g.action_id, g.unique_id));
                }
                for it in &tile.items {
                    items.push((it.server_id, it.action_id, it.unique_id));
                }
                items
            }
            None => return,
        }
    };

    let events: Vec<(i32, bool, u16)> = {
        let registry = g_script_registry().lock().unwrap();
        registry.move_events.collect_step_events(tile_pos, etype, &tile_items)
    };

    if events.is_empty() {
        return;
    }
    tracing::info!(?tile_pos, ?etype, n_events = events.len(), thread = ?std::thread::current().id(), "DBG step_event");

    let lua = g_lua();
    for (script_id, from_lua, item_server_id) in events {
        if !ScriptEnvironment::reserve() {
            tracing::error!("MoveEvent::executeStep - Call stack overflow");
            return;
        }
        ScriptEnvironment::set_script_id(script_id, "MoveEvents Interface");

        let _ = (|| -> mlua::Result<()> {
            let func = if from_lua {
                let registry = g_script_registry().lock().unwrap();
                registry.get_callback_function(lua, script_id)
            } else {
                let registry = g_script_registry().lock().unwrap();
                registry.move_events.get_script_function(script_id)
            };
            let Some(func) = func else { return Ok(()); };

            let creature_tbl = push_creature(lua, creature_id)?;
            let item_val: mlua::Value = if item_server_id > 0 {
                mlua::Value::Table(push_item(lua, item_server_id, tile_pos, 0)?)
            } else {
                mlua::Value::Nil
            };
            let pos_tbl = push_position(lua, tile_pos)?;
            let from_pos_tbl = push_position(lua, from_pos)?;

            tracing::info!(script_id, item_server_id, "DBG step_event: calling Lua onStep");
            let r = func.call::<bool>((creature_tbl, item_val, pos_tbl, from_pos_tbl));
            tracing::info!(script_id, ok = r.is_ok(), "DBG step_event: Lua onStep returned");
            match r {
                Ok(_) => Ok(()),
                Err(e) => { tracing::error!("Lua onStep error: {e}"); Ok(()) }
            }
        })();

        ScriptEnvironment::reset();
    }
}

/// Fire `onEquip(player, item, slot, isCheck)` or `onDeEquip` movement events.
/// Mirrors `MoveEvents::onPlayerEquip` / `onPlayerDeEquip`.
#[allow(clippy::too_many_arguments)]
pub fn execute_equip_event(
    player_id: CreatureId,
    item_server_id: u16,
    item_pos: Position,
    item_index: i32,
    item_action_id: u16,
    item_unique_id: u16,
    slot: u32,
    is_equip: bool,
    is_check: bool,
) -> bool {
    use crate::events::movement::MoveEventType;

    let etype = if is_equip { MoveEventType::Equip } else { MoveEventType::DeEquip };

    let events: Vec<(i32, bool)> = {
        let registry = g_script_registry().lock().unwrap();
        registry.move_events.collect_equip_events(item_server_id, item_action_id, item_unique_id, etype, slot)
    };

    if events.is_empty() {
        return true;
    }

    let lua = g_lua();
    for (script_id, from_lua) in events {
        if !ScriptEnvironment::reserve() {
            tracing::error!("MoveEvent::executeEquip - Call stack overflow");
            return false;
        }
        ScriptEnvironment::set_script_id(script_id, "MoveEvents Interface");

        let result = (|| -> mlua::Result<bool> {
            let func = if from_lua {
                let registry = g_script_registry().lock().unwrap();
                registry.get_callback_function(lua, script_id)
            } else {
                let registry = g_script_registry().lock().unwrap();
                registry.move_events.get_script_function(script_id)
            };
            let Some(func) = func else { return Ok(true); };

            let player_tbl = push_player(lua, player_id)?;
            let item_tbl = push_item(lua, item_server_id, item_pos, item_index)?;

            match func.call::<bool>((player_tbl, item_tbl, slot as i64, is_check)) {
                Ok(v) => Ok(v),
                Err(e) => { tracing::error!("Lua onEquip error: {e}"); Ok(false) }
            }
        })();

        ScriptEnvironment::reset();

        if !result.unwrap_or(false) {
            return false;
        }
    }

    true
}
