use std::collections::BTreeMap;

use crate::creatures::{CreatureId, Direction};
use crate::map::Position;

pub const MINSPAWN_INTERVAL: i32 = 10 * 1000;
pub const MAXSPAWN_INTERVAL: i32 = 24 * 60 * 60 * 1000;

pub struct SpawnBlock {
    pub pos: Position,
    pub monster_names: Vec<(String, u16)>,
    pub last_spawn: i64,
    pub interval: u32,
    pub direction: Direction,
}

pub struct NpcBlock {
    pub pos: Position,
    pub name: String,
    pub direction: Direction,
}

pub struct Spawn {
    pub center_pos: Position,
    pub radius: i32,
    pub interval: u32,
    pub spawned_map: BTreeMap<u32, CreatureId>,
    pub spawn_map: BTreeMap<u32, SpawnBlock>,
    pub npc_blocks: Vec<NpcBlock>,
    check_spawn_event: u32,
}

impl Spawn {
    pub fn new(pos: Position, radius: i32) -> Self {
        Self {
            center_pos: pos,
            radius,
            interval: 60000,
            spawned_map: BTreeMap::new(),
            spawn_map: BTreeMap::new(),
            npc_blocks: Vec::new(),
            check_spawn_event: 0,
        }
    }

    pub fn add_block(&mut self, sb: SpawnBlock) -> bool {
        self.interval = self.interval.min(sb.interval);
        let next_id = self.spawn_map.len() as u32 + 1;
        self.spawn_map.insert(next_id, sb);
        true
    }

    pub fn add_monster(&mut self, name: &str, pos: Position, dir: Direction, interval: u32) -> bool {
        let sb = SpawnBlock {
            monster_names: vec![(name.to_owned(), 100)],
            pos,
            direction: dir,
            interval,
            last_spawn: 0,
        };
        self.add_block(sb)
    }

    pub fn remove_monster(&mut self, creature_id: CreatureId) {
        self.spawned_map.retain(|_, v| *v != creature_id);
    }

    pub fn is_in_spawn_zone(&self, pos: Position) -> bool {
        Spawns::is_in_zone(self.center_pos, self.radius, pos)
    }

    pub fn get_interval(&self) -> u32 {
        self.interval
    }

    pub fn startup(&mut self) {
        use crate::creatures::{Creature, monster::Monster, npc::Npc};
        use crate::game::g_game;

        for (&spawn_id, sb) in &self.spawn_map {
            for (name, _weight) in &sb.monster_names {
                let Some(mut monster) = Monster::create_monster(name) else { continue };
                monster.base.position = sb.pos;
                monster.spawn_pos = sb.pos;
                monster.base.direction = sb.direction;

                let creature_id = monster.base.id;
                let pos = sb.pos;
                let placed = g_game().lock().unwrap()
                    .place_creature(Creature::Monster(monster));

                if placed {
                    // Broadcast after the lock is released (no-op pre-login, but
                    // keeps the appear path consistent with runtime respawns).
                    crate::net::game_protocol::broadcast_creature_appear(creature_id, pos);
                    self.spawned_map.insert(spawn_id, creature_id);
                    break;
                }
            }
        }

        for nb in &self.npc_blocks {
            let Some(mut npc) = Npc::create_npc(&nb.name) else { continue };
            npc.base.position = nb.pos;
            npc.base.direction = nb.direction;
            let creature_id = npc.base.id;
            let pos = nb.pos;
            let placed = g_game().lock().unwrap().place_creature(Creature::Npc(npc));
            if placed {
                crate::net::game_protocol::broadcast_creature_appear(creature_id, pos);
            }
        }
    }

    pub fn start_spawn_check(&mut self) {
    }

    pub fn stop_event(&mut self) {
        if self.check_spawn_event != 0 {
            self.check_spawn_event = 0;
        }
    }

    pub fn cleanup(&mut self) {
        self.stop_event();
    }

    pub fn find_player(_pos: Position) -> bool {
        false
    }

    pub fn spawn_monster(&mut self, _spawn_id: u32, _startup: bool) -> bool {
        false
    }

    pub fn check_spawn(&mut self) {
    }
}

pub struct Spawns {
    pub spawn_list: Vec<Spawn>,
    pub npc_list: Vec<u32>,
    pub filename: String,
    pub loaded: bool,
    pub started: bool,
}

impl Spawns {
    pub fn new() -> Self {
        Self {
            spawn_list: Vec::new(),
            npc_list: Vec::new(),
            filename: String::new(),
            loaded: false,
            started: false,
        }
    }

