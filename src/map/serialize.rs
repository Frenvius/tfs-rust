#![allow(dead_code)]

use anyhow::Result;

use crate::db::{Database, DatabaseEngine};

pub const ATTR_CONTAINER_ITEMS: u8 = 23;

pub struct IOMapSerialize;

impl IOMapSerialize {
    /// Load owner/paid/warnings and access lists from `houses` + `house_lists`.
    /// Mirrors C++ `IOMapSerialize::loadHouseInfo`.
    pub async fn load_house_info(db: &Database) -> Result<()> {
        use crate::game::g_game;

        if let Some(mut r) = db.store_query("SELECT `id`, `owner`, `paid`, `warnings` FROM `houses`").await? {
            loop {
                let id = r.get_u64("id").unwrap_or(0) as u32;
                let owner = r.get_u64("owner").unwrap_or(0) as u32;
                let paid = r.get_i64("paid").unwrap_or(0);
                let warnings = r.get_u64("warnings").unwrap_or(0) as u32;
                {
                    let mut game = g_game().lock().unwrap();
                    if let Some(house) = game.map.houses.get_house_mut(id) {
                        house.set_owner(owner);
                        house.set_paid_until(paid);
                        house.set_pay_rent_warnings(warnings);
                    }
                }
                if !r.next() { break; }
            }
        }

        if let Some(mut r) = db.store_query("SELECT `house_id`, `listid`, `list` FROM `house_lists`").await? {
            loop {
                let house_id = r.get_u64("house_id").unwrap_or(0) as u32;
                let list_id = r.get_u64("listid").unwrap_or(0) as u32;
                let list = r.get_string("list").unwrap_or_default();
                {
                    let mut game = g_game().lock().unwrap();
                    if let Some(house) = game.map.houses.get_house_mut(house_id) {
                        house.set_access_list(list_id, &list);
                    }
                }
                if !r.next() { break; }
            }
        }
        Ok(())
    }

    /// Persist owner/paid/warnings rows and access lists.
    /// Mirrors C++ `IOMapSerialize::saveHouseInfo`.
    pub async fn save_house_info(db: &Database) -> Result<()> {
        use crate::game::g_game;

        // Snapshot house data (owner, rent, lists, ...) without holding the lock across awaits.
        struct HouseRow {
            id: u32, owner: u32, paid: i64, warnings: u32, name: String,
            town_id: u32, rent: u32, size: usize, beds: usize,
            guest: String, subowner: String,
        }
        let rows: Vec<HouseRow> = {
            let game = g_game().lock().unwrap();
            game.map.houses.get_houses().values().map(|h| HouseRow {
                id: h.id, owner: h.owner, paid: h.paid_until, warnings: h.rent_warnings,
                name: h.name.clone(), town_id: h.town_id, rent: h.rent,
                size: h.tiles.len(), beds: h.get_bed_count(),
                guest: h.get_access_list(crate::map::houses::GUEST_LIST).unwrap_or("").to_owned(),
                subowner: h.get_access_list(crate::map::houses::SUBOWNER_LIST).unwrap_or("").to_owned(),
            }).collect()
        };

        if !db.execute("DELETE FROM `house_lists`").await? {
            return Ok(());
        }

        for h in &rows {
            let name = db.escape_string(&h.name);
            let exists = db.store_query(&format!("SELECT `id` FROM `houses` WHERE `id` = {}", h.id)).await?.is_some();
            let q = if exists {
                format!("UPDATE `houses` SET `owner` = {}, `paid` = {}, `warnings` = {}, `name` = {name}, `town_id` = {}, `rent` = {}, `size` = {}, `beds` = {} WHERE `id` = {}",
                    h.owner, h.paid, h.warnings, h.town_id, h.rent, h.size, h.beds, h.id)
            } else {
                format!("INSERT INTO `houses` (`id`, `owner`, `paid`, `warnings`, `name`, `town_id`, `rent`, `size`, `beds`) VALUES ({}, {}, {}, {}, {name}, {}, {}, {}, {})",
                    h.id, h.owner, h.paid, h.warnings, h.town_id, h.rent, h.size, h.beds)
            };
            let _ = db.execute(&q).await;
        }

        let mut list_rows: Vec<String> = Vec::new();
        for h in &rows {
            if !h.guest.is_empty() {
                list_rows.push(format!("({}, {}, {})", h.id, crate::map::houses::GUEST_LIST, db.escape_string(&h.guest)));
            }
            if !h.subowner.is_empty() {
                list_rows.push(format!("({}, {}, {})", h.id, crate::map::houses::SUBOWNER_LIST, db.escape_string(&h.subowner)));
            }
        }
        if !list_rows.is_empty() {
            let q = format!("INSERT INTO `house_lists` (`house_id`, `listid`, `list`) VALUES {}", list_rows.join(", "));
            let _ = db.execute(&q).await;
        }
        Ok(())
    }

