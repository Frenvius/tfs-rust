use crate::creatures::CreatureId;
use crate::items::{ItemKind, Items};

use super::Position;

pub const TILESTATE_NONE: u32 = 0;
pub const TILESTATE_FLOORCHANGE_DOWN: u32 = 1 << 0;
pub const TILESTATE_FLOORCHANGE_NORTH: u32 = 1 << 1;
pub const TILESTATE_FLOORCHANGE_SOUTH: u32 = 1 << 2;
pub const TILESTATE_FLOORCHANGE_EAST: u32 = 1 << 3;
pub const TILESTATE_FLOORCHANGE_WEST: u32 = 1 << 4;
pub const TILESTATE_FLOORCHANGE_SOUTH_ALT: u32 = 1 << 5;
pub const TILESTATE_FLOORCHANGE_EAST_ALT: u32 = 1 << 6;
pub const TILESTATE_PROTECTIONZONE: u32 = 1 << 7;
pub const TILESTATE_NOPVPZONE: u32 = 1 << 8;
pub const TILESTATE_NOLOGOUT: u32 = 1 << 9;
pub const TILESTATE_PVPZONE: u32 = 1 << 10;
pub const TILESTATE_TELEPORT: u32 = 1 << 11;
pub const TILESTATE_MAGICFIELD: u32 = 1 << 12;
pub const TILESTATE_MAILBOX: u32 = 1 << 13;
pub const TILESTATE_TRASHHOLDER: u32 = 1 << 14;
pub const TILESTATE_BED: u32 = 1 << 15;
pub const TILESTATE_DEPOT: u32 = 1 << 16;
pub const TILESTATE_BLOCKSOLID: u32 = 1 << 17;
pub const TILESTATE_BLOCKPATH: u32 = 1 << 18;
pub const TILESTATE_IMMOVABLEBLOCKSOLID: u32 = 1 << 19;
pub const TILESTATE_IMMOVABLEBLOCKPATH: u32 = 1 << 20;
pub const TILESTATE_IMMOVABLENOFIELDBLOCKPATH: u32 = 1 << 21;
pub const TILESTATE_NOFIELDBLOCKPATH: u32 = 1 << 22;
pub const TILESTATE_SUPPORTS_HANGABLE: u32 = 1 << 23;
pub const TILESTATE_FLOORCHANGE: u32 = TILESTATE_FLOORCHANGE_DOWN
    | TILESTATE_FLOORCHANGE_NORTH
    | TILESTATE_FLOORCHANGE_SOUTH
    | TILESTATE_FLOORCHANGE_EAST
    | TILESTATE_FLOORCHANGE_WEST
    | TILESTATE_FLOORCHANGE_SOUTH_ALT
    | TILESTATE_FLOORCHANGE_EAST_ALT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileKind {
    Static,
    Dynamic,
    House { house_id: u32 },
}

#[derive(Debug, Clone, Default)]
pub struct MapItem {
    pub server_id: u16,
    pub count: u16,
    pub action_id: u16,
    pub unique_id: u16,
    pub text: String,
    pub description: String,
    pub teleport_destination: Option<Position>,
    pub depot_id: u16,
    pub rune_charges: u8,
    pub house_door_id: u8,
    pub charges: u16,
    pub duration: i32,
    pub decaying_state: u8,
    pub written_date: u32,
    pub written_by: String,
    pub sleeper_guid: u32,
    pub sleep_start: u32,
    pub name: String,
    pub article: String,
    pub plural_name: String,
    pub weight: u32,
    pub attack: i32,
    pub defense: i32,
    pub extra_defense: i32,
    pub armor: i32,
    pub hit_chance: i8,
    pub shoot_range: u8,
    pub decay_to: i32,
    pub fluid_type: u16,
    pub loaded_from_map: bool,
    pub custom_attributes: std::collections::HashMap<String, CustomAttributeValue>,
    pub children: Vec<MapItem>,
    pub owner_id: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CustomAttributeValue {
    String(String),
    Int(i64),
    Double(f64),
    Bool(bool),
}

#[derive(Debug, Clone)]
pub struct Tile {
    pub position: Position,
    pub kind: TileKind,
    pub flags: u32,
    pub ground: Option<MapItem>,
    pub items: Vec<MapItem>,
    pub creature_ids: Vec<CreatureId>,
    down_item_count: u16,
}

impl Tile {
    pub fn new(position: Position, kind: TileKind) -> Self {
        Self {
            position,
            kind,
            flags: TILESTATE_NONE,
            ground: None,
            items: Vec::new(),
            creature_ids: Vec::new(),
            down_item_count: 0,
        }
    }

