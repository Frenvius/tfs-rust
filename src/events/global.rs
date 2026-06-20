use std::collections::BTreeMap;

use serde::Deserialize;

use crate::lua::script::LuaScriptInterface;

// ---------------------------------------------------------------------------
// SCHEDULER_MINTICKS — mirrors scheduler.h
// ---------------------------------------------------------------------------

pub const SCHEDULER_MINTICKS: i64 = 50;

// ---------------------------------------------------------------------------
// GlobalEventType
// ---------------------------------------------------------------------------

/// Mirrors `GlobalEvent_t` from `globalevent.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum GlobalEventType {
    None = 0,
    Timer = 1,
    Startup = 2,
    Shutdown = 3,
    Record = 4,
}

impl GlobalEventType {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "startup" | "start" => Some(Self::Startup),
            "shutdown" => Some(Self::Shutdown),
            "record" => Some(Self::Record),
            "timer" => Some(Self::Timer),
            _ => None,
        }
    }

    pub fn script_event_name(self) -> &'static str {
        match self {
            Self::Startup => "onStartup",
            Self::Shutdown => "onShutdown",
            Self::Record => "onRecord",
            Self::Timer => "onTime",
            Self::None => "onThink",
        }
    }
}

// ---------------------------------------------------------------------------
// XML schema
// ---------------------------------------------------------------------------

/// A single entry in `data/globalevents/globalevents.xml`.
#[derive(Debug, Deserialize)]
pub struct GlobalEventEntry {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@script")]
    pub script: Option<String>,
    #[serde(rename = "@type")]
    pub event: Option<String>,
    #[serde(rename = "@time")]
    pub time: Option<String>,
    #[serde(rename = "@interval")]
    pub interval: Option<i64>,
}

/// Top-level document for `data/globalevents/globalevents.xml`.
#[derive(Debug, Deserialize)]
pub struct GlobalEventsXml {
    #[serde(rename = "globalevent", default)]
    pub globalevents: Vec<GlobalEventEntry>,
}

// ---------------------------------------------------------------------------
// Runtime types
// ---------------------------------------------------------------------------

/// Mirrors the C++ `GlobalEvent` class.
#[derive(Debug, Clone)]
pub struct GlobalEvent {
    pub name: String,
    pub event_type: GlobalEventType,
    pub script_id: i32,
    pub next_execution: i64,
    pub interval: u32,
    pub scripted: bool,
    pub from_lua: bool,
}

/// Mirrors the C++ `GlobalEvents` class (inherits `BaseEvents`).
pub struct GlobalEvents {
    script_interface: LuaScriptInterface,
    /// Think events (interval-driven, `GLOBALEVENT_NONE`).
    think_map: BTreeMap<String, GlobalEvent>,
    /// Server-lifecycle events (startup / shutdown / record).
    server_map: BTreeMap<String, GlobalEvent>,
    /// Wall-clock timer events (`GLOBALEVENT_TIMER`).
    timer_map: BTreeMap<String, GlobalEvent>,
    /// Scheduler event ID for the think loop (0 = not scheduled).
    pub think_event_id: i32,
    /// Scheduler event ID for the timer loop (0 = not scheduled).
    pub timer_event_id: i32,
}

impl GlobalEvents {
    /// Mirrors `GlobalEvents::GlobalEvents()`.
    pub fn new() -> Self {
        let mut iface = LuaScriptInterface::new("GlobalEvent Interface");
        if let Err(e) = iface.init_state() {
            tracing::error!("GlobalEvents::new - init_state failed: {e}");
        }
        Self {
            script_interface: iface,
            think_map: BTreeMap::new(),
            server_map: BTreeMap::new(),
            timer_map: BTreeMap::new(),
            think_event_id: 0,
            timer_event_id: 0,
        }
    }

