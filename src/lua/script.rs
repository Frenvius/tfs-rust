use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use mlua::prelude::*;

use crate::map::Position;

use super::registrations;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const EVENT_ID_LOADING: i32 = 1;
pub const EVENT_ID_USER: i32 = 1000;

// ---------------------------------------------------------------------------
// LuaDataType
// ---------------------------------------------------------------------------

#[allow(dead_code)]
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LuaDataType {
    Unknown = 0,
    Item = 1,
    Container = 2,
    Teleport = 3,
    Player = 4,
    Monster = 5,
    Npc = 6,
    Tile = 7,
}

// ---------------------------------------------------------------------------
// LuaVariantType
// ---------------------------------------------------------------------------

#[allow(dead_code)]
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LuaVariantType {
    #[default]
    None = 0,
    Number = 1,
    Position = 2,
    TargetPosition = 3,
    String = 4,
}

// ---------------------------------------------------------------------------
// LuaVariant
// ---------------------------------------------------------------------------

#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct LuaVariant {
    pub variant_type: LuaVariantType,
    pub text: String,
    pub pos: Position,
    pub number: u32,
}

// ---------------------------------------------------------------------------
// LuaTimerEventDesc
// ---------------------------------------------------------------------------

#[allow(dead_code)]
#[derive(Debug)]
pub struct LuaTimerEventDesc {
    pub script_id: i32,
    /// Lua registry integer ref for the callback function.
    pub function_ref: i32,
    /// Lua registry integer refs for each parameter, in call order.
    pub parameters: Vec<i32>,
    pub event_id: u32,
}

// ---------------------------------------------------------------------------
// ScriptEnvironment – thread-local call-stack frames
// ---------------------------------------------------------------------------

/// Per-call frame pushed onto the thread-local stack when a Lua event fires.
#[derive(Debug, Default, Clone)]
pub struct ScriptEnvFrame {
    pub script_id: i32,
    pub callback_id: i32,
    pub timer_event: bool,
    pub interface_name: String,
    pub loading_file: String,
}

thread_local! {
    static SCRIPT_ENV_STACK: RefCell<Vec<ScriptEnvFrame>> =
        RefCell::new(Vec::with_capacity(16));
    static CURRENT_NPC_ID: Cell<u32> = const { Cell::new(0) };
}

/// Set the NPC creature ID active for the current Lua script call.
/// Mirrors `ScriptEnvironment::setNpc(npc)` in C++.
pub fn set_current_npc(id: u32) {
    CURRENT_NPC_ID.with(|c| c.set(id));
}

/// Return the NPC creature ID active for the current Lua script call, or 0.
/// Mirrors `ScriptEnvironment::getNpc()` in C++.
pub fn get_current_npc() -> u32 {
    CURRENT_NPC_ID.with(|c| c.get())
}

/// Static helpers that mirror the C++ `LuaScriptInterface::scriptEnv` array
/// with its `scriptEnvIndex` counter.
pub struct ScriptEnvironment;

impl ScriptEnvironment {
    /// Push a blank frame.  Returns `false` when the 16-frame hard limit is
    /// reached, matching the C++ overflow guard.
    pub fn reserve() -> bool {
        SCRIPT_ENV_STACK.with(|s| {
            let mut stack = s.borrow_mut();
            if stack.len() >= 16 {
                return false;
            }
            stack.push(ScriptEnvFrame::default());
            true
        })
    }

    /// Pop the top frame, discarding its contents.  Mirrors `resetEnv()`.
    pub fn reset() {
        SCRIPT_ENV_STACK.with(|s| {
            s.borrow_mut().pop();
        });
    }

    /// Set `script_id` and `interface_name` on the current (top) frame.
    pub fn set_script_id(script_id: i32, interface_name: &str) {
        SCRIPT_ENV_STACK.with(|s| {
            if let Some(frame) = s.borrow_mut().last_mut() {
                frame.script_id = script_id;
                frame.interface_name = interface_name.to_owned();
            }
        });
    }

    /// Mark the current frame as executing inside a timer event.
    pub fn set_timer_event() {
        SCRIPT_ENV_STACK.with(|s| {
            if let Some(frame) = s.borrow_mut().last_mut() {
                frame.timer_event = true;
            }
        });
    }

