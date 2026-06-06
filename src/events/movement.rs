use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::lua::script::LuaScriptInterface;
use crate::map::Position;

// ---------------------------------------------------------------------------
// MoveEventType
// ---------------------------------------------------------------------------

/// Mirrors `MoveEvent_t` from `movement.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MoveEventType {
    StepIn = 0,
    StepOut = 1,
    Equip = 2,
    DeEquip = 3,
    AddItem = 4,
    RemoveItem = 5,
    AddItemTile = 6,
    RemoveItemTile = 7,
}

impl MoveEventType {
    /// Parse the event type string as used in JSON5 / XML.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "stepin" => Some(Self::StepIn),
            "stepout" => Some(Self::StepOut),
            "equip" => Some(Self::Equip),
            "deequip" => Some(Self::DeEquip),
            "additem" => Some(Self::AddItem),
            "removeitem" => Some(Self::RemoveItem),
            _ => None,
        }
    }

    /// Lua event function name. Mirrors `MoveEvent::getScriptEventName()`.
    pub fn script_event_name(self) -> &'static str {
        match self {
            Self::StepIn => "onStepIn",
            Self::StepOut => "onStepOut",
            Self::Equip => "onEquip",
            Self::DeEquip => "onDeEquip",
            Self::AddItem | Self::AddItemTile => "onAddItem",
            Self::RemoveItem | Self::RemoveItemTile => "onRemoveItem",
        }
    }
}

// ---------------------------------------------------------------------------
// JSON5 schema
// ---------------------------------------------------------------------------

/// Position sub-object in a movement entry.
#[derive(Debug, Deserialize)]
pub struct MoveEventPosEntry {
    pub x: u16,
    pub y: u16,
    pub z: u8,
}

/// A single entry in `data/movements/movements.json5`. Mirrors a
/// `<movevent>` XML node.
#[derive(Debug, Deserialize)]
pub struct MoveEventEntry {
    pub script: Option<String>,
    pub event: Option<String>,
    #[serde(rename = "itemid")]
    pub item_id: Option<JsonValue>,
    #[serde(rename = "fromid")]
    pub from_id: Option<u32>,
    #[serde(rename = "toid")]
    pub to_id: Option<u32>,
    #[serde(rename = "uniqueid")]
    pub unique_id: Option<JsonValue>,
    #[serde(rename = "fromuid")]
    pub from_uid: Option<u32>,
    #[serde(rename = "touid")]
    pub to_uid: Option<u32>,
    #[serde(rename = "actionid")]
    pub action_id: Option<JsonValue>,
    #[serde(rename = "fromaid")]
    pub from_aid: Option<u32>,
    #[serde(rename = "toaid")]
    pub to_aid: Option<u32>,
    pub pos: Option<MoveEventPosEntry>,
    pub slot: Option<String>,
    pub level: Option<u32>,
    #[serde(rename = "maglevel")]
    pub mag_level: Option<u32>,
    #[serde(default, deserialize_with = "crate::util::json5::deserialize_bool_or_int")]
    pub premium: Option<bool>,
    #[serde(default, deserialize_with = "crate::util::json5::deserialize_bool_or_int")]
    pub tileitem: Option<bool>,
}

/// Wrapper for the nested JSON5 structure.
#[derive(Debug, Deserialize)]
struct MoveEventsWrapper {
    movements: MoveEventsFile,
}

/// Inner document for `data/movements/movements.json5`.
#[derive(Debug, Deserialize)]
pub struct MoveEventsFile {
    pub movevents: Vec<MoveEventEntry>,
}

// ---------------------------------------------------------------------------
// Slot and wield constants
// ---------------------------------------------------------------------------

