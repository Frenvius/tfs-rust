use crate::creatures::CreatureId;
use crate::map::Position;

pub const ATTR_SLEEPERGUID: u8 = 20;
pub const ATTR_SLEEPSTART: u8 = 21;

#[derive(Debug)]
pub struct BedItem {
    pub item_id: u16,
    pub house_id: Option<u32>,
    pub sleeper_guid: u32,
    pub sleep_start: u64,
    pub special_description: String,
}

impl BedItem {
    pub fn new(item_id: u16) -> Self {
        Self {
            item_id,
            house_id: None,
            sleeper_guid: 0,
            sleep_start: 0,
            special_description: String::from("Nobody is sleeping there."),
        }
    }

    pub fn get_sleeper(&self) -> u32 {
        self.sleeper_guid
    }

    pub fn can_remove(&self) -> bool {
        self.house_id.is_none()
    }

    pub fn serialize_attr(&self, out: &mut Vec<u8>) {
        if self.sleeper_guid != 0 {
            out.push(ATTR_SLEEPERGUID);
            out.extend_from_slice(&self.sleeper_guid.to_le_bytes());
        }
        if self.sleep_start != 0 {
            out.push(ATTR_SLEEPSTART);
            out.extend_from_slice(&(self.sleep_start as u32).to_le_bytes());
        }
    }

    pub fn read_attr(&mut self, attr_type: u8, data: &mut &[u8]) -> bool {
        match attr_type {
            ATTR_SLEEPERGUID => {
                if data.len() < 4 { return false; }
                let guid = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                *data = &data[4..];
                if guid != 0 { self.sleeper_guid = guid; }
                true
            }
            ATTR_SLEEPSTART => {
                if data.len() < 4 { return false; }
                let val = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                *data = &data[4..];
                self.sleep_start = val as u64;
                true
            }
            _ => false,
        }
    }
}

struct BedUseInfo {
    house_id: Option<u32>,
    is_pz: bool,
    sleeper_guid: u32,
    transform_to_free: u16,
    transform_male: u16,
    transform_female: u16,
    partner_dir: u8,
    p_guid: u32,
    p_sex: crate::creatures::player::PlayerSex,
    premium: bool,
    house_owner: u32,
    my_access: crate::map::houses::AccessHouseLevel,
}

pub fn handle_use_bed(game: &mut crate::game::Game, creature_id: CreatureId, pos: Position, server_id: u16) {
    use crate::config::{g_config, BooleanConfig};
    use crate::map::tile::TileKind;
    use crate::map::tile::TILESTATE_PROTECTIONZONE;

    let info = {
        let tile = match game.map.get_tile(pos) {
            Some(t) => t,
            None => return,
        };
        let house_id = match tile.kind {
            TileKind::House { house_id } => Some(house_id),
            _ => None,
        };
        let is_pz = tile.has_flag(TILESTATE_PROTECTIONZONE);
        let sleeper_guid = tile
            .find_item_index_by_server_id(server_id)
            .map(|idx| tile.items[idx].sleeper_guid)
            .unwrap_or(0);

        let it = game.items.get_item_type(usize::from(server_id));
        let transform_to_free = it.transform_to_free;
        let transform_male = it.transform_to_on_use[1];
        let transform_female = it.transform_to_on_use[0];
        let partner_dir = it.bed_partner_dir;

        let player = match game.get_player(creature_id) {
            Some(p) => p,
            None => return,
        };
        let p_guid = player.guid;
        let p_account = player.account_number;
        let p_sex = player.sex;
        let premium = player.is_premium();
        let can_edit = player.group_flags & crate::creatures::player::PLAYER_FLAG_CAN_EDIT_HOUSES != 0;
        let p_name = player.name.clone();

        let owned_by_account = g_config().get_boolean(BooleanConfig::HouseOwnedByAccount);
        let (house_owner, my_access) = match house_id.and_then(|hid| game.map.houses.get_house(hid)) {
            Some(h) => (
                h.get_owner(),
                h.access_level_for(p_guid, p_account, can_edit, owned_by_account, &p_name, "", ""),
            ),
            None => (0, crate::map::houses::AccessHouseLevel::NotInvited),
        };

        BedUseInfo {
            house_id, is_pz, sleeper_guid, transform_to_free, transform_male, transform_female,
            partner_dir, p_guid, p_sex, premium, house_owner, my_access,
        }
    };

    let has_house = info.house_id.is_some();
    let can_use = if !has_house || !info.premium || !info.is_pz {
        false
    } else if info.sleeper_guid == 0 {
        true
    } else {
        info.my_access == crate::map::houses::AccessHouseLevel::Owner
    };

    if !can_use {
        let msg = if !has_house {
            "You can not use this bed."
        } else if !info.premium {
            "You need a premium account."
        } else {
            "You cannot use this object."
        };
        crate::net::game_protocol::send_status_message_to_player(creature_id, msg);
        return;
    }

    if info.sleeper_guid != 0 {
        if info.transform_to_free != 0 && info.house_owner == info.p_guid {
            wake_bed_at(game, pos, server_id);
        }
        let ppos = game.get_player(creature_id).map(|p| p.base.position).unwrap_or(pos);
        game.add_magic_effect(ppos, crate::game::CONST_ME_POFF);
        return;
    }

    bed_sleep(game, creature_id, pos, server_id, &info);
}

