use std::collections::BTreeMap;

use crate::creatures::CreatureId;

pub const EXPERIENCE_SHARE_RANGE: i32 = 30;
pub const EXPERIENCE_SHARE_FLOORS: i32 = 1;

pub struct Party {
    pub leader_id: CreatureId,
    member_ids: Vec<CreatureId>,
    invite_ids: Vec<CreatureId>,
    ticks_map: BTreeMap<u32, i64>,
    shared_exp_active: bool,
    shared_exp_enabled: bool,
}

impl Party {
    pub fn new(leader_id: CreatureId) -> Self {
        Self {
            leader_id,
            member_ids: Vec::new(),
            invite_ids: Vec::new(),
            ticks_map: BTreeMap::new(),
            shared_exp_active: false,
            shared_exp_enabled: false,
        }
    }

    pub fn get_leader_id(&self) -> CreatureId { self.leader_id }
    pub fn get_members(&self) -> &[CreatureId] { &self.member_ids }
    pub fn get_invitees(&self) -> &[CreatureId] { &self.invite_ids }
    pub fn invite_ids_mut(&mut self) -> &mut Vec<CreatureId> { &mut self.invite_ids }
    pub fn member_ids_mut(&mut self) -> &mut Vec<CreatureId> { &mut self.member_ids }
    pub fn set_shared_exp_enabled(&mut self, enabled: bool) { self.shared_exp_enabled = enabled; }
    pub fn set_shared_exp_active(&mut self, active: bool) { self.shared_exp_active = active; }
    pub fn get_player_tick(&self, player_id: CreatureId) -> Option<i64> { self.ticks_map.get(&player_id).copied() }
    pub fn set_leader_id(&mut self, leader_id: CreatureId) { self.leader_id = leader_id; }
    pub fn get_member_count(&self) -> usize { self.member_ids.len() }
    pub fn get_invitation_count(&self) -> usize { self.invite_ids.len() }
    pub fn is_player_invited(&self, player_id: CreatureId) -> bool { self.invite_ids.contains(&player_id) }
    pub fn is_empty(&self) -> bool { self.member_ids.is_empty() && self.invite_ids.is_empty() }
    pub fn is_shared_experience_active(&self) -> bool { self.shared_exp_active }
    pub fn is_shared_experience_enabled(&self) -> bool { self.shared_exp_enabled }

    pub fn remove_invite(&mut self, player_id: CreatureId, _remove_from_player: bool) -> bool {
        let pos = self.invite_ids.iter().position(|&id| id == player_id);
        match pos {
            None => false,
            Some(i) => {
                self.invite_ids.remove(i);
                if self.is_empty() {
                    self.disband();
                }
                true
            }
        }
    }

    pub fn update_player_ticks(&mut self, player_id: CreatureId, points: u32) {
        if points != 0 {
            self.ticks_map.insert(player_id, crate::util::otsys_time());
        }
    }

    pub fn clear_player_points(&mut self, player_id: CreatureId) {
        self.ticks_map.remove(&player_id);
    }

    pub fn disband(&mut self) {
        self.member_ids.clear();
        self.invite_ids.clear();
    }
}

use crate::game::Game;
use crate::net::game_protocol::send_packet_to_player;
use crate::net::output_message::OutputMessage;

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

pub fn get_party_shield(game: &Game, observer_id: CreatureId, target_id: CreatureId) -> u8 {
    let observer_party = game.get_player(observer_id).and_then(|p| p.party_id);
    let target_party = game.get_player(target_id).and_then(|p| p.party_id);

    if let Some(leader_id) = observer_party {
        let (active, enabled) = game
            .get_party(leader_id)
            .map(|p| (p.is_shared_experience_active(), p.is_shared_experience_enabled()))
            .unwrap_or((false, false));
        if target_id == leader_id {
            if active {
                if enabled { return SHIELD_YELLOW_SHAREDEXP; }
                if can_use_shared_experience(game, leader_id, target_id) { return SHIELD_YELLOW_NOSHAREDEXP; }
                return SHIELD_YELLOW_NOSHAREDEXP_BLINK;
            }
            return SHIELD_YELLOW;
        }
        if target_party == Some(leader_id) {
            if active {
                if enabled { return SHIELD_BLUE_SHAREDEXP; }
                if can_use_shared_experience(game, leader_id, target_id) { return SHIELD_BLUE_NOSHAREDEXP; }
                return SHIELD_BLUE_NOSHAREDEXP_BLINK;
            }
            return SHIELD_BLUE;
        }
        if observer_id == leader_id
            && game.get_party(leader_id).map(|p| p.is_player_invited(target_id)).unwrap_or(false)
        {
            return SHIELD_WHITEBLUE;
        }
    }

    if let Some(t_leader) = target_party {
        if target_id == t_leader
            && game.get_party(t_leader).map(|p| p.is_player_invited(observer_id)).unwrap_or(false)
        {
            return SHIELD_WHITEYELLOW;
        }
    }
    SHIELD_NONE
}

