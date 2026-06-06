use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::lua::script::LuaScriptInterface;

// ---------------------------------------------------------------------------
// JSON5 schema
// ---------------------------------------------------------------------------

/// A single entry in `data/actions/actions.json5`. Mirrors an `<action>` node
/// from the C++ `actions.xml`.
#[derive(Debug, Deserialize)]
pub struct ActionEntry {
    pub script: Option<String>,
    #[serde(rename = "itemid")]
    pub item_id: Option<JsonValue>,
    #[serde(rename = "fromid")]
    pub from_id: Option<u16>,
    #[serde(rename = "toid")]
    pub to_id: Option<u16>,
    #[serde(rename = "uniqueid")]
    pub unique_id: Option<JsonValue>,
    #[serde(rename = "fromuid")]
    pub from_uid: Option<u16>,
    #[serde(rename = "touid")]
    pub to_uid: Option<u16>,
    #[serde(rename = "actionid")]
    pub action_id: Option<JsonValue>,
    #[serde(rename = "fromaid")]
    pub from_aid: Option<u16>,
    #[serde(rename = "toaid")]
    pub to_aid: Option<u16>,
    #[serde(rename = "allowfaruse")]
    pub allow_far_use: Option<bool>,
    /// `blockwalls` maps to `checkLineOfSight` (C++ `configureEvent` parity).
    #[serde(rename = "blockwalls")]
    pub block_walls: Option<bool>,
    #[serde(rename = "checkfloor")]
    pub check_floor: Option<bool>,
}

/// Top-level document for `data/actions/actions.json5`.
#[derive(Debug, Deserialize)]
pub struct ActionsFile {
    pub actions: Vec<ActionEntry>,
}

// ---------------------------------------------------------------------------
// Runtime types
// ---------------------------------------------------------------------------

/// Mirrors the C++ `Action` class.
#[derive(Debug, Clone)]
pub struct Action {
    pub script_id: i32,
    pub script_interface_name: String,
    pub allow_far_use: bool,
    pub check_floor: bool,
    pub check_line_of_sight: bool,
    pub item_ids: Vec<u16>,
    pub unique_ids: Vec<u16>,
    pub action_ids: Vec<u16>,
    pub scripted: bool,
    pub from_lua: bool,
}

impl Default for Action {
    fn default() -> Self {
        Self {
            script_id: 0,
            script_interface_name: String::new(),
            allow_far_use: false,
            check_floor: true,
            check_line_of_sight: true,
            item_ids: Vec::new(),
            unique_ids: Vec::new(),
            action_ids: Vec::new(),
            scripted: false,
            from_lua: false,
        }
    }
}

/// Mirrors the C++ `Actions` class (inherits `BaseEvents`).
pub struct Actions {
    script_interface: LuaScriptInterface,
    use_item_map: BTreeMap<u16, Action>,
    unique_item_map: BTreeMap<u16, Action>,
    action_item_map: BTreeMap<u16, Action>,
}

impl Actions {
    /// Mirrors `Actions::Actions()`.
    pub fn new() -> Self {
        let mut iface = LuaScriptInterface::new("Action Interface");
        if let Err(e) = iface.init_state() {
            tracing::error!("Actions::new - init_state failed: {e}");
        }
        let lib = "data/actions/lib/actions.lua";
        if let Err(e) = iface.load_file(lib) {
            tracing::warn!("Actions::new - cannot load actions lib: {e}");
        }
        Self {
            script_interface: iface,
            use_item_map: BTreeMap::new(),
            unique_item_map: BTreeMap::new(),
            action_item_map: BTreeMap::new(),
        }
    }