    /// Set `callback_id` on the current frame.  Returns `false` when a
    /// callback is already set (nested callbacks are forbidden, matching
    /// the C++ guard in `ScriptEnvironment::setCallbackId`).
    pub fn set_callback_id(callback_id: i32, interface_name: &str) -> bool {
        SCRIPT_ENV_STACK.with(|s| {
            let mut stack = s.borrow_mut();
            match stack.last_mut() {
                Some(frame) if frame.callback_id == 0 => {
                    frame.callback_id = callback_id;
                    frame.interface_name = interface_name.to_owned();
                    true
                }
                Some(_) => false,
                None => false,
            }
        })
    }

    /// Set `loading_file` on the current frame.
    pub fn set_loading_file(path: &str) {
        SCRIPT_ENV_STACK.with(|s| {
            if let Some(frame) = s.borrow_mut().last_mut() {
                frame.loading_file = path.to_owned();
            }
        });
    }

    /// Return `(script_id, interface_name, callback_id, timer_event)` for the
    /// top frame, or `None` when the stack is empty.
    pub fn get_event_info() -> Option<(i32, String, i32, bool)> {
        SCRIPT_ENV_STACK.with(|s| {
            s.borrow().last().map(|f| {
                (
                    f.script_id,
                    f.interface_name.clone(),
                    f.callback_id,
                    f.timer_event,
                )
            })
        })
    }
}

// ---------------------------------------------------------------------------
// LuaScriptInterface
// ---------------------------------------------------------------------------

/// Per-subsystem Lua interface.  Each event subsystem (Weapons, Spells, etc.)
/// owns one of these.  The underlying `mlua::Lua` state lives in
/// `LuaEnvironment`; this struct holds only the per-interface event table
/// registry key and associated metadata.
pub struct LuaScriptInterface {
    pub interface_name: String,
    event_table_ref: Option<LuaRegistryKey>,
    running_event_id: i32,
    cache_files: HashMap<i32, String>,
    pub loading_file: String,
    pub last_lua_error: String,
}

impl std::fmt::Debug for LuaScriptInterface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LuaScriptInterface")
            .field("interface_name", &self.interface_name)
            .finish_non_exhaustive()
    }
}

