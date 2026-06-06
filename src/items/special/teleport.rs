#![allow(dead_code)]

use crate::map::Position;

pub const ATTR_TELE_DEST: u8 = 8;

#[derive(Debug)]
pub struct Teleport {
    pub item_id: u16,
    pub dest_pos: Position,
}

impl Teleport {
    pub fn new(item_id: u16) -> Self {
        Self {
            item_id,
            dest_pos: Position::default(),
        }
    }

    pub fn get_dest_pos(&self) -> Position {
        self.dest_pos
    }

    pub fn set_dest_pos(&mut self, pos: Position) {
        self.dest_pos = pos;
    }

    pub fn serialize_attr(&self, out: &mut Vec<u8>) {
        out.push(ATTR_TELE_DEST);
        out.extend_from_slice(&self.dest_pos.x.to_le_bytes());
        out.extend_from_slice(&self.dest_pos.y.to_le_bytes());
        out.push(self.dest_pos.z);
    }

    pub fn read_attr(&mut self, attr_type: u8, data: &mut &[u8]) -> bool {
        if attr_type == ATTR_TELE_DEST {
            if data.len() < 5 {
                return false;
            }
            self.dest_pos.x = u16::from_le_bytes([data[0], data[1]]);
            self.dest_pos.y = u16::from_le_bytes([data[2], data[3]]);
            self.dest_pos.z = data[4];
            *data = &data[5..];
            true
        } else {
            false
        }
    }
}
