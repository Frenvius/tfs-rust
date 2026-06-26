use crate::map::Position;

pub const ITEM_PARCEL: u16 = 2595;
pub const ITEM_LETTER: u16 = 2597;
const ITEM_LABEL: u16 = 2599;

pub fn can_send(item_id: u16) -> bool {
    item_id == ITEM_PARCEL || item_id == ITEM_LETTER
}

pub fn get_receiver(
    game: &crate::game::Game,
    item: &crate::map::tile::MapItem,
) -> Option<(String, u32)> {
    let is_container = game.items.get_item_type(usize::from(item.server_id)).group
        == crate::items::ItemGroup::Container;
    if is_container {
        for child in &item.children {
            if child.server_id == ITEM_LABEL {
                if let Some(r) = get_receiver(game, child) {
                    return Some(r);
                }
            }
        }
        return None;
    }

    if item.text.trim().is_empty() {
        return None;
    }
    let mut lines = item.text.lines();
    let name = lines.next().unwrap_or("").trim().to_string();
    let town_name = lines.next().unwrap_or("").trim();
    let depot_id = game
        .map
        .towns
        .values()
        .find(|t| t.name.eq_ignore_ascii_case(town_name))
        .map(|t| t.id)?;
    Some((name, depot_id))
}

pub fn deliver(game: &mut crate::game::Game, item: &crate::map::tile::MapItem) -> bool {
    let Some((name, depot_id)) = get_receiver(game, item) else { return false };
    if name.is_empty() || depot_id == 0 {
        return false;
    }
    let Some(cid) = game.get_player_id_by_name(&name) else { return false };

    let mut delivered = item.clone();
    delivered.server_id = item.server_id.saturating_add(1);
    match game.get_player_mut(cid) {
        Some(p) => p.depot_items.entry(depot_id).or_default().insert(0, delivered),
        None => return false,
    }

    if is_near_depot_box(game, cid) {
        game.send_text_message(
            cid,
            crate::world::raids::MESSAGE_EVENT_ADVANCE,
            "New mail has arrived.".to_string(),
        );
    }
    true
}

pub fn is_near_depot_box(game: &crate::game::Game, creature_id: crate::creatures::CreatureId) -> bool {
    let Some(pos) = game.get_player(creature_id).map(|p| p.base.position) else { return false };
    for cx in -1i32..=1 {
        for cy in -1i32..=1 {
            let p = Position {
                x: pos.x.wrapping_add(cx as u16),
                y: pos.y.wrapping_add(cy as u16),
                z: pos.z,
            };
            if game
                .map
                .get_tile(p)
                .map(|t| t.has_flag(crate::map::tile::TILESTATE_DEPOT))
                .unwrap_or(false)
            {
                return true;
            }
        }
    }
    false
}