    /// Load movable house items from `tile_store` binary blobs onto map tiles.
    /// Mirrors C++ `IOMapSerialize::loadHouseItems`.
    pub async fn load_house_items(db: &Database) -> Result<()> {
        use crate::game::g_game;

        let Some(mut r) = db.store_query("SELECT `data` FROM `tile_store`").await? else {
            return Ok(());
        };
        loop {
            if let Some(blob) = r.get_bytes("data") {
                let mut data: &[u8] = &blob;
                if let (Some(x), Some(y), Some(z), Some(count)) =
                    (rd_u16(&mut data), rd_u16(&mut data), rd_u8(&mut data), rd_u32(&mut data))
                {
                    let pos = Position { x, y, z };
                    let mut items = Vec::with_capacity(count as usize);
                    let mut ok = true;
                    for _ in 0..count {
                        match deserialize_item(&mut data) {
                            Some(it) => items.push(it),
                            None => { ok = false; break; }
                        }
                    }
                    if ok {
                        let mut game = g_game().lock().unwrap();
                        let items_arc = game.items.clone();
                        if let Some(tile) = game.map.get_tile_mut(pos) {
                            for it in &items {
                                tile.internal_add_item(it.clone(), &items_arc);
                            }
                        }
                        for it in &items {
                            game.start_decay(it.server_id, pos);
                        }
                    }
                }
            }
            if !r.next() { break; }
        }
        Ok(())
    }

    /// Persist movable house items into `tile_store`.
    /// Mirrors C++ `IOMapSerialize::saveHouseItems`.
    pub async fn save_house_items(db: &Database) -> Result<()> {
        use crate::game::g_game;

        // Build (house_id, blob) rows under the lock, then write outside it.
        let rows: Vec<(u32, Vec<u8>)> = {
            let game = g_game().lock().unwrap();
            let items_arc = game.items.clone();
            let mut out = Vec::new();
            for house in game.map.houses.get_houses().values() {
                for &pos in &house.tiles {
                    let Some(tile) = game.map.get_tile(pos) else { continue };
                    let qualifying: Vec<&MapItem> = tile.items.iter().filter(|item| {
                        let it = items_arc.get_item_type(usize::from(item.server_id));
                        it.moveable
                            || it.force_serialize
                            || it.can_write_text
                            || it.kind == crate::items::ItemKind::Door
                            || it.kind == crate::items::ItemKind::Bed
                            || (it.kind == crate::items::ItemKind::Container && !item.children.is_empty())
                    }).collect();
                    if qualifying.is_empty() { continue; }
                    let mut blob = Vec::new();
                    blob.extend_from_slice(&pos.x.to_le_bytes());
                    blob.extend_from_slice(&pos.y.to_le_bytes());
                    blob.push(pos.z);
                    blob.extend_from_slice(&(qualifying.len() as u32).to_le_bytes());
                    for item in qualifying {
                        serialize_item(item, &items_arc, &mut blob);
                    }
                    out.push((house.id, blob));
                }
            }
            out
        };

        if !db.execute("DELETE FROM `tile_store`").await? {
            return Ok(());
        }
        if !rows.is_empty() {
            let values: Vec<String> = rows.iter()
                .map(|(hid, blob)| format!("({hid}, {})", db.escape_blob(blob)))
                .collect();
            let q = format!("INSERT INTO `tile_store` (`house_id`, `data`) VALUES {}", values.join(", "));
            let _ = db.execute(&q).await;
        }
        Ok(())
    }

