#![allow(dead_code)]

pub const ITEM_PARCEL: u16 = 2595;
pub const ITEM_LETTER: u16 = 2597;

#[derive(Debug)]
pub struct Mailbox {
    pub item_id: u16,
}

impl Mailbox {
    pub fn new(item_id: u16) -> Self {
        Self { item_id }
    }

    pub fn can_send(item_id: u16) -> bool {
        item_id == ITEM_PARCEL || item_id == ITEM_LETTER
    }

    pub fn get_receiver(
        &self,
        _item_id: u16,
        _text: &str,
    ) -> Option<(String, u32)> {
        // Dead code — mailbox delivery is in game_protocol.rs::mailbox_deliver
        None
    }

    pub fn send_item(&self, _item_id: u16, _receiver_name: &str, _depot_id: u32) -> bool {
        // Dead code — mailbox delivery is in game_protocol.rs::mailbox_deliver
        false
    }
}
