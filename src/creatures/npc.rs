use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::creatures::{CreatureBase, CreatureId, Outfit};
use crate::map::Position;

static G_NPCS: OnceLock<Npcs> = OnceLock::new();

pub fn g_npcs() -> &'static Npcs {
    G_NPCS.get().expect("npcs not initialized")
}

pub fn init_npcs(n: Npcs) {
    G_NPCS.set(n).unwrap_or_else(|_| panic!("npcs already initialized"));
}

/// Stores per-NPC-instance Lua event IDs keyed by creature id.  Mirrors C++
/// `NpcEventsHandler`, one per spawned NPC, loaded at spawn time.
static G_NPC_INSTANCE_EVENTS: OnceLock<Mutex<HashMap<CreatureId, NpcEventIds>>> = OnceLock::new();

#[derive(Debug, Clone, Default)]
pub struct NpcEventIds {
    pub creature_say: i32,
    pub think: i32,
    pub creature_appear: i32,
    pub creature_disappear: i32,
    pub player_close_channel: i32,
    pub player_end_trade: i32,
    pub creature_move: i32,
}

impl NpcEventIds {
    pub fn new(say: i32, think: i32, appear: i32, disappear: i32, close_ch: i32, end_trade: i32, creature_move: i32) -> Self {
        Self {
            creature_say: say,
            think,
            creature_appear: appear,
            creature_disappear: disappear,
            player_close_channel: close_ch,
            player_end_trade: end_trade,
            creature_move,
        }
    }
}

fn instance_events_map() -> &'static Mutex<HashMap<CreatureId, NpcEventIds>> {
    G_NPC_INSTANCE_EVENTS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn get_npc_instance_events(npc_id: CreatureId) -> Option<NpcEventIds> {
    instance_events_map().lock().ok()?.get(&npc_id).cloned()
}

/// Load the per-instance script for a freshly spawned NPC and store its event
/// IDs.  Looks up the NPC's `script` file by its type name.  Call after the NPC
/// is placed in the game (so `get_current_npc` resolves to a live creature).
pub fn register_npc_instance(npc_id: CreatureId, type_name: &str) {
    let script_file = match g_npcs().get_npc_type(type_name) {
        Some(nt) if !nt.script_file.is_empty() => nt.script_file.clone(),
        _ => return,
    };
    let (say, think, appear, disappear, close_ch, end_trade, creature_move) =
        crate::lua::script::load_npc_instance_script(npc_id, &script_file);
    let ids = NpcEventIds::new(say, think, appear, disappear, close_ch, end_trade, creature_move);
    instance_events_map()
        .lock()
        .expect("G_NPC_INSTANCE_EVENTS mutex poisoned")
        .insert(npc_id, ids);
}

pub fn remove_npc_instance(npc_id: CreatureId) {
    if let Ok(mut map) = instance_events_map().lock() {
        map.remove(&npc_id);
    }
}

static NPC_AUTO_ID: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0x80000000);

#[derive(Debug, Clone, Default)]
pub struct NpcType {
    pub name: String,
    pub health: i32,
    pub health_max: i32,
    pub base_speed: u32,
    pub outfit: Outfit,
    pub walk_interval: u32,
    pub speech_bubble: u8,
    pub script_file: String,
    pub parameters: HashMap<String, String>,
    pub creature_say_event: i32,
    pub think_event: i32,
    pub creature_appear_event: i32,
    pub creature_disappear_event: i32,
    pub player_close_channel_event: i32,
    pub player_end_trade_event: i32,
    pub creature_move_event: i32,
}

impl NpcType {
    pub fn has_script(&self) -> bool {
        !self.script_file.is_empty()
    }

    pub fn has_any_event(&self) -> bool {
        self.creature_say_event != -1
            || self.think_event != -1
            || self.creature_appear_event != -1
            || self.creature_disappear_event != -1
            || self.player_close_channel_event != -1
            || self.player_end_trade_event != -1
            || self.creature_move_event != -1
    }
}

#[derive(Default)]
pub struct Npcs {
    pub npc_types: HashMap<String, NpcType>,
}