    /// Charge house rent from owners' bank balances. Mirrors C++
    /// `Houses::payHouses`: deduct rent and extend `paid_until` when the owner
    /// can pay, otherwise increment warnings and evict after 7. The rent period
    /// comes from the `houseRentPeriod` config (`never` => no-op, the default).
    ///
    /// GAP vs C++: the warning letter delivered to the owner's depot locker is
    /// not sent (depot/inbox integration is still stubbed) — only the warning
    /// counter and eventual eviction are applied.
    pub async fn pay_houses(db: &Database) -> Result<()> {
        use crate::config::{g_config, StringConfig};
        use crate::game::g_game;
        use crate::map::houses::RentPeriod;

        let period = match g_config().get_string(StringConfig::HouseRentPeriod).to_lowercase().as_str() {
            "daily" => RentPeriod::Daily,
            "weekly" => RentPeriod::Weekly,
            "monthly" => RentPeriod::Monthly,
            "yearly" => RentPeriod::Yearly,
            _ => return Ok(()), // "never" / unknown => rent disabled
        };
        let period_secs: i64 = match period {
            RentPeriod::Daily => 24 * 60 * 60,
            RentPeriod::Weekly => 24 * 60 * 60 * 7,
            RentPeriod::Monthly => 24 * 60 * 60 * 30,
            RentPeriod::Yearly => 24 * 60 * 60 * 365,
            RentPeriod::Never => return Ok(()),
        };

        let now = crate::util::get_milliseconds_time() / 1000;

        // Snapshot houses that owe rent.
        let due: Vec<(u32, u32, u32, u32)> = {
            let game = g_game().lock().unwrap();
            game.map.houses.get_houses().values()
                .filter(|h| h.owner != 0 && h.rent != 0 && h.paid_until <= now)
                .map(|h| (h.id, h.owner, h.rent, h.rent_warnings))
                .collect()
        };

        for (house_id, owner, rent, warnings) in due {
            // Read the (possibly offline) owner's current bank balance.
            let balance = match db.store_query(&format!(
                "SELECT `balance` FROM `players` WHERE `id` = {owner}"
            )).await? {
                Some(r) => r.get_u64("balance").unwrap_or(0),
                None => {
                    // Owner no longer exists — reset house owner.
                    let mut game = g_game().lock().unwrap();
                    if let Some(h) = game.map.houses.get_house_mut(house_id) { h.set_owner(0); }
                    continue;
                }
            };

            if balance >= rent as u64 {
                let _ = db.execute(&format!(
                    "UPDATE `players` SET `balance` = `balance` - {rent} WHERE `id` = {owner}"
                )).await;
                let mut game = g_game().lock().unwrap();
                if let Some(h) = game.map.houses.get_house_mut(house_id) {
                    h.set_paid_until(now + period_secs);
                }
            } else if warnings < 7 {
                let days_left = 7 - warnings as i32;
                let period_str = match period {
                    RentPeriod::Daily => "daily",
                    RentPeriod::Weekly => "weekly",
                    RentPeriod::Monthly => "monthly",
                    RentPeriod::Yearly => "annual",
                    RentPeriod::Never => "",
                };
                let mut game = g_game().lock().unwrap();
                let (town_id, house_name) = game
                    .map
                    .houses
                    .get_house(house_id)
                    .map(|h| (h.town_id, h.name.clone()))
                    .unwrap_or((0, String::new()));
                // Deliver the stamped warning letter to the owner's depot when
                // they are online (full text preserved in memory). Offline
                // delivery is not done: the player-item DB layer persists only
                // itemtype+count, not the attribute blob, so a relogged owner
                // would receive a blank letter — worse than not delivering.
                if let Some(cid) = game.get_player_id_by_guid(owner) {
                    let text = format!(
                        "Warning! \nThe {period_str} rent of {rent} gold for your house \"{house_name}\" is payable. Have it within {days_left} days or you will lose this house."
                    );
                    let letter = crate::map::tile::MapItem {
                        server_id: 2598, // ITEM_LETTER_STAMPED
                        count: 1,
                        text,
                        ..crate::map::tile::MapItem::default()
                    };
                    if let Some(p) = game.get_player_mut(cid) {
                        p.depot_items.entry(town_id).or_default().insert(0, letter);
                    }
                }
                if let Some(h) = game.map.houses.get_house_mut(house_id) {
                    h.set_pay_rent_warnings(warnings + 1);
                }
            } else {
                let mut game = g_game().lock().unwrap();
                game.house_set_owner(house_id, 0);
            }
        }
        Ok(())
    }
}

