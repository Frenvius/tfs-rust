use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex as StdMutex;

use mlua::prelude::*;

use crate::combat::condition::{create_condition, ConditionId, ConditionType};
use crate::config::{g_config, BooleanConfig, IntegerConfig, StringConfig};
use crate::creatures::CreatureId;
use crate::db::{g_database, DatabaseEngine, DbResult};
use crate::game::g_game;
use crate::map::{Position, TileKind};
use crate::net::game_protocol::{send_packet_to_player, broadcast_effect_to_spectators};
use crate::net::message::NetworkMessage;
use crate::runtime::g_scheduler;
use crate::runtime::scheduler::SchedulerTask;

fn get_creature_id(t: &LuaTable) -> LuaResult<CreatureId> {
    t.raw_get::<u32>(1)
}


static NEXT_AREA_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
type AreaMap = std::sync::Mutex<std::collections::HashMap<u32, Vec<(i8, i8)>>>;
static AREA_PATTERNS: std::sync::OnceLock<AreaMap> = std::sync::OnceLock::new();
fn get_area_patterns() -> &'static AreaMap {
    AREA_PATTERNS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn parse_area_table(tbl: &LuaTable) -> Vec<(i8, i8)> {
    let mut offsets = Vec::new();
    let mut center_row = 0i8;
    let mut center_col = 0i8;
    let mut found_center = false;
    let mut rows: Vec<Vec<u8>> = Vec::new();
    for row_val in tbl.clone().sequence_values::<LuaTable>().flatten() {
        let mut row = Vec::new();
        for cell_val in row_val.sequence_values::<u8>().flatten() {
            if cell_val == 3 && !found_center {
                center_row = rows.len() as i8;
                center_col = row.len() as i8;
                found_center = true;
            }
            row.push(cell_val);
        }
        rows.push(row);
    }
    for (r, row) in rows.iter().enumerate() {
        for (c, &cell) in row.iter().enumerate() {
            if cell != 0 {
                offsets.push((r as i8 - center_row, c as i8 - center_col));
            }
        }
    }
    offsets
}

fn pos_field(t: &LuaTable, key: &str) -> LuaResult<i64> {
    Ok(t.get::<Option<i64>>(key)?.unwrap_or(0))
}

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

pub(crate) fn table_to_position(t: &LuaTable) -> LuaResult<Position> {
    Ok(Position {
        x: t.get::<Option<u16>>("x")?.unwrap_or(0),
        y: t.get::<Option<u16>>("y")?.unwrap_or(0),
        z: t.get::<Option<u8>>("z")?.unwrap_or(0),
    })
}

fn get_return_message(code: i32) -> &'static str {
    match code {
        1 => "Sorry, not possible.",
        2 | 13 => "There is not enough room.",
        3 => "You can not enter a protection zone after attacking another player.",
        4 => "You are not invited.",
        5 => "You cannot throw there.",
        6 => "There is no way.",
        7 => "Destination is out of range.",
        9 => "You cannot move this object.",
        10 => "Drop the double-handed object first.",
        11 => "Both hands need to be free.",
        12 => "You may only use one weapon.",
        14 => "You cannot dress this object there.",
        15 => "Put this object in your hand.",
        16 => "Put this object in both hands.",
        17 => "You are too far away.",
        18 => "First go downstairs.",
        19 => "First go upstairs.",
        20 => "You cannot put more objects in this container.",
        21 => "This object is too heavy for you to carry.",
        22 => "You cannot take this object.",
        23 => "This is impossible.",
        24 => "You cannot put more items in this depot.",
        25 => "Creature does not exist.",
        26 => "You cannot use this object.",
        27 => "A player with this name is not online.",
        28 => "You do not have the required magic level to use this rune.",
        29 => "You are already trading. Finish this trade first.",
        30 => "This player is already trading.",
        31 => "You may not logout during or immediately after a fight!",
        32 => "You are not allowed to shoot directly on players.",
        33 => "Your level is too low.",
        34 => "You do not have enough magic level.",
        35 => "You do not have enough mana.",
        36 => "You do not have enough soul.",
        37 => "You are exhausted.",
        38 => "You cannot use objects that fast.",
        39 => "Player is not reachable.",
        40 => "You can only use it on creatures.",
        41 => "This action is not permitted in a protection zone.",
        42 => "You may not attack this person.",
        43 => "You may not attack a person in a protection zone.",
        44 => "You may not attack a person while you are in a protection zone.",
        45 => "You may not attack this creature.",
        46 => "You can only use it on creatures.",
        47 => "Creature is not reachable.",
        48 => "Turn secure mode off if you really want to attack unmarked players.",
        49 => "You need a premium account.",
        50 => "You must learn this spell first.",
        51 => "Your vocation cannot use this spell.",
        52 => "You need a weapon to use this spell.",
        53 => "You can not leave a pvp zone after attacking another player.",
        54 => "You can not enter a pvp zone after attacking another player.",
        55 => "This action is not permitted in a non pvp zone.",
        56 => "You can not log out here.",
        57 => "You need a magic item to cast this spell.",
        58 => "You cannot conjure items here.",
        59 => "You need to split your spears first.",
        60 => "Name is too ambiguous.",
        61 => "You may use only one shield.",
        62 => "No party members in range.",
        63 => "You are not the owner.",
        64 => "No such raid exists.",
        65 => "Another raid is already executing.",
        66 => "Trade player is too far away.",
        67 => "You don't own this house.",
        68 => "Trade player already owns a house.",
        69 => "Trade player is the highest bidder.",
        70 => "You cannot trade this house.",
        _ => "Sorry, not possible.",
    }
}

pub(crate) fn push_creature_ref(lua: &Lua, cid: CreatureId, class_name: &str) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;
    t.raw_set(1, cid)?;
    if let Ok(mt) = lua.named_registry_value::<LuaTable>(class_name) {
        let _ = t.set_metatable(Some(mt));
    }
    Ok(t)
}

/// Build an Item ref table. Does NOT lock g_game — pass `uid`/`action_id`
/// directly (callers iterating tiles already hold the `MapItem`). Re-locking
/// here while a tile method holds the lock is a re-entrant deadlock on the
/// non-reentrant g_game mutex.
fn push_item_ref(lua: &Lua, server_id: u16, pos: Position, index: i32) -> LuaResult<LuaTable> {
    push_item_ref_attrs(lua, server_id, pos, index, 0, 0)
}

fn push_item_ref_attrs(
    lua: &Lua,
    server_id: u16,
    pos: Position,
    index: i32,
    uid: u16,
    action_id: u16,
) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;
    t.raw_set(1, server_id as i64)?;
    t.raw_set("itemid", server_id as i64)?;
    t.raw_set("_pos_x", pos.x)?;
    t.raw_set("_pos_y", pos.y)?;
    t.raw_set("_pos_z", pos.z)?;
    t.raw_set("_idx", index)?;
    t.raw_set("uid", uid as i64)?;
    if action_id > 0 {
        t.raw_set("actionid", action_id as i64)?;
    }

    if let Ok(mt) = lua.named_registry_value::<LuaTable>("Item") {
        let _ = t.set_metatable(Some(mt));
    }
    Ok(t)
}

fn get_item_from_lua<'a>(
    this: &LuaTable,
    game: &'a crate::game::Game,
) -> Option<&'a crate::map::tile::MapItem> {
    let pos_x: Option<u16> = this.raw_get("_pos_x").ok();
    let pos_y: Option<u16> = this.raw_get("_pos_y").ok();
    let pos_z: Option<u8> = this.raw_get("_pos_z").ok();
    let idx: Option<i32> = this.raw_get("_idx").ok();
    let (Some(x), Some(y), Some(z), Some(idx)) = (pos_x, pos_y, pos_z, idx) else {
        return None;
    };
    let pos = Position { x, y, z };
    let tile = game.map.get_tile(pos)?;
    if idx == -1 {
        return tile.ground.as_ref();
    }
    tile.items.get(idx as usize)
}

fn get_item_from_lua_mut<'a>(
    this: &LuaTable,
    game: &'a mut crate::game::Game,
) -> Option<&'a mut crate::map::tile::MapItem> {
    let pos_x: Option<u16> = this.raw_get("_pos_x").ok();
    let pos_y: Option<u16> = this.raw_get("_pos_y").ok();
    let pos_z: Option<u8> = this.raw_get("_pos_z").ok();
    let idx: Option<i32> = this.raw_get("_idx").ok();
    let (Some(x), Some(y), Some(z), Some(idx)) = (pos_x, pos_y, pos_z, idx) else {
        return None;
    };
    let pos = Position { x, y, z };
    let tile = game.map.get_tile_mut(pos)?;
    if idx == -1 {
        return tile.ground.as_mut();
    }
    tile.items.get_mut(idx as usize)
}

use crate::map::tile::MapItem;

const ITEM_ATTR_ACTIONID: u32 = 1;
const ITEM_ATTR_UNIQUEID: u32 = 2;
const ITEM_ATTR_DESCRIPTION: u32 = 3;
const ITEM_ATTR_TEXT: u32 = 4;
const ITEM_ATTR_CHARGES: u32 = 5;
const ITEM_ATTR_NAME: u32 = 6;
const ITEM_ATTR_ARTICLE: u32 = 7;
const ITEM_ATTR_PLURALNAME: u32 = 8;
const ITEM_ATTR_WEIGHT: u32 = 9;
const ITEM_ATTR_ATTACK: u32 = 10;
const ITEM_ATTR_DEFENSE: u32 = 11;
const ITEM_ATTR_EXTRADEFENSE: u32 = 12;
const ITEM_ATTR_ARMOR: u32 = 13;
const ITEM_ATTR_HITCHANCE: u32 = 14;
const ITEM_ATTR_SHOOTRANGE: u32 = 15;
const ITEM_ATTR_DURATION: u32 = 16;
const ITEM_ATTR_DECAYTO: u32 = 17;
const ITEM_ATTR_WRITER: u32 = 18;
const ITEM_ATTR_FLUIDTYPE: u32 = 19;

fn attr_name_to_id(name: &str) -> u32 {
    match name {
        "aid" | "actionid" => ITEM_ATTR_ACTIONID,
        "uid" | "uniqueid" => ITEM_ATTR_UNIQUEID,
        "description" => ITEM_ATTR_DESCRIPTION,
        "text" => ITEM_ATTR_TEXT,
        "charges" => ITEM_ATTR_CHARGES,
        "name" => ITEM_ATTR_NAME,
        "article" => ITEM_ATTR_ARTICLE,
        "pluralname" => ITEM_ATTR_PLURALNAME,
        "weight" => ITEM_ATTR_WEIGHT,
        "attack" => ITEM_ATTR_ATTACK,
        "defense" => ITEM_ATTR_DEFENSE,
        "extradefense" => ITEM_ATTR_EXTRADEFENSE,
        "armor" => ITEM_ATTR_ARMOR,
        "hitchance" => ITEM_ATTR_HITCHANCE,
        "shootrange" => ITEM_ATTR_SHOOTRANGE,
        "duration" => ITEM_ATTR_DURATION,
        "decayto" => ITEM_ATTR_DECAYTO,
        "writer" => ITEM_ATTR_WRITER,
        "fluidtype" => ITEM_ATTR_FLUIDTYPE,
        _ => 0,
    }
}

fn item_has_attr_by_id(item: &MapItem, attr_id: u32) -> bool {
    match attr_id {
        ITEM_ATTR_ACTIONID => item.action_id != 0,
        ITEM_ATTR_UNIQUEID => item.unique_id != 0,
        ITEM_ATTR_DESCRIPTION => !item.description.is_empty(),
        ITEM_ATTR_TEXT => !item.text.is_empty(),
        ITEM_ATTR_CHARGES => item.charges != 0,
        ITEM_ATTR_NAME => !item.name.is_empty(),
        ITEM_ATTR_ARTICLE => !item.article.is_empty(),
        ITEM_ATTR_PLURALNAME => !item.plural_name.is_empty(),
        ITEM_ATTR_WEIGHT => item.weight != 0,
        ITEM_ATTR_ATTACK => item.attack != 0,
        ITEM_ATTR_DEFENSE => item.defense != 0,
        ITEM_ATTR_EXTRADEFENSE => item.extra_defense != 0,
        ITEM_ATTR_ARMOR => item.armor != 0,
        ITEM_ATTR_HITCHANCE => item.hit_chance != 0,
        ITEM_ATTR_SHOOTRANGE => item.shoot_range != 0,
        ITEM_ATTR_DURATION => item.duration != 0,
        ITEM_ATTR_DECAYTO => item.decay_to != -1,
        ITEM_ATTR_WRITER => !item.written_by.is_empty(),
        ITEM_ATTR_FLUIDTYPE => item.fluid_type != 0,
        _ => false,
    }
}

fn item_has_attr_by_name(item: &MapItem, name: &str) -> bool {
    item_has_attr_by_id(item, attr_name_to_id(name))
}

fn item_get_attr_by_id(lua: &Lua, item: &MapItem, attr_id: u32) -> LuaResult<LuaValue> {
    match attr_id {
        ITEM_ATTR_ACTIONID => Ok(LuaValue::Integer(item.action_id as i64)),
        ITEM_ATTR_UNIQUEID => Ok(LuaValue::Integer(item.unique_id as i64)),
        ITEM_ATTR_DESCRIPTION => Ok(LuaValue::String(lua.create_string(&item.description)?)),
        ITEM_ATTR_TEXT => Ok(LuaValue::String(lua.create_string(&item.text)?)),
        ITEM_ATTR_CHARGES => Ok(LuaValue::Integer(item.charges as i64)),
        ITEM_ATTR_NAME => Ok(LuaValue::String(lua.create_string(&item.name)?)),
        ITEM_ATTR_ARTICLE => Ok(LuaValue::String(lua.create_string(&item.article)?)),
        ITEM_ATTR_PLURALNAME => Ok(LuaValue::String(lua.create_string(&item.plural_name)?)),
        ITEM_ATTR_WEIGHT => Ok(LuaValue::Integer(item.weight as i64)),
        ITEM_ATTR_ATTACK => Ok(LuaValue::Integer(item.attack as i64)),
        ITEM_ATTR_DEFENSE => Ok(LuaValue::Integer(item.defense as i64)),
        ITEM_ATTR_EXTRADEFENSE => Ok(LuaValue::Integer(item.extra_defense as i64)),
        ITEM_ATTR_ARMOR => Ok(LuaValue::Integer(item.armor as i64)),
        ITEM_ATTR_HITCHANCE => Ok(LuaValue::Integer(item.hit_chance as i64)),
        ITEM_ATTR_SHOOTRANGE => Ok(LuaValue::Integer(item.shoot_range as i64)),
        ITEM_ATTR_DURATION => Ok(LuaValue::Integer(item.duration as i64)),
        ITEM_ATTR_DECAYTO => Ok(LuaValue::Integer(item.decay_to as i64)),
        ITEM_ATTR_WRITER => Ok(LuaValue::String(lua.create_string(&item.written_by)?)),
        ITEM_ATTR_FLUIDTYPE => Ok(LuaValue::Integer(item.fluid_type as i64)),
        _ => Ok(LuaValue::Nil),
    }
}

fn item_get_attr_by_name(lua: &Lua, item: &MapItem, name: &str) -> LuaResult<LuaValue> {
    item_get_attr_by_id(lua, item, attr_name_to_id(name))
}

fn item_set_attr_by_id(item: &mut MapItem, attr_id: u32, val: &LuaValue) {
    let as_i32 = match val {
        LuaValue::Integer(n) => *n as i32,
        LuaValue::Number(n) => *n as i32,
        _ => 0,
    };
    let as_str = match val {
        LuaValue::String(s) => s.to_string_lossy().to_string(),
        _ => String::new(),
    };
    match attr_id {
        ITEM_ATTR_ACTIONID => item.action_id = as_i32.max(100) as u16,
        ITEM_ATTR_DESCRIPTION => item.description = as_str,
        ITEM_ATTR_TEXT => item.text = as_str,
        ITEM_ATTR_CHARGES => item.charges = as_i32 as u16,
        ITEM_ATTR_NAME => item.name = as_str,
        ITEM_ATTR_ARTICLE => item.article = as_str,
        ITEM_ATTR_PLURALNAME => item.plural_name = as_str,
        ITEM_ATTR_WEIGHT => item.weight = as_i32 as u32,
        ITEM_ATTR_ATTACK => item.attack = as_i32,
        ITEM_ATTR_DEFENSE => item.defense = as_i32,
        ITEM_ATTR_EXTRADEFENSE => item.extra_defense = as_i32,
        ITEM_ATTR_ARMOR => item.armor = as_i32,
        ITEM_ATTR_HITCHANCE => item.hit_chance = as_i32 as i8,
        ITEM_ATTR_SHOOTRANGE => item.shoot_range = as_i32 as u8,
        ITEM_ATTR_DURATION => item.duration = as_i32,
        ITEM_ATTR_DECAYTO => item.decay_to = as_i32,
        ITEM_ATTR_WRITER => item.written_by = as_str,
        ITEM_ATTR_FLUIDTYPE => item.fluid_type = as_i32 as u16,
        _ => {}
    }
}

fn item_set_attr_by_name(item: &mut MapItem, name: &str, val: &LuaValue) {
    item_set_attr_by_id(item, attr_name_to_id(name), val)
}

fn item_remove_attr_by_id(item: &mut MapItem, attr_id: u32) {
    match attr_id {
        ITEM_ATTR_ACTIONID => item.action_id = 0,
        ITEM_ATTR_DESCRIPTION => item.description.clear(),
        ITEM_ATTR_TEXT => item.text.clear(),
        ITEM_ATTR_CHARGES => item.charges = 0,
        ITEM_ATTR_NAME => item.name.clear(),
        ITEM_ATTR_ARTICLE => item.article.clear(),
        ITEM_ATTR_PLURALNAME => item.plural_name.clear(),
        ITEM_ATTR_WEIGHT => item.weight = 0,
        ITEM_ATTR_ATTACK => item.attack = 0,
        ITEM_ATTR_DEFENSE => item.defense = 0,
        ITEM_ATTR_EXTRADEFENSE => item.extra_defense = 0,
        ITEM_ATTR_ARMOR => item.armor = 0,
        ITEM_ATTR_HITCHANCE => item.hit_chance = 0,
        ITEM_ATTR_SHOOTRANGE => item.shoot_range = 0,
        ITEM_ATTR_DURATION => item.duration = 0,
        ITEM_ATTR_DECAYTO => item.decay_to = -1,
        ITEM_ATTR_WRITER => item.written_by.clear(),
        ITEM_ATTR_FLUIDTYPE => item.fluid_type = 0,
        _ => {}
    }
}

fn item_remove_attr_by_name(item: &mut MapItem, name: &str) {
    item_remove_attr_by_id(item, attr_name_to_id(name))
}

pub fn register_all(lua: &Lua) -> LuaResult<()> {
    register_print(lua)?;
    register_legacy_globals(lua)?;
    register_bit_library(lua)?;
    register_config_manager_table(lua)?;
    register_db_table(lua)?;
    register_result_table(lua)?;
    register_enums(lua)?;
    register_global_methods(lua)?;
    register_config_keys(lua)?;
    register_os_extensions(lua)?;
    register_table_extensions(lua)?;
    register_game_class(lua)?;
    register_variant_class(lua)?;
    register_position_class(lua)?;
    register_tile_class(lua)?;
    register_network_message_class(lua)?;
    register_item_class(lua)?;
    register_container_class(lua)?;
    register_teleport_class(lua)?;
    register_creature_class(lua)?;
    register_player_class(lua)?;
    register_monster_class(lua)?;
    register_npc_class(lua)?;
    register_guild_class(lua)?;
    register_group_class(lua)?;
    register_vocation_class(lua)?;
    register_town_class(lua)?;
    register_house_class(lua)?;
    register_item_type_class(lua)?;
    register_combat_class(lua)?;
    register_condition_class(lua)?;
    register_outfit_class(lua)?;
    register_monster_type_class(lua)?;
    register_loot_class(lua)?;
    register_monster_spell_class(lua)?;
    register_party_class(lua)?;
    register_spell_class(lua)?;
    register_action_class(lua)?;
    register_talk_action_class(lua)?;
    register_creature_event_class(lua)?;
    register_move_event_class(lua)?;
    register_global_event_class(lua)?;
    register_weapon_class(lua)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fnv1a_hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 14695981039346656037;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

fn lua_table_to_u16_vec(table: &LuaTable, field: &str) -> LuaResult<Vec<u16>> {
    let ids_table: LuaTable = match table.get(field) {
        Ok(t) => t,
        Err(_) => return Ok(Vec::new()),
    };
    let mut ids = Vec::new();
    for pair in ids_table.pairs::<i64, LuaValue>() {
        let (_, val) = pair?;
        match val {
            LuaValue::Integer(n) => ids.push(n as u16),
            LuaValue::Number(n) => ids.push(n as u16),
            _ => {}
        }
    }
    Ok(ids)
}

fn lua_table_to_u32_vec(table: &LuaTable, field: &str) -> LuaResult<Vec<u32>> {
    let ids_table: LuaTable = match table.get(field) {
        Ok(t) => t,
        Err(_) => return Ok(Vec::new()),
    };
    let mut ids = Vec::new();
    for pair in ids_table.pairs::<i64, LuaValue>() {
        let (_, val) = pair?;
        match val {
            LuaValue::Integer(n) => ids.push(n as u32),
            LuaValue::Number(n) => ids.push(n as u32),
            _ => {}
        }
    }
    Ok(ids)
}

fn lua_table_to_string_vec(table: &LuaTable, field: &str) -> LuaResult<Vec<String>> {
    let t: LuaTable = match table.get(field) {
        Ok(t) => t,
        Err(_) => return Ok(Vec::new()),
    };
    let mut strings = Vec::new();
    for pair in t.pairs::<i64, LuaValue>() {
        let (_, val) = pair?;
        if let LuaValue::String(s) = val {
            strings.push(s.to_string_lossy().to_string());
        }
    }
    Ok(strings)
}

// ---------------------------------------------------------------------------
// Lua timer event system (mirrors LuaTimerEventDesc in C++)
// ---------------------------------------------------------------------------

static LUA_TIMER_ID: AtomicU32 = AtomicU32::new(1);

struct LuaTimerEvent {
    scheduler_event_id: u32,
    script_id: i32,
    function_key: LuaRegistryKey,
    param_keys: Vec<LuaRegistryKey>,
}

static LUA_TIMER_EVENTS: std::sync::OnceLock<StdMutex<HashMap<u32, LuaTimerEvent>>> =
    std::sync::OnceLock::new();

fn lua_timer_events() -> &'static StdMutex<HashMap<u32, LuaTimerEvent>> {
    LUA_TIMER_EVENTS.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn execute_lua_timer_event(timer_id: u32) {
    use crate::lua::ScriptEnvironment;

    let event = {
        let mut events = lua_timer_events().lock().unwrap();
        events.remove(&timer_id)
    };

    let Some(event) = event else { return };

    let lua = crate::lua::g_lua();

    let func: LuaFunction = match lua.registry_value(&event.function_key) {
        Ok(f) => f,
        Err(_) => {
            lua.remove_registry_value(event.function_key).ok();
            for key in event.param_keys {
                lua.remove_registry_value(key).ok();
            }
            return;
        }
    };

    let mut args = LuaMultiValue::new();
    for key in &event.param_keys {
        match lua.registry_value::<LuaValue>(key) {
            Ok(v) => args.push_back(v),
            Err(_) => args.push_back(LuaValue::Nil),
        }
    }

    if ScriptEnvironment::reserve() {
        ScriptEnvironment::set_timer_event();
        ScriptEnvironment::set_script_id(event.script_id, "Main Interface");

        if let Err(e) = func.call::<()>(args) {
            crate::lua::LuaScriptInterface::report_error(None, &e.to_string());
        }

        ScriptEnvironment::reset();
    } else {
        tracing::error!("[LuaEnvironment] execute_timer_event: call stack overflow (timer_id={})", timer_id);
    }

    lua.remove_registry_value(event.function_key).ok();
    for key in event.param_keys {
        lua.remove_registry_value(key).ok();
    }
}

// ---------------------------------------------------------------------------
// Lua DB result store (mirrors ScriptEnvironment::m_tempResults in C++)
// ---------------------------------------------------------------------------

static DB_RESULT_ID: AtomicU32 = AtomicU32::new(1);

static DB_RESULTS: std::sync::OnceLock<StdMutex<HashMap<u32, DbResult>>> =
    std::sync::OnceLock::new();

fn db_results() -> &'static StdMutex<HashMap<u32, DbResult>> {
    DB_RESULTS.get_or_init(|| StdMutex::new(HashMap::new()))
}

static LAST_INSERT_ID: AtomicU32 = AtomicU32::new(0);

fn db_blocking<F, T>(f: F) -> Result<T, String>
where
    F: std::future::Future<Output = Result<T, crate::db::DatabaseError>> + Send,
    T: Send,
{
    let handle = tokio::runtime::Handle::current();
    std::thread::scope(|s| {
        s.spawn(|| handle.block_on(f))
            .join()
            .unwrap_or_else(|_| Err(crate::db::DatabaseError::NotConnected))
            .map_err(|e| e.to_string())
    })
}

/// Mirrors `LuaScriptInterface::registerClass`.
/// Returns the methods table (== _G[class_name]).
fn register_class(
    lua: &Lua,
    class_name: &str,
    base_class: Option<&str>,
    lua_data_type: i64,
    constructor: Option<LuaFunction>,
) -> LuaResult<LuaTable> {
    let globals = lua.globals();
    let methods: LuaTable = lua.create_table()?;
    let methods_mt: LuaTable = lua.create_table()?;

    if let Some(ctor) = constructor {
        methods_mt.set("__call", ctor)?;
    }

    let parents: i64 = if let Some(base) = base_class {
        let base_methods: LuaTable = globals.get(base)?;
        methods_mt.set("__index", base_methods)?;
        1
    } else {
        0
    };

    methods.set_metatable(Some(methods_mt))?;
    globals.set(class_name, methods.clone())?;

    let instance_mt: LuaTable = lua.create_table()?;
    instance_mt.set("__index", methods.clone())?;
    instance_mt.set("__metatable", methods.clone())?;
    instance_mt.raw_set(104i64, fnv1a_hash(class_name.as_bytes()) as i64)?; // 'h'
    instance_mt.raw_set(112i64, parents)?; // 'p'
    instance_mt.raw_set(116i64, lua_data_type)?; // 't'

    lua.set_named_registry_value(class_name, instance_mt)?;

    Ok(methods)
}

fn set_meta(lua: &Lua, class_name: &str, meta_name: &str, func: LuaFunction) -> LuaResult<()> {
    let instance_mt: LuaTable = lua.named_registry_value(class_name)?;
    instance_mt.set(meta_name, func)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// LuaDataType values (mirrors luascript.h)
// ---------------------------------------------------------------------------
const LUA_DATA_UNKNOWN: i64 = 0;
const LUA_DATA_ITEM: i64 = 1;
const LUA_DATA_CONTAINER: i64 = 2;
const LUA_DATA_TELEPORT: i64 = 3;
const LUA_DATA_PLAYER: i64 = 4;
const LUA_DATA_MONSTER: i64 = 5;
const LUA_DATA_NPC: i64 = 6;
const LUA_DATA_TILE: i64 = 7;

// ---------------------------------------------------------------------------
// print → tracing
// ---------------------------------------------------------------------------

fn register_print(lua: &Lua) -> LuaResult<()> {
    let f = lua.create_function(|_, args: LuaMultiValue| {
        let parts: Vec<String> = args
            .iter()
            .map(|v| match v {
                LuaValue::String(s) => {
                    s.to_str().map(|t| t.to_owned()).unwrap_or_else(|_| "?".to_owned())
                }
                other => format!("{other:?}"),
            })
            .collect();
        tracing::info!("[Lua] {}", parts.join("\t"));
        Ok(())
    })?;
    lua.globals().set("print", f)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Legacy global functions
// ---------------------------------------------------------------------------

fn register_legacy_globals(lua: &Lua) -> LuaResult<()> {
    let g = lua.globals();

    g.set("getWorldTime", lua.create_function(|_, ()| -> LuaResult<i64> {
        Ok(g_game().lock().unwrap().get_world_time() as i64)
    })?)?;

    g.set("getWorldLight", lua.create_function(|lua, ()| -> LuaResult<(u8, u8)> {
        let (level, color) = g_game().lock().unwrap().get_world_light_info();
        let _ = lua;
        Ok((level, color))
    })?)?;

    g.set("setWorldLight", lua.create_function(|_, (level, color): (u8, u8)| -> LuaResult<()> {
        g_game().lock().unwrap().set_world_light_info(level, color);
        Ok(())
    })?)?;

    g.set("getWorldUpTime", lua.create_function(|_, ()| -> LuaResult<i64> {
        Ok(g_game().lock().unwrap().get_uptime_seconds())
    })?)?;

    g.set("addEvent", lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<i64> {
        use crate::lua::ScriptEnvironment;

        let args_vec: Vec<LuaValue> = args.into_iter().collect();
        if args_vec.len() < 2 {
            return Err(LuaError::RuntimeError(
                format!("Not enough parameters: {}.", args_vec.len()),
            ));
        }

        let func: LuaFunction = match args_vec[0].clone() {
            LuaValue::Function(f) => f,
            _ => return Err(LuaError::RuntimeError("callback parameter should be a function.".into())),
        };

        let delay = match &args_vec[1] {
            LuaValue::Integer(n) => (*n as u32).max(100),
            LuaValue::Number(n) => (*n as u32).max(100),
            _ => return Err(LuaError::RuntimeError("delay parameter should be a number.".into())),
        };

        let func_key = lua.create_registry_value(func)?;
        let mut param_keys = Vec::new();
        for arg in args_vec.into_iter().skip(2) {
            param_keys.push(lua.create_registry_value(arg)?);
        }

        let script_id = ScriptEnvironment::get_event_info()
            .map(|(sid, _, _, _)| sid)
            .unwrap_or(0);

        let timer_id = LUA_TIMER_ID.fetch_add(1, Ordering::Relaxed);

        let scheduler_event_id = g_scheduler().add_event(SchedulerTask::new(delay, move || {
            execute_lua_timer_event(timer_id);
        }));

        lua_timer_events().lock().unwrap().insert(timer_id, LuaTimerEvent {
            scheduler_event_id,
            script_id,
            function_key: func_key,
            param_keys,
        });

        Ok(timer_id as i64)
    })?)?;

    g.set("stopEvent", lua.create_function(|lua, id: i64| -> LuaResult<bool> {
        let timer_id = id as u32;
        let event = {
            let mut events = lua_timer_events().lock().unwrap();
            events.remove(&timer_id)
        };
        match event {
            Some(event) => {
                g_scheduler().stop_event(event.scheduler_event_id);
                lua.remove_registry_value(event.function_key).ok();
                for key in event.param_keys {
                    lua.remove_registry_value(key).ok();
                }
                Ok(true)
            }
            None => Ok(false),
        }
    })?)?;

    g.set("isInWar", lua.create_function(|_, (_a, _b): (LuaValue, LuaValue)| -> LuaResult<bool> {
        Ok(false)
    })?)?;

    g.set("saveServer", lua.create_function(|_, ()| -> LuaResult<bool> {
        Ok(true)
    })?)?;

    g.set("cleanMap", lua.create_function(|_, ()| -> LuaResult<i64> {
        Ok(0)
    })?)?;

    g.set("debugPrint", lua.create_function(|_, msg: String| -> LuaResult<()> {
        tracing::debug!("[Lua debug] {}", msg);
        Ok(())
    })?)?;

    g.set("isScriptsInterface", lua.create_function(|_, ()| -> LuaResult<bool> {
        Ok(true)
    })?)?;

    g.set("doPlayerAddItem", lua.create_function(|_, _args: LuaMultiValue| -> LuaResult<bool> {
        Ok(true)
    })?)?;
    g.set("isValidUID", lua.create_function(|_, _uid: i32| -> LuaResult<bool> {
        Ok(false)
    })?)?;
    g.set("isDepot", lua.create_function(|_, _uid: i32| -> LuaResult<bool> {
        Ok(false)
    })?)?;
    g.set("isMovable", lua.create_function(|_, _uid: i32| -> LuaResult<bool> {
        Ok(false)
    })?)?;
    g.set("doAddContainerItem", lua.create_function(|_, _args: LuaMultiValue| -> LuaResult<bool> {
        Ok(true)
    })?)?;
    g.set("getDepotId", lua.create_function(|_, _uid: i32| -> LuaResult<i32> {
        Ok(0)
    })?)?;
    g.set("getSubTypeName", lua.create_function(|lua, sub_type: i32| -> LuaResult<LuaValue> {
        if sub_type > 0 {
            let game = g_game().lock().unwrap();
            let name = game.items.get_item_type(sub_type as usize).name.clone();
            drop(game);
            Ok(LuaValue::String(lua.create_string(&name)?))
        } else {
            Ok(LuaValue::Nil)
        }
    })?)?;

    g.set("createCombatArea", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<u32> {
        let mut iter = args.into_iter();
        let area_id = NEXT_AREA_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let offsets = match iter.next() {
            Some(LuaValue::Table(t)) => parse_area_table(&t),
            _ => vec![(0i8, 0i8)],
        };
        get_area_patterns().lock().unwrap().insert(area_id, offsets);
        Ok(area_id)
    })?)?;

    g.set("doAreaCombat", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<bool> {
        use crate::combat::combat::{Combat, CombatParams};
        use crate::combat::{CombatDamage, CombatOrigin, CombatType};
        use crate::util::normal_random;
        let mut iter = args.into_iter();
        let caster_id: Option<crate::creatures::CreatureId> = match iter.next() {
            Some(LuaValue::Integer(n)) if n != 0 => Some(n as u32),
            Some(LuaValue::Table(t)) => t.raw_get::<u32>(1).ok(),
            _ => None,
        };
        let combat_type_raw: u32 = match iter.next() {
            Some(LuaValue::Integer(n)) => n as u32,
            _ => return Ok(false),
        };
        let center_pos: crate::map::Position = match iter.next() {
            Some(LuaValue::Table(t)) => {
                let x: u16 = t.get("x").unwrap_or(0);
                let y: u16 = t.get("y").unwrap_or(0);
                let z: u8 = t.get("z").unwrap_or(0);
                crate::map::Position { x, y, z }
            }
            _ => return Ok(false),
        };
        let area_id: u32 = match iter.next() {
            Some(LuaValue::Integer(n)) => n as u32,
            _ => 0,
        };
        let min_val: i32 = match iter.next() {
            Some(LuaValue::Integer(n)) => n as i32,
            Some(LuaValue::Number(n)) => n as i32,
            _ => 0,
        };
        let max_val: i32 = match iter.next() {
            Some(LuaValue::Integer(n)) => n as i32,
            Some(LuaValue::Number(n)) => n as i32,
            _ => 0,
        };
        let impact_effect: u8 = match iter.next() {
            Some(LuaValue::Integer(n)) => n as u8,
            _ => 0,
        };
        let origin_raw: u8 = match iter.next() {
            Some(LuaValue::Integer(n)) => n as u8,
            _ => 2,
        };

        let offsets: Vec<(i8, i8)> = {
            let patterns = get_area_patterns().lock().unwrap();
            patterns.get(&area_id).cloned().unwrap_or_else(|| vec![(0i8, 0i8)])
        };

        let combat_type = CombatType::from_u16(combat_type_raw as u16);
        let origin = match origin_raw {
            1 => CombatOrigin::Condition, 2 => CombatOrigin::Spell,
            3 => CombatOrigin::Melee, 4 => CombatOrigin::Ranged, 5 => CombatOrigin::Wand,
            _ => CombatOrigin::None,
        };
        let params = CombatParams {
            combat_type,
            impact_effect,
            ..CombatParams::default()
        };

        let affected_positions: Vec<crate::map::Position> = offsets.iter().map(|(dr, dc)| {
            crate::map::Position {
                x: (center_pos.x as i32 + *dc as i32) as u16,
                y: (center_pos.y as i32 + *dr as i32) as u16,
                z: center_pos.z,
            }
        }).collect();

        let target_ids: Vec<crate::creatures::CreatureId> = {
            let game = g_game().lock().unwrap();
            let mut ids = Vec::new();
            for pos in &affected_positions {
                if let Some(tile) = game.map.get_tile(*pos) {
                    ids.extend(tile.creature_ids.iter().copied());
                }
            }
            ids
        };

        for target_id in target_ids {
            let mut damage = CombatDamage::new(combat_type);
            damage.primary_value = normal_random(min_val, max_val);
            damage.origin = origin;
            Combat::do_target_combat(caster_id, target_id, &mut damage, &params);
        }

        if impact_effect > 0 {
            for pos in &affected_positions {
                crate::net::game_protocol::broadcast_magic_effect(*pos, impact_effect);
            }
        }

        Ok(true)
    })?)?;
    g.set("doTargetCombat", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<bool> {
        use crate::combat::combat::{Combat, CombatParams};
        use crate::combat::{CombatDamage, CombatOrigin, CombatType};
        use crate::util::normal_random;
        let mut iter = args.into_iter();
        let caster_id: Option<crate::creatures::CreatureId> = match iter.next() {
            Some(LuaValue::Integer(n)) if n != 0 => Some(n as u32),
            Some(LuaValue::Table(t)) => t.raw_get::<u32>(1).ok(),
            _ => None,
        };
        let target_id: crate::creatures::CreatureId = match iter.next() {
            Some(LuaValue::Integer(n)) => n as u32,
            Some(LuaValue::Table(t)) => match t.raw_get::<u32>(1) {
                Ok(id) => id,
                _ => return Ok(false),
            },
            _ => return Ok(false),
        };
        let combat_type_raw: u32 = match iter.next() {
            Some(LuaValue::Integer(n)) => n as u32,
            _ => return Ok(false),
        };
        let min_val: i32 = match iter.next() {
            Some(LuaValue::Integer(n)) => n as i32,
            Some(LuaValue::Number(n)) => n as i32,
            _ => 0,
        };
        let max_val: i32 = match iter.next() {
            Some(LuaValue::Integer(n)) => n as i32,
            Some(LuaValue::Number(n)) => n as i32,
            _ => 0,
        };
        let impact_effect: u8 = match iter.next() {
            Some(LuaValue::Integer(n)) => n as u8,
            _ => 0,
        };
        let origin_raw: u8 = match iter.next() {
            Some(LuaValue::Integer(n)) => n as u8,
            _ => 2, // ORIGIN_SPELL
        };
        let blocked_by_armor: bool = match iter.next() {
            Some(LuaValue::Boolean(b)) => b,
            _ => false,
        };
        let blocked_by_shield: bool = match iter.next() {
            Some(LuaValue::Boolean(b)) => b,
            _ => false,
        };
        let ignore_resistances: bool = match iter.next() {
            Some(LuaValue::Boolean(b)) => b,
            _ => false,
        };

        let combat_type = CombatType::from_u16(combat_type_raw as u16);
        let origin = match origin_raw {
            1 => CombatOrigin::Condition,
            2 => CombatOrigin::Spell,
            3 => CombatOrigin::Melee,
            4 => CombatOrigin::Ranged,
            5 => CombatOrigin::Wand,
            _ => CombatOrigin::None,
        };
        let mut damage = CombatDamage::new(combat_type);
        damage.primary_value = normal_random(min_val, max_val);
        damage.origin = origin;

        let params = CombatParams {
            combat_type,
            impact_effect,
            blocked_by_armor,
            blocked_by_shield,
            ignore_resistances,
            ..CombatParams::default()
        };

        Combat::do_target_combat(caster_id, target_id, &mut damage, &params);
        Ok(true)
    })?)?;
    g.set("doChallengeCreature", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<bool> {
        let mut it = args.into_iter();
        let cid = match it.next() {
            Some(LuaValue::Integer(n)) => n as u32,
            Some(LuaValue::Table(t)) => t.raw_get::<u32>(1).unwrap_or(0),
            _ => return Ok(false),
        };
        let target_id = match it.next() {
            Some(LuaValue::Integer(n)) => n as u32,
            Some(LuaValue::Table(t)) => t.raw_get::<u32>(1).unwrap_or(0),
            _ => return Ok(false),
        };
        let force = matches!(it.next(), Some(LuaValue::Boolean(true)));
        let mut game = g_game().lock().unwrap();
        let is_summon = game.get_creature(cid).map(|c| c.base().master_id.is_some()).unwrap_or(true);
        if is_summon { return Ok(false); }
        let is_challengeable = game.get_creature(cid)
            .and_then(|c| c.as_monster())
            .map(|m| m.mtype_info.is_challengeable)
            .unwrap_or(false);
        if !is_challengeable && !force { return Ok(false); }
        let target_exists = game.get_creature(target_id).is_some();
        if !target_exists { return Ok(false); }
        if let Some(creature) = game.get_creature_mut(cid) {
            creature.base_mut().attacked_creature_id = Some(target_id);
            if let Some(m) = creature.as_monster_mut() {
                m.target_change_cooldown = 8000;
                m.challenge_focus_duration = 8000;
                m.target_change_ticks = 0;
            }
        }
        Ok(true)
    })?)?;
    g.set("getWaypointPositionByName", lua.create_function(|lua, _name: String| -> LuaResult<LuaValue> {
        Ok(LuaValue::Table(push_position(lua, Position { x: 0, y: 0, z: 0 })?))
    })?)?;
    g.set("sendChannelMessage", lua.create_function(|_, (channel_id, speak_type, message): (u16, u8, String)| -> LuaResult<bool> {
        use crate::net::game_protocol::broadcast_channel_message;
        broadcast_channel_message(channel_id, "", 0, speak_type, message.as_bytes());
        Ok(true)
    })?)?;
    g.set("sendGuildChannelMessage", lua.create_function(|_, (guild_id, speak_type, message): (u32, u8, String)| -> LuaResult<bool> {
        use crate::chat::g_chat;
        let channel_id = {
            let chat = g_chat().lock().unwrap();
            chat.get_guild_channel_by_id(guild_id).map(|c| c.id)
        };
        if let Some(cid) = channel_id {
            use crate::net::game_protocol::broadcast_channel_message;
            broadcast_channel_message(cid, "", 0, speak_type, message.as_bytes());
            Ok(true)
        } else {
            Ok(false)
        }
    })?)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// bit library (Lua 5.1 bitwise ops)
// ---------------------------------------------------------------------------

fn register_bit_library(lua: &Lua) -> LuaResult<()> {
    let bit = lua.create_table()?;
    bit.set("bnot", lua.create_function(|_, n: i32| Ok(!n))?)?;
    bit.set("band", lua.create_function(|_, (a, b): (i32, i32)| Ok(a & b))?)?;
    bit.set("bor",  lua.create_function(|_, (a, b): (i32, i32)| Ok(a | b))?)?;
    bit.set("bxor", lua.create_function(|_, (a, b): (i32, i32)| Ok(a ^ b))?)?;
    bit.set("lshift", lua.create_function(|_, (a, b): (i32, u32)| Ok(a << (b & 31)))?)?;
    bit.set("rshift", lua.create_function(|_, (a, b): (u32, u32)| {
        Ok((a >> (b & 31)) as i32)
    })?)?;
    bit.set("arshift", lua.create_function(|_, (a, b): (i32, u32)| Ok(a >> (b & 31)))?)?;
    bit.set("tobit",  lua.create_function(|_, n: i32| Ok(n))?)?;
    bit.set("tohex",  lua.create_function(|_, (n, d): (u32, LuaValue)| {
        let digits = match d { LuaValue::Integer(i) => i.unsigned_abs() as usize, _ => 8 };
        Ok(format!("{:0>width$x}", n, width = digits))
    })?)?;
    lua.globals().set("bit", bit)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// configManager table
// ---------------------------------------------------------------------------

fn register_config_manager_table(lua: &Lua) -> LuaResult<()> {
    let tbl = lua.create_table()?;
    tbl.set("getString", lua.create_function(|_, key: i32| -> LuaResult<String> {
        let cfg = g_config();
        let sc = match key {
            0 => StringConfig::IpString,
            1 => StringConfig::MapName,
            2 => StringConfig::HouseRentPeriod,
            3 => StringConfig::ServerName,
            4 => StringConfig::OwnerName,
            5 => StringConfig::OwnerEmail,
            6 => StringConfig::Url,
            7 => StringConfig::Location,
            8 => StringConfig::Motd,
            9 => StringConfig::WorldType,
            10 => StringConfig::MysqlHost,
            11 => StringConfig::MysqlUser,
            12 => StringConfig::MysqlPass,
            13 => StringConfig::MysqlDb,
            14 => StringConfig::MysqlSock,
            15 => StringConfig::DefaultPriority,
            16 => StringConfig::MapAuthor,
            _ => return Ok(String::new()),
        };
        Ok(cfg.get_string(sc).to_owned())
    })?)?;
    tbl.set("getNumber", lua.create_function(|_, key: i32| -> LuaResult<i32> {
        let cfg = g_config();
        let ic = match key {
            0 => IntegerConfig::Ip,
            1 => IntegerConfig::SqlPort,
            2 => IntegerConfig::MaxPlayers,
            3 => IntegerConfig::PzLocked,
            4 => IntegerConfig::DefaultDespawnRange,
            5 => IntegerConfig::DefaultDespawnRadius,
            6 => IntegerConfig::DefaultWalkToSpawnRadius,
            7 => IntegerConfig::RateExperience,
            8 => IntegerConfig::RateSkill,
            9 => IntegerConfig::RateLoot,
            10 => IntegerConfig::RateMagic,
            11 => IntegerConfig::RateSpawn,
            12 => IntegerConfig::HousePrice,
            13 => IntegerConfig::KillsToRed,
            14 => IntegerConfig::KillsToBlack,
            15 => IntegerConfig::MaxMessageBuffer,
            16 => IntegerConfig::ActionsDelayInterval,
            17 => IntegerConfig::ExActionsDelayInterval,
            18 => IntegerConfig::KickAfterMinutes,
            19 => IntegerConfig::ProtectionLevel,
            20 => IntegerConfig::DeathLosePercent,
            21 => IntegerConfig::StatusQueryTimeout,
            22 => IntegerConfig::FragTime,
            23 => IntegerConfig::WhiteSkullTime,
            24 => IntegerConfig::GamePort,
            25 => IntegerConfig::LoginPort,
            26 => IntegerConfig::StatusPort,
            27 => IntegerConfig::StairhopDelay,
            28 => IntegerConfig::ExpFromPlayersLevelRange,
            29 => IntegerConfig::MaxPacketsPerSecond,
            30 => IntegerConfig::ServerSaveNotifyDuration,
            31 => IntegerConfig::YellMinimumLevel,
            32 => IntegerConfig::MinimumLevelToSendPrivate,
            33 => IntegerConfig::VipFreeLimit,
            34 => IntegerConfig::VipPremiumLimit,
            35 => IntegerConfig::DepotFreeLimit,
            36 => IntegerConfig::DepotPremiumLimit,
            _ => return Ok(0),
        };
        Ok(cfg.get_number(ic))
    })?)?;
    tbl.set("getBoolean", lua.create_function(|_, key: i32| -> LuaResult<bool> {
        let cfg = g_config();
        let bc = match key {
            0 => BooleanConfig::AllowChangeOutfit,
            1 => BooleanConfig::OnePlayerOnAccount,
            2 => BooleanConfig::AimbotHotkeyEnabled,
            3 => BooleanConfig::RemoveRuneCharges,
            4 => BooleanConfig::RemoveWeaponAmmo,
            5 => BooleanConfig::RemoveWeaponCharges,
            6 => BooleanConfig::RemovePotionCharges,
            7 => BooleanConfig::PzLockSkullAttacker,
            8 => BooleanConfig::ExperienceFromPlayers,
            9 => BooleanConfig::FreePremium,
            10 => BooleanConfig::ReplaceKickOnLogin,
            11 => BooleanConfig::AllowClones,
            12 => BooleanConfig::AllowWalkthrough,
            13 => BooleanConfig::BindOnlyGlobalAddress,
            14 => BooleanConfig::OptimizeDatabase,
            15 => BooleanConfig::EmoteSpells,
            16 => BooleanConfig::StaminaSystem,
            17 => BooleanConfig::WarnUnsafeScripts,
            18 => BooleanConfig::ConvertUnsafeScripts,
            19 => BooleanConfig::ClassicEquipmentSlots,
            20 => BooleanConfig::ClassicAttackSpeed,
            21 => BooleanConfig::ScriptsConsoleLogs,
            22 => BooleanConfig::ServerSaveNotifyMessage,
            23 => BooleanConfig::ServerSaveCleanMap,
            24 => BooleanConfig::ServerSaveClose,
            25 => BooleanConfig::ServerSaveShutdown,
            26 => BooleanConfig::OnlineOfflineCharlist,
            27 => BooleanConfig::YellAllowPremium,
            28 => BooleanConfig::PremiumToSendPrivate,
            29 => BooleanConfig::ForceMonsterTypeLoad,
            30 => BooleanConfig::DefaultWorldLight,
            31 => BooleanConfig::HouseOwnedByAccount,
            32 => BooleanConfig::LuaItemDesc,
            33 => BooleanConfig::CleanProtectionZones,
            34 => BooleanConfig::HouseDoorShowPrice,
            35 => BooleanConfig::OnlyInvitedCanMoveHouseItems,
            36 => BooleanConfig::RemoveOnDespawn,
            37 => BooleanConfig::PlayerConsoleLogs,
            _ => return Ok(false),
        };
        Ok(cfg.get_boolean(bc))
    })?)?;
    lua.globals().set("configManager", tbl)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// db table
// ---------------------------------------------------------------------------

fn register_db_table(lua: &Lua) -> LuaResult<()> {
    let tbl = lua.create_table()?;

    tbl.set("query", lua.create_function(|_, query: String| -> LuaResult<bool> {
        let db = g_database();
        match db_blocking(db.execute(&query)) {
            Ok(v) => Ok(v),
            Err(e) => {
                tracing::error!("[db.query] {}", e);
                Ok(false)
            }
        }
    })?)?;

    tbl.set("asyncQuery", lua.create_function(|_, query: String| -> LuaResult<bool> {
        let db = g_database();
        match db_blocking(db.execute(&query)) {
            Ok(v) => Ok(v),
            Err(e) => {
                tracing::error!("[db.asyncQuery] {}", e);
                Ok(false)
            }
        }
    })?)?;

    tbl.set("storeQuery", lua.create_function(|_lua, query: String| -> LuaResult<LuaValue> {
        let db = g_database();
        match db_blocking(db.store_query(&query)) {
            Ok(Some(result)) => {
                let id = DB_RESULT_ID.fetch_add(1, Ordering::Relaxed);
                db_results().lock().unwrap().insert(id, result);
                Ok(LuaValue::Integer(id as i64))
            }
            Ok(None) => Ok(LuaValue::Boolean(false)),
            Err(e) => {
                tracing::error!("[db.storeQuery] {}", e);
                Ok(LuaValue::Boolean(false))
            }
        }
    })?)?;

    tbl.set("asyncStoreQuery", lua.create_function(|_lua, query: String| -> LuaResult<LuaValue> {
        let db = g_database();
        match db_blocking(db.store_query(&query)) {
            Ok(Some(result)) => {
                let id = DB_RESULT_ID.fetch_add(1, Ordering::Relaxed);
                db_results().lock().unwrap().insert(id, result);
                Ok(LuaValue::Integer(id as i64))
            }
            Ok(None) => Ok(LuaValue::Boolean(false)),
            Err(e) => {
                tracing::error!("[db.asyncStoreQuery] {}", e);
                Ok(LuaValue::Boolean(false))
            }
        }
    })?)?;

    tbl.set("escapeString", lua.create_function(|_, s: String| -> LuaResult<String> {
        let db = g_database();
        Ok(db.escape_string(&s))
    })?)?;

    tbl.set("escapeBlob", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<String> {
        let db = g_database();
        let s = args.iter().next()
            .and_then(|v| match v { LuaValue::String(s) => Some(s.as_bytes().to_vec()), _ => None })
            .unwrap_or_default();
        Ok(db.escape_blob(&s))
    })?)?;

    tbl.set("lastInsertId", lua.create_function(|_, ()| -> LuaResult<u64> {
        Ok(u64::from(LAST_INSERT_ID.load(Ordering::Relaxed)))
    })?)?;

    tbl.set("tableExists", lua.create_function(|_, name: String| -> LuaResult<bool> {
        let db = g_database();
        let query = format!(
            "SELECT `TABLE_NAME` FROM `information_schema`.`tables` WHERE `TABLE_SCHEMA` = DATABASE() AND `TABLE_NAME` = {}",
            db.escape_string(&name)
        );
        match db_blocking(db.store_query(&query)) {
            Ok(Some(_)) => Ok(true),
            _ => Ok(false),
        }
    })?)?;

    lua.globals().set("db", tbl)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// result table
// ---------------------------------------------------------------------------

fn register_result_table(lua: &Lua) -> LuaResult<()> {
    let tbl = lua.create_table()?;

    tbl.set("getNumber", lua.create_function(|_, (id, col): (u32, String)| -> LuaResult<i64> {
        let results = db_results().lock().unwrap();
        let value = results.get(&id)
            .and_then(|r| r.get_i64(&col))
            .unwrap_or(0);
        Ok(value)
    })?)?;

    tbl.set("getString", lua.create_function(|_, (id, col): (u32, String)| -> LuaResult<String> {
        let results = db_results().lock().unwrap();
        let value = results.get(&id)
            .and_then(|r| r.get_string(&col))
            .unwrap_or_default();
        Ok(value)
    })?)?;

    tbl.set("getStream", lua.create_function(|_, (id, col): (u32, String)| -> LuaResult<String> {
        let results = db_results().lock().unwrap();
        let value = results.get(&id)
            .and_then(|r| r.get_string(&col))
            .unwrap_or_default();
        Ok(value)
    })?)?;

    tbl.set("next", lua.create_function(|_, id: u32| -> LuaResult<bool> {
        let mut results = db_results().lock().unwrap();
        let advanced = results.get_mut(&id)
            .map(|r| r.next())
            .unwrap_or(false);
        Ok(advanced)
    })?)?;

    tbl.set("free", lua.create_function(|_, id: u32| -> LuaResult<bool> {
        let mut results = db_results().lock().unwrap();
        results.remove(&id);
        Ok(true)
    })?)?;

    lua.globals().set("result", tbl)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Enum constants
// ---------------------------------------------------------------------------

fn register_enums(lua: &Lua) -> LuaResult<()> {
    let g = lua.globals();
    macro_rules! e {
        ($name:expr, $val:expr) => { g.set($name, $val as i64)?; };
    }

    // ACCOUNT_TYPE
    e!("ACCOUNT_TYPE_NORMAL", 1); e!("ACCOUNT_TYPE_TUTOR", 2);
    e!("ACCOUNT_TYPE_SENIORTUTOR", 3); e!("ACCOUNT_TYPE_GAMEMASTER", 4);
    e!("ACCOUNT_TYPE_COMMUNITYMANAGER", 5); e!("ACCOUNT_TYPE_GOD", 6);

    // AMMO
    e!("AMMO_NONE", 0); e!("AMMO_BOLT", 1); e!("AMMO_ARROW", 2);
    e!("AMMO_SPEAR", 3); e!("AMMO_THROWINGSTAR", 4); e!("AMMO_THROWINGKNIFE", 5);
    e!("AMMO_STONE", 6); e!("AMMO_SNOWBALL", 7);

    // BUG_CATEGORY
    e!("BUG_CATEGORY_MAP", 0); e!("BUG_CATEGORY_TYPO", 1);
    e!("BUG_CATEGORY_TECHNICAL", 2); e!("BUG_CATEGORY_OTHER", 3);

    // CALLBACK_PARAM
    e!("CALLBACK_PARAM_LEVELMAGICVALUE", 0); e!("CALLBACK_PARAM_SKILLVALUE", 1);
    e!("CALLBACK_PARAM_TARGETTILE", 2); e!("CALLBACK_PARAM_TARGETCREATURE", 3);

    // COMBAT_FORMULA
    e!("COMBAT_FORMULA_UNDEFINED", 0); e!("COMBAT_FORMULA_LEVELMAGIC", 1);
    e!("COMBAT_FORMULA_SKILL", 2); e!("COMBAT_FORMULA_DAMAGE", 3);

    // DIRECTION
    e!("DIRECTION_NORTH", 0); e!("DIRECTION_EAST", 1);
    e!("DIRECTION_SOUTH", 2); e!("DIRECTION_WEST", 3);
    e!("DIRECTION_DIAGONAL_MASK", 4);
    e!("DIRECTION_SOUTHWEST", 4); e!("DIRECTION_SOUTHEAST", 5);
    e!("DIRECTION_NORTHWEST", 6); e!("DIRECTION_NORTHEAST", 7);
    e!("DIRECTION_LAST", 7); e!("DIRECTION_NONE", 8);

    // COMBAT damage types
    e!("COMBAT_NONE", 0); e!("COMBAT_PHYSICALDAMAGE", 1);
    e!("COMBAT_ENERGYDAMAGE", 2); e!("COMBAT_EARTHDAMAGE", 4);
    e!("COMBAT_FIREDAMAGE", 8); e!("COMBAT_UNDEFINEDDAMAGE", 16);
    e!("COMBAT_LIFEDRAIN", 32); e!("COMBAT_MANADRAIN", 64);
    e!("COMBAT_HEALING", 128); e!("COMBAT_DROWNDAMAGE", 256);
    e!("COMBAT_ICEDAMAGE", 512); e!("COMBAT_HOLYDAMAGE", 1024);
    e!("COMBAT_DEATHDAMAGE", 2048); e!("COMBAT_COUNT", 12);

    // COMBAT_PARAM
    e!("COMBAT_PARAM_TYPE", 1); e!("COMBAT_PARAM_EFFECT", 2);
    e!("COMBAT_PARAM_DISTANCEEFFECT", 3); e!("COMBAT_PARAM_BLOCKSHIELD", 4);
    e!("COMBAT_PARAM_BLOCKARMOR", 5); e!("COMBAT_PARAM_TARGETCASTERORTOPMOST", 6);
    e!("COMBAT_PARAM_CREATEITEM", 7); e!("COMBAT_PARAM_AGGRESSIVE", 8);
    e!("COMBAT_PARAM_DISPEL", 9); e!("COMBAT_PARAM_USECHARGES", 10);

    // CONDITION types
    e!("CONDITION_NONE", 0); e!("CONDITION_POISON", 1);
    e!("CONDITION_FIRE", 2); e!("CONDITION_ENERGY", 4);
    e!("CONDITION_BLEEDING", 8); e!("CONDITION_HASTE", 16);
    e!("CONDITION_PARALYZE", 32); e!("CONDITION_OUTFIT", 64);
    e!("CONDITION_INVISIBLE", 128); e!("CONDITION_LIGHT", 256);
    e!("CONDITION_MANASHIELD", 512); e!("CONDITION_INFIGHT", 1024);
    e!("CONDITION_DRUNK", 2048); e!("CONDITION_EXHAUST_WEAPON", 4096);
    e!("CONDITION_REGENERATION", 8192); e!("CONDITION_SOUL", 16384);
    e!("CONDITION_DROWN", 32768); e!("CONDITION_MUTED", 65536);
    e!("CONDITION_CHANNELMUTEDTICKS", 131072); e!("CONDITION_YELLTICKS", 262144);
    e!("CONDITION_ATTRIBUTES", 524288); e!("CONDITION_FREEZING", 1048576);
    e!("CONDITION_DAZZLED", 2097152); e!("CONDITION_CURSED", 4194304);
    e!("CONDITION_EXHAUST_COMBAT", 8388608); e!("CONDITION_EXHAUST_HEAL", 16777216);
    e!("CONDITION_PACIFIED", 33554432);

    // CONDITIONID
    e!("CONDITIONID_DEFAULT", -1); e!("CONDITIONID_COMBAT", 0);
    e!("CONDITIONID_HEAD", 1); e!("CONDITIONID_NECKLACE", 2);
    e!("CONDITIONID_BACKPACK", 3); e!("CONDITIONID_ARMOR", 4);
    e!("CONDITIONID_RIGHT", 5); e!("CONDITIONID_LEFT", 6);
    e!("CONDITIONID_LEGS", 7); e!("CONDITIONID_FEET", 8);
    e!("CONDITIONID_RING", 9); e!("CONDITIONID_AMMO", 10);

    // CONDITION_PARAM
    e!("CONDITION_PARAM_OWNER", 1); e!("CONDITION_PARAM_TICKS", 2);
    e!("CONDITION_PARAM_HEALTHGAIN", 4); e!("CONDITION_PARAM_HEALTHTICKS", 5);
    e!("CONDITION_PARAM_MANAGAIN", 6); e!("CONDITION_PARAM_MANATICKS", 7);
    e!("CONDITION_PARAM_DELAYED", 8); e!("CONDITION_PARAM_SPEED", 9);
    e!("CONDITION_PARAM_LIGHT_LEVEL", 10); e!("CONDITION_PARAM_LIGHT_COLOR", 11);
    e!("CONDITION_PARAM_SOULGAIN", 12); e!("CONDITION_PARAM_SOULTICKS", 13);
    e!("CONDITION_PARAM_MINVALUE", 14); e!("CONDITION_PARAM_MAXVALUE", 15);
    e!("CONDITION_PARAM_STARTVALUE", 16); e!("CONDITION_PARAM_TICKINTERVAL", 17);
    e!("CONDITION_PARAM_FORCEUPDATE", 18);
    e!("CONDITION_PARAM_SKILL_MELEE", 19); e!("CONDITION_PARAM_SKILL_FIST", 20);
    e!("CONDITION_PARAM_SKILL_CLUB", 21); e!("CONDITION_PARAM_SKILL_SWORD", 22);
    e!("CONDITION_PARAM_SKILL_AXE", 23); e!("CONDITION_PARAM_SKILL_DISTANCE", 24);
    e!("CONDITION_PARAM_SKILL_SHIELD", 25); e!("CONDITION_PARAM_SKILL_FISHING", 26);
    e!("CONDITION_PARAM_STAT_MAXHITPOINTS", 27);
    e!("CONDITION_PARAM_STAT_MAXMANAPOINTS", 28);
    e!("CONDITION_PARAM_STAT_MAGICPOINTS", 30);
    e!("CONDITION_PARAM_STAT_MAXHITPOINTSPERCENT", 31);
    e!("CONDITION_PARAM_STAT_MAXMANAPOINTSPERCENT", 32);
    e!("CONDITION_PARAM_STAT_MAGICPOINTSPERCENT", 34);
    e!("CONDITION_PARAM_PERIODICDAMAGE", 35);
    e!("CONDITION_PARAM_SKILL_MELEEPERCENT", 36);
    e!("CONDITION_PARAM_SKILL_FISTPERCENT", 37);
    e!("CONDITION_PARAM_SKILL_CLUBPERCENT", 38);
    e!("CONDITION_PARAM_SKILL_SWORDPERCENT", 39);
    e!("CONDITION_PARAM_SKILL_AXEPERCENT", 40);
    e!("CONDITION_PARAM_SKILL_DISTANCEPERCENT", 41);
    e!("CONDITION_PARAM_SKILL_SHIELDPERCENT", 42);
    e!("CONDITION_PARAM_SKILL_FISHINGPERCENT", 43);
    e!("CONDITION_PARAM_BUFF_SPELL", 44);
    e!("CONDITION_PARAM_SUBID", 45);
    e!("CONDITION_PARAM_FIELD", 46);
    e!("CONDITION_PARAM_DISABLE_DEFENSE", 47);
    e!("CONDITION_PARAM_SPECIALSKILL_CRITICALHITCHANCE", 48);
    e!("CONDITION_PARAM_SPECIALSKILL_CRITICALHITAMOUNT", 49);
    e!("CONDITION_PARAM_SPECIALSKILL_LIFELEECHCHANCE", 50);
    e!("CONDITION_PARAM_SPECIALSKILL_LIFELEECHAMOUNT", 51);
    e!("CONDITION_PARAM_SPECIALSKILL_MANALEECHCHANCE", 52);
    e!("CONDITION_PARAM_SPECIALSKILL_MANALEECHAMOUNT", 53);
    e!("CONDITION_PARAM_AGGRESSIVE", 54);
    e!("CONDITION_PARAM_DRUNKENNESS", 55);

    // CONST_ME (magic effects) 0-70
    e!("CONST_ME_NONE", 0); e!("CONST_ME_DRAWBLOOD", 1);
    e!("CONST_ME_LOSEENERGY", 2); e!("CONST_ME_POFF", 3);
    e!("CONST_ME_BLOCKHIT", 4); e!("CONST_ME_EXPLOSIONAREA", 5);
    e!("CONST_ME_EXPLOSIONHIT", 6); e!("CONST_ME_FIREAREA", 7);
    e!("CONST_ME_YELLOW_RINGS", 8); e!("CONST_ME_GREEN_RINGS", 9);
    e!("CONST_ME_HITAREA", 10); e!("CONST_ME_TELEPORT", 11);
    e!("CONST_ME_ENERGYHIT", 12); e!("CONST_ME_MAGIC_BLUE", 13);
    e!("CONST_ME_MAGIC_RED", 14); e!("CONST_ME_MAGIC_GREEN", 15);
    e!("CONST_ME_HITBYFIRE", 16); e!("CONST_ME_HITBYPOISON", 17);
    e!("CONST_ME_MORTAREA", 18); e!("CONST_ME_SOUND_GREEN", 19);
    e!("CONST_ME_SOUND_RED", 20); e!("CONST_ME_POISONAREA", 21);
    e!("CONST_ME_SOUND_YELLOW", 22); e!("CONST_ME_SOUND_PURPLE", 23);
    e!("CONST_ME_SOUND_BLUE", 24); e!("CONST_ME_SOUND_WHITE", 25);
    e!("CONST_ME_BUBBLES", 26); e!("CONST_ME_CRAPS", 27);
    e!("CONST_ME_GIFT_WRAPS", 28); e!("CONST_ME_FIREWORK_YELLOW", 29);
    e!("CONST_ME_FIREWORK_RED", 30); e!("CONST_ME_FIREWORK_BLUE", 31);
    e!("CONST_ME_STUN", 32); e!("CONST_ME_SLEEP", 33);
    e!("CONST_ME_WATERCREATURE", 34); e!("CONST_ME_GROUNDSHAKER", 35);
    e!("CONST_ME_HEARTS", 36); e!("CONST_ME_FIREATTACK", 37);
    e!("CONST_ME_ENERGYAREA", 38); e!("CONST_ME_SMALLCLOUDS", 39);
    e!("CONST_ME_HOLYDAMAGE", 40); e!("CONST_ME_BIGCLOUDS", 41);
    e!("CONST_ME_ICEAREA", 42); e!("CONST_ME_ICETORNADO", 43);
    e!("CONST_ME_ICEATTACK", 44); e!("CONST_ME_STONES", 45);
    e!("CONST_ME_SMALLPLANTS", 46); e!("CONST_ME_CARNIPHILA", 47);
    e!("CONST_ME_PURPLEENERGY", 48); e!("CONST_ME_YELLOWENERGY", 49);
    e!("CONST_ME_HOLYAREA", 50); e!("CONST_ME_BIGPLANTS", 51);
    e!("CONST_ME_CAKE", 52); e!("CONST_ME_GIANTICE", 53);
    e!("CONST_ME_WATERSPLASH", 54); e!("CONST_ME_PLANTATTACK", 55);
    e!("CONST_ME_TUTORIALARROW", 56); e!("CONST_ME_TUTORIALSQUARE", 57);
    e!("CONST_ME_MIRRORHORIZONTAL", 58); e!("CONST_ME_MIRRORVERTICAL", 59);
    e!("CONST_ME_SKULLHORIZONTAL", 60); e!("CONST_ME_SKULLVERTICAL", 61);
    e!("CONST_ME_ASSASSIN", 62); e!("CONST_ME_STEPSHORIZONTAL", 63);
    e!("CONST_ME_BLOODYSTEPS", 64); e!("CONST_ME_STEPSVERTICAL", 65);
    e!("CONST_ME_YALAHARIGHOST", 66); e!("CONST_ME_BATS", 67);
    e!("CONST_ME_SMOKE", 68); e!("CONST_ME_INSECTS", 69);
    e!("CONST_ME_DRAGONHEAD", 70);

    // CONST_ANI (projectiles) 0-41, 254
    e!("CONST_ANI_NONE", 0); e!("CONST_ANI_SPEAR", 1);
    e!("CONST_ANI_BOLT", 2); e!("CONST_ANI_ARROW", 3);
    e!("CONST_ANI_FIRE", 4); e!("CONST_ANI_ENERGY", 5);
    e!("CONST_ANI_POISONARROW", 6); e!("CONST_ANI_BURSTARROW", 7);
    e!("CONST_ANI_THROWINGSTAR", 8); e!("CONST_ANI_THROWINGKNIFE", 9);
    e!("CONST_ANI_SMALLSTONE", 10); e!("CONST_ANI_DEATH", 11);
    e!("CONST_ANI_LARGEROCK", 12); e!("CONST_ANI_SNOWBALL", 13);
    e!("CONST_ANI_POWERBOLT", 14); e!("CONST_ANI_POISON", 15);
    e!("CONST_ANI_INFERNALBOLT", 16); e!("CONST_ANI_HUNTINGSPEAR", 17);
    e!("CONST_ANI_ENCHANTEDSPEAR", 18); e!("CONST_ANI_REDSTAR", 19);
    e!("CONST_ANI_GREENSTAR", 20); e!("CONST_ANI_ROYALSPEAR", 21);
    e!("CONST_ANI_SNIPERARROW", 22); e!("CONST_ANI_ONYXARROW", 23);
    e!("CONST_ANI_PIERCINGBOLT", 24); e!("CONST_ANI_WHIRLWINDSWORD", 25);
    e!("CONST_ANI_WHIRLWINDAXE", 26); e!("CONST_ANI_WHIRLWINDCLUB", 27);
    e!("CONST_ANI_ETHEREALSPEAR", 28); e!("CONST_ANI_ICE", 29);
    e!("CONST_ANI_EARTH", 30); e!("CONST_ANI_HOLY", 31);
    e!("CONST_ANI_SUDDENDEATH", 32); e!("CONST_ANI_FLASHARROW", 33);
    e!("CONST_ANI_FLAMMINGARROW", 34); e!("CONST_ANI_SHIVERARROW", 35);
    e!("CONST_ANI_ENERGYBALL", 36); e!("CONST_ANI_SMALLICE", 37);
    e!("CONST_ANI_SMALLHOLY", 38); e!("CONST_ANI_SMALLEARTH", 39);
    e!("CONST_ANI_EARTHARROW", 40); e!("CONST_ANI_EXPLOSION", 41);
    e!("CONST_ANI_WEAPONTYPE", 254);

    // CONST_PROP
    e!("CONST_PROP_BLOCKSOLID", 0); e!("CONST_PROP_HASHEIGHT", 1);
    e!("CONST_PROP_BLOCKPROJECTILE", 2); e!("CONST_PROP_BLOCKPATH", 3);
    e!("CONST_PROP_ISVERTICAL", 4); e!("CONST_PROP_ISHORIZONTAL", 5);
    e!("CONST_PROP_MOVEABLE", 6); e!("CONST_PROP_IMMOVABLEBLOCKSOLID", 7);
    e!("CONST_PROP_IMMOVABLEBLOCKPATH", 8);
    e!("CONST_PROP_IMMOVABLENOFIELDBLOCKPATH", 9);
    e!("CONST_PROP_NOFIELDBLOCKPATH", 10); e!("CONST_PROP_SUPPORTHANGABLE", 11);

    // CONST_SLOT
    e!("CONST_SLOT_WHEREEVER", 0); e!("CONST_SLOT_HEAD", 1);
    e!("CONST_SLOT_NECKLACE", 2); e!("CONST_SLOT_BACKPACK", 3);
    e!("CONST_SLOT_ARMOR", 4); e!("CONST_SLOT_RIGHT", 5);
    e!("CONST_SLOT_LEFT", 6); e!("CONST_SLOT_LEGS", 7);
    e!("CONST_SLOT_FEET", 8); e!("CONST_SLOT_RING", 9);
    e!("CONST_SLOT_AMMO", 10);
    e!("CONST_SLOT_FIRST", 1); e!("CONST_SLOT_LAST", 10);

    // CREATURE_EVENT
    e!("CREATURE_EVENT_NONE", 0); e!("CREATURE_EVENT_LOGIN", 1);
    e!("CREATURE_EVENT_LOGOUT", 2); e!("CREATURE_EVENT_THINK", 3);
    e!("CREATURE_EVENT_PREPAREDEATH", 4); e!("CREATURE_EVENT_DEATH", 5);
    e!("CREATURE_EVENT_KILL", 6); e!("CREATURE_EVENT_ADVANCE", 7);
    e!("CREATURE_EVENT_TEXTEDIT", 8); e!("CREATURE_EVENT_HEALTHCHANGE", 9);
    e!("CREATURE_EVENT_MANACHANGE", 10); e!("CREATURE_EVENT_EXTENDED_OPCODE", 11);

    // GAME_STATE
    e!("GAME_STATE_STARTUP", 0); e!("GAME_STATE_INIT", 1);
    e!("GAME_STATE_NORMAL", 2); e!("GAME_STATE_CLOSED", 3);
    e!("GAME_STATE_SHUTDOWN", 4); e!("GAME_STATE_CLOSING", 5);
    e!("GAME_STATE_MAINTAIN", 6);

    // MESSAGE
    e!("MESSAGE_STATUS_CONSOLE_RED", 18); e!("MESSAGE_EVENT_ORANGE", 19);
    e!("MESSAGE_STATUS_CONSOLE_ORANGE", 20); e!("MESSAGE_STATUS_WARNING", 21);
    e!("MESSAGE_EVENT_ADVANCE", 22); e!("MESSAGE_EVENT_DEFAULT", 23);
    e!("MESSAGE_STATUS_DEFAULT", 24); e!("MESSAGE_INFO_DESCR", 25);
    e!("MESSAGE_STATUS_SMALL", 26); e!("MESSAGE_STATUS_CONSOLE_BLUE", 27);

    // CREATURETYPE
    e!("CREATURETYPE_PLAYER", 0); e!("CREATURETYPE_MONSTER", 1);
    e!("CREATURETYPE_NPC", 2); e!("CREATURETYPE_SUMMON_OWN", 3);
    e!("CREATURETYPE_SUMMON_OTHERS", 4);

    // CLIENTOS
    e!("CLIENTOS_NONE", 0); e!("CLIENTOS_LINUX", 1);
    e!("CLIENTOS_WINDOWS", 2); e!("CLIENTOS_FLASH", 3);
    e!("CLIENTOS_OTCLIENT_LINUX", 10); e!("CLIENTOS_OTCLIENT_WINDOWS", 11);
    e!("CLIENTOS_OTCLIENT_MAC", 12);

    // FIGHTMODE
    e!("FIGHTMODE_ATTACK", 1); e!("FIGHTMODE_BALANCED", 2); e!("FIGHTMODE_DEFENSE", 3);

    // ITEM_ATTRIBUTE
    e!("ITEM_ATTRIBUTE_NONE", 0); e!("ITEM_ATTRIBUTE_ACTIONID", 1);
    e!("ITEM_ATTRIBUTE_UNIQUEID", 2); e!("ITEM_ATTRIBUTE_DESCRIPTION", 4);
    e!("ITEM_ATTRIBUTE_TEXT", 8); e!("ITEM_ATTRIBUTE_DATE", 16);
    e!("ITEM_ATTRIBUTE_WRITER", 32); e!("ITEM_ATTRIBUTE_NAME", 64);
    e!("ITEM_ATTRIBUTE_ARTICLE", 128); e!("ITEM_ATTRIBUTE_PLURALNAME", 256);
    e!("ITEM_ATTRIBUTE_WEIGHT", 512); e!("ITEM_ATTRIBUTE_ATTACK", 1024);
    e!("ITEM_ATTRIBUTE_DEFENSE", 2048); e!("ITEM_ATTRIBUTE_EXTRADEFENSE", 4096);
    e!("ITEM_ATTRIBUTE_ARMOR", 8192); e!("ITEM_ATTRIBUTE_HITCHANCE", 16384);
    e!("ITEM_ATTRIBUTE_SHOOTRANGE", 32768); e!("ITEM_ATTRIBUTE_OWNER", 65536);
    e!("ITEM_ATTRIBUTE_DURATION", 131072); e!("ITEM_ATTRIBUTE_DECAYSTATE", 262144);
    e!("ITEM_ATTRIBUTE_CORPSEOWNER", 524288); e!("ITEM_ATTRIBUTE_CHARGES", 1048576);
    e!("ITEM_ATTRIBUTE_FLUIDTYPE", 2097152); e!("ITEM_ATTRIBUTE_DOORID", 4194304);
    e!("ITEM_ATTRIBUTE_DECAYTO", 8388608); e!("ITEM_ATTRIBUTE_WRAPID", 16777216);
    e!("ITEM_ATTRIBUTE_STOREITEM", 33554432); e!("ITEM_ATTRIBUTE_ATTACK_SPEED", 67108864);
    e!("ITEM_ATTRIBUTE_CUSTOM", 2147483648u32 as i64);

    // ITEM_TYPE
    e!("ITEM_TYPE_NONE", 0); e!("ITEM_TYPE_DEPOT", 1);
    e!("ITEM_TYPE_MAILBOX", 2); e!("ITEM_TYPE_TRASHHOLDER", 3);
    e!("ITEM_TYPE_CONTAINER", 4); e!("ITEM_TYPE_DOOR", 5);
    e!("ITEM_TYPE_MAGICFIELD", 6); e!("ITEM_TYPE_TELEPORT", 7);
    e!("ITEM_TYPE_BED", 8); e!("ITEM_TYPE_KEY", 9);
    e!("ITEM_TYPE_RUNE", 10); e!("ITEM_TYPE_LAST", 11);

    // ITEM_GROUP
    e!("ITEM_GROUP_NONE", 0); e!("ITEM_GROUP_GROUND", 1);
    e!("ITEM_GROUP_CONTAINER", 2); e!("ITEM_GROUP_WEAPON", 3);
    e!("ITEM_GROUP_AMMUNITION", 4); e!("ITEM_GROUP_ARMOR", 5);
    e!("ITEM_GROUP_CHARGES", 6); e!("ITEM_GROUP_TELEPORT", 7);
    e!("ITEM_GROUP_MAGICFIELD", 8); e!("ITEM_GROUP_WRITEABLE", 9);
    e!("ITEM_GROUP_KEY", 10); e!("ITEM_GROUP_SPLASH", 11);
    e!("ITEM_GROUP_FLUID", 12); e!("ITEM_GROUP_DOOR", 13);
    e!("ITEM_GROUP_DEPRECATED", 14); e!("ITEM_GROUP_LAST", 15);

    // Specific ITEM IDs
    e!("ITEM_FIREFIELD_PVP_FULL", 1487); e!("ITEM_FIREFIELD_PVP_MEDIUM", 1488);
    e!("ITEM_FIREFIELD_PVP_SMALL", 1489); e!("ITEM_POISONFIELD_PVP", 1490);
    e!("ITEM_ENERGYFIELD_PVP", 1491);
    e!("ITEM_FIREFIELD_PERSISTENT_FULL", 1492); e!("ITEM_FIREFIELD_PERSISTENT_MEDIUM", 1493);
    e!("ITEM_FIREFIELD_PERSISTENT_SMALL", 1494);
    e!("ITEM_ENERGYFIELD_PERSISTENT", 1495); e!("ITEM_POISONFIELD_PERSISTENT", 1496);
    e!("ITEM_MAGICWALL", 1497); e!("ITEM_MAGICWALL_PERSISTENT", 1498);
    e!("ITEM_WILDGROWTH", 1499); e!("ITEM_FIREFIELD_NOPVP", 1500);
    e!("ITEM_POISONFIELD_NOPVP", 1503); e!("ITEM_ENERGYFIELD_NOPVP", 1504);
    e!("ITEM_BAG", 1987); e!("ITEM_BACKPACK", 1988);
    e!("ITEM_GOLD_COIN", 2148); e!("ITEM_PLATINUM_COIN", 2152);
    e!("ITEM_CRYSTAL_COIN", 2160); e!("ITEM_LOCKER", 2589);
    e!("ITEM_DEPOT", 2594); e!("ITEM_PARCEL", 2595);
    e!("ITEM_LETTER", 2597); e!("ITEM_LETTER_STAMPED", 2598);
    e!("ITEM_LABEL", 2599); e!("ITEM_WILDGROWTH_PERSISTENT", 2721);
    e!("ITEM_MALE_CORPSE", 3058); e!("ITEM_FEMALE_CORPSE", 3065);
    e!("ITEM_FULLSPLASH", 2016); e!("ITEM_SMALLSPLASH", 2019);
    e!("ITEM_AMULETOFLOSS", 2173); e!("ITEM_DOCUMENT_RO", 1968);
    e!("ITEM_MAGICWALL_SAFE", 11098); e!("ITEM_WILDGROWTH_SAFE", 11099);

    // WIELDINFO
    e!("WIELDINFO_NONE", 0); e!("WIELDINFO_LEVEL", 1);
    e!("WIELDINFO_MAGLV", 2); e!("WIELDINFO_VOCREQ", 4);
    e!("WIELDINFO_PREMIUM", 8);

    // PlayerFlag (u64 bitflags, stored as i64)
    e!("PlayerFlag_CannotUseCombat", 1i64);
    e!("PlayerFlag_CannotAttackPlayer", 2i64);
    e!("PlayerFlag_CannotAttackMonster", 4i64);
    e!("PlayerFlag_CannotBeAttacked", 8i64);
    e!("PlayerFlag_CanConvinceAll", 16i64);
    e!("PlayerFlag_CanSummonAll", 32i64);
    e!("PlayerFlag_CanIllusionAll", 64i64);
    e!("PlayerFlag_CanSenseInvisibility", 128i64);
    e!("PlayerFlag_IgnoredByMonsters", 256i64);
    e!("PlayerFlag_NotGainInFight", 512i64);
    e!("PlayerFlag_HasInfiniteMana", 1024i64);
    e!("PlayerFlag_HasInfiniteSoul", 2048i64);
    e!("PlayerFlag_HasNoExhaustion", 4096i64);
    e!("PlayerFlag_CannotUseSpells", 8192i64);
    e!("PlayerFlag_CannotPickupItem", 16384i64);
    e!("PlayerFlag_CanAlwaysLogin", 32768i64);
    e!("PlayerFlag_CanBroadcast", 65536i64);
    e!("PlayerFlag_CanEditHouses", 131072i64);
    e!("PlayerFlag_CannotBeBanned", 262144i64);
    e!("PlayerFlag_CannotBePushed", 524288i64);
    e!("PlayerFlag_HasInfiniteCapacity", 1048576i64);
    e!("PlayerFlag_CanPushAllCreatures", 2097152i64);
    e!("PlayerFlag_CanTalkRedPrivate", 4194304i64);
    e!("PlayerFlag_CanTalkRedChannel", 8388608i64);
    e!("PlayerFlag_TalkOrangeHelpChannel", 16777216i64);
    e!("PlayerFlag_NotGainExperience", 33554432i64);
    e!("PlayerFlag_NotGainMana", 67108864i64);
    e!("PlayerFlag_NotGainHealth", 134217728i64);
    e!("PlayerFlag_NotGainSkill", 268435456i64);
    e!("PlayerFlag_SetMaxSpeed", 536870912i64);
    e!("PlayerFlag_SpecialVIP", 1073741824i64);
    e!("PlayerFlag_NotGenerateLoot", 2147483648i64);
    e!("PlayerFlag_CanTalkRedChannelAnonymous", 4294967296i64);
    e!("PlayerFlag_IgnoreProtectionZone", 8589934592i64);
    e!("PlayerFlag_IgnoreSpellCheck", 17179869184i64);
    e!("PlayerFlag_IgnoreWeaponCheck", 34359738368i64);
    e!("PlayerFlag_CannotBeMuted", 68719476736i64);
    e!("PlayerFlag_IsAlwaysPremium", 137438953472i64);
    e!("PlayerFlag_IgnoreYellCheck", 274877906944i64);
    e!("PlayerFlag_IgnoreSendPrivateCheck", 549755813888i64);

    // PLAYERSEX
    e!("PLAYERSEX_FEMALE", 0); e!("PLAYERSEX_MALE", 1);

    // REPORT_REASON / REPORT_TYPE
    e!("REPORT_REASON_NAMEINAPPROPRIATE", 0); e!("REPORT_REASON_NAMEPOORFORMATTED", 1);
    e!("REPORT_REASON_NAMEADVERTISING", 2); e!("REPORT_REASON_NAMEUNFITTING", 3);
    e!("REPORT_REASON_NAMERULEVIOLATION", 4); e!("REPORT_REASON_INSULTINGSTATEMENT", 5);
    e!("REPORT_REASON_SPAMMING", 6); e!("REPORT_REASON_ADVERTISINGSTATEMENT", 7);
    e!("REPORT_REASON_UNFITTINGSTATEMENT", 8); e!("REPORT_REASON_LANGUAGESTATEMENT", 9);
    e!("REPORT_REASON_DISCLOSURE", 10); e!("REPORT_REASON_RULEVIOLATION", 11);
    e!("REPORT_REASON_STATEMENT_BUGABUSE", 12); e!("REPORT_REASON_UNOFFICIALSOFTWARE", 13);
    e!("REPORT_REASON_PRETENDING", 14); e!("REPORT_REASON_HARASSINGOWNERS", 15);
    e!("REPORT_REASON_FALSEINFO", 16); e!("REPORT_REASON_ACCOUNTSHARING", 17);
    e!("REPORT_REASON_STEALINGDATA", 18); e!("REPORT_REASON_SERVICEATTACKING", 19);
    e!("REPORT_REASON_SERVICEAGREEMENT", 20);
    e!("REPORT_TYPE_NAME", 0); e!("REPORT_TYPE_STATEMENT", 1); e!("REPORT_TYPE_BOT", 2);

    // VOCATION
    e!("VOCATION_NONE", 0);

    // SKILL
    e!("SKILL_FIST", 0); e!("SKILL_CLUB", 1); e!("SKILL_SWORD", 2);
    e!("SKILL_AXE", 3); e!("SKILL_DISTANCE", 4); e!("SKILL_SHIELD", 5);
    e!("SKILL_FISHING", 6); e!("SKILL_MAGLEVEL", 7); e!("SKILL_LEVEL", 8);
    e!("SKILL_FIRST", 0); e!("SKILL_LAST", 6);

    // SPECIALSKILL
    e!("SPECIALSKILL_CRITICALHITCHANCE", 0); e!("SPECIALSKILL_CRITICALHITAMOUNT", 1);
    e!("SPECIALSKILL_LIFELEECHCHANCE", 2); e!("SPECIALSKILL_LIFELEECHAMOUNT", 3);
    e!("SPECIALSKILL_MANALEECHCHANCE", 4); e!("SPECIALSKILL_MANALEECHAMOUNT", 5);

    // SKULL
    e!("SKULL_NONE", 0); e!("SKULL_YELLOW", 1); e!("SKULL_GREEN", 2);
    e!("SKULL_WHITE", 3); e!("SKULL_RED", 4); e!("SKULL_BLACK", 5);

    // FLUID
    e!("FLUID_NONE", 0); e!("FLUID_WATER", 1); e!("FLUID_BLOOD", 2);
    e!("FLUID_BEER", 3); e!("FLUID_SLIME", 4); e!("FLUID_LEMONADE", 5);
    e!("FLUID_MILK", 6); e!("FLUID_MANA", 7);
    e!("FLUID_LIFE", 10); e!("FLUID_OIL", 11); e!("FLUID_URINE", 13);
    e!("FLUID_COCONUTMILK", 14); e!("FLUID_WINE", 15);
    e!("FLUID_MUD", 19); e!("FLUID_FRUITJUICE", 21);
    e!("FLUID_LAVA", 26); e!("FLUID_RUM", 27); e!("FLUID_SWAMP", 28);
    e!("FLUID_TEA", 35); e!("FLUID_MEAD", 43);

    // TALKTYPE
    e!("TALKTYPE_SAY", 1); e!("TALKTYPE_WHISPER", 2); e!("TALKTYPE_YELL", 3);
    e!("TALKTYPE_PRIVATE_PN", 4); e!("TALKTYPE_PRIVATE_NP", 5);
    e!("TALKTYPE_PRIVATE", 6); e!("TALKTYPE_CHANNEL_Y", 7);
    e!("TALKTYPE_CHANNEL_W", 8); e!("TALKTYPE_RVR_CHANNEL", 9);
    e!("TALKTYPE_RVR_ANSWER", 10); e!("TALKTYPE_RVR_CONTINUE", 11);
    e!("TALKTYPE_BROADCAST", 12); e!("TALKTYPE_CHANNEL_R1", 13);
    e!("TALKTYPE_PRIVATE_RED", 14); e!("TALKTYPE_CHANNEL_O", 15);
    e!("TALKTYPE_CHANNEL_R2", 17); e!("TALKTYPE_MONSTER_SAY", 19);
    e!("TALKTYPE_MONSTER_YELL", 20);

    // TEXTCOLOR
    e!("TEXTCOLOR_BLACK", 0); e!("TEXTCOLOR_BLUE", 5); e!("TEXTCOLOR_GREEN", 18);
    e!("TEXTCOLOR_LIGHTGREEN", 66); e!("TEXTCOLOR_DARKBROWN", 78);
    e!("TEXTCOLOR_LIGHTBLUE", 89); e!("TEXTCOLOR_MAYABLUE", 95);
    e!("TEXTCOLOR_DARKRED", 108); e!("TEXTCOLOR_DARKPURPLE", 112);
    e!("TEXTCOLOR_BROWN", 120); e!("TEXTCOLOR_GREY", 129);
    e!("TEXTCOLOR_TEAL", 143); e!("TEXTCOLOR_DARKPINK", 152);
    e!("TEXTCOLOR_PURPLE", 154); e!("TEXTCOLOR_DARKORANGE", 156);
    e!("TEXTCOLOR_RED", 180); e!("TEXTCOLOR_PINK", 190);
    e!("TEXTCOLOR_ORANGE", 192); e!("TEXTCOLOR_DARKYELLOW", 205);
    e!("TEXTCOLOR_YELLOW", 210); e!("TEXTCOLOR_WHITE", 215);
    e!("TEXTCOLOR_NONE", 255);

    // TILESTATE
    e!("TILESTATE_NONE", 0);
    e!("TILESTATE_FLOORCHANGE_DOWN", 1); e!("TILESTATE_FLOORCHANGE_NORTH", 2);
    e!("TILESTATE_FLOORCHANGE_SOUTH", 4); e!("TILESTATE_FLOORCHANGE_EAST", 8);
    e!("TILESTATE_FLOORCHANGE_WEST", 16); e!("TILESTATE_FLOORCHANGE_SOUTH_ALT", 32);
    e!("TILESTATE_FLOORCHANGE_EAST_ALT", 64); e!("TILESTATE_PROTECTIONZONE", 128);
    e!("TILESTATE_NOPVPZONE", 256); e!("TILESTATE_NOLOGOUT", 512);
    e!("TILESTATE_PVPZONE", 1024); e!("TILESTATE_TELEPORT", 2048);
    e!("TILESTATE_MAGICFIELD", 4096); e!("TILESTATE_MAILBOX", 8192);
    e!("TILESTATE_TRASHHOLDER", 16384); e!("TILESTATE_BED", 32768);
    e!("TILESTATE_DEPOT", 65536); e!("TILESTATE_BLOCKSOLID", 131072);
    e!("TILESTATE_BLOCKPATH", 262144); e!("TILESTATE_IMMOVABLEBLOCKSOLID", 524288);
    e!("TILESTATE_IMMOVABLEBLOCKPATH", 1048576);
    e!("TILESTATE_IMMOVABLENOFIELDBLOCKPATH", 2097152);
    e!("TILESTATE_NOFIELDBLOCKPATH", 4194304);
    e!("TILESTATE_SUPPORTS_HANGABLE", 8388608);
    e!("TILESTATE_FLOORCHANGE", 127);

    // WEAPON
    e!("WEAPON_NONE", 0); e!("WEAPON_SWORD", 1); e!("WEAPON_CLUB", 2);
    e!("WEAPON_AXE", 3); e!("WEAPON_SHIELD", 4); e!("WEAPON_DISTANCE", 5);
    e!("WEAPON_WAND", 6); e!("WEAPON_AMMO", 7);

    // WORLD_TYPE
    e!("WORLD_TYPE_NO_PVP", 1); e!("WORLD_TYPE_PVP", 2);
    e!("WORLD_TYPE_PVP_ENFORCED", 3);

    // FLAG (cylinder flags)
    e!("FLAG_NOLIMIT", 1); e!("FLAG_IGNOREBLOCKITEM", 2);
    e!("FLAG_IGNOREBLOCKCREATURE", 4); e!("FLAG_CHILDISOWNER", 8);
    e!("FLAG_PATHFINDING", 16); e!("FLAG_IGNOREFIELDDAMAGE", 32);
    e!("FLAG_IGNORENOTMOVEABLE", 64); e!("FLAG_IGNOREAUTOSTACK", 128);

    // SLOTP
    e!("SLOTP_WHEREEVER", 0xFFFF_FFFFu32 as i64);
    e!("SLOTP_HEAD", 1); e!("SLOTP_NECKLACE", 2); e!("SLOTP_BACKPACK", 4);
    e!("SLOTP_ARMOR", 8); e!("SLOTP_RIGHT", 16); e!("SLOTP_LEFT", 32);
    e!("SLOTP_LEGS", 64); e!("SLOTP_FEET", 128); e!("SLOTP_RING", 256);
    e!("SLOTP_AMMO", 512); e!("SLOTP_DEPOT", 1024); e!("SLOTP_TWO_HAND", 2048);
    e!("SLOTP_HAND", 48);

    // ORIGIN
    e!("ORIGIN_NONE", 0); e!("ORIGIN_CONDITION", 1); e!("ORIGIN_SPELL", 2);
    e!("ORIGIN_MELEE", 3); e!("ORIGIN_RANGED", 4); e!("ORIGIN_WAND", 5);

    // House access
    e!("GUEST_LIST", 0x0100_0000i64); e!("SUBOWNER_LIST", 0x0200_0000i64);

    // MAPMARK
    e!("MAPMARK_TICK", 0); e!("MAPMARK_QUESTION", 1); e!("MAPMARK_EXCLAMATION", 2);
    e!("MAPMARK_STAR", 3); e!("MAPMARK_CROSS", 4); e!("MAPMARK_TEMPLE", 5);
    e!("MAPMARK_KISS", 6); e!("MAPMARK_SHOVEL", 7); e!("MAPMARK_SWORD", 8);
    e!("MAPMARK_FLAG", 9); e!("MAPMARK_LOCK", 10); e!("MAPMARK_BAG", 11);
    e!("MAPMARK_SKULL", 12); e!("MAPMARK_DOLLAR", 13); e!("MAPMARK_REDNORTH", 14);
    e!("MAPMARK_REDSOUTH", 15); e!("MAPMARK_REDEAST", 16); e!("MAPMARK_REDWEST", 17);
    e!("MAPMARK_GREENNORTH", 18); e!("MAPMARK_GREENSOUTH", 19);

    // RETURNVALUE (0–75 sequential)
    let rv = [
        "RETURNVALUE_NOERROR", "RETURNVALUE_NOTPOSSIBLE", "RETURNVALUE_NOTENOUGHROOM",
        "RETURNVALUE_PLAYERISPZLOCKED", "RETURNVALUE_PLAYERISNOTINVITED",
        "RETURNVALUE_CANNOTTHROW", "RETURNVALUE_THEREISNOWAY",
        "RETURNVALUE_DESTINATIONOUTOFREACH", "RETURNVALUE_CREATUREBLOCK",
        "RETURNVALUE_NOTMOVEABLE", "RETURNVALUE_DROPTWOHANDEDITEM",
        "RETURNVALUE_BOTHHANDSNEEDTOBEFREE", "RETURNVALUE_CANONLYUSEONEWEAPON",
        "RETURNVALUE_NEEDEXCHANGE", "RETURNVALUE_CANNOTBEDRESSED",
        "RETURNVALUE_PUTTHISOBJECTINYOURHAND", "RETURNVALUE_PUTTHISOBJECTINBOTHHANDS",
        "RETURNVALUE_TOOFARAWAY", "RETURNVALUE_FIRSTGODOWNSTAIRS",
        "RETURNVALUE_FIRSTGOUPSTAIRS", "RETURNVALUE_CONTAINERNOTENOUGHROOM",
        "RETURNVALUE_NOTENOUGHCAPACITY", "RETURNVALUE_CANNOTPICKUP",
        "RETURNVALUE_THISISIMPOSSIBLE", "RETURNVALUE_DEPOTISFULL",
        "RETURNVALUE_CREATUREDOESNOTEXIST", "RETURNVALUE_CANNOTUSETHISOBJECT",
        "RETURNVALUE_PLAYERWITHTHISNAMEISNOTONLINE",
        "RETURNVALUE_NOTREQUIREDLEVELTOUSERUNE",
        "RETURNVALUE_YOUAREALREADYTRADING", "RETURNVALUE_THISPLAYERISALREADYTRADING",
        "RETURNVALUE_YOUMAYNOTLOGOUTDURINGAFIGHT", "RETURNVALUE_DIRECTPLAYERSHOOT",
        "RETURNVALUE_NOTENOUGHLEVEL", "RETURNVALUE_NOTENOUGHMAGICLEVEL",
        "RETURNVALUE_NOTENOUGHMANA", "RETURNVALUE_NOTENOUGHSOUL",
        "RETURNVALUE_YOUAREEXHAUSTED", "RETURNVALUE_YOUCANNOTUSEOBJECTSTHATFAST",
        "RETURNVALUE_PLAYERISNOTREACHABLE",
        "RETURNVALUE_CANONLYUSETHISRUNEONCREATURES",
        "RETURNVALUE_ACTIONNOTPERMITTEDINPROTECTIONZONE",
        "RETURNVALUE_YOUMAYNOTATTACKTHISPLAYER",
        "RETURNVALUE_YOUMAYNOTATTACKAPERSONINPROTECTIONZONE",
        "RETURNVALUE_YOUMAYNOTATTACKAPERSONWHILEINPROTECTIONZONE",
        "RETURNVALUE_YOUMAYNOTATTACKTHISCREATURE",
        "RETURNVALUE_YOUCANONLYUSEITONCREATURES",
        "RETURNVALUE_CREATUREISNOTREACHABLE",
        "RETURNVALUE_TURNSECUREMODETOATTACKUNMARKEDPLAYERS",
        "RETURNVALUE_YOUNEEDPREMIUMACCOUNT",
        "RETURNVALUE_YOUNEEDTOLEARNTHISSPELL",
        "RETURNVALUE_YOURVOCATIONCANNOTUSETHISSPELL",
        "RETURNVALUE_YOUNEEDAWEAPONTOUSETHISSPELL",
        "RETURNVALUE_PLAYERISPZLOCKEDLEAVEPVPZONE",
        "RETURNVALUE_PLAYERISPZLOCKEDENTERPVPZONE",
        "RETURNVALUE_ACTIONNOTPERMITTEDINANOPVPZONE",
        "RETURNVALUE_YOUCANNOTLOGOUTHERE",
        "RETURNVALUE_YOUNEEDAMAGICITEMTOCASTSPELL",
        "RETURNVALUE_CANNOTCONJUREITEMHERE",
        "RETURNVALUE_YOUNEEDTOSPLITYOURSPEARS",
        "RETURNVALUE_NAMEISTOOAMBIGUOUS",
        "RETURNVALUE_CANONLYUSEONESHIELD",
        "RETURNVALUE_NOPARTYMEMBERSINRANGE",
        "RETURNVALUE_YOUARENOTTHEOWNER",
        "RETURNVALUE_NOSUCHRAIDEXISTS",
        "RETURNVALUE_ANOTHERRAIDISALREADYEXECUTING",
        "RETURNVALUE_TRADEPLAYERFARAWAY",
        "RETURNVALUE_YOUDONTOWNTHISHOUSE",
        "RETURNVALUE_TRADEPLAYERALREADYOWNSAHOUSE",
        "RETURNVALUE_TRADEPLAYERHIGHESTBIDDER",
        "RETURNVALUE_YOUCANNOTTRADETHISHOUSE",
        "RETURNVALUE_YOUDONTHAVEREQUIREDPROFESSION",
        "RETURNVALUE_ITEMCANNOTBEMOVEDTHERE",
        "RETURNVALUE_YOUCANNOTUSETHISBED",
    ];
    for (i, name) in rv.iter().enumerate() {
        g.set(*name, i as i64)?;
    }

    // RELOAD_TYPE
    let reload = [
        "RELOAD_TYPE_ALL", "RELOAD_TYPE_ACTIONS", "RELOAD_TYPE_CHAT",
        "RELOAD_TYPE_CONFIG", "RELOAD_TYPE_CREATURESCRIPTS", "RELOAD_TYPE_EVENTS",
        "RELOAD_TYPE_GLOBAL", "RELOAD_TYPE_GLOBALEVENTS", "RELOAD_TYPE_ITEMS",
        "RELOAD_TYPE_MONSTERS", "RELOAD_TYPE_MOVEMENTS", "RELOAD_TYPE_NPCS",
        "RELOAD_TYPE_QUESTS", "RELOAD_TYPE_RAIDS", "RELOAD_TYPE_SCRIPTS",
        "RELOAD_TYPE_SPELLS", "RELOAD_TYPE_TALKACTIONS", "RELOAD_TYPE_WEAPONS",
    ];
    for (i, name) in reload.iter().enumerate() {
        g.set(*name, i as i64)?;
    }

    // ZONE
    e!("ZONE_PROTECTION", 0); e!("ZONE_NOPVP", 1); e!("ZONE_PVP", 2);
    e!("ZONE_NOLOGOUT", 3); e!("ZONE_NORMAL", 4);

    // Misc
    e!("MAX_LOOTCHANCE", 100000);
    e!("SPELL_UNDEFINED", 0); e!("SPELL_INSTANT", 1); e!("SPELL_RUNE", 2);
    e!("MONSTERS_EVENT_NONE", 0); e!("MONSTERS_EVENT_THINK", 1);
    e!("MONSTERS_EVENT_APPEAR", 2); e!("MONSTERS_EVENT_DISAPPEAR", 3);
    e!("MONSTERS_EVENT_MOVE", 4); e!("MONSTERS_EVENT_SAY", 5);

    Ok(())
}

// ---------------------------------------------------------------------------
// Global methods and variables
// ---------------------------------------------------------------------------

fn register_global_methods(lua: &Lua) -> LuaResult<()> {
    let g = lua.globals();
    g.set("INDEX_WHEREEVER", -1i32)?;
    g.set("VIRTUAL_PARENT", true)?;

    g.set("isType", lua.create_function(|_, (obj, class): (LuaValue, LuaTable)| {
        let obj_mt = match &obj {
            LuaValue::Table(t) => match t.metatable() {
                Some(mt) => mt,
                None => return Ok(false),
            },
            _ => return Ok(false),
        };
        let obj_h: i64 = obj_mt.raw_get(104i64).unwrap_or(0);
        if obj_h == 0 {
            return Ok(false);
        }
        let class_h: i64 = class.metatable()
            .and_then(|mt| mt.raw_get::<i64>(104).ok())
            .unwrap_or(0);
        if obj_h == class_h {
            return Ok(true);
        }
        if let Some(class_mt) = class.metatable() {
            if let Ok(LuaValue::Table(parent)) = class_mt.get::<LuaValue>("__index") {
                let parent_h: i64 = parent.metatable()
                    .and_then(|mt| mt.raw_get::<i64>(104).ok())
                    .unwrap_or(0);
                if parent_h != 0 && parent_h == obj_h {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    })?)?;

    g.set("rawgetmetatable", lua.create_function(|lua, obj: LuaValue| -> LuaResult<LuaValue> {
        match obj {
            LuaValue::String(s) => {
                let name = s.to_string_lossy();
                match lua.named_registry_value::<LuaTable>(name.as_ref()) {
                    Ok(t) => Ok(LuaValue::Table(t)),
                    Err(_) => Ok(LuaValue::Nil),
                }
            }
            LuaValue::Table(t) => Ok(t.metatable().map(LuaValue::Table).unwrap_or(LuaValue::Nil)),
            _ => Ok(LuaValue::Nil),
        }
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// configKeys table
// ---------------------------------------------------------------------------

fn register_config_keys(lua: &Lua) -> LuaResult<()> {
    let tbl = lua.create_table()?;
    macro_rules! ck { ($name:expr, $val:expr) => { tbl.set($name, $val as i64)?; }; }

    // BooleanConfig (0-based)
    ck!("ALLOW_CHANGEOUTFIT", 0); ck!("ONE_PLAYER_ON_ACCOUNT", 1);
    ck!("AIMBOT_HOTKEY_ENABLED", 2); ck!("REMOVE_RUNE_CHARGES", 3);
    ck!("REMOVE_WEAPON_AMMO", 4); ck!("REMOVE_WEAPON_CHARGES", 5);
    ck!("REMOVE_POTION_CHARGES", 6); ck!("PZLOCK_SKULL_ATTACKER", 7);
    ck!("EXPERIENCE_FROM_PLAYERS", 8); ck!("FREE_PREMIUM", 9);
    ck!("REPLACE_KICK_ON_LOGIN", 10); ck!("ALLOW_CLONES", 11);
    ck!("BIND_ONLY_GLOBAL_ADDRESS", 13); ck!("OPTIMIZE_DATABASE", 14);
    ck!("EMOTE_SPELLS", 15); ck!("STAMINA_SYSTEM", 16);
    ck!("WARN_UNSAFE_SCRIPTS", 17); ck!("CONVERT_UNSAFE_SCRIPTS", 18);
    ck!("CLASSIC_EQUIPMENT_SLOTS", 19); ck!("CLASSIC_ATTACK_SPEED", 20);
    ck!("SERVER_SAVE_NOTIFY_MESSAGE", 22); ck!("SERVER_SAVE_CLEAN_MAP", 23);
    ck!("SERVER_SAVE_CLOSE", 24); ck!("SERVER_SAVE_SHUTDOWN", 25);
    ck!("ONLINE_OFFLINE_CHARLIST", 26); ck!("LUA_ITEM_DESC", 32);
    ck!("PLAYER_CONSOLE_LOGS", 37);

    // StringConfig (0-based)
    ck!("MAP_NAME", 1); ck!("HOUSE_RENT_PERIOD", 2); ck!("SERVER_NAME", 3);
    ck!("OWNER_NAME", 4); ck!("OWNER_EMAIL", 5); ck!("URL", 6);
    ck!("LOCATION", 7); ck!("MOTD", 8); ck!("WORLD_TYPE", 9);
    ck!("MYSQL_HOST", 10); ck!("MYSQL_USER", 11); ck!("MYSQL_PASS", 12);
    ck!("MYSQL_DB", 13); ck!("MYSQL_SOCK", 14); ck!("DEFAULT_PRIORITY", 15);
    ck!("MAP_AUTHOR", 16);

    // IntegerConfig (0-based) — IP conflicts with StringConfig so we use the int value
    ck!("SQL_PORT", 1); ck!("MAX_PLAYERS", 2); ck!("PZ_LOCKED", 3);
    ck!("DEFAULT_DESPAWNRANGE", 4); ck!("DEFAULT_DESPAWNRADIUS", 5);
    ck!("DEFAULT_WALKTOSPAWNRADIUS", 6); ck!("REMOVE_ON_DESPAWN", 7);
    ck!("RATE_EXPERIENCE", 7); ck!("RATE_SKILL", 8); ck!("RATE_LOOT", 9);
    ck!("RATE_MAGIC", 10); ck!("RATE_SPAWN", 11); ck!("HOUSE_PRICE", 12);
    ck!("KILLS_TO_RED", 13); ck!("KILLS_TO_BLACK", 14);
    ck!("MAX_MESSAGEBUFFER", 15); ck!("ACTIONS_DELAY_INTERVAL", 16);
    ck!("EX_ACTIONS_DELAY_INTERVAL", 17); ck!("KICK_AFTER_MINUTES", 18);
    ck!("PROTECTION_LEVEL", 19); ck!("DEATH_LOSE_PERCENT", 20);
    ck!("STATUSQUERY_TIMEOUT", 21); ck!("FRAG_TIME", 22);
    ck!("WHITE_SKULL_TIME", 23); ck!("GAME_PORT", 24);
    ck!("LOGIN_PORT", 25); ck!("STATUS_PORT", 26); ck!("STAIRHOP_DELAY", 27);
    ck!("EXP_FROM_PLAYERS_LEVEL_RANGE", 28); ck!("MAX_PACKETS_PER_SECOND", 29);
    ck!("SERVER_SAVE_NOTIFY_DURATION", 30);

    lua.globals().set("configKeys", tbl)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// os / table extensions
// ---------------------------------------------------------------------------

fn register_os_extensions(lua: &Lua) -> LuaResult<()> {
    let os: LuaTable = lua.globals().get("os")?;
    os.set("mtime", lua.create_function(|_, ()| {
        use std::time::{SystemTime, UNIX_EPOCH};
        Ok(SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64)
    })?)?;
    Ok(())
}

fn register_table_extensions(lua: &Lua) -> LuaResult<()> {
    let tbl: LuaTable = lua.globals().get("table")?;

    tbl.set("create", lua.create_function(|lua, (size, val): (usize, LuaValue)| {
        let t = lua.create_table_with_capacity(size, 0)?;
        for i in 1..=size as i64 {
            t.set(i, val.clone())?;
        }
        Ok(t)
    })?)?;

    tbl.set("pack", lua.create_function(|lua, args: LuaMultiValue| {
        let t = lua.create_table()?;
        let n = args.len() as i64;
        for (i, v) in args.into_iter().enumerate() {
            t.set(i as i64 + 1, v)?;
        }
        t.set("n", n)?;
        Ok(t)
    })?)?;

    tbl.set("contains", lua.create_function(|_, (tbl, value): (LuaTable, LuaValue)| {
        for v in tbl.sequence_values::<LuaValue>().flatten() {
            if v == value {
                return Ok(true);
            }
        }
        Ok(false)
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Game class
// ---------------------------------------------------------------------------

fn register_game_class(lua: &Lua) -> LuaResult<()> {
    let tbl = lua.create_table()?;
    lua.globals().set("Game", tbl.clone())?;

    tbl.set("getGameState", lua.create_function(|_, ()| -> LuaResult<i64> {
        let state = g_game().lock().unwrap().get_game_state();
        Ok(match state {
            crate::game::GameState::Startup => 0,
            crate::game::GameState::Init => 1,
            crate::game::GameState::Normal => 2,
            crate::game::GameState::Closed => 3,
            crate::game::GameState::Shutdown => 4,
            crate::game::GameState::Closing => 5,
            crate::game::GameState::Maintain => 6,
        })
    })?)?;

    tbl.set("setGameState", lua.create_function(|_, state: i64| -> LuaResult<()> {
        let gs = match state {
            0 => crate::game::GameState::Startup,
            1 => crate::game::GameState::Init,
            2 => crate::game::GameState::Normal,
            3 => crate::game::GameState::Closed,
            4 => crate::game::GameState::Shutdown,
            5 => crate::game::GameState::Closing,
            6 => crate::game::GameState::Maintain,
            _ => return Ok(()),
        };
        g_game().lock().unwrap().set_game_state(gs);
        Ok(())
    })?)?;

    tbl.set("getWorldType", lua.create_function(|_, ()| -> LuaResult<i64> {
        Ok(match g_game().lock().unwrap().get_world_type() {
            crate::game::WorldType::NoPvp => 1,
            crate::game::WorldType::Pvp => 2,
            crate::game::WorldType::PvpEnforced => 3,
        })
    })?)?;

    tbl.set("setWorldType", lua.create_function(|_, wt: i64| -> LuaResult<()> {
        let world_type = match wt {
            1 => crate::game::WorldType::NoPvp,
            2 => crate::game::WorldType::Pvp,
            3 => crate::game::WorldType::PvpEnforced,
            _ => return Ok(()),
        };
        g_game().lock().unwrap().set_world_type(world_type);
        Ok(())
    })?)?;

    tbl.set("getSpectators", lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaTable> {
        let mut iter = args.into_iter();
        let pos_tbl = match iter.next() { Some(LuaValue::Table(t)) => t, _ => return lua.create_table() };
        let pos = table_to_position(&pos_tbl)?;
        let mf = match iter.next() { Some(LuaValue::Boolean(b)) => b, _ => false };
        let op = match iter.next() { Some(LuaValue::Boolean(b)) => b, _ => false };
        let min_rx: i32 = match iter.next() { Some(LuaValue::Integer(n)) => n as i32, _ => 0 };
        let max_rx: i32 = match iter.next() { Some(LuaValue::Integer(n)) => n as i32, _ => 0 };
        let min_ry: i32 = match iter.next() { Some(LuaValue::Integer(n)) => n as i32, _ => 0 };
        let max_ry: i32 = match iter.next() { Some(LuaValue::Integer(n)) => n as i32, _ => 0 };
        let ids = g_game().lock().unwrap().map.get_spectators(pos, mf, op, min_rx, max_rx, min_ry, max_ry);
        let result = lua.create_table()?;
        for (i, id) in ids.iter().enumerate() {
            let game = g_game().lock().unwrap();
            let class = if game.get_player(*id).is_some() { "Player" }
                else { "Creature" };
            drop(game);
            let c = push_creature_ref(lua, *id, class)?;
            result.raw_set(i as i64 + 1, c)?;
        }
        Ok(result)
    })?)?;

    tbl.set("getPlayers", lua.create_function(|lua, ()| -> LuaResult<LuaTable> {
        let game = g_game().lock().unwrap();
        let player_ids = game.get_all_players();
        drop(game);
        let result = lua.create_table()?;
        for (i, id) in player_ids.iter().enumerate() {
            let c = push_creature_ref(lua, *id, "Player")?;
            result.raw_set(i as i64 + 1, c)?;
        }
        Ok(result)
    })?)?;

    tbl.set("getPlayerCount", lua.create_function(|_, ()| -> LuaResult<i64> {
        Ok(g_game().lock().unwrap().get_player_count() as i64)
    })?)?;

    tbl.set("getMonsterCount", lua.create_function(|_, ()| -> LuaResult<i64> {
        Ok(g_game().lock().unwrap().get_monster_count() as i64)
    })?)?;

    tbl.set("getNpcCount", lua.create_function(|_, ()| -> LuaResult<i64> {
        Ok(g_game().lock().unwrap().get_npc_count() as i64)
    })?)?;

    tbl.set("getWorldTime", lua.create_function(|_, ()| -> LuaResult<i64> {
        Ok(g_game().lock().unwrap().get_world_time() as i64)
    })?)?;

    tbl.set("getWorldLight", lua.create_function(|lua, ()| -> LuaResult<LuaTable> {
        let (level, color) = g_game().lock().unwrap().get_world_light_info();
        let t = lua.create_table()?;
        t.set("level", level)?;
        t.set("color", color)?;
        Ok(t)
    })?)?;

    tbl.set("getExperienceForLevel", lua.create_function(|_, level: i64| -> LuaResult<i64> {
        if level <= 0 { return Ok(0); }
        let lv = level as u64;
        Ok(((50 * lv * lv * lv) / 3 - 100 * lv * lv + (850 * lv) / 3 - 200) as i64)
    })?)?;

    tbl.set("getClientVersion", lua.create_function(|lua, ()| -> LuaResult<LuaTable> {
        let t = lua.create_table()?;
        t.set("min", 860)?;
        t.set("max", 860)?;
        Ok(t)
    })?)?;

    tbl.set("getExperienceStage", lua.create_function(|_, level: i64| -> LuaResult<f64> {
        let cfg = g_config();
        let rate = cfg.get_number(IntegerConfig::RateExperience);
        let _ = level;
        Ok(rate as f64)
    })?)?;

    tbl.set("getTowns", lua.create_function(|lua, ()| -> LuaResult<LuaTable> {
        let game = g_game().lock().unwrap();
        let result = lua.create_table()?;
        let mut i = 1i64;
        for (_, town) in game.map.towns.iter() {
            let tt = lua.create_table()?;
            tt.raw_set(1, town.id)?;
            if let Ok(mt) = lua.named_registry_value::<LuaTable>("Town") {
                let _ = tt.set_metatable(Some(mt));
            }
            result.raw_set(i, tt)?;
            i += 1;
        }
        Ok(result)
    })?)?;

    tbl.set("loadMap", lua.create_function(|_, _path: String| -> LuaResult<()> {
        Ok(())
    })?)?;

    tbl.set("getMonsterTypes", lua.create_function(|lua, ()| -> LuaResult<LuaTable> {
        lua.create_table()
    })?)?;

    tbl.set("getCurrencyItems", lua.create_function(|lua, ()| -> LuaResult<LuaTable> {
        let result = lua.create_table()?;
        let game = g_game().lock().unwrap();
        let currency_items = game.items.get_currency_items();
        let mut i = 1i64;
        for (_, &item_id) in currency_items.iter() {
            let t = lua.create_table()?;
            t.raw_set(1, item_id)?;
            if let Ok(mt) = lua.named_registry_value::<LuaTable>("ItemType") {
                let _ = t.set_metatable(Some(mt));
            }
            result.raw_set(i, t)?;
            i += 1;
        }
        Ok(result)
    })?)?;

    tbl.set("getHouses", lua.create_function(|lua, ()| -> LuaResult<LuaTable> {
        let result = lua.create_table()?;
        let game = g_game().lock().unwrap();
        let mut i = 1i64;
        for (&house_id, _) in game.map.houses.get_houses().iter() {
            let t = lua.create_table()?;
            t.raw_set(1, house_id)?;
            if let Ok(mt) = lua.named_registry_value::<LuaTable>("House") {
                let _ = t.set_metatable(Some(mt));
            }
            result.raw_set(i, t)?;
            i += 1;
        }
        Ok(result)
    })?)?;

    tbl.set("getItemAttributeByName", lua.create_function(|_, _name: String| -> LuaResult<i32> {
        Ok(-1)
    })?)?;

    tbl.set("getReturnMessage", lua.create_function(|_, code: i32| -> LuaResult<String> {
        Ok(String::from(get_return_message(code)))
    })?)?;

    tbl.set("createItem", lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let mut it = args.into_iter();
        let arg1 = it.next().unwrap_or(LuaValue::Nil);
        let count_val = it.next();
        let pos_val = it.next();

        let mut game = g_game().lock().unwrap();
        let id: u16 = match &arg1 {
            LuaValue::Integer(n) => *n as u16,
            LuaValue::Number(n) => *n as u16,
            LuaValue::String(s) => {
                let name = s.to_string_lossy();
                game.items.get_item_id_by_name(&name).unwrap_or(0)
            }
            _ => 0,
        };
        if id == 0 { return Ok(LuaValue::Nil); }

        let mut count: u16 = match count_val {
            Some(LuaValue::Integer(n)) => n as u16,
            Some(LuaValue::Number(n)) => n as u16,
            _ => 1,
        };

        let it_type = game.items.get_item_type(id as usize);
        if it_type.stackable {
            count = count.min(100);
        }

        let item = crate::map::tile::MapItem { server_id: id, count, ..Default::default() };

        if let Some(LuaValue::Table(pos_table)) = pos_val {
            let pos = table_to_position(&pos_table)?;
            let items_arc = game.items.clone();
            if let Some(tile) = game.map.get_tile_mut(pos) {
                let idx = tile.items.len() as i32;
                tile.internal_add_item(item, &items_arc);
                drop(game);
                return Ok(LuaValue::Table(push_item_ref(lua, id, pos, idx)?));
            } else {
                return Ok(LuaValue::Nil);
            }
        }

        drop(game);
        let t = push_item_ref(lua, id, Position::default(), -1)?;
        t.raw_set("_count", count)?;
        Ok(LuaValue::Table(t))
    })?)?;

    tbl.set("createContainer", lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let mut it = args.into_iter();
        let arg1 = it.next().unwrap_or(LuaValue::Nil);
        let _size_val = it.next();
        let pos_val = it.next();

        let mut game = g_game().lock().unwrap();
        let id: u16 = match &arg1 {
            LuaValue::Integer(n) => *n as u16,
            LuaValue::Number(n) => *n as u16,
            LuaValue::String(s) => {
                let name = s.to_string_lossy();
                game.items.get_item_id_by_name(&name).unwrap_or(0)
            }
            _ => 0,
        };
        if id == 0 { return Ok(LuaValue::Nil); }

        let item = crate::map::tile::MapItem { server_id: id, ..Default::default() };

        if let Some(LuaValue::Table(pos_table)) = pos_val {
            let pos = table_to_position(&pos_table)?;
            let items_arc = game.items.clone();
            if let Some(tile) = game.map.get_tile_mut(pos) {
                let idx = tile.items.len() as i32;
                tile.internal_add_item(item, &items_arc);
                drop(game);
                return Ok(LuaValue::Table(push_item_ref(lua, id, pos, idx)?));
            } else {
                return Ok(LuaValue::Nil);
            }
        }
        drop(game);
        Ok(LuaValue::Table(push_item_ref(lua, id, Position::default(), -1)?))
    })?)?;

    tbl.set("createMonster", lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let mut it = args.into_iter();
        let name_val = it.next().unwrap_or(LuaValue::Nil);
        let pos_val = it.next();
        let _extended = it.next();
        let _force = it.next();

        let name = match &name_val {
            LuaValue::String(s) => s.to_string_lossy().to_string(),
            _ => return Ok(LuaValue::Nil),
        };

        let pos = match pos_val {
            Some(LuaValue::Table(t)) => table_to_position(&t)?,
            _ => return Ok(LuaValue::Nil),
        };

        let mut game = g_game().lock().unwrap();
        let cid = game.next_creature_id();
        let base = crate::creatures::CreatureBase::new(cid, pos);
        let mtype_info = crate::creatures::monsters::MonsterInfo::default();
        let monster = crate::creatures::monster::Monster::new(name, mtype_info, base);
        let creature = crate::creatures::Creature::Monster(Box::new(monster));
        game.add_creature(creature);
        drop(game);
        Ok(LuaValue::Table(push_creature_ref(lua, cid, "Monster")?))
    })?)?;

    tbl.set("createNpc", lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let mut it = args.into_iter();
        let name_val = it.next().unwrap_or(LuaValue::Nil);
        let pos_val = it.next();

        let name = match &name_val {
            LuaValue::String(s) => s.to_string_lossy().to_string(),
            _ => return Ok(LuaValue::Nil),
        };

        let pos = match pos_val {
            Some(LuaValue::Table(t)) => table_to_position(&t)?,
            _ => return Ok(LuaValue::Nil),
        };

        let mut game = g_game().lock().unwrap();
        let cid = game.next_creature_id();
        let base = crate::creatures::CreatureBase::new(cid, pos);
        let npc = crate::creatures::npc::Npc::new(base, name, 2000, String::new());
        let creature = crate::creatures::Creature::Npc(Box::new(npc));
        game.add_creature(creature);
        drop(game);
        Ok(LuaValue::Table(push_creature_ref(lua, cid, "Npc")?))
    })?)?;

    tbl.set("createTile", lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let mut it = args.into_iter();
        let arg1 = it.next();
        let pos = match arg1 {
            Some(LuaValue::Table(t)) => table_to_position(&t)?,
            Some(LuaValue::Integer(x)) => {
                let y = match it.next() { Some(LuaValue::Integer(v)) => v as u16, _ => 0 };
                let z = match it.next() { Some(LuaValue::Integer(v)) => v as u8, _ => 0 };
                Position { x: x as u16, y, z }
            }
            _ => return Ok(LuaValue::Nil),
        };

        let mut game = g_game().lock().unwrap();
        if game.map.get_tile(pos).is_none() {
            game.map.set_tile(pos, crate::map::tile::Tile::new(pos, crate::map::tile::TileKind::Static));
        }
        drop(game);
        Ok(LuaValue::Table(push_position(lua, pos)?))
    })?)?;

    tbl.set("createMonsterType", lua.create_function(|lua, name: String| -> LuaResult<LuaValue> {
        let t = lua.create_table()?;
        t.set("_name", name)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("MonsterType") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(LuaValue::Table(t))
    })?)?;

    tbl.set("startRaid", lua.create_function(|_, _name: String| -> LuaResult<bool> {
        Ok(false)
    })?)?;

    tbl.set("sendAnimatedText", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<bool> {
        let mut it = args.into_iter();
        let message = match it.next() {
            Some(LuaValue::String(s)) => s.as_bytes().to_vec(),
            _ => return Ok(false),
        };
        let pos = match it.next() {
            Some(LuaValue::Table(t)) => crate::lua::registrations::table_to_position(&t)?,
            _ => return Ok(false),
        };
        let color = match it.next() {
            Some(LuaValue::Integer(n)) => n as u8,
            _ => return Ok(false),
        };
        if pos.x == 0 && pos.y == 0 { return Ok(false); }
        crate::net::game_protocol::broadcast_animated_text(pos, color, message);
        Ok(true)
    })?)?;

    tbl.set("reload", lua.create_function(|_, _reload_type: i32| -> LuaResult<bool> {
        Ok(true)
    })?)?;

    tbl.set("getAccountStorageValue", lua.create_function(|_, (_account_id, _key): (u32, u32)| -> LuaResult<i32> {
        Ok(-1)
    })?)?;

    tbl.set("setAccountStorageValue", lua.create_function(|_, (_account_id, _key, _value): (u32, u32, i32)| -> LuaResult<bool> {
        Ok(true)
    })?)?;

    tbl.set("saveAccountStorageValues", lua.create_function(|_, ()| -> LuaResult<bool> {
        Ok(true)
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Variant class
// ---------------------------------------------------------------------------

fn register_variant_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| {
        let t = lua.create_table()?;
        match args.into_iter().next() {
            Some(LuaValue::Integer(n)) => { t.set("type", 1i32)?; t.set("number", n)?; }
            Some(LuaValue::Number(f))  => { t.set("type", 1i32)?; t.set("number", f as i64)?; }
            Some(LuaValue::String(s))  => { t.set("type", 4i32)?; t.set("string", s.to_str().map(|b| (*b).to_string()).unwrap_or_default())?; }
            Some(LuaValue::Table(pos)) => { t.set("type", 2i32)?; t.set("pos", pos)?; }
            _ => { t.set("type", 0i32)?; }
        }
        Ok(t)
    })?;
    let methods = register_class(lua, "Variant", None, LUA_DATA_UNKNOWN, Some(ctor))?;

    methods.set("getNumber",   lua.create_function(|_, t: LuaTable| t.get::<i64>("number"))?)?;
    methods.set("getString",   lua.create_function(|_, t: LuaTable| t.get::<String>("string"))?)?;
    methods.set("getPosition", lua.create_function(|_, t: LuaTable| t.get::<LuaValue>("pos"))?)?;
    methods.set("getType",     lua.create_function(|_, t: LuaTable| t.get::<i32>("type"))?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Position class
// ---------------------------------------------------------------------------

fn register_position_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaTable> {
        let mut iter = args.into_iter().skip(1);
        let first = iter.next();
        let (x, y, z, stackpos) = if let Some(LuaValue::Table(pos_t)) = &first {
            let x: i64 = pos_t.get("x").unwrap_or(0);
            let y: i64 = pos_t.get("y").unwrap_or(0);
            let z: i64 = pos_t.get("z").unwrap_or(0);
            let sp: i32 = pos_t.get("stackpos").unwrap_or(0);
            (x, y, z, sp)
        } else {
            let x = match &first {
                Some(LuaValue::Integer(n)) => *n,
                Some(LuaValue::Number(n)) => *n as i64,
                _ => 0,
            };
            let y = match iter.next() {
                Some(LuaValue::Integer(n)) => n,
                Some(LuaValue::Number(n)) => n as i64,
                _ => 0,
            };
            let z = match iter.next() {
                Some(LuaValue::Integer(n)) => n,
                Some(LuaValue::Number(n)) => n as i64,
                _ => 0,
            };
            let sp = match iter.next() {
                Some(LuaValue::Integer(n)) => n as i32,
                Some(LuaValue::Number(n)) => n as i32,
                _ => 0,
            };
            (x, y, z, sp)
        };
        let t = lua.create_table()?;
        t.set("x", x)?;
        t.set("y", y)?;
        t.set("z", z)?;
        t.set("stackpos", stackpos)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Position") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(t)
    })?;
    let methods = register_class(lua, "Position", None, LUA_DATA_UNKNOWN, Some(ctor))?;

    // __add: pos + pos -> pos
    let add_fn = lua.create_function(|lua, (a, b): (LuaTable, LuaTable)| {
        let t = lua.create_table()?;
        t.set("x", pos_field(&a, "x")? + pos_field(&b, "x")?)?;
        t.set("y", pos_field(&a, "y")? + pos_field(&b, "y")?)?;
        t.set("z", pos_field(&a, "z")? + pos_field(&b, "z")?)?;
        t.set("stackpos", 0i32)?;
        Ok(t)
    })?;
    let sub_fn = lua.create_function(|lua, (a, b): (LuaTable, LuaTable)| {
        let t = lua.create_table()?;
        t.set("x", pos_field(&a, "x")? - pos_field(&b, "x")?)?;
        t.set("y", pos_field(&a, "y")? - pos_field(&b, "y")?)?;
        t.set("z", pos_field(&a, "z")? - pos_field(&b, "z")?)?;
        t.set("stackpos", 0i32)?;
        Ok(t)
    })?;
    let eq_fn = lua.create_function(|_, (a, b): (LuaTable, LuaTable)| -> LuaResult<bool> {
        Ok(pos_field(&a, "x")? == pos_field(&b, "x")?
            && pos_field(&a, "y")? == pos_field(&b, "y")?
            && pos_field(&a, "z")? == pos_field(&b, "z")?)
    })?;
    set_meta(lua, "Position", "__add", add_fn)?;
    set_meta(lua, "Position", "__sub", sub_fn)?;
    set_meta(lua, "Position", "__eq",  eq_fn)?;

    methods.set("getDistance", lua.create_function(|_, (this, other): (LuaTable, LuaTable)| -> LuaResult<f64> {
        let ax: f64 = this.get::<i64>("x")? as f64;
        let ay: f64 = this.get::<i64>("y")? as f64;
        let az: f64 = this.get::<i64>("z")? as f64;
        let bx: f64 = other.get::<i64>("x")? as f64;
        let by: f64 = other.get::<i64>("y")? as f64;
        let bz: f64 = other.get::<i64>("z")? as f64;
        let dx = (ax - bx).abs();
        let dy = (ay - by).abs();
        let dz = (az - bz).abs();
        let xy = dx.max(dy);
        Ok(xy.max(dz))
    })?)?;

    methods.set("isSightClear", lua.create_function(|_, (_this, _other, _sight_clear): (LuaTable, LuaTable, Option<bool>)| -> LuaResult<bool> {
        Ok(true)
    })?)?;

    methods.set("sendMagicEffect", lua.create_function(|_, (this, effect): (LuaTable, u8)| -> LuaResult<bool> {
        let pos = table_to_position(&this)?;
        broadcast_effect_to_spectators(pos, |output| {
            output.add_byte(0x83);
            output.add_position(pos.x, pos.y, pos.z);
            output.add_byte(effect);
        });
        Ok(true)
    })?)?;

    methods.set("sendDistanceEffect", lua.create_function(|_, (this, target, effect): (LuaTable, LuaTable, u8)| -> LuaResult<bool> {
        let from = table_to_position(&this)?;
        let to = table_to_position(&target)?;
        broadcast_effect_to_spectators(from, |output| {
            output.add_byte(0x85);
            output.add_position(from.x, from.y, from.z);
            output.add_position(to.x, to.y, to.z);
            output.add_byte(effect);
        });
        Ok(true)
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tile class
// ---------------------------------------------------------------------------

fn register_tile_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let mut iter = args.into_iter().skip(1);
        let pos = match (iter.next(), iter.next(), iter.next()) {
            (Some(LuaValue::Table(tbl)), _, _) => {
                table_to_position(&tbl)?
            }
            (Some(LuaValue::Integer(x)), Some(LuaValue::Integer(y)), Some(LuaValue::Integer(z))) => {
                Position { x: x as u16, y: y as u16, z: z as u8 }
            }
            _ => return Ok(LuaValue::Nil),
        };
        let tile_exists = {
            let g = g_game().lock().unwrap();
            g.map.get_tile(pos).is_some()
        };
        if !tile_exists {
            return Ok(LuaValue::Nil);
        }
        let t = lua.create_table()?;
        t.set("x", pos.x)?;
        t.set("y", pos.y)?;
        t.set("z", pos.z)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Tile") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(LuaValue::Table(t))
    })?;
    let methods = register_class(lua, "Tile", None, LUA_DATA_TILE, Some(ctor))?;
    set_meta(lua, "Tile", "__eq", lua.create_function(|_, (a, b): (LuaTable, LuaTable)| {
        let ax: u16 = a.get("x").unwrap_or(0);
        let ay: u16 = a.get("y").unwrap_or(0);
        let az: u8 = a.get("z").unwrap_or(0);
        let bx: u16 = b.get("x").unwrap_or(0);
        let by: u16 = b.get("y").unwrap_or(0);
        let bz: u8 = b.get("z").unwrap_or(0);
        Ok(ax == bx && ay == by && az == bz)
    })?)?;

    methods.set("getPosition", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let pos = table_to_position(&this)?;
        push_position(lua, pos)
    })?)?;

    methods.set("getCreatureCount", lua.create_function(|_, this: LuaTable| -> LuaResult<usize> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.map.get_tile(pos).map(|t| t.get_creature_count()).unwrap_or(0))
    })?)?;

    methods.set("getCreatures", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        let result = lua.create_table()?;
        if let Some(tile) = game.map.get_tile(pos) {
            for (i, &cid) in tile.get_creatures().iter().enumerate() {
                let c = push_creature_ref(lua, cid, "Creature")?;
                result.raw_set(i as i64 + 1, c)?;
            }
        }
        Ok(result)
    })?)?;

    methods.set("getItemCount", lua.create_function(|_, this: LuaTable| -> LuaResult<usize> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.map.get_tile(pos).map(|t| t.items.len() + if t.ground.is_some() { 1 } else { 0 }).unwrap_or(0))
    })?)?;

    methods.set("getTopItemCount", lua.create_function(|_, this: LuaTable| -> LuaResult<usize> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.map.get_tile(pos).map(|t| t.get_top_item_count()).unwrap_or(0))
    })?)?;

    methods.set("getDownItemCount", lua.create_function(|_, this: LuaTable| -> LuaResult<usize> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.map.get_tile(pos).map(|t| t.get_down_item_count()).unwrap_or(0))
    })?)?;

    methods.set("hasFlag", lua.create_function(|_, (this, flag): (LuaTable, u32)| -> LuaResult<bool> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.map.get_tile(pos).map(|t| t.has_flag(flag)).unwrap_or(false))
    })?)?;

    methods.set("isWalkable", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        let Some(tile) = game.map.get_tile(pos) else { return Ok(false) };
        if tile.ground.is_none() { return Ok(false); }
        if tile.has_flag(crate::map::tile::TILESTATE_BLOCKSOLID) { return Ok(false); }
        Ok(true)
    })?)?;

    methods.set("hasProperty", lua.create_function(|_, (this, prop): (LuaTable, i32)| -> LuaResult<bool> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(tile) = game.map.get_tile(pos) {
            let check_item = |item: &crate::map::tile::MapItem| -> bool {
                let it = game.items.get_item_type(item.server_id as usize);
                let is_magic_field = it.kind == crate::items::ItemKind::MagicField;
                match prop {
                    0 => it.block_solid,
                    1 => it.has_height,
                    2 => it.block_projectile,
                    3 => it.block_path_find,
                    4 => it.is_vertical,
                    5 => it.is_horizontal,
                    6 => it.moveable,
                    7 => it.block_solid && !it.moveable,
                    8 => it.block_path_find && !it.moveable,
                    9 => it.block_path_find && !it.moveable && !is_magic_field,
                    10 => it.block_path_find && !is_magic_field,
                    11 => it.supports_hangable,
                    _ => false,
                }
            };
            if let Some(ground) = &tile.ground {
                if check_item(ground) { return Ok(true); }
            }
            for item in &tile.items {
                if check_item(item) { return Ok(true); }
            }
        }
        Ok(false)
    })?)?;

    methods.set("remove", lua.create_function(|_, _: LuaTable| -> LuaResult<()> {
        Ok(())
    })?)?;

    methods.set("getGround", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(tile) = game.map.get_tile(pos) {
            if let Some(ground) = &tile.ground {
                return Ok(LuaValue::Table(push_item_ref_attrs(lua, ground.server_id, pos, -1, ground.unique_id, ground.action_id)?));
            }
        }
        Ok(LuaValue::Nil)
    })?)?;

    methods.set("getThing", lua.create_function(|lua, (this, index): (LuaTable, i32)| -> LuaResult<LuaValue> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(tile) = game.map.get_tile(pos) {
            let mut i = index;
            if let Some(ground) = &tile.ground {
                if i == 0 {
                    return Ok(LuaValue::Table(push_item_ref_attrs(lua, ground.server_id, pos, -1, ground.unique_id, ground.action_id)?));
                }
                i -= 1;
            }
            let top_count = tile.get_top_item_count();
            if (i as usize) < top_count {
                let item = &tile.items[i as usize];
                return Ok(LuaValue::Table(push_item_ref_attrs(lua, item.server_id, pos, i, item.unique_id, item.action_id)?));
            }
            i -= top_count as i32;
            if (i as usize) < tile.creature_ids.len() {
                let cid = tile.creature_ids[i as usize];
                let class = match game.get_creature(cid) {
                    Some(crate::creatures::Creature::Player(_)) => "Player",
                    Some(crate::creatures::Creature::Monster(_)) => "Monster",
                    Some(crate::creatures::Creature::Npc(_)) => "Npc",
                    None => "Creature",
                };
                return Ok(LuaValue::Table(push_creature_ref(lua, cid, class)?));
            }
            i -= tile.creature_ids.len() as i32;
            let down_start = top_count;
            let down_count = tile.get_down_item_count();
            if (i as usize) < down_count {
                let actual_idx = down_start + i as usize;
                let item = &tile.items[actual_idx];
                return Ok(LuaValue::Table(push_item_ref_attrs(lua, item.server_id, pos, actual_idx as i32, item.unique_id, item.action_id)?));
            }
        }
        Ok(LuaValue::Nil)
    })?)?;

    methods.set("getThingCount", lua.create_function(|_, this: LuaTable| -> LuaResult<usize> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.map.get_tile(pos).map(|t| {
            t.get_creature_count() + t.items.len() + if t.ground.is_some() { 1 } else { 0 }
        }).unwrap_or(0))
    })?)?;

    methods.set("getTopVisibleThing", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(tile) = game.map.get_tile(pos) {
            let top_count = tile.get_top_item_count();
            if top_count > 0 {
                let item = &tile.items[top_count - 1];
                return Ok(LuaValue::Table(push_item_ref_attrs(lua, item.server_id, pos, (top_count - 1) as i32, item.unique_id, item.action_id)?));
            }
            if !tile.creature_ids.is_empty() {
                let cid = tile.creature_ids[0];
                return Ok(LuaValue::Table(push_creature_ref(lua, cid, "Creature")?));
            }
            if let Some(ground) = &tile.ground {
                return Ok(LuaValue::Table(push_item_ref_attrs(lua, ground.server_id, pos, -1, ground.unique_id, ground.action_id)?));
            }
        }
        Ok(LuaValue::Nil)
    })?)?;
    methods.set("getTopTopItem", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(tile) = game.map.get_tile(pos) {
            let top_count = tile.get_top_item_count();
            if top_count > 0 {
                let item = &tile.items[top_count - 1];
                return Ok(LuaValue::Table(push_item_ref_attrs(lua, item.server_id, pos, (top_count - 1) as i32, item.unique_id, item.action_id)?));
            }
        }
        Ok(LuaValue::Nil)
    })?)?;
    methods.set("getTopDownItem", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(tile) = game.map.get_tile(pos) {
            let top_count = tile.get_top_item_count();
            if top_count < tile.items.len() {
                let item = &tile.items[top_count];
                return Ok(LuaValue::Table(push_item_ref_attrs(lua, item.server_id, pos, top_count as i32, item.unique_id, item.action_id)?));
            }
        }
        Ok(LuaValue::Nil)
    })?)?;
    methods.set("getFieldItem", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(tile) = game.map.get_tile(pos) {
            for (idx, item) in tile.items.iter().enumerate() {
                let it = game.items.get_item_type(item.server_id as usize);
                if it.kind == crate::items::ItemKind::MagicField {
                    return Ok(LuaValue::Table(push_item_ref_attrs(lua, item.server_id, pos, idx as i32, item.unique_id, item.action_id)?));
                }
            }
        }
        Ok(LuaValue::Nil)
    })?)?;
    methods.set("getItemById", lua.create_function(|lua, (this, item_id, sub_type): (LuaTable, u16, Option<i32>)| -> LuaResult<LuaValue> {
        let pos = table_to_position(&this)?;
        let _sub = sub_type.unwrap_or(-1);
        let game = g_game().lock().unwrap();
        if let Some(tile) = game.map.get_tile(pos) {
            if let Some(ground) = &tile.ground {
                if ground.server_id == item_id {
                    return Ok(LuaValue::Table(push_item_ref_attrs(lua, ground.server_id, pos, -1, ground.unique_id, ground.action_id)?));
                }
            }
            for (idx, item) in tile.items.iter().enumerate() {
                if item.server_id == item_id {
                    return Ok(LuaValue::Table(push_item_ref_attrs(lua, item.server_id, pos, idx as i32, item.unique_id, item.action_id)?));
                }
            }
        }
        Ok(LuaValue::Nil)
    })?)?;
    methods.set("getItemByType", lua.create_function(|lua, (this, item_type): (LuaTable, u32)| -> LuaResult<LuaValue> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(tile) = game.map.get_tile(pos) {
            if let Some(ground) = &tile.ground {
                let it = game.items.get_item_type(ground.server_id as usize);
                if it.kind as u32 == item_type {
                    return Ok(LuaValue::Table(push_item_ref_attrs(lua, ground.server_id, pos, -1, ground.unique_id, ground.action_id)?));
                }
            }
            for (idx, item) in tile.items.iter().enumerate() {
                let it = game.items.get_item_type(item.server_id as usize);
                if it.kind as u32 == item_type {
                    return Ok(LuaValue::Table(push_item_ref_attrs(lua, item.server_id, pos, idx as i32, item.unique_id, item.action_id)?));
                }
            }
        }
        Ok(LuaValue::Nil)
    })?)?;
    methods.set("getItemByTopOrder", lua.create_function(|lua, (this, top_order): (LuaTable, u8)| -> LuaResult<LuaValue> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(tile) = game.map.get_tile(pos) {
            for (idx, item) in tile.items.iter().enumerate() {
                let it = game.items.get_item_type(item.server_id as usize);
                if it.always_on_top && it.always_on_top_order == top_order {
                    return Ok(LuaValue::Table(push_item_ref_attrs(lua, item.server_id, pos, idx as i32, item.unique_id, item.action_id)?));
                }
            }
        }
        Ok(LuaValue::Nil)
    })?)?;
    methods.set("getItemCountById", lua.create_function(|_, (this, item_id, sub_type): (LuaTable, u16, Option<i32>)| -> LuaResult<i32> {
        let pos = table_to_position(&this)?;
        let _sub = sub_type.unwrap_or(-1);
        let game = g_game().lock().unwrap();
        let mut count = 0i32;
        if let Some(tile) = game.map.get_tile(pos) {
            if let Some(ground) = &tile.ground {
                if ground.server_id == item_id {
                    count += 1;
                }
            }
            for item in &tile.items {
                if item.server_id == item_id {
                    count += 1;
                }
            }
        }
        Ok(count)
    })?)?;

    methods.set("getBottomCreature", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(tile) = game.map.get_tile(pos) {
            if let Some(&cid) = tile.get_creatures().last() {
                drop(game);
                return Ok(LuaValue::Table(push_creature_ref(lua, cid, "Creature")?));
            }
        }
        Ok(LuaValue::Nil)
    })?)?;

    methods.set("getTopCreature", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(tile) = game.map.get_tile(pos) {
            if let Some(&cid) = tile.get_creatures().first() {
                drop(game);
                return Ok(LuaValue::Table(push_creature_ref(lua, cid, "Creature")?));
            }
        }
        Ok(LuaValue::Nil)
    })?)?;

    methods.set("getBottomVisibleCreature", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(tile) = game.map.get_tile(pos) {
            if let Some(&cid) = tile.get_creatures().last() {
                drop(game);
                return Ok(LuaValue::Table(push_creature_ref(lua, cid, "Creature")?));
            }
        }
        Ok(LuaValue::Nil)
    })?)?;
    methods.set("getTopVisibleCreature", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(tile) = game.map.get_tile(pos) {
            if let Some(&cid) = tile.get_creatures().first() {
                drop(game);
                return Ok(LuaValue::Table(push_creature_ref(lua, cid, "Creature")?));
            }
        }
        Ok(LuaValue::Nil)
    })?)?;

    methods.set("getItems", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        let result = lua.create_table()?;
        if let Some(tile) = game.map.get_tile(pos) {
            let mut lua_idx = 1i64;
            if let Some(ground) = &tile.ground {
                result.raw_set(lua_idx, push_item_ref_attrs(lua, ground.server_id, pos, -1, ground.unique_id, ground.action_id)?)?;
                lua_idx += 1;
            }
            for (idx, item) in tile.items.iter().enumerate() {
                result.raw_set(lua_idx, push_item_ref_attrs(lua, item.server_id, pos, idx as i32, item.unique_id, item.action_id)?)?;
                lua_idx += 1;
            }
        }
        Ok(result)
    })?)?;

    methods.set("getThingIndex", lua.create_function(|_, (this, thing): (LuaTable, LuaTable)| -> LuaResult<i32> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(tile) = game.map.get_tile(pos) {
            if let Ok(cid) = get_creature_id(&thing) {
                for (idx, &c) in tile.creature_ids.iter().enumerate() {
                    if c == cid {
                        let offset = if tile.ground.is_some() { 1 } else { 0 } + tile.get_top_item_count();
                        return Ok((offset + idx) as i32);
                    }
                }
            }
        }
        Ok(-1)
    })?)?;

    methods.set("queryAdd", lua.create_function(|_, (this, _item, _flags): (LuaTable, LuaValue, Option<u32>)| -> LuaResult<i32> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        if game.map.get_tile(pos).is_some() {
            Ok(0)
        } else {
            Ok(1)
        }
    })?)?;

    methods.set("addItem", lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Ok(LuaValue::Nil),
        };
        let item_arg = iter.next().unwrap_or(LuaValue::Nil);
        let count = match iter.next() {
            Some(LuaValue::Integer(n)) => n as u16,
            _ => 1,
        };
        let pos = table_to_position(&this)?;
        let server_id: u16 = match &item_arg {
            LuaValue::Integer(n) => *n as u16,
            LuaValue::Number(n) => *n as u16,
            LuaValue::Table(t) => t.get::<u16>("_server_id").unwrap_or(0),
            _ => 0,
        };
        if server_id == 0 { return Ok(LuaValue::Nil); }

        let item = crate::map::tile::MapItem { server_id, count, ..Default::default() };

        let mut game = g_game().lock().unwrap();
        let items_arc = game.items.clone();
        if let Some(tile) = game.map.get_tile_mut(pos) {
            let idx = tile.items.len() as i32;
            tile.internal_add_item(item, &items_arc);
            drop(game);
            return Ok(LuaValue::Table(push_item_ref(lua, server_id, pos, idx)?));
        }
        Ok(LuaValue::Nil)
    })?)?;

    methods.set("addItemEx", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<i32> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Ok(1),
        };
        let item_arg = iter.next().unwrap_or(LuaValue::Nil);
        let pos = table_to_position(&this)?;
        let server_id: u16 = match &item_arg {
            LuaValue::Integer(n) => *n as u16,
            LuaValue::Number(n) => *n as u16,
            LuaValue::Table(t) => t.get::<u16>("_server_id").unwrap_or(0),
            _ => 0,
        };
        if server_id == 0 { return Ok(1); }

        let item = crate::map::tile::MapItem { server_id, ..Default::default() };

        let mut game = g_game().lock().unwrap();
        let items_arc = game.items.clone();
        if let Some(tile) = game.map.get_tile_mut(pos) {
            tile.internal_add_item(item, &items_arc);
            return Ok(0);
        }
        Ok(1)
    })?)?;

    methods.set("getHouse", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let pos = table_to_position(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(tile) = game.map.get_tile(pos) {
            if let TileKind::House { house_id } = tile.kind {
                let t = lua.create_table()?;
                t.raw_set(1, house_id)?;
                if let Ok(mt) = lua.named_registry_value::<LuaTable>("House") {
                    let _ = t.set_metatable(Some(mt));
                }
                return Ok(LuaValue::Table(t));
            }
        }
        Ok(LuaValue::Nil)
    })?)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// NetworkMessage class
// ---------------------------------------------------------------------------

fn register_network_message_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, _args: LuaMultiValue| -> LuaResult<LuaValue> {
        let msg = NetworkMessage::new();
        let ud = lua.create_any_userdata(std::cell::RefCell::new(msg))?;
        Ok(LuaValue::UserData(ud))
    })?;
    let methods = register_class(lua, "NetworkMessage", None, LUA_DATA_UNKNOWN, Some(ctor))?;
    set_meta(lua, "NetworkMessage", "__eq", lua.create_function(|_, (_a, _b): (LuaValue, LuaValue)| Ok(false))?)?;
    set_meta(lua, "NetworkMessage", "__gc", lua.create_function(|_, _: LuaValue| -> LuaResult<()> { Ok(()) })?)?;

    methods.set("delete", lua.create_function(|_, _: LuaValue| -> LuaResult<()> { Ok(()) })?)?;

    methods.set("getByte", lua.create_function(|_, this: LuaAnyUserData| -> LuaResult<u8> {
        let cell = this.borrow::<std::cell::RefCell<NetworkMessage>>()?;
        let v = cell.borrow_mut().get_byte();
        Ok(v)
    })?)?;

    methods.set("getU16", lua.create_function(|_, this: LuaAnyUserData| -> LuaResult<u16> {
        let cell = this.borrow::<std::cell::RefCell<NetworkMessage>>()?;
        let v = cell.borrow_mut().get_u16();
        Ok(v)
    })?)?;

    methods.set("getU32", lua.create_function(|_, this: LuaAnyUserData| -> LuaResult<u32> {
        let cell = this.borrow::<std::cell::RefCell<NetworkMessage>>()?;
        let v = cell.borrow_mut().get_u32();
        Ok(v)
    })?)?;

    methods.set("getU64", lua.create_function(|_, this: LuaAnyUserData| -> LuaResult<u64> {
        let cell = this.borrow::<std::cell::RefCell<NetworkMessage>>()?;
        let v = cell.borrow_mut().get_u64();
        Ok(v)
    })?)?;

    methods.set("getString", lua.create_function(|lua, this: LuaAnyUserData| -> LuaResult<LuaString> {
        let cell = this.borrow::<std::cell::RefCell<NetworkMessage>>()?;
        let bytes = cell.borrow_mut().get_string(None);
        drop(cell);
        lua.create_string(&bytes)
    })?)?;

    methods.set("getPosition", lua.create_function(|lua, this: LuaAnyUserData| -> LuaResult<LuaTable> {
        let cell = this.borrow::<std::cell::RefCell<NetworkMessage>>()?;
        let p = cell.borrow_mut().get_position();
        drop(cell);
        push_position(lua, Position { x: p.x, y: p.y, z: p.z })
    })?)?;

    methods.set("addByte", lua.create_function(|_, (this, val): (LuaAnyUserData, u8)| -> LuaResult<bool> {
        let cell = this.borrow::<std::cell::RefCell<NetworkMessage>>()?;
        cell.borrow_mut().add_byte(val);
        Ok(true)
    })?)?;

    methods.set("addU16", lua.create_function(|_, (this, val): (LuaAnyUserData, u16)| -> LuaResult<bool> {
        let cell = this.borrow::<std::cell::RefCell<NetworkMessage>>()?;
        cell.borrow_mut().add_u16(val);
        Ok(true)
    })?)?;

    methods.set("addU32", lua.create_function(|_, (this, val): (LuaAnyUserData, u32)| -> LuaResult<bool> {
        let cell = this.borrow::<std::cell::RefCell<NetworkMessage>>()?;
        cell.borrow_mut().add_u32(val);
        Ok(true)
    })?)?;

    methods.set("addU64", lua.create_function(|_, (this, val): (LuaAnyUserData, u64)| -> LuaResult<bool> {
        let cell = this.borrow::<std::cell::RefCell<NetworkMessage>>()?;
        cell.borrow_mut().add_u64(val);
        Ok(true)
    })?)?;

    methods.set("addString", lua.create_function(|_, (this, val): (LuaAnyUserData, LuaString)| -> LuaResult<bool> {
        let cell = this.borrow::<std::cell::RefCell<NetworkMessage>>()?;
        let bytes = val.as_bytes().to_vec();
        cell.borrow_mut().add_string(&bytes);
        Ok(true)
    })?)?;

    methods.set("addPosition", lua.create_function(|_, (this, pos): (LuaAnyUserData, LuaTable)| -> LuaResult<bool> {
        let p = table_to_position(&pos)?;
        let cell = this.borrow::<std::cell::RefCell<NetworkMessage>>()?;
        cell.borrow_mut().add_position(crate::net::message::Position { x: p.x, y: p.y, z: p.z });
        Ok(true)
    })?)?;

    methods.set("addDouble", lua.create_function(|_, (this, val): (LuaAnyUserData, f64)| -> LuaResult<bool> {
        let cell = this.borrow::<std::cell::RefCell<NetworkMessage>>()?;
        cell.borrow_mut().add_double(val, 2);
        Ok(true)
    })?)?;

    methods.set("addItem", lua.create_function(|_, (_this, _item): (LuaAnyUserData, LuaValue)| -> LuaResult<bool> {
        Ok(true)
    })?)?;

    methods.set("addItemId", lua.create_function(|_, (_this, _item_id): (LuaAnyUserData, LuaValue)| -> LuaResult<bool> {
        Ok(true)
    })?)?;

    methods.set("reset", lua.create_function(|_, this: LuaAnyUserData| -> LuaResult<bool> {
        let cell = this.borrow::<std::cell::RefCell<NetworkMessage>>()?;
        cell.borrow_mut().reset();
        Ok(true)
    })?)?;

    methods.set("seek", lua.create_function(|_, (this, pos): (LuaAnyUserData, u16)| -> LuaResult<bool> {
        let cell = this.borrow::<std::cell::RefCell<NetworkMessage>>()?;
        let v = cell.borrow_mut().set_buffer_position(pos);
        Ok(v)
    })?)?;

    methods.set("tell", lua.create_function(|_, this: LuaAnyUserData| -> LuaResult<u16> {
        let cell = this.borrow::<std::cell::RefCell<NetworkMessage>>()?;
        let pos = cell.borrow().get_buffer_position();
        drop(cell);
        Ok(pos - NetworkMessage::INITIAL_BUFFER_POSITION)
    })?)?;

    methods.set("len", lua.create_function(|_, this: LuaAnyUserData| -> LuaResult<u16> {
        let cell = this.borrow::<std::cell::RefCell<NetworkMessage>>()?;
        let v = cell.borrow().get_length();
        Ok(v)
    })?)?;

    methods.set("skipBytes", lua.create_function(|_, (this, count): (LuaAnyUserData, i16)| -> LuaResult<bool> {
        let cell = this.borrow::<std::cell::RefCell<NetworkMessage>>()?;
        cell.borrow_mut().skip_bytes(count);
        Ok(true)
    })?)?;

    methods.set("sendToPlayer", lua.create_function(|_, (_this, _player): (LuaAnyUserData, LuaValue)| -> LuaResult<bool> {
        Ok(true)
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Item class
// ---------------------------------------------------------------------------

fn register_item_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, (_mt, uid): (LuaValue, u32)| -> LuaResult<LuaValue> {
        if uid == 0 {
            return Ok(LuaValue::Nil);
        }
        let t = lua.create_table()?;
        t.raw_set(1, uid)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Item") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(LuaValue::Table(t))
    })?;
    let methods = register_class(lua, "Item", None, LUA_DATA_ITEM, Some(ctor))?;
    set_meta(lua, "Item", "__eq", lua.create_function(|_, (a, b): (LuaTable, LuaTable)| {
        let id_a: u32 = a.raw_get(1).unwrap_or(0);
        let id_b: u32 = b.raw_get(1).unwrap_or(0);
        Ok(id_a == id_b && id_a != 0)
    })?)?;

    methods.set("isItem", lua.create_function(|_, _this: LuaTable| Ok(true))?)?;
    methods.set("isContainer", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let uid: u32 = this.raw_get(1).unwrap_or(0);
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(uid as usize).kind == crate::items::ItemKind::Container)
    })?)?;
    methods.set("getParent", lua.create_function(|_, _this: LuaTable| -> LuaResult<LuaValue> { Ok(LuaValue::Nil) })?)?;
    methods.set("getTopParent", lua.create_function(|_, _this: LuaTable| -> LuaResult<LuaValue> { Ok(LuaValue::Nil) })?)?;
    methods.set("getType", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let uid: u32 = this.raw_get(1).unwrap_or(0);
        let t = lua.create_table()?;
        t.raw_set(1, uid)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("ItemType") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(LuaValue::Table(t))
    })?)?;
    methods.set("getId", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        this.raw_get::<u32>(1)
    })?)?;
    methods.set("clone", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let uid: u32 = this.raw_get(1)?;
        let t = lua.create_table()?;
        t.raw_set(1, uid)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Item") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(t)
    })?)?;
    methods.set("split", lua.create_function(|lua, (this, _count): (LuaTable, Option<u32>)| -> LuaResult<LuaTable> {
        let uid: u32 = this.raw_get(1)?;
        let t = lua.create_table()?;
        t.raw_set(1, uid)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Item") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(t)
    })?)?;
    methods.set("remove", lua.create_function(|_, (this, _count_opt): (LuaTable, Option<i32>)| -> LuaResult<bool> {
        // Handle inventory slot items (from getSlotItem).
        let owner_id: u32 = this.raw_get::<u32>("_owner_id").unwrap_or(0);
        if owner_id != 0 {
            let slot: usize = this.raw_get::<u32>("_slot").unwrap_or(0) as usize;
            if slot > 0 {
                let mut game = g_game().lock().unwrap();
                if let Some(player) = game.get_player_mut(owner_id) {
                    player.inventory[slot] = None;
                    player.inventory_count[slot] = 0;
                    if slot < player.inventory_items.len() {
                        player.inventory_items[slot] = None;
                    }
                }
                drop(game);
                crate::net::game_protocol::send_clear_inventory_slot(owner_id, slot as u8);
                return Ok(true);
            }
        }

        // Handle tile items (from map).
        let pos_x: Option<u16> = this.raw_get("_pos_x").ok();
        let pos_y: Option<u16> = this.raw_get("_pos_y").ok();
        let pos_z: Option<u8> = this.raw_get("_pos_z").ok();
        let idx: Option<i32> = this.raw_get("_idx").ok();

        if let (Some(x), Some(y), Some(z), Some(idx)) = (pos_x, pos_y, pos_z, idx) {
            let pos = Position { x, y, z };
            let mut game = g_game().lock().unwrap();
            if let Some(tile) = game.map.get_tile_mut(pos) {
                if idx == -1 {
                    tile.ground = None;
                } else if (idx as usize) < tile.items.len() {
                    tile.items.remove(idx as usize);
                }
            }
        }
        Ok(true)
    })?)?;
    methods.set("getUniqueId", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        this.raw_get::<u32>(1)
    })?)?;
    methods.set("getActionId", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let game = g_game().lock().unwrap();
        if let Some(item) = get_item_from_lua(&this, &game) {
            return Ok(item.action_id as u32);
        }
        Ok(0)
    })?)?;
    methods.set("setActionId", lua.create_function(|_, (this, aid): (LuaTable, u16)| -> LuaResult<bool> {
        let aid = aid.max(100);
        let mut game = g_game().lock().unwrap();
        if let Some(item) = get_item_from_lua_mut(&this, &mut game) {
            item.action_id = aid;
        }
        Ok(true)
    })?)?;
    methods.set("getCount", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let game = g_game().lock().unwrap();
        if let Some(item) = get_item_from_lua(&this, &game) {
            return Ok(item.count as u32);
        }
        Ok(1)
    })?)?;
    methods.set("getCharges", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let game = g_game().lock().unwrap();
        if let Some(item) = get_item_from_lua(&this, &game) {
            return Ok(item.charges as u32);
        }
        Ok(0)
    })?)?;
    methods.set("getFluidType", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let game = g_game().lock().unwrap();
        if let Some(item) = get_item_from_lua(&this, &game) {
            return Ok(item.fluid_type as u32);
        }
        Ok(0)
    })?)?;
    methods.set("getWeight", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let uid: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        let it = game.get_items().get_item_type(uid as usize);
        Ok(it.weight)
    })?)?;
    methods.set("getWorth", lua.create_function(|_, this: LuaTable| -> LuaResult<u64> {
        let uid: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        let it = game.get_items().get_item_type(uid as usize);
        Ok(it.worth)
    })?)?;
    methods.set("getSubType", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let uid: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        let it = game.get_items().get_item_type(uid as usize);
        let is_fluid = matches!(it.group, crate::items::ItemGroup::Fluid | crate::items::ItemGroup::Splash);
        if is_fluid || it.stackable {
            if let Some(item) = get_item_from_lua(&this, &game) {
                return Ok(item.count as u32);
            }
        }
        if it.charges > 0 {
            if let Some(item) = get_item_from_lua(&this, &game) {
                return Ok(item.charges as u32);
            }
        }
        if let Some(item) = get_item_from_lua(&this, &game) {
            return Ok(item.count as u32);
        }
        Ok(1)
    })?)?;
    methods.set("getName", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaString> {
        let uid: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        let it = game.get_items().get_item_type(uid as usize);
        lua.create_string(&it.name)
    })?)?;
    methods.set("getPluralName", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaString> {
        let uid: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        let it = game.get_items().get_item_type(uid as usize);
        lua.create_string(&it.plural_name)
    })?)?;
    methods.set("getArticle", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaString> {
        let uid: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        let it = game.get_items().get_item_type(uid as usize);
        lua.create_string(&it.article)
    })?)?;
    methods.set("getPosition", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let pos_x: Option<u16> = this.raw_get("_pos_x").ok();
        let pos_y: Option<u16> = this.raw_get("_pos_y").ok();
        let pos_z: Option<u8> = this.raw_get("_pos_z").ok();
        let pos = match (pos_x, pos_y, pos_z) {
            (Some(x), Some(y), Some(z)) => Position { x, y, z },
            _ => Position { x: 0, y: 0, z: 0 },
        };
        push_position(lua, pos)
    })?)?;
    methods.set("getTile", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let pos_x: Option<u16> = this.raw_get("_pos_x").ok();
        let pos_y: Option<u16> = this.raw_get("_pos_y").ok();
        let pos_z: Option<u8> = this.raw_get("_pos_z").ok();
        if let (Some(x), Some(y), Some(z)) = (pos_x, pos_y, pos_z) {
            let pos = Position { x, y, z };
            let t = push_position(lua, pos)?;
            if let Ok(mt) = lua.named_registry_value::<LuaTable>("Tile") {
                let _ = t.set_metatable(Some(mt));
            }
            return Ok(LuaValue::Table(t));
        }
        Ok(LuaValue::Nil)
    })?)?;
    methods.set("hasAttribute", lua.create_function(|_, (this, key): (LuaTable, LuaValue)| -> LuaResult<bool> {
        let game = g_game().lock().unwrap();
        let item = get_item_from_lua(&this, &game);
        let Some(item) = item else { return Ok(false); };
        let attr_name = match &key {
            LuaValue::String(s) => s.to_string_lossy().to_lowercase(),
            LuaValue::Integer(n) => return Ok(item_has_attr_by_id(item, *n as u32)),
            _ => return Ok(false),
        };
        Ok(item_has_attr_by_name(item, &attr_name))
    })?)?;
    methods.set("getAttribute", lua.create_function(|lua, (this, key): (LuaTable, LuaValue)| -> LuaResult<LuaValue> {
        let game = g_game().lock().unwrap();
        let item = get_item_from_lua(&this, &game);
        let Some(item) = item else { return Ok(LuaValue::Nil); };
        let attr_name = match &key {
            LuaValue::String(s) => s.to_string_lossy().to_lowercase(),
            LuaValue::Integer(n) => return item_get_attr_by_id(lua, item, *n as u32),
            _ => return Ok(LuaValue::Nil),
        };
        item_get_attr_by_name(lua, item, &attr_name)
    })?)?;
    methods.set("setAttribute", lua.create_function(|_, (this, key, val): (LuaTable, LuaValue, LuaValue)| -> LuaResult<bool> {
        let mut game = g_game().lock().unwrap();
        let item = get_item_from_lua_mut(&this, &mut game);
        let Some(item) = item else { return Ok(false); };
        let attr_name = match &key {
            LuaValue::String(s) => s.to_string_lossy().to_lowercase(),
            LuaValue::Integer(n) => { item_set_attr_by_id(item, *n as u32, &val); return Ok(true); }
            _ => return Ok(false),
        };
        item_set_attr_by_name(item, &attr_name, &val);
        Ok(true)
    })?)?;
    methods.set("removeAttribute", lua.create_function(|_, (this, key): (LuaTable, LuaValue)| -> LuaResult<bool> {
        let mut game = g_game().lock().unwrap();
        let item = get_item_from_lua_mut(&this, &mut game);
        let Some(item) = item else { return Ok(false); };
        let attr_name = match &key {
            LuaValue::String(s) => s.to_string_lossy().to_lowercase(),
            LuaValue::Integer(n) => { item_remove_attr_by_id(item, *n as u32); return Ok(true); }
            _ => return Ok(false),
        };
        item_remove_attr_by_name(item, &attr_name);
        Ok(true)
    })?)?;
    methods.set("getCustomAttribute", lua.create_function(|lua, (this, key): (LuaTable, LuaValue)| -> LuaResult<LuaValue> {
        let game = g_game().lock().unwrap();
        let item = get_item_from_lua(&this, &game);
        let Some(item) = item else { return Ok(LuaValue::Nil); };
        let key_str = match &key {
            LuaValue::String(s) => s.to_string_lossy().to_lowercase(),
            LuaValue::Integer(n) => n.to_string(),
            _ => return Ok(LuaValue::Nil),
        };
        match item.custom_attributes.get(&key_str) {
            Some(crate::map::tile::CustomAttributeValue::String(s)) => Ok(LuaValue::String(lua.create_string(s)?)),
            Some(crate::map::tile::CustomAttributeValue::Int(n)) => Ok(LuaValue::Integer(*n)),
            Some(crate::map::tile::CustomAttributeValue::Double(d)) => Ok(LuaValue::Number(*d)),
            Some(crate::map::tile::CustomAttributeValue::Bool(b)) => Ok(LuaValue::Boolean(*b)),
            None => Ok(LuaValue::Nil),
        }
    })?)?;
    methods.set("setCustomAttribute", lua.create_function(|_, (this, key, val): (LuaTable, LuaValue, LuaValue)| -> LuaResult<bool> {
        let mut game = g_game().lock().unwrap();
        let item = get_item_from_lua_mut(&this, &mut game);
        let Some(item) = item else { return Ok(false); };
        let key_str = match &key {
            LuaValue::String(s) => s.to_string_lossy().to_lowercase(),
            LuaValue::Integer(n) => n.to_string(),
            _ => return Ok(false),
        };
        let custom_val = match val {
            LuaValue::String(s) => crate::map::tile::CustomAttributeValue::String(s.to_string_lossy().to_string()),
            LuaValue::Integer(n) => crate::map::tile::CustomAttributeValue::Int(n),
            LuaValue::Number(n) => crate::map::tile::CustomAttributeValue::Double(n),
            LuaValue::Boolean(b) => crate::map::tile::CustomAttributeValue::Bool(b),
            _ => return Ok(false),
        };
        item.custom_attributes.insert(key_str, custom_val);
        Ok(true)
    })?)?;
    methods.set("removeCustomAttribute", lua.create_function(|_, (this, key): (LuaTable, LuaValue)| -> LuaResult<bool> {
        let mut game = g_game().lock().unwrap();
        let item = get_item_from_lua_mut(&this, &mut game);
        let Some(item) = item else { return Ok(false); };
        let key_str = match &key {
            LuaValue::String(s) => s.to_string_lossy().to_lowercase(),
            LuaValue::Integer(n) => n.to_string(),
            _ => return Ok(false),
        };
        Ok(item.custom_attributes.remove(&key_str).is_some())
    })?)?;
    methods.set("moveTo", lua.create_function(|_, (this, dest, _flags): (LuaTable, LuaValue, Option<u32>)| -> LuaResult<bool> {
        use crate::map::tile::MapItem;
        // Source: player inventory slot (has _owner_id + _slot).
        let owner_id: u32 = this.raw_get::<u32>("_owner_id").unwrap_or(0);
        let slot: usize = this.raw_get::<u32>("_slot").unwrap_or(0) as usize;
        if owner_id == 0 || slot == 0 {
            return Ok(false);
        }

        let dest_tbl = match &dest {
            LuaValue::Table(t) => t,
            _ => return Ok(false),
        };
        let dest_pos_x: u16 = dest_tbl.raw_get::<u16>("_pos_x").unwrap_or(0);
        let dest_pos_y: u16 = dest_tbl.raw_get::<u16>("_pos_y").unwrap_or(0);
        let dest_pos_z: u8 = dest_tbl.raw_get::<u8>("_pos_z").unwrap_or(0);
        let dest_idx: i32 = dest_tbl.raw_get::<i32>("_idx").unwrap_or(-2);
        if dest_pos_x == 0 && dest_pos_y == 0 && dest_pos_z == 0 {
            return Ok(false);
        }
        let dest_pos = Position { x: dest_pos_x, y: dest_pos_y, z: dest_pos_z };

        let mut game = g_game().lock().unwrap();

        // Read item from player slot (use full tree if available for container contents).
        let child = {
            let Some(player) = game.get_player(owner_id) else { return Ok(false); };
            if let Some(Some(full_item)) = player.inventory_items.get(slot) {
                full_item.clone()
            } else {
                match player.inventory.get(slot).copied().flatten() {
                    Some(sid) => {
                        let count = player.inventory_count.get(slot).copied().unwrap_or(1);
                        MapItem { server_id: sid, count, ..MapItem::default() }
                    }
                    None => return Ok(false),
                }
            }
        };
        let added = if dest_idx >= 0 {
            let Some(tile) = game.map.get_tile_mut(dest_pos) else { return Ok(false); };
            let Some(container) = tile.items.get_mut(dest_idx as usize) else { return Ok(false); };
            container.children.push(child);
            true
        } else {
            false
        };
        if !added { return Ok(false); }

        // Remove item from player slot.
        if let Some(player) = game.get_player_mut(owner_id) {
            player.inventory[slot] = None;
            player.inventory_count[slot] = 0;
            if slot < player.inventory_items.len() {
                player.inventory_items[slot] = None;
            }
        }
        drop(game);

        crate::net::game_protocol::send_clear_inventory_slot(owner_id, slot as u8);
        Ok(true)
    })?)?;
    methods.set("transform", lua.create_function(|_, (this, id_val, sub_type): (LuaTable, LuaValue, Option<i32>)| -> LuaResult<bool> {
        let new_id: u16 = match id_val {
            LuaValue::Integer(n) => n as u16,
            LuaValue::Number(n) => n as u16,
            LuaValue::String(s) => {
                let name = s.to_string_lossy();
                let game = g_game().lock().unwrap();
                game.items.get_item_id_by_name(&name).unwrap_or(0)
            }
            _ => return Ok(false),
        };
        if new_id == 0 {
            return Ok(false);
        }

        let pos_x: Option<u16> = this.raw_get("_pos_x").ok();
        let pos_y: Option<u16> = this.raw_get("_pos_y").ok();
        let pos_z: Option<u8> = this.raw_get("_pos_z").ok();
        let idx: Option<i32> = this.raw_get("_idx").ok();

        // Inventory item (pos.x == 0xFFFF): transform the owning player's
        // equipment slot and push a 0x78 slot update, rather than a tile update.
        if pos_x == Some(0xFFFF) {
            let slot = pos_y.unwrap_or(0) as usize;
            let owner: Option<u32> = this.raw_get("_owner_cid").ok();
            if let (Some(cid), true) = (
                owner,
                (crate::creatures::player::CONST_SLOT_HEAD..=crate::creatures::player::CONST_SLOT_LAST)
                    .contains(&slot),
            ) {
                let new_count = sub_type.and_then(|s| if s >= 0 { Some(s as u16) } else { None });
                let mut game = g_game().lock().unwrap();
                let items_arc = game.items.clone();
                let mut sent_count = 1u8;
                let mut light_update: Option<(crate::map::Position, crate::creatures::LightInfo)> = None;
                if let Some(player) = game.get_player_mut(cid) {
                    if player.inventory[slot].is_some() {
                        player.inventory[slot] = Some(new_id);
                        if let Some(c) = new_count {
                            player.inventory_count[slot] = c.max(1);
                        }
                        if let Some(it) = player.inventory_items[slot].as_mut() {
                            it.server_id = new_id;
                            if let Some(c) = new_count { it.count = c; }
                        }
                        sent_count = player.inventory_count[slot].max(1) as u8;
                        if player.update_items_light() {
                            light_update = Some((player.base.position, player.get_creature_light()));
                        }
                    }
                }
                drop(game);
                crate::net::game_protocol::send_inventory_slot_update(cid, slot as u8, new_id, sent_count, &items_arc);
                if let Some((pos, light)) = light_update {
                    crate::net::game_protocol::broadcast_creature_light(cid, pos, light);
                }
            }
            this.raw_set(1, new_id as i64)?;
            return Ok(true);
        }

        if let (Some(x), Some(y), Some(z), Some(idx)) = (pos_x, pos_y, pos_z, idx) {
            let pos = Position { x, y, z };
            let mut game = g_game().lock().unwrap();
            let items_arc = game.items.clone();
            if let Some(tile) = game.map.get_tile_mut(pos) {
                let count_val = sub_type.unwrap_or(1).max(1) as u8;
                let new_count = sub_type.and_then(|s| if s >= 0 { Some(s as u16) } else { None });

                if idx == -1 {
                    if let Some(ground) = &mut tile.ground {
                        ground.server_id = new_id;
                        if let Some(c) = new_count { ground.count = c; }
                    }
                    tile.recalculate_flags(&items_arc);
                    drop(game);
                    crate::net::game_protocol::broadcast_tile_item_transform(
                        pos, 0, new_id, count_val, &items_arc,
                    );
                } else if (idx as usize) < tile.items.len() {
                    let old_aot = items_arc
                        .get_item_type(tile.items[idx as usize].server_id as usize)
                        .always_on_top;
                    let new_aot = items_arc.get_item_type(new_id as usize).always_on_top;
                    if old_aot != new_aot {
                        // C++ Game::transformItem: alwaysOnTop changed (down<->top)
                        // => remove the old item and re-add the new so it lands
                        // in the correct partition; tell the client via remove+add.
                        if let Some((old_sp, new_sp)) =
                            tile.repartition_transform(idx as usize, new_id, new_count, &items_arc)
                        {
                            drop(game);
                            crate::net::game_protocol::broadcast_tile_item_repartition(
                                pos, old_sp, new_sp, new_id, count_val, &items_arc,
                            );
                        }
                    } else {
                        let item = &mut tile.items[idx as usize];
                        item.server_id = new_id;
                        if let Some(c) = new_count { item.count = c; }
                        tile.recalculate_flags(&items_arc);
                        let stackpos = tile.item_client_stackpos(idx as usize);
                        drop(game);
                        crate::net::game_protocol::broadcast_tile_item_transform(
                            pos, stackpos, new_id, count_val, &items_arc,
                        );
                    }
                }
            }
        }

        this.raw_set(1, new_id as i64)?;
        Ok(true)
    })?)?;
    methods.set("decay", lua.create_function(|_, _this: LuaTable| -> LuaResult<bool> {
        Ok(true)
    })?)?;
    methods.set("getDescription", lua.create_function(|lua, (this, dist): (LuaTable, Option<i32>)| -> LuaResult<LuaString> {
        let uid: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        let it = game.get_items().get_item_type(uid as usize);
        let look_distance = dist.unwrap_or(0);
        let count = this.raw_get::<i64>("_count").unwrap_or(1).max(1) as u32;
        let desc = crate::items::description::get_item_description(it, look_distance, count);
        drop(game);
        lua.create_string(&desc)
    })?)?;
    methods.set("getSpecialDescription", lua.create_function(|lua, _this: LuaTable| -> LuaResult<LuaString> {
        lua.create_string("")
    })?)?;
    methods.set("hasProperty", lua.create_function(|_, (this, prop): (LuaTable, u32)| -> LuaResult<bool> {
        let uid: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        let it = game.get_items().get_item_type(uid as usize);
        let has_uid = get_item_from_lua(&this, &game).map(|i| i.unique_id != 0).unwrap_or(false);
        let result = match prop {
            0 => it.block_solid,
            1 => it.has_height,
            2 => it.block_projectile,
            3 => it.block_path_find,
            4 => it.is_vertical,
            5 => it.is_horizontal,
            6 => it.moveable && !has_uid,
            7 => it.block_solid && (!it.moveable || has_uid),
            8 => it.block_path_find && (!it.moveable || has_uid),
            9 => it.kind != crate::items::ItemKind::MagicField && it.block_path_find && (!it.moveable || has_uid),
            10 => it.kind != crate::items::ItemKind::MagicField && it.block_path_find,
            11 => it.is_horizontal || it.is_vertical,
            _ => false,
        };
        Ok(result)
    })?)?;
    methods.set("isLoadedFromMap", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let game = g_game().lock().unwrap();
        Ok(get_item_from_lua(&this, &game).map(|i| i.loaded_from_map).unwrap_or(false))
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Container class (extends Item)
// ---------------------------------------------------------------------------

fn register_container_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, (_mt, uid): (LuaValue, u32)| -> LuaResult<LuaValue> {
        if uid == 0 { return Ok(LuaValue::Nil); }
        let t = lua.create_table()?;
        t.raw_set(1, uid)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Container") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(LuaValue::Table(t))
    })?;
    let methods = register_class(lua, "Container", Some("Item"), LUA_DATA_CONTAINER, Some(ctor))?;
    set_meta(lua, "Container", "__eq", lua.create_function(|_, (a, b): (LuaTable, LuaTable)| {
        let id_a: u32 = a.raw_get(1).unwrap_or(0);
        let id_b: u32 = b.raw_get(1).unwrap_or(0);
        Ok(id_a == id_b && id_a != 0)
    })?)?;

    methods.set("getSize", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let children: Option<LuaTable> = this.raw_get("_children").ok();
        Ok(children.map(|t| t.raw_len() as u32).unwrap_or(0))
    })?)?;
    methods.set("getCapacity", lua.create_function(|_, this: LuaTable| -> LuaResult<u16> {
        let uid: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        let it = game.get_items().get_item_type(uid as usize);
        Ok(it.max_items)
    })?)?;
    methods.set("getEmptySlots", lua.create_function(|_, (this, _recursive): (LuaTable, Option<bool>)| -> LuaResult<u32> {
        let uid: u32 = this.raw_get(1).unwrap_or(0);
        let capacity = {
            let game = g_game().lock().unwrap();
            game.get_items().get_item_type(uid as usize).max_items as u32
        };
        let children: Option<LuaTable> = this.raw_get("_children").ok();
        let size = children.map(|t| t.raw_len() as u32).unwrap_or(0);
        Ok(capacity.saturating_sub(size))
    })?)?;
    methods.set("getContentDescription", lua.create_function(|lua, _this: LuaTable| -> LuaResult<LuaString> {
        lua.create_string("")
    })?)?;
    methods.set("getItems", lua.create_function(|lua, (_this, _recursive): (LuaTable, Option<bool>)| -> LuaResult<LuaTable> {
        lua.create_table()
    })?)?;
    methods.set("getItemHoldingCount", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let children: Option<LuaTable> = this.raw_get("_children").ok();
        Ok(children.map(|t| t.raw_len() as u32).unwrap_or(0))
    })?)?;
    methods.set("getItemCountById", lua.create_function(|_, (this, id, _sub): (LuaTable, u16, Option<i32>)| -> LuaResult<u32> {
        let children: Option<LuaTable> = this.raw_get("_children").ok();
        let Some(ch) = children else { return Ok(0); };
        let mut count = 0u32;
        for v in ch.sequence_values::<LuaTable>().flatten() {
            let child_id: u16 = v.raw_get(1).unwrap_or(0);
            if child_id == id {
                let c: u16 = v.raw_get("_count").unwrap_or(1);
                count += c as u32;
            }
        }
        Ok(count)
    })?)?;
    methods.set("getItem", lua.create_function(|_, (this, idx): (LuaTable, u32)| -> LuaResult<LuaValue> {
        let children: Option<LuaTable> = this.raw_get("_children").ok();
        let Some(ch) = children else { return Ok(LuaValue::Nil); };
        Ok(ch.raw_get(idx + 1).unwrap_or(LuaValue::Nil))
    })?)?;
    methods.set("hasItem", lua.create_function(|_, (this, item): (LuaTable, LuaValue)| -> LuaResult<bool> {
        let children: Option<LuaTable> = this.raw_get("_children").ok();
        let Some(ch) = children else { return Ok(false); };
        let search_id: u16 = match item {
            LuaValue::Table(t) => t.raw_get::<u16>(1).unwrap_or(0),
            LuaValue::Integer(n) => n as u16,
            _ => return Ok(false),
        };
        for v in ch.sequence_values::<LuaTable>().flatten() {
            let child_id: u16 = v.raw_get(1).unwrap_or(0);
            if child_id == search_id { return Ok(true); }
        }
        Ok(false)
    })?)?;
    methods.set("addItem", lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let mut iter = args.into_iter();
        let this = match iter.next() { Some(LuaValue::Table(t)) => t, _ => return Ok(LuaValue::Nil) };
        let item_id: u16 = match iter.next() {
            Some(LuaValue::Integer(n)) => n as u16,
            Some(LuaValue::Number(n)) => n as u16,
            _ => return Ok(LuaValue::Nil),
        };
        let count: u16 = match iter.next() {
            Some(LuaValue::Integer(n)) => n as u16,
            Some(LuaValue::Number(n)) => n as u16,
            _ => 1,
        };
        let stackable = crate::items::g_items().get_item_type(item_id as usize).stackable;
        let capped = if stackable { count.min(100) } else { count };
        let item_t = lua.create_table()?;
        item_t.raw_set(1, item_id)?;
        item_t.raw_set("_count", capped)?;
        item_t.raw_set("_pos_x", 0u16)?;
        item_t.raw_set("_pos_y", 0u16)?;
        item_t.raw_set("_pos_z", 0u8)?;
        item_t.raw_set("_idx", -1i32)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Item") {
            let _ = item_t.set_metatable(Some(mt));
        }
        let children: LuaTable = this.raw_get("_children").unwrap_or_else(|_| lua.create_table().unwrap());
        let len = children.raw_len();
        children.raw_set(len + 1, item_t.clone())?;
        this.raw_set("_children", children)?;
        Ok(LuaValue::Table(item_t))
    })?)?;
    methods.set("addItemEx", lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<u32> {
        let mut iter = args.into_iter();
        let this = match iter.next() { Some(LuaValue::Table(t)) => t, _ => return Ok(1) };
        let item_tbl = match iter.next() { Some(LuaValue::Table(t)) => t, _ => return Ok(1) };
        let children: LuaTable = this.raw_get("_children").unwrap_or_else(|_| lua.create_table().unwrap());
        let len = children.raw_len();
        children.raw_set(len + 1, item_tbl)?;
        this.raw_set("_children", children)?;
        Ok(0)
    })?)?;
    methods.set("getCorpseOwner", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let pos_x: u16 = this.raw_get("_pos_x").unwrap_or(0);
        let pos_y: u16 = this.raw_get("_pos_y").unwrap_or(0);
        let pos_z: u8 = this.raw_get("_pos_z").unwrap_or(0);
        let idx: i32 = this.raw_get("_idx").unwrap_or(-2);
        if idx < 0 { return Ok(0); }
        let pos = Position { x: pos_x, y: pos_y, z: pos_z };
        let game = g_game().lock().unwrap();
        if let Some(tile) = game.map.get_tile(pos) {
            if let Some(item) = tile.items.get(idx as usize) {
                return Ok(item.owner_id);
            }
        }
        Ok(0)
    })?)?;
    methods.set("createLootItem", lua.create_function(|_, _args: LuaMultiValue| -> LuaResult<LuaValue> {
        Ok(LuaValue::Nil)
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Teleport class (extends Item)
// ---------------------------------------------------------------------------

fn register_teleport_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, (_mt, uid): (LuaValue, u32)| -> LuaResult<LuaValue> {
        if uid == 0 { return Ok(LuaValue::Nil); }
        let t = lua.create_table()?;
        t.raw_set(1, uid)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Teleport") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(LuaValue::Table(t))
    })?;
    let methods = register_class(lua, "Teleport", Some("Item"), LUA_DATA_TELEPORT, Some(ctor))?;
    set_meta(lua, "Teleport", "__eq", lua.create_function(|_, (a, b): (LuaTable, LuaTable)| {
        let id_a: u32 = a.raw_get(1).unwrap_or(0);
        let id_b: u32 = b.raw_get(1).unwrap_or(0);
        Ok(id_a == id_b && id_a != 0)
    })?)?;

    methods.set("getDestination", lua.create_function(|lua, _this: LuaTable| -> LuaResult<LuaTable> {
        push_position(lua, Position { x: 0, y: 0, z: 0 })
    })?)?;
    methods.set("setDestination", lua.create_function(|_, (_this, _pos): (LuaTable, LuaTable)| -> LuaResult<bool> {
        Ok(true)
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Creature class
// ---------------------------------------------------------------------------

fn register_creature_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let arg = args.into_iter().nth(1);
        let game = g_game().lock().unwrap();
        let cid = match arg {
            Some(LuaValue::Integer(id)) => {
                if game.get_creature(id as u32).is_some() { Some(id as u32) } else { None }
            }
            Some(LuaValue::String(s)) => {
                let name = s.to_string_lossy();
                game.get_creature_by_name(&name).map(|c| c.id())
            }
            _ => None,
        };
        match cid {
            Some(id) => {
                drop(game);
                Ok(LuaValue::Table(push_creature_ref(lua, id, "Creature")?))
            }
            None => Ok(LuaValue::Nil),
        }
    })?;
    let methods = register_class(lua, "Creature", None, LUA_DATA_UNKNOWN, Some(ctor))?;

    set_meta(lua, "Creature", "__eq", lua.create_function(|_, (a, b): (LuaTable, LuaTable)| {
        let id_a: u32 = a.raw_get(1).unwrap_or(0);
        let id_b: u32 = b.raw_get(1).unwrap_or(0);
        Ok(id_a == id_b && id_a != 0)
    })?)?;

    methods.set("isCreature", lua.create_function(|_, _: LuaTable| Ok(true))?)?;

    methods.set("isRemoved", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        Ok(g_game().lock().unwrap().get_creature(cid).is_none())
    })?)?;

    methods.set("isInGhostMode", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_creature(cid).map(|c| c.is_in_ghost_mode()).unwrap_or(false))
    })?)?;

    methods.set("isHealthHidden", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_creature(cid).map(|c| c.base().hidden_health).unwrap_or(false))
    })?)?;

    methods.set("isMovementBlocked", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_creature(cid).map(|c| c.base().movement_blocked).unwrap_or(false))
    })?)?;

    methods.set("isImmune", lua.create_function(|_, (this, arg): (LuaTable, LuaValue)| -> LuaResult<LuaValue> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        let Some(creature) = game.get_creature(cid) else { return Ok(LuaValue::Nil); };
        match arg {
            LuaValue::Integer(n) => {
                let flags = creature.get_condition_immunities();
                Ok(LuaValue::Boolean((flags & n as u32) != 0))
            }
            LuaValue::Number(n) => {
                let flags = creature.get_condition_immunities();
                Ok(LuaValue::Boolean((flags & n as u32) != 0))
            }
            _ => Ok(LuaValue::Nil),
        }
    })?)?;

    methods.set("canSee", lua.create_function(|_, (this, pos_tbl): (LuaTable, LuaTable)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let target = table_to_position(&pos_tbl)?;
        let game = g_game().lock().unwrap();
        let my_pos = match game.get_creature(cid) {
            Some(c) => c.position(),
            None => return Ok(false),
        };
        let dx = (my_pos.x as i32 - target.x as i32).unsigned_abs();
        let dy = (my_pos.y as i32 - target.y as i32).unsigned_abs();
        let dz = (my_pos.z as i32 - target.z as i32).unsigned_abs();
        Ok(dx <= crate::map::MAX_VIEWPORT_X as u32 && dy <= crate::map::MAX_VIEWPORT_Y as u32 && dz <= 2)
    })?)?;

    methods.set("canSeeCreature", lua.create_function(|_, (this, other): (LuaTable, LuaTable)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let other_cid = get_creature_id(&other)?;
        let game = g_game().lock().unwrap();
        let other_creature = game.get_creature(other_cid);
        let other_is_ghost = other_creature
            .and_then(|c| c.as_player())
            .map(|p| p.is_ghost_mode)
            .unwrap_or(false);
        if !other_is_ghost {
            return Ok(true);
        }
        let observer = game.get_creature(cid);
        let can_see_ghost = observer
            .and_then(|c| c.as_player())
            .map(|p| p.group_flags & crate::creatures::player::PLAYER_FLAG_CAN_SENSE_INVISIBILITY != 0)
            .unwrap_or(false);
        Ok(can_see_ghost)
    })?)?;

    methods.set("canSeeGhostMode", lua.create_function(|_, (this, _other): (LuaTable, LuaTable)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature(cid) {
            if let Some(player) = creature.as_player() {
                return Ok(player.group_flags & crate::creatures::player::PLAYER_FLAG_CAN_SENSE_INVISIBILITY != 0);
            }
        }
        Ok(false)
    })?)?;

    methods.set("canSeeInvisibility", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature(cid) {
            if let Some(player) = creature.as_player() {
                return Ok(player.group_flags & crate::creatures::player::PLAYER_FLAG_CAN_SENSE_INVISIBILITY != 0);
            }
        }
        Ok(false)
    })?)?;

    methods.set("getId", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        get_creature_id(&this)
    })?)?;

    methods.set("getName", lua.create_function(|_, this: LuaTable| -> LuaResult<String> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(match game.get_creature(cid) {
            Some(crate::creatures::Creature::Player(p)) => p.name.clone(),
            Some(crate::creatures::Creature::Monster(m)) => m.get_name().to_owned(),
            Some(crate::creatures::Creature::Npc(n)) => n.get_name().to_owned(),
            None => String::new(),
        })
    })?)?;

    methods.set("getPosition", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        let pos = game.get_creature(cid).map(|c| c.position()).unwrap_or_default();
        push_position(lua, pos)
    })?)?;

    methods.set("getDirection", lua.create_function(|_, this: LuaTable| -> LuaResult<u8> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_creature(cid).map(|c| c.base().direction as u8).unwrap_or(0))
    })?)?;

    methods.set("setDirection", lua.create_function(|_, (this, dir): (LuaTable, u8)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature_mut(cid) {
            if let Some(d) = crate::creatures::Direction::from_u8(dir) {
                creature.base_mut().direction = d;
                return Ok(true);
            }
        }
        Ok(false)
    })?)?;

    methods.set("getHealth", lua.create_function(|_, this: LuaTable| -> LuaResult<i32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_creature(cid).map(|c| c.get_health()).unwrap_or(0))
    })?)?;

    methods.set("setHealth", lua.create_function(|_, (this, hp): (LuaTable, i32)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature_mut(cid) {
            creature.base_mut().health = hp;
        }
        Ok(())
    })?)?;

    methods.set("addHealth", lua.create_function(|_, (this, hp): (LuaTable, i32)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature_mut(cid) {
            let b = creature.base_mut();
            b.health = (b.health + hp).max(0).min(b.health_max);
        }
        Ok(())
    })?)?;

    methods.set("getMaxHealth", lua.create_function(|_, this: LuaTable| -> LuaResult<i32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_creature(cid).map(|c| c.get_max_health()).unwrap_or(0))
    })?)?;

    methods.set("setMaxHealth", lua.create_function(|_, (this, mhp): (LuaTable, i32)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature_mut(cid) {
            creature.base_mut().health_max = mhp;
        }
        Ok(())
    })?)?;

    methods.set("setHiddenHealth", lua.create_function(|_, (this, hidden): (LuaTable, bool)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature_mut(cid) {
            creature.base_mut().hidden_health = hidden;
        }
        Ok(())
    })?)?;

    methods.set("setMovementBlocked", lua.create_function(|_, (this, blocked): (LuaTable, bool)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature_mut(cid) {
            creature.base_mut().movement_blocked = blocked;
        }
        Ok(())
    })?)?;

    methods.set("getSpeed", lua.create_function(|_, this: LuaTable| -> LuaResult<i32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_creature(cid).map(|c| c.get_speed()).unwrap_or(0))
    })?)?;

    methods.set("getBaseSpeed", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_creature(cid).map(|c| c.base().base_speed).unwrap_or(0))
    })?)?;

    methods.set("changeSpeed", lua.create_function(|_, (this, delta): (LuaTable, i32)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature_mut(cid) {
            creature.base_mut().var_speed += delta;
        }
        Ok(())
    })?)?;

    methods.set("getLight", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        let light = game.get_creature(cid).map(|c| c.base().internal_light).unwrap_or_default();
        let t = lua.create_table()?;
        t.set("level", light.level)?;
        t.set("color", light.color)?;
        Ok(t)
    })?)?;

    methods.set("setLight", lua.create_function(|_, (this, light_tbl): (LuaTable, LuaTable)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let level: u8 = light_tbl.get("level")?;
        let color: u8 = light_tbl.get("color")?;
        let mut game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature_mut(cid) {
            creature.base_mut().internal_light = crate::creatures::LightInfo::new(level, color);
        }
        Ok(())
    })?)?;

    methods.set("getSkull", lua.create_function(|_, this: LuaTable| -> LuaResult<u8> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_creature(cid).map(|c| c.get_skull() as u8).unwrap_or(0))
    })?)?;

    methods.set("setSkull", lua.create_function(|_, (this, skull): (LuaTable, u8)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature_mut(cid) {
            creature.base_mut().skull = match skull {
                1 => crate::creatures::Skull::Yellow,
                2 => crate::creatures::Skull::Green,
                3 => crate::creatures::Skull::White,
                4 => crate::creatures::Skull::Red,
                5 => crate::creatures::Skull::Black,
                6 => crate::creatures::Skull::Orange,
                _ => crate::creatures::Skull::None,
            };
        }
        Ok(())
    })?)?;

    methods.set("getOutfit", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        let outfit = game.get_creature(cid).map(|c| c.base().current_outfit).unwrap_or_default();
        let t = lua.create_table()?;
        t.set("lookType", outfit.look_type)?;
        t.set("lookTypeEx", outfit.look_type_ex)?;
        t.set("lookHead", outfit.look_head)?;
        t.set("lookBody", outfit.look_body)?;
        t.set("lookLegs", outfit.look_legs)?;
        t.set("lookFeet", outfit.look_feet)?;
        t.set("lookAddons", outfit.look_addons)?;
        Ok(t)
    })?)?;

    methods.set("setOutfit", lua.create_function(|_, (this, outfit_tbl): (LuaTable, LuaTable)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let outfit = crate::creatures::Outfit {
            look_type: outfit_tbl.get("lookType").unwrap_or(0),
            look_type_ex: outfit_tbl.get("lookTypeEx").unwrap_or(0),
            look_head: outfit_tbl.get("lookHead").unwrap_or(0),
            look_body: outfit_tbl.get("lookBody").unwrap_or(0),
            look_legs: outfit_tbl.get("lookLegs").unwrap_or(0),
            look_feet: outfit_tbl.get("lookFeet").unwrap_or(0),
            look_addons: outfit_tbl.get("lookAddons").unwrap_or(0),
            look_mount: outfit_tbl.get("lookMount").unwrap_or(0),
        };
        let mut game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature_mut(cid) {
            creature.base_mut().current_outfit = outfit;
        }
        Ok(())
    })?)?;

    methods.set("getTarget", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        let target = game.get_creature(cid).and_then(|c| c.base().attacked_creature_id);
        match target {
            Some(tid) => {
                drop(game);
                Ok(LuaValue::Table(push_creature_ref(lua, tid, "Creature")?))
            }
            None => Ok(LuaValue::Nil),
        }
    })?)?;

    methods.set("setTarget", lua.create_function(|_, (this, target): (LuaTable, LuaValue)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let target_id = match target {
            LuaValue::Table(t) => Some(get_creature_id(&t)?),
            _ => None,
        };
        let mut game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature_mut(cid) {
            creature.base_mut().attacked_creature_id = target_id;
        }
        Ok(())
    })?)?;

    methods.set("getFollowCreature", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        let follow = game.get_creature(cid).and_then(|c| c.base().follow_creature_id);
        match follow {
            Some(fid) => {
                drop(game);
                Ok(LuaValue::Table(push_creature_ref(lua, fid, "Creature")?))
            }
            None => Ok(LuaValue::Nil),
        }
    })?)?;

    methods.set("setFollowCreature", lua.create_function(|_, (this, follow): (LuaTable, LuaValue)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let follow_id = match follow {
            LuaValue::Table(t) => Some(get_creature_id(&t)?),
            _ => None,
        };
        let mut game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature_mut(cid) {
            creature.base_mut().follow_creature_id = follow_id;
        }
        Ok(())
    })?)?;

    methods.set("getMaster", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        let master = game.get_creature(cid).and_then(|c| c.base().master_id);
        match master {
            Some(mid) => {
                drop(game);
                Ok(LuaValue::Table(push_creature_ref(lua, mid, "Creature")?))
            }
            None => Ok(LuaValue::Nil),
        }
    })?)?;

    methods.set("setMaster", lua.create_function(|_, (this, master): (LuaTable, LuaValue)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let master_id = match master {
            LuaValue::Table(t) => Some(get_creature_id(&t)?),
            _ => None,
        };
        let mut game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature_mut(cid) {
            creature.base_mut().master_id = master_id;
        }
        Ok(())
    })?)?;

    methods.set("setDropLoot", lua.create_function(|_, (this, drop): (LuaTable, bool)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature_mut(cid) {
            creature.base_mut().loot_drop = drop;
        }
        Ok(())
    })?)?;

    methods.set("setSkillLoss", lua.create_function(|_, (this, loss): (LuaTable, bool)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature_mut(cid) {
            creature.base_mut().skill_loss = loss;
        }
        Ok(())
    })?)?;

    methods.set("getSummons", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        let summons = game.get_creature(cid).map(|c| c.base().summon_ids.clone()).unwrap_or_default();
        drop(game);
        let result = lua.create_table()?;
        for (i, sid) in summons.iter().enumerate() {
            let c = push_creature_ref(lua, *sid, "Creature")?;
            result.raw_set(i as i64 + 1, c)?;
        }
        Ok(result)
    })?)?;

    methods.set("getZone", lua.create_function(|_, this: LuaTable| -> LuaResult<u8> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_creature(cid).map(|c| c.get_zone() as u8).unwrap_or(0))
    })?)?;

    methods.set("teleportTo", lua.create_function(|_, (this, pos_tbl, _push): (LuaTable, LuaTable, Option<bool>)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let new_pos = table_to_position(&pos_tbl)?;
        let (old_pos, old_stackpos, is_player) = {
            let game = g_game().lock().unwrap();
            let creature = match game.get_creature(cid) {
                Some(c) => c,
                None => return Ok(false),
            };
            let old_pos = creature.position();
            let is_player = creature.is_player();
            let sp = game.map.get_tile(old_pos)
                .map(|t| t.get_client_index_of_creature(cid))
                .unwrap_or(0);
            (old_pos, sp as u8, is_player)
        };
        if old_pos == new_pos {
            return Ok(true);
        }
        {
            let mut game = g_game().lock().unwrap();
            game.move_creature_position(cid, old_pos, new_pos);
        }
        if is_player {
            crate::net::game_protocol::stop_auto_walk(cid);
            crate::net::game_protocol::send_teleport_map_to_player(cid, old_pos, old_stackpos, new_pos);
            crate::net::game_protocol::send_cancel_walk_to_session(cid);
        }
        crate::net::game_protocol::broadcast_creature_teleport_pub(cid, old_pos, old_stackpos, new_pos);
        Ok(true)
    })?)?;

    methods.set("say", lua.create_function(|_, (this, text, speak_type, _ghost, _spectators, _pos): (LuaTable, String, Option<u8>, Option<bool>, Option<LuaValue>, Option<LuaTable>)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let s2c_type = speak_type.unwrap_or(0x01);
        let game = g_game().lock().unwrap();
        let (name, pos) = match game.get_creature(cid) {
            Some(c) => {
                let name = match c {
                    crate::creatures::Creature::Player(p) => p.name.clone(),
                    crate::creatures::Creature::Monster(m) => m.get_name().to_string(),
                    crate::creatures::Creature::Npc(n) => n.get_name().to_string(),
                };
                (name, c.position())
            }
            None => return Ok(false),
        };
        drop(game);
        broadcast_effect_to_spectators(pos, |output| {
            output.add_byte(0xAA);
            output.add_u32(0x00);
            output.add_string(name.as_bytes());
            output.add_u16(0);
            output.add_byte(s2c_type);
            output.add_position(pos.x, pos.y, pos.z);
            output.add_string(text.as_bytes());
        });
        Ok(true)
    })?)?;

    methods.set("remove", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        game.remove_creature(cid);
        Ok(true)
    })?)?;

    methods.set("getDescription", lua.create_function(|_, (this, dist): (LuaTable, Option<i32>)| -> LuaResult<String> {
        let cid = get_creature_id(&this)?;
        let look_distance = dist.unwrap_or(0);
        let game = g_game().lock().unwrap();
        match game.get_creature(cid) {
            Some(crate::creatures::Creature::Player(p)) => {
                let mut s = String::new();
                // C++ uses group->access (NOT account type/group_id) — god is non-access.
                let access = crate::world::groups::access_for_group_id(p.group_id);
                let group_name = match p.group_id {
                    1 => "Player", 2 => "Tutor", 3 => "Senior Tutor",
                    4 => "Gamemaster", 5 => "Community Manager", 6 => "God",
                    _ => "",
                };
                let voc_desc = crate::world::vocation::g_vocations()
                    .get_vocation(p.vocation_id)
                    .map(|v| v.description.clone())
                    .unwrap_or_default();

                if look_distance == -1 {
                    s.push_str("yourself.");
                    if access {
                        if !group_name.is_empty() {
                            s.push_str(&format!(" You are {}.", group_name));
                        }
                    } else if p.vocation_id != 0 {
                        if !voc_desc.is_empty() {
                            s.push_str(&format!(" You are {}.", voc_desc));
                        }
                    } else {
                        s.push_str(" You have no vocation.");
                    }
                } else {
                    s.push_str(&p.name);
                    if !access {
                        s.push_str(&format!(" (Level {})", p.level));
                    }
                    s.push('.');
                    let pronoun = if p.sex == crate::creatures::player::PlayerSex::Female { " She" } else { " He" };
                    s.push_str(pronoun);
                    if access {
                        if !group_name.is_empty() {
                            s.push_str(&format!(" is {}.", group_name));
                        }
                    } else if p.vocation_id != 0 && !voc_desc.is_empty() {
                        s.push_str(&format!(" is {}.", voc_desc));
                    } else {
                        s.push_str(" has no vocation.");
                    }
                }
                Ok(s)
            }
            Some(crate::creatures::Creature::Monster(m)) => {
                Ok(m.get_description())
            }
            Some(crate::creatures::Creature::Npc(n)) => {
                Ok(format!("{}.", n.get_name()))
            }
            None => Ok(String::new()),
        }
    })?)?;

    methods.set("getDamageMap", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        let result = lua.create_table()?;
        if let Some(creature) = game.get_creature(cid) {
            for (&attacker_id, block) in &creature.base().damage_map {
                let entry = lua.create_table()?;
                entry.set("total", block.total)?;
                entry.set("ticks", block.ticks)?;
                result.set(attacker_id, entry)?;
            }
        }
        Ok(result)
    })?)?;

    methods.set("getEvents", lua.create_function(|lua, (this, _event_type): (LuaTable, Option<u32>)| -> LuaResult<LuaTable> {
        let _cid = get_creature_id(&this)?;
        lua.create_table()
    })?)?;

    methods.set("registerEvent", lua.create_function(|_, (this, _name): (LuaTable, String)| -> LuaResult<bool> {
        let _cid = get_creature_id(&this)?;
        Ok(true)
    })?)?;

    methods.set("unregisterEvent", lua.create_function(|_, (this, _name): (LuaTable, String)| -> LuaResult<bool> {
        let _cid = get_creature_id(&this)?;
        Ok(true)
    })?)?;

    methods.set("getParent", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature(cid) {
            let pos = creature.position();
            drop(game);
            let t = push_position(lua, pos)?;
            if let Ok(mt) = lua.named_registry_value::<LuaTable>("Tile") {
                let _ = t.set_metatable(Some(mt));
            }
            return Ok(LuaValue::Table(t));
        }
        Ok(LuaValue::Nil)
    })?)?;

    methods.set("getTile", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature(cid) {
            let pos = creature.position();
            drop(game);
            let t = push_position(lua, pos)?;
            if let Ok(mt) = lua.named_registry_value::<LuaTable>("Tile") {
                let _ = t.set_metatable(Some(mt));
            }
            return Ok(LuaValue::Table(t));
        }
        Ok(LuaValue::Nil)
    })?)?;

    methods.set("getCondition", lua.create_function(|lua, (this, cond_type, cond_id, sub_id): (LuaTable, u32, Option<u32>, Option<u32>)| -> LuaResult<LuaValue> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature(cid) {
            let cond_id = cond_id.unwrap_or(0);
            let sub_id = sub_id.unwrap_or(0);
            for condition in &creature.base().conditions {
                if condition.get_type() as u32 == cond_type
                    && (cond_id == 0 || condition.get_id() as u32 == cond_id)
                    && (sub_id == 0 || condition.get_sub_id() == sub_id) {
                    let t = lua.create_table()?;
                    t.set("type", condition.get_type() as u32)?;
                    t.set("id", condition.get_id() as u32)?;
                    t.set("subId", condition.get_sub_id())?;
                    t.set("ticks", condition.get_ticks())?;
                    if let Ok(mt) = lua.named_registry_value::<LuaTable>("Condition") {
                        let _ = t.set_metatable(Some(mt));
                    }
                    return Ok(LuaValue::Table(t));
                }
            }
        }
        Ok(LuaValue::Nil)
    })?)?;

    methods.set("addCondition", lua.create_function(|_, (this, condition, _force): (LuaTable, LuaValue, Option<bool>)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let cond_table = match condition {
            LuaValue::Table(t) => t,
            _ => return Ok(false),
        };
        let cond_type_raw: i32 = cond_table.get("_conditionType").unwrap_or(0);
        let cond_id_raw: i32 = cond_table.get("_conditionId").unwrap_or(1);
        let sub_id: u32 = cond_table.get("_subId").unwrap_or(0);
        let ticks: i32 = cond_table.get("_ticks").unwrap_or(0);
        let condition_type = ConditionType::from_u32(cond_type_raw as u32);
        let condition_id = match cond_id_raw {
            0 => ConditionId::Combat,
            1 => ConditionId::Head,
            2 => ConditionId::Necklace,
            3 => ConditionId::Backpack,
            4 => ConditionId::Armor,
            5 => ConditionId::Right,
            6 => ConditionId::Left,
            7 => ConditionId::Legs,
            8 => ConditionId::Feet,
            9 => ConditionId::Ring,
            10 => ConditionId::Ammo,
            _ => ConditionId::Default,
        };
        if let Some(cond) = create_condition(condition_id, condition_type, ticks, 0, false, sub_id, false) {
            let effects = {
                let mut game = g_game().lock().unwrap();
                if let Some(creature) = game.get_creature_mut(cid) {
                    let base_speed = creature.base().base_speed as i32;
                    let conditions = &mut creature.base_mut().conditions;
                    crate::combat::condition::add_condition_to_creature(conditions, cond, base_speed)
                } else {
                    return Ok(false);
                }
            };
            if !effects.is_empty() {
                crate::game::tick::apply_condition_effects(cid, &effects);
            }
            return Ok(true);
        }
        Ok(false)
    })?)?;

    methods.set("removeCondition", lua.create_function(|_, (this, cond_type, sub_id, _force): (LuaTable, u32, Option<u32>, Option<bool>)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let effects = {
            let mut game = g_game().lock().unwrap();
            let Some(creature) = game.get_creature_mut(cid) else { return Ok(false) };
            let sub_id = sub_id.unwrap_or(0);
            let base = creature.base_mut();
            let initial_len = base.conditions.len();
            let mut end_effects = Vec::new();
            let mut i = 0;
            while i < base.conditions.len() {
                if base.conditions[i].get_type() as u32 == cond_type
                    && (sub_id == 0 || base.conditions[i].get_sub_id() == sub_id)
                {
                    let mut cond = base.conditions.remove(i);
                    cond.end_condition();
                    end_effects.extend(cond.on_end());
                } else {
                    i += 1;
                }
            }
            if base.conditions.len() < initial_len {
                end_effects
            } else {
                return Ok(false);
            }
        };
        if !effects.is_empty() {
            crate::game::tick::apply_condition_effects(cid, &effects);
        }
        Ok(true)
    })?)?;

    methods.set("hasCondition", lua.create_function(|_, (this, cond_type, sub_id): (LuaTable, u32, Option<u32>)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature(cid) {
            let sub_id = sub_id.unwrap_or(0);
            return Ok(creature.base().conditions.iter().any(|c| {
                c.get_type() as u32 == cond_type && (sub_id == 0 || c.get_sub_id() == sub_id)
            }));
        }
        Ok(false)
    })?)?;

    methods.set("getPathTo", lua.create_function(|_lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let mut iter = args.into_iter();
        let this = match iter.next() { Some(LuaValue::Table(t)) => t, _ => return Ok(LuaValue::Boolean(false)) };
        let pos_table = match iter.next() { Some(LuaValue::Table(t)) => t, _ => return Ok(LuaValue::Boolean(false)) };
        let _cid = get_creature_id(&this)?;
        let _target = table_to_position(&pos_table)?;
        Ok(LuaValue::Boolean(false))
    })?)?;

    methods.set("move", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<u32> {
        let mut iter = args.into_iter();
        let this = match iter.next() { Some(LuaValue::Table(t)) => t, _ => return Ok(1) };
        let direction = match iter.next() {
            Some(LuaValue::Integer(n)) => n as u8,
            _ => return Ok(1),
        };
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        let old_pos = match game.get_creature(cid) {
            Some(c) => c.position(),
            None => return Ok(1),
        };
        let new_pos = match direction {
            0 => Position { x: old_pos.x, y: old_pos.y.wrapping_sub(1), z: old_pos.z },
            1 => Position { x: old_pos.x + 1, y: old_pos.y, z: old_pos.z },
            2 => Position { x: old_pos.x, y: old_pos.y + 1, z: old_pos.z },
            3 => Position { x: old_pos.x.wrapping_sub(1), y: old_pos.y, z: old_pos.z },
            4 => Position { x: old_pos.x + 1, y: old_pos.y.wrapping_sub(1), z: old_pos.z },
            5 => Position { x: old_pos.x + 1, y: old_pos.y + 1, z: old_pos.z },
            6 => Position { x: old_pos.x.wrapping_sub(1), y: old_pos.y + 1, z: old_pos.z },
            7 => Position { x: old_pos.x.wrapping_sub(1), y: old_pos.y.wrapping_sub(1), z: old_pos.z },
            _ => return Ok(1),
        };
        game.move_creature_position(cid, old_pos, new_pos);
        Ok(0)
    })?)?;

    methods.set("registerEvent", lua.create_function(|_, (this, name): (LuaTable, String)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(c) = game.get_creature_mut(cid) {
            c.base_mut().register_creature_event(&name);
            Ok(true)
        } else {
            Ok(false)
        }
    })?)?;

    methods.set("unregisterEvent", lua.create_function(|_, (this, name): (LuaTable, String)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(c) = game.get_creature_mut(cid) {
            let base = c.base_mut();
            let was_registered = base.events_list.iter().any(|n| n == &name);
            if was_registered {
                use crate::events::registry::g_script_registry;
                let etype = {
                    let registry = g_script_registry().lock().unwrap();
                    registry.creature_events.get_event_by_name(&name, false).map(|e| e.event_type)
                };
                base.events_list.retain(|n| n != &name);
                if let Some(etype) = etype {
                    let still_has = base.events_list.iter().any(|n| {
                        let registry = g_script_registry().lock().unwrap();
                        registry.creature_events.get_event_by_name(n, false)
                            .map(|e| e.event_type == etype)
                            .unwrap_or(false)
                    });
                    if !still_has {
                        base.script_events_bit_field &= !(1u32 << etype as u32);
                    }
                }
            }
            Ok(was_registered)
        } else {
            Ok(false)
        }
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Player class (extends Creature)
// ---------------------------------------------------------------------------

fn register_player_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let arg = args.into_iter().nth(1);
        let game = g_game().lock().unwrap();
        let cid = match arg {
            Some(LuaValue::Integer(id)) => {
                if game.get_player(id as u32).is_some() { Some(id as u32) } else { None }
            }
            Some(LuaValue::String(s)) => {
                let name = s.to_string_lossy();
                game.get_player_id_by_name(&name)
            }
            _ => None,
        };
        match cid {
            Some(id) => {
                drop(game);
                Ok(LuaValue::Table(push_creature_ref(lua, id, "Player")?))
            }
            None => Ok(LuaValue::Nil),
        }
    })?;
    let methods = register_class(lua, "Player", Some("Creature"), LUA_DATA_PLAYER, Some(ctor))?;

    set_meta(lua, "Player", "__eq", lua.create_function(|_, (a, b): (LuaTable, LuaTable)| {
        let id_a: u32 = a.raw_get(1).unwrap_or(0);
        let id_b: u32 = b.raw_get(1).unwrap_or(0);
        Ok(id_a == id_b && id_a != 0)
    })?)?;

    methods.set("isPlayer", lua.create_function(|_, _: LuaTable| Ok(true))?)?;

    methods.set("getGuid", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.guid).unwrap_or(0))
    })?)?;

    methods.set("getIp", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.last_ip).unwrap_or(0))
    })?)?;

    methods.set("getAccountId", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.account_number).unwrap_or(0))
    })?)?;

    methods.set("getLastLoginSaved", lua.create_function(|_, this: LuaTable| -> LuaResult<i64> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.last_login_saved).unwrap_or(0))
    })?)?;

    methods.set("getLastLogout", lua.create_function(|_, this: LuaTable| -> LuaResult<i64> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.last_logout).unwrap_or(0))
    })?)?;

    methods.set("getAccountType", lua.create_function(|_, this: LuaTable| -> LuaResult<u8> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.account_type as u8).unwrap_or(1))
    })?)?;

    methods.set("setAccountType", lua.create_function(|_, (this, at): (LuaTable, u8)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.account_type = match at {
                2 => crate::creatures::player::AccountType::Tutor,
                3 => crate::creatures::player::AccountType::SeniorTutor,
                4 => crate::creatures::player::AccountType::GameMaster,
                5 => crate::creatures::player::AccountType::CommunityManager,
                6 => crate::creatures::player::AccountType::God,
                _ => crate::creatures::player::AccountType::Normal,
            };
        }
        Ok(())
    })?)?;

    methods.set("getCapacity", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.capacity).unwrap_or(0))
    })?)?;

    methods.set("setCapacity", lua.create_function(|_, (this, cap): (LuaTable, u32)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.capacity = cap;
        }
        Ok(())
    })?)?;

    methods.set("getFreeCapacity", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.capacity.saturating_sub(p.inventory_weight)).unwrap_or(0))
    })?)?;

    methods.set("getExperience", lua.create_function(|_, this: LuaTable| -> LuaResult<u64> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.experience).unwrap_or(0))
    })?)?;

    methods.set("getLevel", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.level).unwrap_or(1))
    })?)?;

    methods.set("getMagicLevel", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.get_magic_level()).unwrap_or(0))
    })?)?;

    methods.set("getBaseMagicLevel", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.get_base_magic_level()).unwrap_or(0))
    })?)?;

    methods.set("getMana", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.mana).unwrap_or(0))
    })?)?;

    methods.set("addMana", lua.create_function(|_, (this, amount): (LuaTable, i32)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            let new_mana = (player.mana as i32 + amount).max(0) as u32;
            player.mana = new_mana.min(player.get_max_mana());
        }
        Ok(())
    })?)?;

    methods.set("getMaxMana", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.get_max_mana()).unwrap_or(0))
    })?)?;

    methods.set("setMaxMana", lua.create_function(|_, (this, mm): (LuaTable, u32)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.mana_max = mm;
        }
        Ok(())
    })?)?;

    methods.set("getManaSpent", lua.create_function(|_, this: LuaTable| -> LuaResult<u64> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.mana_spent).unwrap_or(0))
    })?)?;

    methods.set("getBaseMaxHealth", lua.create_function(|_, this: LuaTable| -> LuaResult<i32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.base.health_max).unwrap_or(0))
    })?)?;

    methods.set("getBaseMaxMana", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.mana_max).unwrap_or(0))
    })?)?;

    methods.set("getSkillLevel", lua.create_function(|_, (this, skill): (LuaTable, usize)| -> LuaResult<u16> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).and_then(|p| p.skills.get(skill)).map(|s| s.level).unwrap_or(10))
    })?)?;

    methods.set("getEffectiveSkillLevel", lua.create_function(|_, (this, skill): (LuaTable, usize)| -> LuaResult<i32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(player) = game.get_player(cid) {
            let base = player.skills.get(skill).map(|s| s.level as i32).unwrap_or(10);
            let bonus = player.var_skills.get(skill).copied().unwrap_or(0);
            return Ok((base + bonus).max(0));
        }
        Ok(10)
    })?)?;

    methods.set("getSkillPercent", lua.create_function(|_, (this, skill): (LuaTable, usize)| -> LuaResult<u8> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).and_then(|p| p.skills.get(skill)).map(|s| s.percent).unwrap_or(0))
    })?)?;

    methods.set("getSkillTries", lua.create_function(|_, (this, skill): (LuaTable, usize)| -> LuaResult<u64> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).and_then(|p| p.skills.get(skill)).map(|s| s.tries).unwrap_or(0))
    })?)?;

    methods.set("getSpecialSkill", lua.create_function(|_, (this, skill): (LuaTable, usize)| -> LuaResult<i32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).and_then(|p| p.var_special_skills.get(skill).copied()).unwrap_or(0))
    })?)?;

    methods.set("getVocation", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        let voc_id = game.get_player(cid).map(|p| p.vocation_id).unwrap_or(0);
        let t = lua.create_table()?;
        t.raw_set(1, voc_id)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Vocation") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(t)
    })?)?;

    methods.set("setVocation", lua.create_function(|_, (this, voc_id): (LuaTable, u16)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.vocation_id = voc_id;
        }
        Ok(())
    })?)?;

    methods.set("getSex", lua.create_function(|_, this: LuaTable| -> LuaResult<u8> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.sex as u8).unwrap_or(0))
    })?)?;

    methods.set("setSex", lua.create_function(|_, (this, sex): (LuaTable, u8)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.sex = if sex == 1 { crate::creatures::player::PlayerSex::Male } else { crate::creatures::player::PlayerSex::Female };
        }
        Ok(())
    })?)?;

    methods.set("getTown", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        let town_id = game.get_player(cid).map(|p| p.town_id).unwrap_or(0);
        let t = lua.create_table()?;
        t.raw_set(1, town_id)?;
        Ok(t)
    })?)?;

    methods.set("setTown", lua.create_function(|_, (this, town_tbl): (LuaTable, LuaTable)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let town_id: u32 = town_tbl.raw_get(1)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.town_id = town_id;
        }
        Ok(())
    })?)?;

    methods.set("getGuildNick", lua.create_function(|_, this: LuaTable| -> LuaResult<String> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.guild_nick.clone()).unwrap_or_default())
    })?)?;

    methods.set("setGuildNick", lua.create_function(|_, (this, nick): (LuaTable, String)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.guild_nick = nick;
        }
        Ok(())
    })?)?;

    methods.set("getGroup", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        let group_id = game.get_player(cid).map(|p| p.group_id).unwrap_or(0);
        let t = lua.create_table()?;
        t.raw_set(1, group_id)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Group") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(t)
    })?)?;

    methods.set("setGroup", lua.create_function(|_, (this, group_tbl): (LuaTable, LuaTable)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let group_id: u32 = group_tbl.raw_get(1)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.group_id = group_id;
        }
        Ok(())
    })?)?;

    methods.set("getStamina", lua.create_function(|_, this: LuaTable| -> LuaResult<u16> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.stamina_minutes).unwrap_or(0))
    })?)?;

    methods.set("setStamina", lua.create_function(|_, (this, stamina): (LuaTable, u16)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.stamina_minutes = stamina;
        }
        Ok(())
    })?)?;

    methods.set("getSoul", lua.create_function(|_, this: LuaTable| -> LuaResult<u8> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.soul).unwrap_or(0))
    })?)?;

    methods.set("addSoul", lua.create_function(|_, (this, amount): (LuaTable, i32)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.soul = ((player.soul as i32) + amount).clamp(0, 255) as u8;
        }
        Ok(())
    })?)?;

    methods.set("getMaxSoul", lua.create_function(|_, this: LuaTable| -> LuaResult<u8> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(player) = game.get_player(cid) {
            let vocations = crate::world::vocation::g_vocations();
            if let Some(voc) = vocations.get_vocation(player.vocation_id) {
                return Ok(voc.soul_max);
            }
        }
        Ok(100)
    })?)?;

    methods.set("getBankBalance", lua.create_function(|_, this: LuaTable| -> LuaResult<u64> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.bank_balance).unwrap_or(0))
    })?)?;

    methods.set("setBankBalance", lua.create_function(|_, (this, bal): (LuaTable, u64)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.bank_balance = bal;
        }
        Ok(())
    })?)?;

    methods.set("getStorageValue", lua.create_function(|_, (this, key): (LuaTable, u32)| -> LuaResult<i32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).and_then(|p| p.storage_map.get(&key).copied()).unwrap_or(-1))
    })?)?;

    methods.set("setStorageValue", lua.create_function(|_, (this, key, value): (LuaTable, u32, i32)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            if value == -1 {
                player.storage_map.remove(&key);
            } else {
                player.storage_map.insert(key, value);
            }
        }
        Ok(())
    })?)?;

    methods.set("getSkullTime", lua.create_function(|_, this: LuaTable| -> LuaResult<i64> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.skull_ticks).unwrap_or(0))
    })?)?;

    methods.set("setSkullTime", lua.create_function(|_, (this, ticks): (LuaTable, i64)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.skull_ticks = ticks;
        }
        Ok(())
    })?)?;

    methods.set("getPremiumEndsAt", lua.create_function(|_, this: LuaTable| -> LuaResult<i64> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.premium_ends_at).unwrap_or(0))
    })?)?;

    methods.set("setPremiumEndsAt", lua.create_function(|_, (this, ts): (LuaTable, i64)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let account_id = {
            let mut game = g_game().lock().unwrap();
            if let Some(player) = game.get_player_mut(cid) {
                player.premium_ends_at = ts;
                player.account_number
            } else {
                return Ok(false);
            }
        };
        tokio::spawn(crate::db::login::update_premium_time(account_id, ts));
        Ok(true)
    })?)?;

    methods.set("hasBlessing", lua.create_function(|_, (this, id): (LuaTable, u8)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.blessings & (1 << id) != 0).unwrap_or(false))
    })?)?;

    methods.set("addBlessing", lua.create_function(|_, (this, id): (LuaTable, u8)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.blessings |= 1 << id;
        }
        Ok(())
    })?)?;

    methods.set("removeBlessing", lua.create_function(|_, (this, id): (LuaTable, u8)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.blessings &= !(1 << id);
        }
        Ok(())
    })?)?;

    methods.set("isPzLocked", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.pz_locked).unwrap_or(false))
    })?)?;

    methods.set("sendCancelMessage", lua.create_function(|_, (this, code): (LuaTable, i32)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let msg = get_return_message(code);
        crate::net::game_protocol::send_status_message_to_player(cid, msg);
        Ok(true)
    })?)?;

    methods.set("hasChaseMode", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.chase_mode).unwrap_or(false))
    })?)?;

    methods.set("hasSecureMode", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.secure_mode).unwrap_or(false))
    })?)?;

    methods.set("getFightMode", lua.create_function(|_, this: LuaTable| -> LuaResult<u8> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.fight_mode as u8).unwrap_or(1))
    })?)?;

    methods.set("setGhostMode", lua.create_function(|_, (this, ghost): (LuaTable, bool)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.is_ghost_mode = ghost;
        }
        Ok(())
    })?)?;

    methods.set("hasLearnedSpell", lua.create_function(|_, (this, name): (LuaTable, String)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.learned_instant_spells.iter().any(|s| s.eq_ignore_ascii_case(&name))).unwrap_or(false))
    })?)?;

    methods.set("learnSpell", lua.create_function(|_, (this, name): (LuaTable, String)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            if !player.learned_instant_spells.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
                player.learned_instant_spells.push(name);
            }
        }
        Ok(())
    })?)?;

    methods.set("forgetSpell", lua.create_function(|_, (this, name): (LuaTable, String)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.learned_instant_spells.retain(|s| !s.eq_ignore_ascii_case(&name));
        }
        Ok(())
    })?)?;

    methods.set("getInstantSpells", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        let spells = game.get_player(cid).map(|p| p.learned_instant_spells.clone()).unwrap_or_default();
        let result = lua.create_table()?;
        for (i, spell) in spells.iter().enumerate() {
            result.raw_set(i as i64 + 1, spell.as_str())?;
        }
        Ok(result)
    })?)?;

    methods.set("sendTextMessage", lua.create_function(|_, (this, msg_type, text): (LuaTable, u8, String)| -> LuaResult<()> {
        let cid: u32 = this.raw_get(1)?;
        let wire_type = crate::net::protocol_version::translate_message_class_to_client(msg_type);
        send_packet_to_player(cid, move |output| {
            output.add_byte(0xB4);
            output.add_byte(wire_type);
            output.add_string(text.as_bytes());
        });
        Ok(())
    })?)?;

    methods.set("sendChannelMessage", lua.create_function(|_, (this, author, text, msg_type, channel_id): (LuaTable, String, String, u8, u16)| -> LuaResult<()> {
        let cid: u32 = this.raw_get(1)?;
        let wire_type = crate::net::protocol_version::translate_speak_class_to_client(msg_type);
        send_packet_to_player(cid, move |output| {
            output.add_byte(0xAA);
            output.add_u32(0x00);
            output.add_string(author.as_bytes());
            output.add_u16(0);
            output.add_byte(wire_type);
            output.add_u16(channel_id);
            output.add_string(text.as_bytes());
        });
        Ok(())
    })?)?;

    methods.set("sendPrivateMessage", lua.create_function(|_, (this, _speaker, text, msg_type): (LuaTable, LuaValue, String, Option<u8>)| -> LuaResult<()> {
        let cid: u32 = this.raw_get(1)?;
        let speak_type = msg_type.unwrap_or(0x04);
        send_packet_to_player(cid, |output| {
            output.add_byte(0xAA);
            output.add_u32(0x00);
            output.add_string(b"");
            output.add_u16(0);
            output.add_byte(speak_type);
            output.add_string(text.as_bytes());
        });
        Ok(())
    })?)?;

    methods.set("channelSay", lua.create_function(|_, (this, _speaker, speak_type, text, channel_id): (LuaTable, LuaValue, u8, String, u16)| -> LuaResult<()> {
        let cid: u32 = this.raw_get(1)?;
        send_packet_to_player(cid, |output| {
            output.add_byte(0xAA);
            output.add_u32(0x00);
            output.add_string(b"");
            output.add_u16(0);
            output.add_byte(speak_type);
            output.add_u16(channel_id);
            output.add_string(text.as_bytes());
        });
        Ok(())
    })?)?;

    methods.set("openChannel", lua.create_function(|_, (this, channel_id): (LuaTable, u16)| -> LuaResult<()> {
        let cid: u32 = this.raw_get(1)?;
        send_packet_to_player(cid, |output| {
            output.add_byte(0xAC);
            output.add_u16(channel_id);
            output.add_string(b"");
        });
        Ok(())
    })?)?;

    methods.set("popupFYI", lua.create_function(|_, (this, text): (LuaTable, String)| -> LuaResult<()> {
        let cid: u32 = this.raw_get(1)?;
        send_packet_to_player(cid, |output| {
            output.add_byte(0x15);
            output.add_string(text.as_bytes());
        });
        Ok(())
    })?)?;

    methods.set("save", lua.create_function(|_, _: LuaTable| -> LuaResult<bool> {
        Ok(true)
    })?)?;

    methods.set("getDepotChest", lua.create_function(|lua, (this, depot_id, _auto_create): (LuaTable, u32, Option<bool>)| -> LuaResult<LuaValue> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(player) = game.get_player(cid) {
            if let Some(&item_id) = player.depot_chests.get(&depot_id) {
                let t = lua.create_table()?;
                t.raw_set(1, item_id as u32)?;
                if let Ok(mt) = lua.named_registry_value::<LuaTable>("Container") {
                    let _ = t.set_metatable(Some(mt));
                }
                return Ok(LuaValue::Table(t));
            }
        }
        Ok(LuaValue::Nil)
    })?)?;
    methods.set("getDeathPenalty", lua.create_function(|_, this: LuaTable| -> LuaResult<f64> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(player) = game.get_player(cid) {
            let bless_count = player.blessings.count_ones() as f64;
            let loss_percent = if player.level >= 25 {
                let tmp_level = player.level as f64 + (player.level_percent as f64 / 100.0);
                ((tmp_level + 50.0) * 50.0 * ((tmp_level * tmp_level) - (5.0 * tmp_level) + 8.0))
                    / player.experience as f64
            } else {
                10.0
            };
            let percent_reduction = bless_count * 8.0;
            let result = loss_percent * (1.0 - (percent_reduction / 100.0)) / 100.0;
            return Ok(result * 100.0);
        }
        Ok(0.0)
    })?)?;
    methods.set("addExperience", lua.create_function(|_, (this, exp, _send_text): (LuaTable, i64, Option<bool>)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.experience = (player.experience as i64 + exp).max(0) as u64;
        }
        Ok(())
    })?)?;
    methods.set("removeExperience", lua.create_function(|_, (this, exp): (LuaTable, u64)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.experience = player.experience.saturating_sub(exp);
        }
        Ok(())
    })?)?;
    methods.set("addManaSpent", lua.create_function(|_, (this, amount): (LuaTable, u64)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.mana_spent = player.mana_spent.saturating_add(amount);
        }
        Ok(())
    })?)?;
    methods.set("removeManaSpent", lua.create_function(|_, (this, amount): (LuaTable, u64)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.mana_spent = player.mana_spent.saturating_sub(amount);
        }
        Ok(())
    })?)?;
    methods.set("addSkillTries", lua.create_function(|_, (this, skill, tries): (LuaTable, u8, u64)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            let si = skill as usize;
            if si < crate::creatures::player::SKILL_COUNT {
                player.skills[si].tries = player.skills[si].tries.saturating_add(tries);
            }
        }
        Ok(())
    })?)?;
    methods.set("removeSkillTries", lua.create_function(|_, (this, skill, tries): (LuaTable, u8, u64)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            let si = skill as usize;
            if si < crate::creatures::player::SKILL_COUNT {
                player.skills[si].tries = player.skills[si].tries.saturating_sub(tries);
            }
        }
        Ok(())
    })?)?;
    methods.set("addSpecialSkill", lua.create_function(|_, (this, skill, value): (LuaTable, u8, i32)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            let si = skill as usize;
            if si < crate::creatures::player::SPECIALSKILL_COUNT {
                player.var_special_skills[si] += value;
            }
        }
        Ok(())
    })?)?;
    methods.set("getItemCount", lua.create_function(|_, (this, item_id, _sub_type): (LuaTable, u16, Option<i32>)| -> LuaResult<u32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(player) = game.get_player(cid) {
            let mut count = 0u32;
            for sid in player.inventory.iter().flatten() {
                if *sid == item_id {
                    count += 1;
                }
            }
            return Ok(count);
        }
        Ok(0)
    })?)?;
    methods.set("getItemById", lua.create_function(|lua, (this, item_id, _deep_search): (LuaTable, u16, Option<bool>)| -> LuaResult<LuaValue> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(player) = game.get_player(cid) {
            for sid in player.inventory.iter().flatten() {
                if *sid == item_id {
                    let t = lua.create_table()?;
                    t.raw_set(1, *sid as u32)?;
                    if let Ok(mt) = lua.named_registry_value::<LuaTable>("Item") {
                        let _ = t.set_metatable(Some(mt));
                    }
                    return Ok(LuaValue::Table(t));
                }
            }
        }
        Ok(LuaValue::Nil)
    })?)?;
    methods.set("getGuild", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(player) = game.get_player(cid) {
            if let Some(guild_id) = player.guild_id {
                let t = lua.create_table()?;
                t.raw_set(1, guild_id)?;
                if let Ok(mt) = lua.named_registry_value::<LuaTable>("Guild") {
                    let _ = t.set_metatable(Some(mt));
                }
                return Ok(LuaValue::Table(t));
            }
        }
        Ok(LuaValue::Nil)
    })?)?;
    methods.set("setGuild", lua.create_function(|_, (this, guild): (LuaTable, LuaValue)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            match guild {
                LuaValue::Table(t) => {
                    let guild_id: u32 = t.raw_get(1)?;
                    player.guild_id = Some(guild_id);
                }
                LuaValue::Nil => {
                    player.guild_id = None;
                    player.guild_rank_id = None;
                }
                _ => {}
            }
        }
        Ok(())
    })?)?;
    methods.set("getGuildLevel", lua.create_function(|_, this: LuaTable| -> LuaResult<u8> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).and_then(|p| p.guild_rank_id).unwrap_or(0) as u8)
    })?)?;
    methods.set("setGuildLevel", lua.create_function(|_, (this, level): (LuaTable, u8)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.guild_rank_id = Some(level as u32);
        }
        Ok(())
    })?)?;
    methods.set("addItem", lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        use crate::creatures::player::{CONST_SLOT_FIRST, CONST_SLOT_LAST, CONST_SLOT_WHEREEVER};
        let mut iter = args.into_iter();
        let this = match iter.next() { Some(LuaValue::Table(t)) => t, _ => return Ok(LuaValue::Nil) };
        let item_id_raw = iter.next().unwrap_or(LuaValue::Nil);
        let count_raw = iter.next().unwrap_or(LuaValue::Integer(1));
        let can_drop = match iter.next() { Some(LuaValue::Boolean(b)) => b, _ => true };
        let sub_type_raw = iter.next().unwrap_or(LuaValue::Integer(1));
        let slot_raw = iter.next().unwrap_or(LuaValue::Integer(0));
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        let item_id: u16 = match &item_id_raw {
            LuaValue::Integer(n) => *n as u16,
            LuaValue::String(s) => game.items.get_item_id_by_name(&s.to_string_lossy()).unwrap_or(0),
            _ => return Ok(LuaValue::Nil),
        };
        if item_id == 0 { return Ok(LuaValue::Nil); }
        let it = game.items.get_item_type(item_id as usize);
        let stackable = it.stackable;
        let has_sub = it.has_sub_type();
        let is_container = it.kind == crate::items::ItemKind::Container;
        let mut count = match count_raw { LuaValue::Integer(n) => n as i32, LuaValue::Number(n) => n as i32, _ => 1 };
        let sub_type = match sub_type_raw { LuaValue::Integer(n) => n as i32, LuaValue::Number(n) => n as i32, _ => 1 };
        if count < 1 { count = 1; }
        let slot_pref: usize = match slot_raw { LuaValue::Integer(n) => n as usize, _ => CONST_SLOT_WHEREEVER };
        // For stackable items, add to existing stack or free slot
        if stackable || has_sub {
            let cap = 100i32;
            let qty = if has_sub && !stackable { sub_type } else { count.min(cap) };
            // Try to add to existing stack
            let player = match game.get_player_mut(cid) { Some(p) => p, None => return Ok(LuaValue::Nil) };
            let target_slot = if (CONST_SLOT_FIRST..=CONST_SLOT_LAST).contains(&slot_pref)
                && (player.inventory[slot_pref] == Some(item_id) || player.inventory[slot_pref].is_none()) {
                Some(slot_pref)
            } else {
                (CONST_SLOT_FIRST..=CONST_SLOT_LAST).find(|&s| {
                    player.inventory[s] == Some(item_id) && (player.inventory_count[s] as i32) < cap
                }).or_else(|| (CONST_SLOT_FIRST..=CONST_SLOT_LAST).find(|&s| player.inventory[s].is_none()))
            };
            if let Some(sl) = target_slot {
                if player.inventory[sl] == Some(item_id) {
                    let new_count = (player.inventory_count[sl] as i32 + qty).min(cap) as u16;
                    player.inventory_count[sl] = new_count;
                } else {
                    player.inventory[sl] = Some(item_id);
                    player.inventory_count[sl] = qty.max(1) as u16;
                }
                let pos = player.base.position;
                drop(game);
                crate::net::game_protocol::send_inventory_item_to_player(cid, sl as u8, item_id, qty as u16);
                let t = push_item_ref(lua, item_id as u16, pos, sl as i32)?;
                if is_container {
                    if let Ok(mt) = lua.named_registry_value::<LuaTable>("Container") {
                        let _ = t.set_metatable(Some(mt));
                    }
                }
                return Ok(LuaValue::Table(t));
            }
        } else {
            // Non-stackable: find free slot
            let player = match game.get_player_mut(cid) { Some(p) => p, None => return Ok(LuaValue::Nil) };
            let target_slot = if (CONST_SLOT_FIRST..=CONST_SLOT_LAST).contains(&slot_pref) && player.inventory[slot_pref].is_none() {
                Some(slot_pref)
            } else {
                (CONST_SLOT_FIRST..=CONST_SLOT_LAST).find(|&s| player.inventory[s].is_none())
            };
            if let Some(sl) = target_slot {
                player.inventory[sl] = Some(item_id);
                player.inventory_count[sl] = 1;
                let pos = player.base.position;
                drop(game);
                crate::net::game_protocol::send_inventory_item_to_player(cid, sl as u8, item_id, 1);
                let t = push_item_ref(lua, item_id as u16, pos, sl as i32)?;
                if is_container {
                    if let Ok(mt) = lua.named_registry_value::<LuaTable>("Container") {
                        let _ = t.set_metatable(Some(mt));
                    }
                }
                return Ok(LuaValue::Table(t));
            }
        }
        // No space in inventory — drop on map if allowed
        if can_drop {
            let pos = game.get_player(cid).map(|p| p.base.position);
            if let Some(pos) = pos {
                let items_arc = game.items.clone();
                let item = crate::map::tile::MapItem { server_id: item_id, count: count.max(1) as u16, ..Default::default() };
                if let Some(tile) = game.map.get_tile_mut(pos) {
                    let idx = tile.items.len() as i32;
                    tile.internal_add_item(item, &items_arc);
                    drop(game);
                    let t = push_item_ref(lua, item_id, pos, idx)?;
                    if is_container {
                        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Container") {
                            let _ = t.set_metatable(Some(mt));
                        }
                    }
                    return Ok(LuaValue::Table(t));
                }
            }
        }
        Ok(LuaValue::Nil)
    })?)?;
    methods.set("addItemEx", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<u32> {
        use crate::creatures::player::{CONST_SLOT_FIRST, CONST_SLOT_LAST};
        const RETURNVALUE_NOERROR: u32 = 0;
        const RETURNVALUE_NOTENOUGHROOM: u32 = 8;
        let mut iter = args.into_iter();
        let this = match iter.next() { Some(LuaValue::Table(t)) => t, _ => return Ok(1) };
        let cid = get_creature_id(&this)?;
        let item_tbl = match iter.next() { Some(LuaValue::Table(t)) => t, _ => return Ok(1) };
        let server_id: u16 = item_tbl.raw_get::<u16>(1).unwrap_or(0);
        if server_id == 0 { return Ok(1); }
        let pos_x: u16 = item_tbl.raw_get("_pos_x").unwrap_or(0);
        let pos_y: u16 = item_tbl.raw_get("_pos_y").unwrap_or(0);
        let pos_z: u8 = item_tbl.raw_get("_pos_z").unwrap_or(0);
        let idx: i32 = item_tbl.raw_get("_idx").unwrap_or(-1);
        let count: u16 = if pos_x == 0 && pos_y == 0 && pos_z == 0 {
            item_tbl.raw_get::<u16>("_count").unwrap_or(1)
        } else {
            let game = g_game().lock().unwrap();
            let pos = crate::map::Position { x: pos_x, y: pos_y, z: pos_z };
            let mi = game.map.get_tile(pos).and_then(|t| {
                if idx == -1 { t.ground.as_ref() } else { t.items.get(idx as usize) }
            });
            mi.map(|m| m.count).unwrap_or(1)
        };
        // If the item is a container with children, expand children into player inventory.
        let children: Option<LuaTable> = item_tbl.raw_get("_children").ok();
        if let Some(ref ch) = children {
            if ch.raw_len() > 0 {
                let items: Vec<(u16, u16)> = ch.clone().sequence_values::<LuaTable>().flatten()
                    .map(|t| {
                        let sid: u16 = t.raw_get(1).unwrap_or(0);
                        let cnt: u16 = t.raw_get("_count").unwrap_or(1);
                        (sid, cnt)
                    })
                    .filter(|(sid, _)| *sid != 0)
                    .collect();
                let mut any_placed = false;
                for (child_id, child_count) in items {
                    let child_stackable = crate::items::g_items().get_item_type(child_id as usize).stackable;
                    let mut game = g_game().lock().unwrap();
                    let Some(player) = game.get_player_mut(cid) else { continue; };
                    let mut remaining = child_count as u32;
                    if child_stackable {
                        for slot in CONST_SLOT_FIRST..=CONST_SLOT_LAST {
                            if remaining == 0 { break; }
                            if player.inventory[slot] == Some(child_id) {
                                let cur = player.inventory_count[slot] as u32;
                                if cur < 100 {
                                    let add = remaining.min(100 - cur);
                                    player.inventory_count[slot] = (cur + add) as u16;
                                    let nc = player.inventory_count[slot];
                                    crate::net::game_protocol::send_inventory_item_to_player(cid, slot as u8, child_id, nc);
                                    remaining -= add;
                                    any_placed = true;
                                }
                            }
                        }
                        if remaining > 0 {
                            if let Some(slot) = (CONST_SLOT_FIRST..=CONST_SLOT_LAST).find(|&s| player.inventory[s].is_none()) {
                                let add = remaining.min(100) as u16;
                                player.inventory[slot] = Some(child_id);
                                player.inventory_count[slot] = add;
                                crate::net::game_protocol::send_inventory_item_to_player(cid, slot as u8, child_id, add);
                                any_placed = true;
                            }
                        }
                    } else {
                        if let Some(slot) = (CONST_SLOT_FIRST..=CONST_SLOT_LAST).find(|&s| player.inventory[s].is_none()) {
                            player.inventory[slot] = Some(child_id);
                            player.inventory_count[slot] = child_count;
                            crate::net::game_protocol::send_inventory_item_to_player(cid, slot as u8, child_id, child_count);
                            any_placed = true;
                        }
                    }
                }
                return Ok(if any_placed { RETURNVALUE_NOERROR } else { RETURNVALUE_NOTENOUGHROOM });
            }
        }
        let stackable = crate::items::g_items().get_item_type(server_id as usize).stackable;
        let mut game = g_game().lock().unwrap();
        let Some(player) = game.get_player_mut(cid) else { return Ok(1); };
        if stackable {
            let mut remaining = count as u32;
            for slot in CONST_SLOT_FIRST..=CONST_SLOT_LAST {
                if remaining == 0 { break; }
                if player.inventory[slot] == Some(server_id) {
                    let cur = player.inventory_count[slot] as u32;
                    if cur < 100 {
                        let add = remaining.min(100 - cur);
                        player.inventory_count[slot] = (cur + add) as u16;
                        let nc = player.inventory_count[slot];
                        crate::net::game_protocol::send_inventory_item_to_player(cid, slot as u8, server_id, nc);
                        remaining -= add;
                    }
                }
            }
            if remaining > 0 {
                if let Some(slot) = (CONST_SLOT_FIRST..=CONST_SLOT_LAST).find(|&s| player.inventory[s].is_none()) {
                    let add = remaining.min(100) as u16;
                    player.inventory[slot] = Some(server_id);
                    player.inventory_count[slot] = add;
                    crate::net::game_protocol::send_inventory_item_to_player(cid, slot as u8, server_id, add);
                    remaining = remaining.saturating_sub(100);
                }
            }
            if remaining > 0 { return Ok(RETURNVALUE_NOTENOUGHROOM); }
        } else {
            if let Some(slot) = (CONST_SLOT_FIRST..=CONST_SLOT_LAST).find(|&s| player.inventory[s].is_none()) {
                player.inventory[slot] = Some(server_id);
                player.inventory_count[slot] = count;
                crate::net::game_protocol::send_inventory_item_to_player(cid, slot as u8, server_id, count);
            } else {
                return Ok(RETURNVALUE_NOTENOUGHROOM);
            }
        }
        Ok(RETURNVALUE_NOERROR)
    })?)?;
    methods.set("removeItem", lua.create_function(|_, (this, item_id, count, _sub_type, _ignore_equipped): (LuaTable, u16, Option<u32>, Option<i32>, Option<bool>)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let count = count.unwrap_or(1);
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            let mut remaining = count;
            for slot in crate::creatures::player::CONST_SLOT_FIRST..=crate::creatures::player::CONST_SLOT_LAST {
                if remaining == 0 { break; }
                if player.inventory[slot] == Some(item_id) {
                    let available = player.inventory_count[slot] as u32;
                    if available <= remaining {
                        remaining -= available;
                        player.inventory[slot] = None;
                        player.inventory_count[slot] = 0;
                        crate::net::game_protocol::send_clear_inventory_slot(cid, slot as u8);
                    } else {
                        player.inventory_count[slot] -= remaining as u16;
                        let new_count = player.inventory_count[slot];
                        crate::net::game_protocol::send_inventory_item_to_player(cid, slot as u8, item_id, new_count);
                        remaining = 0;
                    }
                }
            }
            return Ok(remaining == 0);
        }
        Ok(false)
    })?)?;
    methods.set("getMoney", lua.create_function(|_, this: LuaTable| -> LuaResult<u64> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_player(cid).map(|p| p.get_money()).unwrap_or(0))
    })?)?;
    methods.set("addMoney", lua.create_function(|_, (this, money): (LuaTable, u64)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let mut remaining = money;
        let pos = {
            let game = g_game().lock().unwrap();
            game.get_player(cid).map(|p| p.base.position)
        };
        let Some(pos) = pos else { return Ok(false); };
        // Add crystal coins, then platinum, then gold
        for (coin_id, worth) in [(2160u16, 10_000u64), (2152, 100), (2148, 1)] {
            if remaining >= worth {
                let qty = (remaining / worth).min(0xFFFF) as u16;
                remaining -= worth * qty as u64;
                let mut game = g_game().lock().unwrap();
                if let Some(player) = game.get_player_mut(cid) {
                    // Find existing stack or free slot
                    let mut placed = false;
                    for slot in crate::creatures::player::CONST_SLOT_FIRST..=crate::creatures::player::CONST_SLOT_LAST {
                        if player.inventory[slot] == Some(coin_id) {
                            let new_qty = player.inventory_count[slot].saturating_add(qty);
                            player.inventory_count[slot] = new_qty;
                            crate::net::game_protocol::send_inventory_item_to_player(cid, slot as u8, coin_id, new_qty);
                            placed = true;
                            break;
                        }
                    }
                    if !placed {
                        if let Some(slot) = (crate::creatures::player::CONST_SLOT_FIRST..=crate::creatures::player::CONST_SLOT_LAST).find(|&s| player.inventory[s].is_none()) {
                            player.inventory[slot] = Some(coin_id);
                            player.inventory_count[slot] = qty;
                            drop(game);
                            crate::net::game_protocol::send_inventory_item_to_player(cid, slot as u8, coin_id, qty);
                        } else {
                            // No inventory space — drop on tile
                            let _ = player;
                            let items_arc = game.items.clone();
                            let item = crate::map::tile::MapItem { server_id: coin_id, count: qty, ..Default::default() };
                            if let Some(tile) = game.map.get_tile_mut(pos) {
                                tile.internal_add_item(item, &items_arc);
                            }
                        }
                    }
                }
            }
        }
        Ok(remaining == 0)
    })?)?;
    methods.set("removeMoney", lua.create_function(|_, (this, money): (LuaTable, u64)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        let player = match game.get_player_mut(cid) { Some(p) => p, None => return Ok(false) };
        if player.get_money() < money { return Ok(false); }
        let mut remaining = money;
        for (coin_id, worth) in [(2148u16, 1u64), (2152, 100), (2160, 10_000)] {
            if remaining == 0 { break; }
            for slot in crate::creatures::player::CONST_SLOT_FIRST..=crate::creatures::player::CONST_SLOT_LAST {
                if remaining == 0 { break; }
                if player.inventory[slot] == Some(coin_id) {
                    let available = player.inventory_count[slot] as u64;
                    let can_take = available.min(remaining / worth);
                    if can_take > 0 {
                        remaining -= can_take * worth;
                        let new_count = available - can_take;
                        if new_count == 0 {
                            player.inventory[slot] = None;
                            player.inventory_count[slot] = 0;
                            crate::net::game_protocol::send_clear_inventory_slot(cid, slot as u8);
                        } else {
                            player.inventory_count[slot] = new_count as u16;
                            let nc = new_count as u16;
                            crate::net::game_protocol::send_inventory_item_to_player(cid, slot as u8, coin_id, nc);
                        }
                    }
                }
            }
        }
        Ok(remaining == 0)
    })?)?;
    methods.set("showTextDialog", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<()> {
        let mut iter = args.into_iter();
        let this = match iter.next() { Some(LuaValue::Table(t)) => t, _ => return Ok(()) };
        let cid = get_creature_id(&this)?;
        let item_id = match iter.next() { Some(LuaValue::Integer(n)) => n as u16, _ => return Ok(()) };
        let text = match iter.next() { Some(LuaValue::String(s)) => s.to_string_lossy().to_string(), _ => String::new() };
        let can_write = match iter.next() { Some(LuaValue::Boolean(b)) => b, _ => false };
        let max_len = match iter.next() { Some(LuaValue::Integer(n)) => n as u16, _ => 0 };
        send_packet_to_player(cid, |output| {
            output.add_byte(0x96);
            output.add_u32(0);
            output.add_u16(item_id);
            if can_write {
                output.add_u16(max_len);
                output.add_string(text.as_bytes());
            } else {
                output.add_u16(text.len() as u16);
                output.add_string(text.as_bytes());
            }
            output.add_string(b"");
            output.add_string(b"");
        });
        Ok(())
    })?)?;
    methods.set("getSlotItem", lua.create_function(|lua, (this, slot): (LuaTable, u8)| -> LuaResult<LuaValue> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        let (server_id, count) = game.get_player(cid)
            .map(|p| (
                p.inventory.get(slot as usize).copied().flatten(),
                p.inventory_count.get(slot as usize).copied().unwrap_or(1),
            ))
            .unwrap_or((None, 1));
        match server_id {
            Some(sid) => {
                let t = lua.create_table()?;
                t.raw_set(1, sid as i64)?;
                t.raw_set("itemid", sid as i64)?;
                t.raw_set("_owner_id", cid as i64)?;
                t.raw_set("_slot", slot as i64)?;
                t.raw_set("_count", count as i64)?;
                if let Ok(mt) = lua.named_registry_value::<LuaTable>("Item") {
                    let _ = t.set_metatable(Some(mt));
                }
                Ok(LuaValue::Table(t))
            }
            None => Ok(LuaValue::Nil),
        }
    })?)?;
    methods.set("getParty", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(player) = game.get_player(cid) {
            if let Some(leader_id) = player.party_id {
                if game.get_party(leader_id).is_some() {
                    let t = lua.create_table()?;
                    t.raw_set(1, leader_id)?;
                    if let Ok(mt) = lua.named_registry_value::<LuaTable>("Party") {
                        let _ = t.set_metatable(Some(mt));
                    }
                    return Ok(LuaValue::Table(t));
                }
            }
        }
        Ok(LuaValue::Nil)
    })?)?;
    methods.set("addOutfit", lua.create_function(|_, (this, look_type): (LuaTable, u16)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            if !player.outfits.iter().any(|o| o.look_type == look_type) {
                player.outfits.push(crate::creatures::player::OutfitEntry { look_type, addons: 0 });
            }
        }
        Ok(())
    })?)?;
    methods.set("addOutfitAddon", lua.create_function(|_, (this, look_type, addon): (LuaTable, u16, u8)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            if let Some(entry) = player.outfits.iter_mut().find(|o| o.look_type == look_type) {
                entry.addons |= addon;
            } else {
                player.outfits.push(crate::creatures::player::OutfitEntry { look_type, addons: addon });
            }
        }
        Ok(())
    })?)?;
    methods.set("removeOutfit", lua.create_function(|_, (this, look_type): (LuaTable, u16)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.outfits.retain(|o| o.look_type != look_type);
        }
        Ok(())
    })?)?;
    methods.set("removeOutfitAddon", lua.create_function(|_, (this, look_type, addon): (LuaTable, u16, u8)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            if let Some(entry) = player.outfits.iter_mut().find(|o| o.look_type == look_type) {
                entry.addons &= !addon;
            }
        }
        Ok(())
    })?)?;
    methods.set("hasOutfit", lua.create_function(|_, (this, look_type, addon): (LuaTable, u16, Option<u8>)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(player) = game.get_player(cid) {
            let addon = addon.unwrap_or(0);
            return Ok(player.outfits.iter().any(|o| o.look_type == look_type && (o.addons & addon) == addon));
        }
        Ok(false)
    })?)?;
    methods.set("canWearOutfit", lua.create_function(|_, (this, look_type, addon): (LuaTable, u16, Option<u8>)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(player) = game.get_player(cid) {
            if player.group_flags & crate::creatures::player::PLAYER_FLAG_CAN_ILLUSION_ALL != 0 {
                return Ok(true);
            }
            let addon = addon.unwrap_or(0);
            return Ok(player.outfits.iter().any(|o| o.look_type == look_type && (o.addons & addon) == addon));
        }
        Ok(false)
    })?)?;
    methods.set("sendOutfitWindow", lua.create_function(|_, this: LuaTable| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(player) = game.get_player(cid) {
            let outfits = player.outfits.clone();
            let outfit = player.base.current_outfit;
            drop(game);
            send_packet_to_player(cid, |output| {
                output.add_byte(0xC8);
                output.add_u16(outfit.look_type);
                output.add_byte(outfit.look_head);
                output.add_byte(outfit.look_body);
                output.add_byte(outfit.look_legs);
                output.add_byte(outfit.look_feet);
                output.add_byte(outfit.look_addons);
                let count = outfits.len().min(255) as u8;
                output.add_byte(count);
                for o in outfits.iter().take(count as usize) {
                    output.add_u16(o.look_type);
                    output.add_string(b"");
                    output.add_byte(o.addons);
                }
            });
        }
        Ok(())
    })?)?;
    methods.set("canLearnSpell", lua.create_function(|_, (this, _spell_name): (LuaTable, String)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(player) = game.get_player(cid) {
            if player.has_flag(crate::creatures::player::PLAYER_FLAG_IGNORE_SPELL_CHECK) {
                return Ok(true);
            }
        }
        Ok(true)
    })?)?;
    methods.set("sendTutorial", lua.create_function(|_, (this, tutorial_id): (LuaTable, u8)| -> LuaResult<()> {
        let cid: u32 = this.raw_get(1)?;
        send_packet_to_player(cid, |output| {
            output.add_byte(0xDC);
            output.add_byte(tutorial_id);
        });
        Ok(())
    })?)?;
    methods.set("addMapMark", lua.create_function(|_, (this, pos, mark_type, desc): (LuaTable, LuaTable, u8, Option<String>)| -> LuaResult<()> {
        let cid: u32 = this.raw_get(1)?;
        let p = table_to_position(&pos)?;
        let description = desc.unwrap_or_default();
        send_packet_to_player(cid, |output| {
            output.add_byte(0xDD);
            output.add_position(p.x, p.y, p.z);
            output.add_byte(mark_type);
            output.add_string(description.as_bytes());
        });
        Ok(())
    })?)?;
    methods.set("getClient", lua.create_function(|lua, _this: LuaTable| -> LuaResult<LuaTable> {
        let t = lua.create_table()?;
        t.set("version", 860i64)?;
        t.set("os", 2i64)?;
        Ok(t)
    })?)?;
    methods.set("getHouse", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(player) = game.get_player(cid) {
            if let Some(house) = game.map.houses.get_house_by_player_id(player.guid) {
                let t = lua.create_table()?;
                t.raw_set(1, house.id)?;
                if let Ok(mt) = lua.named_registry_value::<LuaTable>("House") {
                    let _ = t.set_metatable(Some(mt));
                }
                return Ok(LuaValue::Table(t));
            }
        }
        Ok(LuaValue::Nil)
    })?)?;
    methods.set("sendHouseWindow", lua.create_function(|_, (this, house_val, list_id): (LuaTable, LuaValue, u32)| -> LuaResult<LuaValue> {
        let cid = get_creature_id(&this)?;
        let house_id: Option<u32> = match &house_val {
            LuaValue::Table(t) => t.raw_get("id").ok().or_else(|| t.raw_get(1).ok()),
            LuaValue::Integer(n) => Some(*n as u32),
            _ => None,
        };
        let Some(house_id) = house_id else {
            return Ok(LuaValue::Nil);
        };
        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(cid) else {
            return Ok(LuaValue::Nil);
        };
        let window_text_id = player.window_text_id;
        let text = game.map.houses.get_house(house_id)
            .and_then(|h: &crate::map::houses::House| h.get_access_list(list_id).map(|s| s.to_string()))
            .unwrap_or_default();
        drop(game);
        crate::net::game_protocol::send_house_window_to_player(cid, window_text_id, &text);
        Ok(LuaValue::Boolean(true))
    })?)?;
    methods.set("setEditHouse", lua.create_function(|_, (this, house, list_id): (LuaTable, LuaValue, u32)| -> LuaResult<()> {
        let cid = get_creature_id(&this)?;
        let house_id: Option<u32> = match house {
            LuaValue::Table(t) => t.raw_get("id").ok().or_else(|| t.raw_get(1).ok()),
            LuaValue::Integer(n) => Some(n as u32),
            _ => None,
        };
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(cid) {
            player.window_text_id = player.window_text_id.wrapping_add(1);
            player.edit_house_id = house_id;
            player.edit_list_id = list_id;
        }
        Ok(())
    })?)?;
    methods.set("getContainerId", lua.create_function(|_, (this, _container): (LuaTable, LuaTable)| -> LuaResult<i32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(player) = game.get_player(cid) {
            if let Some((&cid_key, _)) = player.open_containers.iter().next() {
                return Ok(cid_key as i32);
            }
        }
        Ok(-1)
    })?)?;
    methods.set("getContainerById", lua.create_function(|lua, (this, id): (LuaTable, u8)| -> LuaResult<LuaValue> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(player) = game.get_player(cid) {
            if let Some(oc) = player.open_containers.get(&id) {
                let t = lua.create_table()?;
                let server_id = crate::creatures::player::resolve_container_server_id(&game, oc);
                t.raw_set(1, server_id as u32)?;
                if let Ok(mt) = lua.named_registry_value::<LuaTable>("Container") {
                    let _ = t.set_metatable(Some(mt));
                }
                return Ok(LuaValue::Table(t));
            }
        }
        Ok(LuaValue::Nil)
    })?)?;
    methods.set("getContainerIndex", lua.create_function(|_, (this, id): (LuaTable, u8)| -> LuaResult<u16> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(player) = game.get_player(cid) {
            if let Some(oc) = player.open_containers.get(&id) {
                return Ok(oc.scroll_index);
            }
        }
        Ok(0)
    })?)?;
    methods.set("canCast", lua.create_function(|_, (this, _spell): (LuaTable, LuaValue)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        if let Some(player) = game.get_player(cid) {
            if player.has_flag(crate::creatures::player::PLAYER_FLAG_IGNORE_SPELL_CHECK) {
                return Ok(true);
            }
        }
        Ok(true)
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Monster class (extends Creature)
// ---------------------------------------------------------------------------

fn register_monster_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let arg = args.into_iter().nth(1);
        let game = g_game().lock().unwrap();
        let cid = match arg {
            Some(LuaValue::Integer(id)) => {
                if game.get_creature(id as u32).and_then(|c| c.as_monster()).is_some() {
                    Some(id as u32)
                } else { None }
            }
            _ => None,
        };
        match cid {
            Some(id) => {
                drop(game);
                Ok(LuaValue::Table(push_creature_ref(lua, id, "Monster")?))
            }
            None => Ok(LuaValue::Nil),
        }
    })?;
    let methods = register_class(lua, "Monster", Some("Creature"), LUA_DATA_MONSTER, Some(ctor))?;
    set_meta(lua, "Monster", "__eq", lua.create_function(|_, (a, b): (LuaTable, LuaTable)| {
        let id_a: u32 = a.raw_get(1).unwrap_or(0);
        let id_b: u32 = b.raw_get(1).unwrap_or(0);
        Ok(id_a == id_b && id_a != 0)
    })?)?;

    methods.set("isMonster", lua.create_function(|_, _: LuaTable| Ok(true))?)?;

    methods.set("getType", lua.create_function(|lua, _this: LuaTable| -> LuaResult<LuaValue> {
        let t = lua.create_table()?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("MonsterType") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(LuaValue::Table(t))
    })?)?;
    methods.set("rename", lua.create_function(|_, (_this, _name, _name_desc): (LuaTable, String, Option<String>)| -> LuaResult<bool> {
        Ok(true)
    })?)?;
    methods.set("getSpawnPosition", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        let pos = game.get_creature(cid).map(|c| c.position()).unwrap_or_default();
        drop(game);
        push_position(lua, pos)
    })?)?;
    methods.set("isInSpawnRange", lua.create_function(|_, (_this, _pos): (LuaTable, Option<LuaTable>)| -> LuaResult<bool> {
        Ok(true)
    })?)?;
    methods.set("isIdle", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_creature(cid).and_then(|c| c.as_monster()).map(|m| m.is_idle).unwrap_or(true))
    })?)?;
    methods.set("setIdle", lua.create_function(|_, (this, idle): (LuaTable, bool)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(m) = game.get_creature_mut(cid).and_then(|c| c.as_monster_mut()) {
            m.is_idle = idle;
        }
        Ok(true)
    })?)?;
    methods.set("isTarget", lua.create_function(|_, (this, creature): (LuaTable, LuaTable)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let target_cid = get_creature_id(&creature)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_creature(cid).and_then(|c| c.as_monster()).map(|m| m.target_list.contains(&target_cid)).unwrap_or(false))
    })?)?;
    methods.set("isOpponent", lua.create_function(|_, (this, creature): (LuaTable, LuaTable)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let target_cid = get_creature_id(&creature)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_creature(cid).and_then(|c| c.as_monster()).map(|m| m.target_list.contains(&target_cid)).unwrap_or(false))
    })?)?;
    methods.set("isFriend", lua.create_function(|_, (this, creature): (LuaTable, LuaTable)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let target_cid = get_creature_id(&creature)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_creature(cid).and_then(|c| c.as_monster()).map(|m| m.friend_set.contains(&target_cid)).unwrap_or(false))
    })?)?;
    methods.set("addFriend", lua.create_function(|_, (this, creature): (LuaTable, LuaTable)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let target_cid = get_creature_id(&creature)?;
        let mut game = g_game().lock().unwrap();
        if let Some(m) = game.get_creature_mut(cid).and_then(|c| c.as_monster_mut()) {
            m.friend_set.insert(target_cid);
        }
        Ok(true)
    })?)?;
    methods.set("removeFriend", lua.create_function(|_, (this, creature): (LuaTable, LuaTable)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let target_cid = get_creature_id(&creature)?;
        let mut game = g_game().lock().unwrap();
        if let Some(m) = game.get_creature_mut(cid).and_then(|c| c.as_monster_mut()) {
            m.friend_set.remove(&target_cid);
        }
        Ok(true)
    })?)?;
    methods.set("getFriendList", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        let tbl = lua.create_table()?;
        if let Some(m) = game.get_creature(cid).and_then(|c| c.as_monster()) {
            for (i, &fid) in m.friend_set.iter().enumerate() {
                tbl.raw_set(i + 1, push_creature_ref(lua, fid, "Creature")?)?;
            }
        }
        Ok(tbl)
    })?)?;
    methods.set("getFriendCount", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_creature(cid).and_then(|c| c.as_monster()).map(|m| m.friend_set.len() as u32).unwrap_or(0))
    })?)?;
    methods.set("addTarget", lua.create_function(|_, (this, creature, push_front): (LuaTable, LuaTable, Option<bool>)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let target_cid = get_creature_id(&creature)?;
        let mut game = g_game().lock().unwrap();
        if let Some(m) = game.get_creature_mut(cid).and_then(|c| c.as_monster_mut()) {
            if push_front.unwrap_or(false) {
                m.target_list.push_front(target_cid);
            } else {
                m.target_list.push_back(target_cid);
            }
        }
        Ok(true)
    })?)?;
    methods.set("removeTarget", lua.create_function(|_, (this, creature): (LuaTable, LuaTable)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let target_cid = get_creature_id(&creature)?;
        let mut game = g_game().lock().unwrap();
        if let Some(m) = game.get_creature_mut(cid).and_then(|c| c.as_monster_mut()) {
            m.target_list.retain(|&id| id != target_cid);
        }
        Ok(true)
    })?)?;
    methods.set("getTargetList", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        let tbl = lua.create_table()?;
        if let Some(m) = game.get_creature(cid).and_then(|c| c.as_monster()) {
            for (i, &tid) in m.target_list.iter().enumerate() {
                tbl.raw_set(i + 1, push_creature_ref(lua, tid, "Creature")?)?;
            }
        }
        Ok(tbl)
    })?)?;
    methods.set("getTargetCount", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_creature(cid).and_then(|c| c.as_monster()).map(|m| m.target_list.len() as u32).unwrap_or(0))
    })?)?;
    methods.set("selectTarget", lua.create_function(|_, (this, creature): (LuaTable, LuaTable)| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let target_cid = get_creature_id(&creature)?;
        let mut game = g_game().lock().unwrap();
        if let Some(m) = game.get_creature_mut(cid).and_then(|c| c.as_monster_mut()) {
            if m.target_list.contains(&target_cid) {
                m.base.follow_creature_id = Some(target_cid);
                m.base.attacked_creature_id = Some(target_cid);
                return Ok(true);
            }
        }
        Ok(false)
    })?)?;
    methods.set("searchTarget", lua.create_function(|_, (_this, _search_type): (LuaTable, Option<u32>)| -> LuaResult<bool> {
        Ok(false)
    })?)?;
    methods.set("isWalkingToSpawn", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_creature(cid).and_then(|c| c.as_monster()).map(|m| m.walking_to_spawn).unwrap_or(false))
    })?)?;
    methods.set("walkToSpawn", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let cid = get_creature_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(m) = game.get_creature_mut(cid).and_then(|c| c.as_monster_mut()) {
            m.walking_to_spawn = true;
            return Ok(true);
        }
        Ok(false)
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Npc class (extends Creature)
// ---------------------------------------------------------------------------

fn register_npc_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let arg = args.into_iter().nth(1);
        let game = g_game().lock().unwrap();
        let cid = match arg {
            Some(LuaValue::Integer(id)) => {
                if game.get_creature(id as u32).and_then(|c| c.as_npc()).is_some() {
                    Some(id as u32)
                } else { None }
            }
            _ => None,
        };
        match cid {
            Some(id) => {
                drop(game);
                Ok(LuaValue::Table(push_creature_ref(lua, id, "Npc")?))
            }
            None => Ok(LuaValue::Nil),
        }
    })?;
    let methods = register_class(lua, "Npc", Some("Creature"), LUA_DATA_NPC, Some(ctor))?;
    set_meta(lua, "Npc", "__eq", lua.create_function(|_, (a, b): (LuaTable, LuaTable)| {
        let id_a: u32 = a.raw_get(1).unwrap_or(0);
        let id_b: u32 = b.raw_get(1).unwrap_or(0);
        Ok(id_a == id_b && id_a != 0)
    })?)?;

    methods.set("isNpc", lua.create_function(|_, _: LuaTable| Ok(true))?)?;
    methods.set("setMasterPos", lua.create_function(|_, (_this, _pos, _radius): (LuaTable, LuaTable, Option<u32>)| -> LuaResult<bool> {
        Ok(true)
    })?)?;
    methods.set("getParameter", lua.create_function(|lua, (this, key): (LuaTable, String)| -> LuaResult<LuaValue> {
        let npc_id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        let type_name = game.get_creature(npc_id)
            .and_then(|c| c.as_npc())
            .map(|n| n.type_name.clone())
            .unwrap_or_default();
        drop(game);
        let val = crate::creatures::npc::g_npcs()
            .get_npc_type(&type_name)
            .and_then(|nt| nt.parameters.get(&key))
            .cloned();
        match val {
            Some(s) => Ok(LuaValue::String(lua.create_string(s.as_bytes())?)),
            None => Ok(LuaValue::Nil),
        }
    })?)?;
    methods.set("setFocus", lua.create_function(|_, (_this, _creature): (LuaTable, LuaValue)| -> LuaResult<()> {
        Ok(())
    })?)?;
    methods.set("openShopWindow", lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<bool> {
        use crate::creatures::player::ShopInfo;
        let mut it = args.into_iter();
        let npc_id = match it.next() {
            Some(LuaValue::Table(t)) => t.raw_get::<u32>(1).unwrap_or(0),
            Some(LuaValue::Integer(n)) => n as u32,
            _ => return Ok(false),
        };
        let player_id = match it.next() {
            Some(LuaValue::Table(t)) => t.raw_get::<u32>(1).unwrap_or(0),
            Some(LuaValue::Integer(n)) => n as u32,
            _ => return Ok(false),
        };
        let items_tbl = match it.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Ok(false),
        };
        // buy callback (arg 4), sell callback (arg 5) — store in script registry
        let sell_cb_val = it.next();
        let buy_cb_val = it.next();
        let sell_cb_id = if let Some(LuaValue::Function(f)) = sell_cb_val {
            let key = lua.create_registry_value(f)?;
            let id = {
                let registry = crate::events::registry::g_script_registry();
                let mut r = registry.lock().unwrap();
                let id = r.next_id();
                r.lua_callbacks.insert(id, key);
                id
            };
            id
        } else { -1 };
        let buy_cb_id = if let Some(LuaValue::Function(f)) = buy_cb_val {
            let key = lua.create_registry_value(f)?;
            let id = {
                let registry = crate::events::registry::g_script_registry();
                let mut r = registry.lock().unwrap();
                let id = r.next_id();
                r.lua_callbacks.insert(id, key);
                id
            };
            id
        } else { -1 };
        // Parse shop item list
        let mut shop_items: Vec<ShopInfo> = Vec::new();
        for entry in items_tbl.clone().sequence_values::<LuaTable>().flatten() {
            let item_id: u32 = entry.get("id").unwrap_or(0);
            let sub_type: i32 = entry.get::<i32>("subType").unwrap_or_else(|_| entry.get::<i32>("subtype").unwrap_or(1));
            let buy_price: u32 = entry.get("buy").unwrap_or(0);
            let sell_price: u32 = entry.get("sell").unwrap_or(0);
            let real_name: String = entry.get("name").unwrap_or_default();
            shop_items.push(ShopInfo { item_id, sub_type, buy_price, sell_price, real_name });
        }
        // Set shop owner and callbacks on player, then send shop packets
        {
            let mut game = g_game().lock().unwrap();
            if let Some(player) = game.get_player_mut(player_id) {
                player.shop_owner_id = Some(npc_id);
                player.purchase_callback = buy_cb_id;
                player.sale_callback = sell_cb_id;
                player.shop_item_list = shop_items.clone();
            } else {
                return Ok(false);
            }
        }
        crate::net::game_protocol::send_shop_to_player(player_id, &shop_items);
        crate::net::game_protocol::send_sale_item_list(player_id, &shop_items);
        Ok(true)
    })?)?;
    methods.set("closeShopWindow", lua.create_function(|_lua, args: LuaMultiValue| -> LuaResult<bool> {
        let mut it = args.into_iter();
        let _npc_id = match it.next() {
            Some(LuaValue::Table(t)) => t.raw_get::<u32>(1).unwrap_or(0),
            Some(LuaValue::Integer(n)) => n as u32,
            _ => return Ok(false),
        };
        let player_id = match it.next() {
            Some(LuaValue::Table(t)) => t.raw_get::<u32>(1).unwrap_or(0),
            Some(LuaValue::Integer(n)) => n as u32,
            _ => return Ok(false),
        };
        let (buy_cb, sell_cb) = {
            let mut game = g_game().lock().unwrap();
            if let Some(player) = game.get_player_mut(player_id) {
                let bc = player.purchase_callback;
                let sc = player.sale_callback;
                player.shop_owner_id = None;
                player.purchase_callback = -1;
                player.sale_callback = -1;
                player.shop_item_list.clear();
                (bc, sc)
            } else {
                return Ok(false);
            }
        };
        // Unref Lua callbacks
        let registry = crate::events::registry::g_script_registry();
        let mut r = registry.lock().unwrap();
        if buy_cb != -1 { r.lua_callbacks.remove(&buy_cb); }
        if sell_cb != -1 { r.lua_callbacks.remove(&sell_cb); }
        drop(r);
        crate::net::game_protocol::send_close_shop(player_id);
        Ok(true)
    })?)?;

    // NPC-exclusive global functions (selfSay, etc.) are registered separately below.
    register_npc_globals(lua)?;
    Ok(())
}

fn register_npc_globals(lua: &Lua) -> LuaResult<()> {
    use crate::lua::script::get_current_npc;

    // selfSay(text[, player]) — NPC says text to all or to a specific player.
    lua.globals().set("selfSay", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<()> {
        let mut it = args.into_iter();
        let text = match it.next() {
            Some(LuaValue::String(s)) => s.to_string_lossy().to_string(),
            _ => return Ok(()),
        };
        let target_id: Option<u32> = match it.next() {
            Some(LuaValue::Table(t)) => t.raw_get::<u32>(1).ok(),
            Some(LuaValue::Integer(n)) => Some(n as u32),
            _ => None,
        };
        let npc_id = get_current_npc();
        if npc_id == 0 { return Ok(()); }
        let (pos, name) = {
            let game = g_game().lock().unwrap();
            let Some(creature) = game.get_creature(npc_id) else { return Ok(()); };
            (creature.position(), creature.as_npc().map(|n| n.get_name().to_owned()).unwrap_or_default())
        };
        if let Some(player_id) = target_id {
            // Private NPC-to-player message (TALKTYPE_PRIVATE_NP = 0x04 in wire).
            crate::net::game_protocol::send_packet_to_player(player_id, move |output: &mut crate::net::output_message::OutputMessage| {
                output.add_byte(0xAA);
                output.add_u32(npc_id);
                output.add_string(name.as_bytes());
                output.add_u16(0); // level irrelevant for NPCs
                output.add_byte(crate::net::protocol_version::translate_speak_class_to_client(4));
                output.add_string(text.as_bytes());
            });
        } else {
            // Public say (TALKTYPE_SAY = 0x01).
            crate::net::game_protocol::broadcast_creature_say(npc_id, pos, &name, 0, 1, text.as_bytes());
        }
        Ok(())
    })?)?;

    // selfMoveTo — NPC pathfind to position. No-op (NPC pathfinding not ported, no scripts use it).
    lua.globals().set("selfMoveTo", lua.create_function(|_, _args: LuaMultiValue| -> LuaResult<bool> {
        Ok(false)
    })?)?;

    // selfFollow — NPC follow player. No-op (NPC pathfinding not ported, no scripts use it).
    lua.globals().set("selfFollow", lua.create_function(|_, _args: LuaMultiValue| -> LuaResult<bool> {
        Ok(false)
    })?)?;

    // selfMove(direction) — move NPC one step.
    lua.globals().set("selfMove", lua.create_function(|_, dir_val: u8| -> LuaResult<()> {
        use crate::creatures::Direction;
        let npc_id = get_current_npc();
        if npc_id == 0 { return Ok(()); }
        let Some(dir) = Direction::from_u8(dir_val) else { return Ok(()); };
        let (pos, old_stackpos) = {
            let game = g_game().lock().unwrap();
            let Some(creature) = game.get_creature(npc_id) else { return Ok(()); };
            let pos = creature.position();
            let sp = game.map.get_tile(pos).map(|t| t.get_creature_client_stackpos()).unwrap_or(0);
            (pos, sp)
        };
        let new_pos = pos.offset_direction(dir);
        let walkable = g_game().lock().unwrap().map.get_tile(new_pos).map(|t| t.is_walkable()).unwrap_or(false);
        if !walkable { return Ok(()); }
        {
            let mut game = g_game().lock().unwrap();
            if let Some(c) = game.get_creature_mut(npc_id) { c.base_mut().direction = dir; }
            game.move_creature_position(npc_id, pos, new_pos);
        }
        crate::net::game_protocol::broadcast_creature_move(npc_id, pos, new_pos, old_stackpos);
        Ok(())
    })?)?;

    // selfTurn(direction) — rotate NPC in place.
    lua.globals().set("selfTurn", lua.create_function(|_, dir_val: u8| -> LuaResult<()> {
        use crate::creatures::Direction;
        let npc_id = get_current_npc();
        if npc_id == 0 { return Ok(()); }
        let Some(dir) = Direction::from_u8(dir_val) else { return Ok(()); };
        {
            let mut game = g_game().lock().unwrap();
            if let Some(c) = game.get_creature_mut(npc_id) { c.base_mut().direction = dir; }
        }
        let (pos, stackpos) = {
            let game = g_game().lock().unwrap();
            let Some(c) = game.get_creature(npc_id) else { return Ok(()); };
            let sp = game.map.get_tile(c.position()).map(|t| t.get_creature_client_stackpos()).unwrap_or(0);
            (c.position(), sp as i32)
        };
        crate::net::game_protocol::broadcast_creature_turn(npc_id, pos, stackpos, dir);
        Ok(())
    })?)?;

    // getNpcCid() — return the NPC creature ID.
    lua.globals().set("getNpcCid", lua.create_function(|_, ()| -> LuaResult<u32> {
        Ok(get_current_npc())
    })?)?;

    // getNpcParameter(key) — return the NPC parameter value.
    lua.globals().set("getNpcParameter", lua.create_function(|lua, key: String| -> LuaResult<LuaValue> {
        let npc_id = get_current_npc();
        if npc_id == 0 { return Ok(LuaValue::Nil); }
        let type_name = {
            let game = g_game().lock().unwrap();
            game.get_creature(npc_id)
                .and_then(|c| c.as_npc())
                .map(|n| n.type_name.clone())
                .unwrap_or_default()
        };
        let val = crate::creatures::npc::g_npcs()
            .get_npc_type(&type_name)
            .and_then(|nt| nt.parameters.get(&key))
            .cloned();
        match val {
            Some(s) => Ok(LuaValue::String(lua.create_string(s.as_bytes())?)),
            None => Ok(LuaValue::Nil),
        }
    })?)?;

    // getDistanceTo(uid) — Chebyshev distance from NPC to creature/item uid.
    lua.globals().set("getDistanceTo", lua.create_function(|_, uid: u32| -> LuaResult<i32> {
        let npc_id = get_current_npc();
        if npc_id == 0 { return Ok(-1); }
        let game = g_game().lock().unwrap();
        let npc_pos = game.get_creature(npc_id).map(|c| c.position());
        let thing_pos = game.get_creature(uid).map(|c| c.position());
        match (npc_pos, thing_pos) {
            (Some(np), Some(tp)) => {
                if np.z != tp.z { return Ok(-1); }
                let dx = (np.x as i32 - tp.x as i32).abs();
                let dy = (np.y as i32 - tp.y as i32).abs();
                Ok(dx.max(dy))
            }
            _ => Ok(-1),
        }
    })?)?;

    // doNpcSetCreatureFocus(cid) — NPC faces the given creature.
    lua.globals().set("doNpcSetCreatureFocus", lua.create_function(|_, _cid: LuaValue| -> LuaResult<()> {
        Ok(())
    })?)?;

    // openShopWindow(cid, items, buyCallback, sellCallback)
    lua.globals().set("openShopWindow", lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<bool> {
        use crate::creatures::player::ShopInfo;
        let mut it = args.into_iter();
        let player_id: u32 = match it.next() {
            Some(LuaValue::Table(t)) => t.raw_get::<u32>(1).unwrap_or(0),
            Some(LuaValue::Integer(n)) => n as u32,
            _ => return Ok(false),
        };
        let items_tbl = match it.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Ok(false),
        };
        let buy_cb_id = if let Some(LuaValue::Function(f)) = it.next() {
            let key = lua.create_registry_value(f)?;
            let registry = crate::events::registry::g_script_registry();
            let mut r = registry.lock().unwrap();
            let id = r.next_id();
            r.lua_callbacks.insert(id, key);
            id
        } else { -1 };
        let sell_cb_id = if let Some(LuaValue::Function(f)) = it.next() {
            let key = lua.create_registry_value(f)?;
            let registry = crate::events::registry::g_script_registry();
            let mut r = registry.lock().unwrap();
            let id = r.next_id();
            r.lua_callbacks.insert(id, key);
            id
        } else { -1 };
        let npc_id = get_current_npc();
        if npc_id == 0 { return Ok(false); }
        let mut shop_items: Vec<ShopInfo> = Vec::new();
        for entry in items_tbl.sequence_values::<LuaTable>().flatten() {
            let item_id: u32 = entry.get("id").unwrap_or(0);
            let sub_type: i32 = entry.get::<i32>("subType").unwrap_or_else(|_| entry.get::<i32>("subtype").unwrap_or(1));
            let buy_price: u32 = entry.get("buy").unwrap_or(0);
            let sell_price: u32 = entry.get("sell").unwrap_or(0);
            let real_name: String = entry.get("name").unwrap_or_default();
            shop_items.push(ShopInfo { item_id, sub_type, buy_price, sell_price, real_name });
        }
        // Close any existing shop window first.
        let (old_buy_cb, old_sell_cb) = {
            let mut game = g_game().lock().unwrap();
            if let Some(player) = game.get_player_mut(player_id) {
                let old_buy = player.purchase_callback;
                let old_sell = player.sale_callback;
                player.shop_owner_id = None;
                player.purchase_callback = -1;
                player.sale_callback = -1;
                player.shop_item_list.clear();
                (old_buy, old_sell)
            } else {
                (-1, -1)
            }
        };
        {
            let registry = crate::events::registry::g_script_registry();
            let mut r = registry.lock().unwrap();
            if old_buy_cb != -1 { r.lua_callbacks.remove(&old_buy_cb); }
            if old_sell_cb != -1 { r.lua_callbacks.remove(&old_sell_cb); }
        }
        {
            let mut game = g_game().lock().unwrap();
            if let Some(player) = game.get_player_mut(player_id) {
                player.shop_owner_id = Some(npc_id);
                player.purchase_callback = buy_cb_id;
                player.sale_callback = sell_cb_id;
                player.shop_item_list = shop_items.clone();
            } else {
                return Ok(false);
            }
        }
        crate::net::game_protocol::send_shop_to_player(player_id, &shop_items);
        crate::net::game_protocol::send_sale_item_list(player_id, &shop_items);
        Ok(true)
    })?)?;

    // closeShopWindow(cid)
    lua.globals().set("closeShopWindow", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<bool> {
        let player_id: u32 = match args.into_iter().next() {
            Some(LuaValue::Table(t)) => t.raw_get::<u32>(1).unwrap_or(0),
            Some(LuaValue::Integer(n)) => n as u32,
            _ => return Ok(false),
        };
        let npc_id = get_current_npc();
        if npc_id == 0 { return Ok(false); }
        let (buy_cb, sell_cb, is_owner) = {
            let game = g_game().lock().unwrap();
            if let Some(player) = game.get_player(player_id) {
                let owner_match = player.shop_owner_id == Some(npc_id);
                (player.purchase_callback, player.sale_callback, owner_match)
            } else {
                return Ok(false);
            }
        };
        if is_owner {
            {
                let mut game = g_game().lock().unwrap();
                if let Some(player) = game.get_player_mut(player_id) {
                    player.shop_owner_id = None;
                    player.purchase_callback = -1;
                    player.sale_callback = -1;
                    player.shop_item_list.clear();
                }
            }
            let registry = crate::events::registry::g_script_registry();
            let mut r = registry.lock().unwrap();
            if buy_cb != -1 { r.lua_callbacks.remove(&buy_cb); }
            if sell_cb != -1 { r.lua_callbacks.remove(&sell_cb); }
            drop(r);
            crate::net::game_protocol::send_close_shop(player_id);
        }
        Ok(true)
    })?)?;

    // doSellItem(cid, itemid, amount, subtype, actionid, canDropOnMap) -> sell_count
    lua.globals().set("doSellItem", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<u32> {
        use crate::creatures::player::{CONST_SLOT_FIRST, CONST_SLOT_LAST};
        let mut it = args.into_iter();
        let player_id: u32 = match it.next() {
            Some(LuaValue::Table(t)) => t.raw_get::<u32>(1).unwrap_or(0),
            Some(LuaValue::Integer(n)) => n as u32,
            _ => return Ok(0),
        };
        let item_id: u16 = match it.next() {
            Some(LuaValue::Integer(n)) => n as u16,
            Some(LuaValue::Number(n)) => n as u16,
            _ => return Ok(0),
        };
        let amount: u32 = match it.next() {
            Some(LuaValue::Integer(n)) => n as u32,
            Some(LuaValue::Number(n)) => n as u32,
            _ => return Ok(0),
        };
        let sub_type: i32 = match it.next() {
            Some(LuaValue::Integer(n)) if n != -1 => n as i32,
            Some(LuaValue::Number(n)) if n as i32 != -1 => n as i32,
            _ => 1,
        };
        let _action_id: u32 = match it.next() {
            Some(LuaValue::Integer(n)) => n as u32,
            Some(LuaValue::Number(n)) => n as u32,
            _ => 0,
        };
        let can_drop_on_map: bool = match it.next() {
            Some(LuaValue::Boolean(b)) => b,
            Some(LuaValue::Integer(n)) => n != 0,
            _ => true,
        };
        let stackable = {
            let items = crate::items::g_items();
            items.get_item_type(item_id as usize).stackable
        };
        let mut sell_count = 0u32;
        let mut remaining = amount;
        while remaining > 0 {
            let stack_count = if stackable { remaining.min(100) } else { 1 };
            // Try to add to inventory
            let added = {
                let mut game = g_game().lock().unwrap();
                let player_pos = game.get_player(player_id).map(|p| p.base.position);
                let Some(player) = game.get_player_mut(player_id) else { break };
                let mut placed = false;
                if stackable {
                    // Find existing stack with room, then free slot
                    for slot in CONST_SLOT_FIRST..=CONST_SLOT_LAST {
                        if player.inventory[slot] == Some(item_id) {
                            let cur = player.inventory_count[slot] as u32;
                            if cur < 100 {
                                let add = stack_count.min(100 - cur);
                                player.inventory_count[slot] = (cur + add) as u16;
                                let count_out = player.inventory_count[slot];
                                crate::net::game_protocol::send_inventory_item_to_player(player_id, slot as u8, item_id, count_out);
                                sell_count += add;
                                remaining = remaining.saturating_sub(add);
                                placed = true;
                                break;
                            }
                        }
                    }
                    if !placed {
                        for slot in CONST_SLOT_FIRST..=CONST_SLOT_LAST {
                            if player.inventory[slot].is_none() {
                                player.inventory[slot] = Some(item_id);
                                player.inventory_count[slot] = stack_count as u16;
                                crate::net::game_protocol::send_inventory_item_to_player(player_id, slot as u8, item_id, stack_count as u16);
                                sell_count += stack_count;
                                remaining = remaining.saturating_sub(stack_count);
                                placed = true;
                                break;
                            }
                        }
                    }
                } else {
                    for slot in CONST_SLOT_FIRST..=CONST_SLOT_LAST {
                        if player.inventory[slot].is_none() {
                            player.inventory[slot] = Some(item_id);
                            player.inventory_count[slot] = sub_type.max(1) as u16;
                            crate::net::game_protocol::send_inventory_item_to_player(player_id, slot as u8, item_id, sub_type.max(1) as u16);
                            sell_count += 1;
                            placed = true;
                            break;
                        }
                    }
                }
                if !placed && can_drop_on_map {
                    if let Some(pos) = player_pos {
                        let count_val = if stackable { stack_count as u16 } else { sub_type.max(1) as u16 };
                        if let Some(tile) = game.map.get_tile_mut(pos) {
                            tile.items.push(crate::map::tile::MapItem {
                                server_id: item_id,
                                count: count_val,
                                ..Default::default()
                            });
                        }
                        sell_count += if stackable { stack_count } else { 1 };
                        placed = true;
                    }
                }
                placed
            };
            if !added { break; }
            if !stackable { remaining = remaining.saturating_sub(1); }
        }
        Ok(sell_count)
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Guild class
// ---------------------------------------------------------------------------

fn register_guild_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let arg = args.into_iter().nth(1);
        let guild_id: u32 = match arg {
            Some(LuaValue::Integer(id)) => id as u32,
            _ => return Ok(LuaValue::Nil),
        };
        if guild_id == 0 {
            return Ok(LuaValue::Nil);
        }
        let t = lua.create_table()?;
        t.raw_set(1, guild_id)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Guild") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(LuaValue::Table(t))
    })?;
    let methods = register_class(lua, "Guild", None, LUA_DATA_UNKNOWN, Some(ctor))?;

    set_meta(lua, "Guild", "__eq", lua.create_function(|_, (a, b): (LuaTable, LuaTable)| {
        let ia: u32 = a.raw_get(1).unwrap_or(0);
        let ib: u32 = b.raw_get(1).unwrap_or(0);
        Ok(ia == ib && ia != 0)
    })?)?;

    methods.set("getId", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        this.raw_get(1)
    })?)?;

    methods.set("getName", lua.create_function(|_, this: LuaTable| -> LuaResult<String> {
        let guild_id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_guild(guild_id).map(|g| g.name.clone()).unwrap_or_default())
    })?)?;

    methods.set("getMembersOnline", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let guild_id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        let t = lua.create_table()?;
        if let Some(guild) = game.get_guild(guild_id) {
            for (i, &member_id) in guild.get_members_online().iter().enumerate() {
                let pt = lua.create_table()?;
                pt.raw_set(1, member_id)?;
                if let Ok(mt) = lua.named_registry_value::<LuaTable>("Player") {
                    let _ = pt.set_metatable(Some(mt));
                }
                t.raw_set(i as i64 + 1, pt)?;
            }
        }
        Ok(t)
    })?)?;

    methods.set("addRank", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<bool> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Ok(false),
        };
        let guild_id: u32 = this.raw_get(1)?;
        let rank_id = match iter.next() { Some(LuaValue::Integer(n)) => n as u32, _ => return Ok(false) };
        let rank_name = match iter.next() { Some(LuaValue::String(s)) => s.to_string_lossy().to_string(), _ => return Ok(false) };
        let rank_level = match iter.next() { Some(LuaValue::Integer(n)) => n as u8, _ => return Ok(false) };
        let mut game = g_game().lock().unwrap();
        if let Some(guild) = game.get_guild_mut(guild_id) {
            guild.add_rank(rank_id, rank_name, rank_level);
            return Ok(true);
        }
        Ok(false)
    })?)?;

    methods.set("getRankById", lua.create_function(|lua, (this, id): (LuaTable, u32)| -> LuaResult<LuaValue> {
        let guild_id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        if let Some(guild) = game.get_guild(guild_id) {
            if let Some(rank) = guild.get_rank_by_id(id) {
                let t = lua.create_table()?;
                t.set("id", rank.id)?;
                t.set("name", rank.name.as_str())?;
                t.set("level", rank.level)?;
                return Ok(LuaValue::Table(t));
            }
        }
        Ok(LuaValue::Nil)
    })?)?;

    methods.set("getRankByLevel", lua.create_function(|lua, (this, level): (LuaTable, u8)| -> LuaResult<LuaValue> {
        let guild_id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        if let Some(guild) = game.get_guild(guild_id) {
            if let Some(rank) = guild.get_rank_by_level(level) {
                let t = lua.create_table()?;
                t.set("id", rank.id)?;
                t.set("name", rank.name.as_str())?;
                t.set("level", rank.level)?;
                return Ok(LuaValue::Table(t));
            }
        }
        Ok(LuaValue::Nil)
    })?)?;

    methods.set("getMotd", lua.create_function(|_, this: LuaTable| -> LuaResult<String> {
        let guild_id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_guild(guild_id).map(|g| g.motd.clone()).unwrap_or_default())
    })?)?;

    methods.set("setMotd", lua.create_function(|_, (this, motd): (LuaTable, String)| -> LuaResult<bool> {
        let guild_id: u32 = this.raw_get(1)?;
        let mut game = g_game().lock().unwrap();
        if let Some(guild) = game.get_guild_mut(guild_id) {
            guild.set_motd(motd);
            return Ok(true);
        }
        Ok(false)
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Group class
// ---------------------------------------------------------------------------

fn register_group_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let arg = args.into_iter().nth(1);
        let group_id: u16 = match arg {
            Some(LuaValue::Integer(id)) => id as u16,
            _ => return Ok(LuaValue::Nil),
        };
        let t = lua.create_table()?;
        t.raw_set(1, group_id)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Group") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(LuaValue::Table(t))
    })?;
    let methods = register_class(lua, "Group", None, LUA_DATA_UNKNOWN, Some(ctor))?;
    set_meta(lua, "Group", "__eq", lua.create_function(|_, (a, b): (LuaTable, LuaTable)| {
        let ia: u16 = a.raw_get(1).unwrap_or(0);
        let ib: u16 = b.raw_get(1).unwrap_or(0);
        Ok(ia == ib)
    })?)?;

    methods.set("getId", lua.create_function(|_, this: LuaTable| -> LuaResult<u16> {
        this.raw_get(1)
    })?)?;

    methods.set("getName", lua.create_function(|_, this: LuaTable| -> LuaResult<String> {
        let id: u16 = this.raw_get(1)?;
        Ok(match id {
            1 => "Player".to_owned(),
            2 => "Tutor".to_owned(),
            3 => "Senior Tutor".to_owned(),
            4 => "Gamemaster".to_owned(),
            5 => "Community Manager".to_owned(),
            6 => "God".to_owned(),
            _ => String::new(),
        })
    })?)?;

    methods.set("getFlags", lua.create_function(|_, this: LuaTable| -> LuaResult<u64> {
        let id: u16 = this.raw_get(1)?;
        Ok(crate::world::groups::flags_for_group_id(id as u32))
    })?)?;

    methods.set("getAccess", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let id: u16 = this.raw_get(1)?;
        Ok(crate::world::groups::access_for_group_id(id as u32))
    })?)?;

    methods.set("hasFlag", lua.create_function(|_, (this, flag): (LuaTable, u64)| -> LuaResult<bool> {
        let id: u16 = this.raw_get(1)?;
        let flags = crate::world::groups::flags_for_group_id(id as u32);
        Ok(flags & flag != 0)
    })?)?;

    methods.set("getMaxDepotItems", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        Ok(if id >= 4 { 2000 } else { 1000 })
    })?)?;

    methods.set("getMaxVipEntries", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        Ok(if id >= 4 { 200 } else { 100 })
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Vocation class
// ---------------------------------------------------------------------------

fn register_vocation_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let arg = args.into_iter().nth(1);
        let voc_id: u16 = match arg {
            Some(LuaValue::Integer(id)) => id as u16,
            _ => return Ok(LuaValue::Nil),
        };
        let t = lua.create_table()?;
        t.raw_set(1, voc_id)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Vocation") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(LuaValue::Table(t))
    })?;
    let methods = register_class(lua, "Vocation", None, LUA_DATA_UNKNOWN, Some(ctor))?;
    set_meta(lua, "Vocation", "__eq", lua.create_function(|_, (a, b): (LuaTable, LuaTable)| {
        let ia: u16 = a.raw_get(1).unwrap_or(0);
        let ib: u16 = b.raw_get(1).unwrap_or(0);
        Ok(ia == ib)
    })?)?;

    methods.set("getId", lua.create_function(|_, this: LuaTable| -> LuaResult<u16> {
        this.raw_get(1)
    })?)?;
    methods.set("getClientId", lua.create_function(|_, this: LuaTable| -> LuaResult<u16> {
        let id: u16 = this.raw_get(1)?;
        Ok(crate::world::vocation::g_vocations().get_vocation(id).map(|v| v.client_id).unwrap_or(id))
    })?)?;
    methods.set("getName", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaString> {
        let id: u16 = this.raw_get(1)?;
        let name = crate::world::vocation::g_vocations().get_vocation(id)
            .map(|v| v.name.clone()).unwrap_or_default();
        lua.create_string(&name)
    })?)?;
    methods.set("getDescription", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaString> {
        let id: u16 = this.raw_get(1)?;
        let desc = crate::world::vocation::g_vocations().get_vocation(id)
            .map(|v| v.description.clone()).unwrap_or_default();
        lua.create_string(&desc)
    })?)?;
    methods.set("getRequiredSkillTries", lua.create_function(|_, (this, skill, level): (LuaTable, u8, u16)| -> LuaResult<u64> {
        let id: u16 = this.raw_get(1)?;
        Ok(crate::world::vocation::g_vocations().get_vocation(id)
            .map(|v| v.get_req_skill_tries(skill, level)).unwrap_or(0))
    })?)?;
    methods.set("getRequiredManaSpent", lua.create_function(|_, (this, magic_level): (LuaTable, u32)| -> LuaResult<u64> {
        let id: u16 = this.raw_get(1)?;
        Ok(crate::world::vocation::g_vocations().get_vocation(id)
            .map(|v| v.get_req_mana(magic_level)).unwrap_or(0))
    })?)?;
    methods.set("getCapacityGain", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        Ok(crate::world::vocation::g_vocations().get_vocation(id).map(|v| v.gain_cap).unwrap_or(500))
    })?)?;
    methods.set("getHealthGain", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        Ok(crate::world::vocation::g_vocations().get_vocation(id).map(|v| v.gain_hp).unwrap_or(5))
    })?)?;
    methods.set("getHealthGainTicks", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        Ok(crate::world::vocation::g_vocations().get_vocation(id).map(|v| v.gain_health_ticks).unwrap_or(6))
    })?)?;
    methods.set("getHealthGainAmount", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        Ok(crate::world::vocation::g_vocations().get_vocation(id).map(|v| v.gain_health_amount).unwrap_or(1))
    })?)?;
    methods.set("getManaGain", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        Ok(crate::world::vocation::g_vocations().get_vocation(id).map(|v| v.gain_mana).unwrap_or(5))
    })?)?;
    methods.set("getManaGainTicks", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        Ok(crate::world::vocation::g_vocations().get_vocation(id).map(|v| v.gain_mana_ticks).unwrap_or(6))
    })?)?;
    methods.set("getManaGainAmount", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        Ok(crate::world::vocation::g_vocations().get_vocation(id).map(|v| v.gain_mana_amount).unwrap_or(1))
    })?)?;
    methods.set("getMaxSoul", lua.create_function(|_, this: LuaTable| -> LuaResult<u8> {
        let id: u16 = this.raw_get(1)?;
        Ok(crate::world::vocation::g_vocations().get_vocation(id).map(|v| v.soul_max).unwrap_or(100))
    })?)?;
    methods.set("getSoulGainTicks", lua.create_function(|_, this: LuaTable| -> LuaResult<u16> {
        let id: u16 = this.raw_get(1)?;
        Ok(crate::world::vocation::g_vocations().get_vocation(id).map(|v| v.gain_soul_ticks).unwrap_or(120))
    })?)?;
    methods.set("getAttackSpeed", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        Ok(crate::world::vocation::g_vocations().get_vocation(id).map(|v| v.attack_speed).unwrap_or(1500))
    })?)?;
    methods.set("getBaseSpeed", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        Ok(crate::world::vocation::g_vocations().get_vocation(id).map(|v| v.base_speed).unwrap_or(220))
    })?)?;
    methods.set("getDemotion", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let id: u16 = this.raw_get(1)?;
        if let Some(voc) = crate::world::vocation::g_vocations().get_vocation(id) {
            if voc.from_vocation != 0 && voc.from_vocation as u16 != id {
                let t = lua.create_table()?;
                t.raw_set(1, voc.from_vocation as u16)?;
                if let Ok(mt) = lua.named_registry_value::<LuaTable>("Vocation") {
                    let _ = t.set_metatable(Some(mt));
                }
                return Ok(LuaValue::Table(t));
            }
        }
        Ok(LuaValue::Nil)
    })?)?;
    methods.set("getPromotion", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let id: u16 = this.raw_get(1)?;
        let promo = crate::world::vocation::g_vocations().get_promoted_vocation(id);
        if promo != 0 {
            let t = lua.create_table()?;
            t.raw_set(1, promo)?;
            if let Ok(mt) = lua.named_registry_value::<LuaTable>("Vocation") {
                let _ = t.set_metatable(Some(mt));
            }
            return Ok(LuaValue::Table(t));
        }
        Ok(LuaValue::Nil)
    })?)?;
    methods.set("allowsPvp", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let id: u16 = this.raw_get(1)?;
        Ok(crate::world::vocation::g_vocations().get_vocation(id).map(|v| v.allow_pvp).unwrap_or(true))
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Town class
// ---------------------------------------------------------------------------

fn register_town_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let arg = args.into_iter().nth(1);
        let game = g_game().lock().unwrap();
        let town_id = match arg {
            Some(LuaValue::Integer(id)) => {
                if game.map.towns.contains_key(&(id as u32)) { Some(id as u32) } else { None }
            }
            Some(LuaValue::String(s)) => {
                let name = s.to_string_lossy();
                game.map.towns.values().find(|t| t.name.eq_ignore_ascii_case(&name)).map(|t| t.id)
            }
            _ => None,
        };
        match town_id {
            Some(id) => {
                let t = lua.create_table()?;
                t.raw_set(1, id)?;
                if let Ok(mt) = lua.named_registry_value::<LuaTable>("Town") {
                    let _ = t.set_metatable(Some(mt));
                }
                Ok(LuaValue::Table(t))
            }
            None => Ok(LuaValue::Nil),
        }
    })?;
    let methods = register_class(lua, "Town", None, LUA_DATA_UNKNOWN, Some(ctor))?;
    set_meta(lua, "Town", "__eq", lua.create_function(|_, (a, b): (LuaTable, LuaTable)| {
        let ia: u32 = a.raw_get(1).unwrap_or(0);
        let ib: u32 = b.raw_get(1).unwrap_or(0);
        Ok(ia == ib && ia != 0)
    })?)?;

    methods.set("getId", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        this.raw_get(1)
    })?)?;

    methods.set("getName", lua.create_function(|_, this: LuaTable| -> LuaResult<String> {
        let id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.map.towns.get(&id).map(|t| t.name.clone()).unwrap_or_default())
    })?)?;

    methods.set("getTemplePosition", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        let pos = game.map.towns.get(&id).map(|t| t.temple_pos).unwrap_or_default();
        push_position(lua, pos)
    })?)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// House class
// ---------------------------------------------------------------------------

fn register_house_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let house_id: u32 = match args.into_iter().nth(1) {
            Some(LuaValue::Integer(id)) => id as u32,
            _ => return Ok(LuaValue::Nil),
        };
        if house_id == 0 {
            return Ok(LuaValue::Nil);
        }
        let game = g_game().lock().unwrap();
        if game.map.houses.get_house(house_id).is_none() {
            return Ok(LuaValue::Nil);
        }
        drop(game);
        let t = lua.create_table()?;
        t.raw_set(1, house_id)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("House") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(LuaValue::Table(t))
    })?;
    let methods = register_class(lua, "House", None, LUA_DATA_UNKNOWN, Some(ctor))?;

    set_meta(lua, "House", "__eq", lua.create_function(|_, (a, b): (LuaTable, LuaTable)| {
        let ia: u32 = a.raw_get(1).unwrap_or(0);
        let ib: u32 = b.raw_get(1).unwrap_or(0);
        Ok(ia == ib && ia != 0)
    })?)?;

    methods.set("getId", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        this.raw_get(1)
    })?)?;

    methods.set("getName", lua.create_function(|_, this: LuaTable| -> LuaResult<String> {
        let id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.map.houses.get_house(id).map(|h| h.name.clone()).unwrap_or_default())
    })?)?;

    methods.set("getTown", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.map.houses.get_house(id).map(|h| h.town_id).unwrap_or(0))
    })?)?;

    methods.set("getExitPosition", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        let pos = game.map.houses.get_house(id).map(|h| h.entry_position).unwrap_or(Position { x: 0, y: 0, z: 0 });
        push_position(lua, pos)
    })?)?;

    methods.set("getRent", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.map.houses.get_house(id).map(|h| h.rent).unwrap_or(0))
    })?)?;

    methods.set("setRent", lua.create_function(|_, (this, rent): (LuaTable, u32)| -> LuaResult<()> {
        let id: u32 = this.raw_get(1)?;
        let mut game = g_game().lock().unwrap();
        if let Some(h) = game.map.houses.get_house_mut(id) {
            h.rent = rent;
        }
        Ok(())
    })?)?;

    methods.set("getPaidUntil", lua.create_function(|_, this: LuaTable| -> LuaResult<i64> {
        let id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.map.houses.get_house(id).map(|h| h.paid_until).unwrap_or(0))
    })?)?;

    methods.set("setPaidUntil", lua.create_function(|_, (this, val): (LuaTable, i64)| -> LuaResult<()> {
        let id: u32 = this.raw_get(1)?;
        let mut game = g_game().lock().unwrap();
        if let Some(h) = game.map.houses.get_house_mut(id) {
            h.paid_until = val;
        }
        Ok(())
    })?)?;

    methods.set("getPayRentWarnings", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.map.houses.get_house(id).map(|h| h.rent_warnings).unwrap_or(0))
    })?)?;

    methods.set("setPayRentWarnings", lua.create_function(|_, (this, val): (LuaTable, u32)| -> LuaResult<()> {
        let id: u32 = this.raw_get(1)?;
        let mut game = g_game().lock().unwrap();
        if let Some(h) = game.map.houses.get_house_mut(id) {
            h.rent_warnings = val;
        }
        Ok(())
    })?)?;

    methods.set("getOwnerGuid", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.map.houses.get_house(id).map(|h| h.owner).unwrap_or(0))
    })?)?;

    methods.set("setOwnerGuid", lua.create_function(|_, (this, val): (LuaTable, u32)| -> LuaResult<bool> {
        let id: u32 = this.raw_get(1)?;
        let prev_owner = {
            let mut game = g_game().lock().unwrap();
            if game.map.houses.get_house(id).is_none() {
                return Ok(false);
            }
            let prev = game.map.houses.get_house(id).map(|h| h.owner).unwrap_or(0);
            game.house_set_owner(id, val);
            prev
        };

        // DB writes (houses row + owner name/account resolution), async.
        if prev_owner != val {
            tokio::spawn(async move {
                use crate::db::DatabaseEngine;
                let db = crate::db::g_database();
                let _ = db
                    .execute(&format!(
                        "UPDATE `houses` SET `owner` = {val}, `bid` = 0, `bid_end` = 0, `last_bid` = 0, `highest_bidder` = 0 WHERE `id` = {id}"
                    ))
                    .await;
                if val != 0 {
                    let q = format!(
                        "SELECT `p`.`name` AS `name`, `p`.`account_id` AS `account_id` FROM `players` `p` WHERE `p`.`id` = {val}"
                    );
                    if let Ok(Some(res)) = db.store_query(&q).await {
                        let name = res.get_string("name").unwrap_or_default();
                        let account_id = res.get_u64("account_id").unwrap_or(0) as u32;
                        let mut game = g_game().lock().unwrap();
                        if let Some(h) = game.map.houses.get_house_mut(id) {
                            h.owner_name = name;
                            h.owner_account_id = account_id;
                        }
                    }
                }
            });
        }
        Ok(true)
    })?)?;

    methods.set("startTrade", lua.create_function(|_, _args: LuaMultiValue| -> LuaResult<bool> {
        Ok(false)
    })?)?;

    methods.set("getBeds", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        let tbl = lua.create_table()?;
        if let Some(house) = game.map.houses.get_house(id) {
            for (i, &bed_pos) in house.beds.iter().enumerate() {
                if let Some(tile) = game.map.get_tile(bed_pos) {
                    for (idx, item) in tile.items.iter().enumerate() {
                        let it = game.items.get_item_type(item.server_id as usize);
                        if it.kind == crate::items::ItemKind::Bed {
                            tbl.raw_set(i + 1, push_item_ref_attrs(lua, item.server_id, bed_pos, idx as i32, item.unique_id, item.action_id)?)?;
                            break;
                        }
                    }
                }
            }
        }
        Ok(tbl)
    })?)?;

    methods.set("getBedCount", lua.create_function(|_, this: LuaTable| -> LuaResult<i32> {
        let id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.map.houses.get_house(id).map(|h| h.beds.len() as i32).unwrap_or(0))
    })?)?;

    methods.set("getDoors", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        let tbl = lua.create_table()?;
        if let Some(house) = game.map.houses.get_house(id) {
            for (i, (&_door_id, &door_pos)) in house.doors.iter().enumerate() {
                if let Some(tile) = game.map.get_tile(door_pos) {
                    for (idx, item) in tile.items.iter().enumerate() {
                        let it = game.items.get_item_type(item.server_id as usize);
                        if it.kind == crate::items::ItemKind::Door {
                            tbl.raw_set(i + 1, push_item_ref_attrs(lua, item.server_id, door_pos, idx as i32, item.unique_id, item.action_id)?)?;
                            break;
                        }
                    }
                }
            }
        }
        Ok(tbl)
    })?)?;

    methods.set("getDoorCount", lua.create_function(|_, this: LuaTable| -> LuaResult<i32> {
        let id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.map.houses.get_house(id).map(|h| h.doors.len() as i32).unwrap_or(0))
    })?)?;

    methods.set("getDoorIdByPosition", lua.create_function(|_, (this, pos_tbl): (LuaTable, LuaTable)| -> LuaResult<i32> {
        let id: u32 = this.raw_get(1)?;
        let pos = table_to_position(&pos_tbl)?;
        let game = g_game().lock().unwrap();
        if let Some(house) = game.map.houses.get_house(id) {
            for (&door_id, &door_pos) in &house.doors {
                if door_pos == pos {
                    return Ok(door_id as i32);
                }
            }
        }
        Ok(-1)
    })?)?;

    methods.set("getTiles", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        let tbl = lua.create_table()?;
        if let Some(house) = game.map.houses.get_house(id) {
            for (i, &tile_pos) in house.tiles.iter().enumerate() {
                tbl.raw_set(i + 1, push_position(lua, tile_pos)?)?;
            }
        }
        Ok(tbl)
    })?)?;

    methods.set("getItems", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        let tbl = lua.create_table()?;
        let mut lua_idx = 1i64;
        if let Some(house) = game.map.houses.get_house(id) {
            for &tile_pos in &house.tiles {
                if let Some(tile) = game.map.get_tile(tile_pos) {
                    for (idx, item) in tile.items.iter().enumerate() {
                        tbl.raw_set(lua_idx, push_item_ref_attrs(lua, item.server_id, tile_pos, idx as i32, item.unique_id, item.action_id)?)?;
                        lua_idx += 1;
                    }
                }
            }
        }
        Ok(tbl)
    })?)?;

    methods.set("getTileCount", lua.create_function(|_, this: LuaTable| -> LuaResult<i32> {
        let id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.map.houses.get_house(id).map(|h| h.tiles.len() as i32).unwrap_or(0))
    })?)?;

    methods.set("canEditAccessList", lua.create_function(|_, (this, list_id, player_tbl): (LuaTable, u32, LuaTable)| -> LuaResult<bool> {
        let id: u32 = this.raw_get(1)?;
        let player_cid = get_creature_id(&player_tbl)?;
        let game = g_game().lock().unwrap();
        let Some(p) = game.get_player(player_cid) else { return Ok(false) };
        let guid = p.guid;
        let account_id = p.account_number;
        let can_edit_flag = crate::world::groups::access_for_group_id(p.group_id);
        let name = p.name.clone();
        let result = game.map.houses.get_house(id).map(|h| {
            let access_level = h.access_level_for(guid, account_id, can_edit_flag, false, &name, "", "");
            h.can_edit_access_list(list_id, access_level)
        }).unwrap_or(false);
        Ok(result)
    })?)?;

    methods.set("getAccessList", lua.create_function(|_, (this, list_id): (LuaTable, u32)| -> LuaResult<String> {
        let id: u32 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.map.houses.get_house(id)
            .and_then(|h| h.get_access_list(list_id))
            .unwrap_or("").to_string())
    })?)?;

    methods.set("setAccessList", lua.create_function(|_, (this, list_id, text): (LuaTable, u32, String)| -> LuaResult<()> {
        let id: u32 = this.raw_get(1)?;
        let mut game = g_game().lock().unwrap();
        if let Some(h) = game.map.houses.get_house_mut(id) {
            h.set_access_list(list_id, &text);
        }
        Ok(())
    })?)?;

    methods.set("kickPlayer", lua.create_function(|_, (this, player): (LuaTable, LuaTable)| -> LuaResult<bool> {
        let _id: u32 = this.raw_get(1)?;
        let _player_cid = get_creature_id(&player)?;
        Ok(true)
    })?)?;

    methods.set("save", lua.create_function(|_, _: LuaTable| -> LuaResult<bool> {
        Ok(true)
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// ItemType class
// ---------------------------------------------------------------------------

fn register_item_type_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let arg = args.into_iter().nth(1);
        let item_id: u16 = match arg {
            Some(LuaValue::Integer(id)) => id as u16,
            Some(LuaValue::String(s)) => {
                let name = s.to_string_lossy();
                let game = g_game().lock().unwrap();
                let items = &game.items;
                let mut found = 0u16;
                for i in 1..items.len() {
                    let it = items.get_item_type(i);
                    if it.name.eq_ignore_ascii_case(&name) {
                        found = it.id;
                        break;
                    }
                }
                found
            }
            _ => 0,
        };
        if item_id == 0 {
            return Ok(LuaValue::Nil);
        }
        let t = lua.create_table()?;
        t.raw_set(1, item_id)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("ItemType") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(LuaValue::Table(t))
    })?;
    let methods = register_class(lua, "ItemType", None, LUA_DATA_UNKNOWN, Some(ctor))?;
    set_meta(lua, "ItemType", "__eq", lua.create_function(|_, (a, b): (LuaTable, LuaTable)| {
        let ia: u16 = a.raw_get(1).unwrap_or(0);
        let ib: u16 = b.raw_get(1).unwrap_or(0);
        Ok(ia == ib && ia != 0)
    })?)?;

    macro_rules! item_prop {
        ($methods:ident, $lua:ident, $name:expr, $field:ident, $ty:ty) => {
            $methods.set($name, $lua.create_function(|_, this: LuaTable| -> LuaResult<$ty> {
                let id: u16 = this.raw_get(1)?;
                let game = g_game().lock().unwrap();
                let it = game.items.get_item_type(id as usize);
                Ok(it.$field as $ty)
            })?)?;
        };
    }

    macro_rules! item_bool {
        ($methods:ident, $lua:ident, $name:expr, $field:ident) => {
            $methods.set($name, $lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
                let id: u16 = this.raw_get(1)?;
                let game = g_game().lock().unwrap();
                let it = game.items.get_item_type(id as usize);
                Ok(it.$field)
            })?)?;
        };
    }

    methods.set("getId", lua.create_function(|_, this: LuaTable| -> LuaResult<u16> {
        this.raw_get(1)
    })?)?;

    item_prop!(methods, lua, "getClientId", client_id, u16);
    item_prop!(methods, lua, "getWeight", weight, u32);
    item_prop!(methods, lua, "getAttack", attack, i32);
    item_prop!(methods, lua, "getDefense", defense, i32);
    item_prop!(methods, lua, "getExtraDefense", extra_defense, i32);
    item_prop!(methods, lua, "getArmor", armor, i32);
    item_prop!(methods, lua, "getHitChance", hit_chance, i32);
    item_prop!(methods, lua, "getCharges", charges, u32);
    item_prop!(methods, lua, "getDecayId", decay_to, i32);
    item_prop!(methods, lua, "getLevelDoor", level_door, u32);
    item_prop!(methods, lua, "getCapacity", max_items, u32);

    item_bool!(methods, lua, "isStackable", stackable);
    item_bool!(methods, lua, "isMovable", moveable);
    item_bool!(methods, lua, "isPickupable", pickupable);
    item_bool!(methods, lua, "isUseable", useable);
    item_bool!(methods, lua, "isBlocking", block_solid);
    item_bool!(methods, lua, "hasShowAttributes", show_attributes);
    item_bool!(methods, lua, "hasShowCount", show_count);
    item_bool!(methods, lua, "hasShowCharges", show_charges);
    item_bool!(methods, lua, "hasShowDuration", show_duration);
    item_bool!(methods, lua, "hasAllowDistRead", allow_dist_read);
    item_bool!(methods, lua, "isWritable", can_write_text);
    item_bool!(methods, lua, "isReadable", can_read_text);
    item_bool!(methods, lua, "isRotatable", rotatable);

    methods.set("getName", lua.create_function(|_, this: LuaTable| -> LuaResult<String> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).name.clone())
    })?)?;

    methods.set("getPluralName", lua.create_function(|_, this: LuaTable| -> LuaResult<String> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).plural_name.clone())
    })?)?;

    methods.set("getArticle", lua.create_function(|_, this: LuaTable| -> LuaResult<String> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).article.clone())
    })?)?;

    methods.set("getDescription", lua.create_function(|_, this: LuaTable| -> LuaResult<String> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).description.clone())
    })?)?;

    methods.set("getWorth", lua.create_function(|_, this: LuaTable| -> LuaResult<u64> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).worth)
    })?)?;

    methods.set("getGroup", lua.create_function(|_, this: LuaTable| -> LuaResult<u8> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).group as u8)
    })?)?;

    methods.set("getType", lua.create_function(|_, this: LuaTable| -> LuaResult<u8> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).kind as u8)
    })?)?;

    methods.set("isGroundTile", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).is_ground_tile())
    })?)?;

    methods.set("isDoor", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).kind == crate::items::ItemKind::Door)
    })?)?;

    methods.set("isContainer", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).kind == crate::items::ItemKind::Container)
    })?)?;

    methods.set("isMagicField", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).kind == crate::items::ItemKind::MagicField)
    })?)?;

    methods.set("isRune", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).kind == crate::items::ItemKind::Rune)
    })?)?;

    methods.set("hasSubType", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        let it = game.items.get_item_type(id as usize);
        Ok(it.stackable || it.charges > 0 || it.kind == crate::items::ItemKind::Rune)
    })?)?;

    methods.set("getDuration", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).decay_time)
    })?)?;

    methods.set("getTransformEquipId", lua.create_function(|_, this: LuaTable| -> LuaResult<u16> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).transform_equip_to)
    })?)?;

    methods.set("getTransformDeEquipId", lua.create_function(|_, this: LuaTable| -> LuaResult<u16> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).transform_de_equip_to)
    })?)?;

    methods.set("isCorpse", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        let it = game.items.get_item_type(id as usize);
        Ok(it.kind == crate::items::ItemKind::Container && it.name.to_lowercase().contains("corpse"))
    })?)?;
    methods.set("isFluidContainer", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).group == crate::items::ItemGroup::Fluid)
    })?)?;
    methods.set("getSlotPosition", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(u32::from(game.items.get_item_type(id as usize).slot_position))
    })?)?;
    methods.set("getFluidSource", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(u32::from(game.items.get_item_type(id as usize).fluid_source))
    })?)?;
    methods.set("getShootRange", lua.create_function(|_, this: LuaTable| -> LuaResult<u8> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).shoot_range)
    })?)?;
    methods.set("getAttackSpeed", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).attack_speed)
    })?)?;
    methods.set("getWeaponType", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(u32::from(game.items.get_item_type(id as usize).weapon_type))
    })?)?;
    methods.set("getElementType", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(u32::from(game.items.get_item_type(id as usize).element_type))
    })?)?;
    methods.set("getElementDamage", lua.create_function(|_, this: LuaTable| -> LuaResult<i32> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(i32::from(game.items.get_item_type(id as usize).element_damage))
    })?)?;
    methods.set("getDestroyId", lua.create_function(|_, this: LuaTable| -> LuaResult<u16> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).destroy_to)
    })?)?;
    methods.set("getRequiredLevel", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).min_req_level)
    })?)?;
    methods.set("getAmmoType", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(u32::from(game.items.get_item_type(id as usize).ammo_type))
    })?)?;
    methods.set("getCorpseType", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(u32::from(game.items.get_item_type(id as usize).corpse_type))
    })?)?;
    methods.set("getAbilities", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        let it = game.items.get_item_type(id as usize);
        let tbl = lua.create_table()?;
        tbl.set("healthGain", 0u32)?;
        tbl.set("healthTicks", 0u32)?;
        tbl.set("manaGain", 0u32)?;
        tbl.set("manaTicks", 0u32)?;
        tbl.set("conditionImmunities", 0u32)?;
        tbl.set("conditionSuppressions", 0u32)?;
        tbl.set("speed", 0i32)?;
        tbl.set("elementDamage", it.element_damage)?;
        tbl.set("elementType", it.element_type)?;
        tbl.set("manaShield", false)?;
        tbl.set("invisible", false)?;
        tbl.set("regeneration", false)?;
        let stats = lua.create_table()?;
        tbl.set("stats", stats)?;
        let stats_pct = lua.create_table()?;
        tbl.set("statsPercent", stats_pct)?;
        let skills = lua.create_table()?;
        tbl.set("skills", skills)?;
        let special = lua.create_table()?;
        tbl.set("specialSkills", special)?;
        let field_absorb = lua.create_table()?;
        tbl.set("fieldAbsorbPercent", field_absorb)?;
        let absorb = lua.create_table()?;
        tbl.set("absorbPercent", absorb)?;
        Ok(tbl)
    })?)?;
    methods.set("getWieldInfo", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).wield_info)
    })?)?;
    methods.set("getRuneSpellName", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaString> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        lua.create_string(&game.items.get_item_type(id as usize).rune_spell_name)
    })?)?;
    methods.set("getVocationString", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaString> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        lua.create_string(&game.items.get_item_type(id as usize).vocation_string)
    })?)?;
    methods.set("getMinReqLevel", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).min_req_level)
    })?)?;
    methods.set("getMinReqMagicLevel", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        let id: u16 = this.raw_get(1)?;
        let game = g_game().lock().unwrap();
        Ok(game.items.get_item_type(id as usize).min_req_magic_level)
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Combat class
// ---------------------------------------------------------------------------

fn register_combat_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, _args: LuaMultiValue| -> LuaResult<LuaTable> {
        let t = lua.create_table()?;
        t.set("_params", lua.create_table()?)?;
        t.set("_formulaType", 0i32)?;
        t.set("_mina", 0.0f64)?;
        t.set("_minb", 0.0f64)?;
        t.set("_maxa", 0.0f64)?;
        t.set("_maxb", 0.0f64)?;
        t.set("_origin", 0i32)?;
        t.set("_areaId", 0u32)?;
        t.set("_conditions", lua.create_table()?)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Combat") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(t)
    })?;
    let methods = register_class(lua, "Combat", None, LUA_DATA_UNKNOWN, Some(ctor))?;

    set_meta(lua, "Combat", "__gc", lua.create_function(|_, _: LuaValue| -> LuaResult<()> {
        Ok(())
    })?)?;
    set_meta(lua, "Combat", "__eq", lua.create_function(|_, (_a, _b): (LuaValue, LuaValue)| -> LuaResult<bool> {
        Ok(false)
    })?)?;

    methods.set("delete", lua.create_function(|_, _: LuaTable| -> LuaResult<()> {
        Ok(())
    })?)?;

    methods.set("setParameter", lua.create_function(|_, (this, key, value): (LuaTable, i32, LuaValue)| -> LuaResult<bool> {
        let val: u32 = match value {
            LuaValue::Boolean(b) => if b { 1 } else { 0 },
            LuaValue::Integer(n) => n as u32,
            LuaValue::Number(n) => n as u32,
            _ => 0,
        };
        let params: LuaTable = this.get("_params")?;
        params.set(key, val)?;
        Ok(true)
    })?)?;

    methods.set("getParameter", lua.create_function(|_, (this, key): (LuaTable, i32)| -> LuaResult<LuaValue> {
        let params: LuaTable = this.get("_params")?;
        let val: Option<i32> = params.get(key)?;
        match val {
            Some(v) => Ok(LuaValue::Integer(v as i64)),
            None => Ok(LuaValue::Nil),
        }
    })?)?;

    methods.set("setFormula", lua.create_function(|_, (this, ftype, mina, minb, maxa, maxb): (LuaTable, i32, f64, f64, f64, f64)| -> LuaResult<bool> {
        this.set("_formulaType", ftype)?;
        this.set("_mina", mina)?;
        this.set("_minb", minb)?;
        this.set("_maxa", maxa)?;
        this.set("_maxb", maxb)?;
        Ok(true)
    })?)?;

    methods.set("setArea", lua.create_function(|_, (this, area_id): (LuaTable, u32)| -> LuaResult<bool> {
        this.set("_areaId", area_id)?;
        Ok(true)
    })?)?;

    methods.set("addCondition", lua.create_function(|_, (this, condition): (LuaTable, LuaValue)| -> LuaResult<bool> {
        if let LuaValue::Table(cond_t) = condition {
            let conditions: LuaTable = this.get("_conditions")?;
            let len = conditions.raw_len() as i64;
            conditions.raw_set(len + 1, cond_t)?;
            Ok(true)
        } else {
            Ok(false)
        }
    })?)?;

    methods.set("clearConditions", lua.create_function(|lua, this: LuaTable| -> LuaResult<bool> {
        this.set("_conditions", lua.create_table()?)?;
        Ok(true)
    })?)?;

    methods.set("setCallback", lua.create_function(|lua, (this, key, func): (LuaTable, i32, LuaValue)| -> LuaResult<bool> {
        let cb_key = format!("_callback_{key}");
        match func {
            LuaValue::Function(_) => {
                this.set(cb_key, func)?;
            }
            LuaValue::String(ref s) => {
                // C++ uses global function name string — look it up now
                let fname: String = s.to_string_lossy().to_owned();
                let f: LuaValue = lua.globals().get(fname.as_str())?;
                this.set(cb_key, f)?;
            }
            _ => {}
        }
        Ok(true)
    })?)?;

    methods.set("setOrigin", lua.create_function(|_, (this, origin): (LuaTable, i32)| -> LuaResult<bool> {
        this.set("_origin", origin)?;
        Ok(true)
    })?)?;

    methods.set("execute", lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<bool> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Ok(false),
        };
        let player_val = match iter.next() {
            Some(v) => v,
            _ => return Ok(false),
        };
        let var_val = iter.next().unwrap_or(LuaValue::Nil);

        let caster_id: u32 = match &player_val {
            LuaValue::Table(t) => t.raw_get::<u32>(1).unwrap_or(0),
            _ => return Ok(false),
        };
        if caster_id == 0 { return Ok(false); }

        let params: LuaTable = this.get("_params")
            .unwrap_or_else(|_| lua.create_table().unwrap());
        let combat_type: i32 = params.get::<i32>(1).unwrap_or(0);
        let effect: u8 = params.get::<u32>(2).map(|v| v as u8).unwrap_or(0);
        let distance_effect: u8 = params.get::<u32>(3).map(|v| v as u8).unwrap_or(0);

        let (level, mag_level, caster_pos) = {
            let game = g_game().lock().unwrap();
            match game.get_player(caster_id) {
                Some(p) => (p.level, p.mag_level, p.base.position),
                None => return Ok(false),
            }
        };

        // Resolve formula via levelMagicCallback (CALLBACK_PARAM_LEVELMAGICVALUE = 0)
        let callback: Option<LuaFunction> = this.get("_callback_0").ok();
        let (min_raw, max_raw) = if let Some(f) = callback {
            let pt = match &player_val { LuaValue::Table(t) => t.clone(), _ => return Ok(false) };
            match f.call::<(f64, f64)>((pt, level as f64, mag_level as f64)) {
                Ok((mn, mx)) => (mn, mx),
                Err(e) => { tracing::error!("Combat formula callback error: {e}"); return Ok(false); }
            }
        } else {
            let formula_type: i32 = this.get("_formulaType").unwrap_or(-1);
            if formula_type == 0 {
                let mina: f64 = this.get("_mina").unwrap_or(0.0);
                let minb: f64 = this.get("_minb").unwrap_or(0.0);
                let maxa: f64 = this.get("_maxa").unwrap_or(0.0);
                let maxb: f64 = this.get("_maxb").unwrap_or(0.0);
                let base = level as f64 / 5.0 + mag_level as f64 * 1.4 + 8.0;
                (base * mina + minb, base * maxa + maxb)
            } else {
                (0.0, 0.0)
            }
        };

        if min_raw == 0.0 && max_raw == 0.0 { return Ok(true); }

        let lo = min_raw.min(max_raw) as i32;
        let hi = min_raw.max(max_raw) as i32;
        let change = crate::util::normal_random(lo, hi);

        use crate::combat::combat::{Combat, CombatParams};
        use crate::combat::{CombatDamage, CombatType, CombatOrigin};
        let ct = CombatType::from_u16(combat_type.unsigned_abs() as u16);
        let area_id: u32 = this.get("_areaId").unwrap_or(0);
        let origin_raw: u8 = this.get("_origin").map(|v: i32| v as u8).unwrap_or(2);
        let origin = match origin_raw {
            1 => CombatOrigin::Condition, 2 => CombatOrigin::Spell,
            3 => CombatOrigin::Melee, 4 => CombatOrigin::Ranged, 5 => CombatOrigin::Wand,
            _ => CombatOrigin::None,
        };
        let cp = CombatParams { combat_type: ct, impact_effect: effect, distance_effect, ..CombatParams::default() };

        // COMBAT_PARAM_CREATEITEM (7): field/item spawned on each affected tile
        // (fire/poison/energy field runes). Mirrors C++ combat field creation.
        let create_item: u16 = {
            let params: LuaTable = this.get("_params")?;
            params.get::<Option<i32>>(7).ok().flatten().unwrap_or(0) as u16
        };

        // Determine variant type and target
        let (var_type, target_id, target_pos_opt): (i32, u32, Option<crate::map::Position>) = match &var_val {
            LuaValue::Table(t) => {
                let vt: i32 = t.get("type").unwrap_or(0);
                if vt == 1 {
                    let tid = t.get::<u32>("number").unwrap_or(caster_id);
                    (1, tid, None)
                } else if vt == 2 || vt == 3 {
                    let px: u16 = t.get("number").map(|v: u16| v).unwrap_or_else(|_|
                        t.get::<u16>("x").unwrap_or(caster_pos.x));
                    let py: u16 = t.get::<u16>("y").unwrap_or(caster_pos.y);
                    let pz: u8 = t.get::<u8>("z").unwrap_or(caster_pos.z);
                    let pos = crate::map::Position { x: px, y: py, z: pz };
                    (vt, 0, Some(pos))
                } else {
                    (0, caster_id, None)
                }
            }
            _ => (0, caster_id, None),
        };

        if var_type == 1 || target_pos_opt.is_none() {
            // Single-target combat
            let resolved_id = if target_id != 0 { target_id } else { caster_id };
            let target_pos = {
                let game = g_game().lock().unwrap();
                game.get_creature(resolved_id).map(|c| c.base().position).unwrap_or(caster_pos)
            };
            if distance_effect > 0 {
                crate::net::game_protocol::broadcast_distance_effect(caster_pos, target_pos, distance_effect);
            }
            // C++ Combat::doCombat adds the impact effect BEFORE doTargetCombat
            // (so the 0x83 precedes the heal's 0xA0 stats in the bundled frame).
            if effect > 0 {
                crate::net::game_protocol::broadcast_magic_effect(target_pos, effect);
            }
            let mut dmg = CombatDamage::new(ct);
            dmg.primary_value = change;
            dmg.origin = origin;
            Combat::do_target_combat(Some(caster_id), resolved_id, &mut dmg, &cp);
            if create_item != 0 {
                g_game().lock().unwrap().place_item_on_tile(target_pos, create_item);
            }
        } else if let Some(center) = target_pos_opt {
            if distance_effect > 0 {
                crate::net::game_protocol::broadcast_distance_effect(caster_pos, center, distance_effect);
            }
            let offsets: Vec<(i8, i8)> = {
                let patterns = get_area_patterns().lock().unwrap();
                patterns.get(&area_id).cloned().unwrap_or_else(|| vec![(0i8, 0i8)])
            };
            let affected_positions: Vec<crate::map::Position> = offsets.iter().map(|(dr, dc)| crate::map::Position {
                x: (center.x as i32 + *dc as i32) as u16,
                y: (center.y as i32 + *dr as i32) as u16,
                z: center.z,
            }).collect();
            let target_ids: Vec<u32> = {
                let game = g_game().lock().unwrap();
                let mut ids = Vec::new();
                for pos in &affected_positions {
                    if let Some(tile) = game.map.get_tile(*pos) {
                        ids.extend(tile.creature_ids.iter().copied());
                    }
                }
                ids
            };
            for tid in target_ids {
                let mut dmg = CombatDamage::new(ct);
                dmg.primary_value = crate::util::normal_random(lo, hi);
                dmg.origin = origin;
                Combat::do_target_combat(Some(caster_id), tid, &mut dmg, &cp);
            }
            if effect > 0 {
                for pos in &affected_positions {
                    crate::net::game_protocol::broadcast_magic_effect(*pos, effect);
                }
            }
            if create_item != 0 {
                let mut game = g_game().lock().unwrap();
                for pos in &affected_positions {
                    game.place_item_on_tile(*pos, create_item);
                }
            }
        }

        Ok(true)
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Condition class
// ---------------------------------------------------------------------------

fn register_condition_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let mut iter = args.into_iter().skip(1);
        let cond_type: i32 = match iter.next() {
            Some(LuaValue::Integer(n)) => n as i32,
            Some(LuaValue::Number(n)) => n as i32,
            _ => return Ok(LuaValue::Nil),
        };
        let cond_id: i32 = match iter.next() {
            Some(LuaValue::Integer(n)) => n as i32,
            Some(LuaValue::Number(n)) => n as i32,
            _ => 1, // CONDITIONID_COMBAT
        };
        let t = lua.create_table()?;
        t.set("_conditionType", cond_type)?;
        t.set("_conditionId", cond_id)?;
        t.set("_subId", 0u32)?;
        t.set("_ticks", 0i32)?;
        t.set("_endTime", 0i64)?;
        t.set("_icons", 0u32)?;
        t.set("_params", lua.create_table()?)?;
        t.set("_damages", lua.create_table()?)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Condition") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(LuaValue::Table(t))
    })?;
    let methods = register_class(lua, "Condition", None, LUA_DATA_UNKNOWN, Some(ctor))?;

    set_meta(lua, "Condition", "__gc", lua.create_function(|_, _: LuaValue| -> LuaResult<()> {
        Ok(())
    })?)?;
    set_meta(lua, "Condition", "__eq", lua.create_function(|_, (_a, _b): (LuaValue, LuaValue)| -> LuaResult<bool> {
        Ok(false)
    })?)?;

    methods.set("getId", lua.create_function(|_, this: LuaTable| -> LuaResult<i32> {
        Ok(this.get("_conditionId").unwrap_or(0))
    })?)?;

    methods.set("getSubId", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        Ok(this.get("_subId").unwrap_or(0))
    })?)?;

    methods.set("getType", lua.create_function(|_, this: LuaTable| -> LuaResult<i32> {
        Ok(this.get("_conditionType").unwrap_or(0))
    })?)?;

    methods.set("getIcons", lua.create_function(|_, this: LuaTable| -> LuaResult<u32> {
        Ok(this.get("_icons").unwrap_or(0))
    })?)?;

    methods.set("getEndTime", lua.create_function(|_, this: LuaTable| -> LuaResult<i64> {
        Ok(this.get("_endTime").unwrap_or(0))
    })?)?;

    methods.set("clone", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let t = lua.create_table()?;
        for pair in this.pairs::<LuaValue, LuaValue>() {
            let (k, v) = pair?;
            t.set(k, v)?;
        }
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Condition") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(t)
    })?)?;

    methods.set("getTicks", lua.create_function(|_, this: LuaTable| -> LuaResult<i32> {
        Ok(this.get("_ticks").unwrap_or(0))
    })?)?;

    methods.set("setTicks", lua.create_function(|_, (this, ticks): (LuaTable, i32)| -> LuaResult<bool> {
        this.set("_ticks", ticks)?;
        Ok(true)
    })?)?;

    methods.set("setParameter", lua.create_function(|_, (this, key, value): (LuaTable, i32, LuaValue)| -> LuaResult<bool> {
        let val: i32 = match value {
            LuaValue::Boolean(b) => if b { 1 } else { 0 },
            LuaValue::Integer(n) => n as i32,
            LuaValue::Number(n) => n as i32,
            _ => 0,
        };
        let params: LuaTable = this.get("_params")?;
        params.set(key, val)?;
        Ok(true)
    })?)?;

    methods.set("getParameter", lua.create_function(|_, (this, key): (LuaTable, i32)| -> LuaResult<LuaValue> {
        let params: LuaTable = this.get("_params")?;
        let val: Option<i32> = params.get(key)?;
        match val {
            Some(v) => Ok(LuaValue::Integer(v as i64)),
            None => Ok(LuaValue::Nil),
        }
    })?)?;

    methods.set("setFormula", lua.create_function(|_, (this, mina, minb, maxa, maxb): (LuaTable, f64, f64, f64, f64)| -> LuaResult<bool> {
        this.set("_formulaMina", mina)?;
        this.set("_formulaMinb", minb)?;
        this.set("_formulaMaxa", maxa)?;
        this.set("_formulaMaxb", maxb)?;
        Ok(true)
    })?)?;

    methods.set("setOutfit", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<bool> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Ok(false),
        };
        let outfit = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Ok(false),
        };
        this.set("_outfit", outfit)?;
        Ok(true)
    })?)?;

    methods.set("addDamage", lua.create_function(|lua, (this, rounds, time, value): (LuaTable, i32, i32, i32)| -> LuaResult<bool> {
        let damages: LuaTable = this.get("_damages")?;
        let len = damages.raw_len() as i64;
        let entry = lua.create_table()?;
        entry.set("rounds", rounds)?;
        entry.set("time", time)?;
        entry.set("value", value)?;
        damages.raw_set(len + 1, entry)?;
        Ok(true)
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Outfit class
// ---------------------------------------------------------------------------

fn register_outfit_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let _looktype = match args.into_iter().nth(1) {
            Some(LuaValue::Integer(n)) => n as u16,
            _ => return Ok(LuaValue::Nil),
        };
        let t = lua.create_table()?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Outfit") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(LuaValue::Table(t))
    })?;
    let _methods = register_class(lua, "Outfit", None, LUA_DATA_UNKNOWN, Some(ctor))?;
    set_meta(lua, "Outfit", "__eq", lua.create_function(|_, (_a, _b): (LuaValue, LuaValue)| -> LuaResult<bool> {
        Ok(false)
    })?)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// MonsterType class
// ---------------------------------------------------------------------------

fn register_monster_type_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let name = match args.into_iter().nth(1) {
            Some(LuaValue::String(s)) => s.to_string_lossy(),
            _ => return Ok(LuaValue::Nil),
        };
        let t = lua.create_table()?;
        t.set("_name", name)?;
        t.set("_nameDescription", String::new())?;
        t.set("_health", 0i32)?;
        t.set("_maxHealth", 0i32)?;
        t.set("_runHealth", 0i32)?;
        t.set("_experience", 0u64)?;
        t.set("_skull", 0u32)?;
        t.set("_combatImmunities", 0u32)?;
        t.set("_conditionImmunities", 0u32)?;
        t.set("_armor", 0i32)?;
        t.set("_defense", 0i32)?;
        t.set("_race", 0i32)?;
        t.set("_corpseId", 0u16)?;
        t.set("_manaCost", 0u32)?;
        t.set("_baseSpeed", 200u32)?;
        t.set("_lightLevel", 0u32)?;
        t.set("_lightColor", 0u32)?;
        t.set("_staticAttackChance", 95u32)?;
        t.set("_targetDistance", 1u32)?;
        t.set("_yellChance", 0u32)?;
        t.set("_yellSpeedTicks", 0u32)?;
        t.set("_changeTargetChance", 0u32)?;
        t.set("_changeTargetSpeed", 0u32)?;
        t.set("_maxSummons", 0u32)?;
        t.set("_isAttackable", true)?;
        t.set("_isChallengeable", true)?;
        t.set("_isConvinceable", false)?;
        t.set("_isSummonable", false)?;
        t.set("_isIgnoringSpawnBlock", false)?;
        t.set("_isIllusionable", false)?;
        t.set("_isHostile", true)?;
        t.set("_isPushable", false)?;
        t.set("_isHealthHidden", false)?;
        t.set("_isBoss", false)?;
        t.set("_canPushItems", false)?;
        t.set("_canPushCreatures", false)?;
        t.set("_canWalkOnEnergy", false)?;
        t.set("_canWalkOnFire", false)?;
        t.set("_canWalkOnPoison", false)?;
        t.set("_attacks", lua.create_table()?)?;
        t.set("_defenses", lua.create_table()?)?;
        t.set("_elements", lua.create_table()?)?;
        t.set("_voices", lua.create_table()?)?;
        t.set("_loot", lua.create_table()?)?;
        t.set("_events", lua.create_table()?)?;
        t.set("_summons", lua.create_table()?)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("MonsterType") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(LuaValue::Table(t))
    })?;
    let methods = register_class(lua, "MonsterType", None, LUA_DATA_UNKNOWN, Some(ctor))?;

    set_meta(lua, "MonsterType", "__eq", lua.create_function(|_, (a, b): (LuaTable, LuaTable)| {
        let na: String = a.get("_name").unwrap_or_default();
        let nb: String = b.get("_name").unwrap_or_default();
        Ok(!na.is_empty() && na.eq_ignore_ascii_case(&nb))
    })?)?;

    macro_rules! mt_getter_setter_bool {
        ($methods:ident, $lua:ident, $name:expr, $field:expr) => {
            $methods.set($name, $lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaValue> {
                let mut iter = args.into_iter();
                let this = match iter.next() {
                    Some(LuaValue::Table(t)) => t,
                    _ => return Ok(LuaValue::Nil),
                };
                match iter.next() {
                    Some(LuaValue::Boolean(b)) => { this.set($field, b)?; Ok(LuaValue::Table(this)) }
                    None => { let v: bool = this.get($field).unwrap_or(false); Ok(LuaValue::Boolean(v)) }
                    _ => Ok(LuaValue::Table(this)),
                }
            })?)?;
        };
    }

    macro_rules! mt_getter_setter_u32 {
        ($methods:ident, $lua:ident, $name:expr, $field:expr) => {
            $methods.set($name, $lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaValue> {
                let mut iter = args.into_iter();
                let this = match iter.next() {
                    Some(LuaValue::Table(t)) => t,
                    _ => return Ok(LuaValue::Nil),
                };
                match iter.next() {
                    Some(LuaValue::Integer(n)) => { this.set($field, n as u32)?; Ok(LuaValue::Table(this)) }
                    Some(LuaValue::Number(n)) => { this.set($field, n as u32)?; Ok(LuaValue::Table(this)) }
                    None => { let v: u32 = this.get($field).unwrap_or(0); Ok(LuaValue::Integer(v as i64)) }
                    _ => Ok(LuaValue::Table(this)),
                }
            })?)?;
        };
    }

    macro_rules! mt_getter_setter_i32 {
        ($methods:ident, $lua:ident, $name:expr, $field:expr) => {
            $methods.set($name, $lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaValue> {
                let mut iter = args.into_iter();
                let this = match iter.next() {
                    Some(LuaValue::Table(t)) => t,
                    _ => return Ok(LuaValue::Nil),
                };
                match iter.next() {
                    Some(LuaValue::Integer(n)) => { this.set($field, n as i32)?; Ok(LuaValue::Table(this)) }
                    Some(LuaValue::Number(n)) => { this.set($field, n as i32)?; Ok(LuaValue::Table(this)) }
                    None => { let v: i32 = this.get($field).unwrap_or(0); Ok(LuaValue::Integer(v as i64)) }
                    _ => Ok(LuaValue::Table(this)),
                }
            })?)?;
        };
    }

    mt_getter_setter_bool!(methods, lua, "isAttackable", "_isAttackable");
    mt_getter_setter_bool!(methods, lua, "isChallengeable", "_isChallengeable");
    mt_getter_setter_bool!(methods, lua, "isConvinceable", "_isConvinceable");
    mt_getter_setter_bool!(methods, lua, "isSummonable", "_isSummonable");
    mt_getter_setter_bool!(methods, lua, "isIgnoringSpawnBlock", "_isIgnoringSpawnBlock");
    mt_getter_setter_bool!(methods, lua, "isIllusionable", "_isIllusionable");
    mt_getter_setter_bool!(methods, lua, "isHostile", "_isHostile");
    mt_getter_setter_bool!(methods, lua, "isPushable", "_isPushable");
    mt_getter_setter_bool!(methods, lua, "isHealthHidden", "_isHealthHidden");
    mt_getter_setter_bool!(methods, lua, "isBoss", "_isBoss");
    mt_getter_setter_bool!(methods, lua, "canPushItems", "_canPushItems");
    mt_getter_setter_bool!(methods, lua, "canPushCreatures", "_canPushCreatures");
    mt_getter_setter_bool!(methods, lua, "canWalkOnEnergy", "_canWalkOnEnergy");
    mt_getter_setter_bool!(methods, lua, "canWalkOnFire", "_canWalkOnFire");
    mt_getter_setter_bool!(methods, lua, "canWalkOnPoison", "_canWalkOnPoison");

    methods.set("name", lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Ok(LuaValue::Nil),
        };
        match iter.next() {
            Some(LuaValue::String(s)) => { this.set("_name", s.to_string_lossy())?; Ok(LuaValue::Table(this)) }
            None => { let v: String = this.get("_name").unwrap_or_default(); Ok(LuaValue::String(lua.create_string(&v)?)) }
            _ => Ok(LuaValue::Table(this)),
        }
    })?)?;

    methods.set("nameDescription", lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Ok(LuaValue::Nil),
        };
        match iter.next() {
            Some(LuaValue::String(s)) => { this.set("_nameDescription", s.to_string_lossy())?; Ok(LuaValue::Table(this)) }
            None => { let v: String = this.get("_nameDescription").unwrap_or_default(); Ok(LuaValue::String(lua.create_string(&v)?)) }
            _ => Ok(LuaValue::Table(this)),
        }
    })?)?;

    mt_getter_setter_i32!(methods, lua, "health", "_health");
    mt_getter_setter_i32!(methods, lua, "maxHealth", "_maxHealth");
    mt_getter_setter_i32!(methods, lua, "runHealth", "_runHealth");

    methods.set("experience", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Ok(LuaValue::Nil),
        };
        match iter.next() {
            Some(LuaValue::Integer(n)) => { this.set("_experience", n as u64)?; Ok(LuaValue::Table(this)) }
            Some(LuaValue::Number(n)) => { this.set("_experience", n as u64)?; Ok(LuaValue::Table(this)) }
            None => { let v: u64 = this.get("_experience").unwrap_or(0); Ok(LuaValue::Integer(v as i64)) }
            _ => Ok(LuaValue::Table(this)),
        }
    })?)?;

    mt_getter_setter_u32!(methods, lua, "skull", "_skull");
    mt_getter_setter_u32!(methods, lua, "combatImmunities", "_combatImmunities");
    mt_getter_setter_u32!(methods, lua, "conditionImmunities", "_conditionImmunities");
    mt_getter_setter_i32!(methods, lua, "armor", "_armor");
    mt_getter_setter_i32!(methods, lua, "defense", "_defense");
    mt_getter_setter_i32!(methods, lua, "race", "_race");

    methods.set("corpseId", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Ok(LuaValue::Nil),
        };
        match iter.next() {
            Some(LuaValue::Integer(n)) => { this.set("_corpseId", n as u16)?; Ok(LuaValue::Table(this)) }
            None => { let v: u16 = this.get("_corpseId").unwrap_or(0); Ok(LuaValue::Integer(v as i64)) }
            _ => Ok(LuaValue::Table(this)),
        }
    })?)?;

    mt_getter_setter_u32!(methods, lua, "manaCost", "_manaCost");
    mt_getter_setter_u32!(methods, lua, "baseSpeed", "_baseSpeed");
    mt_getter_setter_u32!(methods, lua, "staticAttackChance", "_staticAttackChance");
    mt_getter_setter_u32!(methods, lua, "targetDistance", "_targetDistance");
    mt_getter_setter_u32!(methods, lua, "yellChance", "_yellChance");
    mt_getter_setter_u32!(methods, lua, "yellSpeedTicks", "_yellSpeedTicks");
    mt_getter_setter_u32!(methods, lua, "changeTargetChance", "_changeTargetChance");
    mt_getter_setter_u32!(methods, lua, "changeTargetSpeed", "_changeTargetSpeed");
    mt_getter_setter_u32!(methods, lua, "maxSummons", "_maxSummons");

    methods.set("light", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Ok(LuaValue::Nil),
        };
        match iter.next() {
            Some(LuaValue::Integer(level)) => {
                this.set("_lightLevel", level as u32)?;
                if let Some(LuaValue::Integer(color)) = iter.next() {
                    this.set("_lightColor", color as u32)?;
                }
                Ok(LuaValue::Table(this))
            }
            _ => Ok(LuaValue::Table(this)),
        }
    })?)?;

    methods.set("outfit", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Ok(LuaValue::Nil),
        };
        match iter.next() {
            Some(LuaValue::Table(outfit)) => { this.set("_outfit", outfit)?; Ok(LuaValue::Table(this)) }
            _ => Ok(LuaValue::Table(this)),
        }
    })?)?;

    macro_rules! mt_list_getter {
        ($methods:ident, $lua:ident, $name:expr, $field:expr) => {
            $methods.set($name, $lua.create_function(|_, this: LuaTable| -> LuaResult<LuaTable> {
                this.get($field)
            })?)?;
        };
    }

    macro_rules! mt_list_adder {
        ($methods:ident, $lua:ident, $name:expr, $field:expr) => {
            $methods.set($name, $lua.create_function(|_, (this, item): (LuaTable, LuaValue)| -> LuaResult<LuaTable> {
                let list: LuaTable = this.get($field)?;
                let len = list.raw_len() as i64;
                list.raw_set(len + 1, item)?;
                Ok(this)
            })?)?;
        };
    }

    mt_list_getter!(methods, lua, "getAttackList", "_attacks");
    mt_list_adder!(methods, lua, "addAttack", "_attacks");
    mt_list_getter!(methods, lua, "getDefenseList", "_defenses");
    mt_list_adder!(methods, lua, "addDefense", "_defenses");
    mt_list_getter!(methods, lua, "getElementList", "_elements");
    mt_list_adder!(methods, lua, "addElement", "_elements");
    mt_list_getter!(methods, lua, "getVoices", "_voices");

    methods.set("addVoice", lua.create_function(|lua, (this, text, interval, chance, yell): (LuaTable, String, u32, u32, bool)| -> LuaResult<LuaTable> {
        let voices: LuaTable = this.get("_voices")?;
        let len = voices.raw_len() as i64;
        let entry = lua.create_table()?;
        entry.set("text", text)?;
        entry.set("interval", interval)?;
        entry.set("chance", chance)?;
        entry.set("yell", yell)?;
        voices.raw_set(len + 1, entry)?;
        Ok(this)
    })?)?;

    mt_list_getter!(methods, lua, "getLoot", "_loot");
    mt_list_adder!(methods, lua, "addLoot", "_loot");
    mt_list_getter!(methods, lua, "getCreatureEvents", "_events");

    methods.set("registerEvent", lua.create_function(|_, (this, name): (LuaTable, String)| -> LuaResult<LuaTable> {
        let events: LuaTable = this.get("_events")?;
        let len = events.raw_len() as i64;
        events.raw_set(len + 1, name)?;
        Ok(this)
    })?)?;

    methods.set("eventType", lua.create_function(|_, (this, _etype): (LuaTable, i32)| -> LuaResult<LuaTable> {
        Ok(this)
    })?)?;

    let on_callback = lua.create_function(|_, this: LuaTable| -> LuaResult<LuaTable> {
        Ok(this)
    })?;
    for name in &["onThink", "onAppear", "onDisappear", "onMove", "onSay"] {
        methods.set(*name, on_callback.clone())?;
    }

    mt_list_getter!(methods, lua, "getSummonList", "_summons");

    methods.set("addSummon", lua.create_function(|lua, (this, name, interval, chance, max): (LuaTable, String, Option<u32>, Option<u32>, Option<u32>)| -> LuaResult<LuaTable> {
        let summons: LuaTable = this.get("_summons")?;
        let len = summons.raw_len() as i64;
        let entry = lua.create_table()?;
        entry.set("name", name)?;
        entry.set("interval", interval.unwrap_or(1000))?;
        entry.set("chance", chance.unwrap_or(100))?;
        entry.set("max", max.unwrap_or(1))?;
        summons.raw_set(len + 1, entry)?;
        Ok(this)
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Loot class
// ---------------------------------------------------------------------------

fn register_loot_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, _args: LuaMultiValue| -> LuaResult<LuaTable> {
        let t = lua.create_table()?;
        t.set("_id", 0u16)?;
        t.set("_maxCount", 1u32)?;
        t.set("_subType", -1i32)?;
        t.set("_chance", 100000u32)?;
        t.set("_actionId", 0u16)?;
        t.set("_description", String::new())?;
        t.set("_children", lua.create_table()?)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Loot") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(t)
    })?;
    let methods = register_class(lua, "Loot", None, LUA_DATA_UNKNOWN, Some(ctor))?;

    set_meta(lua, "Loot", "__gc", lua.create_function(|_, _: LuaValue| -> LuaResult<()> {
        Ok(())
    })?)?;

    methods.set("delete", lua.create_function(|_, _: LuaTable| -> LuaResult<()> {
        Ok(())
    })?)?;

    methods.set("setId", lua.create_function(|_, (this, id): (LuaTable, u16)| -> LuaResult<LuaTable> {
        this.set("_id", id)?;
        Ok(this)
    })?)?;

    methods.set("setMaxCount", lua.create_function(|_, (this, val): (LuaTable, u32)| -> LuaResult<LuaTable> {
        this.set("_maxCount", val)?;
        Ok(this)
    })?)?;

    methods.set("setSubType", lua.create_function(|_, (this, val): (LuaTable, i32)| -> LuaResult<LuaTable> {
        this.set("_subType", val)?;
        Ok(this)
    })?)?;

    methods.set("setChance", lua.create_function(|_, (this, val): (LuaTable, u32)| -> LuaResult<LuaTable> {
        this.set("_chance", val)?;
        Ok(this)
    })?)?;

    methods.set("setActionId", lua.create_function(|_, (this, val): (LuaTable, u16)| -> LuaResult<LuaTable> {
        this.set("_actionId", val)?;
        Ok(this)
    })?)?;

    methods.set("setDescription", lua.create_function(|_, (this, val): (LuaTable, String)| -> LuaResult<LuaTable> {
        this.set("_description", val)?;
        Ok(this)
    })?)?;

    methods.set("addChildLoot", lua.create_function(|_, (this, child): (LuaTable, LuaTable)| -> LuaResult<LuaTable> {
        let children: LuaTable = this.get("_children")?;
        let len = children.raw_len() as i64;
        children.raw_set(len + 1, child)?;
        Ok(this)
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// MonsterSpell class
// ---------------------------------------------------------------------------

fn register_monster_spell_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, _args: LuaMultiValue| -> LuaResult<LuaTable> {
        let t = lua.create_table()?;
        t.set("_type", String::new())?;
        t.set("_scriptName", String::new())?;
        t.set("_chance", 0u32)?;
        t.set("_interval", 2000u32)?;
        t.set("_range", 0u32)?;
        t.set("_combatMinValue", 0i32)?;
        t.set("_combatMaxValue", 0i32)?;
        t.set("_combatType", 0i32)?;
        t.set("_attackValue", 0i32)?;
        t.set("_needTarget", false)?;
        t.set("_needDirection", false)?;
        t.set("_combatLength", 0i32)?;
        t.set("_combatSpread", 0i32)?;
        t.set("_combatRadius", 0i32)?;
        t.set("_combatRing", 0i32)?;
        t.set("_conditionType", 0i32)?;
        t.set("_conditionDamage", 0i32)?;
        t.set("_conditionSpeedChange", 0i32)?;
        t.set("_conditionDuration", 0i32)?;
        t.set("_conditionDrunkenness", 0u8)?;
        t.set("_conditionTickInterval", 0i32)?;
        t.set("_combatShootEffect", 0u8)?;
        t.set("_combatEffect", 0u8)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("MonsterSpell") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(t)
    })?;
    let methods = register_class(lua, "MonsterSpell", None, LUA_DATA_UNKNOWN, Some(ctor))?;

    set_meta(lua, "MonsterSpell", "__gc", lua.create_function(|_, _: LuaValue| -> LuaResult<()> {
        Ok(())
    })?)?;

    methods.set("delete", lua.create_function(|_, _: LuaTable| -> LuaResult<()> {
        Ok(())
    })?)?;

    macro_rules! ms_setter_string {
        ($methods:ident, $lua:ident, $name:expr, $field:expr) => {
            $methods.set($name, $lua.create_function(|_, (this, val): (LuaTable, String)| -> LuaResult<LuaTable> {
                this.set($field, val)?;
                Ok(this)
            })?)?;
        };
    }

    macro_rules! ms_setter_u32 {
        ($methods:ident, $lua:ident, $name:expr, $field:expr) => {
            $methods.set($name, $lua.create_function(|_, (this, val): (LuaTable, u32)| -> LuaResult<LuaTable> {
                this.set($field, val)?;
                Ok(this)
            })?)?;
        };
    }

    macro_rules! ms_setter_i32 {
        ($methods:ident, $lua:ident, $name:expr, $field:expr) => {
            $methods.set($name, $lua.create_function(|_, (this, val): (LuaTable, i32)| -> LuaResult<LuaTable> {
                this.set($field, val)?;
                Ok(this)
            })?)?;
        };
    }

    macro_rules! ms_setter_bool {
        ($methods:ident, $lua:ident, $name:expr, $field:expr) => {
            $methods.set($name, $lua.create_function(|_, (this, val): (LuaTable, bool)| -> LuaResult<LuaTable> {
                this.set($field, val)?;
                Ok(this)
            })?)?;
        };
    }

    macro_rules! ms_setter_u8 {
        ($methods:ident, $lua:ident, $name:expr, $field:expr) => {
            $methods.set($name, $lua.create_function(|_, (this, val): (LuaTable, u8)| -> LuaResult<LuaTable> {
                this.set($field, val)?;
                Ok(this)
            })?)?;
        };
    }

    ms_setter_string!(methods, lua, "setType", "_type");
    ms_setter_string!(methods, lua, "setScriptName", "_scriptName");
    ms_setter_u32!(methods, lua, "setChance", "_chance");
    ms_setter_u32!(methods, lua, "setInterval", "_interval");
    ms_setter_u32!(methods, lua, "setRange", "_range");

    methods.set("setCombatValue", lua.create_function(|_, (this, min, max): (LuaTable, i32, i32)| -> LuaResult<LuaTable> {
        this.set("_combatMinValue", min)?;
        this.set("_combatMaxValue", max)?;
        Ok(this)
    })?)?;

    ms_setter_i32!(methods, lua, "setCombatType", "_combatType");
    ms_setter_i32!(methods, lua, "setAttackValue", "_attackValue");
    ms_setter_bool!(methods, lua, "setNeedTarget", "_needTarget");
    ms_setter_bool!(methods, lua, "setNeedDirection", "_needDirection");
    ms_setter_i32!(methods, lua, "setCombatLength", "_combatLength");
    ms_setter_i32!(methods, lua, "setCombatSpread", "_combatSpread");
    ms_setter_i32!(methods, lua, "setCombatRadius", "_combatRadius");
    ms_setter_i32!(methods, lua, "setCombatRing", "_combatRing");
    ms_setter_i32!(methods, lua, "setConditionType", "_conditionType");
    ms_setter_i32!(methods, lua, "setConditionDamage", "_conditionDamage");
    ms_setter_i32!(methods, lua, "setConditionSpeedChange", "_conditionSpeedChange");
    ms_setter_i32!(methods, lua, "setConditionDuration", "_conditionDuration");
    ms_setter_u8!(methods, lua, "setConditionDrunkenness", "_conditionDrunkenness");
    ms_setter_i32!(methods, lua, "setConditionTickInterval", "_conditionTickInterval");
    ms_setter_u8!(methods, lua, "setCombatShootEffect", "_combatShootEffect");
    ms_setter_u8!(methods, lua, "setCombatEffect", "_combatEffect");

    Ok(())
}

// ---------------------------------------------------------------------------
// Party class
// ---------------------------------------------------------------------------

fn get_party_leader_id(this: &LuaTable) -> LuaResult<CreatureId> {
    this.raw_get::<CreatureId>(1)
}

fn register_party_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let arg = args.into_iter().nth(1);
        let player_id: CreatureId = match arg {
            Some(LuaValue::Table(t)) => match get_creature_id(&t) {
                Ok(id) => id,
                Err(_) => return Ok(LuaValue::Nil),
            },
            Some(LuaValue::Integer(n)) => n as CreatureId,
            _ => return Ok(LuaValue::Nil),
        };
        let mut game = g_game().lock().unwrap();
        if game.get_player(player_id).is_none() {
            return Ok(LuaValue::Nil);
        }
        let has_party = game.get_player(player_id)
            .and_then(|p| p.party_id)
            .is_some();
        if has_party {
            return Ok(LuaValue::Nil);
        }
        game.create_party(player_id);
        drop(game);
        let t = lua.create_table()?;
        t.raw_set(1, player_id)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Party") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(LuaValue::Table(t))
    })?;
    let methods = register_class(lua, "Party", None, LUA_DATA_UNKNOWN, Some(ctor))?;

    set_meta(lua, "Party", "__eq", lua.create_function(|_, (a, b): (LuaTable, LuaTable)| -> LuaResult<bool> {
        let ia: u32 = a.raw_get(1).unwrap_or(0);
        let ib: u32 = b.raw_get(1).unwrap_or(0);
        Ok(ia == ib && ia != 0)
    })?)?;

    methods.set("disband", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let leader_id = get_party_leader_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if game.get_party(leader_id).is_some() {
            game.remove_party(leader_id);
            return Ok(true);
        }
        Ok(false)
    })?)?;

    methods.set("getLeader", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaValue> {
        let leader_id = get_party_leader_id(&this)?;
        let game = g_game().lock().unwrap();
        if game.get_party(leader_id).is_some() {
            let t = lua.create_table()?;
            t.raw_set(1, leader_id)?;
            if let Ok(mt) = lua.named_registry_value::<LuaTable>("Player") {
                let _ = t.set_metatable(Some(mt));
            }
            return Ok(LuaValue::Table(t));
        }
        Ok(LuaValue::Nil)
    })?)?;

    methods.set("setLeader", lua.create_function(|_, (this, player): (LuaTable, LuaTable)| -> LuaResult<bool> {
        let leader_id = get_party_leader_id(&this)?;
        let new_leader_id = get_creature_id(&player)?;
        let mut game = g_game().lock().unwrap();
        if let Some(party) = game.get_party_mut(leader_id) {
            if party.get_members().contains(&new_leader_id) {
                party.leader_id = new_leader_id;
                return Ok(true);
            }
        }
        Ok(false)
    })?)?;

    methods.set("getMembers", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let leader_id = get_party_leader_id(&this)?;
        let game = g_game().lock().unwrap();
        let t = lua.create_table()?;
        if let Some(party) = game.get_party(leader_id) {
            for (i, &member_id) in party.get_members().iter().enumerate() {
                let pt = lua.create_table()?;
                pt.raw_set(1, member_id)?;
                if let Ok(mt) = lua.named_registry_value::<LuaTable>("Player") {
                    let _ = pt.set_metatable(Some(mt));
                }
                t.raw_set(i as i64 + 1, pt)?;
            }
        }
        Ok(t)
    })?)?;

    methods.set("getMemberCount", lua.create_function(|_, this: LuaTable| -> LuaResult<i32> {
        let leader_id = get_party_leader_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_party(leader_id).map(|p| p.get_member_count() as i32).unwrap_or(0))
    })?)?;

    methods.set("getInvitees", lua.create_function(|lua, this: LuaTable| -> LuaResult<LuaTable> {
        let leader_id = get_party_leader_id(&this)?;
        let game = g_game().lock().unwrap();
        let t = lua.create_table()?;
        if let Some(party) = game.get_party(leader_id) {
            for (i, &invitee_id) in party.get_invitees().iter().enumerate() {
                let pt = lua.create_table()?;
                pt.raw_set(1, invitee_id)?;
                if let Ok(mt) = lua.named_registry_value::<LuaTable>("Player") {
                    let _ = pt.set_metatable(Some(mt));
                }
                t.raw_set(i as i64 + 1, pt)?;
            }
        }
        Ok(t)
    })?)?;

    methods.set("getInviteeCount", lua.create_function(|_, this: LuaTable| -> LuaResult<i32> {
        let leader_id = get_party_leader_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_party(leader_id).map(|p| p.get_invitation_count() as i32).unwrap_or(0))
    })?)?;

    methods.set("addInvite", lua.create_function(|_, (this, player): (LuaTable, LuaTable)| -> LuaResult<bool> {
        let leader_id = get_party_leader_id(&this)?;
        let player_id = get_creature_id(&player)?;
        let mut game = g_game().lock().unwrap();
        if let Some(party) = game.get_party_mut(leader_id) {
            if !party.is_player_invited(player_id) {
                party.invite_ids_mut().push(player_id);
                return Ok(true);
            }
        }
        Ok(false)
    })?)?;

    methods.set("removeInvite", lua.create_function(|_, (this, player): (LuaTable, LuaTable)| -> LuaResult<bool> {
        let leader_id = get_party_leader_id(&this)?;
        let player_id = get_creature_id(&player)?;
        let mut game = g_game().lock().unwrap();
        if let Some(party) = game.get_party_mut(leader_id) {
            return Ok(party.remove_invite(player_id, false));
        }
        Ok(false)
    })?)?;

    methods.set("addMember", lua.create_function(|_, (this, player): (LuaTable, LuaTable)| -> LuaResult<bool> {
        let leader_id = get_party_leader_id(&this)?;
        let player_id = get_creature_id(&player)?;
        let mut game = g_game().lock().unwrap();
        if let Some(party) = game.get_party_mut(leader_id) {
            party.member_ids_mut().push(player_id);
        }
        if let Some(player) = game.get_player_mut(player_id) {
            player.party_id = Some(leader_id);
        }
        Ok(true)
    })?)?;

    methods.set("removeMember", lua.create_function(|_, (this, player): (LuaTable, LuaTable)| -> LuaResult<bool> {
        let leader_id = get_party_leader_id(&this)?;
        let player_id = get_creature_id(&player)?;
        let mut game = g_game().lock().unwrap();
        if let Some(party) = game.get_party_mut(leader_id) {
            party.member_ids_mut().retain(|&id| id != player_id);
        }
        if let Some(player) = game.get_player_mut(player_id) {
            player.party_id = None;
        }
        Ok(true)
    })?)?;

    methods.set("isSharedExperienceActive", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let leader_id = get_party_leader_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_party(leader_id).map(|p| p.is_shared_experience_active()).unwrap_or(false))
    })?)?;

    methods.set("isSharedExperienceEnabled", lua.create_function(|_, this: LuaTable| -> LuaResult<bool> {
        let leader_id = get_party_leader_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_party(leader_id).map(|p| p.is_shared_experience_enabled()).unwrap_or(false))
    })?)?;

    methods.set("shareExperience", lua.create_function(|_, (this, _exp): (LuaTable, u64)| -> LuaResult<bool> {
        let leader_id = get_party_leader_id(&this)?;
        let game = g_game().lock().unwrap();
        Ok(game.get_party(leader_id).is_some())
    })?)?;

    methods.set("setSharedExperience", lua.create_function(|_, (this, active): (LuaTable, bool)| -> LuaResult<bool> {
        let leader_id = get_party_leader_id(&this)?;
        let mut game = g_game().lock().unwrap();
        if let Some(party) = game.get_party_mut(leader_id) {
            party.set_shared_exp_enabled(active);
            return Ok(true);
        }
        Ok(false)
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Spell class
// ---------------------------------------------------------------------------

fn register_spell_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let arg = args.into_iter().nth(1);
        let spell_type: i32 = match arg {
            Some(LuaValue::Integer(n)) => n as i32,
            Some(LuaValue::String(s)) => {
                match s.to_string_lossy().to_ascii_lowercase().as_str() {
                    "instant" => 1,
                    "rune" => 2,
                    _ => 0,
                }
            }
            _ => {
                tracing::error!("[Error - Spell::luaSpellCreate] There is no parameter set!");
                return Ok(LuaValue::Nil);
            }
        };
        if spell_type == 0 {
            return Ok(LuaValue::Nil);
        }
        let t = lua.create_table()?;
        t.set("_fromLua", true)?;
        t.set("_scripted", false)?;
        t.set("_spellType", spell_type)?;
        t.set("_name", String::new())?;
        t.set("_spellId", 0u32)?;
        t.set("_group", 0u32)?;
        t.set("_cooldown", 1000u32)?;
        t.set("_groupCooldown", 1000u32)?;
        t.set("_level", 0u32)?;
        t.set("_magicLevel", 0u32)?;
        t.set("_mana", 0u32)?;
        t.set("_manaPercent", 0u32)?;
        t.set("_soul", 0u32)?;
        t.set("_range", -1i32)?;
        t.set("_premium", false)?;
        t.set("_enabled", true)?;
        t.set("_needTarget", false)?;
        t.set("_needWeapon", false)?;
        t.set("_needLearn", false)?;
        t.set("_selfTarget", false)?;
        t.set("_blocking", false)?;
        t.set("_aggressive", true)?;
        t.set("_pzLock", false)?;
        t.set("_words", String::new())?;
        t.set("_needDirection", false)?;
        t.set("_hasParams", false)?;
        t.set("_hasPlayerNameParam", false)?;
        t.set("_needCasterTargetOrDirection", false)?;
        t.set("_blockingWalls", false)?;
        t.set("_runeLevel", 0u32)?;
        t.set("_runeMagicLevel", 0u32)?;
        t.set("_runeId", 0u16)?;
        t.set("_charges", 0u32)?;
        t.set("_allowFarUse", false)?;
        t.set("_blockWalls", false)?;
        t.set("_checkFloor", true)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Spell") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(LuaValue::Table(t))
    })?;
    let methods = register_class(lua, "Spell", None, LUA_DATA_UNKNOWN, Some(ctor))?;

    set_meta(lua, "Spell", "__eq", lua.create_function(|_, (a, b): (LuaTable, LuaTable)| {
        let na: String = a.get("_name").unwrap_or_default();
        let nb: String = b.get("_name").unwrap_or_default();
        Ok(!na.is_empty() && na == nb)
    })?)?;

    methods.set("onCastSpell", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaTable> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Err(LuaError::runtime("Spell:onCastSpell - self expected")),
        };
        if let Some(LuaValue::Function(f)) = iter.next() {
            this.raw_set("onCastSpell", f)?;
        }
        this.raw_set("_scripted", true)?;
        Ok(this)
    })?)?;

    methods.set("register", lua.create_function(|lua, this: LuaTable| -> LuaResult<bool> {
        let callback: Option<LuaFunction> = this.get("onCastSpell").ok();
        if callback.is_none() {
            let scripted: bool = this.get("_scripted").unwrap_or(false);
            if !scripted {
                return Ok(false);
            }
        }

        let name: String = this.get("_name").unwrap_or_default();
        let words: String = this.get("_words").unwrap_or_default();
        let spell_type: i32 = this.get("_spellType").unwrap_or(1);
        let level: u32 = this.get("_level").unwrap_or(0);
        let magic_level: u32 = this.get("_magicLevel").unwrap_or(0);
        let mana: u32 = this.get("_mana").unwrap_or(0);
        let mana_percent: u32 = this.get("_manaPercent").unwrap_or(0);
        let soul: u32 = this.get("_soul").unwrap_or(0);
        let group: u32 = this.get("_group").unwrap_or(0);
        let cooldown: u32 = this.get("_cooldown").unwrap_or(1000);
        let group_cooldown: u32 = this.get("_groupCooldown").unwrap_or(1000);
        let need_target: bool = this.get("_needTarget").unwrap_or(false);
        let need_weapon: bool = this.get("_needWeapon").unwrap_or(false);
        let need_learn: bool = this.get("_needLearn").unwrap_or(false);
        let self_target: bool = this.get("_selfTarget").unwrap_or(false);
        let aggressive: bool = this.get("_aggressive").unwrap_or(true);
        let pz_lock: bool = this.get("_pzLock").unwrap_or(false);
        let has_params: bool = this.get("_hasParams").unwrap_or(false);
        let has_player_name_param: bool = this.get("_hasPlayerNameParam").unwrap_or(false);
        let enabled: bool = this.get("_enabled").unwrap_or(true);

        let mut registry = crate::events::registry::g_script_registry().lock()
            .expect("ScriptRegistry mutex poisoned");
        let script_id = if let Some(func) = callback {
            let key = lua.create_registry_value(func)?;
            let id = registry.next_id();
            registry.lua_callbacks.insert(id, key);
            id
        } else {
            0
        };

        if !words.is_empty() && enabled {
            let entry = crate::events::registry::SpellEntry {
                name: name.clone(),
                words: words.clone(),
                script_id,
                spell_type,
                level,
                magic_level,
                mana,
                mana_percent,
                soul,
                group,
                cooldown,
                group_cooldown,
                need_target,
                need_weapon,
                need_learn,
                self_target,
                aggressive,
                pz_lock,
                has_params,
                has_player_name_param,
                enabled,
            };
            registry.spells.insert(words.to_lowercase(), entry);
        }

        tracing::debug!("[Lua] Spell '{name}' registered (from Lua)");
        Ok(true)
    })?)?;

    macro_rules! spell_getter_setter_u32 {
        ($methods:ident, $lua:ident, $name:expr, $field:expr) => {
            $methods.set($name, $lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaValue> {
                let mut iter = args.into_iter();
                let this = match iter.next() {
                    Some(LuaValue::Table(t)) => t,
                    _ => return Ok(LuaValue::Nil),
                };
                match iter.next() {
                    Some(LuaValue::Integer(n)) => {
                        this.set($field, n as u32)?;
                        Ok(LuaValue::Table(this))
                    }
                    Some(LuaValue::Number(n)) => {
                        this.set($field, n as u32)?;
                        Ok(LuaValue::Table(this))
                    }
                    None => {
                        let val: u32 = this.get($field).unwrap_or(0);
                        Ok(LuaValue::Integer(val as i64))
                    }
                    _ => Ok(LuaValue::Table(this)),
                }
            })?)?;
        };
    }

    macro_rules! spell_getter_setter_bool {
        ($methods:ident, $lua:ident, $name:expr, $field:expr) => {
            $methods.set($name, $lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaValue> {
                let mut iter = args.into_iter();
                let this = match iter.next() {
                    Some(LuaValue::Table(t)) => t,
                    _ => return Ok(LuaValue::Nil),
                };
                match iter.next() {
                    Some(LuaValue::Boolean(b)) => {
                        this.set($field, b)?;
                        Ok(LuaValue::Table(this))
                    }
                    None => {
                        let val: bool = this.get($field).unwrap_or(false);
                        Ok(LuaValue::Boolean(val))
                    }
                    _ => Ok(LuaValue::Table(this)),
                }
            })?)?;
        };
    }

    spell_getter_setter_u32!(methods, lua, "id", "_spellId");
    spell_getter_setter_u32!(methods, lua, "cooldown", "_cooldown");
    spell_getter_setter_u32!(methods, lua, "groupCooldown", "_groupCooldown");
    spell_getter_setter_u32!(methods, lua, "level", "_level");
    spell_getter_setter_u32!(methods, lua, "magicLevel", "_magicLevel");
    spell_getter_setter_u32!(methods, lua, "mana", "_mana");
    spell_getter_setter_u32!(methods, lua, "manaPercent", "_manaPercent");
    spell_getter_setter_u32!(methods, lua, "soul", "_soul");
    spell_getter_setter_u32!(methods, lua, "charges", "_charges");
    spell_getter_setter_u32!(methods, lua, "runeLevel", "_runeLevel");
    spell_getter_setter_u32!(methods, lua, "runeMagicLevel", "_runeMagicLevel");

    spell_getter_setter_bool!(methods, lua, "isPremium", "_premium");
    spell_getter_setter_bool!(methods, lua, "isEnabled", "_enabled");
    spell_getter_setter_bool!(methods, lua, "needTarget", "_needTarget");
    spell_getter_setter_bool!(methods, lua, "needWeapon", "_needWeapon");
    spell_getter_setter_bool!(methods, lua, "needLearn", "_needLearn");
    spell_getter_setter_bool!(methods, lua, "isSelfTarget", "_selfTarget");
    spell_getter_setter_bool!(methods, lua, "isBlocking", "_blocking");
    spell_getter_setter_bool!(methods, lua, "isAggressive", "_aggressive");
    spell_getter_setter_bool!(methods, lua, "isPzLock", "_pzLock");
    spell_getter_setter_bool!(methods, lua, "needDirection", "_needDirection");
    spell_getter_setter_bool!(methods, lua, "hasParams", "_hasParams");
    spell_getter_setter_bool!(methods, lua, "hasPlayerNameParam", "_hasPlayerNameParam");
    spell_getter_setter_bool!(methods, lua, "needCasterTargetOrDirection", "_needCasterTargetOrDirection");
    spell_getter_setter_bool!(methods, lua, "isBlockingWalls", "_blockingWalls");
    spell_getter_setter_bool!(methods, lua, "allowFarUse", "_allowFarUse");
    spell_getter_setter_bool!(methods, lua, "blockWalls", "_blockWalls");
    spell_getter_setter_bool!(methods, lua, "checkFloor", "_checkFloor");

    methods.set("name", lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Ok(LuaValue::Nil),
        };
        match iter.next() {
            Some(LuaValue::String(s)) => {
                this.set("_name", s.to_string_lossy())?;
                Ok(LuaValue::Table(this))
            }
            None => {
                let val: String = this.get("_name").unwrap_or_default();
                Ok(LuaValue::String(lua.create_string(&val)?))
            }
            _ => Ok(LuaValue::Table(this)),
        }
    })?)?;

    methods.set("words", lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Ok(LuaValue::Nil),
        };
        match iter.next() {
            Some(LuaValue::String(s)) => {
                this.set("_words", s.to_string_lossy())?;
                Ok(LuaValue::Table(this))
            }
            None => {
                let val: String = this.get("_words").unwrap_or_default();
                Ok(LuaValue::String(lua.create_string(&val)?))
            }
            _ => Ok(LuaValue::Table(this)),
        }
    })?)?;

    methods.set("group", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Ok(LuaValue::Nil),
        };
        match iter.next() {
            Some(LuaValue::String(s)) => {
                let group: u32 = match s.to_string_lossy().to_ascii_lowercase().as_str() {
                    "attack" | "1" => 1,
                    "healing" | "2" => 2,
                    "support" | "3" => 3,
                    "special" | "4" => 4,
                    _ => 0,
                };
                this.set("_group", group)?;
                Ok(LuaValue::Table(this))
            }
            Some(LuaValue::Integer(n)) => {
                this.set("_group", n as u32)?;
                Ok(LuaValue::Table(this))
            }
            None => {
                let val: u32 = this.get("_group").unwrap_or(0);
                Ok(LuaValue::Integer(val as i64))
            }
            _ => Ok(LuaValue::Table(this)),
        }
    })?)?;

    methods.set("range", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaValue> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Ok(LuaValue::Nil),
        };
        match iter.next() {
            Some(LuaValue::Integer(n)) => {
                this.set("_range", n as i32)?;
                Ok(LuaValue::Table(this))
            }
            None => {
                let val: i32 = this.get("_range").unwrap_or(-1);
                Ok(LuaValue::Integer(val as i64))
            }
            _ => Ok(LuaValue::Table(this)),
        }
    })?)?;

    methods.set("runeId", lua.create_function(|_, (this, val): (LuaTable, u16)| -> LuaResult<LuaTable> {
        this.set("_runeId", val)?;
        Ok(this)
    })?)?;

    methods.set("vocation", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaTable> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Err(LuaError::runtime("Spell:vocation - self expected")),
        };
        Ok(this)
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Action class
// ---------------------------------------------------------------------------

fn register_action_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, _args: LuaMultiValue| -> LuaResult<LuaTable> {
        let t = lua.create_table()?;
        t.set("_fromLua", true)?;
        t.set("_scripted", false)?;
        t.set("_allowFarUse", false)?;
        t.set("_checkLineOfSight", true)?;
        t.set("_checkFloor", true)?;
        t.set("_itemIds", lua.create_table()?)?;
        t.set("_uniqueIds", lua.create_table()?)?;
        t.set("_actionIds", lua.create_table()?)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Action") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(t)
    })?;
    let methods = register_class(lua, "Action", None, LUA_DATA_UNKNOWN, Some(ctor))?;

    methods.set("onUse", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaTable> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Err(LuaError::runtime("Action:onUse - self expected")),
        };
        if let Some(LuaValue::Function(f)) = iter.next() {
            this.raw_set("onUse", f)?;
        }
        this.raw_set("_scripted", true)?;
        Ok(this)
    })?)?;

    methods.set("register", lua.create_function(|lua, this: LuaTable| -> LuaResult<bool> {
        let callback: Option<LuaFunction> = this.get("onUse").ok();
        if callback.is_none() {
            let scripted: bool = this.get("_scripted").unwrap_or(false);
            if !scripted {
                return Ok(false);
            }
        }

        let item_ids = lua_table_to_u16_vec(&this, "_itemIds")?;
        let unique_ids = lua_table_to_u16_vec(&this, "_uniqueIds")?;
        let action_ids = lua_table_to_u16_vec(&this, "_actionIds")?;

        let mut registry = crate::events::registry::g_script_registry().lock()
            .expect("ScriptRegistry mutex poisoned");
        let script_id = if let Some(func) = callback {
            let key = lua.create_registry_value(func)?;
            let id = registry.next_id();
            registry.lua_callbacks.insert(id, key);
            id
        } else {
            0
        };

        let action = crate::events::actions::Action {
            script_id,
            script_interface_name: String::from("Scripts Interface"),
            allow_far_use: this.get("_allowFarUse").unwrap_or(false),
            check_floor: this.get("_checkFloor").unwrap_or(true),
            check_line_of_sight: this.get("_checkLineOfSight").unwrap_or(true),
            item_ids: item_ids.clone(),
            unique_ids: unique_ids.clone(),
            action_ids: action_ids.clone(),
            scripted: true,
            from_lua: true,
        };

        let result = registry.actions.register_lua_event(action);
        tracing::debug!("[Lua] Action registered (ids={:?}, aids={:?}, uids={:?})", item_ids, action_ids, unique_ids);
        Ok(result)
    })?)?;

    methods.set("id", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaTable> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Err(LuaError::runtime("Action:id - self expected")),
        };
        let ids: LuaTable = this.get("_itemIds")?;
        let mut len: i64 = ids.raw_len() as i64;
        for val in iter {
            if let LuaValue::Integer(id) = val {
                len += 1;
                ids.raw_set(len, id as u32)?;
            } else if let LuaValue::Number(id) = val {
                len += 1;
                ids.raw_set(len, id as u32)?;
            }
        }
        Ok(this)
    })?)?;

    methods.set("aid", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaTable> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Err(LuaError::runtime("Action:aid - self expected")),
        };
        let ids: LuaTable = this.get("_actionIds")?;
        let mut len: i64 = ids.raw_len() as i64;
        for val in iter {
            if let LuaValue::Integer(id) = val {
                len += 1;
                ids.raw_set(len, id as u32)?;
            } else if let LuaValue::Number(id) = val {
                len += 1;
                ids.raw_set(len, id as u32)?;
            }
        }
        Ok(this)
    })?)?;

    methods.set("uid", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaTable> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Err(LuaError::runtime("Action:uid - self expected")),
        };
        let ids: LuaTable = this.get("_uniqueIds")?;
        let mut len: i64 = ids.raw_len() as i64;
        for val in iter {
            if let LuaValue::Integer(id) = val {
                len += 1;
                ids.raw_set(len, id as u32)?;
            } else if let LuaValue::Number(id) = val {
                len += 1;
                ids.raw_set(len, id as u32)?;
            }
        }
        Ok(this)
    })?)?;

    methods.set("allowFarUse", lua.create_function(|_, (this, val): (LuaTable, bool)| -> LuaResult<LuaTable> {
        this.set("_allowFarUse", val)?;
        Ok(this)
    })?)?;

    methods.set("blockWalls", lua.create_function(|_, (this, val): (LuaTable, bool)| -> LuaResult<LuaTable> {
        this.set("_checkLineOfSight", val)?;
        Ok(this)
    })?)?;

    methods.set("checkFloor", lua.create_function(|_, (this, val): (LuaTable, bool)| -> LuaResult<LuaTable> {
        this.set("_checkFloor", val)?;
        Ok(this)
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// TalkAction class
// ---------------------------------------------------------------------------

fn register_talk_action_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaTable> {
        let t = lua.create_table()?;
        t.set("_fromLua", true)?;
        t.set("_scripted", false)?;
        t.set("_needAccess", false)?;
        t.set("_requiredAccountType", 0u8)?;
        t.set("_separator", "\"".to_string())?;
        let words_table = lua.create_table()?;
        let mut idx = 1i64;
        for val in args.into_iter().skip(1) {
            if let LuaValue::String(s) = val {
                words_table.raw_set(idx, s.to_string_lossy())?;
                idx += 1;
            }
        }
        t.set("_words", words_table)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("TalkAction") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(t)
    })?;
    let methods = register_class(lua, "TalkAction", None, LUA_DATA_UNKNOWN, Some(ctor))?;

    methods.set("onSay", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaTable> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Err(LuaError::runtime("TalkAction:onSay - self expected")),
        };
        if let Some(LuaValue::Function(f)) = iter.next() {
            this.raw_set("onSay", f)?;
        }
        this.raw_set("_scripted", true)?;
        Ok(this)
    })?)?;

    methods.set("register", lua.create_function(|lua, this: LuaTable| -> LuaResult<bool> {
        let callback: Option<LuaFunction> = this.get("onSay").ok();
        if callback.is_none() {
            let scripted: bool = this.get("_scripted").unwrap_or(false);
            if !scripted {
                return Ok(false);
            }
        }

        let words = lua_table_to_string_vec(&this, "_words")?;
        if words.is_empty() {
            tracing::warn!("[Warning - TalkAction::register] no words set");
            return Ok(false);
        }

        let mut registry = crate::events::registry::g_script_registry().lock()
            .expect("ScriptRegistry mutex poisoned");
        let script_id = if let Some(func) = callback {
            let key = lua.create_registry_value(func)?;
            let id = registry.next_id();
            registry.lua_callbacks.insert(id, key);
            id
        } else {
            0
        };

        let ta = crate::events::talk::TalkAction {
            script_id,
            words: words.first().cloned().unwrap_or_default(),
            words_map: words.clone(),
            separator: this.get("_separator").unwrap_or_else(|_| String::from("\"")),
            need_access: this.get("_needAccess").unwrap_or(false),
            required_account_type: this.get("_requiredAccountType").unwrap_or(0),
            scripted: true,
            from_lua: true,
        };

        let result = registry.talk_actions.register_lua_event(ta);
        tracing::debug!("[Lua] TalkAction registered (words={:?})", words);
        Ok(result)
    })?)?;

    methods.set("separator", lua.create_function(|_, (this, sep): (LuaTable, String)| -> LuaResult<LuaTable> {
        this.set("_separator", sep)?;
        Ok(this)
    })?)?;

    methods.set("access", lua.create_function(|_, (this, val): (LuaTable, bool)| -> LuaResult<LuaTable> {
        this.set("_needAccess", val)?;
        Ok(this)
    })?)?;

    methods.set("accountType", lua.create_function(|_, (this, val): (LuaTable, u8)| -> LuaResult<LuaTable> {
        this.set("_requiredAccountType", val)?;
        Ok(this)
    })?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// CreatureEvent class
// ---------------------------------------------------------------------------

fn register_creature_event_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaTable> {
        let t = lua.create_table()?;
        t.set("_fromLua", true)?;
        t.set("_scripted", false)?;
        t.set("_loaded", false)?;
        t.set("_eventType", 0i32)?;
        let name = match args.into_iter().nth(1) {
            Some(LuaValue::String(s)) => s.to_string_lossy(),
            _ => String::new(),
        };
        t.set("_name", name)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("CreatureEvent") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(t)
    })?;
    let methods = register_class(lua, "CreatureEvent", None, LUA_DATA_UNKNOWN, Some(ctor))?;

    methods.set("type", lua.create_function(|_, (this, type_name): (LuaTable, String)| -> LuaResult<LuaTable> {
        let event_type: i32 = match type_name.to_ascii_lowercase().as_str() {
            "login" => 1,
            "logout" => 2,
            "think" => 3,
            "preparedeath" => 4,
            "death" => 5,
            "kill" => 6,
            "advance" => 7,
            "textedit" => 8,
            "healthchange" => 9,
            "manachange" => 10,
            "extendedopcode" => 11,
            other => {
                tracing::error!("[Error - CreatureEvent::configureLuaEvent] Invalid type for creature event: {other}");
                0
            }
        };
        this.set("_eventType", event_type)?;
        this.set("_loaded", true)?;
        Ok(this)
    })?)?;

    methods.set("register", lua.create_function(|lua, this: LuaTable| -> LuaResult<bool> {
        let name: String = this.get("_name").unwrap_or_default();
        let event_type_i: i32 = this.get("_eventType").unwrap_or(0);

        let callback_name = match event_type_i {
            1 => "onLogin",
            2 => "onLogout",
            3 => "onThink",
            4 => "onPrepareDeath",
            5 => "onDeath",
            6 => "onKill",
            7 => "onAdvance",
            8 => "onTextEdit",
            9 => "onHealthChange",
            10 => "onManaChange",
            11 => "onExtendedOpcode",
            _ => {
                tracing::warn!("[Warning - CreatureEvent::register] unknown event type {event_type_i} for '{name}'");
                return Ok(false);
            }
        };

        let callback: Option<LuaFunction> = this.get(callback_name).ok();
        if callback.is_none() {
            let scripted: bool = this.get("_scripted").unwrap_or(false);
            if !scripted {
                return Ok(false);
            }
        }

        let ce_type = match crate::events::creature::CreatureEventType::from_str(callback_name.trim_start_matches("on")) {
            Some(t) => t,
            None => {
                tracing::warn!("[Warning - CreatureEvent::register] invalid type for '{name}'");
                return Ok(false);
            }
        };

        let mut registry = crate::events::registry::g_script_registry().lock()
            .expect("ScriptRegistry mutex poisoned");
        let script_id = if let Some(func) = callback {
            let key = lua.create_registry_value(func)?;
            let id = registry.next_id();
            registry.lua_callbacks.insert(id, key);
            id
        } else {
            0
        };

        let event = crate::events::creature::CreatureEvent {
            name: name.clone(),
            event_type: ce_type,
            script_id,
            scripted: true,
            from_lua: true,
            loaded: true,
        };

        let result = registry.creature_events.register_lua_event(event);
        tracing::debug!("[Lua] CreatureEvent '{name}' registered (type={callback_name})");
        Ok(result)
    })?)?;

    for cb_name in &[
        "onLogin", "onLogout", "onThink", "onPrepareDeath",
        "onDeath", "onKill", "onAdvance", "onTextEdit",
        "onHealthChange", "onManaChange", "onExtendedOpcode",
    ] {
        let field_name = cb_name.to_string();
        methods.set(*cb_name, lua.create_function(move |_, args: LuaMultiValue| -> LuaResult<LuaTable> {
            let mut iter = args.into_iter();
            let this = match iter.next() {
                Some(LuaValue::Table(t)) => t,
                _ => return Err(LuaError::runtime("CreatureEvent:on* - self expected")),
            };
            if let Some(LuaValue::Function(f)) = iter.next() {
                this.raw_set(field_name.as_str(), f)?;
            }
            this.raw_set("_scripted", true)?;
            Ok(this)
        })?)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// MoveEvent class
// ---------------------------------------------------------------------------

fn register_move_event_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, _args: LuaMultiValue| -> LuaResult<LuaTable> {
        let t = lua.create_table()?;
        t.set("_fromLua", true)?;
        t.set("_scripted", false)?;
        t.set("_eventType", 0i32)?;
        t.set("_slot", 0xFFFFFFFFu32)?;
        t.set("_reqLevel", 0u32)?;
        t.set("_reqMagLevel", 0u32)?;
        t.set("_premium", false)?;
        t.set("_wieldInfo", 0u32)?;
        t.set("_vocationString", String::new())?;
        t.set("_tileItem", false)?;
        t.set("_itemIds", lua.create_table()?)?;
        t.set("_actionIds", lua.create_table()?)?;
        t.set("_uniqueIds", lua.create_table()?)?;
        t.set("_positions", lua.create_table()?)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("MoveEvent") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(t)
    })?;
    let methods = register_class(lua, "MoveEvent", None, LUA_DATA_UNKNOWN, Some(ctor))?;

    methods.set("type", lua.create_function(|_, (this, type_name): (LuaTable, String)| -> LuaResult<LuaTable> {
        let event_type: i32 = match type_name.to_ascii_lowercase().as_str() {
            "stepin" => 0,
            "stepout" => 1,
            "equip" => 2,
            "deequip" => 3,
            "additem" => 4,
            "removeitem" => 5,
            other => {
                tracing::error!("Error: [MoveEvent::configureMoveEvent] No valid event name {other}");
                -1
            }
        };
        this.set("_eventType", event_type)?;
        Ok(this)
    })?)?;

    methods.set("register", lua.create_function(|lua, this: LuaTable| -> LuaResult<bool> {
        let event_type_i: i32 = this.get("_eventType").unwrap_or(-1);
        let callback_name = match event_type_i {
            0 => "onStepIn",
            1 => "onStepOut",
            2 => "onEquip",
            3 => "onDeEquip",
            4 | 6 => "onAddItem",
            5 | 7 => "onRemoveItem",
            _ => {
                tracing::warn!("[Warning - MoveEvent::register] unknown event type");
                return Ok(false);
            }
        };

        let callback: Option<LuaFunction> = this.get(callback_name).ok();
        if callback.is_none() {
            let scripted: bool = this.get("_scripted").unwrap_or(false);
            if !scripted {
                return Ok(false);
            }
        }

        let item_ids = lua_table_to_u32_vec(&this, "_itemIds")?;
        let action_ids = lua_table_to_u32_vec(&this, "_actionIds")?;
        let unique_ids = lua_table_to_u32_vec(&this, "_uniqueIds")?;

        let mut pos_list = Vec::new();
        if let Ok(positions) = this.get::<LuaTable>("_positions") {
            for (_, pos_t) in positions.pairs::<i64, LuaTable>().flatten() {
                let x: u16 = pos_t.get("x").unwrap_or(0);
                let y: u16 = pos_t.get("y").unwrap_or(0);
                let z: u8 = pos_t.get("z").unwrap_or(0);
                pos_list.push(Position { x, y, z });
            }
        }

        let me_type = match event_type_i {
            0 => crate::events::movement::MoveEventType::StepIn,
            1 => crate::events::movement::MoveEventType::StepOut,
            2 => crate::events::movement::MoveEventType::Equip,
            3 => crate::events::movement::MoveEventType::DeEquip,
            4 => crate::events::movement::MoveEventType::AddItem,
            5 => crate::events::movement::MoveEventType::RemoveItem,
            6 => crate::events::movement::MoveEventType::AddItemTile,
            7 => crate::events::movement::MoveEventType::RemoveItemTile,
            _ => crate::events::movement::MoveEventType::StepIn,
        };

        let mut registry = crate::events::registry::g_script_registry().lock()
            .expect("ScriptRegistry mutex poisoned");
        let script_id = if let Some(func) = callback {
            let key = lua.create_registry_value(func)?;
            let id = registry.next_id();
            registry.lua_callbacks.insert(id, key);
            id
        } else {
            0
        };

        let event = crate::events::movement::MoveEvent {
            event_type: me_type,
            script_id,
            scripted: true,
            from_lua: true,
            slot: this.get("_slot").unwrap_or(0xFFFFFFFF),
            req_level: this.get("_reqLevel").unwrap_or(0),
            req_mag_level: this.get("_reqMagLevel").unwrap_or(0),
            premium: this.get("_premium").unwrap_or(false),
            vocation_string: this.get("_vocationString").unwrap_or_default(),
            wield_info: this.get("_wieldInfo").unwrap_or(0),
            voc_equip_map: std::collections::BTreeMap::new(),
            tile_item: this.get("_tileItem").unwrap_or(false),
            item_id_range: item_ids,
            action_id_range: action_ids,
            unique_id_range: unique_ids,
            pos_list,
        };

        let result = registry.move_events.register_lua_event(event);
        tracing::debug!("[Lua] MoveEvent registered (type={callback_name})");
        Ok(result)
    })?)?;

    methods.set("level", lua.create_function(|_, (this, lvl): (LuaTable, u32)| -> LuaResult<LuaTable> {
        this.set("_reqLevel", lvl)?;
        let wi: u32 = this.get("_wieldInfo").unwrap_or(0);
        this.set("_wieldInfo", wi | 1)?;
        Ok(this)
    })?)?;

    methods.set("magicLevel", lua.create_function(|_, (this, lvl): (LuaTable, u32)| -> LuaResult<LuaTable> {
        this.set("_reqMagLevel", lvl)?;
        let wi: u32 = this.get("_wieldInfo").unwrap_or(0);
        this.set("_wieldInfo", wi | 2)?;
        Ok(this)
    })?)?;

    methods.set("slot", lua.create_function(|_, (this, slot_name): (LuaTable, String)| -> LuaResult<LuaTable> {
        let event_type: i32 = this.get("_eventType").unwrap_or(-1);
        if event_type == 2 || event_type == 3 {
            let slot: u32 = match slot_name.to_ascii_lowercase().as_str() {
                "head" => 1,
                "necklace" => 2,
                "backpack" => 4,
                "armor" | "body" => 8,
                "right-hand" => 16,
                "left-hand" => 32,
                "hand" | "shield" => 16 | 32,
                "legs" => 64,
                "feet" => 128,
                "ring" => 256,
                "ammo" => 512,
                other => {
                    tracing::warn!("[Warning - MoveEvent::configureMoveEvent] Unknown slot type: {other}");
                    return Ok(this);
                }
            };
            this.set("_slot", slot)?;
        }
        Ok(this)
    })?)?;

    methods.set("id", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaTable> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Err(LuaError::runtime("MoveEvent:id - self expected")),
        };
        let ids: LuaTable = this.get("_itemIds")?;
        let mut len: i64 = ids.raw_len() as i64;
        for val in iter {
            if let LuaValue::Integer(id) = val {
                len += 1;
                ids.raw_set(len, id as u32)?;
            } else if let LuaValue::Number(id) = val {
                len += 1;
                ids.raw_set(len, id as u32)?;
            }
        }
        Ok(this)
    })?)?;

    methods.set("aid", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaTable> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Err(LuaError::runtime("MoveEvent:aid - self expected")),
        };
        let ids: LuaTable = this.get("_actionIds")?;
        let mut len: i64 = ids.raw_len() as i64;
        for val in iter {
            if let LuaValue::Integer(id) = val {
                len += 1;
                ids.raw_set(len, id as u32)?;
            } else if let LuaValue::Number(id) = val {
                len += 1;
                ids.raw_set(len, id as u32)?;
            }
        }
        Ok(this)
    })?)?;

    methods.set("uid", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaTable> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Err(LuaError::runtime("MoveEvent:uid - self expected")),
        };
        let ids: LuaTable = this.get("_uniqueIds")?;
        let mut len: i64 = ids.raw_len() as i64;
        for val in iter {
            if let LuaValue::Integer(id) = val {
                len += 1;
                ids.raw_set(len, id as u32)?;
            } else if let LuaValue::Number(id) = val {
                len += 1;
                ids.raw_set(len, id as u32)?;
            }
        }
        Ok(this)
    })?)?;

    methods.set("position", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaTable> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Err(LuaError::runtime("MoveEvent:position - self expected")),
        };
        let positions: LuaTable = this.get("_positions")?;
        let mut len: i64 = positions.raw_len() as i64;
        for val in iter {
            if let LuaValue::Table(pos_t) = val {
                len += 1;
                positions.raw_set(len, pos_t)?;
            }
        }
        Ok(this)
    })?)?;

    methods.set("premium", lua.create_function(|_, (this, val): (LuaTable, bool)| -> LuaResult<LuaTable> {
        this.set("_premium", val)?;
        let wi: u32 = this.get("_wieldInfo").unwrap_or(0);
        this.set("_wieldInfo", wi | 4)?;
        Ok(this)
    })?)?;

    methods.set("vocation", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaTable> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Err(LuaError::runtime("MoveEvent:vocation - self expected")),
        };
        let voc_name = match iter.next() {
            Some(LuaValue::String(s)) => s.to_string_lossy(),
            _ => String::new(),
        };
        let show_in_desc = match iter.next() {
            Some(LuaValue::Boolean(b)) => b,
            _ => false,
        };
        let last_voc = match iter.next() {
            Some(LuaValue::Boolean(b)) => b,
            _ => false,
        };
        let wi: u32 = this.get("_wieldInfo").unwrap_or(0);
        this.set("_wieldInfo", wi | 8)?;
        if show_in_desc {
            let mut vs: String = this.get("_vocationString").unwrap_or_default();
            if vs.is_empty() {
                vs = format!("{}s", voc_name.to_ascii_lowercase());
            } else if last_voc {
                vs = format!("{vs} and {}s", voc_name.to_ascii_lowercase());
            } else {
                vs = format!("{vs}, {}s", voc_name.to_ascii_lowercase());
            }
            this.set("_vocationString", vs)?;
        }
        Ok(this)
    })?)?;

    methods.set("tileItem", lua.create_function(|_, (this, val): (LuaTable, bool)| -> LuaResult<LuaTable> {
        this.set("_tileItem", val)?;
        Ok(this)
    })?)?;

    for cb_name in &["onEquip", "onDeEquip", "onStepIn", "onStepOut", "onAddItem", "onRemoveItem"] {
        let field_name = cb_name.to_string();
        methods.set(*cb_name, lua.create_function(move |_, args: LuaMultiValue| -> LuaResult<LuaTable> {
            let mut iter = args.into_iter();
            let this = match iter.next() {
                Some(LuaValue::Table(t)) => t,
                _ => return Err(LuaError::runtime("MoveEvent:on* - self expected")),
            };
            if let Some(LuaValue::Function(f)) = iter.next() {
                this.raw_set(field_name.as_str(), f)?;
            }
            this.raw_set("_scripted", true)?;
            Ok(this)
        })?)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// GlobalEvent class
// ---------------------------------------------------------------------------

fn register_global_event_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaTable> {
        let t = lua.create_table()?;
        t.set("_fromLua", true)?;
        t.set("_scripted", false)?;
        t.set("_eventType", 0i32)?;
        t.set("_interval", 0u32)?;
        t.set("_nextExecution", 0i64)?;
        let name = match args.into_iter().nth(1) {
            Some(LuaValue::String(s)) => s.to_string_lossy(),
            _ => String::new(),
        };
        t.set("_name", name)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("GlobalEvent") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(t)
    })?;
    let methods = register_class(lua, "GlobalEvent", None, LUA_DATA_UNKNOWN, Some(ctor))?;

    methods.set("type", lua.create_function(|_, (this, type_name): (LuaTable, String)| -> LuaResult<LuaTable> {
        let event_type: i32 = match type_name.to_ascii_lowercase().as_str() {
            "startup" => 2,
            "shutdown" => 3,
            "record" => 4,
            other => {
                tracing::error!("[Error - GlobalEvent::configureLuaEvent] Invalid type for global event: {other}");
                0
            }
        };
        this.set("_eventType", event_type)?;
        Ok(this)
    })?)?;

    methods.set("register", lua.create_function(|lua, this: LuaTable| -> LuaResult<bool> {
        let name: String = this.get("_name").unwrap_or_default();
        let event_type_i: i32 = this.get("_eventType").unwrap_or(0);
        let interval: u32 = this.get("_interval").unwrap_or(0);

        if event_type_i == 0 && interval == 0 {
            tracing::error!("[Error - GlobalEvent::register] No interval for globalevent with name {name}");
            return Ok(false);
        }

        let callback_name = match event_type_i {
            0 => "onThink",
            1 => "onTime",
            2 => "onStartup",
            3 => "onShutdown",
            4 => "onRecord",
            _ => "onThink",
        };

        let callback: Option<LuaFunction> = this.get(callback_name).ok();
        if callback.is_none() {
            let scripted: bool = this.get("_scripted").unwrap_or(false);
            if !scripted {
                return Ok(false);
            }
        }

        let ge_type = match event_type_i {
            0 => crate::events::global::GlobalEventType::None,
            1 => crate::events::global::GlobalEventType::Timer,
            2 => crate::events::global::GlobalEventType::Startup,
            3 => crate::events::global::GlobalEventType::Shutdown,
            4 => crate::events::global::GlobalEventType::Record,
            _ => crate::events::global::GlobalEventType::None,
        };

        let mut registry = crate::events::registry::g_script_registry().lock()
            .expect("ScriptRegistry mutex poisoned");
        let script_id = if let Some(func) = callback {
            let key = lua.create_registry_value(func)?;
            let id = registry.next_id();
            registry.lua_callbacks.insert(id, key);
            id
        } else {
            0
        };

        let event = crate::events::global::GlobalEvent {
            name: name.clone(),
            event_type: ge_type,
            script_id,
            next_execution: this.get("_nextExecution").unwrap_or(0),
            interval,
            scripted: true,
            from_lua: true,
        };

        let result = registry.global_events.register_lua_event(event);
        tracing::debug!("[Lua] GlobalEvent '{name}' registered (type={callback_name})");
        Ok(result)
    })?)?;

    methods.set("time", lua.create_function(|_, (this, timer): (LuaTable, String)| -> LuaResult<LuaTable> {
        let parts: Vec<i32> = timer.split(':').filter_map(|s| s.trim().parse().ok()).collect();
        let hour = parts.first().copied().unwrap_or(0);
        if !(0..=23).contains(&hour) {
            let name: String = this.get("_name").unwrap_or_default();
            tracing::error!("[Error - GlobalEvent::configureEvent] Invalid hour \"{timer}\" for globalevent with name: {name}");
            return Ok(this);
        }
        this.set("_interval", (hour as u32) << 16)?;
        let min = parts.get(1).copied().unwrap_or(0);
        let sec = parts.get(2).copied().unwrap_or(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let day_seconds = hour as i64 * 3600 + min as i64 * 60 + sec as i64;
        let today_start = now - (now % 86400);
        let mut target = today_start + day_seconds;
        if target <= now {
            target += 86400;
        }
        this.set("_nextExecution", target)?;
        this.set("_eventType", 1i32)?;
        Ok(this)
    })?)?;

    methods.set("interval", lua.create_function(|_, (this, interval): (LuaTable, u32)| -> LuaResult<LuaTable> {
        this.set("_interval", interval)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        this.set("_nextExecution", now + interval as i64)?;
        Ok(this)
    })?)?;

    for cb_name in &["onThink", "onTime", "onStartup", "onShutdown", "onRecord"] {
        let field_name = cb_name.to_string();
        methods.set(*cb_name, lua.create_function(move |_, args: LuaMultiValue| -> LuaResult<LuaTable> {
            let mut iter = args.into_iter();
            let this = match iter.next() {
                Some(LuaValue::Table(t)) => t,
                _ => return Err(LuaError::runtime("GlobalEvent:on* - self expected")),
            };
            if let Some(LuaValue::Function(f)) = iter.next() {
                this.raw_set(field_name.as_str(), f)?;
            }
            this.raw_set("_scripted", true)?;
            Ok(this)
        })?)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Weapon class
// ---------------------------------------------------------------------------

fn register_weapon_class(lua: &Lua) -> LuaResult<()> {
    let ctor = lua.create_function(|lua, args: LuaMultiValue| -> LuaResult<LuaTable> {
        let t = lua.create_table()?;
        t.set("_fromLua", true)?;
        t.set("_scripted", false)?;
        let weapon_type = match args.into_iter().nth(1) {
            Some(LuaValue::Integer(n)) => n as i32,
            Some(LuaValue::Number(n)) => n as i32,
            _ => 0,
        };
        t.set("_weaponType", weapon_type)?;
        t.set("_weaponAction", 0i32)?;
        t.set("_itemId", 0u16)?;
        t.set("_level", 0u32)?;
        t.set("_magicLevel", 0u32)?;
        t.set("_mana", 0u32)?;
        t.set("_manaPercent", 0u32)?;
        t.set("_health", 0i32)?;
        t.set("_healthPercent", 0i32)?;
        t.set("_soul", 0u32)?;
        t.set("_breakChance", 0u32)?;
        t.set("_premium", false)?;
        t.set("_wieldUnproperly", false)?;
        t.set("_attack", 0i32)?;
        t.set("_defense", 0i32)?;
        t.set("_range", 0u32)?;
        t.set("_charges", 0u32)?;
        t.set("_duration", 0u32)?;
        t.set("_decayTo", 0i32)?;
        t.set("_transformEquipTo", 0u16)?;
        t.set("_transformDeEquipTo", 0u16)?;
        t.set("_slotType", 0u32)?;
        t.set("_hitChance", 0i32)?;
        t.set("_ammoType", 0u8)?;
        t.set("_maxHitChance", 0i32)?;
        t.set("_shootType", 0u8)?;
        t.set("_elementType", 0i32)?;
        t.set("_elementDamage", 0i32)?;
        if let Ok(mt) = lua.named_registry_value::<LuaTable>("Weapon") {
            let _ = t.set_metatable(Some(mt));
        }
        Ok(t)
    })?;
    let methods = register_class(lua, "Weapon", None, LUA_DATA_UNKNOWN, Some(ctor))?;

    methods.set("action", lua.create_function(|_, (this, type_name): (LuaTable, String)| -> LuaResult<LuaTable> {
        let action: i32 = match type_name.to_ascii_lowercase().as_str() {
            "removecount" => 0,
            "removecharge" => 1,
            "move" => 2,
            other => {
                tracing::error!("Error: [Weapon::action] No valid action {other}");
                -1
            }
        };
        this.set("_weaponAction", action)?;
        Ok(this)
    })?)?;

    methods.set("register", lua.create_function(|lua, this: LuaTable| -> LuaResult<bool> {
        let callback: Option<LuaFunction> = this.get("onUseWeapon").ok();
        if callback.is_none() {
            let scripted: bool = this.get("_scripted").unwrap_or(false);
            if !scripted {
                return Ok(false);
            }
        }

        let mut registry = crate::events::registry::g_script_registry().lock()
            .expect("ScriptRegistry mutex poisoned");
        let _script_id = if let Some(func) = callback {
            let key = lua.create_registry_value(func)?;
            let id = registry.next_id();
            registry.lua_callbacks.insert(id, key);
            id
        } else {
            0
        };

        tracing::debug!("[Lua] Weapon registered (from Lua)");
        Ok(true)
    })?)?;

    methods.set("onUseWeapon", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaTable> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Err(LuaError::runtime("Weapon:onUseWeapon - self expected")),
        };
        if let Some(LuaValue::Function(f)) = iter.next() {
            this.raw_set("onUseWeapon", f)?;
        }
        this.raw_set("_scripted", true)?;
        Ok(this)
    })?)?;

    macro_rules! weapon_setter_u32 {
        ($methods:ident, $lua:ident, $name:expr, $field:expr) => {
            $methods.set($name, $lua.create_function(|_, (this, val): (LuaTable, u32)| -> LuaResult<LuaTable> {
                this.set($field, val)?;
                Ok(this)
            })?)?;
        };
    }

    macro_rules! weapon_setter_i32 {
        ($methods:ident, $lua:ident, $name:expr, $field:expr) => {
            $methods.set($name, $lua.create_function(|_, (this, val): (LuaTable, i32)| -> LuaResult<LuaTable> {
                this.set($field, val)?;
                Ok(this)
            })?)?;
        };
    }

    macro_rules! weapon_setter_bool {
        ($methods:ident, $lua:ident, $name:expr, $field:expr) => {
            $methods.set($name, $lua.create_function(|_, (this, val): (LuaTable, bool)| -> LuaResult<LuaTable> {
                this.set($field, val)?;
                Ok(this)
            })?)?;
        };
    }

    methods.set("id", lua.create_function(|_, (this, val): (LuaTable, u16)| -> LuaResult<LuaTable> {
        this.set("_itemId", val)?;
        Ok(this)
    })?)?;

    weapon_setter_u32!(methods, lua, "level", "_level");
    weapon_setter_u32!(methods, lua, "magicLevel", "_magicLevel");
    weapon_setter_u32!(methods, lua, "mana", "_mana");
    weapon_setter_u32!(methods, lua, "manaPercent", "_manaPercent");
    weapon_setter_i32!(methods, lua, "health", "_health");
    weapon_setter_i32!(methods, lua, "healthPercent", "_healthPercent");
    weapon_setter_u32!(methods, lua, "soul", "_soul");
    weapon_setter_u32!(methods, lua, "breakChance", "_breakChance");
    weapon_setter_bool!(methods, lua, "premium", "_premium");
    weapon_setter_bool!(methods, lua, "wieldUnproperly", "_wieldUnproperly");
    weapon_setter_i32!(methods, lua, "attack", "_attack");
    weapon_setter_i32!(methods, lua, "defense", "_defense");
    weapon_setter_u32!(methods, lua, "range", "_range");
    weapon_setter_u32!(methods, lua, "charges", "_charges");
    weapon_setter_u32!(methods, lua, "duration", "_duration");
    weapon_setter_i32!(methods, lua, "decayTo", "_decayTo");

    methods.set("transformEquipTo", lua.create_function(|_, (this, val): (LuaTable, u16)| -> LuaResult<LuaTable> {
        this.set("_transformEquipTo", val)?;
        Ok(this)
    })?)?;

    methods.set("transformDeEquipTo", lua.create_function(|_, (this, val): (LuaTable, u16)| -> LuaResult<LuaTable> {
        this.set("_transformDeEquipTo", val)?;
        Ok(this)
    })?)?;

    methods.set("slotType", lua.create_function(|_, (this, slot): (LuaTable, String)| -> LuaResult<LuaTable> {
        let slot_val: u32 = match slot.to_ascii_lowercase().as_str() {
            "two-handed" => 16 | 32,
            _ => 0,
        };
        this.set("_slotType", slot_val)?;
        Ok(this)
    })?)?;

    weapon_setter_i32!(methods, lua, "hitChance", "_hitChance");

    methods.set("extraElement", lua.create_function(|_, (this, damage, element): (LuaTable, i32, i32)| -> LuaResult<LuaTable> {
        this.set("_elementDamage", damage)?;
        this.set("_elementType", element)?;
        Ok(this)
    })?)?;

    methods.set("ammoType", lua.create_function(|_, (this, val): (LuaTable, String)| -> LuaResult<LuaTable> {
        let ammo: u8 = match val.to_ascii_lowercase().as_str() {
            "spear" => 1,
            "bolt" => 2,
            "arrow" => 3,
            "poisonarrow" => 4,
            "burstarrow" => 5,
            "throwingstar" => 6,
            "throwingknife" => 7,
            "smallstone" => 8,
            "largerock" => 9,
            "snowball" => 10,
            "powerbolt" => 11,
            "infernalbolt" => 12,
            _ => 0,
        };
        this.set("_ammoType", ammo)?;
        Ok(this)
    })?)?;

    weapon_setter_i32!(methods, lua, "maxHitChance", "_maxHitChance");

    methods.set("damage", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaTable> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Err(LuaError::runtime("Weapon:damage - self expected")),
        };
        let min_val = match iter.next() {
            Some(LuaValue::Integer(n)) => n as i32,
            Some(LuaValue::Number(n)) => n as i32,
            _ => 0,
        };
        let max_val = match iter.next() {
            Some(LuaValue::Integer(n)) => n as i32,
            Some(LuaValue::Number(n)) => n as i32,
            _ => 0,
        };
        this.set("_damageMin", min_val)?;
        this.set("_damageMax", max_val)?;
        Ok(this)
    })?)?;

    methods.set("element", lua.create_function(|_, (this, val): (LuaTable, i32)| -> LuaResult<LuaTable> {
        this.set("_elementType", val)?;
        Ok(this)
    })?)?;

    methods.set("vocation", lua.create_function(|_, args: LuaMultiValue| -> LuaResult<LuaTable> {
        let mut iter = args.into_iter();
        let this = match iter.next() {
            Some(LuaValue::Table(t)) => t,
            _ => return Err(LuaError::runtime("Weapon:vocation - self expected")),
        };
        Ok(this)
    })?)?;

    methods.set("shootType", lua.create_function(|_, (this, val): (LuaTable, u8)| -> LuaResult<LuaTable> {
        this.set("_shootType", val)?;
        Ok(this)
    })?)?;

    Ok(())
}
