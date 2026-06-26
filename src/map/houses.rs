use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;
use thiserror::Error;

use crate::util::xml::{self, XmlLoadError};

use super::Position;

pub const GUEST_LIST: u32 = 0x100;
pub const SUBOWNER_LIST: u32 = 0x101;

/// AccessList — matches `class AccessList` in house.h / house.cpp.
///
/// Stores the raw text list and the parsed sets of player names, guild names,
/// and guild@rank expressions.
#[derive(Debug, Clone, Default)]
pub struct AccessList {
    pub list: String,
    pub player_list: BTreeSet<String>,
    pub guild_list: BTreeSet<String>,
    pub expression_list: Vec<String>,
    pub allow_everyone: bool,
}

impl AccessList {
    pub fn new() -> Self {
        Self::default()
    }

    /// parse_list — port of AccessList::parseList from house.cpp.
    pub fn parse_list(&mut self, list: &str) {
        self.player_list.clear();
        self.guild_list.clear();
        self.expression_list.clear();
        self.allow_everyone = false;
        self.list = list.to_owned();

        if list.is_empty() {
            return;
        }

        let mut line_no: u16 = 1;
        for raw_line in list.lines() {
            if line_no > 100 {
                break;
            }
            line_no += 1;

            let line = raw_line.trim().trim_matches('\t').trim();
            if line.is_empty() || line.starts_with('#') || line.len() > 100 {
                continue;
            }

            if let Some(at_pos) = line.find('@') {
                if at_pos == 0 {
                    self.add_guild(&line[1..]);
                } else {
                    let name_part = line[..at_pos].trim_end();
                    let rank_part = line[at_pos + 1..].trim_start();
                    self.add_guild_rank(name_part, rank_part);
                }
            } else if line == "*" {
                self.allow_everyone = true;
            } else if line.contains('!') || line.contains('*') || line.contains('?') {
                continue;
            } else {
                self.add_player(line);
            }
        }
    }

    pub fn add_player(&mut self, name: &str) {
        self.player_list.insert(name.to_owned());
    }

    pub fn add_guild(&mut self, name: &str) {
        self.guild_list.insert(name.to_owned());
    }

    pub fn add_guild_rank(&mut self, guild_name: &str, rank_name: &str) {
        self.expression_list
            .push(format!("{}@{}", guild_name, rank_name));
    }

    /// is_in_list — port of AccessList::isInList from house.cpp.
    ///
    /// Checks allow_everyone, player name (lower-cased), whole guild, and
    /// guild@rank expressions. Pass values already lower-cased.
    pub fn is_in_list(&self, name: &str, guild_name: &str, guild_rank_name: &str) -> bool {
        if self.allow_everyone {
            return true;
        }

        let name_lower = name.to_lowercase();
        if self.player_list.contains(&name_lower) {
            return true;
        }

        if !guild_name.is_empty() {
            let guild_lower = guild_name.to_lowercase();
            if self.guild_list.contains(&guild_lower) {
                return true;
            }

            if !guild_rank_name.is_empty() {
                let expr = format!("{}@{}", guild_lower, guild_rank_name.to_lowercase());
                if self.expression_list.contains(&expr) {
                    return true;
                }
            }
        }

        false
    }

    pub fn get_list(&self) -> &str {
        &self.list
    }
}

/// Door — matches `class Door` in house.h (data fields only; Item base dropped).
#[derive(Debug, Clone, Default)]
pub struct Door {
    pub door_id: u32,
    pub house_id: u32,
    pub access_list: Option<AccessList>,
}

impl Door {
    pub fn new(door_id: u32, house_id: u32) -> Self {
        Self { door_id, house_id, access_list: None }
    }

    pub fn get_door_id(&self) -> u32 {
        self.door_id
    }

    pub fn set_access_list(&mut self, text_list: &str) {
        let al = self.access_list.get_or_insert_with(AccessList::new);
        al.parse_list(text_list);
    }

    pub fn get_access_list(&self) -> Option<&str> {
        self.access_list.as_ref().map(|al| al.get_list())
    }

    /// is_player_allowed — port of Door::canUse from house.cpp.
    ///
    /// House-level access (subowner/owner bypass) requires Game integration.
    /// This method checks only the door's own access list.
    pub fn is_player_allowed(
        &self,
        player_name: &str,
        guild_name: &str,
        guild_rank_name: &str,
    ) -> bool {
        match &self.access_list {
            Some(al) => al.is_in_list(player_name, guild_name, guild_rank_name),
            None => false,
        }
    }

    pub fn can_use(&self, _player_id: u32) -> bool {
        // C++ checks house access level >= subowner, then falls back to access list.
        // Partial: is_player_allowed exists but doesn't check subowner bypass.
        true
    }

