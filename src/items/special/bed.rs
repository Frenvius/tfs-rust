#![allow(dead_code)]

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

    pub fn can_use(&self, _player_guid: u32, _house_id: u32) -> bool {
        false
    }

    pub fn try_sleep(&mut self, _player_id: CreatureId) -> bool {
        false
    }

    pub fn sleep(&mut self, _player_id: CreatureId) -> bool {
        false
    }

    pub fn wake_up(&mut self, _player_id: Option<CreatureId>) {
    }

    pub fn get_next_bed_item_pos(&self, _items: &crate::items::Items) -> Option<Position> {
        None
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
                if data.len() < 4 {
                    return false;
                }
                let guid = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                *data = &data[4..];
                if guid != 0 {
                    self.sleeper_guid = guid;
                }
                true
            }
            ATTR_SLEEPSTART => {
                if data.len() < 4 {
                    return false;
                }
                let val = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                *data = &data[4..];
                self.sleep_start = val as u64;
                true
            }
            _ => false,
        }
    }
}
