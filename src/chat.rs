#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use serde::Deserialize;

use crate::creatures::CreatureId;

pub const CHANNEL_GUILD: u16 = 0x00;
pub const CHANNEL_PARTY: u16 = 0x01;
pub const CHANNEL_PRIVATE: u16 = 0xFFFF;

pub type UsersMap = BTreeMap<u32, CreatureId>;
pub type InvitedMap = BTreeMap<u32, CreatureId>;

static G_CHAT: OnceLock<Mutex<Chat>> = OnceLock::new();

pub fn g_chat() -> &'static Mutex<Chat> {
    G_CHAT.get_or_init(|| Mutex::new(Chat::new()))
}

#[derive(Debug, Deserialize)]
struct ChatChannelEntry {
    id: u16,
    name: String,
    public: Option<u16>,
    script: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatChannelsFile {
    channels: Vec<ChatChannelEntry>,
}

#[derive(Debug)]
pub struct ChatChannel {
    pub id: u16,
    pub name: String,
    pub public_channel: bool,
    pub users: UsersMap,
    pub can_join_event: i32,
    pub on_join_event: i32,
    pub on_leave_event: i32,
    pub on_speak_event: i32,
}

impl ChatChannel {
    pub fn new(id: u16, name: String) -> Self {
        Self {
            id,
            name,
            public_channel: false,
            users: BTreeMap::new(),
            can_join_event: -1,
            on_join_event: -1,
            on_leave_event: -1,
            on_speak_event: -1,
        }
    }

    pub fn add_user(&mut self, player_id: CreatureId, guid: u32) -> bool {
        if self.users.contains_key(&guid) {
            return false;
        }
        if !self.execute_on_join_event(player_id) {
            return false;
        }
        self.users.insert(guid, player_id);
        true
    }

    pub fn remove_user(&mut self, guid: u32) -> bool {
        if let Some(player_id) = self.users.remove(&guid) {
            self.execute_on_leave_event(player_id);
            true
        } else {
            false
        }
    }

    pub fn has_user(&self, guid: u32) -> bool {
        self.users.contains_key(&guid)
    }

    pub fn get_id(&self) -> u16 {
        self.id
    }

    pub fn get_name(&self) -> &str {
        &self.name
    }

    pub fn is_public_channel(&self) -> bool {
        self.public_channel
    }

    /// Returns the list of creature IDs (player session IDs) of channel users.
    pub fn get_user_ids(&self) -> impl Iterator<Item = CreatureId> + '_ {
        self.users.values().copied()
    }

    pub fn talk(&self, from_player_id: CreatureId, _speak_class: u8, _text: &str) -> bool {
        self.users.values().any(|&id| id == from_player_id)
    }

    pub fn send_to_all(&self, _message: &str, _speak_class: u8) {
        // Broadcast handled externally via get_user_ids().
    }

    pub fn execute_can_join_event(&self, _player_id: CreatureId) -> bool {
        if self.can_join_event == -1 {
            return true;
        }
        true
    }

    pub fn execute_on_join_event(&self, _player_id: CreatureId) -> bool {
        if self.on_join_event == -1 {
            return true;
        }
        true
    }

    pub fn execute_on_leave_event(&self, _player_id: CreatureId) -> bool {
        if self.on_leave_event == -1 {
            return true;
        }
        true
    }

    pub fn execute_on_speak_event(
        &self,
        _player_id: CreatureId,
        _speak_class: &mut u8,
        _message: &str,
    ) -> bool {
        if self.on_speak_event == -1 {
            return true;
        }
        true
    }
}

#[derive(Debug)]
pub struct PrivateChatChannel {
    pub base: ChatChannel,
    pub owner_guid: u32,
    invites: InvitedMap,
}

impl PrivateChatChannel {
    pub fn new(id: u16, name: String) -> Self {
        Self {
            base: ChatChannel::new(id, name),
            owner_guid: 0,
            invites: BTreeMap::new(),
        }
    }

    pub fn is_invited(&self, guid: u32) -> bool {
        if guid == self.owner_guid {
            return true;
        }
        self.invites.contains_key(&guid)
    }

