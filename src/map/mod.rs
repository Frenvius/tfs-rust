use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use thiserror::Error;

use crate::creatures::CreatureId;

pub mod houses;
pub mod otbm;
pub mod serialize;
pub mod tile;

pub use houses::{House, HouseLoadError, Houses};
pub use tile::{
    MapItem, Tile, TileKind, TILESTATE_FLOORCHANGE, TILESTATE_FLOORCHANGE_DOWN,
    TILESTATE_FLOORCHANGE_EAST, TILESTATE_FLOORCHANGE_EAST_ALT, TILESTATE_FLOORCHANGE_NORTH,
    TILESTATE_FLOORCHANGE_SOUTH, TILESTATE_FLOORCHANGE_SOUTH_ALT, TILESTATE_FLOORCHANGE_WEST,
    TILESTATE_PROTECTIONZONE,
};

const FLOOR_BITS: u32 = 3;
const FLOOR_SIZE: usize = 1 << FLOOR_BITS;
const FLOOR_MASK: u16 = (FLOOR_SIZE as u16) - 1;
const MAP_MAX_LAYERS: usize = 16;

pub const MAX_VIEWPORT_X: i32 = 8;
pub const MAX_VIEWPORT_Y: i32 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Position {
    pub x: u16,
    pub y: u16,
    pub z: u8,
}