    pub fn has_flag(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }

    pub fn set_flag(&mut self, flag: u32) {
        self.flags |= flag;
    }

    pub fn get_top_item_count(&self) -> usize {
        self.items
            .len()
            .saturating_sub(usize::from(self.down_item_count))
    }

    pub fn get_down_item_count(&self) -> usize {
        usize::from(self.down_item_count)
    }

    pub fn get_top_top_item(&self) -> Option<&MapItem> {
        if self.get_top_item_count() == 0 {
            return None;
        }

        self.items.last()
    }

    pub fn get_top_down_item(&self) -> Option<&MapItem> {
        if self.down_item_count == 0 {
            return None;
        }

        self.items.first()
    }

    pub fn get_use_item(&self, stackpos: u8) -> Option<&MapItem> {
        if stackpos == 0 {
            return self.ground.as_ref();
        }

        let mut index = usize::from(stackpos - 1);
        let top_start = self.get_down_item_count();
        let top_items = &self.items[top_start..];
        if index < top_items.len() {
            return top_items.get(index);
        }

        index -= top_items.len();
        self.items.get(index)
    }

    pub fn is_walkable(&self) -> bool {
        self.ground.is_some() && !self.has_flag(TILESTATE_BLOCKSOLID)
    }

    /// Port of `Tile::hasHeight`: number of stacked items (ground + items) with
    /// the HAS_HEIGHT property reaches `n`. Used by the ramp floor-change logic.
    pub fn has_height(&self, n: u32, items: &Items) -> bool {
        let mut height = 0u32;
        if let Some(ground) = &self.ground {
            if items.get_item_type(usize::from(ground.server_id)).has_height {
                height += 1;
            }
            if height == n {
                return true;
            }
        }
        for item in &self.items {
            if items.get_item_type(usize::from(item.server_id)).has_height {
                height += 1;
            }
            if height == n {
                return true;
            }
        }
        false
    }

    pub fn is_moveable_blocking(&self) -> bool {
        self.ground.is_none() || self.has_flag(TILESTATE_BLOCKSOLID)
    }

    /// Returns the client-side stack index of the first creature on this tile.
    /// Mirrors C++ Tile::getClientIndexOfCreature: ground counts as 1, then
    /// each always-on-top item adds 1, then the creature itself is at that index.
    /// For single-creature tiles (the common case), this is exact.
    pub fn add_creature(&mut self, creature_id: CreatureId) {
        self.creature_ids.insert(0, creature_id);
    }

    pub fn remove_creature(&mut self, creature_id: CreatureId) {
        if let Some(pos) = self.creature_ids.iter().position(|&id| id == creature_id) {
            self.creature_ids.remove(pos);
        }
    }

    pub fn get_creature_count(&self) -> usize {
        self.creature_ids.len()
    }

    pub fn get_creatures(&self) -> &[CreatureId] {
        &self.creature_ids
    }

    pub fn get_client_index_of_creature(&self, creature_id: CreatureId) -> i32 {
        let mut n: i32 = if self.ground.is_some() { 1 } else { 0 };
        n += self.get_top_item_count() as i32;

        for &cid in self.creature_ids.iter().rev() {
            if cid == creature_id {
                return n;
            }
            n += 1;
        }
        -1
    }

