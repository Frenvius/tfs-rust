use crate::combat::condition::{Condition, ConditionType};
use crate::combat::{
    BlockType, CallBackParam, CombatDamage, CombatOrigin, CombatParam, CombatType, FormulaType, ReturnValue,
};
use crate::creatures::Direction;
use crate::map::Position;

// ──────────────────────────────────────────────────────────────────────────────
// Effect / animation sentinel constants — matching definitions.h / enums.h
// ──────────────────────────────────────────────────────────────────────────────

pub const CONST_ME_NONE: u8 = 0;
pub const CONST_ANI_NONE: u8 = 0;
pub const CONST_ANI_WEAPONTYPE: u8 = 254;

// ──────────────────────────────────────────────────────────────────────────────
// MatrixArea
// ──────────────────────────────────────────────────────────────────────────────

/// Rectangular boolean area with a center cell.
///
/// Flat row-major layout: element (row, col) = arr[row * cols + col].
/// `center` stores (x, y) = (col, row), matching the C++ convention from
/// `setCenter(y, x)` → `center = make_pair(x, y)`.
#[derive(Default)]
pub struct MatrixArea {
    arr: Vec<bool>,
    center: (u32, u32),
    rows: u32,
    cols: u32,
}

impl MatrixArea {
    pub fn new(rows: u32, cols: u32) -> Self {
        Self {
            arr: vec![false; (rows * cols) as usize],
            center: (0, 0),
            rows,
            cols,
        }
    }

    fn with_parts(center: (u32, u32), rows: u32, cols: u32, arr: Vec<bool>) -> Self {
        Self { arr, center, rows, cols }
    }

    pub fn get(&self, row: u32, col: u32) -> bool {
        self.arr[(row * self.cols + col) as usize]
    }

    pub fn set(&mut self, row: u32, col: u32, val: bool) {
        self.arr[(row * self.cols + col) as usize] = val;
    }

    /// setCenter(y, x) in C++ → center = make_pair(x, y).
    pub fn set_center(&mut self, y: u32, x: u32) {
        self.center = (x, y);
    }

    /// Returns center as (x, y) = (col, row).
    pub fn get_center(&self) -> (u32, u32) {
        self.center
    }

    pub fn rows(&self) -> u32 {
        self.rows
    }

    pub fn cols(&self) -> u32 {
        self.cols
    }

    pub fn is_empty(&self) -> bool {
        self.rows == 0 || self.cols == 0
    }

    /// Vertical flip: row i of the new array ← old row (rows-i-1).
    ///
    /// C++ valarray: `newArr[slice(i*cols, cols, 1)] = arr[slice((rows-i-1)*cols, cols, 1)]`
    /// New center: `{cols - center.first - 1, center.second}` (i.e., x flips, y unchanged).
    pub fn flip(&self) -> MatrixArea {
        let n = (self.rows * self.cols) as usize;
        let mut new_arr = vec![false; n];
        let cols = self.cols as usize;
        let rows = self.rows as usize;

        for i in 0..rows {
            let src = (rows - i - 1) * cols;
            let dst = i * cols;
            new_arr[dst..dst + cols].copy_from_slice(&self.arr[src..src + cols]);
        }

        let (cx, cy) = self.center;
        MatrixArea::with_parts((self.cols - cx - 1, cy), self.rows, self.cols, new_arr)
    }

    /// Stride-rows column reversal.
    ///
    /// C++ valarray: for i in 0..cols:
    ///   `newArr[slice(i, cols, rows)] = arr[slice(cols-i-1, cols, rows)]`
    /// New center: `{center.first, rows - center.second - 1}` (x unchanged, y flips).
    pub fn mirror(&self) -> MatrixArea {
        let n = (self.rows * self.cols) as usize;
        let mut new_arr = vec![false; n];
        let cols = self.cols as usize;
        let rows = self.rows as usize;

        for i in 0..cols {
            let src_i = cols - i - 1;
            for k in 0..cols {
                let dst_pos = i + k * rows;
                let src_pos = src_i + k * rows;
                if dst_pos < n && src_pos < n {
                    new_arr[dst_pos] = self.arr[src_pos];
                }
            }
        }

        let (cx, cy) = self.center;
        MatrixArea::with_parts((cx, self.rows - cy - 1), self.rows, self.cols, new_arr)
    }

    /// Transpose: reads arr in col-major order via gslice(0, {cols, rows}, {1, cols}).
    ///
    /// C++ valarray: `arr[gslice(0, {cols, rows}, {1, cols})]`
    /// Outer loop size=cols step=1; inner loop size=rows step=cols.
    /// New center: `{center.second, center.first}` (swap x and y).
    /// Dimensions unchanged (still rows×cols).
    pub fn transpose(&self) -> MatrixArea {
        let n = (self.rows * self.cols) as usize;
        let mut new_arr = vec![false; n];
        let cols = self.cols as usize;
        let rows = self.rows as usize;

        let mut dst = 0;
        for outer in 0..cols {
            for inner in 0..rows {
                let src = outer + inner * cols;
                if src < n && dst < n {
                    new_arr[dst] = self.arr[src];
                }
                dst += 1;
            }
        }

        let (cx, cy) = self.center;
        MatrixArea::with_parts((cy, cx), self.rows, self.cols, new_arr)
    }

    /// 90° clockwise rotation. Output dimensions: (cols, rows).
    ///
    /// C++ valarray: for i in 0..rows:
    ///   `newArr[slice(i, cols, rows)] = arr[slice((rows-i-1)*cols, cols, 1)]`
    /// New center: `{rows - center.second - 1, center.first}`.
    pub fn rotate90(&self) -> MatrixArea {
        let n = (self.rows * self.cols) as usize;
        let mut new_arr = vec![false; n];
        let cols = self.cols as usize;
        let rows = self.rows as usize;

        for i in 0..rows {
            let src_row = rows - i - 1;
            for k in 0..cols {
                let dst_pos = i + k * rows;
                let src_pos = src_row * cols + k;
                if dst_pos < n && src_pos < n {
                    new_arr[dst_pos] = self.arr[src_pos];
                }
            }
        }

        let (cx, cy) = self.center;
        MatrixArea::with_parts((self.rows - cy - 1, cx), self.cols, self.rows, new_arr)
    }

    /// 180° rotation: reverse all elements. Dimensions unchanged.
    ///
    /// C++ valarray: `std::reverse_copy(begin, end, begin(newArr))`
    /// New center: `{cols - center.first - 1, rows - center.second - 1}`.
    pub fn rotate180(&self) -> MatrixArea {
        let mut new_arr = self.arr.clone();
        new_arr.reverse();

        let (cx, cy) = self.center;
        MatrixArea::with_parts(
            (self.cols - cx - 1, self.rows - cy - 1),
            self.rows,
            self.cols,
            new_arr,
        )
    }