pub fn send_creature_shield(game: &Game, to_id: CreatureId, about_id: CreatureId) {
    let shield = get_party_shield(game, to_id, about_id);
    send_packet_to_player(to_id, move |o: &mut OutputMessage| {
        o.add_byte(0x91);
        o.add_u32(about_id);
        o.add_byte(shield);
    });
}

pub fn update_player_shield(game: &mut Game, player_id: CreatureId) {
    let pos = match game.get_player(player_id) {
        Some(p) => p.base.position,
        None => return,
    };
    for spec_id in game.map.get_spectators(pos, true, true, 0, 0, 0, 0) {
        send_creature_shield(game, spec_id, player_id);
    }
}

pub fn update_all_icons(game: &Game, leader_id: CreatureId) {
    let members = game
        .get_party(leader_id)
        .map(|p| p.get_members().to_vec())
        .unwrap_or_default();
    for &member in &members {
        for &other in &members {
            send_creature_shield(game, member, other);
        }
        send_creature_shield(game, member, leader_id);
        send_creature_shield(game, leader_id, member);
    }
    send_creature_shield(game, leader_id, leader_id);
}

fn broadcast_message(game: &mut Game, leader_id: CreatureId, text: &str, to_invitees: bool) {
    let (members, invitees) = match game.get_party(leader_id) {
        Some(p) => (p.get_members().to_vec(), p.get_invitees().to_vec()),
        None => return,
    };
    for member in members {
        game.send_text_message(member, crate::world::raids::MESSAGE_INFO_DESCR, text.to_string());
    }
    game.send_text_message(leader_id, crate::world::raids::MESSAGE_INFO_DESCR, text.to_string());
    if to_invitees {
        for invitee in invitees {
            game.send_text_message(invitee, crate::world::raids::MESSAGE_INFO_DESCR, text.to_string());
        }
    }
}

fn player_text(game: &mut Game, player_id: CreatureId, text: &str) {
    game.send_text_message(player_id, crate::world::raids::MESSAGE_INFO_DESCR, text.to_string());
}