    /// Script base name. Mirrors `GlobalEvents::getScriptBaseName()`.
    pub fn get_script_base_name() -> &'static str {
        "globalevents"
    }

    /// Load `data/globalevents/globalevents.xml`. Returns `true` on success.
    pub fn load_from_xml(&mut self) -> bool {
        let path = "data/globalevents/globalevents.xml";
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("GlobalEvents::load_from_xml - {path} not found: {e}");
                return false;
            }
        };

        let file: GlobalEventsXml = match quick_xml::de::from_str(&source) {
            Ok(f) => f,
            Err(e) => {
                tracing::error!(
                    "GlobalEvents::load_from_xml - parse error in {path}: {e}"
                );
                return false;
            }
        };

        for entry in file.globalevents {
            // Resolve event type and timing fields, mirroring
            // GlobalEvent::configureEvent.
            let (event_type, next_execution, interval) =
                if let Some(ref time_str) = entry.time {
                    match parse_wall_clock_time(time_str) {
                        Some(next) => (GlobalEventType::Timer, next, 0u32),
                        None => {
                            tracing::error!(
                                "GlobalEvents::load_from_xml - invalid time '{}' for '{}'",
                                time_str,
                                entry.name
                            );
                            continue;
                        }
                    }
                } else if let Some(ref type_str) = entry.event {
                    match GlobalEventType::from_str(type_str) {
                        Some(t @ (GlobalEventType::Startup | GlobalEventType::Shutdown | GlobalEventType::Record)) => {
                            (t, 0i64, 0u32)
                        }
                        _ => {
                            tracing::error!(
                                "GlobalEvents::load_from_xml - invalid event type '{}' for '{}'",
                                type_str,
                                entry.name
                            );
                            continue;
                        }
                    }
                } else if let Some(ms) = entry.interval {
                    let clamped = ms.max(SCHEDULER_MINTICKS) as u32;
                    let next = now_ms() + clamped as i64;
                    (GlobalEventType::None, next, clamped)
                } else {
                    tracing::error!(
                        "GlobalEvents::load_from_xml - no time/type/interval for '{}'",
                        entry.name
                    );
                    continue;
                };

            let mut event = GlobalEvent {
                name: entry.name.clone(),
                event_type,
                script_id: 0,
                next_execution,
                interval,
                scripted: false,
                from_lua: false,
            };

            if let Some(ref script_file) = entry.script {
                let script_path =
                    format!("data/globalevents/scripts/{script_file}");
                match self.script_interface.load_file(&script_path) {
                    Ok(()) => {
                        let id = self
                            .script_interface
                            .get_event(event.event_type.script_event_name());
                        if id == -1 {
                            tracing::warn!(
                                "GlobalEvents::load_from_xml - {} not found in {script_path}",
                                event.event_type.script_event_name()
                            );
                            continue;
                        }
                        event.script_id = id;
                        event.scripted = true;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "GlobalEvents::load_from_xml - cannot load {script_path}: {e}"
                        );
                        continue;
                    }
                }
            }

            if !self.register_event_internal(event) {
                tracing::warn!(
                    "GlobalEvents::load_from_xml - duplicate globalevent: '{}'",
                    entry.name
                );
            }
        }

        true
    }

    /// Insert an event into the appropriate map. Returns `false` for duplicates.
    /// Mirrors the body of `GlobalEvents::registerEvent` / `registerLuaEvent`.
    fn register_event_internal(&mut self, event: GlobalEvent) -> bool {
        match event.event_type {
            GlobalEventType::Timer => {
                if let std::collections::btree_map::Entry::Vacant(e) =
                    self.timer_map.entry(event.name.clone())
                {
                    e.insert(event);
                    true
                } else {
                    false
                }
            }
            GlobalEventType::None => {
                if let std::collections::btree_map::Entry::Vacant(e) =
                    self.think_map.entry(event.name.clone())
                {
                    e.insert(event);
                    true
                } else {
                    false
                }
            }
            _ => {
                if let std::collections::btree_map::Entry::Vacant(e) =
                    self.server_map.entry(event.name.clone())
                {
                    e.insert(event);
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Register a global event from a Lua script. Mirrors
    /// `GlobalEvents::registerLuaEvent()`.
    pub fn register_lua_event(&mut self, event: GlobalEvent) -> bool {
        if !self.register_event_internal(event) {
            tracing::warn!(
                "GlobalEvents::register_lua_event - duplicate globalevent"
            );
            return false;
        }
        true
    }

    /// Clear entries matching `from_lua`. Mirrors `GlobalEvents::clear()`.
    pub fn clear(&mut self, from_lua: bool) {
        self.think_map.retain(|_, v| v.from_lua != from_lua);
        self.server_map.retain(|_, v| v.from_lua != from_lua);
        self.timer_map.retain(|_, v| v.from_lua != from_lua);

        if !from_lua {
            if let Err(e) = self.script_interface.re_init_state() {
                tracing::error!("GlobalEvents::clear - re_init_state failed: {e}");
            }
        }
    }

    /// Return a copy of the event map for a given type.
    /// Mirrors `GlobalEvents::getEventMap()`.
    pub fn get_event_map(&self, event_type: GlobalEventType) -> BTreeMap<String, GlobalEvent> {
        match event_type {
            GlobalEventType::None => self.think_map.clone(),
            GlobalEventType::Timer => self.timer_map.clone(),
            GlobalEventType::Startup | GlobalEventType::Shutdown | GlobalEventType::Record => {
                self.server_map
                    .iter()
                    .filter(|(_, v)| v.event_type == event_type)
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            }
        }
    }

    /// Fire all startup events.
    pub fn startup(&self) {
        self.execute(GlobalEventType::Startup);
    }

    /// Execute all server-map events of the given type.
    pub fn execute(&self, event_type: GlobalEventType) {
        for event in self.server_map.values() {
            if event.event_type == event_type {
                self.execute_event(event, 0);
            }
        }
    }

    /// Execute a record event with (current, old) args.
    pub fn execute_record(&self, current: u32, old: u32) {
        for event in self.server_map.values() {
            if event.event_type == GlobalEventType::Record {
                self.execute_record_event(event, current, old);
            }
        }
    }

    /// Advance think-map events and fire those that are due.
    /// Returns the minimum milliseconds until the next event.
    pub fn think(&mut self) -> i64 {
        let now = now_ms();

        let due: Vec<(String, u32, i64)> = self.think_map.values()
            .filter(|e| e.next_execution <= now)
            .map(|e| (e.name.clone(), e.interval, e.next_execution))
            .collect();

        let mut next_scheduled: i64 = i64::MAX;

        for (name, interval, prev_exec) in due {
            let event = match self.think_map.get(&name) {
                Some(e) => e.clone(),
                None => continue,
            };
            if !self.execute_event(&event, interval) {
                tracing::error!("GlobalEvents::think - failed to execute: {name}");
            }
            let next_exec_time = interval as i64;
            if next_exec_time < next_scheduled {
                next_scheduled = next_exec_time;
            }
            if let Some(ev) = self.think_map.get_mut(&name) {
                ev.next_execution = prev_exec + next_exec_time;
            }
        }

        for event in self.think_map.values() {
            let diff = event.next_execution - now;
            if diff > 0 && diff < next_scheduled {
                next_scheduled = diff;
            }
        }

        if next_scheduled == i64::MAX { 0 } else { next_scheduled }
    }

    /// Advance timer-map events and fire those that are due.
    pub fn timer(&mut self) -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let due: Vec<(String, u32, i64)> = self.timer_map.values()
            .filter(|e| e.next_execution <= now)
            .map(|e| (e.name.clone(), e.interval, e.next_execution))
            .collect();

        let mut next_scheduled: i64 = i64::MAX;

        for (name, interval, prev_exec) in due {
            let event = match self.timer_map.get(&name) {
                Some(e) => e.clone(),
                None => continue,
            };
            if !self.execute_event(&event, interval) {
                self.timer_map.remove(&name);
                continue;
            }
            let next_exec_time: i64 = 86400;
            if next_exec_time < next_scheduled {
                next_scheduled = next_exec_time;
            }
            if let Some(ev) = self.timer_map.get_mut(&name) {
                ev.next_execution = prev_exec + next_exec_time;
            }
        }

        for event in self.timer_map.values() {
            let diff = event.next_execution - now;
            if diff > 0 && diff < next_scheduled {
                next_scheduled = diff;
            }
        }

        if next_scheduled == i64::MAX { 0 } else { next_scheduled }
    }

    /// Retrieve a Lua function for an XML-loaded global event script.
    pub fn get_script_function(&self, script_id: i32) -> Option<mlua::Function> {
        self.script_interface.push_function(script_id).ok()
    }

    /// Execute a global event, passing `interval` as the sole arg for think/timer events.
    fn execute_event(&self, event: &GlobalEvent, interval: u32) -> bool {
        use crate::lua::script::{g_lua, ScriptEnvironment};
        let lua = g_lua();

        if !ScriptEnvironment::reserve() {
            tracing::error!("GlobalEvent::executeEvent - Call stack overflow");
            return false;
        }
        ScriptEnvironment::set_script_id(event.script_id, "GlobalEvent Interface");

        let result = (|| -> mlua::Result<bool> {
            let func = if event.from_lua {
                use crate::events::registry::g_script_registry;
                let reg = g_script_registry().lock().unwrap();
                reg.get_callback_function(lua, event.script_id)
            } else {
                self.get_script_function(event.script_id)
            };
            let Some(func) = func else { return Ok(true); };

            let needs_interval = event.event_type == GlobalEventType::None
                || event.event_type == GlobalEventType::Timer;

            let res = if needs_interval {
                func.call::<bool>(interval as i64)
            } else {
                func.call::<bool>(())
            };

            match res {
                Ok(v) => Ok(v),
                Err(e) => { tracing::error!("Lua GlobalEvent {} error: {e}", event.name); Ok(false) }
            }
        })();

        ScriptEnvironment::reset();
        result.unwrap_or(false)
    }

    /// Execute a record event with `(current, old)` args.
    fn execute_record_event(&self, event: &GlobalEvent, current: u32, old: u32) {
        use crate::lua::script::{g_lua, ScriptEnvironment};
        let lua = g_lua();

        if !ScriptEnvironment::reserve() {
            tracing::error!("GlobalEvent::executeRecord - Call stack overflow");
            return;
        }
        ScriptEnvironment::set_script_id(event.script_id, "GlobalEvent Interface");

        let _ = (|| -> mlua::Result<()> {
            let func = if event.from_lua {
                use crate::events::registry::g_script_registry;
                let reg = g_script_registry().lock().unwrap();
                reg.get_callback_function(lua, event.script_id)
            } else {
                self.get_script_function(event.script_id)
            };
            let Some(func) = func else { return Ok(()); };
            match func.call::<bool>((current as i64, old as i64)) {
                Ok(_) => Ok(()),
                Err(e) => { tracing::error!("Lua GlobalEvent record error: {e}"); Ok(()) }
            }
        })();

        ScriptEnvironment::reset();
    }

    /// Iterate all think events.
    pub fn iter_think(&self) -> impl Iterator<Item = (&str, &GlobalEvent)> {
        self.think_map.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Iterate all server events.
    pub fn iter_server(&self) -> impl Iterator<Item = (&str, &GlobalEvent)> {
        self.server_map.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Iterate all timer events.
    pub fn iter_timer(&self) -> impl Iterator<Item = (&str, &GlobalEvent)> {
        self.timer_map.iter().map(|(k, v)| (k.as_str(), v))
    }
}

impl Default for GlobalEvents {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return current time in milliseconds since the Unix epoch.
fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Parse `"HH:MM"` or `"HH:MM:SS"` into a Unix timestamp (seconds) for the
/// next occurrence of that wall-clock time.
///
/// Mirrors the `configureEvent` logic: compute `mktime` for today at the given
/// H/M/S, then add 86 400 if the result is in the past.
fn parse_wall_clock_time(s: &str) -> Option<i64> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() < 2 {
        return None;
    }
    let hour: u8 = parts[0].trim().parse().ok()?;
    let min: u8 = parts[1].trim().parse().ok()?;
    let sec: u8 = if parts.len() > 2 {
        parts[2].trim().parse().ok()?
    } else {
        0
    };

    if hour > 23 || min > 59 || sec > 59 {
        return None;
    }

    use std::time::{SystemTime, UNIX_EPOCH};
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // Compute seconds-since-midnight for the target time.
    let target_sod = (hour as i64) * 3600 + (min as i64) * 60 + (sec as i64);
    // Seconds-since-midnight for right now.
    let now_sod = now_secs % 86400;

    let mut diff = target_sod - now_sod;
    if diff < 0 {
        diff += 86400;
    }

    Some(now_secs + diff)
}