    /// 270° clockwise rotation (= 90° counter-clockwise). Output dimensions: (cols, rows).
    ///
    /// C++ valarray: for i in 0..cols:
    ///   `newArr[slice(i*rows, rows, 1)] = arr[slice(cols-i-1, rows, cols)]`
    /// New center: `{center.second, cols - center.first - 1}`.
    pub fn rotate270(&self) -> MatrixArea {
        let n = (self.rows * self.cols) as usize;
        let mut new_arr = vec![false; n];
        let cols = self.cols as usize;
        let rows = self.rows as usize;

        for i in 0..cols {
            let src_col = cols - i - 1;
            for k in 0..rows {
                let dst_pos = i * rows + k;
                let src_pos = src_col + k * cols;
                if dst_pos < n && src_pos < n {
                    new_arr[dst_pos] = self.arr[src_pos];
                }
            }
        }

        let (cx, cy) = self.center;
        MatrixArea::with_parts((cy, self.cols - cx - 1), self.cols, self.rows, new_arr)
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Area creation helper (file-private, mirrors C++ anonymous namespace)
// ──────────────────────────────────────────────────────────────────────────────

fn create_area(vec: &[u32], rows: u32) -> MatrixArea {
    let cols = if rows == 0 { 0 } else { vec.len() as u32 / rows };
    let mut area = MatrixArea::new(rows, cols);

    let mut x: u32 = 0;
    let mut y: u32 = 0;

    for &value in vec {
        if value == 1 || value == 3 {
            area.set(y, x, true);
        }
        if value == 2 || value == 3 {
            area.set_center(y, x);
        }
        x += 1;
        if cols == x {
            x = 0;
            y += 1;
        }
    }

    area
}

// ──────────────────────────────────────────────────────────────────────────────
// AreaCombat
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct AreaCombat {
    areas: Vec<MatrixArea>,
    has_ext_area: bool,
}

impl AreaCombat {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build 4 cardinal-direction areas from a flat vector.
    ///
    /// Matches C++ AreaCombat::setupArea(const vector<uint32_t>&, uint32_t):
    ///   EAST=rotate90, SOUTH=rotate180, WEST=rotate270, NORTH=original.
    pub fn setup_area(&mut self, vec: &[u32], rows: u32) {
        let area = create_area(vec, rows);

        if self.areas.is_empty() {
            self.areas.resize_with(4, MatrixArea::default);
        }

        self.areas[Direction::East as usize] = area.rotate90();
        self.areas[Direction::South as usize] = area.rotate180();
        self.areas[Direction::West as usize] = area.rotate270();
        self.areas[Direction::North as usize] = area;
    }

    /// Linear/cone area from length and spread.
    ///
    /// Matches C++ AreaCombat::setupArea(int32_t length, int32_t spread) verbatim.
    pub fn setup_area_length_spread(&mut self, length: i32, spread: i32) {
        let rows = length as u32;
        let cols: i32 = if spread != 0 {
            ((length - (length % spread)) / spread) * 2 + 1
        } else {
            1
        };

        let mut col_spread = cols;

        let mut vec: Vec<u32> = Vec::with_capacity((rows as usize) * (cols as usize));

        for y in 1..=rows as i32 {
            let mincol = cols - col_spread + 1;
            let maxcol = cols - (cols - col_spread);

            for x in 1..=cols {
                if y == rows as i32 && x == ((cols - (cols % 2)) / 2) + 1 {
                    vec.push(3);
                } else if x >= mincol && x <= maxcol {
                    vec.push(1);
                } else {
                    vec.push(0);
                }
            }

            if spread > 0 && y % spread == 0 {
                col_spread -= 1;
            }
        }

        self.setup_area(&vec, rows);
    }

    /// Circular area using the 13×13 distance-ring lookup table.
    ///
    /// Matches C++ AreaCombat::setupArea(int32_t radius) verbatim.
    /// Cell value 1 → center (value 3 in vec); value ≤ radius → included (value 1 in vec).
    pub fn setup_area_radius(&mut self, radius: i32) {
        #[rustfmt::skip]
        let area: [[i32; 13]; 13] = [
            [0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 8, 8, 7, 8, 8, 0, 0, 0, 0],
            [0, 0, 0, 8, 7, 6, 6, 6, 7, 8, 0, 0, 0],
            [0, 0, 8, 7, 6, 5, 5, 5, 6, 7, 8, 0, 0],
            [0, 8, 7, 6, 5, 4, 4, 4, 5, 6, 7, 8, 0],
            [0, 8, 6, 5, 4, 3, 2, 3, 4, 5, 6, 8, 0],
            [8, 7, 6, 5, 4, 2, 1, 2, 4, 5, 6, 7, 8],
            [0, 8, 6, 5, 4, 3, 2, 3, 4, 5, 6, 8, 0],
            [0, 8, 7, 6, 5, 4, 4, 4, 5, 6, 7, 8, 0],
            [0, 0, 8, 7, 6, 5, 5, 5, 6, 7, 8, 0, 0],
            [0, 0, 0, 8, 7, 6, 6, 6, 7, 8, 0, 0, 0],
            [0, 0, 0, 0, 8, 8, 7, 8, 8, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0],
        ];

        let mut vec: Vec<u32> = Vec::with_capacity(13 * 13);
        for row in &area {
            for &cell in row {
                if cell == 1 {
                    vec.push(3);
                } else if cell > 0 && cell <= radius {
                    vec.push(1);
                } else {
                    vec.push(0);
                }
            }
        }

        self.setup_area(&vec, 13);
    }

    /// Ring area using the 13×13 ring-index lookup table.
    ///
    /// Matches C++ AreaCombat::setupAreaRing(int32_t ring) verbatim.
    /// Cell value 1 → center; cells where value == ring → included.
    pub fn setup_area_ring(&mut self, ring: i32) {
        #[rustfmt::skip]
        let area: [[i32; 13]; 13] = [
            [0, 0, 0, 0, 0, 7, 7, 7, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 7, 6, 6, 6, 7, 0, 0, 0, 0],
            [0, 0, 0, 7, 6, 5, 5, 5, 6, 7, 0, 0, 0],
            [0, 0, 7, 6, 5, 4, 4, 4, 5, 6, 7, 0, 0],
            [0, 7, 6, 5, 4, 3, 3, 3, 4, 5, 6, 7, 0],
            [7, 6, 5, 4, 3, 2, 0, 2, 3, 4, 5, 6, 7],
            [7, 6, 5, 4, 3, 0, 1, 0, 3, 4, 5, 6, 7],
            [7, 6, 5, 4, 3, 2, 0, 2, 3, 4, 5, 6, 7],
            [0, 7, 6, 5, 4, 3, 3, 3, 4, 5, 6, 7, 0],
            [0, 0, 7, 6, 5, 4, 4, 4, 5, 6, 7, 0, 0],
            [0, 0, 0, 7, 6, 5, 5, 5, 6, 7, 0, 0, 0],
            [0, 0, 0, 0, 7, 6, 6, 6, 7, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 7, 7, 7, 0, 0, 0, 0, 0],
        ];

        let mut vec: Vec<u32> = Vec::with_capacity(13 * 13);
        for row in &area {
            for &cell in row {
                if cell == 1 {
                    vec.push(3);
                } else if cell > 0 && cell == ring {
                    vec.push(1);
                } else {
                    vec.push(0);
                }
            }
        }

        self.setup_area(&vec, 13);
    }

    /// Set diagonal (NW/NE/SW/SE) areas from a flat vector.
    ///
    /// Matches C++ AreaCombat::setupExtArea(vec, rows):
    ///   NW=original, NE=mirror, SW=flip, SE=transpose.
    pub fn setup_ext_area(&mut self, vec: &[u32], rows: u32) {
        if vec.is_empty() {
            return;
        }

        self.has_ext_area = true;
        let area = create_area(vec, rows);
        self.areas.resize_with(8, MatrixArea::default);

        self.areas[Direction::NorthEast as usize] = area.mirror();
        self.areas[Direction::SouthWest as usize] = area.flip();
        self.areas[Direction::SouthEast as usize] = area.transpose();
        self.areas[Direction::NorthWest as usize] = area;
    }

    /// Select the correct area matrix for the given caster→target vector.
    ///
    /// Matches C++ AreaCombat::getArea() exactly, including diagonal fallback
    /// when has_ext_area is set.
    pub fn get_area(&self, center_pos: Position, target_pos: Position) -> &MatrixArea {
        let dx = target_pos.x as i32 - center_pos.x as i32;
        let dy = target_pos.y as i32 - center_pos.y as i32;

        let mut dir = if dx < 0 {
            Direction::West
        } else if dx > 0 {
            Direction::East
        } else if dy < 0 {
            Direction::North
        } else {
            Direction::South
        };

        if self.has_ext_area {
            if dx < 0 && dy < 0 {
                dir = Direction::NorthWest;
            } else if dx > 0 && dy < 0 {
                dir = Direction::NorthEast;
            } else if dx < 0 && dy > 0 {
                dir = Direction::SouthWest;
            } else if dx > 0 && dy > 0 {
                dir = Direction::SouthEast;
            }
        }

        let idx = dir as usize;
        if idx >= self.areas.len() {
            static EMPTY: MatrixArea = MatrixArea {
                arr: Vec::new(),
                center: (0, 0),
                rows: 0,
                cols: 0,
            };
            return &EMPTY;
        }

        &self.areas[idx]
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Lua callbacks
// ──────────────────────────────────────────────────────────────────────────────

/// Corresponds to C++ `class ValueCallback : public CallBack`.
pub struct ValueCallback {
    pub formula_type: FormulaType,
    pub script_id: i32,
}

impl ValueCallback {
    pub fn new(formula_type: FormulaType) -> Self {
        Self { formula_type, script_id: -1 }
    }

    pub fn get_min_max_values(
        &self,
        _player_id: crate::creatures::CreatureId,
        _damage: &mut CombatDamage,
    ) {
    }
}

/// Corresponds to C++ `class TileCallback : public CallBack`.
pub struct TileCallback {
    pub script_id: i32,
}

impl TileCallback {
    pub fn new() -> Self {
        Self { script_id: -1 }
    }

    pub fn on_tile_combat(
        &self,
        _caster_id: Option<crate::creatures::CreatureId>,
        _tile_pos: Position,
    ) {
    }
}

impl Default for TileCallback {
    fn default() -> Self {
        Self::new()
    }
}

/// Corresponds to C++ `class TargetCallback : public CallBack`.
pub struct TargetCallback {
    pub script_id: i32,
}

impl TargetCallback {
    pub fn new() -> Self {
        Self { script_id: -1 }
    }

    pub fn on_target_combat(
        &self,
        _caster_id: Option<crate::creatures::CreatureId>,
        _target_id: crate::creatures::CreatureId,
    ) {
    }
}

impl Default for TargetCallback {
    fn default() -> Self {
        Self::new()
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// CombatParams
// ──────────────────────────────────────────────────────────────────────────────

/// Corresponds to C++ `struct CombatParams`. Fields match 1:1.
pub struct CombatParams {
    pub conditions: Vec<Box<dyn Condition>>,

    pub value_callback: Option<Box<ValueCallback>>,
    pub tile_callback: Option<Box<TileCallback>>,
    pub target_callback: Option<Box<TargetCallback>>,

    pub item_id: u16,
    pub dispel_type: ConditionType,
    pub combat_type: CombatType,
    pub origin: CombatOrigin,

    pub impact_effect: u8,
    pub distance_effect: u8,

    pub blocked_by_armor: bool,
    pub blocked_by_shield: bool,
    pub target_caster_or_top_most: bool,
    pub aggressive: bool,
    pub use_charges: bool,
    pub ignore_resistances: bool,
}

impl Default for CombatParams {
    fn default() -> Self {
        Self {
            conditions: Vec::new(),
            value_callback: None,
            tile_callback: None,
            target_callback: None,
            item_id: 0,
            dispel_type: ConditionType::None,
            combat_type: CombatType::None,
            origin: CombatOrigin::Spell,
            impact_effect: CONST_ME_NONE,
            distance_effect: CONST_ANI_NONE,
            blocked_by_armor: false,
            blocked_by_shield: false,
            target_caster_or_top_most: false,
            aggressive: true,
            use_charges: false,
            ignore_resistances: false,
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Combat
// ──────────────────────────────────────────────────────────────────────────────

/// Corresponds to C++ `class Combat`. Non-copyable (no Clone derive).
pub struct Combat {
    pub params: CombatParams,
    pub formula_type: FormulaType,
    pub mina: f64,
    pub minb: f64,
    pub maxa: f64,
    pub maxb: f64,
    pub area: Option<Box<AreaCombat>>,
}

impl Default for Combat {
    fn default() -> Self {
        Self {
            params: CombatParams::default(),
            formula_type: FormulaType::Undefined,
            mina: 0.0,
            minb: 0.0,
            maxa: 0.0,
            maxb: 0.0,
            area: None,
        }
    }
}

impl Combat {
    pub fn new() -> Self {
        Self::default()
    }

    // ── Static predicates ───────────────────────────────────────────────────

    /// Both attacker and target must be in a PVP zone.
    ///
    /// Matches C++ Combat::isInPvpZone verbatim.
    /// Zone values are resolved by the caller from tile data.
    pub fn is_in_pvp_zone(
        attacker_zone: crate::creatures::ZoneType,
        target_zone: crate::creatures::ZoneType,
    ) -> bool {
        attacker_zone == crate::creatures::ZoneType::Pvp
            && target_zone == crate::creatures::ZoneType::Pvp
    }

    /// Protection check between two players.
    ///
    /// Matches C++ Combat::isProtected verbatim.
    /// Parameters are pre-resolved by the caller to avoid Game dependency here.
    pub fn is_protected(
        attacker_level: u32,
        attacker_vocation_allows_pvp: bool,
        attacker_skull: crate::creatures::Skull,
        attacker_skull_of_target: crate::creatures::Skull,
        target_level: u32,
        target_vocation_allows_pvp: bool,
        protection_level: u32,
    ) -> bool {
        if target_level < protection_level || attacker_level < protection_level {
            return true;
        }
        if !attacker_vocation_allows_pvp || !target_vocation_allows_pvp {
            return true;
        }
        if attacker_skull == crate::creatures::Skull::Black
            && attacker_skull_of_target == crate::creatures::Skull::None
        {
            return true;
        }
        false
    }

    /// Full creature-vs-creature combat permission check.
    /// Mirrors C++ `Combat::canDoCombat(Creature* attacker, Creature* target)`.
    pub fn can_do_combat_creature(
        attacker_id: crate::creatures::CreatureId,
        target_id: crate::creatures::CreatureId,
        aggressive: bool,
    ) -> ReturnValue {
        use crate::game::g_game;
        use crate::creatures::player::*;
        use crate::map::tile::*;

        let game = g_game().lock().unwrap();
        let Some(attacker) = game.get_creature(attacker_id) else { return ReturnValue::NoError };

        // CannotUseCombat
        if let Some(ap) = attacker.as_player() {
            if ap.has_flag(PLAYER_FLAG_CANNOT_USE_COMBAT) {
                return if game.get_creature(target_id).map(|c| c.is_player()).unwrap_or(false) {
                    ReturnValue::YouMayNotAttackThisPlayer
                } else {
                    ReturnValue::YouMayNotAttackThisCreature
                };
            }
        }

        // Target not attackable (CannotBeAttacked)
        if let Some(target) = game.get_creature(target_id) {
            if !target.is_attackable() {
                return if target.is_player() {
                    ReturnValue::YouMayNotAttackThisPlayer
                } else {
                    ReturnValue::YouMayNotAttackThisCreature
                };
            }
        }

        if !aggressive || attacker_id == target_id {
            return ReturnValue::NoError;
        }

        let target_is_player = game.get_creature(target_id).map(|c| c.is_player()).unwrap_or(false);
        let target_is_monster = game.get_creature(target_id).map(|c| c.is_monster()).unwrap_or(false);

        // Target is player
        if target_is_player {
            let target_player = game.get_creature(target_id).and_then(|c| c.as_player());

            if let Some(ap) = attacker.as_player() {
                if ap.has_flag(PLAYER_FLAG_CANNOT_ATTACK_PLAYER) {
                    return ReturnValue::YouMayNotAttackThisPlayer;
                }

                // Secure mode: can't attack unmarked players unless in PvP zone
                if ap.has_secure_mode() {
                    let both_in_pvp = game.map.get_tile(ap.base.position)
                        .map(|t| t.has_flag(TILESTATE_PVPZONE)).unwrap_or(false)
                        && game.get_creature(target_id).and_then(|c| game.map.get_tile(c.position()))
                            .map(|t| t.has_flag(TILESTATE_PVPZONE)).unwrap_or(false);
                    if !both_in_pvp {
                        let wt = game.get_world_type();
                        let skull_of_target = target_player
                            .map(|tp| ap.get_skull_client_of_player(tp, wt))
                            .unwrap_or(crate::creatures::Skull::None);
                        if skull_of_target == crate::creatures::Skull::None {
                            return ReturnValue::TurnSecureModeToAttackUnmarkedPlayers;
                        }
                    }
                }

                // isProtected: protection level + vocation pvp + skull
                if let Some(tp) = target_player {
                    use crate::world::vocation::g_vocations;
                    let wt = game.get_world_type();
                    let prot_level = crate::config::g_config().get_number(
                        crate::config::IntegerConfig::ProtectionLevel) as u32;
                    let ap_allows_pvp = g_vocations().get_vocation(ap.vocation_id)
                        .map(|v| v.allow_pvp).unwrap_or(true);
                    let tp_allows_pvp = g_vocations().get_vocation(tp.vocation_id)
                        .map(|v| v.allow_pvp).unwrap_or(true);
                    if Self::is_protected(
                        ap.level, ap_allows_pvp, ap.base.skull, ap.get_skull_client_of_player(tp, wt),
                        tp.level, tp_allows_pvp, prot_level,
                    ) {
                        return ReturnValue::YouMayNotAttackThisPlayer;
                    }
                }

                // Nopvp zone checks
                if let Some(tp) = target_player {
                    let target_pos = tp.base.position;
                    if game.map.get_tile(target_pos)
                        .map(|t| t.has_flag(TILESTATE_NOPVPZONE))
                        .unwrap_or(false)
                    {
                        return ReturnValue::ActionNotPermittedInANoPvpZone;
                    }
                    let ap_pos = ap.base.position;
                    let ap_in_nopvp = game.map.get_tile(ap_pos)
                        .map(|t| t.has_flag(TILESTATE_NOPVPZONE))
                        .unwrap_or(false);
                    let tp_not_in_pz_or_nopvp = !game.map.get_tile(target_pos)
                        .map(|t| t.has_flag(TILESTATE_PROTECTIONZONE) || t.has_flag(TILESTATE_NOPVPZONE))
                        .unwrap_or(false);
                    if ap_in_nopvp && tp_not_in_pz_or_nopvp {
                        return ReturnValue::ActionNotPermittedInANoPvpZone;
                    }
                }

            }

            // Summon-master checks
            if attacker.as_monster().map(|m| m.base.is_summon()).unwrap_or(false) {
                let master_id = attacker.as_monster().and_then(|m| m.base.master_id);
                if let Some(mid) = master_id {
                    if let Some(master_player) = game.get_creature(mid).and_then(|c| c.as_player()) {
                        if master_player.has_flag(PLAYER_FLAG_CANNOT_ATTACK_PLAYER) {
                            return ReturnValue::YouMayNotAttackThisPlayer;
                        }
                        if let Some(tp) = target_player {
                            use crate::world::vocation::g_vocations;
                            let prot_level = crate::config::g_config().get_number(
                                crate::config::IntegerConfig::ProtectionLevel) as u32;
                            let mp_allows_pvp = g_vocations().get_vocation(master_player.vocation_id)
                                .map(|v| v.allow_pvp).unwrap_or(true);
                            let tp_allows_pvp = g_vocations().get_vocation(tp.vocation_id)
                                .map(|v| v.allow_pvp).unwrap_or(true);
                            let wt = game.get_world_type();
                            if Self::is_protected(
                                master_player.level, mp_allows_pvp, master_player.base.skull,
                                master_player.get_skull_client_of_player(tp, wt),
                                tp.level, tp_allows_pvp, prot_level,
                            ) {
                                return ReturnValue::YouMayNotAttackThisPlayer;
                            }
                        }
                        let target_pos = game.get_creature(target_id).map(|c| c.position()).unwrap_or_default();
                        if game.map.get_tile(target_pos)
                            .map(|t| t.has_flag(TILESTATE_NOPVPZONE))
                            .unwrap_or(false)
                        {
                            return ReturnValue::ActionNotPermittedInANoPvpZone;
                        }
                    }
                }
            }
        } else if target_is_monster {
            // Attacker player vs monster
            if let Some(ap) = attacker.as_player() {
                if ap.has_flag(PLAYER_FLAG_CANNOT_ATTACK_MONSTER) {
                    return ReturnValue::YouMayNotAttackThisCreature;
                }
                // Summon in nopvp zone
                let target_is_summon = game.get_creature(target_id)
                    .and_then(|c| c.as_monster())
                    .map(|m| m.base.is_summon() && m.base.master_id
                        .and_then(|mid| game.get_creature(mid))
                        .map(|c| c.is_player()).unwrap_or(false))
                    .unwrap_or(false);
                if target_is_summon {
                    let target_pos = game.get_creature(target_id).map(|c| c.position()).unwrap_or_default();
                    if game.map.get_tile(target_pos)
                        .map(|t| t.has_flag(TILESTATE_NOPVPZONE))
                        .unwrap_or(false)
                    {
                        return ReturnValue::ActionNotPermittedInANoPvpZone;
                    }
                }
            }
            // Monster-vs-monster: non-player-summon can't attack non-player-summon
            if let Some(am) = attacker.as_monster() {
                let target_master_is_player = game.get_creature(target_id)
                    .and_then(|c| c.as_monster())
                    .and_then(|m| m.base.master_id)
                    .and_then(|mid| game.get_creature(mid))
                    .map(|c| c.is_player())
                    .unwrap_or(false);
                if !target_master_is_player {
                    let attacker_master_is_player = am.base.master_id
                        .and_then(|mid| game.get_creature(mid))
                        .map(|c| c.is_player())
                        .unwrap_or(false);
                    if !attacker_master_is_player {
                        return ReturnValue::YouMayNotAttackThisCreature;
                    }
                }
            }
        }

        // NO_PVP world: player/player-summon can't attack player/player-summon outside PvP zone
        if game.get_world_type() == crate::game::WorldType::NoPvp {
            let attacker_is_player_or_player_summon = attacker.is_player()
                || (attacker.as_monster().map(|m| m.base.is_summon()
                    && m.base.master_id.and_then(|mid| game.get_creature(mid))
                        .map(|c| c.is_player()).unwrap_or(false))
                    .unwrap_or(false));
            if attacker_is_player_or_player_summon {
                let both_in_pvp = game.map.get_tile(attacker.position())
                    .map(|t| t.has_flag(TILESTATE_PVPZONE)).unwrap_or(false)
                    && game.get_creature(target_id).and_then(|c| game.map.get_tile(c.position()))
                        .map(|t| t.has_flag(TILESTATE_PVPZONE)).unwrap_or(false);
                if !both_in_pvp {
                    if target_is_player {
                        return ReturnValue::YouMayNotAttackThisPlayer;
                    }
                    let target_is_player_summon = game.get_creature(target_id)
                        .and_then(|c| c.as_monster())
                        .map(|m| m.base.is_summon() && m.base.master_id
                            .and_then(|mid| game.get_creature(mid))
                            .map(|c| c.is_player()).unwrap_or(false))
                        .unwrap_or(false);
                    if target_is_player_summon {
                        return ReturnValue::YouMayNotAttackThisCreature;
                    }
                }
            }
        }

        ReturnValue::NoError
    }

    /// Target is player-controlled (player or player's summon).
    ///
    /// Matches C++ Combat::isPlayerCombat verbatim.
    pub fn is_player_combat(target_is_player: bool, target_master_is_player: bool) -> bool {
        target_is_player || target_master_is_player
    }

    // ── Type-conversion maps ────────────────────────────────────────────────

    /// Maps ConditionType → CombatType.
    ///
    /// Matches C++ Combat::ConditionToDamageType switch verbatim.
    pub fn condition_to_damage_type(t: ConditionType) -> CombatType {
        match t {
            ConditionType::Fire => CombatType::FireDamage,
            ConditionType::Energy => CombatType::EnergyDamage,
            ConditionType::Bleeding => CombatType::PhysicalDamage,
            ConditionType::Drown => CombatType::DrownDamage,
            ConditionType::Poison => CombatType::EarthDamage,
            ConditionType::Freezing => CombatType::IceDamage,
            ConditionType::Dazzled => CombatType::HolyDamage,
            ConditionType::Cursed => CombatType::DeathDamage,
            _ => CombatType::None,
        }
    }

    /// Maps CombatType → ConditionType.
    ///
    /// Matches C++ Combat::DamageToConditionType switch verbatim.
    pub fn damage_to_condition_type(t: CombatType) -> ConditionType {
        match t {
            CombatType::FireDamage => ConditionType::Fire,
            CombatType::EnergyDamage => ConditionType::Energy,
            CombatType::DrownDamage => ConditionType::Drown,
            CombatType::EarthDamage => ConditionType::Poison,
            CombatType::IceDamage => ConditionType::Freezing,
            CombatType::HolyDamage => ConditionType::Dazzled,
            CombatType::DeathDamage => ConditionType::Cursed,
            CombatType::PhysicalDamage => ConditionType::Bleeding,
            _ => ConditionType::None,
        }
    }

    // ── Tile-level combat permission check ──────────────────────────────────

    /// Tile permission check, pre-resolved flags only (no Tile* / Game* dependency).
    ///
    /// Covers the tile-flag subset of C++ Combat::canDoCombat(Creature*, Tile*, bool).
    /// The events callback portion (g_events->eventCreatureOnAreaCombat) is not
    /// called here; it must be handled by the dispatcher layer once Events is ported.
    #[allow(clippy::too_many_arguments)]
    pub fn can_do_combat_tile(
        block_projectile: bool,
        has_floor_change: bool,
        has_teleport: bool,
        caster_z: Option<u8>,
        tile_z: u8,
        caster_is_privileged_player: bool,
        aggressive: bool,
        tile_has_protection_zone: bool,
    ) -> ReturnValue {
        if block_projectile {
            return ReturnValue::NotEnoughRoom;
        }
        if has_floor_change {
            return ReturnValue::NotEnoughRoom;
        }
        if has_teleport {
            return ReturnValue::NotEnoughRoom;
        }
        if let Some(cz) = caster_z {
            if cz < tile_z {
                return ReturnValue::FirstGoDownStairs;
            }
            if cz > tile_z {
                return ReturnValue::FirstGoUpStairs;
            }
            if caster_is_privileged_player {
                return ReturnValue::NoError;
            }
        }
        if aggressive && tile_has_protection_zone {
            return ReturnValue::ActionNotPermittedInProtectionZone;
        }
        ReturnValue::NoError
    }

    // ── Parameter accessors ─────────────────────────────────────────────────

    /// Set a parameter by enum key, returning true on success.
    ///
    /// Matches C++ Combat::setParam(CombatParam_t, uint32_t) verbatim.
    pub fn set_param(&mut self, param: CombatParam, value: u32) -> bool {
        match param {
            CombatParam::Type => {
                self.params.combat_type = CombatType::from_u16(value as u16);
                true
            }
            CombatParam::Effect => {
                self.params.impact_effect = value as u8;
                true
            }
            CombatParam::DistanceEffect => {
                self.params.distance_effect = value as u8;
                true
            }
            CombatParam::BlockArmor => {
                self.params.blocked_by_armor = value != 0;
                true
            }
            CombatParam::BlockShield => {
                self.params.blocked_by_shield = value != 0;
                true
            }
            CombatParam::TargetCasterOrTopMost => {
                self.params.target_caster_or_top_most = value != 0;
                true
            }
            CombatParam::CreateItem => {
                self.params.item_id = value as u16;
                true
            }
            CombatParam::Aggressive => {
                self.params.aggressive = value != 0;
                true
            }
            CombatParam::Dispel => {
                self.params.dispel_type = ConditionType::from_u32(value);
                true
            }
            CombatParam::UseCharges => {
                self.params.use_charges = value != 0;
                true
            }
        }
    }

    /// Get a parameter by enum key.
    ///
    /// Matches C++ Combat::getParam verbatim; unknown keys return i32::MAX
    /// (no unknown key exists in the current enum, but the fallback preserves
    /// the C++ default-case behavior).
    pub fn get_param(&self, param: CombatParam) -> i32 {
        match param {
            CombatParam::Type => self.params.combat_type as i32,
            CombatParam::Effect => self.params.impact_effect as i32,
            CombatParam::DistanceEffect => self.params.distance_effect as i32,
            CombatParam::BlockArmor => self.params.blocked_by_armor as i32,
            CombatParam::BlockShield => self.params.blocked_by_shield as i32,
            CombatParam::TargetCasterOrTopMost => self.params.target_caster_or_top_most as i32,
            CombatParam::CreateItem => self.params.item_id as i32,
            CombatParam::Aggressive => self.params.aggressive as i32,
            CombatParam::Dispel => self.params.dispel_type as i32,
            CombatParam::UseCharges => self.params.use_charges as i32,
        }
    }

    /// Set Lua callback slot by key.
    ///
    /// Matches C++ Combat::setCallback verbatim.
    pub fn set_callback(&mut self, key: CallBackParam) -> bool {
        match key {
            CallBackParam::LevelMagicValue => {
                self.params.value_callback =
                    Some(Box::new(ValueCallback::new(FormulaType::LevelMagic)));
                true
            }
            CallBackParam::SkillValue => {
                self.params.value_callback =
                    Some(Box::new(ValueCallback::new(FormulaType::Skill)));
                true
            }
            CallBackParam::TargetTile => {
                self.params.tile_callback = Some(Box::new(TileCallback::new()));
                true
            }
            CallBackParam::TargetCreature => {
                self.params.target_callback = Some(Box::new(TargetCallback::new()));
                true
            }
        }
    }

    pub fn get_callback(&self, key: CallBackParam) -> bool {
        match key {
            CallBackParam::LevelMagicValue | CallBackParam::SkillValue => {
                self.params.value_callback.is_some()
            }
            CallBackParam::TargetTile => self.params.tile_callback.is_some(),
            CallBackParam::TargetCreature => self.params.target_callback.is_some(),
        }
    }

    pub fn set_area(&mut self, area: AreaCombat) {
        self.area = Some(Box::new(area));
    }

    pub fn has_area(&self) -> bool {
        self.area.is_some()
    }

    /// Prepend condition to the list (C++ uses forward_list::emplace_front).
    pub fn add_condition(&mut self, condition: Box<dyn Condition>) {
        self.params.conditions.insert(0, condition);
    }

    pub fn clear_conditions(&mut self) {
        self.params.conditions.clear();
    }

    pub fn set_origin(&mut self, origin: CombatOrigin) {
        self.params.origin = origin;
    }

    /// Set formula type and coefficients for player-based damage calculation.
    ///
    /// Matches C++ Combat::setPlayerCombatValues verbatim.
    pub fn set_player_combat_values(
        &mut self,
        formula_type: FormulaType,
        mina: f64,
        minb: f64,
        maxa: f64,
        maxb: f64,
    ) {
        self.formula_type = formula_type;
        self.mina = mina;
        self.minb = minb;
        self.maxa = maxa;
        self.maxb = maxb;
    }

    // ── Unported combat helpers (C++ has real logic — port when area combat is wired) ──

    pub fn post_combat_effects(
        &self,
        _caster_id: Option<crate::creatures::CreatureId>,
        _pos: Position,
    ) {
        // C++ emits distance effect from caster to pos. Used by area combat path.
    }

    pub fn add_distance_effect_static(
        _caster_id: Option<crate::creatures::CreatureId>,
        _from_pos: Position,
        _to_pos: Position,
        _effect: u8,
    ) {
        // C++ resolves CONST_ANI_WEAPONTYPE to weapon type, then calls g_game.addDistanceEffect.
    }

    pub fn get_combat_damage(
        &self,
        _caster_id: Option<crate::creatures::CreatureId>,
        _target_id: Option<crate::creatures::CreatureId>,
    ) -> CombatDamage {
        // C++ dispatches per formula_type (DAMAGE/LEVELMAGIC/SKILL). Returns computed min/max damage.
        CombatDamage::new(self.params.combat_type)
    }

    pub fn combat_tile_effects(
        _caster_id: Option<crate::creatures::CreatureId>,
        _tile_pos: Position,
        _params: &CombatParams,
    ) {
        // C++ spawns field items (fire/poison/energy/magic-wall) and fires tile callback.
    }

    pub fn do_area_combat(
        _caster_id: Option<crate::creatures::CreatureId>,
        _position: Position,
        _area: Option<&AreaCombat>,
        _damage: &mut CombatDamage,
        _params: &CombatParams,
    ) {
        // C++ full AoE pipeline: compute tiles, critical, spectators, per-tile effects + damage.
    }

    // ── Combat dispatch ──────────────────────────────────────────────────────

    /// Apply combat to a specific target creature.
    /// Mirrors C++ `Combat::doTargetCombat` verbatim.
    pub fn do_target_combat(
        caster_id: Option<crate::creatures::CreatureId>,
        target_id: crate::creatures::CreatureId,
        damage: &mut CombatDamage,
        params: &CombatParams,
    ) {
        use crate::game::g_game;
        use crate::creatures::player::{
            SPECIALSKILL_CRITICALHITCHANCE, SPECIALSKILL_CRITICALHITAMOUNT,
            SPECIALSKILL_LIFELEECHCHANCE, SPECIALSKILL_LIFELEECHAMOUNT,
            SPECIALSKILL_MANALEECHCHANCE, SPECIALSKILL_MANALEECHAMOUNT,
        };
        use crate::game::{CONST_ME_BLOODYSTEPS, CONST_ME_MAGIC_RED, CONST_ME_MAGIC_BLUE};

        if let Some(cid) = caster_id {
            if Self::can_do_combat_creature(cid, target_id, params.aggressive) != ReturnValue::NoError {
                return;
            }
        }

        // Distance effect
        if params.distance_effect != CONST_ANI_NONE {
            if let Some(cid) = caster_id {
                let (from, to) = {
                    let game = g_game().lock().unwrap();
                    let from = game.get_creature(cid).map(|c| c.position());
                    let to = game.get_creature(target_id).map(|c| c.position());
                    (from, to)
                };
                if let (Some(from), Some(to)) = (from, to) {
                    g_game().lock().unwrap().add_distance_effect(from, to, params.distance_effect);
                }
            }
        }

        // Gather caster player info
        let (caster_is_player, caster_skull) = {
            let game = g_game().lock().unwrap();
            match caster_id.and_then(|id| game.get_creature(id)) {
                Some(c) => (c.is_player(), c.get_skull()),
                None => (false, crate::creatures::Skull::None),
            }
        };

        let success;

        if damage.primary_type != CombatType::ManaDrain {
            // Block check
            {
                let mut game = g_game().lock().unwrap();
                if game.combat_block_hit(
                    damage,
                    caster_id,
                    target_id,
                    params.blocked_by_shield,
                    params.blocked_by_armor,
                    params.item_id != 0,
                    params.ignore_resistances,
                ) {
                    return;
                }
            }

            // PvP halving (non-black-skull attacker vs player target, not healing)
            if caster_is_player && damage.primary_type != CombatType::Healing {
                let target_is_player = g_game().lock().unwrap().get_creature(target_id).map(|c| c.is_player()).unwrap_or(false);
                let target_skull = g_game().lock().unwrap().get_creature(target_id).map(|c| c.get_skull()).unwrap_or(crate::creatures::Skull::None);
                if target_is_player && caster_skull != crate::creatures::Skull::Black && target_skull != crate::creatures::Skull::Black {
                    damage.primary_value /= 2;
                    damage.secondary_value /= 2;
                }
            }

            // Critical hit
            if caster_is_player && !damage.critical && damage.primary_type != CombatType::Healing && damage.origin != CombatOrigin::Condition {
                let (chance, skill) = {
                    let game = g_game().lock().unwrap();
                    let p = caster_id.and_then(|id| game.get_player(id));
                    (
                        p.map(|p| p.get_special_skill(SPECIALSKILL_CRITICALHITCHANCE) as u16).unwrap_or(0),
                        p.map(|p| p.get_special_skill(SPECIALSKILL_CRITICALHITAMOUNT) as u16).unwrap_or(0),
                    )
                };
                if chance > 0 && skill > 0 && crate::util::normal_random(1, 100) <= chance as i32 {
                    damage.primary_value += (damage.primary_value as f64 * (skill as f64 / 100.0)).round() as i32;
                    damage.secondary_value += (damage.secondary_value as f64 * (skill as f64 / 100.0)).round() as i32;
                    damage.critical = true;
                }
            }

            success = g_game().lock().unwrap().combat_change_health(caster_id, target_id, damage);
        } else {
            success = g_game().lock().unwrap().combat_change_mana(caster_id, target_id, damage);
        }

        if success {
            // Apply conditions
            if damage.block_type == BlockType::None || damage.block_type == BlockType::Armor {
                let caster_id_for_cond = caster_id;
                for condition in &params.conditions {
                    let immune = g_game().lock().unwrap()
                        .get_creature(target_id)
                        .map(|c| c.is_immune_condition(condition.get_type()))
                        .unwrap_or(true);
                    if caster_id == Some(target_id) || !immune {
                        let mut cond_copy = condition.clone_condition();
                        if let Some(cid) = caster_id_for_cond {
                            cond_copy.set_param(crate::combat::condition::ConditionParam::Owner, cid as i32);
                        }
                        let effects = {
                            let mut game_guard = g_game().lock().unwrap();
                            if let Some(creature) = game_guard.get_creature_mut(target_id) {
                                let base_speed = creature.base().base_speed as i32;
                                let conditions = &mut creature.base_mut().conditions;
                                crate::combat::condition::add_condition_to_creature(conditions, cond_copy, base_speed)
                            } else {
                                vec![]
                            }
                        };
                        if !effects.is_empty() {
                            crate::game::tick::apply_condition_effects(target_id, &effects);
                        }
                    }
                }
            }

            // Critical effect
            if damage.critical {
                if let Some(target_pos) = g_game().lock().unwrap().get_creature(target_id).map(|c| c.position()) {
                    g_game().lock().unwrap().add_magic_effect(target_pos, CONST_ME_BLOODYSTEPS);
                }
            }

            // Life / mana leech
            if !damage.leeched && damage.primary_type != CombatType::Healing && caster_is_player && damage.origin != CombatOrigin::Condition {
                let total_damage = (damage.primary_value + damage.secondary_value).unsigned_abs() as i32;

                let (lc, la, mc, ma, c_health, c_max_health, c_mana, c_max_mana, c_pos) = {
                    let game = g_game().lock().unwrap();
                    let p = caster_id.and_then(|id| game.get_player(id));
                    match p {
                        Some(p) => {
                            let pos = p.base.position;
                            (
                                p.get_special_skill(SPECIALSKILL_LIFELEECHCHANCE) as u16,
                                p.get_special_skill(SPECIALSKILL_LIFELEECHAMOUNT) as u16,
                                p.get_special_skill(SPECIALSKILL_MANALEECHCHANCE) as u16,
                                p.get_special_skill(SPECIALSKILL_MANALEECHAMOUNT) as u16,
                                p.base.health,
                                p.base.health_max,
                                p.get_mana() as i32,
                                p.get_max_mana() as i32,
                                pos,
                            )
                        }
                        None => (0, 0, 0, 0, 0, 0, 0, 0, Position::default()),
                    }
                };

                if c_health < c_max_health && lc > 0 && la > 0 && crate::util::normal_random(1, 100) <= lc as i32 {
                    let leech = (total_damage as f64 * (la as f64 / 100.0)).round() as i32;
                    let mut leech_dmg = CombatDamage::new(CombatType::Healing);
                    leech_dmg.primary_value = leech;
                    leech_dmg.origin = CombatOrigin::None;
                    leech_dmg.leeched = true;
                    if let Some(cid) = caster_id {
                        g_game().lock().unwrap().combat_change_health(None, cid, &mut leech_dmg);
                        g_game().lock().unwrap().add_magic_effect(c_pos, CONST_ME_MAGIC_RED);
                    }
                }

                if c_mana < c_max_mana && mc > 0 && ma > 0 && crate::util::normal_random(1, 100) <= mc as i32 {
                    let leech = (total_damage as f64 * (ma as f64 / 100.0)).round() as i32;
                    let mut leech_dmg = CombatDamage::new(CombatType::Healing);
                    leech_dmg.primary_value = leech;
                    leech_dmg.origin = CombatOrigin::None;
                    leech_dmg.leeched = true;
                    if let Some(cid) = caster_id {
                        g_game().lock().unwrap().combat_change_mana(None, cid, &mut leech_dmg);
                        g_game().lock().unwrap().add_magic_effect(c_pos, CONST_ME_MAGIC_BLUE);
                    }
                }
            }

            // Dispel
            if params.dispel_type == ConditionType::Paralyze {
                if let Some(creature) = g_game().lock().unwrap().get_creature_mut(target_id) {
                    creature.base_mut().conditions.retain(|c| c.get_type() != ConditionType::Paralyze);
                }
            } else if params.dispel_type != ConditionType::None {
                let dt = params.dispel_type;
                if let Some(creature) = g_game().lock().unwrap().get_creature_mut(target_id) {
                    creature.base_mut().conditions.retain(|c| c.get_type() != dt);
                }
            }
        }

        if let Some(cb) = &params.target_callback {
            cb.on_target_combat(caster_id, target_id);
        }
    }

}

// ──────────────────────────────────────────────────────────────────────────────
// MagicField
// ──────────────────────────────────────────────────────────────────────────────

/// Corresponds to C++ `class MagicField final : public Item`.
///
/// The Item base is not yet ported; item_id and owner_id substitute for
/// Item::getID() and Item::getOwner() respectively.
pub struct MagicField {
    pub item_id: u16,
    pub owner_id: u32,
    pub create_time: i64,
}

impl MagicField {
    pub fn new(item_id: u16) -> Self {
        Self {
            item_id,
            owner_id: 0,
            create_time: crate::util::otsys_time(),
        }
    }

    /// Whether this field can be replaced by another of the same type.
    pub fn is_replaceable(&self, items: &crate::items::Items) -> bool {
        items.get_item_type(self.item_id as usize).replaceable
    }

    pub fn get_combat_type(&self) -> CombatType {
        match self.item_id {
            1487..=1489 | 1492..=1494 | 1500 | 1501 => CombatType::FireDamage,
            1490 | 1496 | 1503 => CombatType::EarthDamage,
            1491 | 1495 | 1504 => CombatType::EnergyDamage,
            _ => CombatType::None,
        }
    }

    pub fn is_blocking(&self) -> bool {
        matches!(self.item_id, 1497 | 1498 | 1499 | 2721 | 11098 | 11099 | 20669 | 20670)
    }

    pub fn on_step_in_field(&self, creature_id: crate::creatures::CreatureId) {
        use crate::combat::condition::*;
        use crate::game::g_game;

        if self.is_blocking() {
            return;
        }

        if matches!(self.item_id, 1500 | 1501 | 1503 | 1504) {
            let game = g_game().lock().unwrap();
            if game.get_player(creature_id).is_some() {
                return;
            }
        }

        let combat_type = self.get_combat_type();
        if combat_type == CombatType::None {
            return;
        }

        let condition_type = Combat::damage_to_condition_type(combat_type);
        if condition_type == ConditionType::None {
            return;
        }

        let (tick_interval, damage_per_tick) = match combat_type {
            CombatType::FireDamage => (9000, -20),
            CombatType::EarthDamage => (5000, -5),
            CombatType::EnergyDamage => (10000, -25),
            _ => return,
        };

        let mut cond = ConditionDamage::new(
            ConditionId::Default,
            condition_type,
            false,
            0,
            true,
        );
        cond.period_damage = damage_per_tick;
        cond.tick_interval = tick_interval;
        cond.base.ticks = -1;
        cond.field = true;
        if self.owner_id != 0 {
            cond.base.owner = self.owner_id;
        }

        let mut game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature_mut(creature_id) {
            let base_speed = creature.base().base_speed as i32;
            add_condition_to_creature(
                &mut creature.base_mut().conditions,
                Box::new(cond),
                base_speed,
            );
        }
    }
}