pub fn save_item(item_id: u16, item_attrs: &[u8], container_items: Option<&[SavedItem]>, out: &mut Vec<u8>) {
    out.extend_from_slice(&item_id.to_le_bytes());
    out.extend_from_slice(item_attrs);

    if let Some(items) = container_items {
        out.push(ATTR_CONTAINER_ITEMS);
        let count = items.len() as u32;
        out.extend_from_slice(&count.to_le_bytes());
        for child in items.iter().rev() {
            save_item(child.id, &child.attrs, child.container_items.as_deref(), out);
        }
    }

    out.push(0x00);
}

pub fn save_tile(tile_items: &[SavedItem], tile_x: u16, tile_y: u16, tile_z: u8, out: &mut Vec<u8>) {
    if tile_items.is_empty() {
        return;
    }

    out.extend_from_slice(&tile_x.to_le_bytes());
    out.extend_from_slice(&tile_y.to_le_bytes());
    out.push(tile_z);

    let count = tile_items.len() as u32;
    out.extend_from_slice(&count.to_le_bytes());
    for item in tile_items {
        save_item(item.id, &item.attrs, item.container_items.as_deref(), out);
    }
}

#[allow(clippy::result_unit_err)]
pub fn load_container(data: &mut &[u8]) -> Result<Vec<(u16, Vec<u8>)>, ()> {
    let mut items = Vec::new();

    loop {
        let id = load_u16_le(data)?;
        let attrs = load_item_attrs(data)?;
        items.push((id, attrs));

        if data.is_empty() {
            return Err(());
        }

        let end = data[0];
        *data = &data[1..];
        if end == 0 {
            break;
        }
    }

    Ok(items)
}

#[allow(clippy::result_unit_err)]
pub fn load_item(data: &mut &[u8]) -> Result<(u16, Vec<u8>), ()> {
    let id = load_u16_le(data)?;
    let attrs = load_item_attrs(data)?;
    Ok((id, attrs))
}

fn load_u16_le(data: &mut &[u8]) -> Result<u16, ()> {
    if data.len() < 2 {
        return Err(());
    }
    let v = u16::from_le_bytes([data[0], data[1]]);
    *data = &data[2..];
    Ok(v)
}

fn load_item_attrs(data: &mut &[u8]) -> Result<Vec<u8>, ()> {
    let mut attrs = Vec::new();

    loop {
        if data.is_empty() {
            break;
        }
        let attr_type = data[0];
        if attr_type == 0 || attr_type == ATTR_CONTAINER_ITEMS {
            *data = &data[1..];
            break;
        }
        attrs.push(attr_type);
        *data = &data[1..];

        let payload = read_attr_payload(attr_type, data)?;
        attrs.extend_from_slice(&payload);
    }

    Ok(attrs)
}

fn read_attr_payload(attr_type: u8, data: &mut &[u8]) -> Result<Vec<u8>, ()> {
    let len = attr_payload_len(attr_type, data)?;
    if data.len() < len {
        return Err(());
    }
    let payload = data[..len].to_vec();
    *data = &data[len..];
    Ok(payload)
}

fn attr_payload_len(attr_type: u8, data: &[u8]) -> Result<usize, ()> {
    match attr_type {
        3 => Ok(4),
        4 => Ok(2),
        5 => Ok(2),
        6 => read_str_len(data),
        7 => read_str_len(data),
        8 => Ok(5),
        9 => Ok(2),
        10 => Ok(2),
        12 => Ok(2),
        14 => Ok(1),
        15 => Ok(1),
        16 => Ok(4),
        17 => Ok(1),
        18 => Ok(4),
        19 => read_str_len(data),
        20 => Ok(4),
        21 => Ok(4),
        22 => Ok(2),
        24 => read_str_len(data),
        25 => read_str_len(data),
        26 => read_str_len(data),
        27 => Ok(4),
        28 => Ok(2),
        29 => Ok(2),
        30 => Ok(2),
        31 => Ok(2),
        32 => Ok(1),
        33 => Ok(1),
        34 => read_custom_attrs_len(data),
        35 => Ok(2),
        38 => Ok(4),
        _ => Err(()),
    }
}

fn read_str_len(data: &[u8]) -> Result<usize, ()> {
    if data.len() < 2 {
        return Err(());
    }
    let slen = u16::from_le_bytes([data[0], data[1]]) as usize;
    Ok(2 + slen)
}

