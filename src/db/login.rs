use std::collections::HashMap;

use sha1::{Digest, Sha1};

use crate::config::{g_config, IntegerConfig};
use crate::creatures::player::{
    AccountType, Player, PlayerSex, Skill, CONST_SLOT_FIRST, CONST_SLOT_LAST, SKILL_COUNT,
    SLOT_COUNT,
};
use crate::creatures::{Direction, Outfit, Skull};
use crate::db::{g_database, DatabaseEngine};
use crate::map::Position;
use crate::util::get_milliseconds_time;
use crate::world::groups::flags_for_group_id;

pub struct CharacterEntry {
    pub name: String,
    pub world_ip: u32,
    pub world_port: u16,
}

pub struct Account {
    pub id: u32,
    pub characters: Vec<CharacterEntry>,
    pub premium_days: u16,
}

pub enum LoginResult {
    Success(Account),
    WrongPassword,
    NotFound,
    Error(String),
}

/// Serializable snapshot of a player's state for async DB save.
/// All fields that `save_player` needs, no non-Send/non-Clone types.
#[derive(Clone)]
pub struct PlayerSaveSnapshot {
    pub guid: u32,
    pub group_id: u32,
    pub vocation_id: u16,
    pub sex: PlayerSex,
    pub level: u32,
    pub experience: u64,
    pub health: i32,
    pub health_max: i32,
    pub mana: u32,
    pub mana_max: u32,
    pub mana_spent: u64,
    pub mag_level: u32,
    pub soul: u8,
    pub capacity: u32,
    pub stamina_minutes: u16,
    pub bank_balance: u64,
    pub blessings: u8,
    pub last_login_saved: i64,
    pub last_logout: i64,
    pub last_ip: u32,
    pub skull: Skull,
    pub skull_ticks: i64,
    pub town_id: u32,
    pub login_position: Position,
    pub direction: Direction,
    pub default_outfit: Outfit,
    pub skills: [Skill; SKILL_COUNT],
    pub inventory: [Option<u16>; SLOT_COUNT],
    pub inventory_count: [u16; SLOT_COUNT],
    pub inventory_items: Vec<(usize, crate::map::tile::MapItem)>,
    pub depot_items: Vec<(u32, crate::map::tile::MapItem)>,
    pub learned_instant_spells: Vec<String>,
    pub storage_map: HashMap<u32, i32>,
}

impl PlayerSaveSnapshot {
    pub fn from_player(p: &Player) -> Self {
        Self {
            guid: p.guid,
            group_id: p.group_id,
            vocation_id: p.vocation_id,
            sex: p.sex,
            level: p.level,
            experience: p.experience,
            health: p.base.health,
            health_max: p.base.health_max,
            mana: p.mana,
            mana_max: p.mana_max,
            mana_spent: p.mana_spent,
            mag_level: p.mag_level,
            soul: p.soul,
            capacity: p.capacity,
            stamina_minutes: p.stamina_minutes,
            bank_balance: p.bank_balance,
            blessings: p.blessings,
            last_login_saved: p.last_login_saved,
            last_logout: p.last_logout,
            last_ip: p.last_ip,
            skull: p.base.skull,
            skull_ticks: p.skull_ticks,
            town_id: p.town_id,
            login_position: p.login_position,
            direction: p.base.direction,
            default_outfit: p.base.default_outfit,
            skills: p.skills.clone(),
            inventory: p.inventory,
            inventory_count: p.inventory_count,
            inventory_items: p.inventory_items.iter().enumerate()
                .filter_map(|(slot, opt)| opt.as_ref().map(|m| (slot, m.clone())))
                .collect(),
            depot_items: p.depot_items.iter()
                .flat_map(|(&depot_id, items)| items.iter().map(move |m| (depot_id, m.clone())))
                .collect(),
            learned_instant_spells: p.learned_instant_spells.clone(),
            storage_map: p.storage_map.clone(),
        }
    }
}

pub fn sha1_hex(input: &[u8]) -> String {
    let result = Sha1::digest(input);
    result.iter().map(|b| format!("{b:02x}")).collect()
}