    pub fn is_in_zone(center: Position, radius: i32, pos: Position) -> bool {
        if radius == -1 {
            return true;
        }
        (pos.x as i32 >= center.x as i32 - radius)
            && (pos.x as i32 <= center.x as i32 + radius)
            && (pos.y as i32 >= center.y as i32 - radius)
            && (pos.y as i32 <= center.y as i32 + radius)
    }

    pub fn load_from_json5(&mut self, path: &std::path::Path) -> Result<(), anyhow::Error> {
        if self.loaded {
            return Ok(());
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read {:?}: {}", path, e))?;
        let val: serde_json::Value = json5::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse {:?}: {}", path, e))?;

        let entries = val["spawns"].as_array().cloned().unwrap_or_default();

        for entry in &entries {
            let centerx = entry["centerx"].as_u64().unwrap_or(0) as u16;
            let centery = entry["centery"].as_u64().unwrap_or(0) as u16;
            let centerz = entry["centerz"].as_u64().unwrap_or(7) as u8;
            let radius = entry["radius"].as_i64().unwrap_or(0) as i32;

            let center_pos = Position { x: centerx, y: centery, z: centerz };
            let mut spawn = Spawn::new(center_pos, radius);

            // Collect monster entries from "monster" (single) or "monsters" (array).
            let mut monster_entries: Vec<&serde_json::Value> = Vec::new();
            if let Some(arr) = entry["monsters"].as_array() {
                monster_entries.extend(arr.iter());
            }
            if let Some(obj) = entry.get("monster") {
                if obj.is_object() {
                    monster_entries.push(obj);
                }
            }

            for m in &monster_entries {
                let name = m["name"].as_str().unwrap_or("").to_owned();
                if name.is_empty() {
                    continue;
                }
                let rx = m["x"].as_i64().unwrap_or(0) as i16;
                let ry = m["y"].as_i64().unwrap_or(0) as i16;
                let abs_z = m["z"].as_u64().unwrap_or(centerz as u64) as u8;
                let spawntime = m["spawntime"].as_u64().unwrap_or(60);

                let abs_x = (centerx as i32 + rx as i32).clamp(0, u16::MAX as i32) as u16;
                let abs_y = (centery as i32 + ry as i32).clamp(0, u16::MAX as i32) as u16;
                let pos = Position { x: abs_x, y: abs_y, z: abs_z };

                let interval = ((spawntime * 1000) as u32)
                    .clamp(MINSPAWN_INTERVAL as u32, MAXSPAWN_INTERVAL as u32);

                spawn.add_monster(&name, pos, Direction::South, interval);
            }

            // Collect NPC entries from "npc" (single) or "npcs" (array).
            let mut npc_entries: Vec<&serde_json::Value> = Vec::new();
            if let Some(arr) = entry["npcs"].as_array() {
                npc_entries.extend(arr.iter());
            }
            if let Some(obj) = entry.get("npc") {
                if obj.is_object() {
                    npc_entries.push(obj);
                }
            }

            for n in &npc_entries {
                let name = n["name"].as_str().unwrap_or("").to_owned();
                if name.is_empty() {
                    continue;
                }
                let rx = n["x"].as_i64().unwrap_or(0) as i16;
                let ry = n["y"].as_i64().unwrap_or(0) as i16;
                let abs_z = n["z"].as_u64().unwrap_or(centerz as u64) as u8;
                let dir_val = n["direction"].as_u64().unwrap_or(2) as u8;
                let direction = Direction::from_u8(dir_val).unwrap_or(Direction::South);

                let abs_x = (centerx as i32 + rx as i32).clamp(0, u16::MAX as i32) as u16;
                let abs_y = (centery as i32 + ry as i32).clamp(0, u16::MAX as i32) as u16;
                let pos = Position { x: abs_x, y: abs_y, z: abs_z };

                spawn.npc_blocks.push(NpcBlock { pos, name, direction });
            }

            if !spawn.spawn_map.is_empty() || !spawn.npc_blocks.is_empty() {
                self.spawn_list.push(spawn);
            }
        }

        self.filename = path.display().to_string();
        self.loaded = true;
        Ok(())
    }

    pub fn startup(&mut self) {
        if !self.loaded || self.is_started() {
            return;
        }
        for spawn in &mut self.spawn_list {
            spawn.startup();
        }
        self.started = true;
    }

    pub fn clear(&mut self) {
        for spawn in &mut self.spawn_list {
            spawn.stop_event();
        }
        self.spawn_list.clear();
        self.loaded = false;
        self.started = false;
        self.filename.clear();
    }

    pub fn is_started(&self) -> bool {
        self.started
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }
}

impl Default for Spawns {
    fn default() -> Self {
        Self::new()
    }
}