fn read_custom_attrs_len(data: &[u8]) -> Result<usize, ()> {
    if data.len() < 8 {
        return Err(());
    }
    let count = u64::from_le_bytes([
        data[0], data[1], data[2], data[3],
        data[4], data[5], data[6], data[7],
    ]) as usize;
    let mut pos = 8usize;
    for _ in 0..count {
        if pos + 2 > data.len() { return Err(()); }
        let klen = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2 + klen;
        if pos >= data.len() { return Err(()); }
        let vtype = data[pos];
        pos += 1;
        let vsize = match vtype {
            1 => {
                if pos + 2 > data.len() { return Err(()); }
                2 + u16::from_le_bytes([data[pos], data[pos + 1]]) as usize
            }
            2 | 3 => 4,
            4 => 1,
            _ => return Err(()),
        };
        if pos + vsize > data.len() { return Err(()); }
        pos += vsize;
    }
    Ok(pos)
}

#[derive(Debug, Clone)]
pub struct SavedItem {
    pub id: u16,
    pub attrs: Vec<u8>,
    pub container_items: Option<Vec<SavedItem>>,
}

// ── MapItem <-> binary attribute serialization (mirrors C++ Item::serializeAttr
//    / unserializeAttr, used by IOMapSerialize tile_store and player_items) ────

use crate::items::Items;
use crate::map::tile::MapItem;
use crate::map::Position;

const ATTR_ACTION_ID: u8 = 4;
const ATTR_UNIQUE_ID: u8 = 5;
const ATTR_TEXT: u8 = 6;
const ATTR_DESC: u8 = 7;
const ATTR_TELE_DEST: u8 = 8;
const ATTR_DEPOT_ID: u8 = 10;
const ATTR_RUNE_CHARGES: u8 = 12;
const ATTR_HOUSEDOORID: u8 = 14;
const ATTR_COUNT: u8 = 15;
const ATTR_DURATION: u8 = 16;
const ATTR_DECAYING_STATE: u8 = 17;
const ATTR_WRITTENDATE: u8 = 18;
const ATTR_WRITTENBY: u8 = 19;
const ATTR_SLEEPERGUID: u8 = 20;
const ATTR_SLEEPSTART: u8 = 21;
const ATTR_CHARGES: u8 = 22;
const ATTR_NAME: u8 = 24;
const ATTR_ARTICLE: u8 = 25;
const ATTR_PLURALNAME: u8 = 26;
const ATTR_WEIGHT: u8 = 27;
const ATTR_ATTACK: u8 = 28;
const ATTR_DEFENSE: u8 = 29;
const ATTR_EXTRADEFENSE: u8 = 30;
const ATTR_ARMOR: u8 = 31;
const ATTR_HITCHANCE: u8 = 32;
const ATTR_SHOOTRANGE: u8 = 33;
const ATTR_DECAYTO: u8 = 35;

fn write_str(out: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    out.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
    out.extend_from_slice(bytes);
}

/// Serialize a `MapItem` (id + attributes + container recursion + 0x00 end),
/// mirroring C++ `Item::serializeAttr` write order. `tile_item` selects the
/// `Items` registry used to decide stackable/fluid/splash for ATTR_COUNT.
pub fn serialize_item(item: &MapItem, items: &Items, out: &mut Vec<u8>) {
    out.extend_from_slice(&item.server_id.to_le_bytes());
    write_item_attrs(item, items, out);

    // Container children (ATTR_CONTAINER_ITEMS + count + reversed children).
    let it = items.get_item_type(usize::from(item.server_id));
    if it.group == crate::items::ItemGroup::Container && !item.children.is_empty() {
        out.push(ATTR_CONTAINER_ITEMS);
        out.extend_from_slice(&(item.children.len() as u32).to_le_bytes());
        for child in item.children.iter().rev() {
            serialize_item(child, items, out);
        }
    }

    out.push(0x00); // attr end
}