/// Equipment slot bit flags. Mirrors `SLOTP_*` from `enums.h`.
pub mod slot_position {
    pub const WHEREEVER: u32 = 0xFFFF_FFFF;
    pub const HEAD: u32 = 1 << 0;
    pub const NECKLACE: u32 = 1 << 1;
    pub const BACKPACK: u32 = 1 << 2;
    pub const ARMOR: u32 = 1 << 3;
    pub const RIGHT: u32 = 1 << 4;
    pub const LEFT: u32 = 1 << 5;
    pub const LEGS: u32 = 1 << 6;
    pub const FEET: u32 = 1 << 7;
    pub const RING: u32 = 1 << 8;
    pub const AMMO: u32 = 1 << 9;
}

/// Wield-info bitfield constants. Mirrors `WieldInfo_t`.
pub mod wield_info {
    pub const LEVEL: u32 = 1 << 0;
    pub const MAGIC_LEVEL: u32 = 1 << 1;
    pub const PREMIUM: u32 = 1 << 2;
    pub const VOC_REQ: u32 = 1 << 3;
}

// ---------------------------------------------------------------------------
// VocEquipMap
// ---------------------------------------------------------------------------

/// Key = vocation ID; value = whether to show in item description.
/// Mirrors `VocEquipMap`.
pub type VocEquipMap = BTreeMap<u16, bool>;

// ---------------------------------------------------------------------------
// MoveEvent
// ---------------------------------------------------------------------------

/// Mirrors the C++ `MoveEvent` class.
#[derive(Debug, Clone)]
pub struct MoveEvent {
    pub event_type: MoveEventType,
    pub script_id: i32,
    pub scripted: bool,
    pub from_lua: bool,
    /// Equipment slot mask.
    pub slot: u32,
    pub req_level: u32,
    pub req_mag_level: u32,
    pub premium: bool,
    pub vocation_string: String,
    pub wield_info: u32,
    pub voc_equip_map: VocEquipMap,
    pub tile_item: bool,
    pub item_id_range: Vec<u32>,
    pub action_id_range: Vec<u32>,
    pub unique_id_range: Vec<u32>,
    pub pos_list: Vec<Position>,
}