fn bed_sleep(game: &mut crate::game::Game, creature_id: CreatureId, pos: Position, server_id: u16, info: &BedUseInfo) {
    use crate::creatures::player::PlayerSex;
    let now = (crate::util::otsys_time() / 1000) as u32;
    let partner_pos = crate::net::game_protocol::next_position(info.partner_dir, pos);

    let pname = game.get_player(creature_id).map(|p| p.name.clone()).unwrap_or_default();
    let desc = format!("{} is sleeping there.", pname);
    let guid = info.p_guid;

    let partner_sid = game.map.get_tile(partner_pos).and_then(|t| {
        t.items.iter()
            .find(|it| game.items.get_item_type(usize::from(it.server_id)).kind == crate::items::ItemKind::Bed)
            .map(|it| it.server_id)
    });

    game.set_item_sleeper(pos, server_id, guid, now, desc.clone());
    if let Some(psid) = partner_sid {
        game.set_item_sleeper(partner_pos, psid, guid, now, desc);
    }
    game.set_bed_sleeper(guid, pos);
    if let Some(p) = game.get_player_mut(creature_id) {
        p.bed_item_id = Some(server_id);
    }

    let old_pos = game.get_player(creature_id).map(|p| p.base.position).unwrap_or(pos);
    if old_pos != pos {
        game.move_creature_position(creature_id, old_pos, pos);
    }
    game.add_magic_effect(pos, crate::game::CONST_ME_SLEEP);

    let sex_transform = match info.p_sex {
        PlayerSex::Male => info.transform_male,
        PlayerSex::Female => info.transform_female,
    };
    let new_id = if sex_transform != 0 { sex_transform } else { info.transform_to_free };
    game.transform_tile_item(pos, server_id, new_id);

    if let Some(psid) = partner_sid {
        let pit = game.items.get_item_type(usize::from(psid));
        let p_sex_t = match info.p_sex {
            PlayerSex::Male => pit.transform_to_on_use[1],
            PlayerSex::Female => pit.transform_to_on_use[0],
        };
        let p_free = pit.transform_to_free;
        let p_new = if p_sex_t != 0 { p_sex_t } else { p_free };
        game.transform_tile_item(partner_pos, psid, p_new);
    }

    crate::runtime::g_scheduler().add_event(crate::runtime::scheduler::SchedulerTask::new(
        crate::runtime::scheduler::SCHEDULER_MINTICKS,
        move || crate::net::game_protocol::kick_player_by_id(creature_id),
    ));
}

pub fn regenerate_slept_player(game: &mut crate::game::Game, creature_id: CreatureId, sleep_start: u32) {
    use crate::combat::condition::ConditionType;
    let now = (crate::util::otsys_time() / 1000) as u32;
    let slept_time = now.saturating_sub(sleep_start) as i32;

    let soul_max = game
        .get_player(creature_id)
        .and_then(|p| crate::world::vocation::g_vocations().get_vocation(p.vocation_id).cloned())
        .map(|v| v.soul_max)
        .unwrap_or(100);

    let Some(player) = game.get_player_mut(creature_id) else { return };

    let mut regen: i32 = 0;
    let mut has_regen = false;
    if let Some(condition) = player.base.get_condition_mut(ConditionType::Regeneration) {
        has_regen = true;
        let ticks = condition.get_ticks();
        if ticks != -1 {
            regen = std::cmp::min(ticks / 1000, slept_time) / 30;
            let new_regen_ticks = ticks - (regen * 30000);
            if new_regen_ticks <= 0 {
                player.base.remove_condition_by_type(ConditionType::Regeneration);
            } else if let Some(c) = player.base.get_condition_mut(ConditionType::Regeneration) {
                c.set_ticks(new_regen_ticks);
            }
        } else {
            regen = slept_time / 30;
        }
    }

    if has_regen && regen > 0 {
        let new_health = (player.base.health + regen).min(player.base.health_max);
        player.base.health = new_health;
        let new_mana = (player.mana as i32 + regen).min(player.mana_max as i32).max(0) as u32;
        player.mana = new_mana;
    }

    let soul_regen = (slept_time / (60 * 15)) as i32;
    if soul_regen > 0 {
        let new_soul = (player.soul as i32 + soul_regen).clamp(0, soul_max as i32) as u8;
        player.soul = new_soul;
    }
}

pub fn wake_bed_at(game: &mut crate::game::Game, pos: Position, server_id: u16) {
    let (sleeper_guid, sleep_start) = game
        .map
        .get_tile(pos)
        .and_then(|t| {
            t.find_item_index_by_server_id(server_id)
                .map(|i| (t.items[i].sleeper_guid, t.items[i].sleep_start))
        })
        .unwrap_or((0, 0));
    if sleeper_guid == 0 {
        return;
    }

    if let Some(cid) = game.get_player_id_by_guid(sleeper_guid) {
        regenerate_slept_player(game, cid, sleep_start);
        game.add_creature_health(cid);
    }

    game.remove_bed_sleeper(sleeper_guid);

    let partner_pos = {
        let it = game.items.get_item_type(usize::from(server_id));
        crate::net::game_protocol::next_position(it.bed_partner_dir, pos)
    };
    let free_id = game.items.get_item_type(usize::from(server_id)).transform_to_free;
    game.set_item_sleeper(pos, server_id, 0, 0, "Nobody is sleeping there.".to_string());
    game.transform_tile_item(pos, server_id, free_id);

    let partner_sid = game.map.get_tile(partner_pos).and_then(|t| {
        t.items.iter()
            .find(|it| game.items.get_item_type(usize::from(it.server_id)).kind == crate::items::ItemKind::Bed)
            .map(|it| it.server_id)
    });
    if let Some(psid) = partner_sid {
        let p_free = game.items.get_item_type(usize::from(psid)).transform_to_free;
        game.set_item_sleeper(partner_pos, psid, 0, 0, "Nobody is sleeping there.".to_string());
        game.transform_tile_item(partner_pos, psid, p_free);
    }
}