impl LuaScriptInterface {
    /// Create a new interface.  Does NOT call `init_state`; callers must do
    /// so before loading files or registering events — unless `g_lua_env()` is
    /// already initialized, in which case the event table is created lazily on
    /// first use.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            interface_name: name.into(),
            event_table_ref: None,
            running_event_id: EVENT_ID_USER,
            cache_files: HashMap::new(),
            loading_file: String::new(),
            last_lua_error: String::new(),
        }
    }

    /// Return the interface name.  Mirrors `getInterfaceName()`.
    pub fn interface_name(&self) -> &str {
        &self.interface_name
    }

    /// Return the last Lua error string.  Mirrors `getLastLuaError()`.
    pub fn last_lua_error(&self) -> &str {
        &self.last_lua_error
    }

    /// Create the per-interface event table in the Lua registry.
    /// Mirrors `LuaScriptInterface::initState()`.
    pub fn init_state(&mut self) -> LuaResult<()> {
        let lua = g_lua();
        let table = lua.create_table()?;
        let key = lua.create_registry_value(table)?;
        self.event_table_ref = Some(key);
        self.running_event_id = EVENT_ID_USER;
        Ok(())
    }

    /// Release the event table registry entry.
    /// Mirrors `LuaScriptInterface::closeState()`.
    pub fn close_state(&mut self) {
        self.cache_files.clear();
        self.event_table_ref = None;
    }

    /// Close and re-open the event table (used on reload).
    /// Mirrors `LuaScriptInterface::reInitState()`.
    pub fn re_init_state(&mut self) -> LuaResult<()> {
        g_lua_env().clear_combat_objects(&self.interface_name);
        g_lua_env().clear_area_objects(&self.interface_name);
        self.close_state();
        self.init_state()
    }

    /// Alias for `re_init_state` matching the C++ camelCase name.
    /// Used by event subsystem `clear()` methods.
    pub fn reinit_state(&mut self) {
        if let Err(e) = self.re_init_state() {
            tracing::error!(
                "[{}] reinit_state failed: {}",
                self.interface_name,
                e
            );
        }
    }

    /// Ensure the event table exists, creating it lazily if needed.
    fn ensure_event_table(&mut self) -> LuaResult<()> {
        if self.event_table_ref.is_none() {
            self.init_state()?;
        }
        Ok(())
    }

    /// Execute a Lua source file in the shared state.
    ///
    /// Sets `loading_file`, pushes/pops a `ScriptEnvFrame`, and mirrors the
    /// C++ `LuaScriptInterface::loadFile` logic.
    pub fn load_file(&mut self, path: &str) -> LuaResult<()> {
        let lua = g_lua();

        let source = std::fs::read_to_string(path).map_err(|e| {
            let msg = format!("cannot open '{}': {}", path, e);
            self.last_lua_error = msg.clone();
            LuaError::RuntimeError(msg)
        })?;

        self.loading_file = path.to_owned();

        if !ScriptEnvironment::reserve() {
            self.loading_file.clear();
            let msg = "Script call stack overflow".to_owned();
            self.last_lua_error = msg.clone();
            return Err(LuaError::RuntimeError(msg));
        }

        ScriptEnvironment::set_script_id(EVENT_ID_LOADING, &self.interface_name);
        ScriptEnvironment::set_loading_file(path);

        let result = lua.load(&source).set_name(path).exec();

        ScriptEnvironment::reset();

        match &result {
            Err(e) => {
                self.last_lua_error = e.to_string();
                Self::report_error(None, &self.last_lua_error);
            }
            Ok(()) => {
                self.last_lua_error.clear();
            }
        }

        self.loading_file.clear();
        result
    }

    /// Look up a global function by `event_name`, store it in the event table,
    /// nil-out the global, and return its stable integer ID.
    ///
    /// Returns `-1` when the global is absent or not a function.
    /// Mirrors `LuaScriptInterface::getEvent(name)`.
    pub fn get_event(&mut self, event_name: &str) -> i32 {
        if let Err(e) = self.ensure_event_table() {
            tracing::error!("[{}] get_event: failed to init event table: {}", self.interface_name, e);
            return -1;
        }
        match self.get_event_inner(event_name) {
            Ok(id) => id,
            Err(e) => {
                tracing::error!("[{}] get_event({}): {}", self.interface_name, event_name, e);
                -1
            }
        }
    }

    fn get_event_inner(&mut self, event_name: &str) -> LuaResult<i32> {
        let lua = g_lua();
        let table = self.event_table().ok_or_else(|| {
            LuaError::RuntimeError("event table not initialized".into())
        })?;

        let func: Option<LuaFunction> = lua.globals().get(event_name)?;
        let func = match func {
            Some(f) => f,
            None => return Ok(-1),
        };

        let id = self.running_event_id;
        table.raw_set(id, func)?;
        lua.globals().set(event_name, LuaValue::Nil)?;

        self.cache_files
            .insert(id, format!("{}:{}", self.loading_file, event_name));
        self.running_event_id += 1;
        Ok(id)
    }

    /// Store an explicitly provided function in the event table and return its
    /// ID.  For callers inside Lua callbacks that hold the function value.
    pub fn store_function(&mut self, func: LuaFunction) -> i32 {
        if let Err(e) = self.ensure_event_table() {
            tracing::error!("[{}] store_function: failed to init event table: {}", self.interface_name, e);
            return -1;
        }
        let table = match self.event_table() {
            Some(t) => t,
            None => return -1,
        };
        let id = self.running_event_id;
        if let Err(e) = table.raw_set(id, func) {
            tracing::error!("[{}] store_function: {}", self.interface_name, e);
            return -1;
        }
        self.cache_files
            .insert(id, format!("{}:callback", self.loading_file));
        self.running_event_id += 1;
        id
    }

    /// Retrieve `global_name.event_name`, store it in the event table,
    /// nil-out the field, and return the ID.  Returns `-1` when not found.
    /// Mirrors `LuaScriptInterface::getMetaEvent(globalName, eventName)`.
    pub fn get_meta_event(&mut self, global_name: &str, event_name: &str) -> i32 {
        if let Err(e) = self.ensure_event_table() {
            tracing::error!("[{}] get_meta_event: failed to init event table: {}", self.interface_name, e);
            return -1;
        }
        match self.get_meta_event_inner(global_name, event_name) {
            Ok(id) => id,
            Err(e) => {
                tracing::error!(
                    "[{}] get_meta_event({}.{}): {}",
                    self.interface_name, global_name, event_name, e
                );
                -1
            }
        }
    }

    fn get_meta_event_inner(
        &mut self,
        global_name: &str,
        event_name: &str,
    ) -> LuaResult<i32> {
        let lua = g_lua();
        let table = self.event_table().ok_or_else(|| {
            LuaError::RuntimeError("event table not initialized".into())
        })?;

        let global_tbl: Option<LuaTable> = lua.globals().get(global_name)?;
        let global_tbl = match global_tbl {
            Some(t) => t,
            None => return Ok(-1),
        };

        let func: Option<LuaFunction> = global_tbl.get(event_name)?;
        let func = match func {
            Some(f) => f,
            None => return Ok(-1),
        };

        let id = self.running_event_id;
        table.raw_set(id, func)?;
        global_tbl.set(event_name, LuaValue::Nil)?;

        self.cache_files.insert(
            id,
            format!("{}:{}@{}", self.loading_file, global_name, event_name),
        );
        self.running_event_id += 1;
        Ok(id)
    }

    /// Return the source-location string recorded for `script_id`.
    /// Mirrors `LuaScriptInterface::getFileById(id)`.
    pub fn get_file_by_id(&self, script_id: i32) -> &str {
        if script_id == EVENT_ID_LOADING {
            return &self.loading_file;
        }
        self.cache_files
            .get(&script_id)
            .map(String::as_str)
            .unwrap_or("(Unknown scriptfile)")
    }

    /// Log a Lua error via `tracing`, annotated with the current
    /// `ScriptEnvFrame` context.  Mirrors the C++ `reportError` static.
    pub fn report_error(function: Option<&str>, error: &str) {
        let prefix = function
            .map(|f| format!("{}(): ", f))
            .unwrap_or_default();

        if let Some((script_id, iface, callback_id, timer_event)) =
            ScriptEnvironment::get_event_info()
        {
            let location = if timer_event {
                format!(" (timer, script={}, callback={})", script_id, callback_id)
            } else {
                format!(" (script={}, callback={})", script_id, callback_id)
            };
            tracing::error!("[{}] Lua Script Error{}: {}{}", iface, location, prefix, error);
        } else {
            tracing::error!("Lua Script Error: {}{}", prefix, error);
        }
    }

    /// Retrieve a `LuaFunction` from the event table by its integer ID.
    /// Mirrors `LuaScriptInterface::pushFunction(id)`.
    pub fn push_function(&self, function_id: i32) -> LuaResult<LuaFunction> {
        let table = self.event_table().ok_or_else(|| {
            LuaError::RuntimeError("event table not initialized".into())
        })?;
        table.raw_get::<LuaFunction>(function_id).map_err(|_| {
            LuaError::RuntimeError(format!("no function at event id {}", function_id))
        })
    }

    /// Call a Lua function and return the first boolean return value.
    /// Reports errors via `report_error` and returns `false` on failure.
    /// Mirrors `LuaScriptInterface::callFunction(params)`.
    pub fn call_function<A: IntoLuaMulti>(
        &self,
        func: LuaFunction,
        args: A,
    ) -> LuaResult<bool> {
        match func.call::<bool>(args) {
            Ok(v) => Ok(v),
            Err(e) => {
                Self::report_error(None, &e.to_string());
                Ok(false)
            }
        }
    }

    /// Call a Lua function, discarding all return values.
    /// Mirrors `LuaScriptInterface::callVoidFunction(params)`.
    pub fn call_void_function<A: IntoLuaMulti>(
        &self,
        func: LuaFunction,
        args: A,
    ) -> LuaResult<()> {
        func.call::<()>(args).map_err(|e| {
            Self::report_error(None, &e.to_string());
            e
        })
    }

    /// Expose the underlying Lua VM.  Used by Phase 6 binding work.
    pub fn lua(&self) -> &Lua {
        g_lua()
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    fn event_table(&self) -> Option<LuaTable> {
        let lua = g_lua();
        self.event_table_ref
            .as_ref()
            .and_then(|key| lua.registry_value::<LuaTable>(key).ok())
    }
}