impl Default for MoveEvent {
    fn default() -> Self {
        Self {
            event_type: MoveEventType::StepIn,
            script_id: 0,
            scripted: false,
            from_lua: false,
            slot: slot_position::WHEREEVER,
            req_level: 0,
            req_mag_level: 0,
            premium: false,
            vocation_string: String::new(),
            wield_info: 0,
            voc_equip_map: BTreeMap::new(),
            tile_item: false,
            item_id_range: Vec::new(),
            action_id_range: Vec::new(),
            unique_id_range: Vec::new(),
            pos_list: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// MoveEventList
// ---------------------------------------------------------------------------

/// Per-ID list of events indexed by `MoveEventType`. Mirrors `MoveEventList`.
#[derive(Debug, Default, Clone)]
pub struct MoveEventList {
    pub events: [Vec<MoveEvent>; 8],
}

impl MoveEventList {
    pub fn add(&mut self, event: MoveEvent) {
        let idx = event.event_type as usize;
        self.events[idx].push(event);
    }

    pub fn get(&self, event_type: MoveEventType) -> &[MoveEvent] {
        &self.events[event_type as usize]
    }

    pub fn get_mut(&mut self, event_type: MoveEventType) -> &mut Vec<MoveEvent> {
        &mut self.events[event_type as usize]
    }
}

// ---------------------------------------------------------------------------
// MoveEvents
// ---------------------------------------------------------------------------

/// Mirrors the C++ `MoveEvents` class (inherits `BaseEvents`).
pub struct MoveEvents {
    script_interface: LuaScriptInterface,
    unique_id_map: BTreeMap<i32, MoveEventList>,
    action_id_map: BTreeMap<i32, MoveEventList>,
    item_id_map: BTreeMap<i32, MoveEventList>,
    position_map: BTreeMap<(u16, u16, u8), MoveEventList>,
}

impl MoveEvents {
    /// Mirrors `MoveEvents::MoveEvents()`.
    pub fn new() -> Self {
        let mut iface = LuaScriptInterface::new("MoveEvents Interface");
        if let Err(e) = iface.init_state() {
            tracing::error!("MoveEvents::new - init_state failed: {e}");
        }
        let lib = "data/movements/lib/movements.lua";
        if let Err(e) = iface.load_file(lib) {
            tracing::warn!("MoveEvents::new - cannot load movements lib: {e}");
        }
        Self {
            script_interface: iface,
            unique_id_map: BTreeMap::new(),
            action_id_map: BTreeMap::new(),
            item_id_map: BTreeMap::new(),
            position_map: BTreeMap::new(),
        }
    }

    /// Script base name. Mirrors `MoveEvents::getScriptBaseName()`.
    pub fn get_script_base_name() -> &'static str {
        "movements"
    }

    /// Load `data/movements/movements.json5`. Returns `true` on success.
    pub fn load_from_json5(&mut self) -> bool {
        let path = "data/movements/movements.json5";
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("MoveEvents::load_from_json5 - {path} not found: {e}");
                return false;
            }
        };

        let wrapper: MoveEventsWrapper = match json5::from_str(&source) {
            Ok(f) => f,
            Err(e) => {
                tracing::error!("MoveEvents::load_from_json5 - parse error in {path}: {e}");
                return false;
            }
        };
        let file = wrapper.movements;

        for entry in file.movevents {
            let event_type = match entry.event.as_deref().and_then(MoveEventType::from_str) {
                Some(t) => t,
                None => {
                    tracing::warn!(
                        "MoveEvents::load_from_json5 - missing or unknown event type"
                    );
                    continue;
                }
            };

            let mut event = MoveEvent {
                event_type,
                req_level: entry.level.unwrap_or(0),
                req_mag_level: entry.mag_level.unwrap_or(0),
                premium: entry.premium.unwrap_or(false),
                tile_item: entry.tileitem.unwrap_or(false),
                ..Default::default()
            };

            if event.req_level > 0 {
                event.wield_info |= wield_info::LEVEL;
            }
            if event.req_mag_level > 0 {
                event.wield_info |= wield_info::MAGIC_LEVEL;
            }
            if event.premium {
                event.wield_info |= wield_info::PREMIUM;
            }

            if let Some(ref slot_str) = entry.slot {
                event.slot = parse_slot(slot_str);
            }

            if let Some(ref script_file) = entry.script {
                let script_path =
                    format!("data/movements/scripts/{script_file}");
                match self.script_interface.load_file(&script_path) {
                    Ok(()) => {
                        let id = self
                            .script_interface
                            .get_event(event.event_type.script_event_name());
                        if id == -1 {
                            tracing::warn!(
                                "MoveEvents::load_from_json5 - {} not found in {script_path}",
                                event.event_type.script_event_name()
                            );
                            continue;
                        }
                        event.script_id = id;
                        event.scripted = true;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "MoveEvents::load_from_json5 - cannot load {script_path}: {e}"
                        );
                        continue;
                    }
                }
            }

            event.event_type = adjust_type_for_tile(event.event_type, event.tile_item);

            let item_ids = collect_u32_ids(&entry.item_id, entry.from_id, entry.to_id);
            let uid_ids = collect_u32_ids(&entry.unique_id, entry.from_uid, entry.to_uid);
            let aid_ids = collect_u32_ids(&entry.action_id, entry.from_aid, entry.to_aid);

            if !item_ids.is_empty() {
                for id in &item_ids {
                    event.item_id_range.push(*id);
                }
                for id in item_ids {
                    self.item_id_map
                        .entry(id as i32)
                        .or_default()
                        .add(event.clone());
                }
            } else if !uid_ids.is_empty() {
                for id in &uid_ids {
                    event.unique_id_range.push(*id);
                }
                for id in uid_ids {
                    self.unique_id_map
                        .entry(id as i32)
                        .or_default()
                        .add(event.clone());
                }
            } else if !aid_ids.is_empty() {
                for id in &aid_ids {
                    event.action_id_range.push(*id);
                }
                for id in aid_ids {
                    self.action_id_map
                        .entry(id as i32)
                        .or_default()
                        .add(event.clone());
                }
            } else if let Some(ref pos_entry) = entry.pos {
                let key = (pos_entry.x, pos_entry.y, pos_entry.z);
                event.pos_list.push(Position {
                    x: pos_entry.x,
                    y: pos_entry.y,
                    z: pos_entry.z,
                });
                self.position_map.entry(key).or_default().add(event.clone());
            } else {
                tracing::warn!(
                    "MoveEvents::load_from_json5 - entry has no id/uid/aid/pos"
                );
            }
        }