    pub fn invite_player(
        &mut self,
        _owner_guid: u32,
        target_guid: u32,
        target_id: CreatureId,
    ) {
        self.invites.insert(target_guid, target_id);
    }

    pub fn exclude_player(&mut self, _owner_guid: u32, target_guid: u32) {
        if self.invites.remove(&target_guid).is_some() {
            self.base.remove_user(target_guid);
        }
    }

    pub fn remove_invite(&mut self, guid: u32) -> bool {
        self.invites.remove(&guid).is_some()
    }

    pub fn close_channel(&self) {
        // Notification to clients handled externally.
    }

    pub fn get_invited_users(&self) -> &InvitedMap {
        &self.invites
    }

    pub fn get_owner_guid(&self) -> u32 {
        self.owner_guid
    }

    pub fn set_owner(&mut self, guid: u32) {
        self.owner_guid = guid;
    }
}

#[derive(Debug)]
pub struct Chat {
    pub normal_channels: BTreeMap<u16, ChatChannel>,
    pub private_channels: BTreeMap<u16, PrivateChatChannel>,
    pub guild_channels: BTreeMap<u32, ChatChannel>,
    dummy_private: PrivateChatChannel,
}

impl Chat {
    pub fn new() -> Self {
        Self {
            normal_channels: BTreeMap::new(),
            private_channels: BTreeMap::new(),
            guild_channels: BTreeMap::new(),
            dummy_private: PrivateChatChannel::new(CHANNEL_PRIVATE, String::from("Private Chat Channel")),
        }
    }

