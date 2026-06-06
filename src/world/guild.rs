use std::sync::Arc;

use crate::creatures::CreatureId;

#[derive(Debug, Clone)]
pub struct GuildRank {
    pub id: u32,
    pub name: String,
    pub level: u8,
}

pub type GuildRankPtr = Arc<GuildRank>;

pub type GuildWarVector = Vec<u32>;

pub struct Guild {
    pub id: u32,
    pub name: String,
    pub motd: String,
    members_online: Vec<CreatureId>,
    ranks: Vec<GuildRankPtr>,
    member_count: u32,
}

impl Guild {
    pub fn new(id: u32, name: String) -> Self {
        Self {
            id,
            name,
            motd: String::new(),
            members_online: Vec::new(),
            ranks: Vec::new(),
            member_count: 0,
        }
    }

    pub fn add_member(&mut self, player_id: CreatureId) {
        self.members_online.push(player_id);
    }

    pub fn remove_member(&mut self, player_id: CreatureId) {
        self.members_online.retain(|&id| id != player_id);
    }

    pub fn get_id(&self) -> u32 {
        self.id
    }

    pub fn get_name(&self) -> &str {
        &self.name
    }

    pub fn get_members_online(&self) -> &[CreatureId] {
        &self.members_online
    }

    pub fn get_member_count(&self) -> u32 {
        self.member_count
    }

    pub fn set_member_count(&mut self, count: u32) {
        self.member_count = count;
    }

    pub fn get_ranks(&self) -> &[GuildRankPtr] {
        &self.ranks
    }

    pub fn get_rank_by_id(&self, rank_id: u32) -> Option<GuildRankPtr> {
        self.ranks.iter().find(|r| r.id == rank_id).cloned()
    }

    pub fn get_rank_by_name(&self, name: &str) -> Option<GuildRankPtr> {
        self.ranks.iter().find(|r| r.name == name).cloned()
    }

    pub fn get_rank_by_level(&self, level: u8) -> Option<GuildRankPtr> {
        self.ranks.iter().find(|r| r.level == level).cloned()
    }

    pub fn add_rank(&mut self, rank_id: u32, name: String, level: u8) {
        self.ranks.push(Arc::new(GuildRank { id: rank_id, name, level }));
    }

    pub fn get_motd(&self) -> &str {
        &self.motd
    }

    pub fn set_motd(&mut self, motd: String) {
        self.motd = motd;
    }

    pub fn is_members_online_empty(&self) -> bool {
        self.members_online.is_empty()
    }
}