    pub fn get_creature_client_stackpos(&self) -> u8 {
        let n = if self.ground.is_some() { 1u8 } else { 0u8 };
        n.saturating_add(self.get_top_item_count() as u8)
    }

    pub fn find_item_index_by_server_id(&self, server_id: u16) -> Option<usize> {
        self.items.iter().position(|it| it.server_id == server_id)
    }

    /// Client stack index of the item at `items_index` within `self.items`.
    /// Layout is [down items (0..down_count), top items (down_count..)]; the
    /// client renders ground, then top items, then creatures, then down items.
    pub fn item_client_stackpos(&self, items_index: usize) -> u8 {
        let ground = if self.ground.is_some() { 1u8 } else { 0u8 };
        let down = self.get_down_item_count();
        if items_index >= down {
            ground.saturating_add((items_index - down) as u8)
        } else {
            ground
                .saturating_add(self.get_top_item_count() as u8)
                .saturating_add(items_index as u8)
        }
    }

    pub fn internal_add_item(&mut self, item: MapItem, items: &Items) {
        let item_type = items.get_item_type(usize::from(item.server_id));
        if item_type.is_ground_tile() {
            if self.ground.is_none() {
                self.ground = Some(item);
                self.set_tile_flags(item_type);
            }
            return;
        }

        if item_type.always_on_top {
            let insertion_start = usize::from(self.down_item_count);
            let insertion_index = self.items[insertion_start..]
                .iter()
                .position(|existing| {
                    items
                        .get_item_type(usize::from(existing.server_id))
                        .always_on_top_order
                        > item_type.always_on_top_order
                })
                .map(|offset| insertion_start + offset)
                .unwrap_or(self.items.len());
            self.items.insert(insertion_index, item);
        } else {
            self.items.insert(0, item);
            self.down_item_count = self.down_item_count.saturating_add(1);
        }

        self.set_tile_flags(item_type);
    }

    pub fn has_property_block_projectile(&self, items: &Items) -> bool {
        if let Some(ground) = &self.ground {
            if items.get_item_type(usize::from(ground.server_id)).block_projectile {
                return true;
            }
        }
        for item in &self.items {
            if items.get_item_type(usize::from(item.server_id)).block_projectile {
                return true;
            }
        }
        false
    }

    fn set_tile_flags(&mut self, item_type: &crate::items::ItemType) {
        if !self.has_flag(TILESTATE_FLOORCHANGE) && item_type.floor_change != 0 {
            self.set_flag(item_type.floor_change);
        }

        if item_type.block_solid {
            self.set_flag(TILESTATE_BLOCKSOLID);
            if !item_type.moveable {
                self.set_flag(TILESTATE_IMMOVABLEBLOCKSOLID);
            }
        }

        if item_type.block_path_find {
            self.set_flag(TILESTATE_BLOCKPATH);
            if !item_type.moveable {
                self.set_flag(TILESTATE_IMMOVABLEBLOCKPATH);
            }
        }

        if item_type.no_field_block_path {
            self.set_flag(TILESTATE_NOFIELDBLOCKPATH);
            if !item_type.moveable {
                self.set_flag(TILESTATE_IMMOVABLENOFIELDBLOCKPATH);
            }
        }

        match item_type.kind {
            ItemKind::Depot => self.set_flag(TILESTATE_DEPOT),
            ItemKind::Mailbox => self.set_flag(TILESTATE_MAILBOX),
            ItemKind::TrashHolder => self.set_flag(TILESTATE_TRASHHOLDER),
            ItemKind::MagicField => self.set_flag(TILESTATE_MAGICFIELD),
            ItemKind::Teleport => self.set_flag(TILESTATE_TELEPORT),
            ItemKind::Bed => self.set_flag(TILESTATE_BED),
            _ => {}
        }

        if item_type.supports_hangable {
            self.set_flag(TILESTATE_SUPPORTS_HANGABLE);
        }
    }
}