// ---------------------------------------------------------------------------
// LuaEnvironment  – owns the mlua::Lua state and global timer/combat maps
// ---------------------------------------------------------------------------

/// Global owner of the Lua VM and all server-wide Lua resources.
/// Mirrors `LuaEnvironment` from `luascript.h`.
pub struct LuaEnvironment {
    lua: Lua,
    timer_events: Mutex<HashMap<u32, LuaTimerEventDesc>>,
    last_event_timer_id: AtomicU32,
    last_combat_id: AtomicU32,
    last_area_id: AtomicU32,
    test_interface: Mutex<Option<LuaScriptInterface>>,
    /// Per-interface lists of allocated combat IDs, keyed by interface name.
    combat_id_map: Mutex<HashMap<String, Vec<u32>>>,
    /// Per-interface lists of allocated area IDs, keyed by interface name.
    area_id_map: Mutex<HashMap<String, Vec<u32>>>,
    // Deferred: combatMap (Mutex<HashMap<u32, Arc<Combat>>>) and areaMap (Mutex<HashMap<u32, AreaCombat>>)
    // for timer-based combat/area object lifecycle. Not needed for current spell/combat flow.
}

impl std::fmt::Debug for LuaEnvironment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LuaEnvironment").finish_non_exhaustive()
    }
}

