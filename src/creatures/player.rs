#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::creatures::{CreatureBase, CreatureId, LightInfo, Skull};
use crate::map::Position;

pub const PLAYER_MAX_SPEED: i32 = 1500;
pub const PLAYER_MIN_SPEED: i32 = 10;
pub const PLAYER_NAME_LENGTH: i32 = 25;
pub const MAX_PLAYER_SUMMONS: usize = 2;
pub const EXHAUSTION_TICKS: i32 = 60000;
pub const MINIMUM_SKILL_LEVEL: i16 = 10;

pub const SKILL_FIST: usize = 0;
pub const SKILL_CLUB: usize = 1;
pub const SKILL_SWORD: usize = 2;
pub const SKILL_AXE: usize = 3;
pub const SKILL_DISTANCE: usize = 4;
pub const SKILL_SHIELD: usize = 5;
pub const SKILL_FISHING: usize = 6;
pub const SKILL_LAST: usize = SKILL_FISHING;
pub const SKILL_COUNT: usize = SKILL_LAST + 1;

pub const SPECIALSKILL_CRITICALHITCHANCE: usize = 0;
pub const SPECIALSKILL_CRITICALHITAMOUNT: usize = 1;
pub const SPECIALSKILL_LIFELEECHCHANCE: usize = 2;
pub const SPECIALSKILL_LIFELEECHAMOUNT: usize = 3;
pub const SPECIALSKILL_MANALEECHCHANCE: usize = 4;
pub const SPECIALSKILL_MANALEECHAMOUNT: usize = 5;
pub const SPECIALSKILL_LAST: usize = SPECIALSKILL_MANALEECHAMOUNT;
pub const SPECIALSKILL_COUNT: usize = SPECIALSKILL_LAST + 1;

pub const STAT_MAXHITPOINTS: usize = 0;
pub const STAT_MAXMANAPOINTS: usize = 1;
pub const STAT_SOULPOINTS: usize = 2;
pub const STAT_MAGICPOINTS: usize = 3;
pub const STAT_LAST: usize = STAT_MAGICPOINTS;
pub const STAT_COUNT: usize = STAT_LAST + 1;

pub const CONST_SLOT_WHEREEVER: usize = 0;
pub const CONST_SLOT_HEAD: usize = 1;
pub const CONST_SLOT_NECKLACE: usize = 2;
pub const CONST_SLOT_BACKPACK: usize = 3;
pub const CONST_SLOT_ARMOR: usize = 4;
pub const CONST_SLOT_RIGHT: usize = 5;
pub const CONST_SLOT_LEFT: usize = 6;
pub const CONST_SLOT_LEGS: usize = 7;
pub const CONST_SLOT_FEET: usize = 8;
pub const CONST_SLOT_RING: usize = 9;
pub const CONST_SLOT_AMMO: usize = 10;
pub const CONST_SLOT_FIRST: usize = CONST_SLOT_HEAD;
pub const CONST_SLOT_LAST: usize = CONST_SLOT_AMMO;
pub const SLOT_COUNT: usize = CONST_SLOT_LAST + 1;

