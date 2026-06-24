pub mod monster;
pub mod monsters;
pub mod npc;
pub mod player;

use crate::map::Position;
use crate::combat::condition::Condition;

pub use monster::{Monster, TargetSearchType, DESPAWN_RANGE, DESPAWN_RADIUS};
pub use monsters::{LootBlock, MonsterInfo, MonsterSpell, MonsterType, Monsters, SpellBlock, SummonBlock, VoiceBlock, MAX_LOOT_CHANCE};
pub use npc::{Npc, Npcs, NpcType, g_npcs, init_npcs};
pub use player::Player;

pub type CreatureId = u32;

pub const EVENT_CREATURECOUNT: i32 = 10;
pub const EVENT_CREATURE_THINK_INTERVAL: i32 = 1000;
pub const EVENT_CHECK_CREATURE_INTERVAL: i32 = EVENT_CREATURE_THINK_INTERVAL / EVENT_CREATURECOUNT;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    #[default]
    North = 0,
    East = 1,
    South = 2,
    West = 3,
    SouthWest = 4,
    SouthEast = 5,
    NorthWest = 6,
    NorthEast = 7,
}

impl Direction {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::North),
            1 => Some(Self::East),
            2 => Some(Self::South),
            3 => Some(Self::West),
            4 => Some(Self::SouthWest),
            5 => Some(Self::SouthEast),
            6 => Some(Self::NorthWest),
            7 => Some(Self::NorthEast),
            _ => None,
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneType {
    Normal = 0,
    Protection = 1,
    NoPvp = 2,
    Pvp = 3,
    NoLogout = 4,
    Pvp2 = 5,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RaceType {
    #[default]
    None = 0,
    Venom = 1,
    Blood = 2,
    Undead = 3,
    Fire = 4,
    Energy = 5,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Skull {
    #[default]
    None = 0,
    Yellow = 1,
    Green = 2,
    White = 3,
    Red = 4,
    Black = 5,
    Orange = 6,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CreatureType {
    #[default]
    Player = 0,
    Monster = 1,
    Npc = 2,
    SummonOwn = 3,
    SummonOthers = 4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Outfit {
    pub look_type: u16,
    pub look_type_ex: u16,
    pub look_head: u8,
    pub look_body: u8,
    pub look_legs: u8,
    pub look_feet: u8,
    pub look_addons: u8,
    pub look_mount: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LightInfo {
    pub level: u8,
    pub color: u8,
}

impl LightInfo {
    pub fn new(level: u8, color: u8) -> Self {
        Self { level, color }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FindPathParams {
    pub full_path_search: bool,
    pub clear_sight: bool,
    pub allow_diagonal: bool,
    pub keep_distance: bool,
    pub max_search_dist: i32,
    pub min_target_dist: i32,
    pub max_target_dist: i32,
}

impl FindPathParams {
    pub fn default_follow() -> Self {
        Self {
            full_path_search: true,
            clear_sight: true,
            allow_diagonal: true,
            keep_distance: false,
            max_search_dist: 0,
            min_target_dist: -1,
            max_target_dist: -1,
        }
    }
}

pub const MAP_WALK_WIDTH: i32 = crate::map::MAX_VIEWPORT_X * 2 + 1;
pub const MAP_WALK_HEIGHT: i32 = crate::map::MAX_VIEWPORT_Y * 2 + 1;
pub const MAX_WALK_CACHE_WIDTH: i32 = (MAP_WALK_WIDTH - 1) / 2;
pub const MAX_WALK_CACHE_HEIGHT: i32 = (MAP_WALK_HEIGHT - 1) / 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CountBlock {
    pub total: i32,
    pub ticks: i64,
}

/// Shared state for every creature, matching creature.h protected fields verbatim.
#[derive(Debug)]
pub struct CreatureBase {
    pub id: CreatureId,
    pub position: Position,
    pub direction: Direction,

    pub health: i32,
    pub health_max: i32,
    pub base_speed: u32,
    pub var_speed: i32,
    pub drunkenness: u8,

    pub current_outfit: Outfit,
    pub default_outfit: Outfit,

    pub last_position: Position,
    pub internal_light: LightInfo,
    pub skull: Skull,

    pub master_id: Option<CreatureId>,
    pub follow_creature_id: Option<CreatureId>,
    pub attacked_creature_id: Option<CreatureId>,

    pub summon_ids: Vec<CreatureId>,

    pub conditions: Vec<Box<dyn Condition>>,

    pub events_list: Vec<String>,

    pub list_walk_dir: Vec<Direction>,
    pub last_step: u64,
    pub script_events_bit_field: u32,
    pub event_walk: u32,
    pub walk_update_ticks: u32,
    pub last_hit_creature_id: u32,
    pub block_count: u32,
    pub block_ticks: u32,
    pub last_step_cost: u32,

    /// local map walk cache: [row][col]
    pub local_map_cache: Vec<Vec<bool>>,

    pub is_internal_removed: bool,
    pub is_map_loaded: bool,
    pub is_updating_path: bool,
    pub creature_check: bool,
    pub in_check_creatures_vector: bool,
    pub skill_loss: bool,
    pub loot_drop: bool,
    pub cancel_next_walk: bool,
    pub has_follow_path: bool,
    pub force_update_follow_path: bool,
    pub hidden_health: bool,
    pub can_use_defense: bool,
    pub movement_blocked: bool,

    pub damage_map: std::collections::BTreeMap<u32, CountBlock>,
    pub reference_counter: u32,
}

impl CreatureBase {
    pub fn new(id: CreatureId, position: Position) -> Self {
        let rows = MAP_WALK_HEIGHT as usize;
        let cols = MAP_WALK_WIDTH as usize;
        Self {
            id,
            position,
            direction: Direction::South,
            health: 1000,
            health_max: 1000,
            base_speed: 220,
            var_speed: 0,
            drunkenness: 0,
            current_outfit: Outfit::default(),
            default_outfit: Outfit::default(),
            last_position: position,
            internal_light: LightInfo::default(),
            skull: Skull::None,
            master_id: None,
            follow_creature_id: None,
            attacked_creature_id: None,
            summon_ids: Vec::new(),
            conditions: Vec::new(),
            events_list: Vec::new(),
            list_walk_dir: Vec::new(),
            last_step: 0,
            script_events_bit_field: 0,
            event_walk: 0,
            walk_update_ticks: 0,
            last_hit_creature_id: 0,
            block_count: 0,
            block_ticks: 0,
            last_step_cost: 1,
            local_map_cache: vec![vec![false; cols]; rows],
            is_internal_removed: false,
            is_map_loaded: false,
            is_updating_path: false,
            creature_check: false,
            in_check_creatures_vector: false,
            skill_loss: true,
            loot_drop: true,
            cancel_next_walk: false,
            has_follow_path: false,
            force_update_follow_path: false,
            hidden_health: false,
            can_use_defense: true,
            movement_blocked: false,
            damage_map: std::collections::BTreeMap::new(),
            reference_counter: 0,
        }
    }

    pub fn get_speed(&self) -> i32 {
        self.base_speed as i32 + self.var_speed
    }

    /// Register a creature event by name. Mirrors `Creature::registerCreatureEvent`.
    pub fn register_creature_event(&mut self, name: &str) {
        use crate::events::registry::g_script_registry;
        let event_type = {
            let registry = g_script_registry().lock().unwrap();
            registry.creature_events.get_event_by_name(name, true).map(|e| e.event_type)
        };
        if let Some(etype) = event_type {
            if !self.events_list.iter().any(|n| n == name) {
                self.script_events_bit_field |= 1u32 << (etype as u32);
                self.events_list.push(name.to_owned());
            }
        }
    }

    /// Returns event names of a given type registered on this creature.
    pub fn get_creature_event_names(&self, etype: crate::events::creature::CreatureEventType) -> Vec<String> {
        use crate::events::registry::g_script_registry;
        if self.script_events_bit_field & (1u32 << etype as u32) == 0 {
            return Vec::new();
        }
        let registry = g_script_registry().lock().unwrap();
        self.events_list.iter()
            .filter_map(|name| {
                let ev = registry.creature_events.get_event_by_name(name, true)?;
                if ev.event_type == etype { Some(name.clone()) } else { None }
            })
            .collect()
    }

    pub fn is_summon(&self) -> bool {
        self.master_id.is_some()
    }

    pub fn is_invisible(&self) -> bool {
        use crate::combat::condition::ConditionType;
        self.conditions
            .iter()
            .any(|c| c.get_type() == ConditionType::Invisible)
    }

    pub fn has_condition(&self, condition_type: crate::combat::condition::ConditionType) -> bool {
        self.conditions.iter().any(|c| c.get_type() == condition_type)
    }

    pub fn get_condition(
        &self,
        condition_type: crate::combat::condition::ConditionType,
    ) -> Option<&dyn Condition> {
        self.conditions
            .iter()
            .find(|c| c.get_type() == condition_type)
            .map(|b| b.as_ref())
    }

    pub fn get_condition_mut(
        &mut self,
        condition_type: crate::combat::condition::ConditionType,
    ) -> Option<&mut Box<dyn Condition>> {
        self.conditions
            .iter_mut()
            .find(|c| c.get_type() == condition_type)
    }

    pub fn remove_condition_by_type(&mut self, condition_type: crate::combat::condition::ConditionType) {
        self.conditions.retain(|c| c.get_type() != condition_type);
    }

    pub fn is_immune_condition(&self, _condition_type: crate::combat::condition::ConditionType) -> bool {
        false // overridden by concrete creature types
    }

    pub fn has_been_attacked(&self, attacker_id: CreatureId) -> bool {
        self.damage_map.contains_key(&attacker_id)
    }

    pub fn add_damage_points(&mut self, attacker_id: CreatureId, damage: i32) {
        let entry = self.damage_map.entry(attacker_id).or_insert(CountBlock { total: 0, ticks: 0 });
        entry.total += damage;
        entry.ticks = crate::util::otsys_time();
    }

    pub fn get_damage_ratio(&self, attacker_id: CreatureId) -> f64 {
        let total: i32 = self.damage_map.values().map(|b| b.total).sum();
        if total == 0 {
            return 0.0;
        }
        let attacker_total = self.damage_map.get(&attacker_id).map(|b| b.total).unwrap_or(0);
        attacker_total as f64 / total as f64
    }
}

/// Top-level creature discriminant. Stored in `Game::creatures` by ID.
pub enum Creature {
    Player(Box<Player>),
    Monster(Box<Monster>),
    Npc(Box<Npc>),
}

impl Creature {
    pub fn base(&self) -> &CreatureBase {
        match self {
            Creature::Player(p) => &p.base,
            Creature::Monster(m) => &m.base,
            Creature::Npc(n) => &n.base,
        }
    }

    pub fn base_mut(&mut self) -> &mut CreatureBase {
        match self {
            Creature::Player(p) => &mut p.base,
            Creature::Monster(m) => &mut m.base,
            Creature::Npc(n) => &mut n.base,
        }
    }

    pub fn id(&self) -> CreatureId {
        self.base().id
    }

    pub fn position(&self) -> Position {
        self.base().position
    }

    pub fn as_player(&self) -> Option<&Player> {
        match self {
            Creature::Player(p) => Some(p),
            _ => None,
        }
    }

    pub fn as_player_mut(&mut self) -> Option<&mut Player> {
        match self {
            Creature::Player(p) => Some(p),
            _ => None,
        }
    }

    pub fn as_monster(&self) -> Option<&Monster> {
        match self {
            Creature::Monster(m) => Some(m),
            _ => None,
        }
    }

    pub fn as_monster_mut(&mut self) -> Option<&mut Monster> {
        match self {
            Creature::Monster(m) => Some(m),
            _ => None,
        }
    }

    pub fn as_npc(&self) -> Option<&Npc> {
        match self {
            Creature::Npc(n) => Some(n),
            _ => None,
        }
    }

    pub fn as_npc_mut(&mut self) -> Option<&mut Npc> {
        match self {
            Creature::Npc(n) => Some(n),
            _ => None,
        }
    }

    pub fn is_player(&self) -> bool {
        matches!(self, Creature::Player(_))
    }

    pub fn is_monster(&self) -> bool {
        matches!(self, Creature::Monster(_))
    }

    pub fn is_npc(&self) -> bool {
        matches!(self, Creature::Npc(_))
    }

    pub fn get_type(&self) -> CreatureType {
        match self {
            Creature::Player(_) => CreatureType::Player,
            Creature::Monster(_) => CreatureType::Monster,
            Creature::Npc(_) => CreatureType::Npc,
        }
    }

    pub fn get_speed(&self) -> i32 {
        self.base().get_speed()
    }

    pub fn get_health(&self) -> i32 {
        self.base().health
    }

    pub fn get_max_health(&self) -> i32 {
        self.base().health_max
    }

    pub fn is_summon(&self) -> bool {
        self.base().is_summon()
    }

    pub fn get_race(&self) -> RaceType {
        match self {
            Creature::Monster(m) => m.get_race(),
            _ => RaceType::None,
        }
    }

    pub fn get_armor(&self) -> i32 {
        match self {
            Creature::Monster(m) => m.get_armor(),
            _ => 0,
        }
    }

    pub fn get_defense(&self) -> i32 {
        match self {
            Creature::Monster(m) => m.get_defense(),
            _ => 0,
        }
    }

    pub fn is_attackable(&self) -> bool {
        match self {
            Creature::Monster(m) => m.is_attackable(),
            Creature::Player(_) => true,
            Creature::Npc(_) => false,
        }
    }

    pub fn get_skull(&self) -> Skull {
        self.base().skull
    }

    pub fn get_zone(&self) -> ZoneType {
        ZoneType::Normal // requires tile lookup in Game - concrete blocker: Game::getTile
    }

    pub fn is_invisible(&self) -> bool {
        self.base().is_invisible()
    }

    pub fn is_in_ghost_mode(&self) -> bool {
        match self {
            Creature::Player(p) => p.is_ghost_mode,
            _ => false,
        }
    }

    pub fn get_damage_immunities(&self) -> u32 {
        match self {
            Creature::Monster(m) => m.get_damage_immunities(),
            _ => 0,
        }
    }

    pub fn get_condition_immunities(&self) -> u32 {
        match self {
            Creature::Monster(m) => m.get_condition_immunities(),
            _ => 0,
        }
    }

    pub fn is_immune_combat(&self, combat_type: crate::combat::CombatType) -> bool {
        let immunities = self.get_damage_immunities();
        immunities & (combat_type as u32) != 0
    }

    pub fn is_immune_condition(&self, condition_type: crate::combat::condition::ConditionType) -> bool {
        let immunities = self.get_condition_immunities();
        immunities & (condition_type as u32) != 0
    }

    pub fn get_name(&self) -> &str {
        match self {
            Creature::Player(p) => &p.name,
            Creature::Monster(m) => m.get_name(),
            Creature::Npc(n) => n.get_name(),
        }
    }

    pub fn get_name_description(&self) -> String {
        match self {
            Creature::Player(p) => p.name.clone(),
            Creature::Monster(m) => format!("a {}", m.get_name()),
            Creature::Npc(n) => n.get_name().to_string(),
        }
    }

    /// Applies block / defense / armor / immunity reduction to `damage` in-place.
    /// Returns the block type that stopped the hit (or None if damage went through).
    /// Mirrors C++ `Creature::blockHit` verbatim.
    #[allow(clippy::too_many_arguments)]
    pub fn block_hit(
        &mut self,
        _attacker_id: Option<CreatureId>,
        combat_type: crate::combat::CombatType,
        damage: &mut i32,
        check_defense: bool,
        check_armor: bool,
        _field: bool,
        _ignore_resistances: bool,
    ) -> crate::combat::BlockType {
        use crate::combat::BlockType;
        let mut block_type = BlockType::None;
        let mut do_armor = check_armor;

        if self.is_immune_combat(combat_type) {
            *damage = 0;
            block_type = BlockType::Immunity;
        } else if check_defense || check_armor {
            let mut has_defense = false;
            {
                let base = self.base_mut();
                if base.block_count > 0 {
                    base.block_count -= 1;
                    has_defense = true;
                }
            }

            if check_defense && has_defense && self.base().can_use_defense {
                let defense = self.get_defense();
                *damage -= crate::util::uniform_random(defense as i64 / 2, defense as i64) as i32;
                if *damage <= 0 {
                    *damage = 0;
                    block_type = BlockType::Defense;
                    do_armor = false;
                }
            }

            if do_armor && block_type == BlockType::None {
                let armor = self.get_armor();
                if armor > 3 {
                    *damage -= crate::util::uniform_random(
                        armor as i64 / 2,
                        armor as i64 - (armor as i64 % 2 + 1),
                    ) as i32;
                } else if armor > 0 {
                    *damage -= 1;
                }
                if *damage <= 0 {
                    *damage = 0;
                    block_type = BlockType::Armor;
                }
            }
        }

        block_type
    }
}