impl Npcs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_npc_type(&self, name: &str) -> Option<&NpcType> {
        self.npc_types.get(&name.to_lowercase())
    }

    pub fn get_npc_type_mut(&mut self, name: &str) -> Option<&mut NpcType> {
        self.npc_types.get_mut(&name.to_lowercase())
    }

    pub fn load_from_dir(&mut self, dir: &std::path::Path) -> Result<(), anyhow::Error> {
        let entries = std::fs::read_dir(dir)
            .map_err(|e| anyhow::anyhow!("Cannot read NPC dir {:?}: {}", dir, e))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("xml") {
                continue;
            }
            if let Err(e) = self.load_npc_file(&path) {
                tracing::warn!("NPC load error {:?}: {}", path, e);
            }
        }
        Ok(())
    }

    fn load_npc_file(&mut self, path: &std::path::Path) -> Result<(), anyhow::Error> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("read error: {}", e))?;
        let doc = roxmltree::Document::parse(&content)
            .map_err(|e| anyhow::anyhow!("parse error: {}", e))?;

        let npc = doc.root_element();
        if !npc.has_tag_name("npc") {
            return Ok(());
        }

        let name = npc.attribute("name").unwrap_or("").to_owned();
        if name.is_empty() {
            return Ok(());
        }

        let walk_interval = npc.attribute("walkinterval")
            .and_then(|v| v.parse().ok())
            .unwrap_or(2000u32);

        let speech_bubble = npc.attribute("speechbubble")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0u8);

        let script_file = npc.attribute("script").unwrap_or("").to_owned();

        let mut health_now = 100i32;
        let mut health_max = 100i32;
        let mut outfit = Outfit::default();
        let mut parameters = HashMap::new();

        for child in npc.children().filter(|n| n.is_element()) {
            match child.tag_name().name() {
                "health" => {
                    health_now = child.attribute("now").and_then(|v| v.parse().ok()).unwrap_or(100);
                    health_max = child.attribute("max").and_then(|v| v.parse().ok()).unwrap_or(100);
                }
                "look" => {
                    outfit.look_type = child.attribute("type").and_then(|v| v.parse().ok()).unwrap_or(0);
                    outfit.look_head = child.attribute("head").and_then(|v| v.parse().ok()).unwrap_or(0);
                    outfit.look_body = child.attribute("body").and_then(|v| v.parse().ok()).unwrap_or(0);
                    outfit.look_legs = child.attribute("legs").and_then(|v| v.parse().ok()).unwrap_or(0);
                    outfit.look_feet = child.attribute("feet").and_then(|v| v.parse().ok()).unwrap_or(0);
                    outfit.look_addons = child.attribute("addons").and_then(|v| v.parse().ok()).unwrap_or(0);
                }
                "parameters" => {
                    for param in child.children().filter(|n| n.is_element() && n.has_tag_name("parameter")) {
                        let key = param.attribute("key").unwrap_or("").to_owned();
                        let value = param.attribute("value").unwrap_or("").to_owned();
                        if !key.is_empty() {
                            parameters.insert(key, value);
                        }
                    }
                }
                _ => {}
            }
        }

        let nt = NpcType {
            name: name.clone(),
            health: health_now,
            health_max,
            base_speed: 100,
            outfit,
            walk_interval,
            speech_bubble,
            script_file,
            parameters,
            creature_say_event: -1,
            think_event: -1,
            creature_appear_event: -1,
            creature_disappear_event: -1,
            player_close_channel_event: -1,
            player_end_trade_event: -1,
            creature_move_event: -1,
        };

        self.npc_types.insert(name.to_lowercase(), nt);
        Ok(())
    }
}

pub struct Npc {
    pub base: CreatureBase,
    pub name: String,
    pub walk_interval: u32,
    pub walk_timer: u32,
    pub type_name: String,
    pub speech_bubble: u8,
    pub focus_creature: CreatureId,
}

impl Npc {
    pub fn new(base: CreatureBase, name: String, walk_interval: u32, type_name: String) -> Self {
        Self { base, name, walk_interval, walk_timer: 0, type_name, speech_bubble: 0, focus_creature: 0 }
    }

    pub fn get_name(&self) -> &str {
        &self.name
    }

