pub mod actions;
pub mod creature;
pub mod dispatch;
pub mod global;
pub mod movement;
pub mod registry;
pub mod spells;
pub mod talk;

use std::sync::{Mutex, OnceLock};

pub use registry::{g_script_registry, init_script_registry, ScriptRegistry};

pub use actions::Actions;
pub use creature::CreatureEvents;
pub use global::GlobalEvents;
pub use movement::MoveEvents;
pub use talk::TalkActions;

static G_EVENTS: OnceLock<Mutex<Events>> = OnceLock::new();

pub fn g_events() -> &'static Mutex<Events> {
    G_EVENTS.get_or_init(|| Mutex::new(Events::new()))
}

/// Sentinel value indicating an event has no associated script. Mirrors
/// the C++ pattern of initialising all EventsInfo fields to -1.
pub const NONE_EVENT_ID: i32 = -1;

pub enum LookThingType {
    Creature(crate::creatures::CreatureId, &'static str),
    Item(u16, u32),
}

/// A strongly-typed wrapper around the integer script-ID returned by
/// `LuaScriptInterface::get_event` / `get_meta_event`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScriptId(pub i32);

impl ScriptId {
    pub fn is_none(self) -> bool {
        self.0 == NONE_EVENT_ID
    }
}

// ---------------------------------------------------------------------------
// Events (events.h / events.cpp)
// ---------------------------------------------------------------------------

/// Mirrors `Events::EventsInfo` from `events.h`. All fields default to -1
/// (no script registered).
#[derive(Debug, Clone)]
pub struct EventsInfo {
    // Creature
    pub creature_on_change_outfit: i32,
    pub creature_on_area_combat: i32,
    pub creature_on_target_combat: i32,
    pub creature_on_hear: i32,

    // Party
    pub party_on_join: i32,
    pub party_on_leave: i32,
    pub party_on_disband: i32,
    pub party_on_share_experience: i32,

    // Player
    pub player_on_look: i32,
    pub player_on_look_in_battle_list: i32,
    pub player_on_look_in_trade: i32,
    pub player_on_look_in_shop: i32,
    pub player_on_move_item: i32,
    pub player_on_item_moved: i32,
    pub player_on_move_creature: i32,
    pub player_on_report_rule_violation: i32,
    pub player_on_report_bug: i32,
    pub player_on_turn: i32,
    pub player_on_trade_request: i32,
    pub player_on_trade_accept: i32,
    pub player_on_trade_completed: i32,
    pub player_on_gain_experience: i32,
    pub player_on_lose_experience: i32,
    pub player_on_gain_skill_tries: i32,

    // Monster
    pub monster_on_drop_loot: i32,
    pub monster_on_spawn: i32,
}

impl Default for EventsInfo {
    fn default() -> Self {
        Self {
            creature_on_change_outfit: NONE_EVENT_ID,
            creature_on_area_combat: NONE_EVENT_ID,
            creature_on_target_combat: NONE_EVENT_ID,
            creature_on_hear: NONE_EVENT_ID,
            party_on_join: NONE_EVENT_ID,
            party_on_leave: NONE_EVENT_ID,
            party_on_disband: NONE_EVENT_ID,
            party_on_share_experience: NONE_EVENT_ID,
            player_on_look: NONE_EVENT_ID,
            player_on_look_in_battle_list: NONE_EVENT_ID,
            player_on_look_in_trade: NONE_EVENT_ID,
            player_on_look_in_shop: NONE_EVENT_ID,
            player_on_move_item: NONE_EVENT_ID,
            player_on_item_moved: NONE_EVENT_ID,
            player_on_move_creature: NONE_EVENT_ID,
            player_on_report_rule_violation: NONE_EVENT_ID,
            player_on_report_bug: NONE_EVENT_ID,
            player_on_turn: NONE_EVENT_ID,
            player_on_trade_request: NONE_EVENT_ID,
            player_on_trade_accept: NONE_EVENT_ID,
            player_on_trade_completed: NONE_EVENT_ID,
            player_on_gain_experience: NONE_EVENT_ID,
            player_on_lose_experience: NONE_EVENT_ID,
            player_on_gain_skill_tries: NONE_EVENT_ID,
            monster_on_drop_loot: NONE_EVENT_ID,
            monster_on_spawn: NONE_EVENT_ID,
        }
    }
}

/// XML schema for a single entry in `data/events/events.xml`.
#[derive(Debug, serde::Deserialize)]
struct EventEntry {
    #[serde(rename = "@class")]
    pub class: String,
    #[serde(rename = "@method")]
    pub method: String,
    #[serde(rename = "@enabled")]
    pub enabled: Option<u8>,
}

/// XML schema for the top-level `data/events/events.xml` document.
#[derive(Debug, serde::Deserialize)]
struct EventsXml {
    #[serde(rename = "event", default)]
    pub events: Vec<EventEntry>,
}

/// Mirrors the `Events` class from `events.h / events.cpp`.
///
/// Loads `data/events/events.xml`, maps each enabled entry to a Lua
/// function via `get_meta_event`, and stores the resulting script IDs in
/// `EventsInfo`.
pub struct Events {
    script_interface: crate::lua::script::LuaScriptInterface,
    info: EventsInfo,
}

impl Events {
    /// Create a new Events system. Mirrors `Events::Events()`.
    pub fn new() -> Self {
        let mut iface = crate::lua::script::LuaScriptInterface::new("Event Interface");
        if let Err(e) = iface.init_state() {
            tracing::error!("Events::new - init_state failed: {e}");
        }
        Self {
            script_interface: iface,
            info: EventsInfo::default(),
        }
    }

