pub mod tick;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::sync::Arc;

use crate::combat::{BlockType, CombatDamage, CombatOrigin, CombatType};
use crate::creatures::{Creature, CreatureId, RaceType, Skull};
use crate::creatures::player::{Player, PlayerSex};
use crate::combat::condition::ConditionType;
use crate::items::Items;
use crate::map::{Map, Position};
use crate::net::game_protocol::send_packet_to_player;
use crate::net::output_message::OutputMessage;
use crate::runtime::{g_dispatcher, dispatcher::Task};
use crate::world::guild::Guild;
use crate::world::party::Party;
use crate::world::spawn::Spawns;

// ── TextColor constants (const.h TextColor_t) ────────────────────────────────
pub const TEXTCOLOR_BLUE: u8 = 5;
pub const TEXTCOLOR_GREEN: u8 = 18;
pub const TEXTCOLOR_LIGHTGREEN: u8 = 66;
pub const TEXTCOLOR_LIGHTBLUE: u8 = 89;
pub const TEXTCOLOR_MAYABLUE: u8 = 95;
pub const TEXTCOLOR_DARKRED: u8 = 108;
pub const TEXTCOLOR_GREY: u8 = 129;
pub const TEXTCOLOR_TEAL: u8 = 143;
pub const TEXTCOLOR_PURPLE: u8 = 154;
pub const TEXTCOLOR_RED: u8 = 180;
pub const TEXTCOLOR_ORANGE: u8 = 192;
pub const TEXTCOLOR_PASTELRED: u8 = 194;
pub const TEXTCOLOR_YELLOW: u8 = 210;
pub const TEXTCOLOR_WHITE: u8 = 215;
pub const TEXTCOLOR_NONE: u8 = 255;

// ── MagicEffect constants (const.h MagicEffect_t) ────────────────────────────
pub const CONST_ME_DRAWBLOOD: u8 = 1;
pub const CONST_ME_LOSEENERGY: u8 = 2;
pub const CONST_ME_POFF: u8 = 3;
pub const CONST_ME_BLOCKHIT: u8 = 4;
pub const CONST_ME_GREEN_RINGS: u8 = 9;
pub const CONST_ME_HITAREA: u8 = 10;
pub const CONST_ME_TELEPORT: u8 = 11;
pub const CONST_ME_SLEEP: u8 = 33;
pub const CONST_ME_ENERGYHIT: u8 = 12;
pub const CONST_ME_MAGIC_BLUE: u8 = 13;
pub const CONST_ME_MAGIC_RED: u8 = 14;
pub const CONST_ME_HITBYFIRE: u8 = 16;
pub const CONST_ME_HITBYPOISON: u8 = 17;
pub const CONST_ME_SMALLCLOUDS: u8 = 39;
pub const CONST_ME_HOLYDAMAGE: u8 = 40;
pub const CONST_ME_ICEATTACK: u8 = 44;
pub const CONST_ME_BLOODYSTEPS: u8 = 64;

// ── Message type ──────────────────────────────────────────────────────────────
pub const MESSAGE_STATUS_DEFAULT: u8 = 20; // 0x14
pub const MESSAGE_INFO_DESCR: u8 = 21;

static G_GAME: OnceLock<Mutex<Game>> = OnceLock::new();

pub fn g_game() -> &'static Mutex<Game> {
    G_GAME.get().expect("game not initialized")
}

pub(crate) fn init_game(game: Game) {
    G_GAME
        .set(Mutex::new(game))
        .unwrap_or_else(|_| panic!("game already initialized"));

    // DBG watchdog: if g_game stays unlockable for >3s, report a likely deadlock.
    std::thread::spawn(|| {
        let mut stuck_since: Option<std::time::Instant> = None;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            match G_GAME.get().and_then(|m| m.try_lock().ok()) {
                Some(_guard) => { stuck_since = None; }
                None => {
                    let since = stuck_since.get_or_insert_with(std::time::Instant::now);
                    if since.elapsed().as_secs() >= 3 {
                        tracing::error!(
                            "DBG WATCHDOG: g_game unlockable for {}s — likely DEADLOCK",
                            since.elapsed().as_secs()
                        );
                        stuck_since = Some(std::time::Instant::now());
                    }
                }
            }
        }
    });
}

pub const PLAYER_NAME_LENGTH: i32 = 25;

pub const ITEM_GOLD_COIN: u16 = 2148;
pub const ITEM_PLATINUM_COIN: u16 = 2152;
pub const ITEM_CRYSTAL_COIN: u16 = 2160;

pub const EVENT_DECAYINTERVAL: i32 = 250;
pub const EVENT_DECAY_BUCKETS: i32 = 4;

pub const MOVE_CREATURE_INTERVAL: i32 = 1_000;
pub const RANGE_MOVE_CREATURE_INTERVAL: i32 = 1_500;
pub const RANGE_MOVE_ITEM_INTERVAL: i32 = 400;
pub const RANGE_USE_ITEM_INTERVAL: i32 = 400;
pub const RANGE_USE_ITEM_EX_INTERVAL: i32 = 400;
pub const RANGE_USE_WITH_CREATURE_INTERVAL: i32 = 400;
pub const RANGE_ROTATE_ITEM_INTERVAL: i32 = 400;
pub const RANGE_BROWSE_FIELD_INTERVAL: i32 = 400;
pub const RANGE_WRAP_ITEM_INTERVAL: i32 = 400;
pub const RANGE_REQUEST_TRADE_INTERVAL: i32 = 400;

pub const LIGHT_DAY: u8 = 250;
pub const LIGHT_NIGHT: u8 = 40;
pub const GAME_SUNRISE: i16 = 360;
pub const GAME_DAYTIME: i16 = 480;
pub const GAME_SUNSET: i16 = 1080;
pub const GAME_NIGHTTIME: i16 = 1200;