/// Write only an item's attribute TLVs (no server-id prefix, no container
/// recursion, no end byte). Port of `Item::serializeAttr` — this is the exact
/// blob stored in the `attributes` column of `player_items`/`player_depotitems`.
pub fn write_item_attrs(item: &MapItem, items: &Items, out: &mut Vec<u8>) {
    let it = items.get_item_type(usize::from(item.server_id));
    let is_count_type = it.stackable
        || it.group == crate::items::ItemGroup::Fluid
        || it.group == crate::items::ItemGroup::Splash;
    if is_count_type {
        out.push(ATTR_COUNT);
        out.push(item.count.min(255) as u8);
    }
    if item.charges != 0 {
        out.push(ATTR_CHARGES);
        out.extend_from_slice(&item.charges.to_le_bytes());
    }
    if it.moveable && item.action_id != 0 {
        out.push(ATTR_ACTION_ID);
        out.extend_from_slice(&item.action_id.to_le_bytes());
    }
    if item.unique_id != 0 {
        out.push(ATTR_UNIQUE_ID);
        out.extend_from_slice(&item.unique_id.to_le_bytes());
    }
    if !item.text.is_empty() {
        out.push(ATTR_TEXT);
        write_str(out, &item.text);
    }
    if item.written_date != 0 {
        out.push(ATTR_WRITTENDATE);
        out.extend_from_slice(&item.written_date.to_le_bytes());
    }
    if !item.written_by.is_empty() {
        out.push(ATTR_WRITTENBY);
        write_str(out, &item.written_by);
    }
    if !item.description.is_empty() {
        out.push(ATTR_DESC);
        write_str(out, &item.description);
    }
    if item.duration != 0 {
        out.push(ATTR_DURATION);
        out.extend_from_slice(&(item.duration as u32).to_le_bytes());
    }
    if item.decaying_state != 0 {
        out.push(ATTR_DECAYING_STATE);
        out.push(item.decaying_state);
    }
    if !item.name.is_empty() {
        out.push(ATTR_NAME);
        write_str(out, &item.name);
    }
    if !item.article.is_empty() {
        out.push(ATTR_ARTICLE);
        write_str(out, &item.article);
    }
    if !item.plural_name.is_empty() {
        out.push(ATTR_PLURALNAME);
        write_str(out, &item.plural_name);
    }
    if item.weight != 0 {
        out.push(ATTR_WEIGHT);
        out.extend_from_slice(&item.weight.to_le_bytes());
    }
    if item.attack != 0 {
        out.push(ATTR_ATTACK);
        out.extend_from_slice(&item.attack.to_le_bytes());
    }
    if item.defense != 0 {
        out.push(ATTR_DEFENSE);
        out.extend_from_slice(&item.defense.to_le_bytes());
    }
    if item.extra_defense != 0 {
        out.push(ATTR_EXTRADEFENSE);
        out.extend_from_slice(&item.extra_defense.to_le_bytes());
    }
    if item.armor != 0 {
        out.push(ATTR_ARMOR);
        out.extend_from_slice(&item.armor.to_le_bytes());
    }
    if item.hit_chance != 0 {
        out.push(ATTR_HITCHANCE);
        out.push(item.hit_chance as u8);
    }
    if item.shoot_range != 0 {
        out.push(ATTR_SHOOTRANGE);
        out.push(item.shoot_range);
    }
    if item.decay_to != 0 {
        out.push(ATTR_DECAYTO);
        out.extend_from_slice(&(item.decay_to as u32).to_le_bytes());
    }
    if item.depot_id != 0 {
        out.push(ATTR_DEPOT_ID);
        out.extend_from_slice(&item.depot_id.to_le_bytes());
    }
    if item.rune_charges != 0 {
        out.push(ATTR_RUNE_CHARGES);
        out.push(item.rune_charges);
    }
    if item.house_door_id != 0 {
        out.push(ATTR_HOUSEDOORID);
        out.push(item.house_door_id);
    }
    if let Some(dest) = item.teleport_destination {
        out.push(ATTR_TELE_DEST);
        out.extend_from_slice(&dest.x.to_le_bytes());
        out.extend_from_slice(&dest.y.to_le_bytes());
        out.push(dest.z);
    }
    if item.sleeper_guid != 0 {
        out.push(ATTR_SLEEPERGUID);
        out.extend_from_slice(&item.sleeper_guid.to_le_bytes());
    }
    if item.sleep_start != 0 {
        out.push(ATTR_SLEEPSTART);
        out.extend_from_slice(&item.sleep_start.to_le_bytes());
    }
}