    /// Load `data/events/events.xml`. Returns `true` on success.
    /// Mirrors `Events::load()`.
    pub fn load(&mut self) -> bool {
        let path = "data/events/events.xml";
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Events::load - cannot open {path}: {e}");
                return false;
            }
        };

        let file: EventsXml = match quick_xml::de::from_str(&source) {
            Ok(f) => f,
            Err(e) => {
                tracing::error!("Events::load - parse error in {path}: {e}");
                return false;
            }
        };

        self.info = EventsInfo::default();

        let mut loaded_classes: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for entry in file.events {
            if entry.enabled.unwrap_or(1) == 0 {
                continue;
            }

            let class = &entry.class;
            if loaded_classes.insert(class.clone()) {
                let script_path =
                    format!("data/events/scripts/{}.lua", class.to_lowercase());
                if let Err(e) = self.script_interface.load_file(&script_path) {
                    tracing::warn!(
                        "Events::load - cannot load script: {}.lua: {e}",
                        class.to_lowercase()
                    );
                }
            }

            let script_id = self
                .script_interface
                .get_meta_event(class, &entry.method);

            match class.as_str() {
                "Creature" => match entry.method.as_str() {
                    "onChangeOutfit" => self.info.creature_on_change_outfit = script_id,
                    "onAreaCombat" => self.info.creature_on_area_combat = script_id,
                    "onTargetCombat" => self.info.creature_on_target_combat = script_id,
                    "onHear" => self.info.creature_on_hear = script_id,
                    other => tracing::warn!("Events::load - unknown Creature method: {other}"),
                },
                "Party" => match entry.method.as_str() {
                    "onJoin" => self.info.party_on_join = script_id,
                    "onLeave" => self.info.party_on_leave = script_id,
                    "onDisband" => self.info.party_on_disband = script_id,
                    "onShareExperience" => self.info.party_on_share_experience = script_id,
                    other => tracing::warn!("Events::load - unknown Party method: {other}"),
                },
                "Player" => match entry.method.as_str() {
                    "onLook" => self.info.player_on_look = script_id,
                    "onLookInBattleList" => {
                        self.info.player_on_look_in_battle_list = script_id
                    }
                    "onLookInTrade" => self.info.player_on_look_in_trade = script_id,
                    "onLookInShop" => self.info.player_on_look_in_shop = script_id,
                    "onMoveItem" => self.info.player_on_move_item = script_id,
                    "onItemMoved" => self.info.player_on_item_moved = script_id,
                    "onMoveCreature" => self.info.player_on_move_creature = script_id,
                    "onReportRuleViolation" => {
                        self.info.player_on_report_rule_violation = script_id
                    }
                    "onReportBug" => self.info.player_on_report_bug = script_id,
                    "onTurn" => self.info.player_on_turn = script_id,
                    "onTradeRequest" => self.info.player_on_trade_request = script_id,
                    "onTradeAccept" => self.info.player_on_trade_accept = script_id,
                    "onTradeCompleted" => self.info.player_on_trade_completed = script_id,
                    "onGainExperience" => self.info.player_on_gain_experience = script_id,
                    "onLoseExperience" => self.info.player_on_lose_experience = script_id,
                    "onGainSkillTries" => self.info.player_on_gain_skill_tries = script_id,
                    other => tracing::warn!("Events::load - unknown Player method: {other}"),
                },
                "Monster" => match entry.method.as_str() {
                    "onDropLoot" => self.info.monster_on_drop_loot = script_id,
                    "onSpawn" => self.info.monster_on_spawn = script_id,
                    other => tracing::warn!("Events::load - unknown Monster method: {other}"),
                },
                other => tracing::warn!("Events::load - unknown class: {other}"),
            }
        }

