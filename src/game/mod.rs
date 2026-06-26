pub mod tick;
mod combat_ops;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::sync::Arc;

use crate::creatures::{Creature, CreatureId};
use crate::creatures::player::Player;
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
    pub parties: HashMap<CreatureId, Party>,
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

    pub fn regenerate_slept_player(&mut self, creature_id: CreatureId, sleep_start: u32) {
        crate::items::special::bed::regenerate_slept_player(self, creature_id, sleep_start);
    }

    pub fn wake_bed_at(&mut self, pos: Position, server_id: u16) {
        crate::items::special::bed::wake_bed_at(self, pos, server_id);
    }

    pub fn house_transfer_to_depot(&mut self, house_id: u32) -> bool {
        crate::map::houses::transfer_to_depot(self, house_id)
    }

    pub fn house_set_owner(&mut self, house_id: u32, guid: u32) {
        crate::map::houses::set_owner(self, house_id, guid);
    }

    // ── Party system (port of party.cpp + Player party-icon sends) ──────────

    /// Shield `observer` should see for `target`. Port of `Player::getPartyShield`.
    pub fn get_party_shield(&self, observer_id: CreatureId, target_id: CreatureId) -> u8 {
        crate::world::party::get_party_shield(self, observer_id, target_id)
    }

    pub fn send_creature_shield(&self, to_id: CreatureId, about_id: CreatureId) {
        crate::world::party::send_creature_shield(self, to_id, about_id);
    }

    pub fn update_player_shield(&mut self, player_id: CreatureId) {
        crate::world::party::update_player_shield(self, player_id);
    }

    pub fn party_update_all_icons(&self, leader_id: CreatureId) {
        crate::world::party::update_all_icons(self, leader_id);
    }

    pub fn party_can_use_shared_experience(&self, leader_id: CreatureId, player_id: CreatureId) -> bool {
        crate::world::party::can_use_shared_experience(self, leader_id, player_id)
    }

    pub fn party_update_shared_experience(&mut self, leader_id: CreatureId) {
        crate::world::party::update_shared_experience(self, leader_id);
    }

    pub fn player_invite_to_party(&mut self, player_id: CreatureId, invited_id: CreatureId) {
        crate::world::party::invite_to_party(self, player_id, invited_id);
    }

    pub fn player_join_party(&mut self, player_id: CreatureId, leader_id: CreatureId) {
        crate::world::party::join_party(self, player_id, leader_id);
    }

    pub fn player_revoke_party_invitation(&mut self, player_id: CreatureId, invited_id: CreatureId) {
        crate::world::party::revoke_party_invitation(self, player_id, invited_id);
    }

    pub fn player_pass_party_leadership(&mut self, player_id: CreatureId, new_leader_id: CreatureId) {
        crate::world::party::pass_party_leadership(self, player_id, new_leader_id);
    }

    pub fn player_leave_party(&mut self, player_id: CreatureId) {
        crate::world::party::leave_party(self, player_id);
    }

    pub fn party_disband(&mut self, leader_id: CreatureId) {
        crate::world::party::disband_party(self, leader_id);
    }

    pub fn player_enable_shared_party_experience(&mut self, player_id: CreatureId, active: bool) {
        crate::world::party::enable_shared_party_experience(self, player_id, active);
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