    pub fn allocate_id() -> CreatureId {
        NPC_AUTO_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    pub fn create_npc(name: &str) -> Option<Box<Npc>> {
        let nt = g_npcs().get_npc_type(name)?;
        let id = Npc::allocate_id();
        let mut base = CreatureBase::new(id, Position::default());
        base.health = nt.health;
        base.health_max = nt.health_max;
        base.base_speed = nt.base_speed;
        base.current_outfit = nt.outfit;
        base.default_outfit = nt.outfit;
        let mut npc = Npc::new(base, nt.name.clone(), nt.walk_interval, name.to_lowercase());
        npc.speech_bubble = nt.speech_bubble;
        Some(Box::new(npc))
    }
}

fn npc_push_creature_table(lua: &mlua::Lua, cid: CreatureId, class_name: &str) -> mlua::Result<mlua::Table> {
    let t = lua.create_table()?;
    t.raw_set(1, cid)?;
    if let Ok(mt) = lua.named_registry_value::<mlua::Table>(class_name) {
        let _ = t.set_metatable(Some(mt));
    }
    Ok(t)
}

fn call_npc_event_inner(npc_id: CreatureId, event_id: i32, args_builder: impl FnOnce(&mlua::Lua, &crate::lua::script::LuaScriptInterface) -> bool) -> bool {
    use crate::lua::script::{set_current_npc, ScriptEnvironment};

    let iface_lock = match crate::lua::script::g_npc_iface_opt() {
        Some(l) => l,
        None => return false,
    };

    if !ScriptEnvironment::reserve() {
        tracing::error!("[NPC] Lua: call stack overflow");
        return false;
    }
    ScriptEnvironment::set_script_id(event_id, "Npc interface");
    set_current_npc(npc_id);

    let result = {
        let iface = iface_lock.lock().expect("NPC iface lock poisoned");
        args_builder(iface.lua(), &iface)
    };

    set_current_npc(0);
    ScriptEnvironment::reset();
    result
}

/// Fire `onCreatureSay(creature, type, text)` for the given NPC.
pub fn fire_npc_creature_say(npc_id: CreatureId, player_id: CreatureId, speak_type: u8, text: &str) -> bool {
    let events = match get_npc_instance_events(npc_id) {
        Some(e) => e,
        None => return false,
    };
    if events.creature_say == -1 {
        return false;
    }
    if crate::lua::script::g_npc_iface_opt().is_none() {
        return false;
    }
    let text_owned = text.to_owned();
    call_npc_event_inner(npc_id, events.creature_say, move |lua, iface| {
        let Ok(func) = iface.push_function(events.creature_say) else { return false; };
        let Ok(player_tbl) = npc_push_creature_table(lua, player_id, "Player") else { return false; };
        if let Err(e) = func.call::<()>((player_tbl, speak_type as i32, text_owned)) {
            tracing::warn!("[NPC] onCreatureSay error: {}", e);
            return false;
        }
        true
    })
}

/// Fire `onCreatureAppear(creature)` for the given NPC.
pub fn fire_npc_creature_appear(npc_id: CreatureId, who_id: CreatureId, who_class: &str) {
    let events = match get_npc_instance_events(npc_id) {
        Some(e) => e,
        None => return,
    };
    if events.creature_appear == -1 {
        return;
    }
    if crate::lua::script::g_npc_iface_opt().is_none() {
        return;
    }
    let class = who_class.to_owned();
    call_npc_event_inner(npc_id, events.creature_appear, move |lua, iface| {
        let Ok(func) = iface.push_function(events.creature_appear) else { return false; };
        let Ok(tbl) = npc_push_creature_table(lua, who_id, &class) else { return false; };
        if let Err(e) = func.call::<()>(tbl) {
            tracing::warn!("[NPC] onCreatureAppear error: {}", e);
        }
        true
    });
}

/// Invoke a stored NPC shop buy/sell callback, mirroring
/// `NpcEventsHandler::onPlayerTrade`.  The callback is the Lua closure passed to
/// `openShopWindow`; it is called as
/// `cb(player, itemId, subType, amount, ignore, inBackpacks)` with the NPC set
/// as the active script NPC.
#[allow(clippy::too_many_arguments)]
pub fn fire_npc_player_trade(npc_id: CreatureId, callback_id: i32, player_id: CreatureId, item_id: u16, sub_type: u8, amount: u8, ignore: bool, in_backpacks: bool) {
    if callback_id == -1 {
        return;
    }
    if crate::lua::script::g_npc_iface_opt().is_none() {
        return;
    }
    call_npc_event_inner(npc_id, callback_id, move |lua, _iface| {
        let func = {
            let registry = crate::events::registry::g_script_registry().lock().unwrap();
            registry.get_callback_function(lua, callback_id)
        };
        let Some(func) = func else { return false; };
        let Ok(player_tbl) = npc_push_creature_table(lua, player_id, "Player") else { return false; };
        if let Err(e) = func.call::<()>((player_tbl, item_id, sub_type, amount, ignore, in_backpacks)) {
            tracing::warn!("[NPC] onPlayerTrade callback error: {}", e);
            return false;
        }
        true
    });
}

/// Fire `onThink()` for the given NPC.
pub fn fire_npc_think(npc_id: CreatureId) {
    let events = match get_npc_instance_events(npc_id) {
        Some(e) => e,
        None => return,
    };
    if events.think == -1 {
        return;
    }
    if crate::lua::script::g_npc_iface_opt().is_none() {
        return;
    }
    call_npc_event_inner(npc_id, events.think, |_lua, iface| {
        if let Ok(f) = iface.push_function(events.think) {
            let _ = f.call::<()>(());
        }
        true
    });
}