        true
    }

    // -----------------------------------------------------------------------
    // Creature events
    // -----------------------------------------------------------------------

    pub fn event_creature_on_change_outfit(
        &self,
        creature_id: crate::creatures::CreatureId,
        creature_class: &str,
        outfit: &crate::creatures::Outfit,
    ) -> bool {
        if self.info.creature_on_change_outfit == NONE_EVENT_ID {
            return true;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<bool> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.creature_on_change_outfit)?;

            let creature_tbl = lua.create_table()?;
            creature_tbl.raw_set(1, creature_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>(creature_class) {
                let _ = creature_tbl.set_metatable(Some(mt));
            }

            let outfit_tbl = lua.create_table()?;
            outfit_tbl.set("lookType", outfit.look_type as i64)?;
            outfit_tbl.set("lookTypeEx", outfit.look_type_ex as i64)?;
            outfit_tbl.set("lookHead", outfit.look_head as i64)?;
            outfit_tbl.set("lookBody", outfit.look_body as i64)?;
            outfit_tbl.set("lookLegs", outfit.look_legs as i64)?;
            outfit_tbl.set("lookFeet", outfit.look_feet as i64)?;
            outfit_tbl.set("lookAddons", outfit.look_addons as i64)?;

            match func.call::<bool>((creature_tbl, outfit_tbl)) {
                Ok(v) => Ok(v),
                Err(e) => {
                    tracing::error!("eventCreatureOnChangeOutfit error: {e}");
                    Ok(true)
                }
            }
        })();
        result.unwrap_or(true)
    }

    pub fn event_creature_on_area_combat(
        &self,
        creature_id: Option<crate::creatures::CreatureId>,
        creature_class: &str,
        tile_pos: crate::map::Position,
        aggressive: bool,
    ) -> i32 {
        if self.info.creature_on_area_combat == NONE_EVENT_ID {
            return 0;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<i32> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.creature_on_area_combat)?;

            let creature_val: mlua::Value = match creature_id {
                Some(cid) => {
                    let t = lua.create_table()?;
                    t.raw_set(1, cid)?;
                    if let Ok(mt) = lua.named_registry_value::<mlua::Table>(creature_class) {
                        let _ = t.set_metatable(Some(mt));
                    }
                    mlua::Value::Table(t)
                }
                None => mlua::Value::Nil,
            };

            let tile_tbl = lua.create_table()?;
            tile_tbl.set("x", tile_pos.x as i64)?;
            tile_tbl.set("y", tile_pos.y as i64)?;
            tile_tbl.set("z", tile_pos.z as i64)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Tile") {
                let _ = tile_tbl.set_metatable(Some(mt));
            }

            match func.call::<f64>((creature_val, tile_tbl, aggressive)) {
                Ok(v) => Ok(v as i32),
                Err(e) => {
                    tracing::error!("eventCreatureOnAreaCombat error: {e}");
                    Ok(1) // RETURNVALUE_NOTPOSSIBLE
                }
            }
        })();
        result.unwrap_or(1)
    }

    pub fn event_creature_on_target_combat(
        &self,
        creature_id: Option<crate::creatures::CreatureId>,
        creature_class: &str,
        target_id: crate::creatures::CreatureId,
        target_class: &str,
    ) -> i32 {
        if self.info.creature_on_target_combat == NONE_EVENT_ID {
            return 0;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<i32> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.creature_on_target_combat)?;

            let creature_val: mlua::Value = match creature_id {
                Some(cid) => {
                    let t = lua.create_table()?;
                    t.raw_set(1, cid)?;
                    if let Ok(mt) = lua.named_registry_value::<mlua::Table>(creature_class) {
                        let _ = t.set_metatable(Some(mt));
                    }
                    mlua::Value::Table(t)
                }
                None => mlua::Value::Nil,
            };

            let target_tbl = lua.create_table()?;
            target_tbl.raw_set(1, target_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>(target_class) {
                let _ = target_tbl.set_metatable(Some(mt));
            }

            match func.call::<f64>((creature_val, target_tbl)) {
                Ok(v) => Ok(v as i32),
                Err(e) => {
                    tracing::error!("eventCreatureOnTargetCombat error: {e}");
                    Ok(1) // RETURNVALUE_NOTPOSSIBLE
                }
            }
        })();
        result.unwrap_or(1)
    }

    pub fn event_creature_on_hear(
        &self,
        creature_id: crate::creatures::CreatureId,
        creature_class: &str,
        speaker_id: crate::creatures::CreatureId,
        speaker_class: &str,
        words: &str,
        speak_type: u8,
    ) {
        if self.info.creature_on_hear == NONE_EVENT_ID {
            return;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<()> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.creature_on_hear)?;

            let creature_tbl = lua.create_table()?;
            creature_tbl.raw_set(1, creature_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>(creature_class) {
                let _ = creature_tbl.set_metatable(Some(mt));
            }

            let speaker_tbl = lua.create_table()?;
            speaker_tbl.raw_set(1, speaker_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>(speaker_class) {
                let _ = speaker_tbl.set_metatable(Some(mt));
            }

            if let Err(e) = func.call::<()>((creature_tbl, speaker_tbl, words.to_owned(), speak_type as f64)) {
                tracing::error!("eventCreatureOnHear error: {e}");
            }
            Ok(())
        })();
        if let Err(e) = result {
            tracing::error!("eventCreatureOnHear setup error: {e}");
        }
    }

    // -----------------------------------------------------------------------
    // Party events
    // -----------------------------------------------------------------------

    pub fn event_party_on_join(
        &self,
        leader_id: crate::creatures::CreatureId,
        player_id: crate::creatures::CreatureId,
    ) -> bool {
        if self.info.party_on_join == NONE_EVENT_ID {
            return true;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<bool> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.party_on_join)?;

            let party_tbl = lua.create_table()?;
            party_tbl.raw_set(1, leader_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Party") {
                let _ = party_tbl.set_metatable(Some(mt));
            }

            let player_tbl = lua.create_table()?;
            player_tbl.raw_set(1, player_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = player_tbl.set_metatable(Some(mt));
            }

            match func.call::<bool>((party_tbl, player_tbl)) {
                Ok(v) => Ok(v),
                Err(e) => {
                    tracing::error!("eventPartyOnJoin error: {e}");
                    Ok(false)
                }
            }
        })();
        result.unwrap_or(false)
    }

    pub fn event_party_on_leave(
        &self,
        leader_id: crate::creatures::CreatureId,
        player_id: crate::creatures::CreatureId,
    ) -> bool {
        if self.info.party_on_leave == NONE_EVENT_ID {
            return true;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<bool> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.party_on_leave)?;

            let party_tbl = lua.create_table()?;
            party_tbl.raw_set(1, leader_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Party") {
                let _ = party_tbl.set_metatable(Some(mt));
            }

            let player_tbl = lua.create_table()?;
            player_tbl.raw_set(1, player_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = player_tbl.set_metatable(Some(mt));
            }

            match func.call::<bool>((party_tbl, player_tbl)) {
                Ok(v) => Ok(v),
                Err(e) => {
                    tracing::error!("eventPartyOnLeave error: {e}");
                    Ok(false)
                }
            }
        })();
        result.unwrap_or(false)
    }

    pub fn event_party_on_disband(
        &self,
        leader_id: crate::creatures::CreatureId,
    ) -> bool {
        if self.info.party_on_disband == NONE_EVENT_ID {
            return true;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<bool> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.party_on_disband)?;

            let party_tbl = lua.create_table()?;
            party_tbl.raw_set(1, leader_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Party") {
                let _ = party_tbl.set_metatable(Some(mt));
            }

            match func.call::<bool>(party_tbl) {
                Ok(v) => Ok(v),
                Err(e) => {
                    tracing::error!("eventPartyOnDisband error: {e}");
                    Ok(false)
                }
            }
        })();
        result.unwrap_or(false)
    }

    /// Port of `Events::eventPartyOnShareExperience`. Returns the (possibly
    /// modified) shared experience for the party identified by `leader_id`.
    /// With no Lua handler the experience is returned unchanged (matching C++).
    pub fn event_party_on_share_experience(&self, leader_id: crate::creatures::CreatureId, exp: u64) -> u64 {
        if self.info.party_on_share_experience == NONE_EVENT_ID {
            return exp;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<u64> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.party_on_share_experience)?;

            let party_tbl = lua.create_table()?;
            party_tbl.raw_set(1, leader_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Party") {
                let _ = party_tbl.set_metatable(Some(mt));
            }

            match func.call::<f64>((party_tbl, exp as f64)) {
                Ok(v) => Ok(v.max(0.0) as u64),
                Err(e) => {
                    tracing::error!("eventPartyOnShareExperience error: {e}");
                    Ok(exp)
                }
            }
        })();
        result.unwrap_or(exp)
    }

    // -----------------------------------------------------------------------
    // Player events
    // -----------------------------------------------------------------------

    pub fn event_player_on_look(
        &self,
        player_id: crate::creatures::CreatureId,
        thing_type: LookThingType,
        pos: crate::map::Position,
        stackpos: u8,
        look_distance: i32,
    ) {
        if self.info.player_on_look == NONE_EVENT_ID {
            // No Lua handler — send a minimal description so the player sees something.
            let desc = match &thing_type {
                LookThingType::Creature(cid, _) => {
                    let game = crate::game::g_game().lock().unwrap();
                    game.get_creature(*cid)
                        .map(|c| format!("You see {}.", c.get_name_description()))
                        .unwrap_or_else(|| "You see a creature.".to_owned())
                }
                LookThingType::Item(server_id, _) => {
                    let game = crate::game::g_game().lock().unwrap();
                    let it = game.items.get_item_type(*server_id as usize);
                    if !it.name.is_empty() {
                        format!("You see {}.", it.name)
                    } else {
                        format!("You see an item (id={}).", server_id)
                    }
                }
            };
            crate::net::game_protocol::send_packet_to_player(player_id, move |out: &mut crate::net::output_message::OutputMessage| {
                out.add_byte(0xB4);
                out.add_byte(0x13); // MESSAGE_INFO_DESCR
                out.add_string(desc.as_bytes());
            });
            return;
        }

        let lua = crate::lua::script::g_lua();

        let result = (|| -> mlua::Result<()> {
            let func: mlua::Function = self.script_interface.push_function(self.info.player_on_look)?;

            let player_tbl = lua.create_table()?;
            player_tbl.raw_set(1, player_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = player_tbl.set_metatable(Some(mt));
            }

            let thing_tbl: mlua::Value = match thing_type {
                LookThingType::Creature(cid, class_name) => {
                    let t = lua.create_table()?;
                    t.raw_set(1, cid)?;
                    if let Ok(mt) = lua.named_registry_value::<mlua::Table>(class_name) {
                        let _ = t.set_metatable(Some(mt));
                    }
                    mlua::Value::Table(t)
                }
                LookThingType::Item(server_id, count) => {
                    let t = lua.create_table()?;
                    t.raw_set(1, server_id as i64)?;
                    t.raw_set("_pos_x", pos.x)?;
                    t.raw_set("_pos_y", pos.y)?;
                    t.raw_set("_pos_z", pos.z)?;
                    t.raw_set("_count", count as i64)?;
                    if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Item") {
                        let _ = t.set_metatable(Some(mt));
                    }
                    mlua::Value::Table(t)
                }
            };

            let pos_tbl = lua.create_table()?;
            pos_tbl.set("x", pos.x as i64)?;
            pos_tbl.set("y", pos.y as i64)?;
            pos_tbl.set("z", pos.z as i64)?;
            pos_tbl.set("stackpos", stackpos as i64)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Position") {
                let _ = pos_tbl.set_metatable(Some(mt));
            }

            if let Err(e) = func.call::<()>((player_tbl, thing_tbl, pos_tbl, look_distance)) {
                tracing::error!("eventPlayerOnLook error: {e}");
            }

            Ok(())
        })();

        if let Err(e) = result {
            tracing::error!("eventPlayerOnLook setup error: {e}");
        }
    }

    pub fn event_player_on_look_in_battle_list(
        &self,
        player_id: crate::creatures::CreatureId,
        creature_id: crate::creatures::CreatureId,
        creature_class: &str,
        look_distance: i32,
    ) {
        if self.info.player_on_look_in_battle_list == NONE_EVENT_ID {
            return;
        }

        let lua = crate::lua::script::g_lua();

        let result = (|| -> mlua::Result<()> {
            let func: mlua::Function = self.script_interface.push_function(self.info.player_on_look_in_battle_list)?;

            let player_tbl = lua.create_table()?;
            player_tbl.raw_set(1, player_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = player_tbl.set_metatable(Some(mt));
            }

            let creature_tbl = lua.create_table()?;
            creature_tbl.raw_set(1, creature_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>(creature_class) {
                let _ = creature_tbl.set_metatable(Some(mt));
            }

            if let Err(e) = func.call::<()>((player_tbl, creature_tbl, look_distance)) {
                tracing::error!("eventPlayerOnLookInBattleList error: {e}");
            }

            Ok(())
        })();

        if let Err(e) = result {
            tracing::error!("eventPlayerOnLookInBattleList setup error: {e}");
        }
    }

    pub fn event_player_on_look_in_trade(
        &self,
        player_id: crate::creatures::CreatureId,
        partner_id: crate::creatures::CreatureId,
        item_server_id: u16,
        item_count: u32,
        look_distance: i32,
    ) {
        if self.info.player_on_look_in_trade == NONE_EVENT_ID {
            return;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<()> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.player_on_look_in_trade)?;

            let player_tbl = lua.create_table()?;
            player_tbl.raw_set(1, player_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = player_tbl.set_metatable(Some(mt));
            }

            let partner_tbl = lua.create_table()?;
            partner_tbl.raw_set(1, partner_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = partner_tbl.set_metatable(Some(mt));
            }

            let item_tbl = lua.create_table()?;
            item_tbl.raw_set(1, item_server_id as i64)?;
            item_tbl.raw_set("_count", item_count as i64)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Item") {
                let _ = item_tbl.set_metatable(Some(mt));
            }

            if let Err(e) = func.call::<()>((player_tbl, partner_tbl, item_tbl, look_distance)) {
                tracing::error!("eventPlayerOnLookInTrade error: {e}");
            }
            Ok(())
        })();
        if let Err(e) = result {
            tracing::error!("eventPlayerOnLookInTrade setup error: {e}");
        }
    }

    pub fn event_player_on_look_in_shop(
        &self,
        player_id: crate::creatures::CreatureId,
        item_type_id: u16,
        count: u8,
        description: &str,
    ) -> bool {
        if self.info.player_on_look_in_shop == NONE_EVENT_ID {
            return true;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<bool> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.player_on_look_in_shop)?;

            let player_tbl = lua.create_table()?;
            player_tbl.raw_set(1, player_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = player_tbl.set_metatable(Some(mt));
            }

            let itemtype_tbl = lua.create_table()?;
            itemtype_tbl.raw_set(1, item_type_id as i64)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("ItemType") {
                let _ = itemtype_tbl.set_metatable(Some(mt));
            }

            match func.call::<bool>((player_tbl, itemtype_tbl, count as f64, description.to_owned())) {
                Ok(v) => Ok(v),
                Err(e) => {
                    tracing::error!("eventPlayerOnLookInShop error: {e}");
                    Ok(true)
                }
            }
        })();
        result.unwrap_or(true)
    }

    pub fn event_player_on_move_item(
        &self,
        player_id: crate::creatures::CreatureId,
        item_server_id: u16,
        item_count: u16,
        from_pos: crate::map::Position,
        to_pos: crate::map::Position,
    ) -> bool {
        if self.info.player_on_move_item == NONE_EVENT_ID {
            return true;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<bool> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.player_on_move_item)?;

            let player_tbl = lua.create_table()?;
            player_tbl.raw_set(1, player_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = player_tbl.set_metatable(Some(mt));
            }

            let item_tbl = lua.create_table()?;
            item_tbl.raw_set(1, item_server_id as i64)?;
            item_tbl.raw_set("_count", item_count as i64)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Item") {
                let _ = item_tbl.set_metatable(Some(mt));
            }

            let from_tbl = lua.create_table()?;
            from_tbl.set("x", from_pos.x as i64)?;
            from_tbl.set("y", from_pos.y as i64)?;
            from_tbl.set("z", from_pos.z as i64)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Position") {
                let _ = from_tbl.set_metatable(Some(mt));
            }

            let to_tbl = lua.create_table()?;
            to_tbl.set("x", to_pos.x as i64)?;
            to_tbl.set("y", to_pos.y as i64)?;
            to_tbl.set("z", to_pos.z as i64)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Position") {
                let _ = to_tbl.set_metatable(Some(mt));
            }

            match func.call::<bool>((player_tbl, item_tbl, item_count as f64, from_tbl, to_tbl, mlua::Value::Nil, mlua::Value::Nil)) {
                Ok(v) => Ok(v),
                Err(e) => {
                    tracing::error!("eventPlayerOnMoveItem error: {e}");
                    Ok(true)
                }
            }
        })();
        result.unwrap_or(true)
    }

    pub fn event_player_on_item_moved(
        &self,
        player_id: crate::creatures::CreatureId,
        item_server_id: u16,
        item_count: u16,
        from_pos: crate::map::Position,
        to_pos: crate::map::Position,
    ) {
        if self.info.player_on_item_moved == NONE_EVENT_ID {
            return;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<()> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.player_on_item_moved)?;

            let player_tbl = lua.create_table()?;
            player_tbl.raw_set(1, player_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = player_tbl.set_metatable(Some(mt));
            }

            let item_tbl = lua.create_table()?;
            item_tbl.raw_set(1, item_server_id as i64)?;
            item_tbl.raw_set("_count", item_count as i64)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Item") {
                let _ = item_tbl.set_metatable(Some(mt));
            }

            let from_tbl = lua.create_table()?;
            from_tbl.set("x", from_pos.x as i64)?;
            from_tbl.set("y", from_pos.y as i64)?;
            from_tbl.set("z", from_pos.z as i64)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Position") {
                let _ = from_tbl.set_metatable(Some(mt));
            }

            let to_tbl = lua.create_table()?;
            to_tbl.set("x", to_pos.x as i64)?;
            to_tbl.set("y", to_pos.y as i64)?;
            to_tbl.set("z", to_pos.z as i64)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Position") {
                let _ = to_tbl.set_metatable(Some(mt));
            }

            if let Err(e) = func.call::<()>((player_tbl, item_tbl, item_count as f64, from_tbl, to_tbl, mlua::Value::Nil, mlua::Value::Nil)) {
                tracing::error!("eventPlayerOnItemMoved error: {e}");
            }
            Ok(())
        })();
        if let Err(e) = result {
            tracing::error!("eventPlayerOnItemMoved setup error: {e}");
        }
    }

    pub fn event_player_on_move_creature(
        &self,
        player_id: crate::creatures::CreatureId,
        creature_id: crate::creatures::CreatureId,
        creature_class: &str,
        from_pos: crate::map::Position,
        to_pos: crate::map::Position,
    ) -> bool {
        if self.info.player_on_move_creature == NONE_EVENT_ID {
            return true;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<bool> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.player_on_move_creature)?;

            let player_tbl = lua.create_table()?;
            player_tbl.raw_set(1, player_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = player_tbl.set_metatable(Some(mt));
            }

            let creature_tbl = lua.create_table()?;
            creature_tbl.raw_set(1, creature_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>(creature_class) {
                let _ = creature_tbl.set_metatable(Some(mt));
            }

            let from_tbl = lua.create_table()?;
            from_tbl.set("x", from_pos.x as i64)?;
            from_tbl.set("y", from_pos.y as i64)?;
            from_tbl.set("z", from_pos.z as i64)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Position") {
                let _ = from_tbl.set_metatable(Some(mt));
            }

            let to_tbl = lua.create_table()?;
            to_tbl.set("x", to_pos.x as i64)?;
            to_tbl.set("y", to_pos.y as i64)?;
            to_tbl.set("z", to_pos.z as i64)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Position") {
                let _ = to_tbl.set_metatable(Some(mt));
            }

            match func.call::<bool>((player_tbl, creature_tbl, from_tbl, to_tbl)) {
                Ok(v) => Ok(v),
                Err(e) => {
                    tracing::error!("eventPlayerOnMoveCreature error: {e}");
                    Ok(true)
                }
            }
        })();
        result.unwrap_or(true)
    }

    pub fn event_player_on_report_rule_violation(
        &self,
        player_id: crate::creatures::CreatureId,
        target_name: &str,
        report_type: u8,
        report_reason: u8,
        comment: &str,
        translation: &str,
    ) {
        if self.info.player_on_report_rule_violation == NONE_EVENT_ID {
            return;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<()> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.player_on_report_rule_violation)?;

            let player_tbl = lua.create_table()?;
            player_tbl.raw_set(1, player_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = player_tbl.set_metatable(Some(mt));
            }

            if let Err(e) = func.call::<()>((
                player_tbl,
                target_name.to_owned(),
                report_type as f64,
                report_reason as f64,
                comment.to_owned(),
                translation.to_owned(),
            )) {
                tracing::error!("eventPlayerOnReportRuleViolation error: {e}");
            }
            Ok(())
        })();
        if let Err(e) = result {
            tracing::error!("eventPlayerOnReportRuleViolation setup error: {e}");
        }
    }

    pub fn event_player_on_report_bug(
        &self,
        player_id: crate::creatures::CreatureId,
        message: &str,
    ) -> bool {
        if self.info.player_on_report_bug == NONE_EVENT_ID {
            return true;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<bool> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.player_on_report_bug)?;

            let player_tbl = lua.create_table()?;
            player_tbl.raw_set(1, player_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = player_tbl.set_metatable(Some(mt));
            }

            match func.call::<bool>((player_tbl, message.to_owned())) {
                Ok(v) => Ok(v),
                Err(e) => {
                    tracing::error!("eventPlayerOnReportBug error: {e}");
                    Ok(true)
                }
            }
        })();
        result.unwrap_or(true)
    }

    pub fn event_player_on_turn(
        &self,
        player_id: crate::creatures::CreatureId,
        direction: u8,
    ) -> bool {
        if self.info.player_on_turn == NONE_EVENT_ID {
            return true;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<bool> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.player_on_turn)?;

            let player_tbl = lua.create_table()?;
            player_tbl.raw_set(1, player_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = player_tbl.set_metatable(Some(mt));
            }

            match func.call::<bool>((player_tbl, direction as f64)) {
                Ok(v) => Ok(v),
                Err(e) => {
                    tracing::error!("eventPlayerOnTurn error: {e}");
                    Ok(true)
                }
            }
        })();
        result.unwrap_or(true)
    }

    pub fn event_player_on_trade_request(
        &self,
        player_id: crate::creatures::CreatureId,
        target_id: crate::creatures::CreatureId,
        item_server_id: u16,
        item_count: u32,
    ) -> bool {
        if self.info.player_on_trade_request == NONE_EVENT_ID {
            return true;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<bool> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.player_on_trade_request)?;

            let player_tbl = lua.create_table()?;
            player_tbl.raw_set(1, player_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = player_tbl.set_metatable(Some(mt));
            }

            let target_tbl = lua.create_table()?;
            target_tbl.raw_set(1, target_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = target_tbl.set_metatable(Some(mt));
            }

            let item_tbl = lua.create_table()?;
            item_tbl.raw_set(1, item_server_id as i64)?;
            item_tbl.raw_set("_count", item_count as i64)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Item") {
                let _ = item_tbl.set_metatable(Some(mt));
            }

            match func.call::<bool>((player_tbl, target_tbl, item_tbl)) {
                Ok(v) => Ok(v),
                Err(e) => {
                    tracing::error!("eventPlayerOnTradeRequest error: {e}");
                    Ok(true)
                }
            }
        })();
        result.unwrap_or(true)
    }

    pub fn event_player_on_trade_accept(
        &self,
        player_id: crate::creatures::CreatureId,
        target_id: crate::creatures::CreatureId,
        item_server_id: u16,
        item_count: u32,
        target_item_server_id: u16,
        target_item_count: u32,
    ) -> bool {
        if self.info.player_on_trade_accept == NONE_EVENT_ID {
            return true;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<bool> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.player_on_trade_accept)?;

            let player_tbl = lua.create_table()?;
            player_tbl.raw_set(1, player_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = player_tbl.set_metatable(Some(mt));
            }

            let target_tbl = lua.create_table()?;
            target_tbl.raw_set(1, target_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = target_tbl.set_metatable(Some(mt));
            }

            let item_tbl = lua.create_table()?;
            item_tbl.raw_set(1, item_server_id as i64)?;
            item_tbl.raw_set("_count", item_count as i64)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Item") {
                let _ = item_tbl.set_metatable(Some(mt));
            }

            let target_item_tbl = lua.create_table()?;
            target_item_tbl.raw_set(1, target_item_server_id as i64)?;
            target_item_tbl.raw_set("_count", target_item_count as i64)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Item") {
                let _ = target_item_tbl.set_metatable(Some(mt));
            }

            match func.call::<bool>((player_tbl, target_tbl, item_tbl, target_item_tbl)) {
                Ok(v) => Ok(v),
                Err(e) => {
                    tracing::error!("eventPlayerOnTradeAccept error: {e}");
                    Ok(true)
                }
            }
        })();
        result.unwrap_or(true)
    }

    pub fn event_player_on_trade_completed(
        &self,
        player_id: crate::creatures::CreatureId,
        target_id: crate::creatures::CreatureId,
        item_server_id: u16,
        target_item_server_id: u16,
        is_success: bool,
    ) {
        if self.info.player_on_trade_completed == NONE_EVENT_ID {
            return;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<()> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.player_on_trade_completed)?;

            let player_tbl = lua.create_table()?;
            player_tbl.raw_set(1, player_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = player_tbl.set_metatable(Some(mt));
            }

            let target_tbl = lua.create_table()?;
            target_tbl.raw_set(1, target_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = target_tbl.set_metatable(Some(mt));
            }

            let item_tbl = lua.create_table()?;
            item_tbl.raw_set(1, item_server_id as i64)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Item") {
                let _ = item_tbl.set_metatable(Some(mt));
            }

            let target_item_tbl = lua.create_table()?;
            target_item_tbl.raw_set(1, target_item_server_id as i64)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Item") {
                let _ = target_item_tbl.set_metatable(Some(mt));
            }

            if let Err(e) = func.call::<()>((player_tbl, target_tbl, item_tbl, target_item_tbl, is_success)) {
                tracing::error!("eventPlayerOnTradeCompleted error: {e}");
            }
            Ok(())
        })();
        if let Err(e) = result {
            tracing::error!("eventPlayerOnTradeCompleted setup error: {e}");
        }
    }

    /// Player:onGainExperience(source, exp, rawExp) — returns the modified exp.
    /// Mirrors `Events::eventPlayerOnGainExperience`. When no script is bound,
    /// returns `exp` unchanged.
    pub fn event_player_on_gain_experience(
        &self,
        player_id: crate::creatures::CreatureId,
        source_id: Option<crate::creatures::CreatureId>,
        source_class: &str,
        exp: u64,
        raw_exp: u64,
    ) -> u64 {
        if self.info.player_on_gain_experience == NONE_EVENT_ID {
            return exp;
        }

        let lua = crate::lua::script::g_lua();

        let result = (|| -> mlua::Result<u64> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.player_on_gain_experience)?;

            let player_tbl = lua.create_table()?;
            player_tbl.raw_set(1, player_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = player_tbl.set_metatable(Some(mt));
            }

            let source_val: mlua::Value = match source_id {
                Some(cid) => {
                    let t = lua.create_table()?;
                    t.raw_set(1, cid)?;
                    if let Ok(mt) = lua.named_registry_value::<mlua::Table>(source_class) {
                        let _ = t.set_metatable(Some(mt));
                    }
                    mlua::Value::Table(t)
                }
                None => mlua::Value::Nil,
            };

            match func.call::<f64>((player_tbl, source_val, exp as f64, raw_exp as f64)) {
                Ok(v) => Ok(v.max(0.0) as u64),
                Err(e) => {
                    tracing::error!("eventPlayerOnGainExperience error: {e}");
                    Ok(exp)
                }
            }
        })();

        result.unwrap_or(exp)
    }

    pub fn event_player_on_lose_experience(
        &self,
        player_id: crate::creatures::CreatureId,
        exp: u64,
    ) -> u64 {
        if self.info.player_on_lose_experience == NONE_EVENT_ID {
            return exp;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<u64> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.player_on_lose_experience)?;

            let player_tbl = lua.create_table()?;
            player_tbl.raw_set(1, player_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = player_tbl.set_metatable(Some(mt));
            }

            match func.call::<f64>((player_tbl, exp as f64)) {
                Ok(v) => Ok(v.max(0.0) as u64),
                Err(e) => {
                    tracing::error!("eventPlayerOnLoseExperience error: {e}");
                    Ok(exp)
                }
            }
        })();
        result.unwrap_or(exp)
    }

    pub fn event_player_on_gain_skill_tries(
        &self,
        player_id: crate::creatures::CreatureId,
        skill: u8,
        tries: u64,
    ) -> u64 {
        if self.info.player_on_gain_skill_tries == NONE_EVENT_ID {
            return tries;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<u64> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.player_on_gain_skill_tries)?;

            let player_tbl = lua.create_table()?;
            player_tbl.raw_set(1, player_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Player") {
                let _ = player_tbl.set_metatable(Some(mt));
            }

            match func.call::<f64>((player_tbl, skill as f64, tries as f64)) {
                Ok(v) => Ok(v.max(0.0) as u64),
                Err(e) => {
                    tracing::error!("eventPlayerOnGainSkillTries error: {e}");
                    Ok(tries)
                }
            }
        })();
        result.unwrap_or(tries)
    }

    // -----------------------------------------------------------------------
    // Monster events
    // -----------------------------------------------------------------------

    pub fn event_monster_on_drop_loot(
        &self,
        monster_id: crate::creatures::CreatureId,
        corpse_server_id: u16,
        corpse_pos: crate::map::Position,
    ) {
        if self.info.monster_on_drop_loot == NONE_EVENT_ID {
            return;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<()> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.monster_on_drop_loot)?;

            let monster_tbl = lua.create_table()?;
            monster_tbl.raw_set(1, monster_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Monster") {
                let _ = monster_tbl.set_metatable(Some(mt));
            }

            let corpse_tbl = lua.create_table()?;
            corpse_tbl.raw_set(1, corpse_server_id as i64)?;
            corpse_tbl.raw_set("_pos_x", corpse_pos.x)?;
            corpse_tbl.raw_set("_pos_y", corpse_pos.y)?;
            corpse_tbl.raw_set("_pos_z", corpse_pos.z)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Container") {
                let _ = corpse_tbl.set_metatable(Some(mt));
            }

            if let Err(e) = func.call::<()>((monster_tbl, corpse_tbl)) {
                tracing::error!("eventMonsterOnDropLoot error: {e}");
            }
            Ok(())
        })();
        if let Err(e) = result {
            tracing::error!("eventMonsterOnDropLoot setup error: {e}");
        }
    }

    pub fn event_monster_on_spawn(
        &self,
        monster_id: crate::creatures::CreatureId,
        pos: crate::map::Position,
        is_startup: bool,
        is_artificial: bool,
    ) -> bool {
        if self.info.monster_on_spawn == NONE_EVENT_ID {
            return true;
        }

        let lua = crate::lua::script::g_lua();
        let result = (|| -> mlua::Result<bool> {
            let func: mlua::Function =
                self.script_interface.push_function(self.info.monster_on_spawn)?;

            let monster_tbl = lua.create_table()?;
            monster_tbl.raw_set(1, monster_id)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Monster") {
                let _ = monster_tbl.set_metatable(Some(mt));
            }

            let pos_tbl = lua.create_table()?;
            pos_tbl.set("x", pos.x as i64)?;
            pos_tbl.set("y", pos.y as i64)?;
            pos_tbl.set("z", pos.z as i64)?;
            if let Ok(mt) = lua.named_registry_value::<mlua::Table>("Position") {
                let _ = pos_tbl.set_metatable(Some(mt));
            }

            match func.call::<bool>((monster_tbl, pos_tbl, is_startup, is_artificial)) {
                Ok(v) => Ok(v),
                Err(e) => {
                    tracing::error!("eventMonsterOnSpawn error: {e}");
                    Ok(false)
                }
            }
        })();
        result.unwrap_or(false)
    }
}

impl Default for Events {
    fn default() -> Self {
        Self::new()
    }
}
