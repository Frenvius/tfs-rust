use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use crate::combat::CombatType;
use crate::creatures::{LightInfo, Outfit, RaceType, Skull};

static G_MONSTERS: OnceLock<Monsters> = OnceLock::new();

pub fn g_monsters() -> &'static Monsters {
    G_MONSTERS.get().expect("monsters not initialized")
}

pub fn init_monsters(m: Monsters) {
    G_MONSTERS.set(m).unwrap_or_else(|_| panic!("monsters already initialized"));
}

pub const MAX_LOOT_CHANCE: u32 = 100_000;

pub const MONSTERS_EVENT_NONE: u8 = 0;
pub const MONSTERS_EVENT_THINK: u8 = 1;
pub const MONSTERS_EVENT_APPEAR: u8 = 2;
pub const MONSTERS_EVENT_DISAPPEAR: u8 = 3;
pub const MONSTERS_EVENT_MOVE: u8 = 4;
pub const MONSTERS_EVENT_SAY: u8 = 5;

#[derive(Debug, Clone, Default)]
pub struct LootBlock {
    pub id: u16,
    pub count_max: u32,
    pub chance: u32,
    pub sub_type: i32,
    pub action_id: i32,
    pub text: String,
    pub child_loot: Vec<LootBlock>,
}

/// Loot — matches C++ `class Loot` in monsters.h (thin wrapper around LootBlock).
#[derive(Debug, Clone, Default)]
pub struct Loot {
    pub loot_block: LootBlock,
}