pub fn can_use_shared_experience(game: &Game, leader_id: CreatureId, player_id: CreatureId) -> bool {
    let members = match game.get_party(leader_id) {
        Some(p) => p.get_members().to_vec(),
        None => return false,
    };
    if members.is_empty() { return false; }
    let mut highest = game.get_player(leader_id).map(|p| p.get_level()).unwrap_or(0);
    for m in &members {
        if let Some(p) = game.get_player(*m) {
            highest = highest.max(p.get_level());
        }
    }
    let min_level = ((highest as f32 * 2.0) / 3.0).ceil() as u32;
    let player = match game.get_player(player_id) {
        Some(p) => p,
        None => return false,
    };
    if player.get_level() < min_level { return false; }
    let leader_pos = match game.get_player(leader_id) {
        Some(p) => p.base.position,
        None => return false,
    };
    let ppos = player.base.position;
    let dx = (leader_pos.x as i32 - ppos.x as i32).abs();
    let dy = (leader_pos.y as i32 - ppos.y as i32).abs();
    let dz = (leader_pos.z as i32 - ppos.z as i32).abs();
    if dx > EXPERIENCE_SHARE_RANGE || dy > EXPERIENCE_SHARE_RANGE || dz > EXPERIENCE_SHARE_FLOORS {
        return false;
    }
    if !player.has_flag(crate::creatures::player::PLAYER_FLAG_NOT_GAIN_IN_FIGHT) {
        let last = game.get_party(leader_id).and_then(|p| p.get_player_tick(player_id));
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

pub fn can_enable_shared_experience(game: &Game, leader_id: CreatureId) -> bool {
    if !can_use_shared_experience(game, leader_id, leader_id) { return false; }
    let members = game.get_party(leader_id).map(|p| p.get_members().to_vec()).unwrap_or_default();
    members.iter().all(|&m| can_use_shared_experience(game, leader_id, m))
}

pub fn update_shared_experience(game: &mut Game, leader_id: CreatureId) {
    let active = game.get_party(leader_id).map(|p| p.is_shared_experience_active()).unwrap_or(false);
    if !active { return; }
    let result = can_enable_shared_experience(game, leader_id);
    let prev = game.get_party(leader_id).map(|p| p.is_shared_experience_enabled()).unwrap_or(false);
    if result != prev {
        if let Some(p) = game.get_party_mut(leader_id) {
            p.set_shared_exp_enabled(result);
        }
        update_all_icons(game, leader_id);
    }
}

fn leader_possessive(game: &Game, leader_id: CreatureId) -> &'static str {
    use crate::creatures::player::PlayerSex;
    match game.get_player(leader_id).map(|p| p.sex) {
        Some(PlayerSex::Female) => "her",
        _ => "his",
    }
}

pub fn invite_to_party(game: &mut Game, player_id: CreatureId, invited_id: CreatureId) {
    if player_id == invited_id { return; }
    if game.get_player(invited_id).is_none() { return; }
    if game.get_player(invited_id).and_then(|p| p.party_id).is_some() {
        let name = game.get_player(invited_id).map(|p| p.name.clone()).unwrap_or_default();
        player_text(game, player_id, &format!("{name} is already in a party."));
        return;
    }
    let player_party = game.get_player(player_id).and_then(|p| p.party_id);
    let leader_id = match player_party {
        None => {
            game.create_party(player_id);
            player_id
        }
        Some(lid) => {
            if lid != player_id { return; }
            lid
        }
    };

    let was_empty = game.get_party(leader_id).map(|p| p.is_empty()).unwrap_or(true);
    if game.get_party(leader_id).map(|p| p.is_player_invited(invited_id)).unwrap_or(false) { return; }
    if let Some(p) = game.get_party_mut(leader_id) {
        p.invite_ids_mut().push(invited_id);
    }
    if let Some(p) = game.get_player_mut(invited_id) {
        if !p.invite_party_list.contains(&leader_id) {
            p.invite_party_list.push(leader_id);
        }
    }

    let invited_name = game.get_player(invited_id).map(|p| p.name.clone()).unwrap_or_default();
    let leader_name = game.get_player(leader_id).map(|p| p.name.clone()).unwrap_or_default();
    if was_empty {
        player_text(game, leader_id, &format!("{invited_name} has been invited. Open the party channel to communicate with your members."));
        update_player_shield(game, leader_id);
    } else {
        player_text(game, leader_id, &format!("{invited_name} has been invited."));
    }
    send_creature_shield(game, leader_id, invited_id);
    send_creature_shield(game, invited_id, leader_id);
    let his = leader_possessive(game, leader_id);
    player_text(game, invited_id, &format!("{leader_name} has invited you to {his} party."));
}

pub fn join_party(game: &mut Game, player_id: CreatureId, leader_id: CreatureId) {
    let invited = game.get_party(leader_id).map(|p| p.is_player_invited(player_id)).unwrap_or(false);
    if !invited { return; }
    if game.get_player(player_id).and_then(|p| p.party_id).is_some() {
        player_text(game, player_id, "You are already in a party.");
        return;
    }

    let name = game.get_player(player_id).map(|p| p.name.clone()).unwrap_or_default();
    broadcast_message(game, leader_id, &format!("{name} has joined the party."), false);

    if let Some(p) = game.get_party_mut(leader_id) {
        p.invite_ids_mut().retain(|&id| id != player_id);
        p.member_ids_mut().push(player_id);
    }
    if let Some(p) = game.get_player_mut(player_id) {
        p.party_id = Some(leader_id);
        p.invite_party_list.retain(|&id| id != leader_id);
    }
    update_player_shield(game, player_id);
    update_all_icons(game, leader_id);
    update_shared_experience(game, leader_id);

    let leader_name = game.get_player(leader_id).map(|p| p.name.clone()).unwrap_or_default();
    let suffix = if leader_name.ends_with('s') { "" } else { "s" };
    player_text(game, player_id, &format!("You have joined {leader_name}'{suffix} party. Open the party channel to communicate with your companions."));
}

fn remove_invite(game: &mut Game, leader_id: CreatureId, invited_id: CreatureId) {
    let was_invited = game.get_party(leader_id).map(|p| p.is_player_invited(invited_id)).unwrap_or(false);
    if !was_invited { return; }
    if let Some(p) = game.get_party_mut(leader_id) {
        p.invite_ids_mut().retain(|&id| id != invited_id);
    }
    if let Some(p) = game.get_player_mut(invited_id) {
        p.invite_party_list.retain(|&id| id != leader_id);
    }
    send_creature_shield(game, leader_id, invited_id);
    send_creature_shield(game, invited_id, leader_id);
    if game.get_party(leader_id).map(|p| p.is_empty()).unwrap_or(true) {
        disband_party(game, leader_id);
    }
}

pub fn revoke_party_invitation(game: &mut Game, player_id: CreatureId, invited_id: CreatureId) {
    let party_leader = game.get_player(player_id).and_then(|p| p.party_id);
    let Some(leader_id) = party_leader else { return };
    if leader_id != player_id { return; }
    if !game.get_party(leader_id).map(|p| p.is_player_invited(invited_id)).unwrap_or(false) { return; }
    let invited_name = game.get_player(invited_id).map(|p| p.name.clone()).unwrap_or_default();
    let his = leader_possessive(game, leader_id);
    let leader_name = game.get_player(leader_id).map(|p| p.name.clone()).unwrap_or_default();
    player_text(game, invited_id, &format!("{leader_name} has revoked {his} invitation."));
    player_text(game, leader_id, &format!("Invitation for {invited_name} has been revoked."));
    remove_invite(game, leader_id, invited_id);
}

pub fn pass_party_leadership(game: &mut Game, player_id: CreatureId, new_leader_id: CreatureId) {
    let party_leader = game.get_player(player_id).and_then(|p| p.party_id);
    let Some(leader_id) = party_leader else { return };
    if leader_id != player_id { return; }
    let is_member = game.get_party(leader_id).map(|p| p.get_members().contains(&new_leader_id)).unwrap_or(false);
    if !is_member { return; }

    let new_name = game.get_player(new_leader_id).map(|p| p.name.clone()).unwrap_or_default();
    broadcast_message(game, leader_id, &format!("{new_name} is now the leader of the party."), true);

    if let Some(mut party) = game.parties.remove(&leader_id) {
        party.member_ids_mut().retain(|&id| id != new_leader_id);
        party.member_ids_mut().insert(0, leader_id);
        party.set_leader_id(new_leader_id);
        game.parties.insert(new_leader_id, party);
    }
    let members = game.get_party(new_leader_id).map(|p| p.get_members().to_vec()).unwrap_or_default();
    for m in &members {
        if let Some(p) = game.get_player_mut(*m) {
            p.party_id = Some(new_leader_id);
        }
    }
    if let Some(p) = game.get_player_mut(new_leader_id) {
        p.party_id = Some(new_leader_id);
    }

    update_shared_experience(game, new_leader_id);
    update_all_icons(game, new_leader_id);
    player_text(game, new_leader_id, "You are now the leader of the party.");
}

pub fn leave_party(game: &mut Game, player_id: CreatureId) {
    let party_leader = game.get_player(player_id).and_then(|p| p.party_id);
    let Some(leader_id) = party_leader else { return };
    let in_fight = game.get_player(player_id)
        .map(|p| p.base.has_condition(crate::combat::condition::ConditionType::InFight))
        .unwrap_or(false);
    if in_fight { return; }

    let mut missing_leader = false;
    if leader_id == player_id {
        let members = game.get_party(leader_id).map(|p| p.get_members().to_vec()).unwrap_or_default();
        let invitees_empty = game.get_party(leader_id).map(|p| p.get_invitees().is_empty()).unwrap_or(true);
        if !members.is_empty() {
            if members.len() == 1 && invitees_empty {
                missing_leader = true;
            } else {
                pass_party_leadership(game, leader_id, members[0]);
            }
        } else {
            missing_leader = true;
        }
    }

    let cur_leader = game.get_player(player_id).and_then(|p| p.party_id).unwrap_or(leader_id);
    if let Some(p) = game.get_party_mut(cur_leader) {
        p.member_ids_mut().retain(|&id| id != player_id);
    }
    if let Some(p) = game.get_player_mut(player_id) {
        p.party_id = None;
    }
    update_player_shield(game, player_id);

    let members = game.get_party(cur_leader).map(|p| p.get_members().to_vec()).unwrap_or_default();
    for m in &members {
        send_creature_shield(game, *m, player_id);
        send_creature_shield(game, player_id, *m);
    }
    send_creature_shield(game, cur_leader, player_id);
    send_creature_shield(game, player_id, player_id);
    send_creature_shield(game, player_id, cur_leader);

    player_text(game, player_id, "You have left the party.");
    update_shared_experience(game, cur_leader);
    if let Some(p) = game.get_party_mut(cur_leader) {
        p.clear_player_points(player_id);
    }
    let name = game.get_player(player_id).map(|p| p.name.clone()).unwrap_or_default();
    broadcast_message(game, cur_leader, &format!("{name} has left the party."), false);

    let empty = game.get_party(cur_leader).map(|p| p.is_empty()).unwrap_or(true);
    if missing_leader || empty {
        disband_party(game, cur_leader);
    }
}

pub fn disband_party(game: &mut Game, leader_id: CreatureId) {
    let (members, invitees) = match game.get_party(leader_id) {
        Some(p) => (p.get_members().to_vec(), p.get_invitees().to_vec()),
        None => return,
    };

    player_text(game, leader_id, "Your party has been disbanded.");
    for &invitee in &invitees {
        if let Some(p) = game.get_player_mut(invitee) {
            p.invite_party_list.retain(|&id| id != leader_id);
        }
        send_creature_shield(game, leader_id, invitee);
    }
    for &member in &members {
        if let Some(p) = game.get_player_mut(member) {
            p.party_id = None;
        }
        player_text(game, member, "Your party has been disbanded.");
    }
    if let Some(p) = game.get_player_mut(leader_id) {
        p.party_id = None;
    }

    game.parties.remove(&leader_id);

    update_player_shield(game, leader_id);
    for &member in &members {
        update_player_shield(game, member);
    }
}

pub fn enable_shared_party_experience(game: &mut Game, player_id: CreatureId, active: bool) {
    let party_leader = game.get_player(player_id).and_then(|p| p.party_id);
    let Some(leader_id) = party_leader else { return };
    if leader_id != player_id { return; }
    let in_fight = game.get_player(player_id)
        .map(|p| p.base.has_condition(crate::combat::condition::ConditionType::InFight))
        .unwrap_or(false);
    let in_pz = game.get_player(player_id)
        .map(|p| {
            game.map.get_tile(p.base.position)
                .map(|t| t.has_flag(crate::map::tile::TILESTATE_PROTECTIONZONE))
                .unwrap_or(false)
        })
        .unwrap_or(false);
    if in_fight && !in_pz { return; }

    let prev_active = game.get_party(leader_id).map(|p| p.is_shared_experience_active()).unwrap_or(false);
    if prev_active == active { return; }
    if let Some(p) = game.get_party_mut(leader_id) {
        p.set_shared_exp_active(active);
    }
    if active {
        let enabled = can_enable_shared_experience(game, leader_id);
        if let Some(p) = game.get_party_mut(leader_id) {
            p.set_shared_exp_enabled(enabled);
        }
        if enabled {
            player_text(game, leader_id, "Shared Experience is now active.");
        } else {
            player_text(game, leader_id, "Shared Experience has been activated, but some members of your party are inactive.");
        }
    } else {
        player_text(game, leader_id, "Shared Experience has been deactivated.");
    }
    update_all_icons(game, leader_id);
}
