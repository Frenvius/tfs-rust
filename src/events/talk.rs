use std::collections::BTreeMap;

use serde::Deserialize;

use crate::lua::script::LuaScriptInterface;

// ---------------------------------------------------------------------------
// XML schema
// ---------------------------------------------------------------------------

/// A single entry in `data/talkactions/talkactions.xml`. Mirrors a `<talkaction>` node.
#[derive(Debug, Deserialize)]
pub struct TalkActionEntry {
    #[serde(rename = "@script")]
    pub script: Option<String>,
    #[serde(rename = "@words")]
    pub words: Option<String>,
    #[serde(rename = "@separator")]
    pub separator: Option<String>,
    #[serde(rename = "@access")]
    pub access: Option<bool>,
    #[serde(rename = "@accounttype")]
    pub account_type: Option<u8>,
}

/// Top-level document for `data/talkactions/talkactions.xml`.
#[derive(Debug, Deserialize)]
pub struct TalkActionsXml {
    #[serde(rename = "talkaction", default)]
    pub talkactions: Vec<TalkActionEntry>,
}

// ---------------------------------------------------------------------------
// Runtime types
// ---------------------------------------------------------------------------

/// Result of trying to match player speech against registered talk actions.
/// Mirrors `TalkActionResult_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TalkActionResult {
    Continue = 0,
    Break = 1,
    Failed = 2,
}

/// Mirrors the C++ `TalkAction` class.
#[derive(Debug, Clone)]
pub struct TalkAction {
    pub script_id: i32,
    /// The primary trigger word (first element of `words_map`).
    pub words: String,
    /// All trigger words (the original XML allows semicolon-separated lists).
    pub words_map: Vec<String>,
    /// Separator between the trigger word and its parameter. Default `"`.
    pub separator: String,
    pub need_access: bool,
    /// Maps to `AccountType_t`; 0 = ACCOUNT_TYPE_NORMAL.
    pub required_account_type: u8,
    pub scripted: bool,
    pub from_lua: bool,
}

impl Default for TalkAction {
    fn default() -> Self {
        Self {
            script_id: 0,
            words: String::new(),
            words_map: Vec::new(),
            separator: String::from("\""),
            need_access: false,
            required_account_type: 0,
            scripted: false,
            from_lua: false,
        }
    }
}

/// Mirrors the C++ `TalkActions` class (inherits `BaseEvents`).
pub struct TalkActions {
    script_interface: LuaScriptInterface,
    talk_actions: BTreeMap<String, TalkAction>,
}

impl TalkActions {
    /// Mirrors `TalkActions::TalkActions()`.
    pub fn new() -> Self {
        let mut iface = LuaScriptInterface::new("TalkAction Interface");
        if let Err(e) = iface.init_state() {
            tracing::error!("TalkActions::new - init_state failed: {e}");
        }
        let lib = "data/talkactions/lib/talkactions.lua";
        if let Err(e) = iface.load_file(lib) {
            tracing::warn!("TalkActions::new - cannot load talkactions lib: {e}");
        }
        Self {
            script_interface: iface,
            talk_actions: BTreeMap::new(),
        }
    }

    /// Script base name. Mirrors `TalkActions::getScriptBaseName()`.
    pub fn get_script_base_name() -> &'static str {
        "talkactions"
    }

    /// Load `data/talkactions/talkactions.xml`. Returns `true` on success.
    pub fn load_from_xml(&mut self) -> bool {
        let path = "data/talkactions/talkactions.xml";
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("TalkActions::load_from_xml - {path} not found: {e}");
                return false;
            }
        };

        let file: TalkActionsXml = match quick_xml::de::from_str(&source) {
            Ok(f) => f,
            Err(e) => {
                tracing::error!("TalkActions::load_from_xml - parse error in {path}: {e}");
                return false;
            }
        };

        for entry in file.talkactions {
            let mut ta = TalkAction {
                separator: entry.separator.unwrap_or_else(|| String::from("\"")),
                need_access: entry.access.unwrap_or(false),
                required_account_type: entry.account_type.unwrap_or(0),
                ..Default::default()
            };

            let words = collect_words(&entry.words);
            if words.is_empty() {
                tracing::warn!(
                    "TalkActions::load_from_xml - entry missing words, skipping"
                );
                continue;
            }
            ta.words = words[0].clone();
            ta.words_map = words.clone();

            if let Some(ref script_file) = entry.script {
                let script_path =
                    format!("data/talkactions/scripts/{script_file}");
                match self.script_interface.load_file(&script_path) {
                    Ok(()) => {
                        let id = self
                            .script_interface
                            .get_event("onSay");
                        if id == -1 {
                            tracing::warn!(
                                "TalkActions::load_from_xml - onSay not found in {script_path}"
                            );
                            continue;
                        }
                        ta.script_id = id;
                        ta.scripted = true;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "TalkActions::load_from_xml - cannot load {script_path}: {e}"
                        );
                        continue;
                    }
                }
            }

            // Register once per trigger word, mirroring C++ registerEvent.
            for word in ta.words_map.clone() {
                self.talk_actions.insert(word, ta.clone());
            }
        }

        true
    }

    /// Register a talk action from a Lua script. Mirrors
    /// `TalkActions::registerLuaEvent()`.
    pub fn register_lua_event(&mut self, ta: TalkAction) -> bool {
        if ta.words_map.is_empty() {
            tracing::warn!("TalkActions::register_lua_event - event has no words");
            return false;
        }
        for word in &ta.words_map {
            self.talk_actions.insert(word.clone(), ta.clone());
        }
        true
    }

    /// Clear entries matching `from_lua`. Mirrors `TalkActions::clear()`.
    pub fn clear(&mut self, from_lua: bool) {
        self.talk_actions.retain(|_, v| v.from_lua != from_lua);
        if !from_lua {
            if let Err(e) = self.script_interface.re_init_state() {
                tracing::error!("TalkActions::clear - re_init_state failed: {e}");
            }
        }
    }

    /// Dead code — talk action dispatch goes through events/dispatch.rs::execute_talk_action.
    #[allow(dead_code)]
    pub fn player_say_spell(&self, _words: &str) -> TalkActionResult {
        TalkActionResult::Continue
    }

    pub fn get_talk_action(&self, words: &str) -> Option<&TalkAction> {
        for (trigger, ta) in &self.talk_actions {
            if words.starts_with(trigger.as_str()) {
                let rest = &words[trigger.len()..];
                if rest.is_empty() || rest.starts_with(&ta.separator) || rest.starts_with(' ') {
                    return Some(ta);
                }
            }
        }
        None
    }

    pub fn get_script_function(&self, script_id: i32) -> Option<mlua::Function> {
        self.script_interface.push_function(script_id).ok()
    }

    /// Iterate the registered talk actions.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &TalkAction)> {
        self.talk_actions.iter().map(|(k, v)| (k.as_str(), v))
    }
}

impl Default for TalkActions {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn collect_words(value: &Option<String>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(s) = value {
        for w in s.split(';') {
            let trimmed = w.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_owned());
            }
        }
    }
    out
}