        true
    }

    /// Register a move event from a Lua script. Mirrors
    /// `MoveEvents::registerLuaEvent()`.
    pub fn register_lua_event(&mut self, mut event: MoveEvent) -> bool {
        event.event_type = adjust_type_for_tile(event.event_type, event.tile_item);

        if !event.item_id_range.is_empty() {
            for id in event.item_id_range.clone() {
                self.item_id_map
                    .entry(id as i32)
                    .or_default()
                    .add(event.clone());
            }
            return true;
        }

        if !event.action_id_range.is_empty() {
            for id in event.action_id_range.clone() {
                self.action_id_map
                    .entry(id as i32)
                    .or_default()
                    .add(event.clone());
            }
            return true;
        }

        if !event.unique_id_range.is_empty() {
            for id in event.unique_id_range.clone() {
                self.unique_id_map
                    .entry(id as i32)
                    .or_default()
                    .add(event.clone());
            }
            return true;
        }

        if !event.pos_list.is_empty() {
            for pos in event.pos_list.clone() {
                let key = (pos.x, pos.y, pos.z);
                self.position_map.entry(key).or_default().add(event.clone());
            }
            return true;
        }

        false
    }

    /// Clear entries matching `from_lua`. Mirrors `MoveEvents::clear()`.
    pub fn clear(&mut self, from_lua: bool) {
        clear_move_map(&mut self.item_id_map, from_lua);
        clear_move_map(&mut self.action_id_map, from_lua);
        clear_move_map(&mut self.unique_id_map, from_lua);
        clear_pos_map(&mut self.position_map, from_lua);

        if !from_lua {
            if let Err(e) = self.script_interface.re_init_state() {
                tracing::error!("MoveEvents::clear - re_init_state failed: {e}");
            }
        }
    }

    pub fn get_script_function(&self, script_id: i32) -> Option<mlua::Function> {
        self.script_interface.push_function(script_id).ok()
    }

    /// Collect all step events that match the given tile position and tile items.
    /// Returns `Vec<(script_id, from_lua, item_server_id_or_0)>`.
    pub fn collect_step_events(
        &self,
        tile_pos: crate::map::Position,
        event_type: MoveEventType,
        tile_items: &[(u16, u16, u16)], // (server_id, action_id, unique_id)
    ) -> Vec<(i32, bool, u16)> {
        let mut results = Vec::new();

        // Position-based events.
        let pos_key = (tile_pos.x, tile_pos.y, tile_pos.z);
        if let Some(list) = self.position_map.get(&pos_key) {
            for ev in list.get(event_type) {
                if ev.scripted {
                    results.push((ev.script_id, ev.from_lua, 0u16));
                }
            }
        }

        // Item-based events.
        for &(server_id, action_id, unique_id) in tile_items {
            if unique_id > 0 {
                if let Some(list) = self.unique_id_map.get(&(unique_id as i32)) {
                    for ev in list.get(event_type) {
                        if ev.scripted {
                            results.push((ev.script_id, ev.from_lua, server_id));
                        }
                    }
                }
            }
            if action_id > 0 {
                if let Some(list) = self.action_id_map.get(&(action_id as i32)) {
                    for ev in list.get(event_type) {
                        if ev.scripted {
                            results.push((ev.script_id, ev.from_lua, server_id));
                        }
                    }
                }
            }
            if let Some(list) = self.item_id_map.get(&(server_id as i32)) {
                for ev in list.get(event_type) {
                    if ev.scripted {
                        results.push((ev.script_id, ev.from_lua, server_id));
                    }
                }
            }
        }

        results
    }

    /// Collect equip/deequip events for an item.
    pub fn collect_equip_events(
        &self,
        item_server_id: u16,
        item_action_id: u16,
        item_unique_id: u16,
        event_type: MoveEventType,
        slot: u32,
    ) -> Vec<(i32, bool)> {
        let mut results = Vec::new();

        let check_event = |ev: &MoveEvent| -> bool {
            ev.scripted
                && (ev.slot == slot_position::WHEREEVER || ev.slot & slot != 0)
        };

        if item_unique_id > 0 {
            if let Some(list) = self.unique_id_map.get(&(item_unique_id as i32)) {
                for ev in list.get(event_type) {
                    if check_event(ev) {
                        results.push((ev.script_id, ev.from_lua));
                    }
                }
            }
        }
        if item_action_id > 0 {
            if let Some(list) = self.action_id_map.get(&(item_action_id as i32)) {
                for ev in list.get(event_type) {
                    if check_event(ev) {
                        results.push((ev.script_id, ev.from_lua));
                    }
                }
            }
        }
        if let Some(list) = self.item_id_map.get(&(item_server_id as i32)) {
            for ev in list.get(event_type) {
                if check_event(ev) {
                    results.push((ev.script_id, ev.from_lua));
                }
            }
        }

        results
    }
}