    pub fn load(&mut self) -> bool {
        let path = "data/chatchannels/chatchannels.json5";
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Chat::load - cannot open {path}: {e}");
                return false;
            }
        };

        let file: ChatChannelsFile = match json5::from_str(&source) {
            Ok(f) => f,
            Err(e) => {
                tracing::error!("Chat::load - parse error in {path}: {e}");
                return false;
            }
        };

        for entry in file.channels {
            let mut channel = ChatChannel::new(entry.id, entry.name);
            if entry.public.unwrap_or(0) != 0 {
                channel.public_channel = true;
            }
            self.normal_channels.insert(entry.id, channel);
        }

        true
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_channel(
        &mut self,
        player_guid: u32,
        _player_id: CreatureId,
        channel_id: u16,
        guild_id: Option<u32>,
        guild_name: Option<&str>,
        _party_id: Option<u32>,
        is_premium: bool,
    ) -> Option<&mut ChatChannel> {
        match channel_id {
            CHANNEL_GUILD => {
                if let (Some(gid), Some(gname)) = (guild_id, guild_name) {
                    self.guild_channels
                        .entry(gid)
                        .or_insert_with(|| ChatChannel::new(channel_id, gname.to_owned()));
                    self.guild_channels.get_mut(&gid)
                } else {
                    None
                }
            }
            CHANNEL_PRIVATE => {
                if !is_premium || self.get_private_channel(player_guid).is_some() {
                    return None;
                }
                for i in 100u16..10000u16 {
                    if let std::collections::btree_map::Entry::Vacant(e) = self.private_channels.entry(i) {
                        let mut ch = PrivateChatChannel::new(i, String::new());
                        ch.set_owner(player_guid);
                        e.insert(ch);
                        return self.private_channels.get_mut(&i).map(|pc| &mut pc.base);
                    }
                }
                None
            }
            CHANNEL_PARTY => None,
            _ => None,
        }
    }

    pub fn delete_channel(
        &mut self,
        _player_guid: u32,
        channel_id: u16,
        guild_id: Option<u32>,
        _party_id: Option<u32>,
    ) -> bool {
        match channel_id {
            CHANNEL_GUILD => {
                if let Some(gid) = guild_id {
                    self.guild_channels.remove(&gid).is_some()
                } else {
                    false
                }
            }
            CHANNEL_PARTY => false,
            _ => {
                if let Some(ch) = self.private_channels.remove(&channel_id) {
                    ch.close_channel();
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn add_user_to_channel(
        &mut self,
        player_id: CreatureId,
        guid: u32,
        channel_id: u16,
        guild_id: Option<u32>,
    ) -> Option<&mut ChatChannel> {
        let channel = self.get_channel_mut(channel_id, guid, guild_id)?;
        if channel.add_user(player_id, guid) {
            Some(channel)
        } else {
            None
        }
    }

    pub fn remove_user_from_channel(
        &mut self,
        guid: u32,
        player_guid: u32,
        channel_id: u16,
        guild_id: Option<u32>,
    ) -> bool {
        let owner = if channel_id < 100 {
            0
        } else {
            self.private_channels
                .get(&channel_id)
                .map(|pc| pc.owner_guid)
                .unwrap_or(0)
        };
        let removed = if let Some(channel) = self.get_channel_mut(channel_id, guid, guild_id) {
            channel.remove_user(guid)
        } else {
            return false;
        };
        if removed && owner == player_guid {
            self.delete_channel(player_guid, channel_id, guild_id, None);
        }
        removed
    }

    pub fn remove_user_from_all_channels(&mut self, guid: u32) {
        for ch in self.normal_channels.values_mut() {
            ch.remove_user(guid);
        }
        for ch in self.guild_channels.values_mut() {
            ch.remove_user(guid);
        }
        let owner_channels: Vec<u16> = self
            .private_channels
            .iter()
            .filter(|(_, pc)| pc.owner_guid == guid)
            .map(|(id, _)| *id)
            .collect();
        let invited_channels: Vec<u16> = self
            .private_channels
            .iter()
            .filter(|(_, pc)| pc.invites.contains_key(&guid))
            .map(|(id, _)| *id)
            .collect();
        for id in &invited_channels {
            if let Some(pc) = self.private_channels.get_mut(id) {
                pc.remove_invite(guid);
                pc.base.remove_user(guid);
            }
        }
        for id in owner_channels {
            if let Some(pc) = self.private_channels.remove(&id) {
                pc.close_channel();
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn talk_to_channel(
        &mut self,
        player_id: CreatureId,
        player_guid: u32,
        mut speak_class: u8,
        text: &str,
        channel_id: u16,
        guild_rank_level: u32,
        guild_id: Option<u32>,
    ) -> bool {
        let channel = match self.get_channel_mut(channel_id, player_guid, guild_id) {
            Some(c) => c,
            None => return false,
        };

        if channel_id == CHANNEL_GUILD {
            if guild_rank_level > 1 {
                speak_class = 5; // TALKTYPE_CHANNEL_O
            } else if speak_class != 4 {
                // TALKTYPE_CHANNEL_Y
                speak_class = 4;
            }
        } else if channel_id == CHANNEL_PRIVATE || channel_id == CHANNEL_PARTY {
            speak_class = 4; // TALKTYPE_CHANNEL_Y
        }

        if !channel.execute_on_speak_event(player_id, &mut speak_class, text) {
            return false;
        }

        channel.talk(player_id, speak_class, text)
    }

    pub fn get_channel_by_id(&self, channel_id: u16) -> Option<&ChatChannel> {
        self.normal_channels.get(&channel_id)
    }

    pub fn get_guild_channel_by_id(&self, guild_id: u32) -> Option<&ChatChannel> {
        self.guild_channels.get(&guild_id)
    }

    pub fn get_private_channel(&self, player_guid: u32) -> Option<&PrivateChatChannel> {
        self.private_channels
            .values()
            .find(|pc| pc.owner_guid == player_guid)
    }

    pub fn get_private_channel_mut(&mut self, player_guid: u32) -> Option<&mut PrivateChatChannel> {
        self.private_channels
            .values_mut()
            .find(|pc| pc.owner_guid == player_guid)
    }

    /// get_channel_list — port of Chat::getChannelList from chat.cpp.
    ///
    /// Returns references to all channels the player can see:
    /// guild channel (if in a guild), party channel (if in a party),
    /// all public normal channels, invited-to private channels, and
    /// the dummy private channel placeholder when premium and without one.
    ///
    /// `is_premium` controls whether the dummy private channel slot appears.
    /// Party channel lookup requires Game integration (party pointer used as
    /// map key in C++); that branch returns an empty vec slice.
    pub fn get_channel_list_refs(
        &self,
        player_guid: u32,
        is_premium: bool,
        guild_id: Option<u32>,
        has_party: bool,
    ) -> Vec<&ChatChannel> {
        let mut list: Vec<&ChatChannel> = Vec::new();

        if let Some(gid) = guild_id {
            if let Some(ch) = self.guild_channels.get(&gid) {
                list.push(ch);
            }
        }

        if has_party {
            // party channel requires Game integration (party pointer key)
        }

        for ch in self.normal_channels.values() {
            list.push(ch);
        }

        let mut has_private = false;
        for pc in self.private_channels.values() {
            if pc.is_invited(player_guid) {
                list.push(&pc.base);
            }
            if pc.owner_guid == player_guid {
                has_private = true;
            }
        }

        if !has_private && is_premium {
            list.insert(0, &self.dummy_private.base);
        }

        list
    }

    /// get_channel — port of Chat::getChannel from chat.cpp.
    ///
    /// Returns a read-only reference to the channel the player can see for
    /// the given channel_id. Guild and party channels require Game integration
    /// for the dynamic map key; those return None here when the id matches but
    /// the necessary context is absent.
    pub fn get_channel_ref(
        &self,
        channel_id: u16,
        player_guid: u32,
        guild_id: Option<u32>,
    ) -> Option<&ChatChannel> {
        match channel_id {
            CHANNEL_GUILD => {
                let gid = guild_id?;
                self.guild_channels.get(&gid)
            }
            CHANNEL_PARTY => None,
            _ => {
                if let Some(ch) = self.normal_channels.get(&channel_id) {
                    return Some(ch);
                }
                if let Some(pc) = self.private_channels.get(&channel_id) {
                    if pc.is_invited(player_guid) {
                        return Some(&pc.base);
                    }
                }
                None
            }
        }
    }

    pub fn get_channel_list(
        &self,
        _player_id: CreatureId,
        player_guid: u32,
        is_premium: bool,
        has_guild: bool,
        has_party: bool,
        guild_id: Option<u32>,
    ) -> Vec<u16> {
        let mut list = Vec::new();

        if has_guild {
            if let Some(gid) = guild_id {
                if let Some(ch) = self.guild_channels.get(&gid) {
                    list.push(ch.id);
                }
            }
        }

        if has_party {
            list.push(CHANNEL_PARTY);
        }

        for ch in self.normal_channels.values() {
            list.push(ch.id);
        }

        let mut has_private = false;
        for pc in self.private_channels.values() {
            if pc.is_invited(player_guid) {
                list.push(pc.base.id);
            }
            if pc.owner_guid == player_guid {
                has_private = true;
            }
        }

        if !has_private && is_premium {
            list.insert(0, CHANNEL_PRIVATE);
        }

        list
    }

    pub fn get_channel_mut_by_id(
        &mut self,
        channel_id: u16,
        guid: u32,
        guild_id: Option<u32>,
    ) -> Option<&mut ChatChannel> {
        self.get_channel_mut(channel_id, guid, guild_id)
    }

    fn get_channel_mut(
        &mut self,
        channel_id: u16,
        guid: u32,
        guild_id: Option<u32>,
    ) -> Option<&mut ChatChannel> {
        match channel_id {
            CHANNEL_GUILD => {
                let gid = guild_id?;
                self.guild_channels.get_mut(&gid)
            }
            CHANNEL_PARTY => {
                None
            }
            _ => {
                if let Some(ch) = self.normal_channels.get_mut(&channel_id) {
                    return Some(ch);
                }
                if let Some(pc) = self.private_channels.get_mut(&channel_id) {
                    if pc.is_invited(guid) {
                        return Some(&mut pc.base);
                    }
                }
                None
            }
        }
    }
}

impl Default for Chat {
    fn default() -> Self {
        Self::new()
    }
}