impl LuaEnvironment {
    /// Create the global Lua state and register all API tables via
    /// `registrations::register_all`.
    pub fn new() -> LuaResult<Self> {
        let lua = Lua::new();
        registrations::register_all(&lua)?;
        Ok(Self {
            lua,
            timer_events: Mutex::new(HashMap::new()),
            last_event_timer_id: AtomicU32::new(1),
            last_combat_id: AtomicU32::new(0),
            last_area_id: AtomicU32::new(0),
            test_interface: Mutex::new(None),
            combat_id_map: Mutex::new(HashMap::new()),
            area_id_map: Mutex::new(HashMap::new()),
        })
    }

    /// Return a reference to the underlying Lua state.
    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    /// Load and execute a Lua source file in the shared state.
    /// Used by `ScriptingManager` for `data/global.lua`.
    pub fn load_file(&self, path: &str) -> LuaResult<()> {
        let source = std::fs::read_to_string(path).map_err(|e| {
            LuaError::RuntimeError(format!("cannot open '{}': {}", path, e))
        })?;

        if !ScriptEnvironment::reserve() {
            return Err(LuaError::RuntimeError(
                "Script call stack overflow".into(),
            ));
        }

        ScriptEnvironment::set_script_id(EVENT_ID_LOADING, "Main Interface");
        ScriptEnvironment::set_loading_file(path);

        let result = self.lua.load(&source).set_name(path).exec();

        ScriptEnvironment::reset();

        if let Err(ref e) = result {
            LuaScriptInterface::report_error(None, &e.to_string());
        }
        result
    }

    // ------------------------------------------------------------------
    // Test interface (mirrors C++ getTestInterface())
    // ------------------------------------------------------------------

    /// Call `f` with (or after lazily creating) the "Test Interface".
    pub fn with_test_interface<F, R>(&self, f: F) -> LuaResult<R>
    where
        F: FnOnce(&mut LuaScriptInterface) -> LuaResult<R>,
    {
        let mut guard = self
            .test_interface
            .lock()
            .expect("test_interface mutex poisoned");
        if guard.is_none() {
            let mut iface = LuaScriptInterface::new("Test Interface");
            iface.init_state()?;
            *guard = Some(iface);
        }
        f(guard.as_mut().expect("just set above"))
    }

    // ------------------------------------------------------------------
    // Timer events
    // ------------------------------------------------------------------

    /// Register a timer descriptor and return its assigned event ID.
    pub fn add_timer_event(&self, mut desc: LuaTimerEventDesc) -> u32 {
        let id = self.last_event_timer_id.fetch_add(1, Ordering::Relaxed);
        desc.event_id = id;
        self.timer_events
            .lock()
            .expect("timer_events mutex poisoned")
            .insert(id, desc);
        id
    }

    /// Remove a timer descriptor by event ID.
    /// The real timer execution path is in `registrations::execute_lua_timer_event`,
    /// which uses `LuaRegistryKey`-based storage. This method exists for the
    /// legacy `LuaTimerEventDesc` (i32-ref based) map only.
    pub fn remove_timer_event(&self, event_id: u32) -> Option<LuaTimerEventDesc> {
        self.timer_events
            .lock()
            .expect("timer_events mutex poisoned")
            .remove(&event_id)
    }