impl Position {
    /// Apply one step in `dir` (clamping at map boundaries rather than wrapping).
    pub fn offset_direction(self, dir: crate::creatures::Direction) -> Position {
        use crate::creatures::Direction;
        let (dx, dy): (i32, i32) = match dir {
            Direction::North     => (0, -1),
            Direction::South     => (0,  1),
            Direction::East      => (1,  0),
            Direction::West      => (-1, 0),
            Direction::NorthEast => (1, -1),
            Direction::SouthEast => (1,  1),
            Direction::NorthWest => (-1, -1),
            Direction::SouthWest => (-1,  1),
        };
        Position {
            x: (self.x as i32 + dx).clamp(0, u16::MAX as i32) as u16,
            y: (self.y as i32 + dy).clamp(0, u16::MAX as i32) as u16,
            z: self.z,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Town {
    pub id: u32,
    pub name: String,
    pub temple_pos: Position,
}

pub const MAX_VIEWPORT_X_SPECTATOR: i32 = 11;
pub const MAX_VIEWPORT_Y_SPECTATOR: i32 = 11;

#[derive(Default)]
pub struct Map {
    pub width: u16,
    pub height: u16,
    pub spawn_file: Option<String>,
    pub house_file: Option<String>,
    pub towns: BTreeMap<u32, Town>,
    pub waypoints: BTreeMap<String, Position>,
    pub houses: Houses,
    pub description: String,
    root: QTreeNode,
    spectator_cache: HashMap<Position, Vec<CreatureId>>,
    players_spectator_cache: HashMap<Position, Vec<CreatureId>>,
}


impl Map {
    pub fn load_from_path(
        path: impl AsRef<Path>,
        items: &crate::items::Items,
        load_houses: bool,
    ) -> Result<Self, MapLoadError> {
        let mut map = otbm::load_from_path(path, items)?;
        if load_houses {
            if let Some(house_file) = map.house_file.clone() {
                map.houses.load_from_xml(house_file)?;
            }
        }
        Ok(map)
    }

    pub fn set_tile(&mut self, position: Position, tile: Tile) {
        if usize::from(position.z) >= MAP_MAX_LAYERS {
            return;
        }

        let leaf = self
            .root
            .create_leaf(u32::from(position.x), u32::from(position.y), 15);
        let floor = leaf.create_floor(usize::from(position.z));
        floor.set_tile(position.x, position.y, tile);
    }

    pub fn get_tile(&self, position: Position) -> Option<&Tile> {
        if usize::from(position.z) >= MAP_MAX_LAYERS {
            return None;
        }

        let leaf = self
            .root
            .get_leaf(u32::from(position.x), u32::from(position.y))?;
        let floor = leaf.get_floor(usize::from(position.z))?;
        floor.get_tile(position.x, position.y)
    }

    pub fn remove_tile(&mut self, position: Position) -> Option<Tile> {
        if usize::from(position.z) >= MAP_MAX_LAYERS {
            return None;
        }

        let leaf = self
            .root
            .get_leaf_mut(u32::from(position.x), u32::from(position.y))?;
        let floor = leaf.get_floor_mut(usize::from(position.z))?;
        floor.remove_tile(position.x, position.y)
    }

    pub fn resolve_floor_change_destination(&self, position: Position) -> Option<Position> {
        let tile = self.get_tile(position)?;

        if tile.has_flag(TILESTATE_FLOORCHANGE_DOWN) {
            let mut x = position.x;
            let mut y = position.y;
            let z = position.z.wrapping_add(1);

            let south_down = self.get_tile(Position {
                x,
                y: y.wrapping_sub(1),
                z,
            });
            if south_down
                .map(|tile| tile.has_flag(TILESTATE_FLOORCHANGE_SOUTH_ALT))
                .unwrap_or(false)
            {
                y = y.wrapping_sub(2);
                return self.get_tile(Position { x, y, z }).map(|tile| tile.position);
            }

            let east_down = self.get_tile(Position {
                x: x.wrapping_sub(1),
                y,
                z,
            });
            if east_down
                .map(|tile| tile.has_flag(TILESTATE_FLOORCHANGE_EAST_ALT))
                .unwrap_or(false)
            {
                x = x.wrapping_sub(2);
                return self.get_tile(Position { x, y, z }).map(|tile| tile.position);
            }

            let down = self.get_tile(Position { x, y, z })?;
            if down.has_flag(TILESTATE_FLOORCHANGE_NORTH) {
                y = y.wrapping_add(1);
            }
            if down.has_flag(TILESTATE_FLOORCHANGE_SOUTH) {
                y = y.wrapping_sub(1);
            }
            if down.has_flag(TILESTATE_FLOORCHANGE_SOUTH_ALT) {
                y = y.wrapping_sub(2);
            }
            if down.has_flag(TILESTATE_FLOORCHANGE_EAST) {
                x = x.wrapping_sub(1);
            }
            if down.has_flag(TILESTATE_FLOORCHANGE_EAST_ALT) {
                x = x.wrapping_sub(2);
            }
            if down.has_flag(TILESTATE_FLOORCHANGE_WEST) {
                x = x.wrapping_add(1);
            }

            return self.get_tile(Position { x, y, z }).map(|tile| tile.position);
        }

        if tile.has_flag(TILESTATE_FLOORCHANGE) {
            let mut x = position.x;
            let mut y = position.y;
            let z = position.z.wrapping_sub(1);

            if tile.has_flag(TILESTATE_FLOORCHANGE_NORTH) {
                y = y.wrapping_sub(1);
            }
            if tile.has_flag(TILESTATE_FLOORCHANGE_SOUTH) {
                y = y.wrapping_add(1);
            }
            if tile.has_flag(TILESTATE_FLOORCHANGE_EAST) {
                x = x.wrapping_add(1);
            }
            if tile.has_flag(TILESTATE_FLOORCHANGE_WEST) {
                x = x.wrapping_sub(1);
            }
            if tile.has_flag(TILESTATE_FLOORCHANGE_SOUTH_ALT) {
                y = y.wrapping_add(2);
            }
            if tile.has_flag(TILESTATE_FLOORCHANGE_EAST_ALT) {
                x = x.wrapping_add(2);
            }

            return self.get_tile(Position { x, y, z }).map(|tile| tile.position);
        }

        Some(position)
    }

    pub fn get_tile_mut(&mut self, position: Position) -> Option<&mut Tile> {
        if usize::from(position.z) >= MAP_MAX_LAYERS {
            return None;
        }
        let leaf = self
            .root
            .get_leaf_mut(u32::from(position.x), u32::from(position.y))?;
        let floor = leaf.get_floor_mut(usize::from(position.z))?;
        floor.get_tile_mut(position.x, position.y)
    }

    pub fn add_creature_to_tile(&mut self, position: Position, creature_id: CreatureId, is_player: bool) {
        if let Some(tile) = self.get_tile_mut(position) {
            tile.add_creature(creature_id);
        }
        if let Some(leaf) = self.root.get_leaf_mut(u32::from(position.x), u32::from(position.y)) {
            leaf.add_creature(creature_id, is_player);
        }
        self.clear_spectator_cache();
        if is_player {
            self.clear_players_spectator_cache();
        }
    }

    pub fn remove_creature_from_tile(&mut self, position: Position, creature_id: CreatureId, is_player: bool) {
        if let Some(tile) = self.get_tile_mut(position) {
            tile.remove_creature(creature_id);
        }
        if let Some(leaf) = self.root.get_leaf_mut(u32::from(position.x), u32::from(position.y)) {
            leaf.remove_creature(creature_id, is_player);
        }
        self.clear_spectator_cache();
        if is_player {
            self.clear_players_spectator_cache();
        }
    }

    pub fn move_creature_on_map(&mut self, old_pos: Position, new_pos: Position, creature_id: CreatureId, is_player: bool) {
        if let Some(tile) = self.get_tile_mut(old_pos) {
            tile.remove_creature(creature_id);
        }
        if let Some(tile) = self.get_tile_mut(new_pos) {
            tile.add_creature(creature_id);
        }

        let old_leaf_ptr = self.root.get_leaf_mut(u32::from(old_pos.x), u32::from(old_pos.y))
            .map(|l| l as *mut QTreeLeafNode);
        let new_leaf_ptr = self.root.get_leaf_mut(u32::from(new_pos.x), u32::from(new_pos.y))
            .map(|l| l as *mut QTreeLeafNode);

        if old_leaf_ptr != new_leaf_ptr {
            if let Some(ptr) = old_leaf_ptr {
                unsafe { &mut *ptr }.remove_creature(creature_id, is_player);
            }
            if let Some(ptr) = new_leaf_ptr {
                unsafe { &mut *ptr }.add_creature(creature_id, is_player);
            }
        }

        self.clear_spectator_cache();
        if is_player {
            self.clear_players_spectator_cache();
        }
    }

    pub fn clear_spectator_cache(&mut self) {
        self.spectator_cache.clear();
    }

    pub fn clear_players_spectator_cache(&mut self) {
        self.players_spectator_cache.clear();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn get_spectators(
        &mut self,
        center_pos: Position,
        multifloor: bool,
        only_players: bool,
        min_range_x: i32,
        max_range_x: i32,
        min_range_y: i32,
        max_range_y: i32,
    ) -> Vec<CreatureId> {
        if usize::from(center_pos.z) >= MAP_MAX_LAYERS {
            return Vec::new();
        }

        let min_rx = if min_range_x == 0 { -MAX_VIEWPORT_X_SPECTATOR } else { -min_range_x };
        let max_rx = if max_range_x == 0 { MAX_VIEWPORT_X_SPECTATOR } else { max_range_x };
        let min_ry = if min_range_y == 0 { -MAX_VIEWPORT_Y_SPECTATOR } else { -min_range_y };
        let max_ry = if max_range_y == 0 { MAX_VIEWPORT_Y_SPECTATOR } else { max_range_y };

        let is_default_range = min_rx == -MAX_VIEWPORT_X_SPECTATOR
            && max_rx == MAX_VIEWPORT_X_SPECTATOR
            && min_ry == -MAX_VIEWPORT_Y_SPECTATOR
            && max_ry == MAX_VIEWPORT_Y_SPECTATOR
            && multifloor;

        if is_default_range {
            if only_players {
                if let Some(cached) = self.players_spectator_cache.get(&center_pos) {
                    return cached.clone();
                }
            }
            if let Some(cached) = self.spectator_cache.get(&center_pos) {
                if !only_players {
                    return cached.clone();
                }
                return cached.to_vec();
            }
        }

        let (min_range_z, max_range_z) = if multifloor {
            let z = center_pos.z as i32;
            if z > 7 {
                (z.saturating_sub(2).max(0), (z + 2).min(MAP_MAX_LAYERS as i32 - 1))
            } else if z == 6 {
                (0, 8)
            } else if z == 7 {
                (0, 9)
            } else {
                (0, 7)
            }
        } else {
            (center_pos.z as i32, center_pos.z as i32)
        };

        let result = self.get_spectators_internal(
            center_pos, min_rx, max_rx, min_ry, max_ry,
            min_range_z, max_range_z, only_players,
        );

        if is_default_range {
            if only_players {
                self.players_spectator_cache.insert(center_pos, result.clone());
            } else {
                self.spectator_cache.insert(center_pos, result.clone());
            }
        }

        result
    }

    #[allow(clippy::too_many_arguments)]
    fn get_spectators_internal(
        &self,
        center_pos: Position,
        min_range_x: i32,
        max_range_x: i32,
        min_range_y: i32,
        max_range_y: i32,
        min_range_z: i32,
        max_range_z: i32,
        only_players: bool,
    ) -> Vec<CreatureId> {
        let mut spectators = Vec::with_capacity(32);

        let min_y = center_pos.y as i32 + min_range_y;
        let min_x = center_pos.x as i32 + min_range_x;
        let max_y = center_pos.y as i32 + max_range_y;
        let max_x = center_pos.x as i32 + max_range_x;

        let min_offset = center_pos.z as i32 - max_range_z;
        let x1 = (min_x + min_offset).clamp(0, 0xFFFF) as u16;
        let y1 = (min_y + min_offset).clamp(0, 0xFFFF) as u16;

        let max_offset = center_pos.z as i32 - min_range_z;
        let x2 = (max_x + max_offset).clamp(0, 0xFFFF) as u16;
        let y2 = (max_y + max_offset).clamp(0, 0xFFFF) as u16;

        let floor_size = FLOOR_SIZE as u16;
        let startx1 = x1 - (x1 % floor_size);
        let starty1 = y1 - (y1 % floor_size);
        let endx2 = x2 - (x2 % floor_size);
        let endy2 = y2 - (y2 % floor_size);

        // Walk the quadtree leaf grid
        let mut cur_y = starty1;
        while cur_y <= endy2 {
            let mut cur_x = startx1;
            while cur_x <= endx2 {
                if let Some(leaf) = self.root.get_leaf(u32::from(cur_x), u32::from(cur_y)) {
                    let node_list = if only_players { &leaf.player_list } else { &leaf.creature_list };
                    for &creature_id in node_list {
                        spectators.push(creature_id);
                    }
                }
                cur_x = cur_x.saturating_add(floor_size);
            }
            cur_y = cur_y.saturating_add(floor_size);
        }

        spectators
    }

    pub fn move_upstairs_position(&self, position: Position) -> Option<Position> {
        let upper_z = position.z.checked_sub(1)?;
        let default = Position {
            x: position.x,
            y: position.y.wrapping_add(1),
            z: upper_z,
        };

        if self
            .get_tile(default)
            .map(Tile::is_walkable)
            .unwrap_or(false)
        {
            return Some(default);
        }

        for (dx, dy) in &[
            (0i16, -1i16),
            (1, 0),
            (-1, 0),
            (-1, 1),
            (1, 1),
            (-1, -1),
            (1, -1),
        ] {
            let candidate = Position {
                x: position.x.wrapping_add_signed(*dx),
                y: position.y.wrapping_add_signed(*dy),
                z: upper_z,
            };
            if self
                .get_tile(candidate)
                .map(Tile::is_walkable)
                .unwrap_or(false)
            {
                return Some(candidate);
            }
        }

        Some(default)
    }

    pub fn get_town_temple_pos(&self, town_id: u32) -> Option<Position> {
        self.towns.get(&town_id).map(|t| t.temple_pos)
    }

    // ── Line of Sight (port of map.cpp isSightClear / checkSightLine / isTileClear / canThrowObjectTo) ──

    pub fn is_tile_clear(&self, x: u16, y: u16, z: u8, block_floor: bool, items: &crate::items::Items) -> bool {
        let tile = self.get_tile(Position { x, y, z });
        let Some(tile) = tile else { return true };
        if block_floor && tile.ground.is_some() {
            return false;
        }
        !tile.has_property_block_projectile(items)
    }

    pub fn check_sight_line(&self, x0: u16, y0: u16, x1: u16, y1: u16, z: u8, items: &crate::items::Items) -> bool {
        if x0 == x1 && y0 == y1 {
            return true;
        }

        let dy = (y1 as i32) - (y0 as i32);
        let dx = (x1 as i32) - (x0 as i32);

        if dy.abs() > dx.abs() {
            if y1 > y0 {
                return self.check_steep_line(y0, x0, y1, x1, z, items);
            }
            return self.check_steep_line(y1, x1, y0, x0, z, items);
        }

        if x0 > x1 {
            return self.check_slight_line(x1, y1, x0, y0, z, items);
        }

        self.check_slight_line(x0, y0, x1, y1, z, items)
    }

    fn check_steep_line(&self, x0: u16, y0: u16, x1: u16, y1: u16, z: u8, items: &crate::items::Items) -> bool {
        let dx = (x1 as f32) - (x0 as f32);
        let slope = if dx == 0.0 { 1.0 } else { ((y1 as f32) - (y0 as f32)) / dx };
        let mut yi = (y0 as f32) + slope;

        let mut x = x0 + 1;
        while x < x1 {
            if !self.is_tile_clear((yi + 0.1).floor() as u16, x, z, false, items) {
                return false;
            }
            yi += slope;
            x += 1;
        }
        true
    }

    fn check_slight_line(&self, x0: u16, y0: u16, x1: u16, y1: u16, z: u8, items: &crate::items::Items) -> bool {
        let dx = (x1 as f32) - (x0 as f32);
        let slope = if dx == 0.0 { 1.0 } else { ((y1 as f32) - (y0 as f32)) / dx };
        let mut yi = (y0 as f32) + slope;

        let mut x = x0 + 1;
        while x < x1 {
            if !self.is_tile_clear(x, (yi + 0.1).floor() as u16, z, false, items) {
                return false;
            }
            yi += slope;
            x += 1;
        }
        true
    }

    pub fn is_sight_clear(&self, from_pos: Position, to_pos: Position, same_floor: bool, items: &crate::items::Items) -> bool {
        if from_pos.z == to_pos.z {
            let dist_x = (from_pos.x as i32 - to_pos.x as i32).unsigned_abs();
            let dist_y = (from_pos.y as i32 - to_pos.y as i32).unsigned_abs();
            if dist_x < 2 && dist_y < 2 {
                return true;
            }

            let sight_clear = self.check_sight_line(from_pos.x, from_pos.y, to_pos.x, to_pos.y, from_pos.z, items);
            if sight_clear || same_floor {
                return sight_clear;
            }

            if from_pos.z == 0 {
                return true;
            }

            let new_z = from_pos.z - 1;
            return self.is_tile_clear(from_pos.x, from_pos.y, new_z, true, items)
                && self.is_tile_clear(to_pos.x, to_pos.y, new_z, true, items)
                && self.check_sight_line(from_pos.x, from_pos.y, to_pos.x, to_pos.y, new_z, items);
        }

        if same_floor {
            return false;
        }

        if (from_pos.z < 8 && to_pos.z > 7) || (from_pos.z > 7 && to_pos.z < 8) {
            return false;
        }

        if from_pos.z > to_pos.z {
            let dist_z = (from_pos.z as i32 - to_pos.z as i32).unsigned_abs();
            if dist_z > 1 {
                return false;
            }

            let new_z = from_pos.z - 1;
            return self.is_tile_clear(from_pos.x, from_pos.y, new_z, true, items)
                && self.check_sight_line(from_pos.x, from_pos.y, to_pos.x, to_pos.y, new_z, items);
        }

        for z in from_pos.z..to_pos.z {
            if !self.is_tile_clear(to_pos.x, to_pos.y, z, true, items) {
                return false;
            }
        }

        self.check_sight_line(from_pos.x, from_pos.y, to_pos.x, to_pos.y, from_pos.z, items)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn can_throw_object_to(
        &self,
        from_pos: Position,
        to_pos: Position,
        check_line_of_sight: bool,
        same_floor: bool,
        range_x: i32,
        range_y: i32,
        items: &crate::items::Items,
    ) -> bool {
        let dist_x = (from_pos.x as i32 - to_pos.x as i32).abs();
        let dist_y = (from_pos.y as i32 - to_pos.y as i32).abs();
        if dist_x > range_x || dist_y > range_y {
            return false;
        }

        if !check_line_of_sight {
            return true;
        }

        self.is_sight_clear(from_pos, to_pos, same_floor, items)
    }
}

#[derive(Debug, Error)]
pub enum MapLoadError {
    #[error(transparent)]
    Otbm(#[from] otbm::OtbmError),
    #[error(transparent)]
    Houses(#[from] HouseLoadError),
}

#[derive(Debug)]
struct Floor {
    tiles: [Option<Tile>; FLOOR_SIZE * FLOOR_SIZE],
}

impl Default for Floor {
    fn default() -> Self {
        Self {
            tiles: std::array::from_fn(|_| None),
        }
    }
}

impl Floor {
    fn get_tile(&self, x: u16, y: u16) -> Option<&Tile> {
        self.tiles[Self::index(x, y)].as_ref()
    }

    fn get_tile_mut(&mut self, x: u16, y: u16) -> Option<&mut Tile> {
        self.tiles[Self::index(x, y)].as_mut()
    }

    fn set_tile(&mut self, x: u16, y: u16, tile: Tile) {
        self.tiles[Self::index(x, y)] = Some(tile);
    }

    fn remove_tile(&mut self, x: u16, y: u16) -> Option<Tile> {
        self.tiles[Self::index(x, y)].take()
    }

    fn index(x: u16, y: u16) -> usize {
        usize::from(x & FLOOR_MASK) * FLOOR_SIZE + usize::from(y & FLOOR_MASK)
    }
}

use crate::creatures::Direction;

const MAP_NORMALWALKCOST: u16 = 10;
const MAP_DIAGONALWALKCOST: u16 = 25;
const MAX_PATHFIND_NODES: usize = 512;

#[allow(clippy::too_many_arguments)]
pub fn find_path(
    map: &Map,
    start: Position,
    target: Position,
    min_target_dist: i32,
    max_target_dist: i32,
    full_path_search: bool,
    _clear_sight: bool,
    max_search_dist: i32,
) -> Option<Vec<Direction>> {
    if start.z != target.z { return None; }

    let dx = (start.x as i32 - target.x as i32).unsigned_abs();
    let dy = (start.y as i32 - target.y as i32).unsigned_abs();
    if max_target_dist <= 1 && dx <= 1 && dy <= 1 {
        return Some(vec![]);
    }

    let max_dx = if max_search_dist != 0 { max_search_dist } else { MAX_VIEWPORT_X + 1 };
    let max_dy = if max_search_dist != 0 { max_search_dist } else { MAX_VIEWPORT_Y + 1 };
    if dx as i32 > max_dx || dy as i32 > max_dy { return None; }

    use std::collections::{BinaryHeap, HashMap};
    use std::cmp::Reverse;

    #[derive(Clone)]
    struct Node { g: u16, f: u16, parent: Option<(u16, u16)> }

    let mut open: BinaryHeap<Reverse<(u16, u16, u16)>> = BinaryHeap::new();
    let mut best: HashMap<(u16, u16), Node> = HashMap::new();
    let mut closed: std::collections::HashSet<(u16, u16)> = std::collections::HashSet::new();

    let heuristic = |x: u16, y: u16| -> u16 {
        let dx = (x as i32 - target.x as i32).unsigned_abs() as u16;
        let dy = (y as i32 - target.y as i32).unsigned_abs() as u16;
        dx.max(dy) * MAP_NORMALWALKCOST
    };

    let h0 = heuristic(start.x, start.y);
    open.push(Reverse((h0, start.x, start.y)));
    best.insert((start.x, start.y), Node { g: 0, f: h0, parent: None });

    let neighbors: [(i32, i32); 8] = [(-1, 0), (0, 1), (1, 0), (0, -1), (-1, -1), (1, -1), (1, 1), (-1, 1)];

    let mut found: Option<(u16, u16)> = None;
    let mut best_match: i32 = 0;
    let limit = if full_path_search { MAX_PATHFIND_NODES } else { (MAX_VIEWPORT_X as usize) * (MAX_VIEWPORT_Y as usize) };
    let mut iterations = 0usize;

    while let Some(Reverse((_, cx, cy))) = open.pop() {
        let key = (cx, cy);
        if closed.contains(&key) { continue; }
        closed.insert(key);
        iterations += 1;
        if iterations >= limit { break; }

        let pos = Position { x: cx, y: cy, z: start.z };
        let pdx = (pos.x as i32 - target.x as i32).abs();
        let pdy = (pos.y as i32 - target.y as i32).abs();
        let dist = pdx.max(pdy);
        if dist >= min_target_dist && dist <= max_target_dist {
            let m = max_target_dist - dist;
            if m > best_match || found.is_none() {
                best_match = m;
                found = Some(key);
                if best_match == 0 { break; }
            }
        }

        let cur_g = best[&key].g;
        for &(ndx, ndy) in &neighbors {
            let nx = cx as i32 + ndx;
            let ny = cy as i32 + ndy;
            if nx < 0 || ny < 0 || nx > 0xFFFF || ny > 0xFFFF { continue; }
            let (nx, ny) = (nx as u16, ny as u16);
            let nkey = (nx, ny);
            if closed.contains(&nkey) { continue; }

            let npos = Position { x: nx, y: ny, z: start.z };
            let tile = match map.get_tile(npos) {
                Some(t) => t,
                None => continue,
            };
            if tile.ground.is_none() { continue; }
            if tile.has_flag(crate::map::tile::TILESTATE_BLOCKSOLID) && !tile.creature_ids.is_empty() {
                continue;
            }
            if tile.has_flag(crate::map::tile::TILESTATE_BLOCKSOLID) { continue; }

            let walk_cost = if ndx.abs() == ndy.abs() { MAP_DIAGONALWALKCOST } else { MAP_NORMALWALKCOST };
            let creature_cost = if !tile.creature_ids.is_empty() { MAP_NORMALWALKCOST * 3 } else { 0 };
            let ng = cur_g + walk_cost + creature_cost;
            let nh = heuristic(nx, ny);
            let nf = ng + nh;

            if let Some(existing) = best.get(&nkey) {
                if existing.f <= nf { continue; }
            }

            best.insert(nkey, Node { g: ng, f: nf, parent: Some(key) });
            open.push(Reverse((nf, nx, ny)));
        }
    }

    let end = found?;
    let mut dirs = Vec::new();
    let mut cur = end;
    while let Some(parent) = best.get(&cur).and_then(|n| n.parent) {
        let ddx = parent.0 as i32 - cur.0 as i32;
        let ddy = parent.1 as i32 - cur.1 as i32;
        let d = match (ddx, ddy) {
            (1, 1)   => Direction::NorthWest,
            (-1, 1)  => Direction::NorthEast,
            (1, -1)  => Direction::SouthWest,
            (-1, -1) => Direction::SouthEast,
            (1, 0)   => Direction::West,
            (-1, 0)  => Direction::East,
            (0, 1)   => Direction::North,
            (0, -1)  => Direction::South,
            _ => break,
        };
        dirs.push(d);
        cur = parent;
    }
    Some(dirs)
}

#[derive(Debug, Default)]
struct QTreeLeafNode {
    floors: [Option<Floor>; MAP_MAX_LAYERS],
    creature_list: Vec<CreatureId>,
    player_list: Vec<CreatureId>,
}

impl QTreeLeafNode {
    fn create_floor(&mut self, z: usize) -> &mut Floor {
        self.floors[z].get_or_insert_with(Floor::default)
    }

    fn get_floor(&self, z: usize) -> Option<&Floor> {
        self.floors[z].as_ref()
    }

    fn get_floor_mut(&mut self, z: usize) -> Option<&mut Floor> {
        self.floors[z].as_mut()
    }

    fn add_creature(&mut self, id: CreatureId, is_player: bool) {
        self.creature_list.push(id);
        if is_player {
            self.player_list.push(id);
        }
    }

    fn remove_creature(&mut self, id: CreatureId, is_player: bool) {
        if let Some(pos) = self.creature_list.iter().position(|&c| c == id) {
            self.creature_list.swap_remove(pos);
        }
        if is_player {
            if let Some(pos) = self.player_list.iter().position(|&c| c == id) {
                self.player_list.swap_remove(pos);
            }
        }
    }
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
enum QTreeNode {
    Branch {
        children: [Option<Box<QTreeNode>>; 4],
    },
    Leaf(QTreeLeafNode),
}

impl Default for QTreeNode {
    fn default() -> Self {
        Self::Branch {
            children: std::array::from_fn(|_| None),
        }
    }
}

impl QTreeNode {
    fn get_leaf(&self, x: u32, y: u32) -> Option<&QTreeLeafNode> {
        match self {
            Self::Leaf(leaf) => Some(leaf),
            Self::Branch { children } => {
                let index = child_index(x, y);
                children[index].as_deref()?.get_leaf(x << 1, y << 1)
            }
        }
    }

    fn get_leaf_mut(&mut self, x: u32, y: u32) -> Option<&mut QTreeLeafNode> {
        match self {
            Self::Leaf(leaf) => Some(leaf),
            Self::Branch { children } => {
                let index = child_index(x, y);
                children[index].as_deref_mut()?.get_leaf_mut(x << 1, y << 1)
            }
        }
    }

    fn create_leaf(&mut self, x: u32, y: u32, level: u32) -> &mut QTreeLeafNode {
        match self {
            Self::Leaf(leaf) => leaf,
            Self::Branch { children } => {
                let index = child_index(x, y);
                if children[index].is_none() {
                    children[index] = Some(if level != FLOOR_BITS {
                        Box::new(QTreeNode::default())
                    } else {
                        Box::new(QTreeNode::Leaf(QTreeLeafNode::default()))
                    });
                }

                children[index]
                    .as_deref_mut()
                    .expect("child was inserted")
                    .create_leaf(x << 1, y << 1, level.saturating_sub(1))
            }
        }
    }
}

fn child_index(x: u32, y: u32) -> usize {
    (((x & 0x8000) >> 15) | ((y & 0x8000) >> 14)) as usize
}
