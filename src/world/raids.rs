use crate::map::Position;

pub const MAXIMUM_TRIES_PER_MONSTER: i32 = 10;
pub const CHECK_RAIDS_INTERVAL: i32 = 60;
pub const RAID_MINTICKS: i32 = 1000;
pub const MAX_RAND_RANGE: i32 = 10_000_000;

pub const MESSAGE_EVENT_ADVANCE: u8 = 19;
pub const MESSAGE_STATUS_WARNING: u8 = 12;
pub const MESSAGE_EVENT_DEFAULT: u8 = 20;
pub const MESSAGE_INFO_DESCR: u8 = 21;
pub const MESSAGE_STATUS_SMALL: u8 = 22;
pub const MESSAGE_STATUS_CONSOLE_BLUE: u8 = 14;
pub const MESSAGE_STATUS_CONSOLE_RED: u8 = 18;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaidState {
    Idle = 0,
    Executing = 1,
}

pub struct MonsterSpawn {
    pub name: String,
    pub min_amount: u32,
    pub max_amount: u32,
}

impl MonsterSpawn {
    pub fn new(name: String, min_amount: u32, max_amount: u32) -> Self {
        Self { name, min_amount, max_amount }
    }
}

pub trait RaidEvent: Send + Sync {
    fn get_delay(&self) -> u32;
    fn execute_event(&self) -> bool;
}

pub struct AnnounceEvent {
    delay: u32,
    pub message: String,
    pub message_type: u8,
}

impl AnnounceEvent {
    pub fn new(delay: u32, message: String, message_type: u8) -> Self {
        Self { delay, message, message_type }
    }
}

impl RaidEvent for AnnounceEvent {
    fn get_delay(&self) -> u32 {
        self.delay
    }

    fn execute_event(&self) -> bool {
        true
    }
}

pub struct SingleSpawnEvent {
    delay: u32,
    pub monster_name: String,
    pub position: Position,
}

impl SingleSpawnEvent {
    pub fn new(delay: u32, monster_name: String, position: Position) -> Self {
        Self { delay, monster_name, position }
    }
}

impl RaidEvent for SingleSpawnEvent {
    fn get_delay(&self) -> u32 {
        self.delay
    }

    fn execute_event(&self) -> bool {
        true
    }
}

pub struct AreaSpawnEvent {
    delay: u32,
    pub spawn_list: Vec<MonsterSpawn>,
    pub from_pos: Position,
    pub to_pos: Position,
}

impl AreaSpawnEvent {
    pub fn new(delay: u32, spawn_list: Vec<MonsterSpawn>, from_pos: Position, to_pos: Position) -> Self {
        Self { delay, spawn_list, from_pos, to_pos }
    }
}

impl RaidEvent for AreaSpawnEvent {
    fn get_delay(&self) -> u32 {
        self.delay
    }

    fn execute_event(&self) -> bool {
        true
    }
}

pub struct ScriptRaidEvent {
    delay: u32,
    pub script_name: String,
}

impl ScriptRaidEvent {
    pub fn new(delay: u32, script_name: String) -> Self {
        Self { delay, script_name }
    }
}

impl RaidEvent for ScriptRaidEvent {
    fn get_delay(&self) -> u32 {
        self.delay
    }

    fn execute_event(&self) -> bool {
        true
    }
}

pub struct Raid {
    pub name: String,
    interval: u32,
    margin: u64,
    state: RaidState,
    next_event: u32,
    next_event_event: u32,
    loaded: bool,
    repeat: bool,
    events: Vec<Box<dyn RaidEvent>>,
}

impl Raid {
    pub fn new(name: String, interval: u32, margin_time: u32, repeat: bool) -> Self {
        Self {
            name,
            interval,
            margin: margin_time as u64,
            state: RaidState::Idle,
            next_event: 0,
            next_event_event: 0,
            loaded: false,
            repeat,
            events: Vec::new(),
        }
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub fn get_margin(&self) -> u64 {
        self.margin
    }

    pub fn get_interval(&self) -> u32 {
        self.interval
    }

    pub fn can_be_repeated(&self) -> bool {
        self.repeat
    }

    pub fn get_name(&self) -> &str {
        &self.name
    }

    pub fn set_state(&mut self, state: RaidState) {
        self.state = state;
    }

    pub fn get_next_raid_event(&self) -> Option<&dyn RaidEvent> {
        if (self.next_event as usize) < self.events.len() {
            Some(self.events[self.next_event as usize].as_ref())
        } else {
            None
        }
    }

    pub fn start_raid(&mut self) {
    }

    pub fn execute_raid_event(&mut self, _event: &dyn RaidEvent) {
    }

    pub fn reset_raid(&mut self) {
        self.next_event = 0;
        self.state = RaidState::Idle;
    }

    pub fn stop_events(&mut self) {
        if self.next_event_event != 0 {
            self.next_event_event = 0;
        }
    }

    pub fn load_from_json5(&mut self, _path: &std::path::Path) -> Result<(), anyhow::Error> {
        if self.is_loaded() {
            return Ok(());
        }
        self.loaded = true;
        Ok(())
    }
}

pub struct Raids {
    raid_list: Vec<Raid>,
    running: Option<usize>,
    last_raid_end: u64,
    check_raids_event: u32,
    loaded: bool,
    started: bool,
}

impl Raids {
    pub fn new() -> Self {
        Self {
            raid_list: Vec::new(),
            running: None,
            last_raid_end: 0,
            check_raids_event: 0,
            loaded: false,
            started: false,
        }
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub fn is_started(&self) -> bool {
        self.started
    }

    pub fn get_running(&self) -> Option<&Raid> {
        self.running.and_then(|i| self.raid_list.get(i))
    }

    pub fn get_running_mut(&mut self) -> Option<&mut Raid> {
        self.running.and_then(|i| self.raid_list.get_mut(i))
    }

    pub fn set_running(&mut self, name: Option<&str>) {
        self.running = name.and_then(|n| {
            self.raid_list
                .iter()
                .position(|r| r.name.eq_ignore_ascii_case(n))
        });
    }

    pub fn get_raid_by_name(&self, name: &str) -> Option<&Raid> {
        self.raid_list
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(name))
    }

    pub fn get_last_raid_end(&self) -> u64 {
        self.last_raid_end
    }

    pub fn set_last_raid_end(&mut self, t: u64) {
        self.last_raid_end = t;
    }

    pub fn check_raids(&mut self) {
    }

    pub fn load_from_json5(&mut self, _path: &std::path::Path) -> Result<(), anyhow::Error> {
        if self.is_loaded() {
            return Ok(());
        }
        self.loaded = true;
        Ok(())
    }

    pub fn startup(&mut self) -> bool {
        if !self.is_loaded() || self.is_started() {
            return false;
        }
        self.last_raid_end = crate::util::otsys_time() as u64;
        self.started = true;
        true
    }

    pub fn clear(&mut self) {
        self.check_raids_event = 0;

        for raid in &mut self.raid_list {
            raid.stop_events();
        }
        self.raid_list.clear();

        self.loaded = false;
        self.started = false;
        self.running = None;
        self.last_raid_end = 0;
    }

    pub fn reload(&mut self) -> bool {
        self.clear();
        false
    }
}

impl Default for Raids {
    fn default() -> Self {
        Self::new()
    }
}
