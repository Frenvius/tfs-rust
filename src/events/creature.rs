use std::collections::BTreeMap;

use serde::Deserialize;

use crate::lua::script::LuaScriptInterface;

// ---------------------------------------------------------------------------
// CreatureEventType
// ---------------------------------------------------------------------------

/// Mirrors `CreatureEventType_t` from `creatureevent.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CreatureEventType {
    None = 0,
    Login = 1,
    Logout = 2,
    Think = 3,
    PrepareDeath = 4,
    Death = 5,
    Kill = 6,
    Advance = 7,
    TextEdit = 8,
    HealthChange = 9,
    ManaChange = 10,
    ExtendedOpcode = 11,
}

impl CreatureEventType {
    /// Parse the event type string from JSON5 / XML.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "login" => Some(Self::Login),
            "logout" => Some(Self::Logout),
            "think" => Some(Self::Think),
            "preparedeath" => Some(Self::PrepareDeath),
            "death" => Some(Self::Death),
            "kill" => Some(Self::Kill),
            "advance" => Some(Self::Advance),
            "textedit" => Some(Self::TextEdit),
            "healthchange" => Some(Self::HealthChange),
            "manachange" => Some(Self::ManaChange),
            "extendedopcode" => Some(Self::ExtendedOpcode),
            _ => None,
        }
    }

    /// Lua function name. Mirrors `CreatureEvent::getScriptEventName()`.
    pub fn script_event_name(self) -> &'static str {
        match self {
            Self::Login => "onLogin",
            Self::Logout => "onLogout",
            Self::Think => "onThink",
            Self::PrepareDeath => "onPrepareDeath",
            Self::Death => "onDeath",
            Self::Kill => "onKill",
            Self::Advance => "onAdvance",
            Self::TextEdit => "onTextEdit",
            Self::HealthChange => "onHealthChange",
            Self::ManaChange => "onManaChange",
            Self::ExtendedOpcode => "onExtendedOpcode",
            Self::None => "",
        }
    }
}

// ---------------------------------------------------------------------------
// JSON5 schema
// ---------------------------------------------------------------------------

/// A single entry in `data/creaturescripts/creaturescripts.json5`.
#[derive(Debug, Deserialize)]
pub struct CreatureScriptEntry {
    pub name: String,
    #[serde(alias = "type")]
    pub event: Option<String>,
    pub script: Option<String>,
}

/// Top-level wrapper for `data/creaturescripts/creaturescripts.json5`.
#[derive(Debug, Deserialize)]
pub struct CreatureScriptsWrapper {
    pub creaturescripts: CreatureScriptsFile,
}

/// Inner file with the events array.
#[derive(Debug, Deserialize)]
pub struct CreatureScriptsFile {
    pub events: Vec<CreatureScriptEntry>,
}

// ---------------------------------------------------------------------------
// Runtime types
// ---------------------------------------------------------------------------

/// Mirrors the C++ `CreatureEvent` class.
#[derive(Debug, Clone)]
pub struct CreatureEvent {
    pub name: String,
    pub event_type: CreatureEventType,
    pub script_id: i32,
    pub scripted: bool,
    pub from_lua: bool,
    pub loaded: bool,
}

impl CreatureEvent {
    /// Clear the event state, keeping the name. Used on reload.
    /// Mirrors `CreatureEvent::clearEvent()`.
    pub fn clear_event(&mut self) {
        self.script_id = 0;
        self.scripted = false;
        self.loaded = false;
    }

    /// Copy the live state from another event. Used on reload.
    /// Mirrors `CreatureEvent::copyEvent()`.
    pub fn copy_event(&mut self, other: &CreatureEvent) {
        self.script_id = other.script_id;
        self.scripted = other.scripted;
        self.loaded = other.loaded;
        self.from_lua = other.from_lua;
    }
}

/// Mirrors the C++ `CreatureEvents` class (inherits `BaseEvents`).
pub struct CreatureEvents {
    script_interface: LuaScriptInterface,
    creature_events: BTreeMap<String, CreatureEvent>,
}

impl CreatureEvents {
    /// Mirrors `CreatureEvents::CreatureEvents()`.
    pub fn new() -> Self {
        let mut iface = LuaScriptInterface::new("CreatureScript Interface");
        if let Err(e) = iface.init_state() {
            tracing::error!("CreatureEvents::new - init_state failed: {e}");
        }
        let lib = "data/creaturescripts/lib/creaturescripts.lua";
        if let Err(e) = iface.load_file(lib) {
            tracing::warn!("CreatureEvents::new - cannot load creaturescripts lib: {e}");
        }
        Self {
            script_interface: iface,
            creature_events: BTreeMap::new(),
        }
    }