pub const LIGHT_CHANGE_SUNRISE: f32 = {
    let raw = ((LIGHT_DAY as f32 - LIGHT_NIGHT as f32) / (GAME_DAYTIME as f32 - GAME_SUNRISE as f32)) * 100.0;
    (raw as i32) as f32 / 100.0
};
pub const LIGHT_CHANGE_SUNSET: f32 = {
    let raw = ((LIGHT_DAY as f32 - LIGHT_NIGHT as f32) / (GAME_NIGHTTIME as f32 - GAME_SUNSET as f32)) * 100.0;
    (raw as i32) as f32 / 100.0
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameState {
    Startup,
    Init,
    Normal,
    Closed,
    Shutdown,
    Closing,
    Maintain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldType {
    NoPvp = 1,
    Pvp = 2,
    PvpEnforced = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackPosType {
    Move,
    Look,
    TopDownItem,
    UseItem,
    UseTarget,
}

pub const EVENT_CREATURECOUNT: usize = 10;
pub const EVENT_CHECK_CREATURE_INTERVAL: u64 = 100;
pub const EVENT_CREATURE_THINK_INTERVAL: u64 = 1000;
pub const EVENT_WORLDTIMEINTERVAL: u64 = 2_500;
pub const EVENT_LIGHTINTERVAL: u64 = 10_000;

pub struct DecayEntry {
    pub server_id: u16,
    pub position: Position,
    pub remaining_ms: i32,
}

pub struct Game {
    game_state: GameState,
    world_type: WorldType,
    light_level: u8,
    light_color: u8,
    world_time: i16,
    players_record: u32,
    motd_hash: String,
    motd_num: u32,
    start_time: i64,

    pub items: Arc<Items>,
    pub map: Map,
    creatures: HashMap<CreatureId, Creature>,
    player_name_to_id: HashMap<String, CreatureId>,
    player_guid_to_id: HashMap<u32, CreatureId>,
    guilds: HashMap<u32, Guild>,
    parties: HashMap<CreatureId, Party>,
    bed_sleepers: HashMap<u32, Position>,
    next_creature_id: u32,

    pub check_creature_lists: [Vec<CreatureId>; EVENT_CREATURECOUNT],
    pub spawns: Spawns,

    pub to_decay_items: Vec<DecayEntry>,
    pub decay_buckets: [Vec<DecayEntry>; EVENT_DECAY_BUCKETS as usize],
    pub last_decay_bucket: usize,
}

impl Game {
    pub fn new() -> Self {
        Self {
            game_state: GameState::Normal,
            world_type: WorldType::Pvp,
            light_level: 250,
            light_color: 215,
            world_time: 0,
            players_record: 0,
            motd_hash: String::new(),
            motd_num: 0,
            start_time: crate::util::otsys_time(),
            items: Arc::new(Items::new()),
            map: Map::default(),
            creatures: HashMap::new(),
            player_name_to_id: HashMap::new(),
            player_guid_to_id: HashMap::new(),
            guilds: HashMap::new(),
            parties: HashMap::new(),
            bed_sleepers: HashMap::new(),
            // Player ids start at 0x10000000 (C++ Player::playerAutoID); monsters
            // use 0x40000000 and npcs 0x80000000 from their own counters.
            next_creature_id: 0x10000000,
            check_creature_lists: Default::default(),
            spawns: Spawns::default(),
            to_decay_items: Vec::new(),
            decay_buckets: Default::default(),
            last_decay_bucket: 0,
        }
    }

    pub fn add_creature_check(&mut self, creature_id: CreatureId) {
        let Some(creature) = self.creatures.get_mut(&creature_id) else { return };
        if creature.base().in_check_creatures_vector {
            creature.base_mut().creature_check = true;
            return;
        }
        creature.base_mut().creature_check = true;
        creature.base_mut().in_check_creatures_vector = true;
        let bucket = (creature_id as usize) % EVENT_CREATURECOUNT;
        self.check_creature_lists[bucket].push(creature_id);
    }

    pub fn remove_creature_check(&mut self, creature_id: CreatureId) {
        if let Some(creature) = self.creatures.get_mut(&creature_id) {
            creature.base_mut().creature_check = false;
        }
    }

    pub fn get_check_creature_list(&self, index: usize) -> &Vec<CreatureId> {
        &self.check_creature_lists[index % EVENT_CREATURECOUNT]
    }

    pub fn drain_dead_from_check_list(&mut self, index: usize) {
        let bucket = index % EVENT_CREATURECOUNT;
        let to_remove: Vec<CreatureId> = self.check_creature_lists[bucket]
            .iter()
            .copied()
            .filter(|&id| {
                self.creatures.get(&id).map(|c| !c.base().creature_check).unwrap_or(true)
            })
            .collect();
        if !to_remove.is_empty() {
            self.check_creature_lists[bucket].retain(|id| !to_remove.contains(id));
        }
    }

    pub fn get_game_state(&self) -> GameState {
        self.game_state
    }

    pub fn set_game_state(&mut self, new_state: GameState) {
        if self.game_state == GameState::Shutdown {
            return;
        }
        if self.game_state == new_state {
            return;
        }
        self.game_state = new_state;

        if new_state == GameState::Closed {
            let to_kick: Vec<crate::creatures::CreatureId> = self.player_name_to_id.values()
                .copied()
                .filter(|&id| {
                    self.get_player(id)
                        .map(|p| !p.has_flag(crate::creatures::player::PLAYER_FLAG_CAN_ALWAYS_LOGIN))
                        .unwrap_or(false)
                })
                .collect();
            for id in to_kick {
                self.remove_creature_check(id);
                self.remove_player(id);
                crate::net::game_protocol::unregister_player_connection(id);
            }
        }
    }

    pub fn set_world_type(&mut self, world_type: WorldType) {
        self.world_type = world_type;
    }

    pub fn get_world_type(&self) -> WorldType {
        self.world_type
    }

    pub fn get_world_time(&self) -> i16 {
        self.world_time
    }

    pub fn get_players_record(&self) -> u32 {
        self.players_record
    }

    pub fn get_motd_hash(&self) -> &str {
        &self.motd_hash
    }

    pub fn get_motd_num(&self) -> u32 {
        self.motd_num
    }

    pub fn increment_motd_num(&mut self) {
        self.motd_num += 1;
    }

    pub fn get_world_light_info(&self) -> (u8, u8) {
        (self.light_level, self.light_color)
    }

    pub fn set_world_light_info(&mut self, level: u8, color: u8) {
        self.light_level = level;
        self.light_color = color;
    }

    pub fn get_items(&self) -> &Items {
        &self.items
    }

    pub fn get_map(&self) -> &Map {
        &self.map
    }

    pub fn next_creature_id(&mut self) -> CreatureId {
        // Post-increment to match C++ `id = playerAutoID++` (first id = base).
        let id = self.next_creature_id;
        self.next_creature_id += 1;
        id
    }

    pub fn add_creature(&mut self, creature: Creature) {
        let id = creature.id();
        let pos = creature.position();
        let is_player = creature.is_player();

        if let Some(player) = creature.as_player() {
            self.player_name_to_id.insert(player.name.clone(), id);
            self.player_guid_to_id.insert(player.guid, id);
        }

        self.creatures.insert(id, creature);
        self.map.add_creature_to_tile(pos, id, is_player);
    }

    /// Place a creature on the map and broadcast its appearance to spectators.
    /// Mirrors C++ `Game::placeCreature` (non-startup path).
    /// Place a creature on the map. Returns `true` on success.
    ///
    /// NOTE: this does NOT broadcast the appearance to spectators — the caller
    /// must invoke `broadcast_creature_appear` AFTER releasing the `g_game`
    /// lock. Broadcasting from here would re-lock the (non-reentrant) `g_game`
    /// mutex on the same thread and deadlock, since `place_creature` already
    /// runs while the caller holds that lock.
    pub fn place_creature(&mut self, creature: Creature) -> bool {
        let pos = creature.position();
        // Check tile exists and is walkable.
        if self.map.get_tile(pos).is_none() {
            return false;
        }
        let creature_id = creature.id();
        self.add_creature(creature);
        self.add_creature_check(creature_id);
        true
    }

    pub fn remove_creature(&mut self, creature_id: CreatureId) {
        if let Some(creature) = self.creatures.remove(&creature_id) {
            let pos = creature.position();
            let is_player = creature.is_player();

            if let Some(player) = creature.as_player() {
                if self.player_name_to_id.get(&player.name) == Some(&creature_id) {
                    self.player_name_to_id.remove(&player.name);
                }
                if self.player_guid_to_id.get(&player.guid) == Some(&creature_id) {
                    self.player_guid_to_id.remove(&player.guid);
                }
            }

            self.map.remove_creature_from_tile(pos, creature_id, is_player);
        }
    }

    pub fn add_player(&mut self, creature_id: CreatureId, mut player: Player) {
        player.base.id = creature_id;
        let pos = player.base.position;
        let name = player.name.clone();
        let guid = player.guid;
        self.player_name_to_id.insert(name, creature_id);
        self.player_guid_to_id.insert(guid, creature_id);
        self.creatures.insert(creature_id, Creature::Player(Box::new(player)));
        self.map.add_creature_to_tile(pos, creature_id, true);
    }

    pub fn remove_player(&mut self, creature_id: CreatureId) {
        // Clean up party membership so logged-out players leave no dangling ids.
        if let Some(leader_id) = self.get_player(creature_id).and_then(|p| p.party_id) {
            if leader_id == creature_id {
                self.party_disband(leader_id);
            } else {
                if let Some(p) = self.get_party_mut(leader_id) {
                    p.member_ids_mut().retain(|&id| id != creature_id);
                    p.clear_player_points(creature_id);
                }
                if let Some(p) = self.get_player_mut(creature_id) {
                    p.party_id = None;
                }
                self.party_update_shared_experience(leader_id);
                let empty = self.get_party(leader_id).map(|p| p.is_empty()).unwrap_or(true);
                if empty {
                    self.party_disband(leader_id);
                }
            }
        }

        if let Some(creature) = self.creatures.remove(&creature_id) {
            let pos = creature.position();
            if let Some(player) = creature.as_player() {
                if self.player_name_to_id.get(&player.name) == Some(&creature_id) {
                    self.player_name_to_id.remove(&player.name);
                }
                if self.player_guid_to_id.get(&player.guid) == Some(&creature_id) {
                    self.player_guid_to_id.remove(&player.guid);
                }
            }
            self.map.remove_creature_from_tile(pos, creature_id, true);
        }
    }

    pub fn get_creature(&self, creature_id: CreatureId) -> Option<&Creature> {
        self.creatures.get(&creature_id)
    }

    pub fn get_creature_mut(&mut self, creature_id: CreatureId) -> Option<&mut Creature> {
        self.creatures.get_mut(&creature_id)
    }

    pub fn get_player(&self, creature_id: CreatureId) -> Option<&Player> {
        self.creatures.get(&creature_id)?.as_player()
    }

    pub fn get_player_mut(&mut self, creature_id: CreatureId) -> Option<&mut Player> {
        self.creatures.get_mut(&creature_id)?.as_player_mut()
    }

    pub fn get_player_by_name(&self, name: &str) -> Option<&Player> {
        let id = self.player_name_to_id.get(name)?;
        self.get_player(*id)
    }

    pub fn get_player_by_guid(&self, guid: u32) -> Option<&Player> {
        let id = self.player_guid_to_id.get(&guid)?;
        self.get_player(*id)
    }

    pub fn get_player_id_by_name(&self, name: &str) -> Option<CreatureId> {
        self.player_name_to_id.get(name).copied()
    }

    pub fn get_player_id_by_guid(&self, guid: u32) -> Option<CreatureId> {
        self.player_guid_to_id.get(&guid).copied()
    }

    pub fn get_player_count(&self) -> usize {
        self.player_name_to_id.len()
    }

    pub fn get_monster_count(&self) -> usize {
        self.creatures.values().filter(|c| c.is_monster()).count()
    }

    pub fn get_npc_count(&self) -> usize {
        self.creatures.values().filter(|c| c.is_npc()).count()
    }

    pub fn get_uptime_seconds(&self) -> i64 {
        (crate::util::otsys_time() - self.start_time) / 1000
    }

    pub fn get_spectators(
        &mut self,
        center_pos: Position,
        multifloor: bool,
        only_players: bool,
    ) -> Vec<CreatureId> {
        self.map.get_spectators(center_pos, multifloor, only_players, 0, 0, 0, 0)
    }

    /// Resolve a one-step walk destination including height-ramp up/down and
    /// explicit floor-change-flag stairs. Port of `Game::internalMoveCreature(dir)`.
    pub fn resolve_walk_destination(
        &self,
        current_pos: Position,
        dir: crate::creatures::Direction,
        is_player: bool,
    ) -> Option<Position> {
        use crate::map::tile::{TILESTATE_BLOCKSOLID, TILESTATE_FLOORCHANGE, TILESTATE_IMMOVABLEBLOCKSOLID};

        let (dx, dy): (i32, i32) = match dir {
            crate::creatures::Direction::North => (0, -1),
            crate::creatures::Direction::South => (0, 1),
            crate::creatures::Direction::East => (1, 0),
            crate::creatures::Direction::West => (-1, 0),
            crate::creatures::Direction::NorthEast => (1, -1),
            crate::creatures::Direction::SouthEast => (1, 1),
            crate::creatures::Direction::SouthWest => (-1, 1),
            crate::creatures::Direction::NorthWest => (-1, -1),
        };
        let mut dest = Position {
            x: current_pos.x.wrapping_add(dx as u16),
            y: current_pos.y.wrapping_add(dy as u16),
            z: current_pos.z,
        };

        let diagonal = (dir as u8) & 0x04 != 0;
        if is_player && !diagonal {
            // try to go up a ramp
            if current_pos.z != 8 {
                let cur_has_height = self
                    .map
                    .get_tile(current_pos)
                    .map(|t| t.has_height(3, &self.items))
                    .unwrap_or(false);
                if cur_has_height {
                    let above_cur = self.map.get_tile(Position {
                        x: current_pos.x,
                        y: current_pos.y,
                        z: current_pos.z.wrapping_sub(1),
                    });
                    let above_cur_open = match above_cur {
                        None => true,
                        Some(t) => t.ground.is_none() && !t.has_flag(TILESTATE_BLOCKSOLID),
                    };
                    if above_cur_open {
                        let above_dest = self.map.get_tile(Position {
                            x: dest.x,
                            y: dest.y,
                            z: dest.z.wrapping_sub(1),
                        });
                        if let Some(ad) = above_dest {
                            if ad.ground.is_some()
                                && !ad.has_flag(TILESTATE_IMMOVABLEBLOCKSOLID)
                                && !ad.has_flag(TILESTATE_FLOORCHANGE)
                            {
                                dest.z = dest.z.wrapping_sub(1);
                            }
                        }
                    }
                }
            }

            // try to go down a ramp (only if we did not go up)
            if current_pos.z != 7 && current_pos.z == dest.z {
                let dest_open = match self.map.get_tile(dest) {
                    None => true,
                    Some(t) => t.ground.is_none() && !t.has_flag(TILESTATE_BLOCKSOLID),
                };
                if dest_open {
                    let below_dest = self.map.get_tile(Position {
                        x: dest.x,
                        y: dest.y,
                        z: dest.z.wrapping_add(1),
                    });
                    if let Some(bd) = below_dest {
                        if bd.has_height(3, &self.items)
                            && !bd.has_flag(TILESTATE_IMMOVABLEBLOCKSOLID)
                        {
                            dest.z = dest.z.wrapping_add(1);
                        }
                    }
                }
            }
        }

        // Explicit floor-change-flag stairs on the resolved destination tile.
        self.map.resolve_floor_change_destination(dest)
    }

    pub fn move_creature_position(&mut self, creature_id: CreatureId, old_pos: Position, new_pos: Position) {
        let is_player = self.creatures.get(&creature_id).map(|c| c.is_player()).unwrap_or(false);
        self.map.move_creature_on_map(old_pos, new_pos, creature_id, is_player);
        if let Some(creature) = self.creatures.get_mut(&creature_id) {
            creature.base_mut().position = new_pos;
        }

        // onAttackedCreatureChangeZone: when this creature enters PZ or nopvp,
        // cancel attack for all players targeting it (unless IgnoreProtectionZone).
        let dest_pz = self.map.get_tile(new_pos)
            .map(|t| t.has_flag(crate::map::tile::TILESTATE_PROTECTIONZONE))
            .unwrap_or(false);
        let dest_nopvp = self.map.get_tile(new_pos)
            .map(|t| t.has_flag(crate::map::tile::TILESTATE_NOPVPZONE))
            .unwrap_or(false);
        let moved_is_player = self.creatures.get(&creature_id).map(|c| c.is_player()).unwrap_or(false);
        if dest_pz || (dest_nopvp && moved_is_player) {
            let to_cancel: Vec<CreatureId> = self.creatures.values()
                .filter_map(|c| {
                    let p = c.as_player()?;
                    if p.base.attacked_creature_id != Some(creature_id) { return None; }
                    if p.has_flag(crate::creatures::player::PLAYER_FLAG_IGNORE_PROTECTION_ZONE) { return None; }
                    if dest_nopvp && !moved_is_player { return None; }
                    Some(p.base.id)
                })
                .collect();
            for pid in &to_cancel {
                if let Some(p) = self.get_player_mut(*pid) {
                    p.base.attacked_creature_id = None;
                }
                crate::net::game_protocol::send_packet_to_player(*pid, |output| {
                    output.add_byte(0xA3);
                    output.add_u32(0);
                });
            }
        }
    }

    pub fn update_world_light_level(&mut self) {
        let wt = self.world_time;
        if (GAME_SUNRISE..=GAME_DAYTIME).contains(&wt) {
            self.light_level = (((GAME_DAYTIME - GAME_SUNRISE) as f32
                - (GAME_DAYTIME - wt) as f32)
                * LIGHT_CHANGE_SUNRISE
                + LIGHT_NIGHT as f32) as u8;
        } else if (GAME_SUNSET..=GAME_NIGHTTIME).contains(&wt) {
            self.light_level = (LIGHT_DAY as f32
                - ((wt - GAME_SUNSET) as f32 * LIGHT_CHANGE_SUNSET)) as u8;
        } else if !(GAME_SUNRISE..GAME_NIGHTTIME).contains(&wt) {
            self.light_level = LIGHT_NIGHT;
        } else {
            self.light_level = LIGHT_DAY;
        }
    }

    pub fn set_world_time(&mut self, t: i16) {
        self.world_time = t;
    }

    pub fn set_motd(&mut self, num: u32, hash: String) {
        self.motd_num = num;
        self.motd_hash = hash;
    }

    pub fn set_players_record(&mut self, record: u32) {
        self.players_record = record;
    }

    pub fn update_world_time(&mut self) {
        self.world_time = (self.world_time + 1) % 1440;
    }

    pub fn get_creature_by_name(&self, name: &str) -> Option<&Creature> {
        self.creatures.values().find(|c| {
            match c {
                Creature::Player(p) => p.name.eq_ignore_ascii_case(name),
                Creature::Monster(m) => m.get_name().eq_ignore_ascii_case(name),
                Creature::Npc(n) => n.get_name().eq_ignore_ascii_case(name),
            }
        })
    }

    pub fn get_all_players(&self) -> Vec<CreatureId> {
        self.player_name_to_id.values().copied().collect()
    }

    pub fn get_players_online(&self) -> impl Iterator<Item = (&CreatureId, &Player)> + '_ {
        self.creatures.iter().filter_map(|(id, c)| {
            c.as_player().map(|p| (id, p))
        })
    }

    pub fn get_guild(&self, guild_id: u32) -> Option<&Guild> {
        self.guilds.get(&guild_id)
    }

    pub fn get_guild_mut(&mut self, guild_id: u32) -> Option<&mut Guild> {
        self.guilds.get_mut(&guild_id)
    }

    pub fn add_guild(&mut self, guild: Guild) {
        self.guilds.insert(guild.id, guild);
    }

    pub fn get_party(&self, leader_id: CreatureId) -> Option<&Party> {
        self.parties.get(&leader_id)
    }

    pub fn get_party_mut(&mut self, leader_id: CreatureId) -> Option<&mut Party> {
        self.parties.get_mut(&leader_id)
    }

    pub fn create_party(&mut self, leader_id: CreatureId) -> &mut Party {
        self.parties.entry(leader_id).or_insert_with(|| Party::new(leader_id));
        if let Some(player) = self.get_player_mut(leader_id) {
            player.party_id = Some(leader_id);
        }
        self.parties.get_mut(&leader_id).unwrap()
    }

    pub fn remove_party(&mut self, leader_id: CreatureId) {
        if let Some(party) = self.parties.remove(&leader_id) {
            for &member_id in party.get_members() {
                if let Some(player) = self.get_player_mut(member_id) {
                    player.party_id = None;
                }
            }
            if let Some(player) = self.get_player_mut(leader_id) {
                player.party_id = None;
            }
        }
    }

    /// Apply a health delta to a creature, clamped to [0, health_max].
    /// Returns `Some((pos, new_health, health_max, hidden, died))` if health actually
    /// changed, or `None` if there was no change. Caller must broadcast and handle death.
    pub fn apply_health_change(
        &mut self,
        creature_id: CreatureId,
        delta: i32,
    ) -> Option<(crate::map::Position, i32, i32, bool, bool)> {
        let creature = self.creatures.get_mut(&creature_id)?;
        let health_max = creature.base().health_max;
        let old = creature.base().health;
        let new = if delta > 0 {
            old + delta.min(health_max - old)
        } else {
            (old + delta).max(0)
        };
        if old == new {
            return None;
        }
        creature.base_mut().health = new;
        let hidden = creature.base().hidden_health;
        let pos = creature.position();
        let died = new <= 0;
        Some((pos, new, health_max, hidden, died))
    }

    /// Get the client-visible stackpos of a creature on its current tile.
    pub fn get_creature_stackpos(&self, creature_id: CreatureId) -> i32 {
        let Some(creature) = self.creatures.get(&creature_id) else { return -1 };
        let pos = creature.position();
        self.map.get_tile(pos)
            .map(|t| t.get_client_index_of_creature(creature_id))
            .unwrap_or(-1)
    }

    // ── Effect / text broadcast helpers ──────────────────────────────────────

    /// Send animated floating text (0x84) to all player spectators at pos.
    pub fn add_animated_text(&mut self, pos: Position, color: u8, text: &str) {
        // The standalone 0x84 AnimatedText S2C packet only exists on the 8.60
        // protocol. On 10.98 it was removed — floating damage/heal numbers are
        // carried inside the 0xB4 text message (MESSAGE_DAMAGE_*/HEALED). Emitting
        // 0x84 to a 10.98 client desyncs the byte stream, so skip it there.
        if crate::net::protocol_version::client_version().is_1098() {
            return;
        }
        let spectators = self.map.get_spectators(pos, true, true, 0, 0, 0, 0);
        let text_bytes: Vec<u8> = text.as_bytes().to_vec();
        for spec_id in spectators {
            let tb = text_bytes.clone();
            send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
                output.add_byte(0x84);
                output.add_position(pos.x, pos.y, pos.z);
                output.add_byte(color);
                output.add_string(&tb);
            });
        }
    }

    /// Send magic effect (0x83) to all player spectators at pos.
    pub fn add_magic_effect(&mut self, pos: Position, effect: u8) {
        let spectators = self.map.get_spectators(pos, true, true, 0, 0, 0, 0);
        for spec_id in spectators {
            send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
                output.add_byte(0x83);
                output.add_position(pos.x, pos.y, pos.z);
                output.add_byte(effect);
            });
        }
    }

    /// Send distance shoot (0x85) to union of spectators at from_pos and to_pos.
    pub fn add_distance_effect(&mut self, from_pos: Position, to_pos: Position, effect: u8) {
        let mut spectators = self.map.get_spectators(from_pos, true, true, 0, 0, 0, 0);
        let to_specs = self.map.get_spectators(to_pos, true, true, 0, 0, 0, 0);
        for s in to_specs {
            if !spectators.contains(&s) {
                spectators.push(s);
            }
        }
        for spec_id in spectators {
            send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
                output.add_byte(0x85);
                output.add_position(from_pos.x, from_pos.y, from_pos.z);
                output.add_position(to_pos.x, to_pos.y, to_pos.z);
                output.add_byte(effect);
            });
        }
    }

    /// Send creature health bar (0x8C) to all player spectators around target.
    pub fn add_creature_health(&mut self, creature_id: CreatureId) {
        let Some(creature) = self.creatures.get(&creature_id) else { return };
        let pos = creature.position();
        let health = creature.get_health();
        let health_max = creature.get_max_health();
        let health_percent = if health_max > 0 {
            ((health as i64 * 100) / health_max as i64).clamp(0, 100) as u8
        } else {
            0
        };
        let spectators = self.map.get_spectators(pos, true, true, 0, 0, 0, 0);
        for spec_id in spectators {
            send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
                output.add_byte(0x8C);
                output.add_u32(creature_id);
                output.add_byte(health_percent);
            });
        }
    }

    /// Add an item to a tile and broadcast `0x6A` (add-tile-thing) to spectators.
    pub fn place_item_on_tile(&mut self, pos: Position, server_id: u16) {
        use crate::map::tile::MapItem;
        self.place_map_item_on_tile(pos, MapItem { server_id, ..MapItem::default() });
    }

    pub fn place_map_item_on_tile(&mut self, pos: Position, item: crate::map::tile::MapItem) {
        let server_id = item.server_id;
        if server_id == 0 { return; }

        let items_arc = self.items.clone();
        let (stackpos, count) = {
            let count = item.count.clamp(1, 255) as u8;
            let Some(tile) = self.map.get_tile_mut(pos) else { return };
            let sp = tile.add_item_get_stackpos(item, &items_arc);
            (sp, count)
        };

        let spectators = self.map.get_spectators(pos, true, true, 0, 0, 0, 0);
        for spec_id in spectators {
            let items_ref = items_arc.clone();
            send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
                output.add_byte(0x6A);
                output.add_position(pos.x, pos.y, pos.z);
                output.add_byte(stackpos);
                crate::net::game_protocol::write_item(output, &items_ref, server_id, count);
            });
        }

        self.start_decay(server_id, pos);
    }

    pub fn set_bed_sleeper(&mut self, guid: u32, pos: Position) {
        self.bed_sleepers.insert(guid, pos);
    }

    pub fn remove_bed_sleeper(&mut self, guid: u32) {
        self.bed_sleepers.remove(&guid);
    }

    pub fn get_bed_by_sleeper(&self, guid: u32) -> Option<Position> {
        self.bed_sleepers.get(&guid).copied()
    }

    /// Replace the item with `old_server_id` on `pos` by `new_server_id` and
    /// broadcast a `0x6B` update-tile-thing to spectators. Mirrors the
    /// appearance change in `BedItem::updateAppearance` via `Game::transformItem`.
    pub fn transform_tile_item(&mut self, pos: Position, old_server_id: u16, new_server_id: u16) {
        if new_server_id == 0 {
            return;
        }
        let items_arc = self.items.clone();
        let Some((stackpos, client_id)) = ({
            let Some(tile) = self.map.get_tile_mut(pos) else { return };
            let Some(idx) = tile.find_item_index_by_server_id(old_server_id) else { return };
            let sp = tile.item_client_stackpos(idx);
            tile.items[idx].server_id = new_server_id;
            tile.recalculate_flags(&items_arc);
            let cid = items_arc.get_item_type(new_server_id as usize).client_id;
            Some((sp, cid))
        }) else { return };

        let spectators = self.map.get_spectators(pos, true, true, 0, 0, 0, 0);
        for spec_id in spectators {
            send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
                output.add_byte(0x6B);
                output.add_position(pos.x, pos.y, pos.z);
                output.add_byte(stackpos);
                output.add_u16(client_id);
            });
        }
    }

    pub fn start_decay(&mut self, server_id: u16, position: Position) {
        let it = self.items.get_item_type(server_id as usize);
        if it.decay_to < 0 || it.decay_time == 0 {
            return;
        }

        let duration_ms = (it.decay_time as i32) * 1000;
        if duration_ms > 0 {
            self.to_decay_items.push(DecayEntry {
                server_id,
                position,
                remaining_ms: duration_ms,
            });
        } else {
            self.internal_decay_item(server_id, position);
        }
    }

    pub fn start_decay_with_duration(&mut self, server_id: u16, position: Position, duration_ms: i32) {
        let it = self.items.get_item_type(server_id as usize);
        if it.decay_to < 0 || it.decay_time == 0 {
            return;
        }

        if duration_ms > 0 {
            self.to_decay_items.push(DecayEntry {
                server_id,
                position,
                remaining_ms: duration_ms,
            });
        } else {
            self.internal_decay_item(server_id, position);
        }
    }

    pub fn internal_decay_item(&mut self, server_id: u16, position: Position) {
        let decay_to = self.items.get_item_type(server_id as usize).decay_to;
        if decay_to > 0 {
            self.transform_tile_item(position, server_id, decay_to as u16);
            self.start_decay(decay_to as u16, position);
        } else {
            self.remove_tile_item(position, server_id);
        }
    }

    pub fn remove_tile_item(&mut self, pos: Position, server_id: u16) {
        let Some(stackpos) = ({
            let Some(tile) = self.map.get_tile_mut(pos) else { return };
            let Some(idx) = tile.find_item_index_by_server_id(server_id) else { return };
            tile.remove_item_at(idx).map(|(_, sp)| sp)
        }) else { return };

        let spectators = self.map.get_spectators(pos, true, true, 0, 0, 0, 0);
        for spec_id in spectators {
            send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
                output.add_byte(0x6C);
                output.add_position(pos.x, pos.y, pos.z);
                output.add_byte(stackpos);
            });
        }
    }

    pub fn check_decay(&mut self) {
        let bucket = (self.last_decay_bucket + 1) % (EVENT_DECAY_BUCKETS as usize);

        let mut entries = std::mem::take(&mut self.decay_buckets[bucket]);
        let mut keep = Vec::new();
        for mut entry in entries.drain(..) {
            let it = self.items.get_item_type(entry.server_id as usize);
            if it.decay_to < 0 || it.decay_time == 0 {
                continue;
            }

            if !self.tile_has_item(entry.position, entry.server_id) {
                continue;
            }

            let decrease = (EVENT_DECAYINTERVAL * EVENT_DECAY_BUCKETS).min(entry.remaining_ms);
            entry.remaining_ms -= decrease;

            if entry.remaining_ms <= 0 {
                self.internal_decay_item(entry.server_id, entry.position);
            } else if entry.remaining_ms < EVENT_DECAYINTERVAL * EVENT_DECAY_BUCKETS {
                let new_bucket = (bucket + ((entry.remaining_ms + EVENT_DECAYINTERVAL / 2) / 1000) as usize) % (EVENT_DECAY_BUCKETS as usize);
                if new_bucket == bucket {
                    self.internal_decay_item(entry.server_id, entry.position);
                } else {
                    self.decay_buckets[new_bucket].push(entry);
                }
            } else {
                keep.push(entry);
            }
        }
        self.decay_buckets[bucket] = keep;

        self.last_decay_bucket = bucket;
        self.distribute_staging_decay_items();
    }

    fn distribute_staging_decay_items(&mut self) {
        let staging = std::mem::take(&mut self.to_decay_items);
        for entry in staging {
            let dur = entry.remaining_ms;
            if dur >= EVENT_DECAYINTERVAL * EVENT_DECAY_BUCKETS {
                self.decay_buckets[self.last_decay_bucket].push(entry);
            } else {
                let target = (self.last_decay_bucket + 1 + (dur / 1000) as usize) % (EVENT_DECAY_BUCKETS as usize);
                self.decay_buckets[target].push(entry);
            }
        }
    }

    fn tile_has_item(&self, pos: Position, server_id: u16) -> bool {
        self.map.get_tile(pos)
            .map(|tile| tile.items.iter().any(|it| it.server_id == server_id))
            .unwrap_or(false)
    }

    /// Set/clear the sleeper info on a bed item at `pos` (by current server id).
    pub fn set_item_sleeper(&mut self, pos: Position, server_id: u16, guid: u32, sleep_start: u32, desc: String) {
        if let Some(tile) = self.map.get_tile_mut(pos) {
            if let Some(idx) = tile.find_item_index_by_server_id(server_id) {
                tile.items[idx].sleeper_guid = guid;
                tile.items[idx].sleep_start = sleep_start;
                tile.items[idx].description = desc;
            }
        }
    }

    /// Regenerate a (now online) player for the time they slept, then clear the
    /// regeneration debt. Port of `BedItem::regeneratePlayer`.
    pub fn regenerate_slept_player(&mut self, creature_id: CreatureId, sleep_start: u32) {
        use crate::combat::condition::ConditionType;
        let now = (crate::util::otsys_time() / 1000) as u32;
        let slept_time = now.saturating_sub(sleep_start) as i32;

        let soul_max = self
            .get_player(creature_id)
            .and_then(|p| crate::world::vocation::g_vocations().get_vocation(p.vocation_id).cloned())
            .map(|v| v.soul_max)
            .unwrap_or(100);

        let Some(player) = self.get_player_mut(creature_id) else { return };

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
                } else {
                    if let Some(c) = player.base.get_condition_mut(ConditionType::Regeneration) {
                        c.set_ticks(new_regen_ticks);
                    }
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

    /// Port of `BedItem::wakeUp`: regenerate the sleeper (if online) and clear
    /// the sleeper info + appearance on the bed and its partner bed.
    pub fn wake_bed_at(&mut self, pos: Position, server_id: u16) {
        let (sleeper_guid, sleep_start) = self
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

        if let Some(cid) = self.get_player_id_by_guid(sleeper_guid) {
            self.regenerate_slept_player(cid, sleep_start);
            self.add_creature_health(cid);
        }

        self.remove_bed_sleeper(sleeper_guid);

        let partner_pos = {
            let it = self.items.get_item_type(usize::from(server_id));
            crate::net::game_protocol::next_position(it.bed_partner_dir, pos)
        };
        let free_id = self.items.get_item_type(usize::from(server_id)).transform_to_free;
        self.set_item_sleeper(pos, server_id, 0, 0, "Nobody is sleeping there.".to_string());
        self.transform_tile_item(pos, server_id, free_id);

        let partner_sid = self.map.get_tile(partner_pos).and_then(|t| {
            t.items
                .iter()
                .find(|it| {
                    self.items.get_item_type(usize::from(it.server_id)).kind
                        == crate::items::ItemKind::Bed
                })
                .map(|it| it.server_id)
        });
        if let Some(psid) = partner_sid {
            let p_free = self.items.get_item_type(usize::from(psid)).transform_to_free;
            self.set_item_sleeper(partner_pos, psid, 0, 0, "Nobody is sleeping there.".to_string());
            self.transform_tile_item(partner_pos, psid, p_free);
        }
    }

    /// Move the old owner's movable house items into their depot. Port of
    /// `House::transferToDepot(Player*)` for the online-owner path: pickupable
    /// tile items move whole; non-pickupable containers contribute their
    /// contents. Returns true if a transfer ran. Offline owners are skipped
    /// (no synchronous DB save here).
    pub fn house_transfer_to_depot(&mut self, house_id: u32) -> bool {
        let (town_id, owner) = match self.map.houses.get_house(house_id) {
            Some(h) => (h.town_id, h.owner),
            None => return false,
        };
        if town_id == 0 || owner == 0 {
            return false;
        }
        let Some(cid) = self.get_player_id_by_guid(owner) else { return false };

        let tiles: Vec<Position> = self
            .map
            .houses
            .get_house(house_id)
            .map(|h| h.tiles.clone())
            .unwrap_or_default();

        let mut moved: Vec<crate::map::tile::MapItem> = Vec::new();
        for pos in &tiles {
            let Some(tile) = self.map.get_tile_mut(*pos) else { continue };
            let mut kept: Vec<crate::map::tile::MapItem> = Vec::new();
            for item in std::mem::take(&mut tile.items) {
                let it = self.items.get_item_type(usize::from(item.server_id));
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
            if let Some(tile) = self.map.get_tile_mut(*pos) {
                tile.items = kept;
            }
        }

        if moved.is_empty() {
            return true;
        }
        if let Some(p) = self.get_player_mut(cid) {
            let depot = p.depot_items.entry(town_id).or_default();
            for item in moved {
                depot.insert(0, item);
            }
        }
        true
    }

    /// Port of `House::setOwner` (synchronous parts). When transferring away
    /// from an existing owner: send their movable items to depot, kick players
    /// out to the entry, wake beds, and clear access lists. DB writes
    /// (`houses` row, name/account resolution) are spawned async by the caller.
    pub fn house_set_owner(&mut self, house_id: u32, guid: u32) {
        let (is_loaded, owner, entry, beds, town_id) = match self.map.houses.get_house(house_id) {
            Some(h) => (h.is_loaded, h.owner, h.entry_position, h.beds.clone(), h.town_id),
            None => return,
        };
        let _ = town_id;

        if is_loaded && owner == guid {
            return;
        }
        if let Some(h) = self.map.houses.get_house_mut(house_id) {
            h.is_loaded = true;
        }

        if owner != 0 {
            self.house_transfer_to_depot(house_id);

            // Kick players standing on house tiles (those without CanEditHouses).
            let tiles: Vec<Position> = self
                .map
                .houses
                .get_house(house_id)
                .map(|h| h.tiles.clone())
                .unwrap_or_default();
            let mut to_kick: Vec<CreatureId> = Vec::new();
            for pos in &tiles {
                if let Some(tile) = self.map.get_tile(*pos) {
                    for &cid in &tile.creature_ids {
                        let can_edit = self
                            .get_player(cid)
                            .map(|p| {
                                p.group_flags
                                    & crate::creatures::player::PLAYER_FLAG_CAN_EDIT_HOUSES
                                    != 0
                            })
                            .unwrap_or(true);
                        if !can_edit {
                            to_kick.push(cid);
                        }
                    }
                }
            }
            for cid in to_kick {
                let old_pos = self.get_player(cid).map(|p| p.base.position);
                if let Some(old_pos) = old_pos {
                    self.move_creature_position(cid, old_pos, entry);
                    self.add_magic_effect(old_pos, CONST_ME_POFF);
                    self.add_magic_effect(entry, CONST_ME_TELEPORT);
                }
            }

            // Wake any sleepers.
            for bed_pos in &beds {
                let bed_sid = self.map.get_tile(*bed_pos).and_then(|t| {
                    t.items
                        .iter()
                        .find(|it| {
                            self.items.get_item_type(usize::from(it.server_id)).kind
                                == crate::items::ItemKind::Bed
                        })
                        .map(|it| (it.server_id, it.sleeper_guid))
                });
                if let Some((sid, sleeper)) = bed_sid {
                    if sleeper != 0 {
                        self.wake_bed_at(*bed_pos, sid);
                    }
                }
            }

            if let Some(h) = self.map.houses.get_house_mut(house_id) {
                h.owner = 0;
                h.owner_account_id = 0;
                h.owner_name = String::new();
                h.guest_list.parse_list("");
                h.sub_owner_list.parse_list("");
            }
        }

        if let Some(h) = self.map.houses.get_house_mut(house_id) {
            h.rent_warnings = 0;
            if guid != 0 {
                h.owner = guid;
            }
        }
    }

    // ── Party system (port of party.cpp + Player party-icon sends) ──────────

    /// Shield `observer` should see for `target`. Port of `Player::getPartyShield`.
    pub fn get_party_shield(&self, observer_id: CreatureId, target_id: CreatureId) -> u8 {
        const SHIELD_NONE: u8 = 0;
        const SHIELD_WHITEYELLOW: u8 = 1;
        const SHIELD_WHITEBLUE: u8 = 2;
        const SHIELD_BLUE: u8 = 3;
        const SHIELD_YELLOW: u8 = 4;
        const SHIELD_BLUE_SHAREDEXP: u8 = 5;
        const SHIELD_YELLOW_SHAREDEXP: u8 = 6;
        const SHIELD_BLUE_NOSHAREDEXP_BLINK: u8 = 7;
        const SHIELD_YELLOW_NOSHAREDEXP_BLINK: u8 = 8;
        const SHIELD_BLUE_NOSHAREDEXP: u8 = 9;
        const SHIELD_YELLOW_NOSHAREDEXP: u8 = 10;

        let observer_party = self.get_player(observer_id).and_then(|p| p.party_id);
        let target_party = self.get_player(target_id).and_then(|p| p.party_id);

        if let Some(leader_id) = observer_party {
            let (active, enabled) = self
                .get_party(leader_id)
                .map(|p| (p.is_shared_experience_active(), p.is_shared_experience_enabled()))
                .unwrap_or((false, false));
            if target_id == leader_id {
                if active {
                    if enabled {
                        return SHIELD_YELLOW_SHAREDEXP;
                    }
                    if self.party_can_use_shared_experience(leader_id, target_id) {
                        return SHIELD_YELLOW_NOSHAREDEXP;
                    }
                    return SHIELD_YELLOW_NOSHAREDEXP_BLINK;
                }
                return SHIELD_YELLOW;
            }
            if target_party == Some(leader_id) {
                if active {
                    if enabled {
                        return SHIELD_BLUE_SHAREDEXP;
                    }
                    if self.party_can_use_shared_experience(leader_id, target_id) {
                        return SHIELD_BLUE_NOSHAREDEXP;
                    }
                    return SHIELD_BLUE_NOSHAREDEXP_BLINK;
                }
                return SHIELD_BLUE;
            }
            if observer_id == leader_id
                && self.get_party(leader_id).map(|p| p.is_player_invited(target_id)).unwrap_or(false)
            {
                return SHIELD_WHITEBLUE;
            }
        }

        if let Some(t_leader) = target_party {
            if target_id == t_leader
                && self.get_party(t_leader).map(|p| p.is_player_invited(observer_id)).unwrap_or(false)
            {
                return SHIELD_WHITEYELLOW;
            }
        }
        SHIELD_NONE
    }

    /// Send a `0x91` party-shield update for `about_id` to `to_id`'s client.
    pub fn send_creature_shield(&self, to_id: CreatureId, about_id: CreatureId) {
        let shield = self.get_party_shield(to_id, about_id);
        send_packet_to_player(to_id, move |o: &mut OutputMessage| {
            o.add_byte(0x91);
            o.add_u32(about_id);
            o.add_byte(shield);
        });
    }

    /// Broadcast a player's party shield to all who can see them. Port of
    /// `Game::updatePlayerShield`.
    pub fn update_player_shield(&mut self, player_id: CreatureId) {
        let pos = match self.get_player(player_id) {
            Some(p) => p.base.position,
            None => return,
        };
        for spec_id in self.map.get_spectators(pos, true, true, 0, 0, 0, 0) {
            self.send_creature_shield(spec_id, player_id);
        }
    }

    /// Refresh shields among all party members + leader. Port of
    /// `Party::updateAllPartyIcons`.
    pub fn party_update_all_icons(&self, leader_id: CreatureId) {
        let members = self
            .get_party(leader_id)
            .map(|p| p.get_members().to_vec())
            .unwrap_or_default();
        for &member in &members {
            for &other in &members {
                self.send_creature_shield(member, other);
            }
            self.send_creature_shield(member, leader_id);
            self.send_creature_shield(leader_id, member);
        }
        self.send_creature_shield(leader_id, leader_id);
    }

    /// Send `text` (MESSAGE_INFO_DESCR) to every party member + leader.
    fn party_broadcast_message(&mut self, leader_id: CreatureId, text: &str, to_invitees: bool) {
        let (members, invitees) = match self.get_party(leader_id) {
            Some(p) => (p.get_members().to_vec(), p.get_invitees().to_vec()),
            None => return,
        };
        for member in members {
            self.send_text_message(member, crate::world::raids::MESSAGE_INFO_DESCR, text.to_string());
        }
        self.send_text_message(leader_id, crate::world::raids::MESSAGE_INFO_DESCR, text.to_string());
        if to_invitees {
            for invitee in invitees {
                self.send_text_message(invitee, crate::world::raids::MESSAGE_INFO_DESCR, text.to_string());
            }
        }
    }

    fn player_text(&mut self, player_id: CreatureId, text: &str) {
        self.send_text_message(player_id, crate::world::raids::MESSAGE_INFO_DESCR, text.to_string());
    }

    /// Port of `Party::canUseSharedExperience`.
    pub fn party_can_use_shared_experience(&self, leader_id: CreatureId, player_id: CreatureId) -> bool {
        let members = match self.get_party(leader_id) {
            Some(p) => p.get_members().to_vec(),
            None => return false,
        };
        if members.is_empty() {
            return false;
        }
        let mut highest = self.get_player(leader_id).map(|p| p.get_level()).unwrap_or(0);
        for m in &members {
            if let Some(p) = self.get_player(*m) {
                highest = highest.max(p.get_level());
            }
        }
        let min_level = ((highest as f32 * 2.0) / 3.0).ceil() as u32;
        let player = match self.get_player(player_id) {
            Some(p) => p,
            None => return false,
        };
        if player.get_level() < min_level {
            return false;
        }
        let leader_pos = match self.get_player(leader_id) {
            Some(p) => p.base.position,
            None => return false,
        };
        let ppos = player.base.position;
        let dx = (leader_pos.x as i32 - ppos.x as i32).abs();
        let dy = (leader_pos.y as i32 - ppos.y as i32).abs();
        let dz = (leader_pos.z as i32 - ppos.z as i32).abs();
        if dx > crate::world::party::EXPERIENCE_SHARE_RANGE
            || dy > crate::world::party::EXPERIENCE_SHARE_RANGE
            || dz > crate::world::party::EXPERIENCE_SHARE_FLOORS
        {
            return false;
        }
        if !player.has_flag(crate::creatures::player::PLAYER_FLAG_NOT_GAIN_IN_FIGHT) {
            let last = self.get_party(leader_id).and_then(|p| p.get_player_tick(player_id));
            match last {
                None => return false,
                Some(t) => {
                    let diff = crate::util::otsys_time() - t;
                    if diff > crate::config::g_config().get_number(crate::config::IntegerConfig::PzLocked) as i64 {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Port of `Party::canEnableSharedExperience`.
    pub fn party_can_enable_shared_experience(&self, leader_id: CreatureId) -> bool {
        if !self.party_can_use_shared_experience(leader_id, leader_id) {
            return false;
        }
        let members = self
            .get_party(leader_id)
            .map(|p| p.get_members().to_vec())
            .unwrap_or_default();
        for m in members {
            if !self.party_can_use_shared_experience(leader_id, m) {
                return false;
            }
        }
        true
    }

    /// Port of `Party::updateSharedExperience`.
    pub fn party_update_shared_experience(&mut self, leader_id: CreatureId) {
        let active = self
            .get_party(leader_id)
            .map(|p| p.is_shared_experience_active())
            .unwrap_or(false);
        if !active {
            return;
        }
        let result = self.party_can_enable_shared_experience(leader_id);
        let prev = self
            .get_party(leader_id)
            .map(|p| p.is_shared_experience_enabled())
            .unwrap_or(false);
        if result != prev {
            if let Some(p) = self.get_party_mut(leader_id) {
                p.set_shared_exp_enabled(result);
            }
            self.party_update_all_icons(leader_id);
        }
    }

    /// Port of `Game::playerInviteToParty` + `Party::invitePlayer`.
    pub fn player_invite_to_party(&mut self, player_id: CreatureId, invited_id: CreatureId) {
        if player_id == invited_id {
            return;
        }
        if self.get_player(invited_id).is_none() {
            return;
        }
        if self.get_player(invited_id).and_then(|p| p.party_id).is_some() {
            let name = self.get_player(invited_id).map(|p| p.name.clone()).unwrap_or_default();
            self.player_text(player_id, &format!("{name} is already in a party."));
            return;
        }
        let player_party = self.get_player(player_id).and_then(|p| p.party_id);
        let leader_id = match player_party {
            None => {
                // Create a new party with player as leader.
                self.create_party(player_id);
                player_id
            }
            Some(lid) => {
                if lid != player_id {
                    return; // only the leader may invite
                }
                lid
            }
        };

        let was_empty = self
            .get_party(leader_id)
            .map(|p| p.is_empty())
            .unwrap_or(true);
        if self.get_party(leader_id).map(|p| p.is_player_invited(invited_id)).unwrap_or(false) {
            return;
        }
        if let Some(p) = self.get_party_mut(leader_id) {
            p.invite_ids_mut().push(invited_id);
        }
        if let Some(p) = self.get_player_mut(invited_id) {
            if !p.invite_party_list.contains(&leader_id) {
                p.invite_party_list.push(leader_id);
            }
        }

        let invited_name = self.get_player(invited_id).map(|p| p.name.clone()).unwrap_or_default();
        let leader_name = self.get_player(leader_id).map(|p| p.name.clone()).unwrap_or_default();
        if was_empty {
            self.player_text(leader_id, &format!("{invited_name} has been invited. Open the party channel to communicate with your members."));
            self.update_player_shield(leader_id);
        } else {
            self.player_text(leader_id, &format!("{invited_name} has been invited."));
        }
        self.send_creature_shield(leader_id, invited_id);
        self.send_creature_shield(invited_id, leader_id);
        let his = self.leader_possessive(leader_id);
        self.player_text(invited_id, &format!("{leader_name} has invited you to {his} party."));
    }

    fn leader_possessive(&self, leader_id: CreatureId) -> &'static str {
        use crate::creatures::player::PlayerSex;
        match self.get_player(leader_id).map(|p| p.sex) {
            Some(PlayerSex::Female) => "her",
            _ => "his",
        }
    }

    /// Port of `Game::playerJoinParty` + `Party::joinParty`.
    pub fn player_join_party(&mut self, player_id: CreatureId, leader_id: CreatureId) {
        let invited = self
            .get_party(leader_id)
            .map(|p| p.is_player_invited(player_id))
            .unwrap_or(false);
        if !invited {
            return;
        }
        if self.get_player(player_id).and_then(|p| p.party_id).is_some() {
            self.player_text(player_id, "You are already in a party.");
            return;
        }

        let name = self.get_player(player_id).map(|p| p.name.clone()).unwrap_or_default();
        self.party_broadcast_message(leader_id, &format!("{name} has joined the party."), false);

        if let Some(p) = self.get_party_mut(leader_id) {
            p.invite_ids_mut().retain(|&id| id != player_id);
            p.member_ids_mut().push(player_id);
        }
        if let Some(p) = self.get_player_mut(player_id) {
            p.party_id = Some(leader_id);
            p.invite_party_list.retain(|&id| id != leader_id);
        }
        self.update_player_shield(player_id);
        self.party_update_all_icons(leader_id);
        self.party_update_shared_experience(leader_id);

        let leader_name = self.get_player(leader_id).map(|p| p.name.clone()).unwrap_or_default();
        let suffix = if leader_name.ends_with('s') { "" } else { "s" };
        self.player_text(player_id, &format!("You have joined {leader_name}'{suffix} party. Open the party channel to communicate with your companions."));
    }

    /// Port of `Game::playerRevokePartyInvitation` + `Party::revokeInvitation`.
    pub fn player_revoke_party_invitation(&mut self, player_id: CreatureId, invited_id: CreatureId) {
        let party_leader = self.get_player(player_id).and_then(|p| p.party_id);
        let Some(leader_id) = party_leader else { return };
        if leader_id != player_id {
            return;
        }
        if !self.get_party(leader_id).map(|p| p.is_player_invited(invited_id)).unwrap_or(false) {
            return;
        }
        let invited_name = self.get_player(invited_id).map(|p| p.name.clone()).unwrap_or_default();
        let his = self.leader_possessive(leader_id);
        let leader_name = self.get_player(leader_id).map(|p| p.name.clone()).unwrap_or_default();
        self.player_text(invited_id, &format!("{leader_name} has revoked {his} invitation."));
        self.player_text(leader_id, &format!("Invitation for {invited_name} has been revoked."));
        self.party_remove_invite(leader_id, invited_id);
    }

    /// Port of `Party::removeInvite`.
    fn party_remove_invite(&mut self, leader_id: CreatureId, invited_id: CreatureId) {
        let was_invited = self
            .get_party(leader_id)
            .map(|p| p.is_player_invited(invited_id))
            .unwrap_or(false);
        if !was_invited {
            return;
        }
        if let Some(p) = self.get_party_mut(leader_id) {
            p.invite_ids_mut().retain(|&id| id != invited_id);
        }
        if let Some(p) = self.get_player_mut(invited_id) {
            p.invite_party_list.retain(|&id| id != leader_id);
        }
        self.send_creature_shield(leader_id, invited_id);
        self.send_creature_shield(invited_id, leader_id);
        if self.get_party(leader_id).map(|p| p.is_empty()).unwrap_or(true) {
            self.party_disband(leader_id);
        }
    }

    /// Port of `Game::playerPassPartyLeadership` + `Party::passPartyLeadership`.
    pub fn player_pass_party_leadership(&mut self, player_id: CreatureId, new_leader_id: CreatureId) {
        let party_leader = self.get_player(player_id).and_then(|p| p.party_id);
        let Some(leader_id) = party_leader else { return };
        if leader_id != player_id {
            return;
        }
        let is_member = self
            .get_party(leader_id)
            .map(|p| p.get_members().contains(&new_leader_id))
            .unwrap_or(false);
        if !is_member {
            return;
        }

        let new_name = self.get_player(new_leader_id).map(|p| p.name.clone()).unwrap_or_default();
        self.party_broadcast_message(leader_id, &format!("{new_name} is now the leader of the party."), true);

        // Re-key the party under the new leader, swap member/leader lists.
        if let Some(mut party) = self.parties.remove(&leader_id) {
            party.member_ids_mut().retain(|&id| id != new_leader_id);
            party.member_ids_mut().insert(0, leader_id);
            party.set_leader_id(new_leader_id);
            self.parties.insert(new_leader_id, party);
        }
        // Repoint every member's + old/new leader's party_id to the new leader.
        let members = self
            .get_party(new_leader_id)
            .map(|p| p.get_members().to_vec())
            .unwrap_or_default();
        for m in &members {
            if let Some(p) = self.get_player_mut(*m) {
                p.party_id = Some(new_leader_id);
            }
        }
        if let Some(p) = self.get_player_mut(new_leader_id) {
            p.party_id = Some(new_leader_id);
        }

        self.party_update_shared_experience(new_leader_id);
        self.party_update_all_icons(new_leader_id);
        self.player_text(new_leader_id, "You are now the leader of the party.");
    }

    /// Port of `Game::playerLeaveParty` + `Party::leaveParty`.
    pub fn player_leave_party(&mut self, player_id: CreatureId) {
        let party_leader = self.get_player(player_id).and_then(|p| p.party_id);
        let Some(leader_id) = party_leader else { return };
        let in_fight = self
            .get_player(player_id)
            .map(|p| p.base.has_condition(crate::combat::condition::ConditionType::InFight))
            .unwrap_or(false);
        if in_fight {
            return;
        }

        let mut missing_leader = false;
        if leader_id == player_id {
            let members = self
                .get_party(leader_id)
                .map(|p| p.get_members().to_vec())
                .unwrap_or_default();
            let invitees_empty = self
                .get_party(leader_id)
                .map(|p| p.get_invitees().is_empty())
                .unwrap_or(true);
            if !members.is_empty() {
                if members.len() == 1 && invitees_empty {
                    missing_leader = true;
                } else {
                    self.player_pass_party_leadership(leader_id, members[0]);
                }
            } else {
                missing_leader = true;
            }
        }

        // After a possible leadership pass, the party may be re-keyed.
        let cur_leader = self.get_player(player_id).and_then(|p| p.party_id).unwrap_or(leader_id);
        if let Some(p) = self.get_party_mut(cur_leader) {
            p.member_ids_mut().retain(|&id| id != player_id);
        }
        if let Some(p) = self.get_player_mut(player_id) {
            p.party_id = None;
        }
        self.update_player_shield(player_id);

        let members = self
            .get_party(cur_leader)
            .map(|p| p.get_members().to_vec())
            .unwrap_or_default();
        for m in &members {
            self.send_creature_shield(*m, player_id);
            self.send_creature_shield(player_id, *m);
        }
        self.send_creature_shield(cur_leader, player_id);
        self.send_creature_shield(player_id, player_id);
        self.send_creature_shield(player_id, cur_leader);

        self.player_text(player_id, "You have left the party.");
        self.party_update_shared_experience(cur_leader);
        if let Some(p) = self.get_party_mut(cur_leader) {
            p.clear_player_points(player_id);
        }
        let name = self.get_player(player_id).map(|p| p.name.clone()).unwrap_or_default();
        self.party_broadcast_message(cur_leader, &format!("{name} has left the party."), false);

        let empty = self.get_party(cur_leader).map(|p| p.is_empty()).unwrap_or(true);
        if missing_leader || empty {
            self.party_disband(cur_leader);
        }
    }

    /// Port of `Party::disband`.
    pub fn party_disband(&mut self, leader_id: CreatureId) {
        let (members, invitees) = match self.get_party(leader_id) {
            Some(p) => (p.get_members().to_vec(), p.get_invitees().to_vec()),
            None => return,
        };

        self.player_text(leader_id, "Your party has been disbanded.");
        for &invitee in &invitees {
            if let Some(p) = self.get_player_mut(invitee) {
                p.invite_party_list.retain(|&id| id != leader_id);
            }
            self.send_creature_shield(leader_id, invitee);
        }
        for &member in &members {
            if let Some(p) = self.get_player_mut(member) {
                p.party_id = None;
            }
            self.player_text(member, "Your party has been disbanded.");
        }
        if let Some(p) = self.get_player_mut(leader_id) {
            p.party_id = None;
        }

        self.parties.remove(&leader_id);

        self.update_player_shield(leader_id);
        for &member in &members {
            self.update_player_shield(member);
        }
    }

    /// Port of `Game::playerEnableSharedPartyExperience` + `Party::setSharedExperience`.
    pub fn player_enable_shared_party_experience(&mut self, player_id: CreatureId, active: bool) {
        let party_leader = self.get_player(player_id).and_then(|p| p.party_id);
        let Some(leader_id) = party_leader else { return };
        if leader_id != player_id {
            return; // only the leader toggles shared exp
        }
        let in_fight = self
            .get_player(player_id)
            .map(|p| p.base.has_condition(crate::combat::condition::ConditionType::InFight))
            .unwrap_or(false);
        let in_pz = self
            .get_player(player_id)
            .map(|p| {
                self.map
                    .get_tile(p.base.position)
                    .map(|t| t.has_flag(crate::map::tile::TILESTATE_PROTECTIONZONE))
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if in_fight && !in_pz {
            return;
        }

        let prev_active = self
            .get_party(leader_id)
            .map(|p| p.is_shared_experience_active())
            .unwrap_or(false);
        if prev_active == active {
            return;
        }
        if let Some(p) = self.get_party_mut(leader_id) {
            p.set_shared_exp_active(active);
        }
        if active {
            let enabled = self.party_can_enable_shared_experience(leader_id);
            if let Some(p) = self.get_party_mut(leader_id) {
                p.set_shared_exp_enabled(enabled);
            }
            if enabled {
                self.player_text(leader_id, "Shared Experience is now active.");
            } else {
                self.player_text(leader_id, "Shared Experience has been activated, but some members of your party are inactive.");
            }
        } else {
            self.player_text(leader_id, "Shared Experience has been deactivated.");
        }
        self.party_update_all_icons(leader_id);
    }

    /// Send text message (0xB4) to a single player.
    pub fn send_text_message(&mut self, player_id: CreatureId, msg_type: u8, text: String) {
        let wire_type = crate::net::protocol_version::translate_message_class_to_client(msg_type);
        send_packet_to_player(player_id, move |output: &mut OutputMessage| {
            output.add_byte(0xB4);
            output.add_byte(wire_type);
            output.add_string(text.as_bytes());
        });
    }

    /// Send a combat text message carrying the floating damage/heal numbers.
    ///
    /// On 10.98 the floating number is embedded in the 0xB4 text message
    /// (position + value(s) + color(s), keyed off the DAMAGE_*/HEALED* class).
    /// On 8.60 those classes don't exist on the wire — the float is the
    /// separate 0x84 AnimatedText, so fall back to a plain status message.
    #[allow(clippy::too_many_arguments)]
    fn send_combat_text_message(
        &mut self,
        player_id: CreatureId,
        wire_type: u8,
        pos: Position,
        primary_value: u32,
        primary_color: u8,
        secondary_value: u32,
        secondary_color: u8,
        text: String,
    ) {
        if !crate::net::protocol_version::client_version().is_1098() {
            self.send_text_message(player_id, MESSAGE_STATUS_DEFAULT, text);
            return;
        }
        let has_secondary = matches!(wire_type, 23 | 24 | 27);
        send_packet_to_player(player_id, move |output: &mut OutputMessage| {
            output.add_byte(0xB4);
            output.add_byte(wire_type);
            output.add_position(pos.x, pos.y, pos.z);
            output.add_u32(primary_value);
            output.add_byte(primary_color);
            if has_secondary {
                output.add_u32(secondary_value);
                output.add_byte(secondary_color);
            }
            output.add_string(text.as_bytes());
        });
    }

    // ── Block check ──────────────────────────────────────────────────────────

    /// Returns the block effect to play for a given block type and combat type.
    fn send_block_effect(&mut self, block_type: BlockType, combat_type: CombatType, target_pos: Position) {
        let effect = match block_type {
            BlockType::Defense => Some(CONST_ME_POFF),
            BlockType::Armor => Some(CONST_ME_BLOCKHIT),
            BlockType::Immunity => {
                let e = match combat_type {
                    CombatType::UndefinedDamage => return,
                    CombatType::EnergyDamage
                    | CombatType::FireDamage
                    | CombatType::PhysicalDamage
                    | CombatType::IceDamage
                    | CombatType::DeathDamage => CONST_ME_BLOCKHIT,
                    CombatType::EarthDamage => CONST_ME_GREEN_RINGS,
                    CombatType::HolyDamage => CONST_ME_HOLYDAMAGE,
                    _ => CONST_ME_POFF,
                };
                Some(e)
            }
            BlockType::None => None,
        };
        if let Some(eff) = effect {
            self.add_magic_effect(target_pos, eff);
        }
    }

    /// Mirrors C++ `Game::combatBlockHit`. Returns true if damage is fully blocked.
    #[allow(clippy::too_many_arguments)]
    pub fn combat_block_hit(
        &mut self,
        damage: &mut CombatDamage,
        attacker_id: Option<CreatureId>,
        target_id: CreatureId,
        check_defense: bool,
        check_armor: bool,
        field: bool,
        ignore_resistances: bool,
    ) -> bool {
        if damage.primary_type == CombatType::None && damage.secondary_type == CombatType::None {
            return true;
        }
        {
            let target = match self.creatures.get(&target_id) { Some(c) => c, None => return true };
            if target.is_player() && target.is_in_ghost_mode() {
                return true;
            }
            if damage.primary_value > 0 {
                return false;
            }
        }

        let target_pos = self.creatures.get(&target_id).map(|c| c.position()).unwrap_or_default();

        let primary_block = if damage.primary_type != CombatType::None {
            damage.primary_value = -damage.primary_value;
            let bt = match self.creatures.get_mut(&target_id) {
                Some(c) => c.block_hit(attacker_id, damage.primary_type, &mut damage.primary_value, check_defense, check_armor, field, ignore_resistances),
                None => BlockType::None,
            };
            damage.primary_value = -damage.primary_value;
            self.send_block_effect(bt, damage.primary_type, target_pos);
            bt
        } else {
            BlockType::None
        };

        let secondary_block = if damage.secondary_type != CombatType::None {
            damage.secondary_value = -damage.secondary_value;
            let bt = match self.creatures.get_mut(&target_id) {
                Some(c) => c.block_hit(attacker_id, damage.secondary_type, &mut damage.secondary_value, false, false, field, ignore_resistances),
                None => BlockType::None,
            };
            damage.secondary_value = -damage.secondary_value;
            self.send_block_effect(bt, damage.secondary_type, target_pos);
            bt
        } else {
            BlockType::None
        };

        damage.block_type = primary_block;

        primary_block != BlockType::None && secondary_block != BlockType::None
    }

    /// Returns (text_color, magic_effect) for a combat type on a target creature.
    /// Mirrors C++ `Game::combatGetTypeInfo`.
    fn combat_get_type_info(&self, combat_type: CombatType, target_id: CreatureId) -> (u8, u8) {
        match combat_type {
            CombatType::PhysicalDamage => {
                let race = self.creatures.get(&target_id).map(|c| c.get_race()).unwrap_or(RaceType::None);
                match race {
                    RaceType::Venom => (TEXTCOLOR_LIGHTGREEN, CONST_ME_HITBYPOISON),
                    RaceType::Blood => (TEXTCOLOR_RED, CONST_ME_DRAWBLOOD),
                    RaceType::Undead => (TEXTCOLOR_GREY, CONST_ME_HITAREA),
                    RaceType::Fire => (TEXTCOLOR_ORANGE, CONST_ME_DRAWBLOOD),
                    RaceType::Energy => (TEXTCOLOR_PURPLE, CONST_ME_ENERGYHIT),
                    RaceType::None => (TEXTCOLOR_NONE, 0),
                }
            }
            CombatType::EnergyDamage => (TEXTCOLOR_PURPLE, CONST_ME_ENERGYHIT),
            CombatType::EarthDamage => (TEXTCOLOR_LIGHTGREEN, CONST_ME_GREEN_RINGS),
            CombatType::DrownDamage => (TEXTCOLOR_LIGHTBLUE, CONST_ME_LOSEENERGY),
            CombatType::FireDamage => (TEXTCOLOR_ORANGE, CONST_ME_HITBYFIRE),
            CombatType::IceDamage => (TEXTCOLOR_TEAL, CONST_ME_ICEATTACK),
            CombatType::HolyDamage => (TEXTCOLOR_YELLOW, CONST_ME_HOLYDAMAGE),
            CombatType::DeathDamage => (TEXTCOLOR_DARKRED, CONST_ME_SMALLCLOUDS),
            CombatType::LifeDrain => (TEXTCOLOR_RED, CONST_ME_MAGIC_RED),
            _ => (TEXTCOLOR_NONE, 0),
        }
    }

    // ── Health change ─────────────────────────────────────────────────────────

    /// Mirrors C++ `Game::combatChangeHealth`.
    /// True when `creature_id` has at least one registered creature-event of
    /// `etype`. Used to gate the HEALTHCHANGE/MANACHANGE event dispatch: those
    /// `fire_*` functions re-lock `g_game`, so they must never be entered while
    /// the lock is already held (e.g. from inside `combat_change_health`). When
    /// no such event is registered there is nothing to fire and we can skip the
    /// re-locking path entirely. (Firing with the lock held remains a TODO:
    /// custom healthchange/manachange scripts need the event to run with
    /// `g_game` released, matching the step/death event pattern.)
    fn has_creature_event(&self, creature_id: CreatureId, etype: crate::events::creature::CreatureEventType) -> bool {
        self.get_creature(creature_id)
            .map(|c| !c.base().get_creature_event_names(etype).is_empty())
            .unwrap_or(false)
    }

    pub fn combat_change_health(
        &mut self,
        attacker_id: Option<CreatureId>,
        target_id: CreatureId,
        damage: &mut CombatDamage,
    ) -> bool {
        // Collect needed data up-front (read-only) to avoid split borrow issues.
        let (target_pos, target_health, target_is_player, target_attackable, target_in_ghost) = {
            let Some(t) = self.creatures.get(&target_id) else { return false };
            (t.position(), t.get_health(), t.is_player(), t.is_attackable(), t.is_in_ghost_mode())
        };
        let attacker_is_player = attacker_id.and_then(|id| self.creatures.get(&id)).map(|c| c.is_player()).unwrap_or(false);
        let attacker_skull = attacker_id.and_then(|id| self.creatures.get(&id)).map(|c| c.get_skull()).unwrap_or(Skull::None);
        let attacker_skull_of_target = if let (Some(aid), true) = (attacker_id, target_is_player) {
            let wt = self.get_world_type();
            match (self.get_player(aid), self.get_player(target_id)) {
                (Some(ap), Some(tp)) => ap.get_skull_client_of_player(tp, wt),
                _ => Skull::None,
            }
        } else {
            Skull::None
        };

        // ── Healing branch ────────────────────────────────────────────────────
        if damage.primary_value > 0 {
            if target_health <= 0 { return false; }
            if attacker_is_player && target_is_player
                && attacker_skull == Skull::Black && attacker_skull_of_target == Skull::None {
                return false;
            }

            if damage.origin != CombatOrigin::None
                && self.has_creature_event(target_id, crate::events::creature::CreatureEventType::HealthChange)
                && crate::events::dispatch::fire_health_change_events(target_id, attacker_id, damage)
            {
                damage.origin = CombatOrigin::None;
                return self.combat_change_health(attacker_id, target_id, damage);
            }

            let old_health = target_health;
            let health_max = self.creatures.get(&target_id).map(|c| c.get_max_health()).unwrap_or(1);
            let gained = damage.primary_value.min(health_max - old_health);
            let new_health = old_health + gained;
            if let Some(creature) = self.creatures.get_mut(&target_id) {
                creature.base_mut().health = new_health;
            }
            let real_change = gained;

            if real_change > 0 && !target_in_ghost {
                let damage_string = format!("{} hitpoint{}", real_change, if real_change != 1 { "s" } else { "" });
                self.add_animated_text(target_pos, TEXTCOLOR_MAYABLUE, &real_change.to_string());

                let spectators = self.map.get_spectators(target_pos, false, true, 0, 0, 0, 0);
                let attacker_name = attacker_id.and_then(|id| self.creatures.get(&id)).map(|c| c.get_name_description());
                let target_name = self.creatures.get(&target_id).map(|c| c.get_name_description()).unwrap_or_default();
                let target_sex_is_female = self.creatures.get(&target_id).and_then(|c| c.as_player()).map(|p| p.get_sex() == PlayerSex::Female).unwrap_or(false);

                for spec_id in spectators {
                    let is_attacker = Some(spec_id) == attacker_id;
                    let is_target = spec_id == target_id;
                    let msg = if is_attacker && !is_target {
                        format!("You heal {} for {}.", target_name, damage_string)
                    } else if is_target {
                        match &attacker_name {
                            None => format!("You were healed for {}.", damage_string),
                            Some(an) if attacker_id == Some(target_id) => format!("You healed yourself for {}.", damage_string),
                            Some(an) => format!("You were healed by {} for {}.", an, damage_string),
                        }
                    } else {
                        match &attacker_name {
                            None => format!("{} was healed for {}.", target_name, damage_string),
                            Some(an) if attacker_id == Some(target_id) => {
                                let pron = if target_sex_is_female { "her" } else { "him" };
                                let mut s = format!("{} healed {}self for {}.", an, pron, damage_string);
                                if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                                s
                            }
                            Some(an) => {
                                let mut s = format!("{} healed {} for {}.", an, target_name, damage_string);
                                if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                                s
                            }
                        }
                    };
                    let wire_type = if is_attacker || is_target { 25 } else { 28 };
                    self.send_combat_text_message(spec_id, wire_type, target_pos, real_change as u32, TEXTCOLOR_PASTELRED, 0, TEXTCOLOR_NONE, msg);
                }
            }

            // Creature health bar updates only when health actually changed
            // (C++ Creature::changeHealth → addCreatureHealth on change), but a
            // player target always gets a stats refresh (C++ Player::changeHealth
            // → sendStats() unconditionally).
            if real_change > 0 {
                self.add_creature_health(target_id);
            }
            if target_is_player {
                self.send_player_stats(target_id);
            }
            return true;
        }

        // ── Damage branch ─────────────────────────────────────────────────────
        if !target_attackable {
            if !target_in_ghost {
                self.add_magic_effect(target_pos, CONST_ME_POFF);
            }
            return true;
        }

        if attacker_is_player && target_is_player
            && attacker_skull == Skull::Black && attacker_skull_of_target == Skull::None {
            return false;
        }

        let primary_abs = damage.primary_value.unsigned_abs() as i32;
        let secondary_abs = damage.secondary_value.unsigned_abs() as i32;
        damage.primary_value = -primary_abs;
        damage.secondary_value = -secondary_abs;

        let health_change = primary_abs + secondary_abs;
        if health_change == 0 {
            return true;
        }

        // ── Mana shield ───────────────────────────────────────────────────────
        let spectators = self.map.get_spectators(target_pos, true, true, 0, 0, 0, 0);
        let (target_mana, target_has_manashield) = if target_is_player {
            let p = self.creatures.get(&target_id).and_then(|c| c.as_player());
            let mana = p.map(|p| p.get_mana() as i32).unwrap_or(0);
            let shield = self.creatures.get(&target_id).map(|c| c.base().has_condition(ConditionType::ManaShield)).unwrap_or(false);
            (mana, shield && damage.primary_type != CombatType::UndefinedDamage)
        } else {
            (0, false)
        };

        let mut remaining_damage = health_change;
        if target_has_manashield && target_mana > 0 {
            let mana_damage = target_mana.min(health_change);
            // drain mana
            if let Some(p) = self.creatures.get_mut(&target_id).and_then(|c| c.as_player_mut()) {
                p.mana = (p.mana as i32 - mana_damage).max(0) as u32;
            }
            self.add_magic_effect(target_pos, CONST_ME_LOSEENERGY);
            self.add_animated_text(target_pos, TEXTCOLOR_BLUE, &mana_damage.to_string());

            let attacker_name = attacker_id.and_then(|id| self.creatures.get(&id)).map(|c| c.get_name_description());
            let target_name = self.creatures.get(&target_id).map(|c| c.get_name_description()).unwrap_or_default();
            let target_sex_is_female = self.creatures.get(&target_id).and_then(|c| c.as_player()).map(|p| p.get_sex() == PlayerSex::Female).unwrap_or(false);

            for &spec_id in &spectators {
                if self.creatures.get(&spec_id).map(|c| c.position().z != target_pos.z).unwrap_or(true) { continue; }
                let is_attacker = Some(spec_id) == attacker_id;
                let is_target = spec_id == target_id;
                let msg = if is_attacker && !is_target {
                    let mut s = format!("{} loses {} mana due to your attack.", target_name, mana_damage);
                    if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                    s
                } else if is_target {
                    match &attacker_name {
                        None => format!("You lose {} mana.", mana_damage),
                        Some(an) if attacker_id == Some(target_id) => format!("You lose {} mana due to your own attack.", mana_damage),
                        Some(an) => format!("You lose {} mana due to an attack by {}.", mana_damage, an),
                    }
                } else {
                    match &attacker_name {
                        None => {
                            let mut s = format!("{} loses {} mana.", target_name, mana_damage);
                            if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                            s
                        }
                        Some(an) if attacker_id == Some(target_id) => {
                            let pron = if target_sex_is_female { "her" } else { "his" };
                            let mut s = format!("{} loses {} mana due to {} own attack.", target_name, mana_damage, pron);
                            if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                            s
                        }
                        Some(an) => {
                            let mut s = format!("{} loses {} mana due to an attack by {}.", target_name, mana_damage, an);
                            if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                            s
                        }
                    }
                };
                let wire_type = if is_attacker && !is_target { 23 } else if is_target { 24 } else { 27 };
                self.send_combat_text_message(spec_id, wire_type, target_pos, mana_damage as u32, TEXTCOLOR_BLUE, 0, TEXTCOLOR_NONE, msg);
            }

            // reduce from damage
            damage.primary_value += mana_damage;
            if damage.primary_value > 0 {
                damage.secondary_value = (damage.secondary_value + damage.primary_value).min(0);
                damage.primary_value = 0;
            }
            // re-abs after mana reduction
            let pa = damage.primary_value.unsigned_abs() as i32;
            let sa = damage.secondary_value.unsigned_abs() as i32;
            remaining_damage = pa + sa;
        }

        if remaining_damage == 0 { return true; }

        if damage.origin != CombatOrigin::None
            && self.has_creature_event(target_id, crate::events::creature::CreatureEventType::HealthChange)
            && crate::events::dispatch::fire_health_change_events(target_id, attacker_id, damage)
        {
            damage.origin = CombatOrigin::None;
            return self.combat_change_health(attacker_id, target_id, damage);
        }

        // Clamp damage to actual health
        let cur_health = self.creatures.get(&target_id).map(|c| c.get_health()).unwrap_or(0);
        let pa = damage.primary_value.unsigned_abs() as i32;
        let sa = damage.secondary_value.unsigned_abs() as i32;
        let (clamped_primary, clamped_secondary) = if pa >= cur_health {
            (cur_health, 0i32)
        } else {
            (pa, sa.min(cur_health - pa))
        };
        let real_damage = clamped_primary + clamped_secondary;
        if real_damage == 0 { return true; }

        // Visual: magic effect + animated text per damage type
        let (primary_color, primary_effect) = self.combat_get_type_info(damage.primary_type, target_id);
        let (secondary_color, secondary_effect) = self.combat_get_type_info(damage.secondary_type, target_id);

        if clamped_primary > 0 {
            if primary_effect != 0 {
                for &spec_id in &spectators {
                    let eff = primary_effect;
                    let pos = target_pos;
                    send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
                        output.add_byte(0x83);
                        output.add_position(pos.x, pos.y, pos.z);
                        output.add_byte(eff);
                    });
                }
            }
            if primary_color != TEXTCOLOR_NONE {
                self.add_animated_text(target_pos, primary_color, &clamped_primary.to_string());
            }
        }

        if clamped_secondary > 0 {
            if secondary_effect != 0 {
                for &spec_id in &spectators {
                    let eff = secondary_effect;
                    let pos = target_pos;
                    send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
                        output.add_byte(0x83);
                        output.add_position(pos.x, pos.y, pos.z);
                        output.add_byte(eff);
                    });
                }
            }
            if secondary_color != TEXTCOLOR_NONE {
                self.add_animated_text(target_pos, secondary_color, &clamped_secondary.to_string());
            }
        }

        // Text messages to spectators
        if primary_color != TEXTCOLOR_NONE || secondary_color != TEXTCOLOR_NONE {
            let damage_string = format!("{} hitpoint{}", real_damage, if real_damage != 1 { "s" } else { "" });
            let attacker_name = attacker_id.and_then(|id| self.creatures.get(&id)).map(|c| c.get_name_description());
            let target_name = self.creatures.get(&target_id).map(|c| c.get_name_description()).unwrap_or_default();
            let target_sex_is_female = self.creatures.get(&target_id).and_then(|c| c.as_player()).map(|p| p.get_sex() == PlayerSex::Female).unwrap_or(false);

            for &spec_id in &spectators {
                if self.creatures.get(&spec_id).map(|c| c.position().z != target_pos.z).unwrap_or(true) { continue; }
                let is_attacker = Some(spec_id) == attacker_id;
                let is_target = spec_id == target_id;
                let msg = if is_attacker && !is_target {
                    let mut s = format!("{} loses {} due to your attack.", target_name, damage_string);
                    if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                    s
                } else if is_target {
                    match &attacker_name {
                        None => format!("You lose {}.", damage_string),
                        Some(_) if attacker_id == Some(target_id) => format!("You lose {} due to your own attack.", damage_string),
                        Some(an) => format!("You lose {} due to an attack by {}.", damage_string, an),
                    }
                } else {
                    match &attacker_name {
                        None => {
                            let mut s = format!("{} loses {}.", target_name, damage_string);
                            if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                            s
                        }
                        Some(an) if attacker_id == Some(target_id) => {
                            let its = if target_is_player {
                                if target_sex_is_female { "her" } else { "his" }
                            } else { "its" };
                            let mut s = format!("{} loses {} due to {} own attack.", target_name, damage_string, its);
                            if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                            s
                        }
                        Some(an) => {
                            let mut s = format!("{} loses {} due to an attack by {}.", target_name, damage_string, an);
                            if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                            s
                        }
                    }
                };
                let wire_type = if is_attacker && !is_target { 23 } else if is_target { 24 } else { 27 };
                let p_color = if clamped_primary > 0 { primary_color } else { TEXTCOLOR_NONE };
                let s_color = if clamped_secondary > 0 { secondary_color } else { TEXTCOLOR_NONE };
                self.send_combat_text_message(spec_id, wire_type, target_pos, clamped_primary as u32, p_color, clamped_secondary as u32, s_color, msg);
            }
        }

        if real_damage >= cur_health
            && self.has_creature_event(target_id, crate::events::creature::CreatureEventType::PrepareDeath)
            && !crate::events::dispatch::fire_prepare_death_events(target_id, attacker_id)
        {
            return false;
        }

        // Apply damage
        let new_health = (cur_health - real_damage).max(0);
        if let Some(creature) = self.creatures.get_mut(&target_id) {
            creature.base_mut().health = new_health;
        }

        // Track damage for experience distribution and kill attribution.
        if let (Some(aid), true) = (attacker_id, real_damage > 0) {
            if aid != target_id {
                if let Some(creature) = self.creatures.get_mut(&target_id) {
                    creature.base_mut().add_damage_points(aid, real_damage);
                    creature.base_mut().last_hit_creature_id = aid;
                }

                // onAttackedCreature + onAttacked — mirrors C++ player.cpp:3347-3406 verbatim.
                let attacker_is_player_here = self.get_player(aid).is_some();
                if attacker_is_player_here {
                    // target->getZone() == ZONE_PVP → skip skull logic entirely
                    let target_in_pvp = self.get_creature(target_id)
                        .and_then(|c| self.map.get_tile(c.position()))
                        .map(|t| t.has_flag(crate::map::tile::TILESTATE_PVPZONE))
                        .unwrap_or(false);

                    let not_gain_in_fight = self.get_player(aid)
                        .map(|p| p.has_flag(crate::creatures::player::PLAYER_FLAG_NOT_GAIN_IN_FIGHT))
                        .unwrap_or(false);

                    if target_is_player && !target_in_pvp && !not_gain_in_fight {
                        // Check party/guild exemption
                        let is_partner = {
                            let ap = self.get_player(aid);
                            let tp = self.get_player(target_id);
                            match (ap, tp) {
                                (Some(a), Some(t)) => {
                                    (a.party_id.is_some() && a.party_id == t.party_id)
                                    || (a.guild_id.is_some() && a.guild_id == t.guild_id)
                                }
                                _ => false,
                            }
                        };

                        if !is_partner {
                            // PVP_ENFORCED: immediate pzLock
                            if self.get_world_type() == crate::game::WorldType::PvpEnforced {
                                let was_locked = self.get_player(aid).map(|p| p.pz_locked).unwrap_or(true);
                                if !was_locked {
                                    if let Some(ap) = self.get_player_mut(aid) {
                                        ap.pz_locked = true;
                                    }
                                    crate::net::game_protocol::send_icons_to_player(aid);
                                }
                            }

                            // Target gets InFight
                            if let Some(tp) = self.get_player_mut(target_id) {
                                tp.add_in_fight_ticks(false);
                            }

                            let (attacker_skull, attacker_skull_of_target, target_skull, target_has_attacked) = {
                                let ap = self.get_player(aid);
                                let tp = self.get_player(target_id);
                                match (ap, tp) {
                                    (Some(a), Some(t)) => {
                                        let wt = self.get_world_type();
                                        (
                                            a.base.skull, a.get_skull_client_of_player(t, wt),
                                            t.base.skull, t.has_attacked(a.guid),
                                        )
                                    }
                                    _ => (Skull::None, Skull::None, Skull::None, false),
                                }
                            };

                            let both_in_pvp_zone = {
                                let ap_pvp = self.get_creature(aid)
                                    .and_then(|c| self.map.get_tile(c.position()))
                                    .map(|t| t.has_flag(crate::map::tile::TILESTATE_PVPZONE))
                                    .unwrap_or(false);
                                let tp_pvp = self.get_creature(target_id)
                                    .and_then(|c| self.map.get_tile(c.position()))
                                    .map(|t| t.has_flag(crate::map::tile::TILESTATE_PVPZONE))
                                    .unwrap_or(false);
                                ap_pvp && tp_pvp
                            };

                            let target_guid = self.get_player(target_id).map(|p| p.guid).unwrap_or(0);

                            if attacker_skull == Skull::None && attacker_skull_of_target == Skull::Yellow {
                                // Yellow skull path: reciprocal attack recording
                                if let Some(ap) = self.get_player_mut(aid) {
                                    ap.attacked_set.insert(target_guid);
                                }
                                crate::net::game_protocol::send_creature_skull_to_player(target_id, aid, Skull::None);
                            } else if !target_has_attacked {
                                // Target hasn't attacked us → pzLock + possible white skull
                                let was_locked = self.get_player(aid).map(|p| p.pz_locked).unwrap_or(true);
                                if !was_locked {
                                    if let Some(ap) = self.get_player_mut(aid) {
                                        ap.pz_locked = true;
                                    }
                                    crate::net::game_protocol::send_icons_to_player(aid);
                                }

                                if !both_in_pvp_zone {
                                    if let Some(ap) = self.get_player_mut(aid) {
                                        ap.attacked_set.insert(target_guid);
                                    }
                                    if target_skull == Skull::None && attacker_skull == Skull::None {
                                        if let Some(ap) = self.get_player_mut(aid) {
                                            ap.base.skull = Skull::White;
                                        }
                                    }
                                    // Send skull to target (only if attacker has no skull)
                                    let current_skull = self.get_player(aid).map(|p| p.base.skull).unwrap_or(Skull::None);
                                    if current_skull == Skull::None {
                                        crate::net::game_protocol::send_creature_skull_to_player(target_id, aid, Skull::None);
                                    } else {
                                        crate::net::game_protocol::send_creature_skull_to_player(target_id, aid, current_skull);
                                    }
                                }
                            }
                        }
                    }

                    // Attacker gets InFight (pzlock for PvP targets)
                    if let Some(p) = self.get_player_mut(aid) {
                        p.add_in_fight_ticks(target_is_player && !target_in_pvp);
                    }
                } else if target_is_player {
                    // Non-player attacker (monster) → target gets InFight (onAttacked)
                    if let Some(p) = self.get_player_mut(target_id) {
                        p.add_in_fight_ticks(false);
                    }
                }

                // Party in-fight ticks (port of Party::updatePlayerTicks via
                // Player::onAttackedCreature) — feeds shared-experience gating.
                if let Some(leader_id) = self.get_player(aid).and_then(|p| p.party_id) {
                    let not_in_fight = self
                        .get_player(aid)
                        .map(|p| p.has_flag(crate::creatures::player::PLAYER_FLAG_NOT_GAIN_IN_FIGHT))
                        .unwrap_or(false);
                    if !not_in_fight {
                        if let Some(party) = self.get_party_mut(leader_id) {
                            party.update_player_ticks(aid, real_damage as u32);
                        }
                        self.party_update_shared_experience(leader_id);
                    }
                }
            }
        }

        // Broadcast updated health bar
        self.add_creature_health(target_id);

        // Schedule death if health hit zero
        if new_health <= 0 {
            let tid = target_id;
            g_dispatcher().add_task(Task::new(move || {
                if let Ok(mut game) = crate::game::g_game().lock() {
                    game.execute_death(tid);
                }
            }));
        }

        true
    }

    /// Mirrors C++ `Game::combatChangeMana`.
    pub fn combat_change_mana(
        &mut self,
        attacker_id: Option<CreatureId>,
        target_id: CreatureId,
        damage: &mut CombatDamage,
    ) -> bool {
        let target_is_player = self.creatures.get(&target_id).map(|c| c.is_player()).unwrap_or(false);
        if !target_is_player { return true; }

        let mana_change = damage.primary_value + damage.secondary_value;
        if mana_change > 0 {
            // healing mana
            let attacker_skull = attacker_id.and_then(|id| self.creatures.get(&id)).map(|c| c.get_skull()).unwrap_or(Skull::None);
            let attacker_skull_of_target = Skull::None;
            if attacker_skull == Skull::Black && attacker_skull_of_target == Skull::None {
                return false;
            }
            if damage.origin != CombatOrigin::None
                && self.has_creature_event(target_id, crate::events::creature::CreatureEventType::ManaChange)
                && crate::events::dispatch::fire_mana_change_events(target_id, attacker_id, damage)
            {
                damage.origin = CombatOrigin::None;
                return self.combat_change_mana(attacker_id, target_id, damage);
            }
            if let Some(p) = self.creatures.get_mut(&target_id).and_then(|c| c.as_player_mut()) {
                p.mana = (p.mana as i32 + mana_change).min(p.mana_max as i32).max(0) as u32;
            }
        } else {
            let target_pos = self.creatures.get(&target_id).map(|c| c.position()).unwrap_or_default();
            let target_attackable = self.creatures.get(&target_id).map(|c| c.is_attackable()).unwrap_or(false);
            let target_in_ghost = self.creatures.get(&target_id).map(|c| c.is_in_ghost_mode()).unwrap_or(false);
            if !target_attackable {
                if !target_in_ghost { self.add_magic_effect(target_pos, CONST_ME_POFF); }
                return false;
            }
            let attacker_skull = attacker_id.and_then(|id| self.creatures.get(&id)).map(|c| c.get_skull()).unwrap_or(Skull::None);
            if attacker_skull == Skull::Black { return false; }

            let target_mana = self.creatures.get(&target_id).and_then(|c| c.as_player()).map(|p| p.get_mana() as i32).unwrap_or(0);
            let mana_loss = target_mana.min(-mana_change);
            // block check
            let bt = match self.creatures.get_mut(&target_id) {
                Some(c) => c.block_hit(attacker_id, CombatType::ManaDrain, &mut { mana_loss }, false, false, false, false),
                None => BlockType::None,
            };
            if bt != BlockType::None {
                self.add_magic_effect(target_pos, CONST_ME_POFF);
                return false;
            }
            if mana_loss <= 0 { return true; }

            if damage.origin != CombatOrigin::None
                && self.has_creature_event(target_id, crate::events::creature::CreatureEventType::ManaChange)
                && crate::events::dispatch::fire_mana_change_events(target_id, attacker_id, damage)
            {
                damage.origin = CombatOrigin::None;
                return self.combat_change_mana(attacker_id, target_id, damage);
            }
            if let Some(p) = self.creatures.get_mut(&target_id).and_then(|c| c.as_player_mut()) {
                p.mana = (p.mana as i32 - mana_loss).max(0) as u32;
            }
            self.add_animated_text(target_pos, TEXTCOLOR_BLUE, &mana_loss.to_string());

            let target_name = self.creatures.get(&target_id).map(|c| c.get_name_description()).unwrap_or_default();
            let attacker_name = attacker_id.and_then(|id| self.creatures.get(&id)).map(|c| c.get_name_description());
            let target_sex_is_female = self.creatures.get(&target_id).and_then(|c| c.as_player()).map(|p| p.get_sex() == PlayerSex::Female).unwrap_or(false);

            let spectators = self.map.get_spectators(target_pos, false, true, 0, 0, 0, 0);
            for spec_id in spectators {
                let is_attacker = Some(spec_id) == attacker_id;
                let is_target = spec_id == target_id;
                let msg = if is_attacker && !is_target {
                    let mut s = format!("{} loses {} mana due to your attack.", target_name, mana_loss);
                    if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                    s
                } else if is_target {
                    match &attacker_name {
                        None => format!("You lose {} mana.", mana_loss),
                        Some(_) if attacker_id == Some(target_id) => format!("You lose {} mana due to your own attack.", mana_loss),
                        Some(an) => format!("You lose {} mana due to an attack by {}.", mana_loss, an),
                    }
                } else {
                    match &attacker_name {
                        None => {
                            let mut s = format!("{} loses {} mana.", target_name, mana_loss);
                            if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                            s
                        }
                        Some(an) if attacker_id == Some(target_id) => {
                            let pron = if target_sex_is_female { "her" } else { "his" };
                            let mut s = format!("{} loses {} mana due to {} own attack.", target_name, mana_loss, pron);
                            if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                            s
                        }
                        Some(an) => {
                            let mut s = format!("{} loses {} mana due to an attack by {}.", target_name, mana_loss, an);
                            if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                            s
                        }
                    }
                };
                self.send_text_message(spec_id, MESSAGE_STATUS_DEFAULT, msg);
            }
        }
        true
    }

    // ── Death handling ────────────────────────────────────────────────────────

    /// Mirrors C++ `Game::executeDeath`. Called by dispatcher after health hits 0.
    pub fn execute_death(&mut self, creature_id: CreatureId) {
        if !self.creatures.contains_key(&creature_id) {
            return;
        }

        // Collect damage map + identity info before mutating.
        let pz_locked_ms = crate::config::g_config()
            .get_number(crate::config::IntegerConfig::PzLocked) as i64;
        let now_ms = crate::util::otsys_time();
        let (is_player, last_hit_id, damage_entries, pos, lost_exp, most_damage_id, player_look_corpse) = {
            let Some(creature) = self.creatures.get(&creature_id) else { return };
            let base = creature.base();
            let last_hit_id = if base.last_hit_creature_id != 0 { Some(base.last_hit_creature_id) } else { None };
            let damage_entries: Vec<(CreatureId, i32)> = base.damage_map.iter()
                .map(|(&id, cb)| (id, cb.total))
                .collect();
            let pos = creature.position();
            let lost_exp: u64 = match creature {
                Creature::Monster(m) => {
                    if m.base.skill_loss { m.mtype_info.experience } else { 0 }
                }
                _ => 0,
            };
            let is_player = creature.is_player();
            let player_look_corpse: u16 = match creature {
                Creature::Player(p) => {
                    if p.sex == crate::creatures::player::PlayerSex::Female { 3065 } else { 3058 }
                }
                _ => 0,
            };
            // Determine most-damage attacker (C++ Creature::onDeath logic).
            let most_damage_id: CreatureId = {
                let mut best_total = 0i32;
                let mut best_id = 0u32;
                for (&attacker_id, cb) in &base.damage_map {
                    if cb.total > best_total && (now_ms - cb.ticks) <= pz_locked_ms {
                        best_total = cb.total;
                        best_id = attacker_id;
                    }
                }
                best_id
            };
            (is_player, last_hit_id, damage_entries, pos, lost_exp, most_damage_id, player_look_corpse)
        };

        // Collect creature Lua event script IDs while creature still in game.
        // These are fired in a deferred task (after game lock is released) to avoid deadlock.
        // Pairs are (script_id, from_lua) — from_lua=true means id is in lua_callbacks,
        // false means it's in the CreatureEvents script interface's event table.
        use crate::events::creature::CreatureEventType;
        let collect_script_info = |base: &crate::creatures::CreatureBase, etype: CreatureEventType| -> Vec<(i32, bool)> {
            use crate::events::registry::g_script_registry;
            let names = base.get_creature_event_names(etype);
            if names.is_empty() { return Vec::new(); }
            let registry = g_script_registry().lock().unwrap();
            names.iter()
                .filter_map(|n| registry.creature_events.get_event_by_name(n, true).map(|e| (e.script_id, e.from_lua)))
                .collect()
        };

        let kill_script_ids_last: Vec<(i32, bool)> = last_hit_id
            .and_then(|id| self.get_creature(id))
            .map(|c| collect_script_info(c.base(), CreatureEventType::Kill))
            .unwrap_or_default();

        let kill_script_ids_most: Vec<(i32, bool)> = if most_damage_id != 0
            && most_damage_id != last_hit_id.unwrap_or(0)
        {
            self.get_creature(most_damage_id)
                .map(|c| collect_script_info(c.base(), CreatureEventType::Kill))
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let death_script_ids: Vec<(i32, bool)> = self.get_creature(creature_id)
            .map(|c| collect_script_info(c.base(), CreatureEventType::Death))
            .unwrap_or_default();

        let last_hit_copy = last_hit_id.unwrap_or(0);
        let most_dmg_copy = most_damage_id;
        let creature_id_copy = creature_id;

        // ── Experience distribution ──────────────────────────────────────────
        // Raw exp per attacker is floor(damageRatio * lostExperience); the
        // experience-stage and stamina multipliers are applied by the Lua
        // `Player:onGainExperience` event, which must run with the game lock
        // released, so the whole step is deferred to a dispatcher task.
        if lost_exp > 0 && !damage_entries.is_empty() {
            let total_damage: i32 = damage_entries.iter().map(|(_, d)| d).sum();
            if total_damage > 0 {
                let pos_copy = pos;
                let source_id = creature_id;
                let mut distribution: Vec<(CreatureId, u64)> = Vec::new();
                for (attacker_id, attacker_damage) in &damage_entries {
                    let gained = ((*attacker_damage as f64 / total_damage as f64) * lost_exp as f64).floor() as u64;
                    if gained == 0 { continue; }
                    distribution.push((*attacker_id, gained));
                }
                if !distribution.is_empty() {
                    crate::runtime::g_dispatcher().add_task(
                        crate::runtime::dispatcher::Task::new(move || {
                            for (attacker_id, raw_gained) in distribution {
                                let modified = crate::events::g_events().lock().unwrap()
                                    .event_player_on_gain_experience(
                                        attacker_id, Some(source_id), "Monster", raw_gained, raw_gained,
                                    );
                                if modified == 0 { continue; }

                                // Party shared experience: if the attacker is in
                                // a party with shared exp active+enabled, the gain
                                // is shared among all members instead of given
                                // directly (port of Player::onGainExperience).
                                let shared_party = {
                                    let game = crate::game::g_game().lock().unwrap();
                                    game.get_player(attacker_id).and_then(|p| p.party_id).filter(|&lid| {
                                        game.get_party(lid)
                                            .map(|pt| pt.is_shared_experience_active() && pt.is_shared_experience_enabled())
                                            .unwrap_or(false)
                                    })
                                };

                                if let Some(leader_id) = shared_party {
                                    // Lua Party:onShareExperience runs with the lock released.
                                    let shared = crate::events::g_events().lock().unwrap()
                                        .event_party_on_share_experience(leader_id, modified);
                                    let recipients = {
                                        let game = crate::game::g_game().lock().unwrap();
                                        let mut r = game.get_party(leader_id)
                                            .map(|pt| pt.get_members().to_vec())
                                            .unwrap_or_default();
                                        r.push(leader_id);
                                        r
                                    };
                                    if let Ok(mut game) = crate::game::g_game().lock() {
                                        for member in recipients {
                                            game.give_experience(member, shared, pos_copy);
                                        }
                                    }
                                } else if let Ok(mut game) = crate::game::g_game().lock() {
                                    game.give_experience(attacker_id, modified, pos_copy);
                                }
                            }
                        }),
                    );
                }
            }
        }

        // ── Player death ─────────────────────────────────────────────────────
        if is_player {
            // Place player corpse on the death tile before teleporting.
            let (corpse_server_id, corpse_pos, corpse_tile_idx) = if player_look_corpse > 0 {
                let corpse_item = crate::map::tile::MapItem {
                    server_id: player_look_corpse,
                    owner_id: creature_id,
                    ..crate::map::tile::MapItem::default()
                };
                let items_arc = self.items.clone();
                if let Some(tile) = self.map.get_tile_mut(pos) {
                    tile.internal_add_item(corpse_item, &items_arc);
                    (player_look_corpse, pos, 0i32)
                } else {
                    (0u16, pos, -1i32)
                }
            } else {
                (0u16, pos, -1i32)
            };
            if corpse_server_id > 0 {
                self.start_decay(corpse_server_id, corpse_pos);
            }
            let corpse_server_id_copy = corpse_server_id;
            let corpse_pos_copy = corpse_pos;
            let corpse_tile_idx_copy = corpse_tile_idx;

            self.execute_player_death(creature_id, last_hit_id);

            // Dispatch player onDeath events (with corpse) and killer onKill events.
            let need_dispatch = !death_script_ids.is_empty()
                || !kill_script_ids_last.is_empty()
                || !kill_script_ids_most.is_empty();
            if need_dispatch {
                crate::runtime::g_dispatcher().add_task(
                    crate::runtime::dispatcher::Task::new(move || {
                        for (sid, from_lua) in &death_script_ids {
                            crate::events::dispatch::fire_death_script(*sid, *from_lua, creature_id_copy, last_hit_copy, most_dmg_copy, corpse_server_id_copy, corpse_pos_copy, corpse_tile_idx_copy);
                        }
                        for (sid, from_lua) in &kill_script_ids_last {
                            crate::events::dispatch::fire_kill_script(*sid, *from_lua, last_hit_copy, creature_id_copy);
                        }
                        for (sid, from_lua) in &kill_script_ids_most {
                            crate::events::dispatch::fire_kill_script(*sid, *from_lua, most_dmg_copy, creature_id_copy);
                        }
                    }),
                );
            }
        } else {
            // Collect corpse item id and loot before removing the creature.
            let (look_corpse, loot_items, loot_drop, skill_loss) = {
                let Some(m) = self.creatures.get(&creature_id).and_then(|c| c.as_monster()) else {
                    self.remove_creature_from_world(creature_id);
                    return;
                };
                (m.mtype_info.look_corpse, m.mtype_info.loot_items.clone(), m.base.loot_drop, m.base.skill_loss)
            };

            // Notify spawn system so monster slot becomes available for respawn.
            let now_ms = crate::util::otsys_time();
            for spawn in &mut self.spawns.spawn_list {
                if spawn.spawned_map.values().any(|&cid| cid == creature_id) {
                    spawn.spawned_map.retain(|_, &mut cid| cid != creature_id);
                    // Mark last_spawn time for respawn interval tracking.
                    for sb in spawn.spawn_map.values_mut() {
                        if sb.last_spawn == 0 {
                            sb.last_spawn = now_ms;
                        }
                    }
                    break;
                }
            }

            self.remove_creature_from_world(creature_id);

            // Place corpse with loot (0 = no corpse defined → POFF effect).
            if look_corpse > 0 {
                let mut corpse = crate::map::tile::MapItem { server_id: look_corpse, ..crate::map::tile::MapItem::default() };
                let killer_suppresses_loot = last_hit_id
                    .and_then(|id| self.get_player(id))
                    .map(|p| p.has_flag(crate::creatures::player::PLAYER_FLAG_NOT_GENERATE_LOOT))
                    .unwrap_or(false);
                if loot_drop && skill_loss && !killer_suppresses_loot {
                    let rate_loot = crate::config::g_config().get_number(crate::config::IntegerConfig::RateLoot);
                    if rate_loot > 0 {
                        for lb in &loot_items {
                            generate_loot_recursive(lb, &mut corpse.children);
                        }
                    }
                }
                // Send "Loot of X: ..." message to the killing player.
                if !corpse.children.is_empty() {
                    if let Some(attacker_id) = last_hit_id {
                        if self.get_player(attacker_id).is_some() {
                            let monster_name = self.creatures.get(&creature_id)
                                .and_then(|c| c.as_monster())
                                .map(|m| m.get_name().to_string())
                                .unwrap_or_default();
                            let loot_desc = describe_loot_items(&corpse.children, self);
                            let msg = format!("Loot of a {}: {}.", monster_name, loot_desc);
                            self.send_text_message(attacker_id, MESSAGE_INFO_DESCR, msg);
                        }
                    }
                }
                self.place_map_item_on_tile(pos, corpse);
            } else {
                self.add_magic_effect(pos, CONST_ME_POFF);
            }

            // Dispatch monster onDeath + killer onKill Lua events.
            let need_dispatch = !kill_script_ids_last.is_empty()
                || !kill_script_ids_most.is_empty()
                || !death_script_ids.is_empty();
            if need_dispatch {
                crate::runtime::g_dispatcher().add_task(
                    crate::runtime::dispatcher::Task::new(move || {
                        for (sid, from_lua) in &kill_script_ids_last {
                            crate::events::dispatch::fire_kill_script(*sid, *from_lua, last_hit_copy, creature_id_copy);
                        }
                        for (sid, from_lua) in &kill_script_ids_most {
                            crate::events::dispatch::fire_kill_script(*sid, *from_lua, most_dmg_copy, creature_id_copy);
                        }
                        for (sid, from_lua) in &death_script_ids {
                            crate::events::dispatch::fire_death_script(*sid, *from_lua, creature_id_copy, last_hit_copy, most_dmg_copy, 0, crate::map::Position::default(), -1);
                        }
                    }),
                );
            }
        }
    }

    /// Give experience to a player attacker and broadcast the text/stats.
    fn give_experience(&mut self, attacker_id: CreatureId, gained: u64, target_pos: Position) {
        use crate::world::vocation::g_vocations;

        let (level, exp_before, health_max, mana_max, cap, voc_id) = {
            let Some(player) = self.get_player(attacker_id) else { return };
            if player.has_flag(crate::creatures::player::PLAYER_FLAG_NOT_GAIN_EXPERIENCE) || gained == 0 {
                return;
            }
            (player.level, player.experience, player.base.health_max, player.mana_max, player.capacity, player.vocation_id)
        };

        let new_exp = exp_before + gained;

        // Check for level-up(s).
        let mut new_level = level;
        let mut new_health_max = health_max;
        let mut new_mana_max = mana_max;
        let mut new_cap = cap;

        let voc = g_vocations().get_vocation(voc_id).cloned();
        loop {
            let needed = Player::get_exp_for_level((new_level + 1) as u64);
            if new_exp < needed { break; }
            new_level += 1;
            if let Some(ref v) = voc {
                new_health_max = (new_health_max + v.gain_hp as i32).max(0);
                new_mana_max = new_mana_max.saturating_add(v.gain_mana);
                new_cap = new_cap.saturating_add(v.gain_cap);
            }
        }

        // Apply changes.
        if let Some(player) = self.get_player_mut(attacker_id) {
            player.experience = new_exp;
            player.level = new_level;
            player.base.health_max = new_health_max;
            player.mana_max = new_mana_max;
            player.capacity = new_cap;
        }

        // Animated text at target location.
        self.add_animated_text(target_pos, TEXTCOLOR_WHITE, &gained.to_string());

        // Text message to the player.
        let leveled_up = new_level > level;
        let msg = if leveled_up {
            format!("You gained {} experience points. You advanced from level {} to level {}.",
                    gained, level, new_level)
        } else {
            format!("You gained {} experience point{}.", gained, if gained != 1 { "s" } else { "" })
        };
        self.send_combat_text_message(attacker_id, 26, target_pos, gained as u32, TEXTCOLOR_WHITE, 0, TEXTCOLOR_NONE, msg);

        if leveled_up {
            // Send level-up magic effect + 0x82 magic effect at player position.
            let ppos = self.get_player(attacker_id).map(|p| p.base.position).unwrap_or_default();
            self.add_magic_effect(ppos, CONST_ME_MAGIC_BLUE);
            // Send updated stats.
            self.send_player_stats(attacker_id);
            // Fire onAdvance (SKILL_LEVEL = 8) for each level gained.
            let aid = attacker_id;
            let prev = level;
            let next = new_level;
            crate::runtime::g_dispatcher().add_task(crate::runtime::dispatcher::Task::new(move || {
                for l in prev..next {
                    crate::events::dispatch::execute_creature_event_advance(aid, 8, l, l + 1);
                }
            }));
        }
    }

    /// Full player death: skill/exp loss, teleport to temple, send packets.
    /// Mirrors C++ `Player::death`.
    fn execute_player_death(&mut self, player_id: CreatureId, _last_hit_id: Option<CreatureId>) {
        use crate::config::{g_config, IntegerConfig};
        use crate::world::vocation::g_vocations;

        let (skill_loss, level, experience, level_percent, voc_id, skull, health_max, mana_max,
             blessed_count, is_promoted, temple_pos, current_pos)
        = {
            let Some(player) = self.get_player(player_id) else { return };
            let temple_pos = {
                let town_id = player.town_id;
                self.map.get_town_temple_pos(town_id)
                    .unwrap_or(player.base.position)
            };
            (
                player.skill_loss,
                player.level,
                player.experience,
                player.level_percent,
                player.vocation_id,
                player.base.skull,
                player.base.health_max,
                player.mana_max,
                player.blessings.count_ones(),
                player.vocation_id > 3,
                temple_pos,
                player.base.position,
            )
        };

        let death_lose_percent_cfg = g_config().get_number(IntegerConfig::DeathLosePercent);

        let loss_pct: f64 = if skill_loss {
            if death_lose_percent_cfg != -1 {
                let pct = death_lose_percent_cfg as f64
                    - if is_promoted { 3.0 } else { 0.0 }
                    - blessed_count as f64;
                pct.max(0.0) / 100.0
            } else if level >= 25 {
                let tmp_level = level as f64 + (level_percent as f64 / 100.0);
                let pct = ((tmp_level + 50.0) * 50.0 * ((tmp_level * tmp_level) - (5.0 * tmp_level) + 8.0))
                    / experience as f64;
                let reduction = if is_promoted { 30.0 } else { 0.0 } + blessed_count as f64 * 8.0;
                pct * (1.0 - reduction / 100.0) / 100.0
            } else {
                10.0 / 100.0
            }
        } else {
            0.0
        };

        if skill_loss && loss_pct > 0.0 {
            let exp_loss = (experience as f64 * loss_pct) as u64;

            // Level loss
            let new_exp = experience.saturating_sub(exp_loss);
            let mut new_level = level;
            let mut new_health_max = health_max;
            let mut new_mana_max = mana_max;
            let voc = g_vocations().get_vocation(voc_id).cloned();

            if voc_id == 0 || level > 7 {
                while new_level > 1 && new_exp < Player::get_exp_for_level(new_level as u64) {
                    new_level -= 1;
                    if let Some(ref v) = voc {
                        new_health_max = (new_health_max - v.gain_hp as i32).max(0);
                        new_mana_max = new_mana_max.saturating_sub(v.gain_mana);
                    }
                }
            }

            let new_level_percent = {
                let curr = Player::get_exp_for_level(new_level as u64);
                let next = Player::get_exp_for_level(new_level as u64 + 1);
                if next > curr {
                    Player::get_percent_level(new_exp.saturating_sub(curr), next - curr)
                } else {
                    0
                }
            };

            let old_level = level;
            if let Some(player) = self.get_player_mut(player_id) {
                player.experience = new_exp;
                player.level = new_level;
                player.level_percent = new_level_percent;
                player.base.health_max = new_health_max;
                player.mana_max = new_mana_max;

                // Reset health/mana (black skull gets 40 hp / 0 mana).
                if skull == Skull::Black {
                    player.base.health = 40;
                    player.mana = 0;
                } else {
                    player.base.health = new_health_max;
                    player.mana = new_mana_max;
                }

                // Remove non-persistent conditions.
                player.base.conditions.retain(|c| !c.is_persistent());

                player.base.position = temple_pos;
            }

            if old_level != new_level {
                let msg = format!("You were downgraded from Level {} to Level {}.", old_level, new_level);
                self.send_text_message(player_id, MESSAGE_STATUS_DEFAULT, msg);
            }
        } else {
            // No skill loss: just heal and teleport.
            if let Some(player) = self.get_player_mut(player_id) {
                player.base.health = player.base.health_max;
                player.base.conditions.retain(|c| !c.is_persistent());
                player.base.position = temple_pos;
            }
        }

        // Move on map.
        self.move_creature_position(player_id, current_pos, temple_pos);

        // Send updated health bar, stats, and re-login window (0x28).
        self.add_creature_health(player_id);
        self.send_player_stats(player_id);
        send_packet_to_player(player_id, |output: &mut OutputMessage| {
            output.add_byte(0x28);
        });
    }

    /// Send player stats (0xA0) packet to the player.
    pub fn send_player_stats(&mut self, player_id: CreatureId) {
        let Some(player) = self.get_player(player_id) else { return };
        // Snapshot mutable data into owned values.
        let health = player.base.health;
        let health_max = player.base.health_max;
        let exp = player.experience;
        let level = player.level;
        let level_percent = player.level_percent;
        let mana = player.mana;
        let mana_max = player.mana_max;
        let magic_level = player.get_magic_level();
        let mag_level_percent = player.mag_level_percent;
        let soul = player.soul;
        let stamina = player.stamina_minutes;
        let free_cap = player.get_free_capacity();
        send_packet_to_player(player_id, move |output: &mut OutputMessage| {
            output.add_byte(0xA0);
            output.add_u16(health.min(0xFFFF) as u16);
            output.add_u16(health_max.min(0xFFFF) as u16);
            output.add_u32(free_cap);
            output.add_u32(exp.min(0x7FFF_FFFF) as u32);
            output.add_u16(level as u16);
            output.add_byte(level_percent);
            output.add_u16(mana.min(0xFFFF) as u16);
            output.add_u16(mana_max.min(0xFFFF) as u16);
            output.add_byte(magic_level.min(0xFF) as u8);
            output.add_byte(mag_level_percent);
            output.add_byte(soul);
            output.add_u16(stamina);
        });
    }

    /// Remove a creature from the game world and notify spectators (0x6C).
    /// Mirrors C++ `Game::removeCreature` (the death/disappear path).
    pub fn remove_creature_from_world(&mut self, creature_id: CreatureId) {
        let Some(creature) = self.creatures.get(&creature_id) else { return };
        let pos = creature.position();
        let is_player = creature.is_player();

        // Summon bookkeeping: detach this creature from its master's summon list
        // (so the master can re-summon), and orphan any summons it owns.
        let (master_id, own_summons) = self
            .get_creature(creature_id)
            .map(|c| (c.base().master_id, c.base().summon_ids.clone()))
            .unwrap_or((None, Vec::new()));
        if let Some(mid) = master_id {
            if let Some(master) = self.get_creature_mut(mid) {
                master.base_mut().summon_ids.retain(|&id| id != creature_id);
            }
        }
        for sid in own_summons {
            if let Some(summon) = self.get_creature_mut(sid) {
                summon.base_mut().master_id = None;
            }
        }

        // Compute stackpos BEFORE removing from tile
        let stackpos = self.map.get_tile(pos)
            .map(|t| t.get_client_index_of_creature(creature_id))
            .unwrap_or(-1);

        // Get spectators BEFORE removing
        let spectators: Vec<CreatureId> = self.map.get_spectators(pos, true, true, 0, 0, 0, 0)
            .into_iter()
            .filter(|&id| id != creature_id)
            .collect();

        // Remove from tile and data structures
        self.map.remove_creature_from_tile(pos, creature_id, is_player);
        if let Some(creature) = self.creatures.remove(&creature_id) {
            if let Some(player) = creature.as_player() {
                self.player_name_to_id.remove(&player.name);
                self.player_guid_to_id.remove(&player.guid);
            }
        }
        self.remove_creature_check(creature_id);

        // Send 0x6C (remove creature) to all spectator players
        if stackpos >= 0 {
            let sp = stackpos as u8;
            for spec_id in spectators {
                send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
                    output.add_byte(0x6C);
                    if sp < 10 {
                        output.add_position(pos.x, pos.y, pos.z);
                        output.add_byte(sp);
                    } else {
                        output.add_u16(0xFFFF);
                        output.add_u32(creature_id);
                    }
                });
            }
        }
    }

    // ── Line of Sight delegates ─────────────────────────────────────────────

    pub fn is_sight_clear(&self, from_pos: Position, to_pos: Position, same_floor: bool) -> bool {
        self.map.is_sight_clear(from_pos, to_pos, same_floor, &self.items)
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
    ) -> bool {
        self.map.can_throw_object_to(from_pos, to_pos, check_line_of_sight, same_floor, range_x, range_y, &self.items)
    }

    // ── Money operations (port of game.cpp removeMoney / addMoney) ──────────

    pub fn remove_money(&mut self, player_id: CreatureId, amount: u64) -> bool {
        if amount == 0 {
            return true;
        }

        let Some(player) = self.get_player(player_id) else { return false };

        let mut money_map: std::collections::BTreeMap<u64, Vec<(usize, usize)>> = std::collections::BTreeMap::new();
        let mut money_count: u64 = 0;

        for slot in crate::creatures::player::CONST_SLOT_FIRST..=crate::creatures::player::CONST_SLOT_LAST {
            if let Some(ref item) = player.inventory_items[slot] {
                let worth_per = self.items.get_item_type(usize::from(item.server_id)).worth;
                if worth_per > 0 {
                    let stack_worth = worth_per * item.count.max(1) as u64;
                    money_count += stack_worth;
                    money_map.entry(stack_worth).or_default().push((slot, usize::MAX));
                }
                Self::collect_money_from_children(item, &self.items, slot, &mut money_map, &mut money_count);
            }
        }

        if money_count < amount {
            return false;
        }

        let mut remaining = amount;

        let entries: Vec<(u64, Vec<(usize, usize)>)> = money_map.into_iter().collect();

        for (worth, locations) in &entries {
            if remaining == 0 {
                break;
            }
            for &(slot, child_idx) in locations {
                if remaining == 0 {
                    break;
                }
                if *worth <= remaining {
                    remaining -= worth;
                    self.remove_money_item(player_id, slot, child_idx);
                } else {
                    let item_info = self.get_money_item_info(player_id, slot, child_idx);
                    if let Some((server_id, count)) = item_info {
                        let worth_per = self.items.get_item_type(usize::from(server_id)).worth;
                        if worth_per > 0 {
                            let remove_count = ((remaining as f64) / (worth_per as f64)).ceil() as u16;
                            let change = (worth_per * remove_count as u64).saturating_sub(remaining);
                            if remove_count >= count {
                                self.remove_money_item(player_id, slot, child_idx);
                            } else {
                                self.reduce_money_item(player_id, slot, child_idx, remove_count);
                            }
                            remaining = 0;
                            if change > 0 {
                                self.add_money(player_id, change);
                            }
                        }
                    }
                    break;
                }
            }
        }

        true
    }

    fn collect_money_from_children(
        item: &crate::map::tile::MapItem,
        items: &Items,
        slot: usize,
        money_map: &mut std::collections::BTreeMap<u64, Vec<(usize, usize)>>,
        money_count: &mut u64,
    ) {
        for (i, child) in item.children.iter().enumerate() {
            let worth_per = items.get_item_type(usize::from(child.server_id)).worth;
            if worth_per > 0 {
                let stack_worth = worth_per * child.count.max(1) as u64;
                *money_count += stack_worth;
                money_map.entry(stack_worth).or_default().push((slot, i));
            }
            if !child.children.is_empty() {
                Self::collect_money_from_children(child, items, slot, money_map, money_count);
            }
        }
    }

    fn get_money_item_info(&self, player_id: CreatureId, slot: usize, child_idx: usize) -> Option<(u16, u16)> {
        let player = self.get_player(player_id)?;
        let item = player.inventory_items[slot].as_ref()?;
        if child_idx == usize::MAX {
            Some((item.server_id, item.count.max(1)))
        } else {
            let child = item.children.get(child_idx)?;
            Some((child.server_id, child.count.max(1)))
        }
    }

    fn remove_money_item(&mut self, player_id: CreatureId, slot: usize, child_idx: usize) {
        let Some(player) = self.get_player_mut(player_id) else { return };
        if child_idx == usize::MAX {
            player.inventory[slot] = None;
            player.inventory_count[slot] = 1;
            player.inventory_items[slot] = None;
        } else if let Some(ref mut item) = player.inventory_items[slot] {
            if child_idx < item.children.len() {
                item.children.remove(child_idx);
            }
        }
    }

    fn reduce_money_item(&mut self, player_id: CreatureId, slot: usize, child_idx: usize, remove_count: u16) {
        let Some(player) = self.get_player_mut(player_id) else { return };
        if child_idx == usize::MAX {
            if let Some(ref mut item) = player.inventory_items[slot] {
                item.count = item.count.saturating_sub(remove_count);
                player.inventory_count[slot] = item.count.max(1);
            }
        } else if let Some(ref mut item) = player.inventory_items[slot] {
            if let Some(child) = item.children.get_mut(child_idx) {
                child.count = child.count.saturating_sub(remove_count);
            }
        }
    }

    pub fn add_money(&mut self, player_id: CreatureId, mut amount: u64) {
        if amount == 0 {
            return;
        }

        let currency_items: Vec<(u64, u16)> = self
            .items
            .get_currency_items()
            .iter()
            .map(|(rev_worth, &item_id)| (rev_worth.0, item_id))
            .collect();

        for (worth, item_id) in &currency_items {
            let mut currency_coins = amount / worth;
            if currency_coins == 0 {
                continue;
            }

            amount -= currency_coins * worth;
            while currency_coins > 0 {
                let count = std::cmp::min(100, currency_coins) as u16;

                let item = crate::map::tile::MapItem {
                    server_id: *item_id,
                    count,
                    ..crate::map::tile::MapItem::default()
                };

                if !self.add_money_item_to_player(player_id, item.clone()) {
                    if let Some(player) = self.get_player(player_id) {
                        let pos = player.base.position;
                        self.place_map_item_on_tile(pos, item);
                    }
                }

                currency_coins -= count as u64;
            }
        }
    }

    fn add_money_item_to_player(&mut self, player_id: CreatureId, item: crate::map::tile::MapItem) -> bool {
        use crate::creatures::player::{CONST_SLOT_FIRST, CONST_SLOT_LAST, CONST_SLOT_BACKPACK};
        let Some(player) = self.get_player_mut(player_id) else { return false };
        if let Some(slot) = (CONST_SLOT_FIRST..=CONST_SLOT_LAST).find(|&s| player.inventory[s].is_none()) {
            player.inventory[slot] = Some(item.server_id);
            player.inventory_count[slot] = item.count.max(1);
            player.inventory_items[slot] = Some(item);
            return true;
        }
        if let Some(Some(bp)) = player.inventory_items.get_mut(CONST_SLOT_BACKPACK) {
            bp.children.insert(0, item);
            return true;
        }
        false
    }
}

impl Default for Game {
    fn default() -> Self {
        Self::new()
    }
}

fn describe_loot_items(items: &[crate::map::tile::MapItem], game: &Game) -> String {
    let parts: Vec<String> = items.iter().map(|item| {
        let name = game.items.get_item_type(item.server_id as usize).name.clone();
        let display = if name.is_empty() { format!("item {}", item.server_id) } else { name };
        if item.count > 1 { format!("{} {}", item.count, display) } else { display }
    }).collect();
    if parts.is_empty() { "nothing".to_string() } else { parts.join(", ") }
}

/// Mirrors C++ `MonsterType::createLoot` — recursively populates `out` with loot items.
fn generate_loot_recursive(lb: &crate::creatures::monsters::LootBlock, out: &mut Vec<crate::map::tile::MapItem>) {
    use crate::creatures::monsters::MAX_LOOT_CHANCE;
    use crate::util::normal_random;
    use crate::map::tile::MapItem;

    if lb.id == 0 { return; }

    if lb.chance < MAX_LOOT_CHANCE {
        let roll = normal_random(0, (MAX_LOOT_CHANCE - 1) as i32) as u32;
        if roll > lb.chance {
            return;
        }
    }

    let count = if lb.sub_type != -1 {
        lb.sub_type.max(1) as u16
    } else {
        normal_random(1, lb.count_max.max(1) as i32).max(1) as u16
    };

    let mut item = MapItem {
        server_id: lb.id,
        count,
        action_id: if lb.action_id != -1 { lb.action_id as u16 } else { 0 },
        text: lb.text.clone(),
        ..MapItem::default()
    };

    for child in &lb.child_loot {
        generate_loot_recursive(child, &mut item.children);
    }

    out.push(item);
}
