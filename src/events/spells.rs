use mlua::prelude::*;
use serde::{Deserialize, Deserializer};

use crate::events::registry::{g_script_registry, SpellEntry};
use crate::lua::script::g_lua;

/// Deserialize a 0/1 flag that the XML may emit as a string (`"1"`/`"true"`/`"0"`/`"false"`).
/// Mirrors the C++ `pugicast` helpers.
fn flag_u8<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u8, D::Error> {
    struct FlagVisitor;
    impl serde::de::Visitor<'_> for FlagVisitor {
        type Value = u8;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a boolean, integer, or string flag")
        }
        fn visit_bool<E>(self, v: bool) -> Result<u8, E> { Ok(u8::from(v)) }
        fn visit_u64<E>(self, v: u64) -> Result<u8, E> { Ok((v != 0) as u8) }
        fn visit_i64<E>(self, v: i64) -> Result<u8, E> { Ok((v != 0) as u8) }
        fn visit_f64<E>(self, v: f64) -> Result<u8, E> { Ok((v != 0.0) as u8) }
        fn visit_str<E>(self, v: &str) -> Result<u8, E> {
            Ok(matches!(v.trim(), "1" | "true" | "yes") as u8)
        }
    }
    deserializer.deserialize_any(FlagVisitor)
}

/// `Option<u8>` variant of [`flag_u8`] for fields that distinguish "absent"
/// from "present and zero" (e.g. `enabled`).
fn opt_flag_u8<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u8>, D::Error> {
    struct OptFlagVisitor;
    impl<'de> serde::de::Visitor<'de> for OptFlagVisitor {
        type Value = Option<u8>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("an optional boolean/integer/string flag")
        }
        fn visit_none<E>(self) -> Result<Option<u8>, E> { Ok(None) }
        fn visit_unit<E>(self) -> Result<Option<u8>, E> { Ok(None) }
        fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<Option<u8>, D2::Error> {
            flag_u8(d).map(Some)
        }
    }
    deserializer.deserialize_option(OptFlagVisitor)
}

#[derive(Debug, Deserialize)]
enum SpellNode {
    #[serde(rename = "instant")]
    Instant(SpellDef),
    #[serde(rename = "rune")]
    Rune(SpellDef),
}

#[derive(Debug, Deserialize)]
struct SpellsXml {
    #[serde(rename = "$value", default)]
    entries: Vec<SpellNode>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SpellDef {
    #[serde(rename = "@spellid", default)]
    spellid: u8,
    #[serde(rename = "@name", default)]
    name: String,
    #[serde(rename = "@words", default)]
    words: String,
    #[serde(rename = "@group", default)]
    group: String,
    #[serde(rename = "@secondarygroup", default)]
    secondary_group: String,
    #[serde(rename = "@secondarygroupcooldown", default)]
    secondary_group_cooldown: u32,
    #[serde(rename = "@level", default)]
    level: u32,
    #[serde(rename = "@magiclevel", default)]
    magic_level: u32,
    #[serde(rename = "@mana", default)]
    mana: u32,
    #[serde(rename = "@manaPercent", default)]
    mana_percent: u32,
    #[serde(rename = "@soul", default)]
    soul: u32,
    #[serde(rename = "@range", default)]
    #[allow(dead_code)]
    range: i32,
    #[serde(rename = "@cooldown", default)]
    cooldown: u32,
    #[serde(rename = "@groupcooldown", default)]
    group_cooldown: u32,
    #[serde(rename = "@premium", default, deserialize_with = "flag_u8")]
    #[allow(dead_code)]
    premium: u8,
    #[serde(rename = "@enabled", default, deserialize_with = "opt_flag_u8")]
    enabled: Option<u8>,
    #[serde(rename = "@needlearn", default, deserialize_with = "flag_u8")]
    needlearn: u8,
    #[serde(rename = "@needweapon", default, deserialize_with = "flag_u8")]
    needweapon: u8,
    #[serde(rename = "@needtarget", default, deserialize_with = "flag_u8")]
    needtarget: u8,
    #[serde(rename = "@selftarget", default, deserialize_with = "flag_u8")]
    selftarget: u8,
    #[serde(rename = "@aggressive", default)]
    aggressive: Option<i32>,
    #[serde(rename = "@hasparams", default, deserialize_with = "flag_u8")]
    hasparams: u8,
    #[serde(rename = "@hasplayernameparam", default, deserialize_with = "flag_u8")]
    has_player_name_param: u8,
    #[serde(rename = "@pzlock", default, deserialize_with = "flag_u8")]
    pz_lock: u8,
    #[serde(rename = "@script")]
    script: Option<String>,
    #[serde(default)]
    vocation: Vec<SpellVocationNode>,
}

#[derive(Debug, Deserialize, Clone)]
struct SpellVocationNode {
    #[serde(rename = "@name", default)]
    name: String,
}

pub fn load_spells() {
    let path = "data/spells/spells.xml";
    if !std::path::Path::new(path).exists() {
        return;
    }

    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => { tracing::error!("Failed to read {path}: {e}"); return; }
    };

