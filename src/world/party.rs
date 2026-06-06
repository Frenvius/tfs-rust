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

    pub fn get_leader_id(&self) -> CreatureId {
        self.leader_id
    }

    pub fn get_members(&self) -> &[CreatureId] {
        &self.member_ids
    }

    pub fn get_invitees(&self) -> &[CreatureId] {
        &self.invite_ids
    }

    pub fn invite_ids_mut(&mut self) -> &mut Vec<CreatureId> {
        &mut self.invite_ids
    }

    pub fn member_ids_mut(&mut self) -> &mut Vec<CreatureId> {
        &mut self.member_ids
    }

    pub fn set_shared_exp_enabled(&mut self, enabled: bool) {
        self.shared_exp_enabled = enabled;
    }

    pub fn set_shared_exp_active(&mut self, active: bool) {
        self.shared_exp_active = active;
    }

    pub fn get_player_tick(&self, player_id: CreatureId) -> Option<i64> {
        self.ticks_map.get(&player_id).copied()
    }

    pub fn set_leader_id(&mut self, leader_id: CreatureId) {
        self.leader_id = leader_id;
    }

    pub fn get_member_count(&self) -> usize {
        self.member_ids.len()
    }

    pub fn get_invitation_count(&self) -> usize {
        self.invite_ids.len()
    }

    pub fn is_player_invited(&self, player_id: CreatureId) -> bool {
        self.invite_ids.contains(&player_id)
    }

    pub fn is_empty(&self) -> bool {
        self.member_ids.is_empty() && self.invite_ids.is_empty()
    }

    pub fn can_open_corpse(&self, _owner_id: u32) -> bool {
        true
    }

    pub fn is_shared_experience_active(&self) -> bool {
        self.shared_exp_active
    }

    pub fn is_shared_experience_enabled(&self) -> bool {
        self.shared_exp_enabled
    }

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
            self.update_shared_experience();
        }
    }

    pub fn clear_player_points(&mut self, player_id: CreatureId) {
        if self.ticks_map.remove(&player_id).is_some() {
            self.update_shared_experience();
        }
    }

    pub fn can_use_shared_experience(&self, _player_id: CreatureId) -> bool {
        false
    }

    pub fn share_experience(&self, _experience: u64, _source_id: Option<CreatureId>) {
    }

    pub fn set_shared_experience(&mut self, _player_id: CreatureId, _active: bool) -> bool {
        false
    }

    pub fn update_all_party_icons(&mut self) {
    }

    pub fn broadcast_party_message(&mut self, _msg_class: u8, _text: &str) {
    }

    pub fn disband(&mut self) {
        self.member_ids.clear();
        self.invite_ids.clear();
    }

    pub fn invite_player(&mut self, _player_id: CreatureId) -> bool {
        false
    }

    pub fn join_party(&mut self, _player_id: CreatureId) -> bool {
        false
    }

    pub fn revoke_invitation(&mut self, _player_id: CreatureId) {
    }

    pub fn pass_leadership(&mut self, _player_id: CreatureId) -> bool {
        false
    }

    pub fn leave_party(&mut self, _player_id: CreatureId) -> bool {
        false
    }

    fn update_shared_experience(&mut self) {
        if self.shared_exp_active {
        }
    }
}
