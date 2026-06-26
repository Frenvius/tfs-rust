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

pub fn open_depot(player_id: crate::creatures::CreatureId, server_id: u16, depot_id: u32) {
    use crate::creatures::player::ContainerParent;
    use crate::game::g_game;
    use crate::net::game_protocol::{send_packet_to_player, send_status_message_to_player, write_close_container, write_container};
    use crate::net::output_message::OutputMessage;

    let game = g_game().lock().unwrap();
    let Some(player) = game.get_player(player_id) else { return };

    let existing = player.open_containers.iter()
        .find(|(_, oc)| matches!(oc.parent, ContainerParent::Depot(d) if d == depot_id))
        .map(|(&cid, _)| cid);
    if let Some(existing_cid) = existing {
        drop(game);
        let mut game = g_game().lock().unwrap();
        if let Some(p) = game.get_player_mut(player_id) { p.close_container(existing_cid); }
        drop(game);
        send_packet_to_player(player_id, move |o: &mut OutputMessage| write_close_container(o, existing_cid));
        return;
    }

    let Some(cid) = player.get_free_container_id() else {
        drop(game);
        send_status_message_to_player(player_id, "You cannot open any more containers.");
        return;
    };
    let item_type = game.items.get_item_type(usize::from(server_id));
    let capacity = item_type.max_items.clamp(1, 255) as u8;
    let children: Vec<crate::map::tile::MapItem> = player.depot_items.get(&depot_id).cloned().unwrap_or_default();
    let chest = crate::map::tile::MapItem { server_id, ..crate::map::tile::MapItem::default() };
    let items_ref = game.items.clone();
    drop(game);

    {
        let mut game = g_game().lock().unwrap();
        if let Some(p) = game.get_player_mut(player_id) {
            p.add_container(cid, ContainerParent::Depot(depot_id));
            p.set_last_depot_id(depot_id as i16);
        }
    }
    send_packet_to_player(player_id, move |o: &mut OutputMessage| {
        write_container(o, cid, &chest, &items_ref, "Depot chest", capacity, false, &children);
    });
}