    let file: SpellsXml = match quick_xml::de::from_str(&text) {
        Ok(f) => f,
        Err(e) => { tracing::error!("Failed to parse {path}: {e}"); return; }
    };

    let mut count = 0usize;

    let all_spells = file.entries.iter().map(|node| match node {
        SpellNode::Instant(s) => (1i32, s),
        SpellNode::Rune(s) => (2i32, s),
    });

    for (spell_type, def) in all_spells {
        if def.words.is_empty() && spell_type == 1 { continue; }
        let enabled = def.enabled.map(|v| v != 0).unwrap_or(true);
        if !enabled { continue; }

        let script_id = if let Some(script_file) = &def.script {
            load_spell_script(script_file)
        } else {
            0
        };

        let group_num: u32 = match def.group.as_str() {
            "attack" => 1,
            "healing" => 2,
            "support" => 3,
            "special" => 4,
            _ => 0,
        };
        let secondary_group_num: u32 = match def.secondary_group.as_str() {
            "attack" => 1,
            "healing" => 2,
            "support" => 3,
            "special" => 4,
            _ => 0,
        };
        let cooldown = if def.cooldown > 0 { def.cooldown } else { 1000 };
        let group_cooldown = if def.group_cooldown > 0 { def.group_cooldown } else { cooldown };

        let aggressive = def.aggressive.map(|v| v != 0)
            .unwrap_or(spell_type == 1 && def.selftarget == 0 && group_num == 1);

        let entry = SpellEntry {
            name: def.name.clone(),
            words: def.words.clone(),
            script_id,
            spell_type,
            spell_id: def.spellid,
            level: def.level,
            magic_level: def.magic_level,
            mana: def.mana,
            mana_percent: def.mana_percent,
            soul: def.soul,
            group: group_num,
            secondary_group: secondary_group_num,
            cooldown,
            group_cooldown,
            secondary_group_cooldown: def.secondary_group_cooldown,
            need_target: def.needtarget != 0,
            need_weapon: def.needweapon != 0,
            need_learn: def.needlearn != 0,
            self_target: def.selftarget != 0,
            aggressive,
            pz_lock: def.pz_lock != 0,
            has_params: def.hasparams != 0,
            has_player_name_param: def.has_player_name_param != 0,
            premium: def.premium != 0,
            learnable: spell_type == 1 && def.needlearn != 0,
            enabled,
            vocations: {
                use crate::world::vocation::g_vocations;
                def.vocation.iter().filter_map(|v| {
                    g_vocations().get_vocation_id(&v.name).map(|id| id as u32)
                }).collect()
            },
        };

        let key = def.words.to_lowercase();
        let mut registry = g_script_registry().lock().unwrap();
        registry.spells.insert(key, entry);
        drop(registry);
        count += 1;
    }

    println!("{count} spells");
}

fn load_spell_script(script_file: &str) -> i32 {
    let path = format!("data/spells/scripts/{script_file}");
    let lua = g_lua();

    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("Failed to read spell script {path}: {e}");
            return 0;
        }
    };

    // Execute the script in Lua — this defines the `onCastSpell` global (and local combat).
    if let Err(e) = lua.load(&text).set_name(path.as_str()).exec() {
        tracing::error!("Failed to load spell script {path}: {e}");
        return 0;
    }

    // Grab the onCastSpell function now defined in globals.
    let func: LuaFunction = match lua.globals().get("onCastSpell") {
        Ok(f) => f,
        Err(_) => return 0,
    };

    let mut registry = g_script_registry().lock().unwrap();
    let id = registry.next_id();
    match lua.create_registry_value(func) {
        Ok(key) => { registry.lua_callbacks.insert(id, key); id }
        Err(e) => { tracing::error!("create_registry_value failed for {path}: {e}"); 0 }
    }
}