pub async fn loginserver_authentication(account_name: &str, password: &str) -> LoginResult {
    let db = g_database();
    let escaped = db.escape_string(account_name);
    let query = format!(
        "SELECT `id`, `password`, `premium_ends_at` FROM `accounts` WHERE `name` = {escaped}"
    );

    let result = match db.store_query(&query).await {
        Ok(Some(r)) => r,
        Ok(None) => return LoginResult::NotFound,
        Err(e) => return LoginResult::Error(e.to_string()),
    };

    let stored = match result.get_string("password") {
        Some(p) => p,
        None => return LoginResult::Error("missing password column".into()),
    };

    if sha1_hex(password.as_bytes()) != stored {
        return LoginResult::WrongPassword;
    }

    let account_id = result.get_u64("id").unwrap_or(0) as u32;
    let premium_ends_at = result.get_i64("premium_ends_at").unwrap_or(0);

    let world_ip = g_config().get_number(IntegerConfig::Ip) as u32;
    let world_port = g_config().get_number(IntegerConfig::GamePort) as u16;

    let query = format!(
        "SELECT `name` FROM `players` \
         WHERE `account_id` = {account_id} AND `deletion` = 0 ORDER BY `name`"
    );

    let characters = match db.store_query(&query).await {
        Ok(Some(mut rows)) => {
            let mut chars = Vec::new();
            loop {
                if let Some(name) = rows.get_string("name") {
                    chars.push(CharacterEntry { name, world_ip, world_port });
                }
                if !rows.next() {
                    break;
                }
            }
            chars
        }
        Ok(None) => Vec::new(),
        Err(e) => return LoginResult::Error(e.to_string()),
    };

    let now = get_milliseconds_time() / 1000;
    let premium_days = if premium_ends_at > now {
        ((premium_ends_at - now) / 86400).min(0xFFFE) as u16
    } else {
        0
    };

    LoginResult::Success(Account { id: account_id, characters, premium_days })
}

pub async fn gameworld_authentication(
    account_name: &str,
    password: &str,
    character_name: &str,
) -> bool {
    let db = g_database();
    let escaped = db.escape_string(account_name);
    let query = format!(
        "SELECT `id`, `password` FROM `accounts` WHERE `name` = {escaped}"
    );

    let result = match db.store_query(&query).await {
        Ok(Some(r)) => r,
        _ => return false,
    };

    let stored = match result.get_string("password") {
        Some(p) => p,
        None => return false,
    };

    if sha1_hex(password.as_bytes()) != stored {
        return false;
    }

    let account_id = result.get_u64("id").unwrap_or(0);
    let escaped_char = db.escape_string(character_name);
    let query = format!(
        "SELECT `id` FROM `players` \
         WHERE `name` = {escaped_char} AND `account_id` = {account_id} AND `deletion` = 0"
    );

    matches!(db.store_query(&query).await, Ok(Some(_)))
}

