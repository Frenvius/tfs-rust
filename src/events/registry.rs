use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use mlua::prelude::*;

use crate::events::actions::Actions;
use crate::events::talk::TalkActions;
use crate::events::movement::MoveEvents;
use crate::events::creature::CreatureEvents;
use crate::events::global::GlobalEvents;

static G_SCRIPT_REGISTRY: OnceLock<Mutex<ScriptRegistry>> = OnceLock::new();

pub fn g_script_registry() -> &'static Mutex<ScriptRegistry> {
    G_SCRIPT_REGISTRY.get().expect("ScriptRegistry not initialized")
}

pub fn init_script_registry() {
    let registry = ScriptRegistry {
        actions: Actions::new(),
        talk_actions: TalkActions::new(),
        move_events: MoveEvents::new(),
        creature_events: CreatureEvents::new(),
        global_events: GlobalEvents::new(),
        lua_callbacks: HashMap::new(),
        next_script_id: 10000,
        spells: HashMap::new(),
        weapons: HashMap::new(),
    };
    G_SCRIPT_REGISTRY
        .set(Mutex::new(registry))
        .unwrap_or_else(|_| panic!("ScriptRegistry already initialized"));
}

#[derive(Clone)]
pub struct SpellEntry {
    pub name: String,
    pub words: String,
    pub script_id: i32,
    pub spell_type: i32,
    pub spell_id: u8,
    pub level: u32,
    pub magic_level: u32,
    pub mana: u32,
    pub mana_percent: u32,
    pub soul: u32,
    pub group: u32,
    pub secondary_group: u32,
    pub cooldown: u32,
    pub group_cooldown: u32,
    pub secondary_group_cooldown: u32,
    pub need_target: bool,
    pub need_weapon: bool,
    pub need_learn: bool,
    pub self_target: bool,
    pub aggressive: bool,
    pub pz_lock: bool,
    pub has_params: bool,
    pub has_player_name_param: bool,
    pub premium: bool,
    pub learnable: bool,
    pub enabled: bool,
    pub vocations: Vec<u32>,
}

pub struct ScriptRegistry {
    pub actions: Actions,
    pub talk_actions: TalkActions,
    pub move_events: MoveEvents,
    pub creature_events: CreatureEvents,
    pub global_events: GlobalEvents,
    pub lua_callbacks: HashMap<i32, LuaRegistryKey>,
    pub next_script_id: i32,
    /// Keyed by lowercase words (e.g. "exura").
    pub spells: HashMap<String, SpellEntry>,
    /// Weapon callbacks keyed by item server_id → script_id.
    pub weapons: HashMap<u16, i32>,
}

impl ScriptRegistry {
    pub fn next_id(&mut self) -> i32 {
        let id = self.next_script_id;
        self.next_script_id += 1;
        id
    }

    pub fn get_callback_function(&self, lua: &Lua, script_id: i32) -> Option<LuaFunction> {
        let key = self.lua_callbacks.get(&script_id)?;
        lua.registry_value::<LuaFunction>(key).ok()
    }
}