pub const PLAYER_FLAG_CANNOT_USE_COMBAT: u64 = 1 << 0;
pub const PLAYER_FLAG_CANNOT_ATTACK_PLAYER: u64 = 1 << 1;
pub const PLAYER_FLAG_CANNOT_ATTACK_MONSTER: u64 = 1 << 2;
pub const PLAYER_FLAG_CANNOT_BE_ATTACKED: u64 = 1 << 3;
pub const PLAYER_FLAG_CAN_CONVINCE_ALL: u64 = 1 << 4;
pub const PLAYER_FLAG_CAN_SUMMON_ALL: u64 = 1 << 5;
pub const PLAYER_FLAG_CAN_ILLUSION_ALL: u64 = 1 << 6;
pub const PLAYER_FLAG_CAN_SENSE_INVISIBILITY: u64 = 1 << 7;
pub const PLAYER_FLAG_IGNORED_BY_MONSTERS: u64 = 1 << 8;
pub const PLAYER_FLAG_NOT_GAIN_IN_FIGHT: u64 = 1 << 9;
pub const PLAYER_FLAG_HAS_INFINITE_MANA: u64 = 1 << 10;
pub const PLAYER_FLAG_HAS_INFINITE_SOUL: u64 = 1 << 11;
pub const PLAYER_FLAG_HAS_NO_EXHAUSTION: u64 = 1 << 12;
pub const PLAYER_FLAG_CANNOT_USE_SPELLS: u64 = 1 << 13;
pub const PLAYER_FLAG_CANNOT_PICKUP_ITEM: u64 = 1 << 14;
pub const PLAYER_FLAG_CAN_ALWAYS_LOGIN: u64 = 1 << 15;
pub const PLAYER_FLAG_CAN_BROADCAST: u64 = 1 << 16;
pub const PLAYER_FLAG_CAN_EDIT_HOUSES: u64 = 1 << 17;
pub const PLAYER_FLAG_CANNOT_BE_BANNED: u64 = 1 << 18;
pub const PLAYER_FLAG_CANNOT_BE_PUSHED: u64 = 1 << 19;
pub const PLAYER_FLAG_HAS_INFINITE_CAPACITY: u64 = 1 << 20;
pub const PLAYER_FLAG_CAN_PUSH_ALL_CREATURES: u64 = 1 << 21;
pub const PLAYER_FLAG_CAN_TALK_RED_PRIVATE: u64 = 1 << 22;
pub const PLAYER_FLAG_CAN_TALK_RED_CHANNEL: u64 = 1 << 23;
pub const PLAYER_FLAG_TALK_ORANGE_HELP_CHANNEL: u64 = 1 << 24;
pub const PLAYER_FLAG_NOT_GAIN_EXPERIENCE: u64 = 1 << 25;
pub const PLAYER_FLAG_NOT_GAIN_MANA: u64 = 1 << 26;
pub const PLAYER_FLAG_NOT_GAIN_HEALTH: u64 = 1 << 27;
pub const PLAYER_FLAG_NOT_GAIN_SKILL: u64 = 1 << 28;
pub const PLAYER_FLAG_SET_MAX_SPEED: u64 = 1 << 29;
pub const PLAYER_FLAG_SPECIAL_VIP: u64 = 1 << 30;
pub const PLAYER_FLAG_NOT_GENERATE_LOOT: u64 = 1u64 << 31;
pub const PLAYER_FLAG_CAN_TALK_RED_CHANNEL_ANONYMOUS: u64 = 1u64 << 32;
pub const PLAYER_FLAG_IGNORE_PROTECTION_ZONE: u64 = 1u64 << 33;
pub const PLAYER_FLAG_IGNORE_SPELL_CHECK: u64 = 1u64 << 34;
pub const PLAYER_FLAG_IGNORE_WEAPON_CHECK: u64 = 1u64 << 35;
pub const PLAYER_FLAG_CANNOT_BE_MUTED: u64 = 1u64 << 36;
pub const PLAYER_FLAG_IS_ALWAYS_PREMIUM: u64 = 1u64 << 37;
pub const PLAYER_FLAG_IGNORE_YELL_CHECK: u64 = 1u64 << 38;
pub const PLAYER_FLAG_IGNORE_SEND_PRIVATE_CHECK: u64 = 1u64 << 39;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayerSex {
    #[default]
    Female = 0,
    Male = 1,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum AccountType {
    #[default]
    Normal = 1,
    Tutor = 2,
    SeniorTutor = 3,
    GameMaster = 4,
    CommunityManager = 5,
    God = 6,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OperatingSystem {
    #[default]
    None = 0,
    Linux = 1,
    Windows = 2,
    Flash = 3,
    OtcLinux = 10,
    OtcWindows = 11,
    OtcMac = 12,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FightMode {
    #[default]
    Attack = 1,
    Balanced = 2,
    Defense = 3,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TradeState {
    #[default]
    None = 0,
    Initiated = 1,
    Accept = 2,
    Acknowledge = 3,
    Transfer = 4,
}

/// Where a player's currently-offered trade item lives, so the secure-trade
/// swap can remove it from the right place on accept. Rust has no `Item*`
/// pointers (unlike C++ `Player::tradeItem`), so we track the location.
#[derive(Debug, Clone)]
pub enum TradeItemLoc {
    Tile(Position, usize),
    Inventory(usize),
    Container(u8, usize),
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlockType {
    #[default]
    None = 0,
    Defense = 1,
    Armor = 2,
    Immunity = 3,
}

#[derive(Debug, Clone)]
pub struct Skill {
    pub tries: u64,
    pub level: u16,
    pub percent: u8,
}

impl Default for Skill {
    fn default() -> Self {
        Self {
            tries: 0,
            level: MINIMUM_SKILL_LEVEL as u16,
            percent: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VipEntry {
    pub guid: u32,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct OutfitEntry {
    pub look_type: u16,
    pub addons: u8,
}

#[derive(Debug, Clone)]
pub enum ContainerParent {
    Tile(Position, usize),
    Inventory(u8),
    Container(u8, usize),
    /// An open depot chest, keyed by depot/town id. Its contents live in
    /// `Player.depot_items[depot_id]`.
    Depot(u32),
}

#[derive(Debug, Clone)]
pub struct OpenContainer {
    pub parent: ContainerParent,
    pub scroll_index: u16,
}

#[derive(Debug, Clone, Default)]
pub struct ShopInfo {
    pub item_id: u32,
    pub sub_type: i32,
    pub buy_price: u32,
    pub sell_price: u32,
    pub real_name: String,
}

#[derive(Debug)]
pub struct Player {
    pub base: CreatureBase,
    pub name: String,
    pub guild_nick: String,

    pub guid: u32,
    pub account_number: u32,
    pub account_type: AccountType,
    pub group_flags: u64,
    pub group_access: bool,
    pub sex: PlayerSex,
    pub operating_system: OperatingSystem,

    pub vocation_id: u16,

    pub level: u32,
    pub level_percent: u8,
    pub mag_level: u32,
    pub mag_level_percent: u8,
    pub experience: u64,

    pub mana: u32,
    pub mana_max: u32,
    pub mana_spent: u64,

    pub soul: u8,
    pub stamina_minutes: u16,

    pub skills: [Skill; SKILL_COUNT],
    pub var_skills: [i32; SKILL_COUNT],
    pub var_special_skills: [i32; SPECIALSKILL_COUNT],
    pub var_stats: [i32; STAT_COUNT],

    pub inventory: [Option<u16>; SLOT_COUNT],
    pub inventory_count: [u16; SLOT_COUNT],
    pub inventory_abilities: [bool; SLOT_COUNT],
    pub inventory_weight: u32,
    pub capacity: u32,
    pub inventory_items: [Option<crate::map::tile::MapItem>; SLOT_COUNT],

    pub open_containers: BTreeMap<u8, OpenContainer>,
    pub depot_locker_map: HashMap<u32, u16>,
    pub depot_chests: HashMap<u32, u16>,
    /// Depot chest contents per depot/town id (the items stored in the depot).
    pub depot_items: HashMap<u32, Vec<crate::map::tile::MapItem>>,
    pub storage_map: HashMap<u32, i32>,

    pub outfits: Vec<OutfitEntry>,
    pub guild_war_vector: Vec<u32>,

    pub shop_item_list: Vec<ShopInfo>,
    pub invite_party_list: Vec<u32>,
    pub learned_instant_spells: Vec<String>,

    pub login_position: Position,
    pub last_walkthrough_position: Position,

    pub last_login_saved: i64,
    pub last_logout: i64,
    pub premium_ends_at: i64,

    pub bank_balance: u64,
    pub last_attack: u64,
    pub last_quest_log_update: u64,
    pub last_failed_follow: i64,
    pub skull_ticks: i64,
    pub last_walkthrough_attempt: i64,
    pub last_ping: i64,
    pub last_pong: i64,
    pub next_action: i64,

    pub bed_item_id: Option<u16>,
    pub guild_id: Option<u32>,
    pub guild_rank_id: Option<u32>,
    pub group_id: u32,
    pub trade_item_id: Option<u16>,
    pub trade_item: Option<crate::map::tile::MapItem>,
    pub trade_item_loc: Option<TradeItemLoc>,
    pub write_item_id: Option<u16>,
    pub write_item_pos: Option<crate::map::Position>,
    pub write_item_stack_pos: u8,
    pub edit_house_id: Option<u32>,
    pub shop_owner_id: Option<u32>,
    pub party_id: Option<u32>,
    pub trade_partner_id: Option<CreatureId>,
    pub town_id: u32,

    pub damage_immunities: u32,
    pub condition_immunities: u32,
    pub condition_suppressions: u32,
    pub blessings: u8,

    pub action_task_event: u32,
    pub next_step_event: u32,
    pub walk_task_event: u32,
    pub message_buffer_ticks: u32,
    pub last_ip: u32,
    pub window_text_id: u32,
    pub edit_list_id: u32,

    pub purchase_callback: i32,
    pub sale_callback: i32,
    pub message_buffer_count: i32,
    pub blood_hit_count: i32,
    pub shield_block_count: i32,
    pub idle_time: i32,

    pub max_write_len: u16,
    pub last_depot_id: i16,

    pub items_light: LightInfo,

    pub last_attack_block_type: BlockType,
    pub trade_state: TradeState,
    pub fight_mode: FightMode,

    pub chase_mode: bool,
    pub secure_mode: bool,
    pub is_ghost_mode: bool,
    pub pz_locked: bool,
    pub is_connecting: bool,
    pub add_attack_skill_point: bool,
    pub skill_loss: bool,

    pub attacked_set: HashSet<u32>,
    pub vip_list: HashSet<u32>,
}

impl Player {
    pub fn new(name: String, guid: u32) -> Self {
        let pos = Position::default();
        let base = CreatureBase::new(0, pos);
        Self {
            base,
            name,
            guild_nick: String::new(),
            guid,
            account_number: 0,
            account_type: AccountType::Normal,
            group_flags: 0,
            group_access: false,
            sex: PlayerSex::Female,
            operating_system: OperatingSystem::None,
            vocation_id: 0,
            level: 1,
            level_percent: 0,
            mag_level: 0,
            mag_level_percent: 0,
            experience: 0,
            mana: 0,
            mana_max: 0,
            mana_spent: 0,
            soul: 0,
            stamina_minutes: 2520,
            skills: core::array::from_fn(|_| Skill::default()),
            var_skills: [0; SKILL_COUNT],
            var_special_skills: [0; SPECIALSKILL_COUNT],
            var_stats: [0; STAT_COUNT],
            inventory: [None; SLOT_COUNT],
            inventory_count: [1; SLOT_COUNT],
            inventory_abilities: [false; SLOT_COUNT],
            inventory_weight: 0,
            capacity: 40000,
            inventory_items: core::array::from_fn(|_| None),
            open_containers: BTreeMap::new(),
            depot_locker_map: HashMap::new(),
            depot_chests: HashMap::new(),
            depot_items: HashMap::new(),
            storage_map: HashMap::new(),
            outfits: Vec::new(),
            guild_war_vector: Vec::new(),
            shop_item_list: Vec::new(),
            invite_party_list: Vec::new(),
            learned_instant_spells: Vec::new(),
            login_position: pos,
            last_walkthrough_position: pos,
            last_login_saved: 0,
            last_logout: 0,
            premium_ends_at: 0,
            bank_balance: 0,
            last_attack: 0,
            last_quest_log_update: 0,
            last_failed_follow: 0,
            skull_ticks: 0,
            last_walkthrough_attempt: 0,
            last_ping: 0,
            last_pong: 0,
            next_action: 0,
            bed_item_id: None,
            guild_id: None,
            guild_rank_id: None,
            group_id: 0,
            trade_item_id: None,
            trade_item: None,
            trade_item_loc: None,
            write_item_id: None,
            write_item_pos: None,
            write_item_stack_pos: 0,
            edit_house_id: None,
            shop_owner_id: None,
            party_id: None,
            trade_partner_id: None,
            town_id: 0,
            damage_immunities: 0,
            condition_immunities: 0,
            condition_suppressions: 0,
            blessings: 0,
            action_task_event: 0,
            next_step_event: 0,
            walk_task_event: 0,
            message_buffer_ticks: 0,
            last_ip: 0,
            window_text_id: 0,
            edit_list_id: 0,
            purchase_callback: -1,
            sale_callback: -1,
            message_buffer_count: 0,
            blood_hit_count: 0,
            shield_block_count: 0,
            idle_time: 0,
            max_write_len: 0,
            last_depot_id: -1,
            items_light: LightInfo::default(),
            last_attack_block_type: BlockType::None,
            trade_state: TradeState::None,
            fight_mode: FightMode::Attack,
            chase_mode: false,
            secure_mode: false,
            is_ghost_mode: false,
            pz_locked: false,
            is_connecting: false,
            add_attack_skill_point: false,
            skill_loss: true,
            attacked_set: HashSet::new(),
            vip_list: HashSet::new(),
        }
    }

    pub fn get_name(&self) -> &str {
        &self.name
    }

    pub fn get_guid(&self) -> u32 {
        self.guid
    }

    pub fn get_level(&self) -> u32 {
        self.level
    }

    pub fn get_level_percent(&self) -> u8 {
        self.level_percent
    }

    pub fn get_magic_level(&self) -> u32 {
        let total = self.mag_level as i32 + self.var_stats[STAT_MAGICPOINTS];
        total.max(0) as u32
    }

    pub fn get_base_magic_level(&self) -> u32 {
        self.mag_level
    }

    pub fn get_magic_level_percent(&self) -> u8 {
        self.mag_level_percent
    }

    pub fn get_max_health(&self) -> i32 {
        let base = self.base.health_max;
        let bonus = self.var_stats[STAT_MAXHITPOINTS];
        (base + bonus).max(1)
    }

    pub fn get_mana(&self) -> u32 {
        self.mana
    }

    pub fn get_max_mana(&self) -> u32 {
        let base = self.mana_max as i32;
        let bonus = self.var_stats[STAT_MAXMANAPOINTS];
        (base + bonus).max(0) as u32
    }

    pub fn get_soul(&self) -> u8 {
        self.soul
    }

    pub fn get_experience(&self) -> u64 {
        self.experience
    }

    pub fn get_stamina_minutes(&self) -> u16 {
        self.stamina_minutes
    }

    pub fn get_bank_balance(&self) -> u64 {
        self.bank_balance
    }

    pub fn get_money(&self) -> u64 {
        let mut total = 0u64;
        for slot in CONST_SLOT_FIRST..=CONST_SLOT_LAST {
            if let Some(sid) = self.inventory[slot] {
                let count = self.inventory_count[slot] as u64;
                total += match sid {
                    2148 => count,
                    2152 => count * 100,
                    2160 => count * 10_000,
                    _ => 0,
                };
            }
        }
        total
    }

    pub fn count_items_of_type(&self, item_id: u16, _sub_type: i32) -> u32 {
        let mut count = 0u32;
        for slot in CONST_SLOT_FIRST..=CONST_SLOT_LAST {
            if self.inventory[slot] == Some(item_id) {
                count += self.inventory_count[slot] as u32;
            }
        }
        count
    }

    pub fn set_bank_balance(&mut self, balance: u64) {
        self.bank_balance = balance;
    }

    pub fn get_vocation_id(&self) -> u16 {
        self.vocation_id
    }

    pub fn get_town_id(&self) -> u32 {
        self.town_id
    }

    pub fn get_sex(&self) -> PlayerSex {
        self.sex
    }

    pub fn get_account(&self) -> u32 {
        self.account_number
    }

    pub fn get_account_type(&self) -> AccountType {
        self.account_type
    }

    pub fn get_guild_id(&self) -> Option<u32> {
        self.guild_id
    }

    pub fn get_guild_nick(&self) -> &str {
        &self.guild_nick
    }

    pub fn get_group_id(&self) -> u32 {
        self.group_id
    }

    pub fn get_last_depot_id(&self) -> i16 {
        self.last_depot_id
    }

    pub fn set_last_depot_id(&mut self, id: i16) {
        self.last_depot_id = id;
    }

    pub fn is_in_ghost_mode(&self) -> bool {
        self.is_ghost_mode
    }

    pub fn switch_ghost_mode(&mut self) {
        self.is_ghost_mode = !self.is_ghost_mode;
    }

    pub fn is_pz_locked(&self) -> bool {
        self.pz_locked
    }

    pub fn has_secure_mode(&self) -> bool {
        self.secure_mode
    }

    pub fn get_skull_ticks(&self) -> i64 {
        self.skull_ticks
    }

    pub fn set_skull_ticks(&mut self, ticks: i64) {
        self.skull_ticks = ticks;
    }

    pub fn is_offline(&self) -> bool {
        self.base.id == 0
    }

    pub fn get_capacity(&self) -> u32 {
        if self.has_flag(PLAYER_FLAG_CANNOT_PICKUP_ITEM) {
            0
        } else if self.has_flag(PLAYER_FLAG_HAS_INFINITE_CAPACITY) {
            u32::MAX
        } else {
            self.capacity
        }
    }

    pub fn get_free_capacity(&self) -> u32 {
        if self.has_flag(PLAYER_FLAG_CANNOT_PICKUP_ITEM) {
            0
        } else if self.has_flag(PLAYER_FLAG_HAS_INFINITE_CAPACITY) {
            u32::MAX
        } else {
            (self.capacity as i32 - self.inventory_weight as i32).max(0) as u32
        }
    }

    pub fn has_flag(&self, flag: u64) -> bool {
        self.group_flags & flag != 0
    }

    pub fn get_var_stats(&self, stat: usize) -> i32 {
        self.var_stats[stat]
    }

    pub fn set_var_stats(&mut self, stat: usize, modifier: i32) {
        self.var_stats[stat] += modifier;
    }

    pub fn get_special_skill(&self, skill: usize) -> i32 {
        self.var_special_skills[skill].max(0)
    }

    pub fn get_skill_level(&self, skill: usize) -> u16 {
        let total = self.skills[skill].level as i32 + self.var_skills[skill];
        total.max(0) as u16
    }

    pub fn get_base_skill(&self, skill: usize) -> u16 {
        self.skills[skill].level
    }

    pub fn get_skill_percent(&self, skill: usize) -> u8 {
        self.skills[skill].percent
    }

    pub fn get_spent_mana(&self) -> u64 {
        self.mana_spent
    }

    pub fn add_blessing(&mut self, blessing: u8) {
        self.blessings |= 1 << blessing;
    }

    pub fn remove_blessing(&mut self, blessing: u8) {
        self.blessings &= !(1 << blessing);
    }

    pub fn has_blessing(&self, blessing: u8) -> bool {
        self.blessings & (1 << blessing) != 0
    }

    pub fn has_learned_instant_spell(&self, spell_name: &str) -> bool {
        self.learned_instant_spells.iter().any(|s| s == spell_name)
    }

    pub fn learn_instant_spell(&mut self, spell_name: &str) {
        if !self.has_learned_instant_spell(spell_name) {
            self.learned_instant_spells.push(spell_name.to_owned());
        }
    }

    pub fn forget_instant_spell(&mut self, spell_name: &str) {
        self.learned_instant_spells.retain(|s| s != spell_name);
    }

    pub fn has_attacked(&self, attacked_guid: u32) -> bool {
        self.attacked_set.contains(&attacked_guid)
    }

    pub fn add_attacked(&mut self, attacked_guid: u32) {
        self.attacked_set.insert(attacked_guid);
    }

    pub fn remove_attacked(&mut self, attacked_guid: u32) {
        self.attacked_set.remove(&attacked_guid);
    }

    pub fn clear_attacked(&mut self) {
        self.attacked_set.clear();
    }

    pub fn is_guild_mate(&self, other_guild_id: Option<u32>) -> bool {
        match (self.guild_id, other_guild_id) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }

    pub fn is_in_war(&self, other_guild_id: Option<u32>) -> bool {
        if let Some(gid) = other_guild_id {
            self.guild_war_vector.contains(&gid)
        } else {
            false
        }
    }

    pub fn get_attack_factor(&self) -> f32 {
        match self.fight_mode {
            FightMode::Attack => 1.0,
            FightMode::Balanced => 1.2,
            FightMode::Defense => 2.0,
        }
    }

    pub fn get_defense_factor(&self) -> f32 {
        use crate::util::otsys_time;
        let elapsed = otsys_time() - self.last_attack as i64;
        let attack_speed = self.get_attack_speed() as i64;
        match self.fight_mode {
            FightMode::Defense => 1.0,
            FightMode::Attack => if elapsed < attack_speed { 0.5 } else { 1.0 },
            FightMode::Balanced => if elapsed < attack_speed { 0.75 } else { 1.0 },
        }
    }

    pub fn get_attack_speed(&self) -> u32 {
        use crate::items::g_items;
        let weapon_speed = self.get_weapon()
            .map(|id| g_items().get_item_type(id as usize).attack_speed)
            .unwrap_or(0);
        if weapon_speed > 0 {
            return weapon_speed;
        }
        crate::world::vocation::g_vocations()
            .get_vocation(self.vocation_id)
            .map(|v| v.attack_speed)
            .unwrap_or(1500)
    }

    /// addSkillAdvance — port of Player::addSkillAdvance from player.cpp.
    pub fn add_skill_advance(&mut self, skill: usize, count: u64) -> bool {
        use crate::world::vocation::g_vocations;
        if skill >= SKILL_COUNT {
            return false;
        }
        let voc = match g_vocations().get_vocation(self.vocation_id) {
            Some(v) => v.clone(),
            None => return false,
        };
        let curr_req = voc.get_req_skill_tries(skill as u8, self.skills[skill].level);
        let next_req = voc.get_req_skill_tries(skill as u8, self.skills[skill].level + 1);
        if curr_req >= next_req {
            return false; // vocation doesn't advance this skill
        }
        self.skills[skill].tries += count;
        let mut new_level = self.skills[skill].level;
        loop {
            let req_next = voc.get_req_skill_tries(skill as u8, new_level + 1);
            if self.skills[skill].tries < req_next {
                break;
            }
            self.skills[skill].tries -= req_next;
            new_level += 1;
        }
        let leveled_up = new_level != self.skills[skill].level;
        if leveled_up {
            self.skills[skill].level = new_level;
            self.skills[skill].percent = 0;
        }
        let next_req2 = voc.get_req_skill_tries(skill as u8, self.skills[skill].level + 1);
        self.skills[skill].percent = Self::get_percent_level(self.skills[skill].tries, next_req2);
        leveled_up
    }

    /// Accumulate mana spent and level up magic if threshold met.
    /// Returns new magic level if leveled, else None.
    pub fn add_mana_spent(&mut self, amount: u64) -> Option<u32> {
        use crate::world::vocation::g_vocations;
        let voc = g_vocations().get_vocation(self.vocation_id)?.clone();
        let next_req = voc.get_req_mana(self.mag_level + 1);
        if next_req == 0 {
            return None; // max magic level for vocation
        }
        self.mana_spent += amount;
        if self.mana_spent >= next_req {
            self.mana_spent -= next_req;
            self.mag_level += 1;
            let next2 = voc.get_req_mana(self.mag_level + 1);
            self.mag_level_percent = Self::get_percent_level(self.mana_spent, next2);
            return Some(self.mag_level);
        }
        let next2 = voc.get_req_mana(self.mag_level + 1);
        self.mag_level_percent = Self::get_percent_level(self.mana_spent, next2);
        None
    }

    pub fn get_weapon(&self) -> Option<u16> {
        use crate::items::g_items;
        let items = g_items();
        for &slot in &[CONST_SLOT_LEFT, CONST_SLOT_RIGHT] {
            if let Some(server_id) = self.inventory[slot] {
                let wt = items.get_item_type(server_id as usize).weapon_type;
                if wt != 0 && wt != 4 { // 4 = shield
                    return Some(server_id);
                }
            }
        }
        None
    }

    pub fn get_weapon_skill(&self, item_id: Option<u16>) -> u32 {
        use crate::items::g_items;
        let weapon_type = item_id
            .map(|id| g_items().get_item_type(id as usize).weapon_type)
            .unwrap_or(0);
        (match weapon_type {
            1 => self.get_skill_level(SKILL_SWORD), // sword
            2 => self.get_skill_level(SKILL_CLUB),  // club
            3 => self.get_skill_level(SKILL_AXE),   // axe
            5 => self.get_skill_level(SKILL_DISTANCE), // distance
            _ => self.get_skill_level(SKILL_FIST),  // fist / unknown
        }) as u32
    }

    pub fn get_weapon_type(&self) -> u8 {
        use crate::items::g_items;
        let items = g_items();
        for &slot in &[CONST_SLOT_LEFT, CONST_SLOT_RIGHT] {
            if let Some(server_id) = self.inventory[slot] {
                let wt = items.get_item_type(server_id as usize).weapon_type;
                if wt != 0 && wt != 4 {
                    return wt;
                }
            }
        }
        0
    }

    pub fn get_armor(&self) -> i32 {
        use crate::items::g_items;
        use crate::world::vocation::g_vocations;
        let armor_slots = [
            CONST_SLOT_HEAD, CONST_SLOT_NECKLACE, CONST_SLOT_ARMOR,
            CONST_SLOT_LEGS, CONST_SLOT_FEET, CONST_SLOT_RING,
        ];
        let mut armor = 0i32;
        let items = g_items();
        for &slot in &armor_slots {
            if let Some(id) = self.inventory[slot] {
                armor += items.get_item_type(id as usize).armor;
            }
        }
        let multiplier = g_vocations()
            .get_vocation(self.vocation_id)
            .map(|v| v.armor_multiplier)
            .unwrap_or(1.0);
        (armor as f32 * multiplier) as i32
    }

    pub fn get_defense(&self) -> i32 {
        use crate::items::g_items;
        use crate::world::vocation::g_vocations;
        let items = g_items();
        let mut shield_defense = 0i32;
        let mut weapon_defense = 0i32;
        let mut weapon_extra_defense = 0i32;
        let mut defense_skill = self.get_skill_level(SKILL_FIST) as i32;
        let mut defense_value = 7i32;

        // Find shield (weapon_type == 4) and weapon in RIGHT/LEFT slots.
        let mut found_weapon = false;
        let mut found_shield = false;
        for &slot in &[CONST_SLOT_RIGHT, CONST_SLOT_LEFT] {
            if let Some(id) = self.inventory[slot] {
                let it = items.get_item_type(id as usize);
                if it.weapon_type == 4 {
                    // Shield
                    if !found_shield || it.defense > shield_defense {
                        shield_defense = it.defense;
                        found_shield = true;
                    }
                } else if it.weapon_type != 0 && !found_weapon {
                    weapon_defense = it.defense;
                    weapon_extra_defense = it.extra_defense;
                    defense_skill = self.get_weapon_skill(Some(id)) as i32;
                    found_weapon = true;
                }
            }
        }

        if found_weapon {
            defense_value = weapon_defense + weapon_extra_defense;
        }
        if found_shield {
            defense_value = if found_weapon {
                shield_defense + weapon_extra_defense
            } else {
                shield_defense
            };
            defense_skill = self.get_skill_level(SKILL_SHIELD) as i32;
        }

        if defense_skill == 0 {
            return match self.fight_mode {
                FightMode::Defense => 2,
                _ => 1,
            };
        }

        let defense_factor = self.get_defense_factor();
        let def_multiplier = g_vocations()
            .get_vocation(self.vocation_id)
            .map(|v| v.defense_multiplier)
            .unwrap_or(1.0);
        ((defense_skill as f64 / 4.0 + 2.23) * defense_value as f64 * 0.15 * defense_factor as f64 * def_multiplier as f64) as i32
    }

    pub fn is_premium(&self) -> bool {
        // Mirrors C++ Player::isPremium: always-premium config/flag, or active
        // premium time. PLAYER_FLAG_IS_ALWAYS_PREMIUM covers staff groups.
        if crate::config::g_config().get_boolean(crate::config::BooleanConfig::FreePremium)
            || self.has_flag(PLAYER_FLAG_IS_ALWAYS_PREMIUM)
        {
            return true;
        }
        let now = crate::util::get_milliseconds_time() / 1000;
        self.premium_ends_at > now
    }

    pub fn is_pushable(&self) -> bool {
        if self.has_flag(PLAYER_FLAG_CANNOT_BE_PUSHED) {
            return false;
        }
        !self.base.has_condition(crate::combat::condition::ConditionType::InFight)
    }

    pub fn update_base_speed(&mut self, vocation_base_speed: u32) {
        if self.has_flag(PLAYER_FLAG_SET_MAX_SPEED) {
            self.base.base_speed = PLAYER_MAX_SPEED as u32;
        } else {
            self.base.base_speed = vocation_base_speed + (2 * self.level.saturating_sub(1));
        }
    }

    pub fn get_step_speed(&self) -> i32 {
        let s = self.base.get_speed();
        s.clamp(PLAYER_MIN_SPEED, PLAYER_MAX_SPEED)
    }

    pub fn can_see_invisibility(&self) -> bool {
        self.has_flag(PLAYER_FLAG_CAN_SENSE_INVISIBILITY) || self.group_access
    }

    /// Creature light as seen by the client: the brighter of the player's own
    /// internal light and the aggregate light of equipped items. Mirrors C++
    /// `Player::getCreatureLight`.
    pub fn get_creature_light(&self) -> crate::creatures::LightInfo {
        if self.base.internal_light.level >= self.items_light.level {
            self.base.internal_light
        } else {
            self.items_light
        }
    }

    /// Recompute the aggregate light emitted by equipped items (the brightest
    /// equipped item wins). Returns true if it changed. Mirrors C++
    /// `Player::updateItemsLight`.
    pub fn update_items_light(&mut self) -> bool {
        use crate::items::g_items;
        let items = g_items();
        let mut max_light = crate::creatures::LightInfo::default();
        for slot in CONST_SLOT_FIRST..=CONST_SLOT_LAST {
            if let Some(sid) = self.inventory[slot] {
                let it = items.get_item_type(sid as usize);
                if it.light_level > max_light.level {
                    max_light = crate::creatures::LightInfo { level: it.light_level, color: it.light_color };
                }
            }
        }
        if max_light.level != self.items_light.level || max_light.color != self.items_light.color {
            self.items_light = max_light;
            true
        } else {
            false
        }
    }

    pub fn is_access_player(&self) -> bool {
        self.group_access
    }

    pub fn send_stats(&self) {
        crate::net::game_protocol::send_stats_to_player(self.base.id);
    }

    pub fn send_skills(&self) {
        crate::net::game_protocol::send_skills_to_player(self.base.id);
    }

    pub fn reset_idle_time(&mut self) {
        self.idle_time = 0;
    }

    pub fn can_do_action(&self) -> bool {
        self.next_action <= crate::util::otsys_time()
    }

    pub fn set_next_action(&mut self, time: i64) {
        if time > self.next_action {
            self.next_action = time;
        }
    }

    pub fn is_near_depot_box(&self) -> bool {
        let game = crate::game::g_game().lock().unwrap();
        let pos = self.base.position;
        for dx in -1i16..=1 {
            for dy in -1i16..=1 {
                let check_pos = crate::map::Position {
                    x: (pos.x as i16 + dx) as u16,
                    y: (pos.y as i16 + dy) as u16,
                    z: pos.z,
                };
                if let Some(tile) = game.map.get_tile(check_pos) {
                    for item in &tile.items {
                        let items = crate::items::g_items();
                        let it = items.get_item_type(item.server_id as usize);
                        if it.kind == crate::items::ItemKind::Depot {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    pub fn can_open_corpse(&self, owner_id: u32) -> bool {
        if self.has_flag(PLAYER_FLAG_NOT_GAIN_EXPERIENCE) {
            return true;
        }
        if self.base.id == owner_id {
            return true;
        }
        if let Some(pid) = self.party_id {
            let game = crate::game::g_game().lock().unwrap();
            if let Some(owner) = game.get_player(owner_id) {
                if owner.party_id == Some(pid) {
                    return true;
                }
            }
        }
        false
    }

    pub fn add_storage_value(&mut self, key: u32, value: i32) {
        self.storage_map.insert(key, value);
    }

    pub fn get_storage_value(&self, key: u32) -> Option<i32> {
        self.storage_map.get(&key).copied()
    }

    pub fn get_inventory_item(&self, slot: usize) -> Option<u16> {
        if slot < SLOT_COUNT {
            self.inventory[slot]
        } else {
            None
        }
    }

    pub fn is_item_ability_enabled(&self, slot: usize) -> bool {
        if slot < SLOT_COUNT {
            self.inventory_abilities[slot]
        } else {
            false
        }
    }

    pub fn set_item_ability(&mut self, slot: usize, enabled: bool) {
        if slot < SLOT_COUNT {
            self.inventory_abilities[slot] = enabled;
        }
    }

    pub fn set_var_skill(&mut self, skill: usize, modifier: i32) {
        self.var_skills[skill] += modifier;
    }

    pub fn set_var_special_skill(&mut self, skill: usize, modifier: i32) {
        self.var_special_skills[skill] += modifier;
    }

    pub fn get_exp_for_level(lv: u64) -> u64 {
        (((lv - 6) * lv + 17) * lv - 12) / 6 * 100
    }

    pub fn get_percent_level(current: u64, next_level_exp: u64) -> u8 {
        if next_level_exp == 0 { return 0; }
        ((current * 100 / next_level_exp).min(100)) as u8
    }

    pub fn get_last_login_saved(&self) -> i64 {
        self.last_login_saved
    }

    pub fn get_last_logout(&self) -> i64 {
        self.last_logout
    }

    pub fn get_login_position(&self) -> Position {
        self.login_position
    }

    pub fn set_guild(&mut self, guild_id: Option<u32>) {
        self.guild_id = guild_id;
    }

    pub fn set_guild_rank(&mut self, rank_id: Option<u32>) {
        self.guild_rank_id = rank_id;
    }

    pub fn set_guild_nick(&mut self, nick: String) {
        self.guild_nick = nick;
    }

    pub fn get_skull_client(&self, other_skull: Skull) -> Skull {
        other_skull
    }

    pub fn set_chase_mode(&mut self, mode: bool) {
        self.chase_mode = mode;
    }

    pub fn set_fight_mode(&mut self, mode: FightMode) {
        self.fight_mode = mode;
    }

    pub fn set_secure_mode(&mut self, mode: bool) {
        self.secure_mode = mode;
    }

    pub fn get_operating_system(&self) -> OperatingSystem {
        self.operating_system
    }

    pub fn set_operating_system(&mut self, os: OperatingSystem) {
        self.operating_system = os;
    }

    pub fn get_trade_state(&self) -> TradeState {
        self.trade_state
    }

    pub fn set_trade_state(&mut self, state: TradeState) {
        self.trade_state = state;
    }

    pub fn add_container(&mut self, cid: u8, parent: ContainerParent) {
        self.open_containers.insert(cid, OpenContainer {
            parent,
            scroll_index: 0,
        });
    }

    pub fn close_container(&mut self, cid: u8) {
        self.open_containers.remove(&cid);
    }

    pub fn get_container_by_id(&self, cid: u8) -> Option<&OpenContainer> {
        self.open_containers.get(&cid)
    }

    pub fn get_free_container_id(&self) -> Option<u8> {
        (0u8..16).find(|&cid| !self.open_containers.contains_key(&cid))
    }

    pub fn get_container_id_by_tile(&self, pos: Position, item_index: usize) -> Option<u8> {
        for (&cid, oc) in &self.open_containers {
            if let ContainerParent::Tile(p, idx) = &oc.parent {
                if *p == pos && *idx == item_index {
                    return Some(cid);
                }
            }
        }
        None
    }

    pub fn get_container_id_by_inventory(&self, slot: u8) -> Option<u8> {
        for (&cid, oc) in &self.open_containers {
            if let ContainerParent::Inventory(s) = &oc.parent {
                if *s == slot {
                    return Some(cid);
                }
            }
        }
        None
    }
}

pub fn resolve_container_server_id(game: &crate::game::Game, oc: &OpenContainer) -> u16 {
    match &oc.parent {
        ContainerParent::Tile(pos, idx) => {
            if let Some(tile) = game.map.get_tile(*pos) {
                tile.items.get(*idx).map(|item| item.server_id).unwrap_or(0)
            } else {
                0
            }
        }
        ContainerParent::Inventory(_slot) => {
            0
        }
        ContainerParent::Container(_parent_cid, _child_idx) => {
            0
        }
        ContainerParent::Depot(_depot_id) => {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Player, PLAYER_FLAG_SET_MAX_SPEED, PLAYER_MAX_SPEED};

    #[test]
    fn update_base_speed_uses_vocation_formula_without_max_speed_flag() {
        let mut player = Player::new(String::from("Tester"), 1);
        player.level = 8;

        player.update_base_speed(220);

        assert_eq!(player.base.base_speed, 234);
    }

    #[test]
    fn update_base_speed_caps_flagged_players_at_max_speed() {
        let mut player = Player::new(String::from("Tester"), 1);
        player.level = 200;
        player.group_flags = PLAYER_FLAG_SET_MAX_SPEED;

        player.update_base_speed(220);

        assert_eq!(player.base.base_speed, PLAYER_MAX_SPEED as u32);
    }
}