    // ------------------------------------------------------------------
    // Combat / area object lifetime management
    // ------------------------------------------------------------------

    /// Release all combat objects owned by the named interface.
    /// Mirrors `LuaEnvironment::clearCombatObjects`.
    pub fn clear_combat_objects(&self, interface_name: &str) {
        self.combat_id_map
            .lock()
            .expect("combat_id_map mutex poisoned")
            .remove(interface_name);
        // Deferred: erase the Arc<Combat> entries from combat_map when it exists.
    }

    /// Release all area objects owned by the named interface.
    /// Mirrors `LuaEnvironment::clearAreaObjects`.
    pub fn clear_area_objects(&self, interface_name: &str) {
        self.area_id_map
            .lock()
            .expect("area_id_map mutex poisoned")
            .remove(interface_name);
        // Deferred: erase the AreaCombat entries from area_map when it exists.
    }

    /// Allocate a new area-object slot and return its ID.
    /// Mirrors `LuaEnvironment::createAreaObject`.
    pub fn create_area_object(&self, interface_name: &str) -> u32 {
        let id = self.last_area_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.area_id_map
            .lock()
            .expect("area_id_map mutex poisoned")
            .entry(interface_name.to_owned())
            .or_default()
            .push(id);
        // Deferred: allocate an AreaCombat and store it in area_map when it exists.
        id
    }

    /// Allocate a new combat-object slot and return its ID.
    /// Mirrors `LuaEnvironment::createCombatObject`.
    pub fn create_combat_object(&self, interface_name: &str) -> u32 {
        let id = self.last_combat_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.combat_id_map
            .lock()
            .expect("combat_id_map mutex poisoned")
            .entry(interface_name.to_owned())
            .or_default()
            .push(id);
        // Deferred: allocate a Combat and store it in combat_map when it exists.
        id
    }
}

// ---------------------------------------------------------------------------
// Global singleton  (mirrors `LuaEnvironment g_luaEnvironment` in C++)
// ---------------------------------------------------------------------------

static G_LUA_ENV: OnceLock<LuaEnvironment> = OnceLock::new();

/// Return the global `LuaEnvironment`.  Panics if `init_lua_env` has not
/// been called.
pub fn g_lua_env() -> &'static LuaEnvironment {
    G_LUA_ENV.get().expect("LuaEnvironment not initialized")
}

/// Return the shared `mlua::Lua` state.  Panics if `init_lua_env` has not
/// been called.
pub fn g_lua() -> &'static Lua {
    g_lua_env().lua()
}

/// Initialize the global `LuaEnvironment`.  Must be called exactly once
/// during server boot, before any Lua files are loaded.
pub fn init_lua_env() -> LuaResult<()> {
    let env = LuaEnvironment::new()?;
    G_LUA_ENV.set(env).map_err(|_| {
        LuaError::RuntimeError("LuaEnvironment already initialized".into())
    })
}

// ---------------------------------------------------------------------------
// NPC script interface  (mirrors NpcScriptInterface in C++)
// ---------------------------------------------------------------------------

pub static G_NPC_IFACE: OnceLock<Mutex<LuaScriptInterface>> = OnceLock::new();

pub fn g_npc_iface() -> &'static Mutex<LuaScriptInterface> {
    G_NPC_IFACE.get().expect("NPC script interface not initialized")
}

/// Return the NPC script interface if initialized, or `None`.
pub fn g_npc_iface_opt() -> Option<&'static Mutex<LuaScriptInterface>> {
    G_NPC_IFACE.get()
}

/// Load the NPC lib (`data/npc/lib/npc.lua`) and initialize the NPC script
/// interface.  Must be called once after `init_lua_env`.
pub fn init_npc_script_interface() {
    let mut iface = LuaScriptInterface::new("Npc interface");
    if let Err(e) = iface.init_state() {
        tracing::warn!("[NPC] init_state failed: {}", e);
        return;
    }
    let lib_path = "data/npc/lib/npc.lua";
    if std::path::Path::new(lib_path).exists() {
        if let Err(e) = iface.load_file(lib_path) {
            tracing::warn!("[NPC] Cannot load npc lib {}: {}", lib_path, e);
        }
    }
    G_NPC_IFACE.set(Mutex::new(iface)).ok();
}