/// Load a player's character data from the `players` table by character name.
/// Returns `None` if not found or DB error.
pub async fn load_player_by_name(char_name: &str) -> Option<Player> {
    let db = g_database();
    let escaped = db.escape_string(char_name);
    let query = format!(
        "SELECT `id`, `name`, `account_id`, `group_id`, `sex`, `vocation`, \
         `experience`, `level`, `maglevel`, `health`, `healthmax`, `blessings`, \
         `mana`, `manamax`, `manaspent`, `soul`, \
         `lookbody`, `lookfeet`, `lookhead`, `looklegs`, `looktype`, `lookaddons`, \
         `posx`, `posy`, `posz`, `cap`, `lastlogin`, `lastlogout`, `lastip`, \
         `skulltime`, `skull`, `town_id`, `balance`, `stamina`, \
         `skill_fist`, `skill_fist_tries`, \
         `skill_club`, `skill_club_tries`, \
         `skill_sword`, `skill_sword_tries`, \
         `skill_axe`, `skill_axe_tries`, \
         `skill_dist`, `skill_dist_tries`, \
         `skill_shielding`, `skill_shielding_tries`, \
         `skill_fishing`, `skill_fishing_tries`, \
         `direction` \
         FROM `players` WHERE `name` = {escaped} AND `deletion` = 0"
    );

    let result = match db.store_query(&query).await {
        Ok(Some(r)) => r,
        _ => return None,
    };

    let guid = result.get_u64("id")? as u32;
    let name = result.get_string("name")?;
    let account_number = result.get_u64("account_id").unwrap_or(0) as u32;
    let group_id = result.get_u64("group_id").unwrap_or(1) as u32;

    let sex = match result.get_u64("sex").unwrap_or(0) {
        1 => PlayerSex::Male,
        _ => PlayerSex::Female,
    };

    let vocation_id = result.get_u64("vocation").unwrap_or(0) as u16;

    let level = (result.get_u64("level").unwrap_or(1) as u32).max(1);
    let experience = result.get_u64("experience").unwrap_or(0);

    let health = result.get_i64("health").unwrap_or(100) as i32;
    let health_max = result.get_i64("healthmax").unwrap_or(100) as i32;
    let blessings = result.get_u64("blessings").unwrap_or(0) as u8;

    let mana = result.get_u64("mana").unwrap_or(0) as u32;
    let mana_max = result.get_u64("manamax").unwrap_or(0) as u32;
    let mana_spent = result.get_u64("manaspent").unwrap_or(0);
    let mag_level = result.get_u64("maglevel").unwrap_or(0) as u32;
    let soul = (result.get_u64("soul").unwrap_or(0) as u8).min(200);

    let look_type = result.get_u64("looktype").unwrap_or(136) as u16;
    let look_head = result.get_u64("lookhead").unwrap_or(0) as u8;
    let look_body = result.get_u64("lookbody").unwrap_or(0) as u8;
    let look_legs = result.get_u64("looklegs").unwrap_or(0) as u8;
    let look_feet = result.get_u64("lookfeet").unwrap_or(0) as u8;
    let look_addons = result.get_u64("lookaddons").unwrap_or(0) as u8;

    let pos_x = result.get_u64("posx").unwrap_or(0) as u16;
    let pos_y = result.get_u64("posy").unwrap_or(0) as u16;
    let pos_z = result.get_u64("posz").unwrap_or(7) as u8;

    let capacity = result.get_u64("cap").unwrap_or(400) as u32 * 100;
    let last_login_saved = result.get_i64("lastlogin").unwrap_or(0);
    let last_logout = result.get_i64("lastlogout").unwrap_or(0);
    let last_ip = result.get_u64("lastip").unwrap_or(0) as u32;

    let skull_raw = result.get_u64("skull").unwrap_or(0) as u8;
    let skulltime = result.get_i64("skulltime").unwrap_or(0);
    let now = get_milliseconds_time() / 1000;
    use crate::creatures::Skull;
    let skull = if skulltime > now {
        match skull_raw {
            4 => Skull::Red,
            5 => Skull::Black,
            _ => Skull::None,
        }
    } else {
        Skull::None
    };

    let town_id = result.get_u64("town_id").unwrap_or(1) as u32;
    let bank_balance = result.get_u64("balance").unwrap_or(0);
    let stamina_minutes = result.get_u64("stamina").unwrap_or(2520) as u16;

    let direction = match result.get_u64("direction").unwrap_or(2) {
        0 => Direction::North,
        1 => Direction::East,
        3 => Direction::West,
        4 => Direction::SouthWest,
        5 => Direction::SouthEast,
        6 => Direction::NorthWest,
        7 => Direction::NorthEast,
        _ => Direction::South,
    };

    let skill_names = [
        ("skill_fist", "skill_fist_tries"),
        ("skill_club", "skill_club_tries"),
        ("skill_sword", "skill_sword_tries"),
        ("skill_axe", "skill_axe_tries"),
        ("skill_dist", "skill_dist_tries"),
        ("skill_shielding", "skill_shielding_tries"),
        ("skill_fishing", "skill_fishing_tries"),
    ];
    let mut skills: [Skill; SKILL_COUNT] = core::array::from_fn(|_| Skill::default());
    for (i, (skill_col, tries_col)) in skill_names.iter().enumerate() {
        let level = result.get_u64(skill_col).unwrap_or(10) as u16;
        let tries = result.get_u64(tries_col).unwrap_or(0);
        skills[i] = Skill { level, tries, percent: 0 };
    }

    let account_type = match result.get_u64("group_id").unwrap_or(1) as u32 {
        2 => AccountType::Tutor,
        3 => AccountType::SeniorTutor,
        4 => AccountType::GameMaster,
        5 => AccountType::CommunityManager,
        6 => AccountType::God,
        _ => AccountType::Normal,
    };

    use crate::creatures::Outfit;
    let login_pos = Position { x: pos_x, y: pos_y, z: pos_z };

    let mut player = Player::new(name, guid);
    player.account_number = account_number;
    player.account_type = account_type;
    player.group_id = group_id;
    player.group_flags = flags_for_group_id(group_id);
    player.group_access = crate::world::groups::access_for_group_id(group_id);
    player.sex = sex;
    player.vocation_id = vocation_id;
    player.level = level;
    player.update_base_speed(player.base.base_speed);
    player.experience = experience;
    player.base.health = health;
    player.base.health_max = health_max;
    player.blessings = blessings;
    player.mana = mana;
    player.mana_max = mana_max;
    player.mana_spent = mana_spent;
    player.mag_level = mag_level;
    // Magic-level progress percent (C++ IOLoginData::loadPlayer): clamp a
    // manaspent that exceeds the next-level requirement to 0, then compute the
    // percent. Without this the client shows 0% until the next cast.
    {
        let next_mana = crate::world::vocation::g_vocations()
            .get_vocation(vocation_id)
            .map(|v| v.get_req_mana(mag_level + 1))
            .unwrap_or(0);
        if next_mana != 0 && player.mana_spent > next_mana {
            player.mana_spent = 0;
        }
        player.mag_level_percent =
            crate::creatures::player::Player::get_percent_level(player.mana_spent, next_mana);
    }
    player.soul = soul;
    player.capacity = capacity;
    player.stamina_minutes = stamina_minutes;
    player.bank_balance = bank_balance;
    player.last_login_saved = last_login_saved;
    player.last_logout = last_logout;
    player.last_ip = last_ip;
    player.skull_ticks = skulltime;
    player.base.skull = skull;
    player.town_id = town_id;
    player.login_position = login_pos;
    player.base.position = login_pos;
    player.base.direction = direction;
    player.skills = skills;
    let outfit = Outfit {
        look_type,
        look_type_ex: 0,
        look_head,
        look_body,
        look_legs,
        look_feet,
        look_addons,
        look_mount: 0,
    };
    player.base.current_outfit = outfit;
    player.base.default_outfit = outfit;

    // Load VIP list from account_viplist (per-account, not per-character).
    let vip_query = format!(
        "SELECT `player_id` FROM `account_viplist` WHERE `account_id` = {}",
        account_number
    );
    if let Ok(Some(mut vip_result)) = db.store_query(&vip_query).await {
        loop {
            if let Some(pid) = vip_result.get_u64("player_id") {
                player.vip_list.insert(pid as u32);
            }
            if !vip_result.next() { break; }
        }
    }

    // Load inventory items with full container hierarchy (mirrors C++ IOLoginData::loadPlayer).
    // Items are ordered DESC by sid so that children (higher sids) appear before parents.
    // We process in descending order: add each child to its parent's children vec before
    // the parent itself is consumed.
    let items_query = format!(
        "SELECT `pid`, `sid`, `itemtype`, `count`, `attributes` FROM `player_items` \
         WHERE `player_id` = {guid} ORDER BY `sid` DESC"
    );
    if let Ok(Some(mut items_result)) = db.store_query(&items_query).await {
        use std::collections::BTreeMap as SidMap;
        use crate::map::tile::MapItem;

        let mut item_map: SidMap<u32, (MapItem, u32)> = SidMap::new();
        loop {
            let sid = items_result.get_u64("sid").unwrap_or(0) as u32;
            let pid = items_result.get_u64("pid").unwrap_or(0) as u32;
            let itemtype = items_result.get_u64("itemtype").unwrap_or(0) as u16;
            let count = items_result.get_u64("count").unwrap_or(1).max(1) as u16;
            if sid > 0 && itemtype != 0 {
                let mut mi = MapItem { server_id: itemtype, count, ..MapItem::default() };
                if let Some(blob) = items_result.get_bytes("attributes") {
                    let mut data = blob.as_slice();
                    let _ = crate::map::serialize::read_item_attrs(&mut mi, &mut data);
                }
                item_map.insert(sid, (mi, pid));
            }
            if !items_result.next() { break; }
        }

        // Collect sids ascending so rev() gives descending (children before parents).
        let sids: Vec<u32> = item_map.keys().cloned().collect();
        for &sid in sids.iter().rev() {
            let Some((map_item, pid)) = item_map.remove(&sid) else { continue; };
            if (CONST_SLOT_FIRST as u32..=CONST_SLOT_LAST as u32).contains(&pid) {
                let slot = pid as usize;
                player.inventory[slot] = Some(map_item.server_id);
                player.inventory_count[slot] = map_item.count;
                player.inventory_items[slot] = Some(map_item);
            } else if let Some(parent) = item_map.get_mut(&pid) {
                parent.0.children.insert(0, map_item);
            }
        }
    }

    // Load depot chest items (player_depotitems). pid 0..100 = depot/town id for
    // a top-level item; higher pids attach to a parent container. Mirrors C++
    // IOLoginData::loadPlayer depot loop.
    let depot_query = format!(
        "SELECT `pid`, `sid`, `itemtype`, `count`, `attributes` FROM `player_depotitems` \
         WHERE `player_id` = {guid} ORDER BY `sid` DESC"
    );
    if let Ok(Some(mut r)) = db.store_query(&depot_query).await {
        use std::collections::BTreeMap as SidMap;
        use crate::map::tile::MapItem;

        let mut item_map: SidMap<u32, (MapItem, u32)> = SidMap::new();
        loop {
            let sid = r.get_u64("sid").unwrap_or(0) as u32;
            let pid = r.get_u64("pid").unwrap_or(0) as u32;
            let itemtype = r.get_u64("itemtype").unwrap_or(0) as u16;
            let count = r.get_u64("count").unwrap_or(1).max(1) as u16;
            if sid > 0 && itemtype != 0 {
                let mut mi = MapItem { server_id: itemtype, count, ..MapItem::default() };
                if let Some(blob) = r.get_bytes("attributes") {
                    let mut data = blob.as_slice();
                    let _ = crate::map::serialize::read_item_attrs(&mut mi, &mut data);
                }
                item_map.insert(sid, (mi, pid));
            }
            if !r.next() { break; }
        }
        let sids: Vec<u32> = item_map.keys().cloned().collect();
        for &sid in sids.iter().rev() {
            let Some((map_item, pid)) = item_map.remove(&sid) else { continue; };
            if pid < 100 {
                player.depot_items.entry(pid).or_default().push(map_item);
            } else if let Some(parent) = item_map.get_mut(&pid) {
                parent.0.children.insert(0, map_item);
            }
        }
    }

    // Load storage map.
    let storage_query = format!(
        "SELECT `key`, `value` FROM `player_storage` WHERE `player_id` = {guid}"
    );
    if let Ok(Some(mut storage_result)) = db.store_query(&storage_query).await {
        loop {
            if let (Some(key), Some(value)) = (
                storage_result.get_u64("key").map(|v| v as u32),
                storage_result.get_i64("value").map(|v| v as i32),
            ) {
                player.storage_map.insert(key, value);
            }
            if !storage_result.next() { break; }
        }
    }

    // Load learned spells.
    let spells_query = format!(
        "SELECT `name` FROM `player_spells` WHERE `player_id` = {guid}"
    );
    if let Ok(Some(mut spells_result)) = db.store_query(&spells_query).await {
        loop {
            if let Some(name) = spells_result.get_string("name") {
                player.learned_instant_spells.push(name);
            }
            if !spells_result.next() { break; }
        }
    }

    // Add a permanent regeneration condition based on the player's vocation.
    {
        use crate::combat::condition::{
            ConditionBase, ConditionId, ConditionRegeneration, ConditionType,
        };
        use crate::world::vocation::g_vocations;

        let voc = g_vocations().get_vocation(player.vocation_id);
        let health_ticks = voc.map(|v| v.gain_health_ticks * 1000).unwrap_or(6000);
        let health_gain = voc.map(|v| v.gain_health_amount).unwrap_or(1);
        let mana_ticks = voc.map(|v| v.gain_mana_ticks * 1000).unwrap_or(6000);
        let mana_gain = voc.map(|v| v.gain_mana_amount).unwrap_or(1);

        let mut regen = ConditionRegeneration {
            base: ConditionBase::new(ConditionId::Default, ConditionType::Regeneration, -1, false, 0, false),
            internal_health_ticks: 0,
            internal_mana_ticks: 0,
            health_ticks,
            health_gain,
            mana_ticks,
            mana_gain,
        };
        regen.base.end_time = i64::MAX;
        player.base.conditions.push(Box::new(regen));
    }

    Some(player)
}