    pub fn on_removed(&mut self) {
        // C++ unlinks this door from its parent house via house->removeDoor(this).
    }

    pub fn read_attr(&mut self, _attr: u8, _prop_stream: &[u8]) -> bool {
        // C++ deserializes ATTR_HOUSEDOORID from binary prop stream.
        false
    }
}

/// AccessHouseLevel_t — matches `enum AccessHouseLevel_t` in house.h.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AccessHouseLevel {
    NotInvited = 0,
    Guest = 1,
    Subowner = 2,
    Owner = 3,
}

/// RentPeriod_t — matches `enum RentPeriod_t` in house.h.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RentPeriod {
    Daily = 0,
    Weekly = 1,
    Monthly = 2,
    Yearly = 3,
    Never = 4,
}

/// HouseTransferItem — matches `class HouseTransferItem` in house.h.
///
/// The Item base is not yet ported; house_id substitutes for the item link.
#[derive(Debug, Clone)]
pub struct HouseTransferItem {
    pub house_id: u32,
}

impl HouseTransferItem {
    pub fn create(house_id: u32) -> Self {
        Self { house_id }
    }

    pub fn on_trade_event(&self, _event: u8, _buyer_id: u32) -> bool {
        // C++ ON_TRADE_TRANSFER → executeTransfer + item removal; ON_TRADE_CANCEL → resetTransferItem.
        false
    }
}

#[derive(Debug, Clone, Default)]
pub struct House {
    pub id: u32,
    pub name: String,
    pub owner_name: String,
    pub entry_position: Position,
    pub rent: u32,
    pub town_id: u32,
    pub owner: u32,
    pub owner_account_id: u32,
    pub paid_until: i64,
    pub rent_warnings: u32,
    pub is_loaded: bool,
    pub tiles: Vec<Position>,
    pub doors: BTreeMap<u8, Position>,
    pub beds: Vec<Position>,
    pub guest_list: AccessList,
    pub sub_owner_list: AccessList,
    pub transfer_item: Option<HouseTransferItem>,
}

impl House {
    pub fn add_tile(&mut self, position: Position) {
        self.tiles.push(position);
    }

    pub fn add_door(&mut self, door_id: u8, position: Position) {
        if door_id != 0 {
            self.doors.insert(door_id, position);
        }
    }

    pub fn add_bed(&mut self, position: Position) {
        self.beds.push(position);
    }

    pub fn get_id(&self) -> u32 {
        self.id
    }

    pub fn get_name(&self) -> &str {
        &self.name
    }

    pub fn get_entry_position(&self) -> Position {
        self.entry_position
    }

    pub fn get_rent(&self) -> u32 {
        self.rent
    }

    pub fn get_town_id(&self) -> u32 {
        self.town_id
    }

    pub fn get_owner(&self) -> u32 {
        self.owner
    }

    /// set_owner — sets the owner GUID.
    ///
    /// The full C++ set_owner (DB update, depot transfer, kick players, clear
    /// access lists) requires Game integration.
    pub fn set_owner(&mut self, owner: u32) {
        self.owner = owner;
    }

    pub fn get_bed_count(&self) -> usize {
        self.beds.len()
    }

    /// get_door_by_number — port of House::getDoorByNumber.
    pub fn get_door_by_number(&self, door_id: u8) -> Option<Position> {
        self.doors.get(&door_id).copied()
    }

    pub fn remove_door(&mut self, door_id: u8) {
        self.doors.remove(&door_id);
    }

    pub fn get_door_by_position(&self, pos: Position) -> Option<u8> {
        self.doors.iter().find(|(_, &p)| p == pos).map(|(&id, _)| id)
    }

    pub fn get_paid_until(&self) -> i64 {
        self.paid_until
    }

    pub fn set_paid_until(&mut self, paid: i64) {
        self.paid_until = paid;
    }

    pub fn get_pay_rent_warnings(&self) -> u32 {
        self.rent_warnings
    }

    pub fn set_pay_rent_warnings(&mut self, warnings: u32) {
        self.rent_warnings = warnings;
    }

    pub fn set_access_list(&mut self, list_id: u32, text_list: &str) {
        if list_id == GUEST_LIST {
            self.guest_list.parse_list(text_list);
        } else if list_id == SUBOWNER_LIST {
            self.sub_owner_list.parse_list(text_list);
        } else {
            let door_id = list_id as u8;
            if self.doors.contains_key(&door_id) {
            }
        }
    }

    /// get_access_list — port of House::getAccessList(listId, list).
    pub fn get_access_list(&self, list_id: u32) -> Option<&str> {
        if list_id == GUEST_LIST {
            return Some(self.guest_list.get_list());
        }
        if list_id == SUBOWNER_LIST {
            return Some(self.sub_owner_list.get_list());
        }
        None
    }