fn rd_u8(d: &mut &[u8]) -> Option<u8> { let v = *d.first()?; *d = &d[1..]; Some(v) }
fn rd_u16(d: &mut &[u8]) -> Option<u16> { if d.len() < 2 { return None; } let v = u16::from_le_bytes([d[0], d[1]]); *d = &d[2..]; Some(v) }
fn rd_u32(d: &mut &[u8]) -> Option<u32> { if d.len() < 4 { return None; } let v = u32::from_le_bytes([d[0], d[1], d[2], d[3]]); *d = &d[4..]; Some(v) }
fn rd_str(d: &mut &[u8]) -> Option<String> {
    let len = rd_u16(d)? as usize;
    if d.len() < len { return None; }
    let s = String::from_utf8_lossy(&d[..len]).into_owned();
    *d = &d[len..];
    Some(s)
}

/// Deserialize one `MapItem` (id + attribute loop + nested container items).
/// Mirrors the OTBM loader's attribute application and `loadContainer`.
pub fn deserialize_item(data: &mut &[u8]) -> Option<MapItem> {
    let server_id = rd_u16(data)?;
    let mut item = MapItem { server_id, count: 1, ..MapItem::default() };

    loop {
        let attr = rd_u8(data)?;
        if attr == 0 {
            break;
        }
        if attr == ATTR_CONTAINER_ITEMS {
            let count = rd_u32(data)?;
            for _ in 0..count {
                let child = deserialize_item(data)?;
                item.children.push(child);
            }
            // C++ writes children then the parent's 0x00 end follows.
            continue;
        }
        apply_item_attr(&mut item, attr, data)?;
    }

    Some(item)
}

/// Read attribute TLVs into an existing item from a player-item `attributes`
/// blob (no server-id prefix, no container recursion, no required terminator).
/// Port of `Item::unserializeAttr` for the player-item DB columns.
pub fn read_item_attrs(item: &mut MapItem, data: &mut &[u8]) -> Option<()> {
    while !data.is_empty() {
        let attr = rd_u8(data)?;
        if attr == 0 {
            break;
        }
        apply_item_attr(item, attr, data)?;
    }
    Some(())
}

fn apply_item_attr(item: &mut MapItem, attr: u8, data: &mut &[u8]) -> Option<()> {
    match attr {
        ATTR_COUNT => item.count = u16::from(rd_u8(data)?),
        ATTR_RUNE_CHARGES => item.rune_charges = rd_u8(data)?,
        ATTR_ACTION_ID => item.action_id = rd_u16(data)?,
        ATTR_UNIQUE_ID => item.unique_id = rd_u16(data)?,
        ATTR_TEXT => item.text = rd_str(data)?,
        ATTR_DESC => item.description = rd_str(data)?,
        ATTR_TELE_DEST => {
            let x = rd_u16(data)?;
            let y = rd_u16(data)?;
            let z = rd_u8(data)?;
            item.teleport_destination = Some(Position { x, y, z });
        }
        ATTR_DEPOT_ID => item.depot_id = rd_u16(data)?,
        ATTR_HOUSEDOORID => item.house_door_id = rd_u8(data)?,
        ATTR_DURATION => item.duration = rd_u32(data)? as i32,
        ATTR_DECAYING_STATE => item.decaying_state = rd_u8(data)?,
        ATTR_WRITTENDATE => item.written_date = rd_u32(data)?,
        ATTR_WRITTENBY => item.written_by = rd_str(data)?,
        ATTR_SLEEPERGUID => item.sleeper_guid = rd_u32(data)?,
        ATTR_SLEEPSTART => item.sleep_start = rd_u32(data)?,
        ATTR_CHARGES => item.charges = rd_u16(data)?,
        ATTR_NAME => item.name = rd_str(data)?,
        ATTR_ARTICLE => item.article = rd_str(data)?,
        ATTR_PLURALNAME => item.plural_name = rd_str(data)?,
        ATTR_WEIGHT => item.weight = rd_u32(data)?,
        ATTR_ATTACK => item.attack = rd_u32(data)? as i32,
        ATTR_DEFENSE => item.defense = rd_u32(data)? as i32,
        ATTR_EXTRADEFENSE => item.extra_defense = rd_u32(data)? as i32,
        ATTR_ARMOR => item.armor = rd_u32(data)? as i32,
        ATTR_HITCHANCE => item.hit_chance = rd_u8(data)? as i8,
        ATTR_SHOOTRANGE => item.shoot_range = rd_u8(data)?,
        ATTR_DECAYTO => item.decay_to = rd_u32(data)? as i32,
        _ => return None,
    }
    Some(())
}
