#![allow(dead_code)]

pub const ATTR_DEPOT_ID: u8 = 10;

#[derive(Debug)]
pub struct DepotChest {
    pub item_id: u16,
    pub max_depot_items: u32,
    pub save: bool,
}

impl DepotChest {
    pub fn new(item_id: u16) -> Self {
        Self {
            item_id,
            max_depot_items: 2000,
            save: false,
        }
    }

    pub fn set_max_depot_items(&mut self, max: u32) {
        self.max_depot_items = max;
    }

    pub fn needs_save(&self) -> bool {
        self.save
    }

    pub fn can_remove(&self) -> bool {
        false
    }

    pub fn on_add_notification(&mut self) {
        self.save = true;
    }

    pub fn on_remove_notification(&mut self) {
        self.save = true;
    }
}

#[derive(Debug)]
pub struct DepotLocker {
    pub item_id: u16,
    pub depot_id: u16,
    pub save: bool,
}

impl DepotLocker {
    pub fn new(item_id: u16) -> Self {
        Self {
            item_id,
            depot_id: 0,
            save: false,
        }
    }

    pub fn get_depot_id(&self) -> u16 {
        self.depot_id
    }

    pub fn set_depot_id(&mut self, id: u16) {
        self.depot_id = id;
    }

    pub fn needs_save(&self) -> bool {
        self.save
    }

    pub fn can_remove(&self) -> bool {
        false
    }

    pub fn on_add_notification(&mut self) {
        self.save = true;
    }

    pub fn on_remove_notification(&mut self) {
        self.save = true;
    }

    pub fn read_attr(&mut self, attr_type: u8, data: &mut &[u8]) -> bool {
        if attr_type == ATTR_DEPOT_ID {
            if data.len() < 2 {
                return false;
            }
            self.depot_id = u16::from_le_bytes([data[0], data[1]]);
            *data = &data[2..];
            true
        } else {
            false
        }
    }
}