    /// is_invited — port of House::isInvited. Requires player context for full check.
    pub fn is_invited_by_name(&self, name: &str, guild_name: &str, guild_rank_name: &str) -> bool {
        self.guest_list.is_in_list(name, guild_name, guild_rank_name)
            || self.sub_owner_list.is_in_list(name, guild_name, guild_rank_name)
    }

    /// can_edit_access_list — port of House::canEditAccessList(listId, player).
    pub fn can_edit_access_list(&self, list_id: u32, access_level: AccessHouseLevel) -> bool {
        match access_level {
            AccessHouseLevel::Owner => true,
            AccessHouseLevel::Subowner => list_id == GUEST_LIST,
            _ => false,
        }
    }

    /// access_level_for — port of House::getHouseAccessLevel(Player*).
    #[allow(clippy::too_many_arguments)]
    pub fn access_level_for(
        &self,
        guid: u32,
        account_id: u32,
        can_edit_houses: bool,
        owned_by_account: bool,
        name: &str,
        guild_name: &str,
        guild_rank: &str,
    ) -> AccessHouseLevel {
        if owned_by_account && self.owner_account_id == account_id {
            return AccessHouseLevel::Owner;
        }
        if can_edit_houses {
            return AccessHouseLevel::Owner;
        }
        if guid != 0 && guid == self.owner {
            return AccessHouseLevel::Owner;
        }
        if self.sub_owner_list.is_in_list(name, guild_name, guild_rank) {
            return AccessHouseLevel::Subowner;
        }
        if self.guest_list.is_in_list(name, guild_name, guild_rank) {
            return AccessHouseLevel::Guest;
        }
        AccessHouseLevel::NotInvited
    }

    /// get_transfer_item — port of House::getTransferItem from house.cpp.
    pub fn get_transfer_item(&mut self) -> &mut HouseTransferItem {
        self.transfer_item.get_or_insert_with(|| HouseTransferItem::create(self.id))
    }

    /// reset_transfer_item — port of House::resetTransferItem from house.cpp.
    pub fn reset_transfer_item(&mut self) {
        self.transfer_item = None;
    }

    pub fn kick_player(&mut self, _player_id: u32) {
        // C++ checks houseTile membership, compares access levels, teleports to entry pos.
    }

    pub fn update_door_description(&self) {
        // C++ iterates all doors, sets specialDescription with owner name + house name.
    }

    pub fn execute_transfer(&mut self, _item: &HouseTransferItem, _new_owner_id: u32) -> bool {
        // C++ validates transferItem == item, calls setOwner(newOwner), clears transferItem.
        false
    }
}

#[derive(Debug, Clone, Default)]
pub struct Houses {
    houses: BTreeMap<u32, House>,
}

impl Houses {
    pub fn add_house(&mut self, id: u32) -> &mut House {
        self.houses.entry(id).or_insert_with(|| House {
            id,
            ..House::default()
        })
    }

    pub fn get_house(&self, id: u32) -> Option<&House> {
        self.houses.get(&id)
    }

    pub fn get_house_mut(&mut self, id: u32) -> Option<&mut House> {
        self.houses.get_mut(&id)
    }

    pub fn get_houses(&self) -> &BTreeMap<u32, House> {
        &self.houses
    }

    pub fn load_from_xml(&mut self, path: impl AsRef<Path>) -> Result<(), HouseLoadError> {
        let data: HousesXml = xml::load_from_path(path)?;

        for house in data.house {
            let existing = self
                .get_house_mut(house.houseid)
                .ok_or(HouseLoadError::UnknownHouseId(house.houseid))?;

            existing.name = house.name;
            existing.entry_position = Position {
                x: house.entryx,
                y: house.entryy,
                z: house.entryz,
            };
            existing.rent = house.rent;
            existing.town_id = house.townid;
            existing.owner = 0;
        }

        Ok(())
    }

    /// get_house_by_player_id — port of Houses::getHouseByPlayerId from house.cpp.
    ///
    /// Pure in-memory lookup: returns the first house owned by the given player GUID.
    pub fn get_house_by_player_id(&self, player_id: u32) -> Option<&House> {
        self.houses.values().find(|h| h.owner == player_id)
    }

}