    /// Script base name. Mirrors `CreatureEvents::getScriptBaseName()`.
    pub fn get_script_base_name() -> &'static str {
        "creaturescripts"
    }

    /// Load `data/creaturescripts/creaturescripts.json5`. Returns `true` on
    /// success.
    pub fn load_from_json5(&mut self) -> bool {
        let path = "data/creaturescripts/creaturescripts.json5";
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("CreatureEvents::load_from_json5 - {path} not found: {e}");
                return false;
            }
        };

        let wrapper: CreatureScriptsWrapper = match json5::from_str(&source) {
            Ok(f) => f,
            Err(e) => {
                tracing::error!(
                    "CreatureEvents::load_from_json5 - parse error in {path}: {e}"
                );
                return false;
            }
        };

        for entry in wrapper.creaturescripts.events {
            let event_str = match &entry.event {
                Some(s) => s.as_str(),
                None => {
                    tracing::warn!(
                        "CreatureEvents::load_from_json5 - missing event type for '{}'",
                        entry.name
                    );
                    continue;
                }
            };
            let event_type = match CreatureEventType::from_str(event_str) {
                Some(t) => t,
                None => {
                    tracing::warn!(
                        "CreatureEvents::load_from_json5 - unknown event type '{}' for '{}'",
                        event_str,
                        entry.name
                    );
                    continue;
                }
            };

            if event_type == CreatureEventType::None {
                tracing::warn!(
                    "CreatureEvents::load_from_json5 - event type None for '{}'",
                    entry.name
                );
                continue;
            }

            let mut event = CreatureEvent {
                name: entry.name.clone(),
                event_type,
                script_id: 0,
                scripted: false,
                from_lua: false,
                loaded: true,
            };

            if let Some(ref script_file) = entry.script {
                let script_path =
                    format!("data/creaturescripts/scripts/{script_file}");
                match self.script_interface.load_file(&script_path) {
                    Ok(()) => {
                        let id = self
                            .script_interface
                            .get_event(event.event_type.script_event_name());
                        if id == -1 {
                            tracing::warn!(
                                "CreatureEvents::load_from_json5 - {} not found in {script_path}",
                                event.event_type.script_event_name()
                            );
                            continue;
                        }
                        event.script_id = id;
                        event.scripted = true;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "CreatureEvents::load_from_json5 - cannot load {script_path}: {e}"
                        );
                        continue;
                    }
                }
            }

            // Reuse or insert, matching C++ registerEvent logic.
            if let Some(old) = self.creature_events.get_mut(&entry.name) {
                if !old.loaded && old.event_type == event_type {
                    old.copy_event(&event);
                }
            } else {
                self.creature_events.insert(entry.name, event);
            }
        }

        true
    }

    /// Look up an event by name. If `force_loaded` is `true`, only loaded
    /// events are returned. Mirrors `CreatureEvents::getEventByName()`.
    pub fn get_event_by_name(
        &self,
        name: &str,
        force_loaded: bool,
    ) -> Option<&CreatureEvent> {
        let event = self.creature_events.get(name)?;
        if force_loaded && !event.loaded {
            return None;
        }
        Some(event)
    }

    /// Register a creature event from a Lua script. Mirrors
    /// `CreatureEvents::registerLuaEvent()`.
    pub fn register_lua_event(&mut self, event: CreatureEvent) -> bool {
        if event.event_type == CreatureEventType::None {
            tracing::warn!(
                "CreatureEvents::register_lua_event - event type None for '{}'",
                event.name
            );
            return false;
        }

        if let Some(old) = self.creature_events.get_mut(&event.name) {
            if !old.loaded && old.event_type == event.event_type {
                old.copy_event(&event);
            }
            return false;
        }

        self.creature_events.insert(event.name.clone(), event);
        true
    }

    /// Clear entries matching `from_lua`. Events are cleared but kept so they
    /// can be reloaded. Mirrors `CreatureEvents::clear()`.
    pub fn clear(&mut self, from_lua: bool) {
        for event in self.creature_events.values_mut() {
            if event.from_lua == from_lua {
                event.clear_event();
            }
        }
        if !from_lua {
            if let Err(e) = self.script_interface.re_init_state() {
                tracing::error!("CreatureEvents::clear - re_init_state failed: {e}");
            }
        }
    }

    /// Remove entries whose `script_id` is 0 (never loaded).
    /// Mirrors `CreatureEvents::removeInvalidEvents()`.
    pub fn remove_invalid_events(&mut self) {
        self.creature_events.retain(|_, v| v.script_id != 0);
    }

    /// Retrieve a Lua function for a script ID stored in this interface's event table.
    /// Used by dispatch functions for JSON5-loaded creature events.
    pub fn get_script_function(&self, script_id: i32) -> Option<mlua::Function> {
        self.script_interface.push_function(script_id).ok()
    }

    /// Retrieve a Lua function stored in a from_lua event (via lua_callbacks in ScriptRegistry).
    /// For events registered via `CreatureEvent:register()` in scripts.
    pub fn get_lua_callback_function(&self, lua: &mlua::Lua, script_id: i32) -> Option<mlua::Function> {
        use crate::events::registry::g_script_registry;
        let registry = g_script_registry().lock().ok()?;
        registry.get_callback_function(lua, script_id)
    }

    /// Retrieve a Lua function for a script ID, checking both the event table and lua_callbacks.
    pub fn get_function_for_event(&self, lua: &mlua::Lua, event: &CreatureEvent) -> Option<mlua::Function> {
        if event.from_lua {
            self.get_lua_callback_function(lua, event.script_id)
        } else {
            self.get_script_function(event.script_id)
        }
    }

    /// Iterate all registered events.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &CreatureEvent)> {
        self.creature_events.iter().map(|(k, v)| (k.as_str(), v))
    }
}

impl Default for CreatureEvents {
    fn default() -> Self {
        Self::new()
    }
}