/// Load the script for a single NPC type by name and store the event IDs back
/// into `g_npcs()`.  Called once per NPC type that has a `script` attribute.
///
/// Safe to call after `init_npc_script_interface`.
pub fn load_npc_type_script(type_name: &str, script_file: &str) -> (i32, i32, i32, i32, i32, i32, i32) {
    let iface_lock = match G_NPC_IFACE.get() {
        Some(l) => l,
        None => return (-1, -1, -1, -1, -1, -1, -1),
    };
    let mut iface = iface_lock.lock().expect("NPC iface lock poisoned");
    let path = format!("data/npc/scripts/{}", script_file);
    if let Err(e) = iface.load_file(&path) {
        tracing::warn!("[NPC] Cannot load script for '{}' ({}): {}", type_name, script_file, e);
        return (-1, -1, -1, -1, -1, -1, -1);
    }
    let say = iface.get_event("onCreatureSay");
    let think = iface.get_event("onThink");
    let appear = iface.get_event("onCreatureAppear");
    let disappear = iface.get_event("onCreatureDisappear");
    let close_ch = iface.get_event("onPlayerCloseChannel");
    let end_trade = iface.get_event("onPlayerEndTrade");
    let creature_move = iface.get_event("onCreatureMove");
    (say, think, appear, disappear, close_ch, end_trade, creature_move)
}

// ---------------------------------------------------------------------------
// Scripts  (script.h / script.cpp)
// ---------------------------------------------------------------------------

/// Manages the "Scripts Interface" that walks `data/scripts/` and loads every
/// `.lua` file.  Mirrors `Scripts` from `script.h`.
pub struct Scripts {
    pub script_interface: LuaScriptInterface,
}

impl Scripts {
    pub fn new() -> LuaResult<Self> {
        let mut iface = LuaScriptInterface::new("Scripts Interface");
        iface.init_state()?;
        Ok(Self {
            script_interface: iface,
        })
    }

    /// Walk `data/{folder_name}` recursively and load every `.lua` file.
    ///
    /// Rules matching the C++ `loadScripts` logic:
    /// - Files whose name starts with `#` are logged as disabled and skipped.
    /// - The `lib/` subdirectory is skipped unless `is_lib` is `true`.
    /// - The `events/` subdirectory is always skipped.
    /// - Files are sorted alphabetically before loading.
    ///
    /// Returns `true` if the directory existed and was processed without a
    /// hard abort.
    pub fn load_scripts(
        &mut self,
        folder_name: &str,
        is_lib: bool,
        reload: bool,
    ) -> bool {
        let base = Path::new("data").join(folder_name);
        if !base.is_dir() {
            tracing::warn!(
                "[Scripts::load_scripts] Cannot load folder '{}'",
                folder_name
            );
            return false;
        }

        let mut paths: Vec<std::path::PathBuf> = Vec::new();
        collect_lua_files(&base, is_lib, &mut paths);
        paths.sort();

        let mut last_parent = String::new();
        for path in &paths {
            let path_str = match path.to_str() {
                Some(s) => s,
                None => continue,
            };

            if !is_lib {
                let parent = path
                    .parent()
                    .and_then(|p| p.to_str())
                    .unwrap_or("")
                    .to_owned();
                if parent != last_parent {
                    if let Some(dir_name) = path
                        .parent()
                        .and_then(|p| p.file_name())
                        .and_then(|n| n.to_str())
                    {
                        tracing::debug!(">> [{}]", dir_name);
                    }
                    last_parent = parent;
                }
            }

            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(path_str);

            match self.script_interface.load_file(path_str) {
                Ok(()) => {
                    if reload {
                        tracing::debug!("> {} [reloaded]", file_name);
                    } else {
                        tracing::debug!("> {} [loaded]", file_name);
                    }
                }
                Err(_) => {
                    tracing::warn!("> {} [error]", file_name);
                    tracing::warn!("^ {}", self.script_interface.last_lua_error);
                }
            }
        }

        true
    }
}

/// Recursively collect `.lua` files from `dir`, honouring the `lib/` and
/// `events/` skip rules.
fn collect_lua_files(
    dir: &Path,
    is_lib: bool,
    out: &mut Vec<std::path::PathBuf>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if dir_name == "events" {
                continue;
            }
            if dir_name == "lib" && !is_lib {
                continue;
            }
            collect_lua_files(&path, is_lib, out);
        } else if path.is_file() {
            if path.extension().and_then(|e| e.to_str()) != Some("lua") {
                continue;
            }
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if file_name.starts_with('#') {
                tracing::debug!("> {} [disabled]", file_name);
                continue;
            }
            out.push(path);
        }
    }
}