impl Default for MoveEvents {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn collect_u32_ids(
    value: &Option<JsonValue>,
    from: Option<u32>,
    to: Option<u32>,
) -> Vec<u32> {
    let mut ids: Vec<u32> = Vec::new();

    if let Some(v) = value {
        match v {
            JsonValue::Number(n) => {
                if let Some(id) = n.as_u64().and_then(|n| u32::try_from(n).ok()) {
                    ids.push(id);
                }
            }
            JsonValue::Array(arr) => {
                for item in arr {
                    if let Some(id) = item.as_u64().and_then(|n| u32::try_from(n).ok()) {
                        ids.push(id);
                    }
                }
            }
            _ => {}
        }
    }

    if let (Some(f), Some(t)) = (from, to) {
        let mut i = f;
        loop {
            ids.push(i);
            if i == t {
                break;
            }
            i = i.saturating_add(1);
            if i > t {
                break;
            }
        }
    }

    ids
}

fn parse_slot(slot: &str) -> u32 {
    match slot.to_lowercase().as_str() {
        "head" => slot_position::HEAD,
        "necklace" => slot_position::NECKLACE,
        "backpack" => slot_position::BACKPACK,
        "armor" => slot_position::ARMOR,
        "right-hand" => slot_position::RIGHT,
        "left-hand" => slot_position::LEFT,
        "hand" | "shield" => slot_position::RIGHT | slot_position::LEFT,
        "legs" => slot_position::LEGS,
        "feet" => slot_position::FEET,
        "ring" => slot_position::RING,
        "ammo" => slot_position::AMMO,
        _ => slot_position::WHEREEVER,
    }
}

fn adjust_type_for_tile(event_type: MoveEventType, tile_item: bool) -> MoveEventType {
    if tile_item {
        match event_type {
            MoveEventType::AddItem => MoveEventType::AddItemTile,
            MoveEventType::RemoveItem => MoveEventType::RemoveItemTile,
            other => other,
        }
    } else {
        event_type
    }
}

fn clear_move_map(map: &mut BTreeMap<i32, MoveEventList>, from_lua: bool) {
    for list in map.values_mut() {
        for events in &mut list.events {
            events.retain(|e| e.from_lua != from_lua);
        }
    }
}

fn clear_pos_map(
    map: &mut BTreeMap<(u16, u16, u8), MoveEventList>,
    from_lua: bool,
) {
    for list in map.values_mut() {
        for events in &mut list.events {
            events.retain(|e| e.from_lua != from_lua);
        }
    }
}