pub fn transfer_to_depot(game: &mut crate::game::Game, house_id: u32) -> bool {
    let (town_id, owner) = match game.map.houses.get_house(house_id) {
        Some(h) => (h.town_id, h.owner),
        None => return false,
    };
    if town_id == 0 || owner == 0 {
        return false;
    }
    let Some(cid) = game.get_player_id_by_guid(owner) else { return false };

    let tiles: Vec<crate::map::Position> = game
        .map
        .houses
        .get_house(house_id)
        .map(|h| h.tiles.clone())
        .unwrap_or_default();

    let mut moved: Vec<crate::map::tile::MapItem> = Vec::new();
    for pos in &tiles {
        let Some(tile) = game.map.get_tile_mut(*pos) else { continue };
        let mut kept: Vec<crate::map::tile::MapItem> = Vec::new();
        for item in std::mem::take(&mut tile.items) {
            let it = game.items.get_item_type(usize::from(item.server_id));
            if it.pickupable {
                moved.push(item);
            } else if it.group == crate::items::ItemGroup::Container {
                moved.extend(item.children.iter().cloned());
                let mut emptied = item;
                emptied.children.clear();
                kept.push(emptied);
            } else {
                kept.push(item);
            }
        }
        if let Some(tile) = game.map.get_tile_mut(*pos) {
            tile.items = kept;
        }
    }

    if moved.is_empty() {
        return true;
    }
    if let Some(p) = game.get_player_mut(cid) {
        let depot = p.depot_items.entry(town_id).or_default();
        for item in moved {
            depot.insert(0, item);
        }
    }
    true
}

pub fn set_owner(game: &mut crate::game::Game, house_id: u32, guid: u32) {
    let (is_loaded, owner, entry, beds, _town_id) = match game.map.houses.get_house(house_id) {
        Some(h) => (h.is_loaded, h.owner, h.entry_position, h.beds.clone(), h.town_id),
        None => return,
    };

    if is_loaded && owner == guid {
        return;
    }
    if let Some(h) = game.map.houses.get_house_mut(house_id) {
        h.is_loaded = true;
    }

    if owner != 0 {
        transfer_to_depot(game, house_id);

        let tiles: Vec<crate::map::Position> = game
            .map
            .houses
            .get_house(house_id)
            .map(|h| h.tiles.clone())
            .unwrap_or_default();
        let mut to_kick: Vec<crate::creatures::CreatureId> = Vec::new();
        for pos in &tiles {
            if let Some(tile) = game.map.get_tile(*pos) {
                for &cid in &tile.creature_ids {
                    let can_edit = game
                        .get_player(cid)
                        .map(|p| p.group_flags & crate::creatures::player::PLAYER_FLAG_CAN_EDIT_HOUSES != 0)
                        .unwrap_or(true);
                    if !can_edit {
                        to_kick.push(cid);
                    }
                }
            }
        }
        for cid in to_kick {
            let old_pos = game.get_player(cid).map(|p| p.base.position);
            if let Some(old_pos) = old_pos {
                game.move_creature_position(cid, old_pos, entry);
                game.add_magic_effect(old_pos, crate::game::CONST_ME_POFF);
                game.add_magic_effect(entry, crate::game::CONST_ME_TELEPORT);
            }
        }

        for bed_pos in &beds {
            let bed_sid = game.map.get_tile(*bed_pos).and_then(|t| {
                t.items.iter()
                    .find(|it| game.items.get_item_type(usize::from(it.server_id)).kind == crate::items::ItemKind::Bed)
                    .map(|it| (it.server_id, it.sleeper_guid))
            });
            if let Some((sid, sleeper)) = bed_sid {
                if sleeper != 0 {
                    game.wake_bed_at(*bed_pos, sid);
                }
            }
        }

        if let Some(h) = game.map.houses.get_house_mut(house_id) {
            h.owner = 0;
            h.owner_account_id = 0;
            h.owner_name = String::new();
            h.guest_list.parse_list("");
            h.sub_owner_list.parse_list("");
        }
    }

    if let Some(h) = game.map.houses.get_house_mut(house_id) {
        h.rent_warnings = 0;
        if guid != 0 {
            h.owner = guid;
        }
    }
}

#[derive(Debug, Error)]
pub enum HouseLoadError {
    #[error(transparent)]
    Xml(#[from] XmlLoadError),
    #[error("unknown house id {0}")]
    UnknownHouseId(u32),
}

#[derive(Debug, Deserialize)]
struct HousesXml {
    #[serde(rename = "house", default)]
    house: Vec<HouseXml>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct HouseXml {
    #[serde(rename = "@houseid")]
    houseid: u32,
    #[serde(rename = "@name")]
    name: String,
    #[serde(rename = "@entryx")]
    entryx: u16,
    #[serde(rename = "@entryy")]
    entryy: u16,
    #[serde(rename = "@entryz")]
    entryz: u8,
    #[serde(rename = "@rent")]
    rent: u32,
    #[serde(rename = "@townid")]
    townid: u32,
    #[serde(rename = "@size")]
    size: Option<u16>,
}