// ---------------------------------------------------------------------------
// ScriptingManager  (scriptmanager.h / scriptmanager.cpp)
// ---------------------------------------------------------------------------

/// Orchestrates the loading of every event subsystem in the order required by
/// `otserv.cpp`.  Mirrors `ScriptingManager::loadScriptSystems()`.
pub struct ScriptingManager;

impl ScriptingManager {
    /// Load all script systems.
    ///
    /// Subsystem failures log a warning and continue rather than aborting,
    /// because XML data files may not exist for all subsystems yet.
    pub fn load_script_systems() -> anyhow::Result<()> {
        use crate::events::registry::init_script_registry;
        init_script_registry();

        // 1. data/global.lua
        if let Err(e) = g_lua_env().load_file("data/global.lua") {
            tracing::warn!(
                "[ScriptingManager] Cannot load data/global.lua: {}",
                e
            );
        }

        // 2. Lua lib scripts
        let mut scripts = Scripts::new()
            .map_err(|e| anyhow::anyhow!("Failed to create Scripts interface: {}", e))?;
        tracing::debug!(">> Loading lua libs");
        if !scripts.load_scripts("scripts/lib", true, false) {
            return Err(anyhow::anyhow!("Unable to load lua libs"));
        }

        // 3. Load XML-backed event subsystems (matches C++ scriptmanager.cpp order)
        {
            let mut registry = crate::events::registry::g_script_registry().lock().unwrap();
            registry.actions.load_from_xml();
            registry.talk_actions.load_from_xml();
            registry.move_events.load_from_xml();
            registry.creature_events.load_from_xml();
            registry.global_events.load_from_xml();
        }

        // 4. Chat channels
        tracing::debug!(">> Loading chat channels");
        {
            let mut chat = crate::chat::g_chat().lock().unwrap();
            chat.load();
        }

        // 5. Load all self-registering scripts from data/scripts/
        // This matches C++ otserv.cpp:249: g_scripts->loadScripts("scripts", false, false)
        tracing::debug!(">> Loading scripts");
        if !scripts.load_scripts("scripts", false, false) {
            tracing::warn!("[ScriptingManager] Warning: some scripts failed to load");
        }

        // 5c. Load spells from data/spells/spells.xml
        print!(">> Loading spells... ");
        crate::events::spells::load_spells();

        // 5b. Load Events (data/events/events.xml + data/events/scripts/*.lua)
        // Must come after scripts/lib (which defines hasEventCallback, EventCallback)
        {
            let mut events = crate::events::g_events().lock().unwrap();
            events.load();
        }

        // 6. Load monster Lua scripts from data/monster/
        tracing::debug!(">> Loading lua monsters");
        if !scripts.load_scripts("monster", false, false) {
            tracing::warn!("[ScriptingManager] Warning: some monster scripts failed to load");
        }

        // 7. Initialize NPC script interface and load per-NPC scripts.
        tracing::debug!(">> Loading npc scripts");
        init_npc_script_interface();
        {
            let type_names: Vec<(String, String)> = {
                let npcs = crate::creatures::npc::g_npcs();
                npcs.npc_types.iter()
                    .filter(|(_, nt)| !nt.script_file.is_empty())
                    .map(|(k, nt)| (k.clone(), nt.script_file.clone()))
                    .collect()
            };
            // Load scripts once per type (NPC types are immutable after init).
            // We store event IDs inside a thread-safe replacement of the OnceLock:
            // Since g_npcs() returns &'static, we can't mutate it directly.
            // Instead store results in a temporary map and pass to a one-time setter.
            #[allow(clippy::type_complexity)]
            let mut results: Vec<(String, i32, i32, i32, i32, i32, i32, i32)> = Vec::new();
            for (type_name, script_file) in type_names {
                let (say, think, appear, disappear, close_ch, end_trade, creature_move) =
                    load_npc_type_script(&type_name, &script_file);
                results.push((type_name, say, think, appear, disappear, close_ch, end_trade, creature_move));
            }
            crate::creatures::npc::apply_npc_script_events(results);
        }

        Ok(())
    }
}