impl LootBlock {
    pub fn new() -> Self {
        Self {
            id: 0,
            count_max: 1,
            chance: 0,
            sub_type: -1,
            action_id: -1,
            text: String::new(),
            child_loot: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SummonBlock {
    pub name: String,
    pub chance: u32,
    pub speed: u32,
    pub max: u32,
    pub force: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SpellBlock {
    pub spell_name: String,
    pub chance: u32,
    pub speed: u32,
    pub range: u32,
    pub min_combat_value: i32,
    pub max_combat_value: i32,
    pub combat_spell: bool,
    pub is_melee: bool,
    pub need_target: bool,
    pub radius: i32,
    pub length: i32,
    pub spread: i32,
    pub combat_type: CombatType,
    pub shoot_effect: u8,
    pub area_effect: u8,
    pub skill: i32,
    pub attack: i32,
}

impl SpellBlock {
    pub fn new() -> Self {
        Self {
            spell_name: String::new(),
            chance: 100,
            speed: 2000,
            range: 0,
            min_combat_value: 0,
            max_combat_value: 0,
            combat_spell: false,
            is_melee: false,
            need_target: false,
            radius: 0,
            length: 0,
            spread: 0,
            combat_type: CombatType::PhysicalDamage,
            shoot_effect: 0,
            area_effect: 0,
            skill: 0,
            attack: 0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct VoiceBlock {
    pub text: String,
    pub yell_text: bool,
}

#[derive(Debug, Clone)]
pub struct MonsterSpell {
    pub name: String,
    pub script_name: String,

    pub chance: u8,
    pub range: u8,
    pub drunkenness: u8,

    pub interval: u16,

    pub min_combat_value: i32,
    pub max_combat_value: i32,
    pub attack: i32,
    pub skill: i32,
    pub length: i32,
    pub spread: i32,
    pub radius: i32,
    pub ring: i32,
    pub condition_min_damage: i32,
    pub condition_max_damage: i32,
    pub condition_start_damage: i32,
    pub tick_interval: i32,
    pub min_speed_change: i32,
    pub max_speed_change: i32,
    pub duration: i32,

    pub is_scripted: bool,
    pub need_target: bool,
    pub need_direction: bool,
    pub combat_spell: bool,
    pub is_melee: bool,

    pub outfit: Outfit,
    pub shoot: u8,
    pub effect: u8,
    pub condition_type: u32,
    pub combat_type: CombatType,
}

impl Default for MonsterSpell {
    fn default() -> Self {
        Self::new()
    }
}

impl MonsterSpell {
    pub fn new() -> Self {
        Self {
            name: String::new(),
            script_name: String::new(),
            chance: 100,
            range: 0,
            drunkenness: 0,
            interval: 2000,
            min_combat_value: 0,
            max_combat_value: 0,
            attack: 0,
            skill: 0,
            length: 0,
            spread: 0,
            radius: 0,
            ring: 0,
            condition_min_damage: 0,
            condition_max_damage: 0,
            condition_start_damage: 0,
            tick_interval: 0,
            min_speed_change: 0,
            max_speed_change: 0,
            duration: 0,
            is_scripted: false,
            need_target: false,
            need_direction: false,
            combat_spell: false,
            is_melee: false,
            outfit: Outfit::default(),
            shoot: 0,
            effect: 0,
            condition_type: 0,
            combat_type: CombatType::UndefinedDamage,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MonsterInfo {
    pub element_map: BTreeMap<CombatType, i32>,

    pub voice_vector: Vec<VoiceBlock>,
    pub loot_items: Vec<LootBlock>,
    pub scripts: Vec<String>,
    pub attack_spells: Vec<SpellBlock>,
    pub defense_spells: Vec<SpellBlock>,
    pub summons: Vec<SummonBlock>,

    pub skull: Skull,
    pub outfit: Outfit,
    pub race: RaceType,

    pub light: LightInfo,
    pub look_corpse: u16,

    pub experience: u64,

    pub mana_cost: u32,
    pub yell_chance: u32,
    pub yell_speed_ticks: u32,
    pub static_attack_chance: u32,
    pub max_summons: u32,
    pub change_target_speed: u32,
    pub condition_immunities: u32,
    pub damage_immunities: u32,
    pub base_speed: u32,

    pub creature_appear_event: i32,
    pub creature_disappear_event: i32,
    pub creature_move_event: i32,
    pub creature_say_event: i32,
    pub think_event: i32,
    pub target_distance: i32,
    pub run_away_health: i32,
    pub health: i32,
    pub health_max: i32,
    pub change_target_chance: i32,
    pub defense: i32,
    pub armor: i32,

    pub can_push_items: bool,
    pub can_push_creatures: bool,
    pub pushable: bool,
    pub is_attackable: bool,
    pub is_boss: bool,
    pub is_challengeable: bool,
    pub is_convinceable: bool,
    pub is_hostile: bool,
    pub is_ignoring_spawn_block: bool,
    pub is_illusionable: bool,
    pub is_summonable: bool,
    pub hidden_health: bool,
    pub can_walk_on_energy: bool,
    pub can_walk_on_fire: bool,
    pub can_walk_on_poison: bool,

    pub event_type: u8,
}

impl Default for MonsterInfo {
    fn default() -> Self {
        Self {
            element_map: BTreeMap::new(),
            voice_vector: Vec::new(),
            loot_items: Vec::new(),
            scripts: Vec::new(),
            attack_spells: Vec::new(),
            defense_spells: Vec::new(),
            summons: Vec::new(),
            skull: Skull::None,
            outfit: Outfit::default(),
            race: RaceType::Blood,
            light: LightInfo::default(),
            look_corpse: 0,
            experience: 0,
            mana_cost: 0,
            yell_chance: 0,
            yell_speed_ticks: 0,
            static_attack_chance: 95,
            max_summons: 0,
            change_target_speed: 0,
            condition_immunities: 0,
            damage_immunities: 0,
            base_speed: 200,
            creature_appear_event: -1,
            creature_disappear_event: -1,
            creature_move_event: -1,
            creature_say_event: -1,
            think_event: -1,
            target_distance: 1,
            run_away_health: 0,
            health: 100,
            health_max: 100,
            change_target_chance: 0,
            defense: 0,
            armor: 0,
            can_push_items: false,
            can_push_creatures: false,
            pushable: true,
            is_attackable: true,
            is_boss: false,
            is_challengeable: true,
            is_convinceable: false,
            is_hostile: true,
            is_ignoring_spawn_block: false,
            is_illusionable: false,
            is_summonable: false,
            hidden_health: false,
            can_walk_on_energy: true,
            can_walk_on_fire: true,
            can_walk_on_poison: true,
            event_type: MONSTERS_EVENT_NONE,
        }
    }
}

#[derive(Debug)]
pub struct MonsterType {
    pub name: String,
    pub name_description: String,
    pub info: MonsterInfo,
}

impl MonsterType {
    pub fn new() -> Self {
        Self {
            name: String::new(),
            name_description: String::new(),
            info: MonsterInfo::default(),
        }
    }

    pub fn load_loot(monster_type: &mut MonsterType, loot_block: LootBlock) {
        monster_type.info.loot_items.push(loot_block);
    }

    /// load_callback — port of MonsterType::loadCallback from monsters.cpp.
    ///
    /// Concrete blocker: requires LuaScriptInterface (Phase 6).
    pub fn load_callback(&mut self) -> bool {
        // Deferred: Lua script callback loading for runtime-defined monster types
        true
    }
}

impl Default for MonsterType {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Default)]
pub struct Monsters {
    pub monsters: BTreeMap<String, MonsterType>,
    pub unloaded_monsters: BTreeMap<String, String>,
    loaded: bool,
}

impl Monsters {
    pub fn new() -> Self {
        Self {
            monsters: BTreeMap::new(),
            unloaded_monsters: BTreeMap::new(),
            loaded: false,
        }
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub fn get_monster_type(&self, name: &str) -> Option<&MonsterType> {
        let lower = name.to_lowercase();
        self.monsters.get(&lower)
    }

    pub fn get_monster_type_mut(&mut self, name: &str) -> Option<&mut MonsterType> {
        let lower = name.to_lowercase();
        self.monsters.get_mut(&lower)
    }

    pub fn get_monster_type_or_load(&mut self, name: &str) -> Option<&MonsterType> {
        let lower = name.to_lowercase();
        if self.monsters.contains_key(&lower) {
            return self.monsters.get(&lower);
        }
        if self.unloaded_monsters.contains_key(&lower) {
            // Deferred: lazy-load from unloaded_monsters on first access
        }
        None
    }

    pub fn reload(&mut self) -> bool {
        self.loaded = false;
        self.load_from_json5(std::path::Path::new("data")).is_ok()
    }

    /// deserialize_spell — port of Monsters::deserializeSpell(MonsterSpell*, spellBlock_t&) from monsters.cpp.
    ///
    /// Concrete blocker: requires Combat spell instantiation and LuaScriptInterface (Phase 6).
    pub fn deserialize_spell(
        _spell: &mut crate::creatures::monsters::MonsterSpell,
        _sb: &mut SpellBlock,
    ) -> bool {
        // Deferred: Combat spell instantiation for Lua-defined monster spells
        false
    }

    pub fn load_from_json5(&mut self, data_dir: &Path) -> Result<(), anyhow::Error> {
        self.unloaded_monsters.clear();
        let monsters_file = data_dir.join("monster").join("monsters.json5");

        let content = std::fs::read_to_string(&monsters_file)
            .map_err(|e| anyhow::anyhow!("Failed to read {:?}: {}", monsters_file, e))?;

        let index: serde_json::Value = json5::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse {:?}: {}", monsters_file, e))?;

        let entries = index["monsters"].as_array().cloned().unwrap_or_default();
        let mut loaded = 0usize;

        for entry in &entries {
            let name = entry["name"].as_str().unwrap_or("").to_lowercase();
            let file_str = entry["file"].as_str().unwrap_or("");
            if name.is_empty() || file_str.is_empty() {
                continue;
            }

            // Resolve file path: replace .xml extension with .json5.
            let json5_path = {
                let p = std::path::Path::new(file_str);
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                let parent = p.parent().and_then(|pp| pp.to_str()).unwrap_or("");
                if parent.is_empty() {
                    data_dir.join("monster").join(format!("{stem}.json5"))
                } else {
                    data_dir.join("monster").join(parent).join(format!("{stem}.json5"))
                }
            };

            if !json5_path.exists() {
                self.unloaded_monsters.insert(name.clone(), file_str.to_owned());
                continue;
            }

            match load_monster_type_from_file(&json5_path) {
                Ok(mt) => {
                    self.monsters.insert(name, mt);
                    loaded += 1;
                }
                Err(e) => {
                    tracing::warn!("Failed to load monster {:?}: {}", json5_path, e);
                }
            }
        }

        tracing::info!("Loaded {} monster types ({} unloaded)", loaded, self.unloaded_monsters.len());
        self.loaded = true;
        Ok(())
    }
}

fn load_monster_type_from_file(path: &Path) -> Result<MonsterType, anyhow::Error> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("read: {}", e))?;
    let val: serde_json::Value = json5::from_str(&content)
        .map_err(|e| anyhow::anyhow!("parse: {}", e))?;
    let m = val.get("monster").ok_or_else(|| anyhow::anyhow!("missing 'monster' key"))?;

    let mut mt = MonsterType::new();
    mt.name = m["name"].as_str().unwrap_or("").to_owned();
    mt.name_description = m["nameDescription"].as_str().unwrap_or("").to_owned();

    mt.info.race = match m["race"].as_str().unwrap_or("blood") {
        "venom" => RaceType::Venom,
        "undead" => RaceType::Undead,
        "fire" => RaceType::Fire,
        "energy" => RaceType::Energy,
        _ => RaceType::Blood,
    };

    mt.info.experience = m["experience"].as_u64().unwrap_or(0);
    mt.info.base_speed = m["speed"].as_u64().unwrap_or(200) as u32;
    mt.info.mana_cost = m["manacost"].as_u64().unwrap_or(0) as u32;

    if let Some(h) = m.get("health") {
        mt.info.health = h["now"].as_i64().unwrap_or(100) as i32;
        mt.info.health_max = h["max"].as_i64().unwrap_or(100) as i32;
    }

    if let Some(look) = m.get("look") {
        mt.info.outfit.look_type = look["type"].as_u64().unwrap_or(0) as u16;
        mt.info.look_corpse = look["corpse"].as_u64().unwrap_or(0) as u16;
    }

    if let Some(tc) = m.get("targetchange") {
        mt.info.change_target_speed = tc["interval"].as_u64().unwrap_or(0) as u32;
        mt.info.change_target_chance = tc["chance"].as_i64().unwrap_or(0) as i32;
    }

    if let Some(flags) = m["flags"].as_array() {
        for flag in flags {
            if let Some(obj) = flag.as_object() {
                for (key, val) in obj {
                    match key.as_str() {
                        "summonable" => mt.info.is_summonable = val.as_bool().unwrap_or(false),
                        "attackable" => mt.info.is_attackable = val.as_bool().unwrap_or(true),
                        "hostile" => mt.info.is_hostile = val.as_bool().unwrap_or(true),
                        "illusionable" => mt.info.is_illusionable = val.as_bool().unwrap_or(false),
                        "convinceable" => mt.info.is_convinceable = val.as_bool().unwrap_or(false),
                        "pushable" => mt.info.pushable = val.as_bool().unwrap_or(true),
                        "canpushitems" => mt.info.can_push_items = val.as_bool().unwrap_or(false),
                        "canpushcreatures" => mt.info.can_push_creatures = val.as_bool().unwrap_or(false),
                        "staticattack" => mt.info.static_attack_chance = val.as_u64().unwrap_or(95) as u32,
                        "runonhealth" => mt.info.run_away_health = val.as_i64().unwrap_or(0) as i32,
                        "targetdistance" => mt.info.target_distance = (val.as_i64().unwrap_or(1) as i32).max(1),
                        "canwalkonenergy" => mt.info.can_walk_on_energy = val.as_bool().unwrap_or(true),
                        "canwalkonfire" => mt.info.can_walk_on_fire = val.as_bool().unwrap_or(true),
                        "canwalkonpoison" => mt.info.can_walk_on_poison = val.as_bool().unwrap_or(true),
                        "boss" => mt.info.is_boss = val.as_bool().unwrap_or(false),
                        "challengeable" => mt.info.is_challengeable = val.as_bool().unwrap_or(true),
                        _ => {}
                    }
                }
            }
        }
    }

    if let Some(def) = m.get("defenses") {
        if let Some(a) = def.get("armor").and_then(|v| v.as_i64()) {
            mt.info.armor = a as i32;
        }
        // `defense` may be the numeric defense value OR a single defense-spell
        // object; `defenses` is the array of defense spells.
        if let Some(d) = def.get("defense") {
            if let Some(n) = d.as_i64() {
                mt.info.defense = n as i32;
            }
        }

        let mut defense_entries: Vec<&serde_json::Value> = Vec::new();
        if let Some(arr) = def.get("defenses").and_then(|v| v.as_array()) {
            defense_entries.extend(arr.iter());
        }
        match def.get("defense") {
            Some(serde_json::Value::Array(arr)) => defense_entries.extend(arr.iter()),
            Some(obj) if obj.is_object() => defense_entries.push(obj),
            _ => {}
        }

        for d in defense_entries {
            let name = d["name"].as_str().unwrap_or("").to_owned();
            // Only genuine combat defenses (healing / direct damage) are cast as
            // combat. Condition buffs (speed/invisible/outfit/strength/...) need
            // monster-condition support that is not yet modeled, so they are
            // skipped rather than misinterpreted as physical self-damage.
            if !is_combat_spell_name(&name) {
                continue;
            }
            let mut sb = SpellBlock::new();
            sb.spell_name = name;
            sb.speed = d["interval"].as_u64().unwrap_or(2000) as u32;
            sb.chance = d["chance"].as_u64().unwrap_or(100) as u32;
            sb.min_combat_value = d["min"].as_i64().unwrap_or(0) as i32;
            sb.max_combat_value = d["max"].as_i64().unwrap_or(0) as i32;
            sb.radius = d["radius"].as_i64().unwrap_or(0) as i32;
            sb.combat_type = spell_name_to_combat_type(&sb.spell_name);
            sb.combat_spell = true;
            for attrs_key in &["attributes", "attribute"] {
                let items: Vec<&serde_json::Value> = if let Some(arr) = d[attrs_key].as_array() {
                    arr.iter().collect()
                } else if let Some(obj) = d.get(attrs_key) {
                    if obj.is_object() { vec![obj] } else { vec![] }
                } else {
                    vec![]
                };
                for item in items {
                    let key = item["key"].as_str().unwrap_or("");
                    let val = item["value"].as_str().unwrap_or("");
                    if key == "areaEffect" {
                        sb.area_effect = const_me_from_str(val);
                    }
                }
            }
            mt.info.defense_spells.push(sb);
        }
    }

    if let Some(elems) = m["elements"].as_array() {
        for elem in elems {
            if let Some(obj) = elem.as_object() {
                for (key, val) in obj {
                    let ct = match key.as_str() {
                        "earthPercent" => Some(CombatType::EarthDamage),
                        "energyPercent" => Some(CombatType::EnergyDamage),
                        "firePercent" => Some(CombatType::FireDamage),
                        "icePercent" => Some(CombatType::IceDamage),
                        "holyPercent" => Some(CombatType::HolyDamage),
                        "deathPercent" => Some(CombatType::DeathDamage),
                        "lifedrainPercent" => Some(CombatType::LifeDrain),
                        "drownPercent" => Some(CombatType::DrownDamage),
                        "physicalPercent" => Some(CombatType::PhysicalDamage),
                        _ => None,
                    };
                    if let Some(c) = ct {
                        mt.info.element_map.insert(c, val.as_i64().unwrap_or(0) as i32);
                    }
                }
            }
        }
    }

    if let Some(imms) = m["immunities"].as_array() {
        for imm in imms {
            if let Some(obj) = imm.as_object() {
                for (key, val) in obj {
                    if !val.as_bool().unwrap_or(false) {
                        continue;
                    }
                    apply_immunity(&mut mt.info, key);
                }
            }
        }
    }

    // Parse attacks.
    if let Some(attacks) = m["attacks"].as_array() {
        for atk in attacks {
            let mut sb = SpellBlock::new();
            sb.spell_name = atk["name"].as_str().unwrap_or("").to_owned();
            sb.speed = atk["interval"].as_u64().unwrap_or(2000) as u32;
            sb.chance = atk["chance"].as_u64().unwrap_or(100) as u32;
            sb.range = atk["range"].as_u64().unwrap_or(0) as u32;
            sb.min_combat_value = atk["min"].as_i64().unwrap_or(0) as i32;
            sb.max_combat_value = atk["max"].as_i64().unwrap_or(0) as i32;
            sb.radius = atk["radius"].as_i64().unwrap_or(0) as i32;
            sb.length = atk["length"].as_i64().unwrap_or(0) as i32;
            sb.spread = atk["spread"].as_i64().unwrap_or(0) as i32;
            sb.need_target = atk["target"].as_bool().unwrap_or(false);
            sb.skill = atk["skill"].as_i64().unwrap_or(0) as i32;
            sb.attack = atk["attack"].as_i64().unwrap_or(0) as i32;
            sb.is_melee = sb.spell_name == "melee";
            sb.combat_spell = !sb.is_melee;
            sb.combat_type = spell_name_to_combat_type(&sb.spell_name);
            // Parse attributes for shoot/area effects.
            for attrs_key in &["attributes", "attribute"] {
                let items: Vec<&serde_json::Value> = if let Some(arr) = atk[attrs_key].as_array() {
                    arr.iter().collect()
                } else if let Some(obj) = atk.get(attrs_key) {
                    if obj.is_object() { vec![obj] } else { vec![] }
                } else {
                    vec![]
                };
                for item in items {
                    let key = item["key"].as_str().unwrap_or("");
                    let val = item["value"].as_str().unwrap_or("");
                    match key {
                        "shootEffect" => sb.shoot_effect = const_ani_from_str(val),
                        "areaEffect" => sb.area_effect = const_me_from_str(val),
                        _ => {}
                    }
                }
            }
            mt.info.attack_spells.push(sb);
        }
    }

    // Parse voices.
    if let Some(voices_obj) = m.get("voices") {
        mt.info.yell_speed_ticks = voices_obj["interval"].as_u64().unwrap_or(5000) as u32;
        mt.info.yell_chance = voices_obj["chance"].as_u64().unwrap_or(0) as u32;
        let sentences: Vec<&serde_json::Value> = {
            let mut v = Vec::new();
            if let Some(arr) = voices_obj["voices"].as_array() {
                v.extend(arr.iter());
            }
            if let Some(arr) = voices_obj["voice"].as_array() {
                v.extend(arr.iter());
            }
            if let Some(obj) = voices_obj.get("voice") {
                if obj.is_object() {
                    v.push(obj);
                }
            }
            v
        };
        for s in sentences {
            let text = s["sentence"].as_str().unwrap_or("").to_owned();
            let yell_text = s["yell"].as_bool().unwrap_or(false);
            if !text.is_empty() {
                mt.info.voice_vector.push(VoiceBlock { text, yell_text });
            }
        }
    }

    // Parse loot. Port of Monsters::loadLootItem / loadLootContainer
    // (monsters.cpp). The xml2json5 converter encodes a single loot child as a
    // singular `loot.item` object and multiple as `loot.items: [...]`; container
    // contents are a nested `items`/`item` under the entry. Container detection
    // is structural here (presence of nested children) rather than via the item
    // type's container flag — faithful to the converted dataset, see D021.
    if let Some(loot_obj) = m.get("loot") {
        for entry in collect_loot_entries(loot_obj) {
            if let Some(lb) = parse_loot_block(entry) {
                mt.info.loot_items.push(lb);
            }
        }
    }

    if let Some(summons_obj) = m.get("summons") {
        mt.info.max_summons = summons_obj["maxSummons"].as_u64().unwrap_or(0) as u32;
        let blocks: Vec<&serde_json::Value> = {
            let mut v = Vec::new();
            if let Some(arr) = summons_obj["summon"].as_array() {
                v.extend(arr.iter());
            } else if let Some(obj) = summons_obj.get("summon") {
                if obj.is_object() {
                    v.push(obj);
                }
            }
            v
        };
        for s in blocks {
            let name = s["name"].as_str().unwrap_or("").to_owned();
            if name.is_empty() {
                continue;
            }
            // XML `speed`/`interval` are the same field; JSON5 uses `interval`.
            let speed = s["interval"]
                .as_u64()
                .or_else(|| s["speed"].as_u64())
                .unwrap_or(1000) as u32;
            let chance = s["chance"].as_u64().unwrap_or(100) as u32;
            let max = s["max"].as_u64().unwrap_or(mt.info.max_summons as u64) as u32;
            let force = s["force"].as_bool().unwrap_or(false);
            mt.info.summons.push(SummonBlock { name, chance, speed, max, force });
        }
    }

    Ok(mt)
}

/// Collect the loot child entries from a `loot` (or nested item) object,
/// accepting both the plural `items: [...]` array and the singular `item: {...}`
/// object emitted by the converter.
fn collect_loot_entries(obj: &serde_json::Value) -> Vec<&serde_json::Value> {
    let mut v = Vec::new();
    if let Some(arr) = obj.get("items").and_then(|x| x.as_array()) {
        v.extend(arr.iter());
    }
    match obj.get("item") {
        Some(serde_json::Value::Array(arr)) => v.extend(arr.iter()),
        Some(o) if o.is_object() => v.push(o),
        _ => {}
    }
    v
}

/// Port of Monsters::loadLootItem (monsters.cpp:1364). Returns None when the
/// entry has no usable id (matching the C++ early-return on a 0 id).
fn parse_loot_block(entry: &serde_json::Value) -> Option<LootBlock> {
    let mut lb = LootBlock::new();

    // Resolve the item id from `id`, or fall back to `name` lookup like C++
    // loadLootItem. The converted dataset uses both forms — e.g. orc spearman
    // and most of dragon lord's loot are name-based, not id-based.
    lb.id = entry["id"].as_u64().unwrap_or(0) as u16;
    if lb.id == 0 {
        if let Some(name) = entry.get("name").and_then(|v| v.as_str()) {
            lb.id = crate::items::g_items().get_item_id_by_name(name).unwrap_or(0);
        }
    }
    if lb.id == 0 {
        return None;
    }

    // countmax: default 1, reject > 100 (matches C++ loadLootItem).
    if let Some(cm) = entry.get("countmax").and_then(|v| v.as_i64()) {
        if cm > 100 {
            return None;
        }
        lb.count_max = cm.max(1) as u32;
    } else {
        lb.count_max = 1;
    }

    // chance / chance1: default MAX_LOOT_CHANCE, clamp to the max.
    let chance = entry
        .get("chance")
        .and_then(|v| v.as_i64())
        .or_else(|| entry.get("chance1").and_then(|v| v.as_i64()));
    lb.chance = match chance {
        Some(c) => (c.max(0) as u32).min(MAX_LOOT_CHANCE),
        None => MAX_LOOT_CHANCE,
    };

    // Nested container contents.
    for child in collect_loot_entries(entry) {
        if let Some(child_lb) = parse_loot_block(child) {
            lb.child_loot.push(child_lb);
        }
    }

    // Optional fields.
    if let Some(st) = entry.get("subtype").and_then(|v| v.as_i64()) {
        lb.sub_type = st as i32;
    }
    if let Some(aid) = entry
        .get("actionId")
        .or_else(|| entry.get("actionid"))
        .and_then(|v| v.as_i64())
    {
        lb.action_id = aid as i32;
    }
    if let Some(text) = entry.get("text").and_then(|v| v.as_str()) {
        lb.text = text.to_owned();
    }

    Some(lb)
}

fn apply_immunity(info: &mut MonsterInfo, key: &str) {
    use crate::combat::condition::ConditionType;
    match key {
        "fire" => {
            info.damage_immunities |= CombatType::FireDamage as u32;
            info.condition_immunities |= ConditionType::Fire as u32;
        }
        "energy" => {
            info.damage_immunities |= CombatType::EnergyDamage as u32;
            info.condition_immunities |= ConditionType::Energy as u32;
        }
        "poison" | "earth" => {
            info.damage_immunities |= CombatType::EarthDamage as u32;
            info.condition_immunities |= ConditionType::Poison as u32;
        }
        "drown" => {
            info.damage_immunities |= CombatType::DrownDamage as u32;
            info.condition_immunities |= ConditionType::Drown as u32;
        }
        "ice" => {
            info.damage_immunities |= CombatType::IceDamage as u32;
            info.condition_immunities |= ConditionType::Freezing as u32;
        }
        "holy" => {
            info.damage_immunities |= CombatType::HolyDamage as u32;
            info.condition_immunities |= ConditionType::Dazzled as u32;
        }
        "death" => {
            info.damage_immunities |= CombatType::DeathDamage as u32;
            info.condition_immunities |= ConditionType::Cursed as u32;
        }
        "paralyze" => info.condition_immunities |= ConditionType::Paralyze as u32,
        "invisible" => info.condition_immunities |= ConditionType::Invisible as u32,
        "bleed" | "bleeding" => info.condition_immunities |= ConditionType::Bleeding as u32,
        _ => {}
    }
}

fn is_combat_spell_name(name: &str) -> bool {
    matches!(
        name,
        "melee"
            | "physical"
            | "fire"
            | "firefield"
            | "energy"
            | "energyfield"
            | "earth"
            | "poisonfield"
            | "ice"
            | "icefield"
            | "holy"
            | "death"
            | "lifedrain"
            | "manadrain"
            | "healing"
            | "drown"
    )
}

fn spell_name_to_combat_type(name: &str) -> CombatType {
    match name {
        "melee" | "physical" => CombatType::PhysicalDamage,
        "fire" | "firefield" => CombatType::FireDamage,
        "energy" | "energyfield" => CombatType::EnergyDamage,
        "earth" | "poisonfield" => CombatType::EarthDamage,
        "ice" | "icefield" => CombatType::IceDamage,
        "holy" => CombatType::HolyDamage,
        "death" => CombatType::DeathDamage,
        "lifedrain" => CombatType::LifeDrain,
        "manadrain" => CombatType::ManaDrain,
        "healing" => CombatType::Healing,
        "drown" => CombatType::DrownDamage,
        _ => CombatType::PhysicalDamage,
    }
}

fn const_ani_from_str(s: &str) -> u8 {
    match s {
        "spear" => 1, "bolt" => 2, "arrow" => 3, "fire" => 4, "energy" => 5,
        "poisonarrow" => 6, "burstarrow" => 7, "throwingstar" => 8, "throwingknife" => 9,
        "smallstone" => 10, "death" | "deathball" => 11, "largerock" => 12, "snowball" => 13,
        "powerbolt" => 14, "poison" => 15, "infernalbolt" => 16, "huntingspear" => 17,
        "enchantedspear" => 18, "redstar" => 19, "greenstar" => 20, "royalspear" => 21,
        "sniperarrow" => 22, "onyxarrow" => 23, "piercingbolt" => 24, "whirlwindsword" => 25,
        "whirlwindaxe" => 26, "whirlwindclub" => 27, "etherealspear" => 28, "ice" => 29,
        "earth" => 30, "holy" => 31, "suddendeath" => 32, "flasharrow" => 33,
        "flammingarrow" => 34, "shiverarrow" => 35, "energyball" => 36, "smallice" => 37,
        "smallholy" => 38, "smallearth" => 39, "eartharrow" => 40, "explosion" => 41,
        "cake" => 42,
        _ => 0,
    }
}

fn const_me_from_str(s: &str) -> u8 {
    match s {
        "redspark" => 1, "bluebubble" => 2, "poff" => 3, "yellowspark" => 4,
        "explosionarea" => 5, "explosion" => 6, "firearea" => 7, "yellowbubble" => 8,
        "greenbubble" => 9, "blackspark" => 10, "teleport" => 11, "energy" => 12,
        "energyarea" => 13, "blueshimmer" | "magic_blue" => 14, "redshimmer" => 15,
        "greenshimmer" => 16, "fire" => 17, "greenspark" => 18, "mortarea" => 19,
        "earthwave" | "green_rings" => 20, "bluefireworks" => 21, "redfireworks" => 22,
        "yellowfireworks" => 23, "teleportblue" => 24, "teleportgreen" => 25,
        "teleportwhite" => 26, "bloodysteps" => 27, "blackspit" => 28, "steamburst" => 29,
        "icetornado" | "icearea" => 30,
        "iceattack" | "icefront" | "ice" => 33, "holydamage" => 35, "holyspark" => 36,
        "poisonarea" => 38, "poisonbubbles" => 39, "poisonmist" => 40, "smallclouds" => 41,
        "hitbleed" => 42, "smokecloud" => 43, "yellowexperience" => 50,
        _ => 0,
    }
}

#[cfg(test)]
mod loot_tests {
    use super::{collect_loot_entries, parse_loot_block, MAX_LOOT_CHANCE};

    #[test]
    fn plural_items_array_parses_every_entry() {
        let v: serde_json::Value = json5::from_str(
            r#"{ items: [
                { id: 2148, countmax: 8, chance: 90000 },
                { id: 2389, countmax: 3, chance: 9000 },
                { id: 2050, chance: 5000 },
            ] }"#,
        )
        .unwrap();
        let entries = collect_loot_entries(&v);
        assert_eq!(entries.len(), 3);
        let blocks: Vec<_> = entries.into_iter().filter_map(parse_loot_block).collect();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].id, 2148);
        assert_eq!(blocks[0].count_max, 8);
        assert_eq!(blocks[0].chance, 90000);
        // No chance specified -> defaults to MAX_LOOT_CHANCE.
        assert_eq!(blocks[2].id, 2050);
        assert_eq!(blocks[2].count_max, 1);
        assert_eq!(blocks[2].chance, 5000);
    }

    #[test]
    fn singular_item_object_parses() {
        let v: serde_json::Value =
            json5::from_str(r#"{ item: { id: 1234, chance: 50000 } }"#).unwrap();
        let blocks: Vec<_> = collect_loot_entries(&v)
            .into_iter()
            .filter_map(parse_loot_block)
            .collect();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].id, 1234);
        assert_eq!(blocks[0].chance, 50000);
    }

    #[test]
    fn nested_container_items_become_child_loot() {
        let v: serde_json::Value = json5::from_str(
            r#"{ items: [
                { id: 1987, items: [
                    { id: 2472, chance: 1000 },
                    { id: 2152, countmax: 3, chance: 2000 },
                ] },
            ] }"#,
        )
        .unwrap();
        let blocks: Vec<_> = collect_loot_entries(&v)
            .into_iter()
            .filter_map(parse_loot_block)
            .collect();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].id, 1987);
        assert_eq!(blocks[0].child_loot.len(), 2);
        assert_eq!(blocks[0].child_loot[1].id, 2152);
        assert_eq!(blocks[0].child_loot[1].count_max, 3);
    }

    #[test]
    fn missing_id_and_overlarge_countmax_are_rejected() {
        let no_id: serde_json::Value = json5::from_str(r#"{ chance: 1000 }"#).unwrap();
        assert!(parse_loot_block(&no_id).is_none());
        let big: serde_json::Value =
            json5::from_str(r#"{ id: 2148, countmax: 101 }"#).unwrap();
        assert!(parse_loot_block(&big).is_none());
    }

    #[test]
    fn chance_clamped_to_max() {
        let v: serde_json::Value =
            json5::from_str(r#"{ id: 2148, chance: 999999 }"#).unwrap();
        let lb = parse_loot_block(&v).unwrap();
        assert_eq!(lb.chance, MAX_LOOT_CHANCE);
    }
}