pub async fn add_vip_entry(account_id: u32, player_guid: u32) {
    let db = g_database();
    let query = format!(
        "INSERT INTO `account_viplist` (`account_id`, `player_id`) VALUES ({account_id}, {player_guid})"
    );
    let _ = db.execute(&query).await;
}

pub async fn remove_vip_entry(account_id: u32, player_guid: u32) {
    let db = g_database();
    let query = format!(
        "DELETE FROM `account_viplist` WHERE `account_id` = {account_id} AND `player_id` = {player_guid}"
    );
    let _ = db.execute(&query).await;
}

pub async fn get_vip_entries(account_id: u32) -> Vec<(u32, String)> {
    let db = g_database();
    let query = format!(
        "SELECT `player_id`, (SELECT `name` FROM `players` WHERE `id` = `player_id`) AS `name` \
         FROM `account_viplist` WHERE `account_id` = {account_id}"
    );
    let mut entries = Vec::new();
    if let Ok(Some(mut result)) = db.store_query(&query).await {
        loop {
            if let (Some(guid), Some(name)) = (
                result.get_u64("player_id").map(|v| v as u32),
                result.get_string("name"),
            ) {
                entries.push((guid, name));
            }
            if !result.next() { break; }
        }
    }
    entries
}

/// Update players_online table on login/logout. Mirrors C++ `IOLoginData::updateOnlineStatus`.
/// Skipped when ALLOW_CLONES config is true.
pub async fn update_online_status(guid: u32, login: bool) {
    let allow_clones = g_config().get_boolean(crate::config::BooleanConfig::AllowClones);
    if allow_clones {
        return;
    }
    let db = g_database();
    if login {
        let _ = db.execute(&format!("INSERT INTO `players_online` VALUES ({guid})")).await;
    } else {
        let _ = db.execute(&format!("DELETE FROM `players_online` WHERE `player_id` = {guid}")).await;
    }
}