    /// Script base name used for path construction. Mirrors
    /// `Actions::getScriptBaseName()`.
    pub fn get_script_base_name() -> &'static str {
        "actions"
    }

    /// Load `data/actions/actions.json5`. Returns `true` on success.
    /// Mirrors `BaseEvents::loadFromXml()` specialised for Actions.
    pub fn load_from_json5(&mut self) -> bool {
        let path = "data/actions/actions.json5";
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Actions::load_from_json5 - {path} not found: {e}");
                return false;
            }
        };

        let file: ActionsFile = match json5::from_str(&source) {
            Ok(f) => f,
            Err(e) => {
                tracing::error!("Actions::load_from_json5 - parse error in {path}: {e}");
                return false;
            }
        };

        for entry in file.actions {
            let mut action = Action {
                allow_far_use: entry.allow_far_use.unwrap_or(false),
                check_line_of_sight: entry.block_walls.unwrap_or(true),
                check_floor: entry.check_floor.unwrap_or(true),
                script_interface_name: self.script_interface.interface_name.clone(),
                ..Default::default()
            };

            if let Some(ref script_file) = entry.script {
                let script_path = format!("data/actions/scripts/{script_file}");
                match self.script_interface.load_file(&script_path) {
                    Ok(()) => {
                        let id = self
                            .script_interface
                            .get_event("onUse");
                        if id == -1 {
                            tracing::warn!(
                                "Actions::load_from_json5 - onUse not found in {script_path}"
                            );
                            continue;
                        }
                        action.script_id = id;
                        action.scripted = true;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Actions::load_from_json5 - cannot load {script_path}: {e}"
                        );
                        continue;
                    }
                }
            }

            let item_ids = collect_ids(&entry.item_id, entry.from_id, entry.to_id);
            let uid_ids = collect_ids(&entry.unique_id, entry.from_uid, entry.to_uid);
            let aid_ids = collect_ids(&entry.action_id, entry.from_aid, entry.to_aid);

            if !item_ids.is_empty() {
                for id in &item_ids {
                    action.item_ids.push(*id);
                }
                for id in item_ids {
                    if let std::collections::btree_map::Entry::Vacant(e) =
                        self.use_item_map.entry(id)
                    {
                        e.insert(action.clone());
                    } else {
                        tracing::warn!(
                            "Actions::load_from_json5 - duplicate item id: {id}"
                        );
                    }
                }
            } else if !uid_ids.is_empty() {
                for id in &uid_ids {
                    action.unique_ids.push(*id);
                }
                for id in uid_ids {
                    if let std::collections::btree_map::Entry::Vacant(e) =
                        self.unique_item_map.entry(id)
                    {
                        e.insert(action.clone());
                    } else {
                        tracing::warn!(
                            "Actions::load_from_json5 - duplicate unique id: {id}"
                        );
                    }
                }
            } else if !aid_ids.is_empty() {
                for id in &aid_ids {
                    action.action_ids.push(*id);
                }
                for id in aid_ids {
                    if let std::collections::btree_map::Entry::Vacant(e) =
                        self.action_item_map.entry(id)
                    {
                        e.insert(action.clone());
                    } else {
                        tracing::warn!(
                            "Actions::load_from_json5 - duplicate action id: {id}"
                        );
                    }
                }
            }
        }

        true
    }

    /// Register an action defined from a Lua script. Mirrors
    /// `Actions::registerLuaEvent()`.
    pub fn register_lua_event(&mut self, action: Action) -> bool {
        if !action.item_ids.is_empty() {
            for id in &action.item_ids {
                if let std::collections::btree_map::Entry::Vacant(e) =
                    self.use_item_map.entry(*id)
                {
                    e.insert(action.clone());
                } else {
                    tracing::warn!(
                        "Actions::register_lua_event - duplicate item id: {id}"
                    );
                }
            }
            return true;
        }

        if !action.unique_ids.is_empty() {
            for id in &action.unique_ids {
                if let std::collections::btree_map::Entry::Vacant(e) =
                    self.unique_item_map.entry(*id)
                {
                    e.insert(action.clone());
                } else {
                    tracing::warn!(
                        "Actions::register_lua_event - duplicate unique id: {id}"
                    );
                }
            }
            return true;
        }

        if !action.action_ids.is_empty() {
            for id in &action.action_ids {
                if let std::collections::btree_map::Entry::Vacant(e) =
                    self.action_item_map.entry(*id)
                {
                    e.insert(action.clone());
                } else {
                    tracing::warn!(
                        "Actions::register_lua_event - duplicate action id: {id}"
                    );
                }
            }
            return true;
        }

        tracing::warn!("Actions::register_lua_event - no id/aid/uid set for event");
        false
    }

    /// Clear entries matching `from_lua`. Mirrors `Actions::clear()`.
    pub fn clear(&mut self, from_lua: bool) {
        self.use_item_map.retain(|_, v| v.from_lua != from_lua);
        self.unique_item_map.retain(|_, v| v.from_lua != from_lua);
        self.action_item_map.retain(|_, v| v.from_lua != from_lua);

        if !from_lua {
            if let Err(e) = self.script_interface.re_init_state() {
                tracing::error!("Actions::clear - re_init_state failed: {e}");
            }
        }
    }

    pub fn get_action_by_item_id(&self, item_id: u16) -> Option<&Action> {
        self.use_item_map.get(&item_id)
    }

    pub fn get_action_by_unique_id(&self, uid: u16) -> Option<&Action> {
        self.unique_item_map.get(&uid)
    }

    pub fn get_action_by_action_id(&self, aid: u16) -> Option<&Action> {
        self.action_item_map.get(&aid)
    }

    pub fn get_action(&self, item_id: u16, unique_id: u16, action_id: u16) -> Option<&Action> {
        if unique_id > 0 {
            if let Some(a) = self.unique_item_map.get(&unique_id) {
                return Some(a);
            }
        }
        if action_id > 0 {
            if let Some(a) = self.action_item_map.get(&action_id) {
                return Some(a);
            }
        }
        self.use_item_map.get(&item_id)
    }

    pub fn get_script_function(&self, script_id: i32) -> Option<mlua::Function> {
        self.script_interface.push_function(script_id).ok()
    }
}

impl Default for Actions {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a JSON5 field that can be a single integer or an array of integers
/// into a `Vec<u16>`. Also handles an optional `from`/`to` range pair.
fn collect_ids(
    value: &Option<JsonValue>,
    from: Option<u16>,
    to: Option<u16>,
) -> Vec<u16> {
    let mut ids: Vec<u16> = Vec::new();

    if let Some(v) = value {
        match v {
            JsonValue::Number(n) => {
                if let Some(id) = n.as_u64().and_then(|n| u16::try_from(n).ok()) {
                    ids.push(id);
                }
            }
            JsonValue::Array(arr) => {
                for item in arr {
                    if let Some(id) = item
                        .as_u64()
                        .and_then(|n| u16::try_from(n).ok())
                    {
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