pub async fn update_premium_time(account_id: u32, end_time: i64) {
    let db = g_database();
    let _ = db.execute(&format!(
        "UPDATE `accounts` SET `premium_ends_at` = {end_time} WHERE `id` = {account_id}"
    )).await;
}

fn collect_item_rows_dfs(
    guid: u32, pid: i32, item: &crate::map::tile::MapItem,
    running_id: &mut i32, rows: &mut Vec<String>,
    items: &crate::items::Items, db: &crate::db::Database,
) {
    *running_id += 1;
    let sid = *running_id;
    let mut blob = Vec::new();
    crate::map::serialize::write_item_attrs(item, items, &mut blob);
    let escaped = db.escape_blob(&blob);
    rows.push(format!(
        "({guid}, {pid}, {sid}, {}, {}, {escaped})",
        item.server_id,
        item.count.max(1),
    ));
    for child in &item.children {
        collect_item_rows_dfs(guid, sid, child, running_id, rows, items, db);
    }
}

/// Save player state to the database. Mirrors C++ `IOLoginData::savePlayer`.
/// Returns true on success.
pub async fn save_player(player: &PlayerSaveSnapshot) -> bool {
    let db = g_database();
    let guid = player.guid;

    // Check the `save` flag — if 0, only update lastlogin/lastip.
    let save_flag = match db.store_query(&format!(
        "SELECT `save` FROM `players` WHERE `id` = {guid}"
    )).await {
        Ok(Some(r)) => r.get_u64("save").unwrap_or(1) as u16,
        _ => return false,
    };

    if save_flag == 0 {
        let q = format!(
            "UPDATE `players` SET `lastlogin` = {}, `lastip` = {} WHERE `id` = {guid}",
            player.last_login_saved, player.last_ip
        );
        return db.execute(&q).await.unwrap_or(false);
    }

    let now_sec = get_milliseconds_time() / 1000;

    let skull_db = match player.skull {
        Skull::Red => 4u8,
        Skull::Black => 5u8,
        _ => 0u8,
    };
    let skulltime_db = if skull_db != 0 { player.skull_ticks } else { 0i64 };

    let direction_db = player.direction as u8;
    let sex_db = match player.sex {
        PlayerSex::Male => 1u8,
        PlayerSex::Female => 0u8,
    };

    let online_time_fragment = if player.last_login_saved != 0 {
        let delta = now_sec - player.last_login_saved;
        let delta = delta.max(0);
        format!("`onlinetime` = `onlinetime` + {delta}, ")
    } else {
        String::new()
    };

    let lastlogin_fragment = if player.last_login_saved != 0 {
        format!("`lastlogin` = {}, ", player.last_login_saved)
    } else {
        String::new()
    };

    let lastip_fragment = if player.last_ip != 0 {
        format!("`lastip` = {}, ", player.last_ip)
    } else {
        String::new()
    };

    // Build the big UPDATE query.
    let query = format!(
        "UPDATE `players` SET \
         `level` = {level}, \
         `group_id` = {group_id}, \
         `vocation` = {vocation}, \
         `health` = {health}, \
         `healthmax` = {healthmax}, \
         `experience` = {experience}, \
         `lookbody` = {lookbody}, \
         `lookfeet` = {lookfeet}, \
         `lookhead` = {lookhead}, \
         `looklegs` = {looklegs}, \
         `looktype` = {looktype}, \
         `lookaddons` = {lookaddons}, \
         `maglevel` = {maglevel}, \
         `mana` = {mana}, \
         `manamax` = {manamax}, \
         `manaspent` = {manaspent}, \
         `soul` = {soul}, \
         `town_id` = {town_id}, \
         `posx` = {posx}, \
         `posy` = {posy}, \
         `posz` = {posz}, \
         `cap` = {cap}, \
         `sex` = {sex}, \
         {lastlogin_fragment}\
         {lastip_fragment}\
         `conditions` = {conditions}, \
         `skulltime` = {skulltime}, \
         `skull` = {skull}, \
         `lastlogout` = {lastlogout}, \
         `balance` = {balance}, \
         `stamina` = {stamina}, \
         `skill_fist` = {sf}, `skill_fist_tries` = {sft}, \
         `skill_club` = {sc}, `skill_club_tries` = {sct}, \
         `skill_sword` = {ss}, `skill_sword_tries` = {sst}, \
         `skill_axe` = {sa}, `skill_axe_tries` = {sat}, \
         `skill_dist` = {sd}, `skill_dist_tries` = {sdt}, \
         `skill_shielding` = {sh}, `skill_shielding_tries` = {sht}, \
         `skill_fishing` = {sfi}, `skill_fishing_tries` = {sfit}, \
         `direction` = {direction}, \
         {online_time_fragment}\
         `blessings` = {blessings} \
         WHERE `id` = {guid}",
        level = player.level,
        group_id = player.group_id,
        vocation = player.vocation_id,
        health = player.health,
        healthmax = player.health_max,
        experience = player.experience,
        lookbody = player.default_outfit.look_body,
        lookfeet = player.default_outfit.look_feet,
        lookhead = player.default_outfit.look_head,
        looklegs = player.default_outfit.look_legs,
        looktype = player.default_outfit.look_type,
        lookaddons = player.default_outfit.look_addons,
        maglevel = player.mag_level,
        mana = player.mana,
        manamax = player.mana_max,
        manaspent = player.mana_spent,
        soul = player.soul as u16,
        town_id = player.town_id,
        posx = player.login_position.x,
        posy = player.login_position.y,
        posz = player.login_position.z,
        cap = player.capacity / 100,
        sex = sex_db,
        lastlogin_fragment = lastlogin_fragment,
        lastip_fragment = lastip_fragment,
        conditions = db.escape_blob(b""),
        skulltime = skulltime_db,
        skull = skull_db,
        lastlogout = player.last_logout,
        balance = player.bank_balance,
        stamina = player.stamina_minutes,
        sf  = player.skills[0].level, sft  = player.skills[0].tries,
        sc  = player.skills[1].level, sct  = player.skills[1].tries,
        ss  = player.skills[2].level, sst  = player.skills[2].tries,
        sa  = player.skills[3].level, sat  = player.skills[3].tries,
        sd  = player.skills[4].level, sdt  = player.skills[4].tries,
        sh  = player.skills[5].level, sht  = player.skills[5].tries,
        sfi = player.skills[6].level, sfit = player.skills[6].tries,
        direction = direction_db,
        online_time_fragment = online_time_fragment,
        blessings = player.blessings,
    );

    if !db.execute(&query).await.unwrap_or(false) {
        return false;
    }

    // Learned spells: delete then reinsert.
    if !db.execute(&format!("DELETE FROM `player_spells` WHERE `player_id` = {guid}")).await.unwrap_or(false) {
        return false;
    }
    if !player.learned_instant_spells.is_empty() {
        let mut spell_rows: Vec<String> = Vec::new();
        for spell in &player.learned_instant_spells {
            let escaped = db.escape_string(spell);
            spell_rows.push(format!("({guid}, {escaped})"));
        }
        let spell_query = format!(
            "INSERT INTO `player_spells` (`player_id`, `name`) VALUES {}",
            spell_rows.join(", ")
        );
        if !db.execute(&spell_query).await.unwrap_or(false) {
            return false;
        }
    }

    // Inventory items: delete then reinsert with full container hierarchy.
    if !db.execute(&format!("DELETE FROM `player_items` WHERE `player_id` = {guid}")).await.unwrap_or(false) {
        return false;
    }
    let mut item_rows: Vec<String> = Vec::new();
    let empty_blob = db.escape_blob(b"");
    // Item attribute (text/charges/etc.) serialization needs the items table.
    let items_table = crate::game::g_game().lock().unwrap().items.clone();
    let mut running_id: i32 = 100;
    // Build a slot → full item map for quick lookup.
    let items_by_slot: std::collections::HashMap<usize, &crate::map::tile::MapItem> =
        player.inventory_items.iter().map(|(slot, item)| (*slot, item)).collect();
    for slot in CONST_SLOT_FIRST..=CONST_SLOT_LAST {
        if let Some(map_item) = items_by_slot.get(&slot) {
            collect_item_rows_dfs(guid, slot as i32, map_item, &mut running_id, &mut item_rows, &items_table, db);
        } else if let Some(server_id) = player.inventory[slot] {
            running_id += 1;
            let count = player.inventory_count[slot].max(1);
            item_rows.push(format!(
                "({guid}, {slot}, {running_id}, {server_id}, {count}, {empty_blob})"
            ));
        }
    }
    if !item_rows.is_empty() {
        let items_query = format!(
            "INSERT INTO `player_items` (`player_id`, `pid`, `sid`, `itemtype`, `count`, `attributes`) VALUES {}",
            item_rows.join(", ")
        );
        if !db.execute(&items_query).await.unwrap_or(false) {
            return false;
        }
    }

    // Depot items: delete then reinsert (pid = depot id, full container tree).
    if !db.execute(&format!("DELETE FROM `player_depotitems` WHERE `player_id` = {guid}")).await.unwrap_or(false) {
        return false;
    }
    let mut depot_rows: Vec<String> = Vec::new();
    let mut depot_running_id: i32 = 100;
    for (depot_id, item) in &player.depot_items {
        collect_item_rows_dfs(guid, *depot_id as i32, item, &mut depot_running_id, &mut depot_rows, &items_table, db);
    }
    if !depot_rows.is_empty() {
        let depot_query = format!(
            "INSERT INTO `player_depotitems` (`player_id`, `pid`, `sid`, `itemtype`, `count`, `attributes`) VALUES {}",
            depot_rows.join(", ")
        );
        if !db.execute(&depot_query).await.unwrap_or(false) {
            return false;
        }
    }

    // Storage map: delete then reinsert.
    if !db.execute(&format!("DELETE FROM `player_storage` WHERE `player_id` = {guid}")).await.unwrap_or(false) {
        return false;
    }
    if !player.storage_map.is_empty() {
        let mut storage_rows: Vec<String> = Vec::new();
        for (&key, &value) in &player.storage_map {
            storage_rows.push(format!("({guid}, {key}, {value})"));
        }
        let storage_query = format!(
            "INSERT INTO `player_storage` (`player_id`, `key`, `value`) VALUES {}",
            storage_rows.join(", ")
        );
        if !db.execute(&storage_query).await.unwrap_or(false) {
            return false;
        }
    }

    true
}
