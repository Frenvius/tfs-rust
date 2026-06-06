use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

use crate::util::json5::{self, Json5LoadError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    pub flags: u64,
    pub max_depot_items: u32,
    pub max_vip_entries: u32,
    pub id: u16,
    pub access: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Groups {
    groups: Vec<Group>,
}

impl Groups {
    pub fn load_from_json5(path: impl AsRef<Path>) -> Result<Self, GroupsError> {
        let data: GroupsJson5 = json5::load_from_path(path)?;
        let mut groups = Vec::with_capacity(data.groups.len());

        for entry in data.groups {
            let mut flags = entry.base_flags.unwrap_or(0);
            apply_flag_nodes(&mut flags, entry.flags.as_deref())?;

            groups.push(Group {
                name: entry.name,
                flags,
                max_depot_items: entry.maxdepotitems,
                max_vip_entries: entry.maxvipentries,
                id: entry.id,
                access: entry.access,
            });
        }

        Ok(Self { groups })
    }

    pub fn get_group(&self, id: u16) -> Option<&Group> {
        self.groups.iter().find(|group| group.id == id)
    }
}

/// Returns the `access` flag for a group_id, matching groups.xml/groups.json5.
/// Only gamemaster (4) and community manager (5) are access groups; god (6) is
/// NOT, matching upstream. Mirrors C++ `Player::isAccessPlayer` (group->access).
pub fn access_for_group_id(group_id: u32) -> bool {
    matches!(group_id, 4 | 5)
}

/// Returns hardcoded group flags for a given group_id, matching data/XML/groups.xml.
/// Used when groups.json5 has not been generated yet.
pub fn flags_for_group_id(group_id: u32) -> u64 {
    use crate::creatures::player::*;
    match group_id {
        1 => 0,
        2 => PLAYER_FLAG_TALK_ORANGE_HELP_CHANNEL | PLAYER_FLAG_CANNOT_BE_MUTED,
        3 => {
            PLAYER_FLAG_TALK_ORANGE_HELP_CHANNEL
                | PLAYER_FLAG_CAN_TALK_RED_PRIVATE
                | PLAYER_FLAG_CAN_TALK_RED_CHANNEL
                | PLAYER_FLAG_CANNOT_BE_MUTED
        }
        4 => {
            // gamemaster: cannotpickupitem=1, hasinfinitecapacity=1, no combat
            PLAYER_FLAG_CANNOT_USE_COMBAT
                | PLAYER_FLAG_CANNOT_ATTACK_PLAYER
                | PLAYER_FLAG_CANNOT_ATTACK_MONSTER
                | PLAYER_FLAG_CANNOT_BE_ATTACKED
                | PLAYER_FLAG_CAN_ILLUSION_ALL
                | PLAYER_FLAG_CAN_SENSE_INVISIBILITY
                | PLAYER_FLAG_IGNORED_BY_MONSTERS
                | PLAYER_FLAG_NOT_GAIN_IN_FIGHT
                | PLAYER_FLAG_HAS_NO_EXHAUSTION
                | PLAYER_FLAG_CANNOT_USE_SPELLS
                | PLAYER_FLAG_CANNOT_PICKUP_ITEM
                | PLAYER_FLAG_CAN_ALWAYS_LOGIN
                | PLAYER_FLAG_CAN_BROADCAST
                | PLAYER_FLAG_CANNOT_BE_BANNED
                | PLAYER_FLAG_CANNOT_BE_PUSHED
                | PLAYER_FLAG_HAS_INFINITE_CAPACITY
                | PLAYER_FLAG_CAN_PUSH_ALL_CREATURES
                | PLAYER_FLAG_CAN_TALK_RED_PRIVATE
                | PLAYER_FLAG_CAN_TALK_RED_CHANNEL
                | PLAYER_FLAG_TALK_ORANGE_HELP_CHANNEL
                | PLAYER_FLAG_NOT_GAIN_EXPERIENCE
                | PLAYER_FLAG_NOT_GAIN_MANA
                | PLAYER_FLAG_NOT_GAIN_HEALTH
                | PLAYER_FLAG_NOT_GAIN_SKILL
                | PLAYER_FLAG_SET_MAX_SPEED
                | PLAYER_FLAG_SPECIAL_VIP
                | PLAYER_FLAG_NOT_GENERATE_LOOT
                | PLAYER_FLAG_IGNORE_PROTECTION_ZONE
                | PLAYER_FLAG_IGNORE_SPELL_CHECK
                | PLAYER_FLAG_IGNORE_WEAPON_CHECK
                | PLAYER_FLAG_CANNOT_BE_MUTED
                | PLAYER_FLAG_IS_ALWAYS_PREMIUM
                | PLAYER_FLAG_IGNORE_YELL_CHECK
                | PLAYER_FLAG_IGNORE_SEND_PRIVATE_CHECK
        }
        5 => {
            // community manager: same as god but without edithouses and notgenerateloot=1
            PLAYER_FLAG_CANNOT_ATTACK_PLAYER
                | PLAYER_FLAG_CANNOT_BE_ATTACKED
                | PLAYER_FLAG_CAN_CONVINCE_ALL
                | PLAYER_FLAG_CAN_SUMMON_ALL
                | PLAYER_FLAG_CAN_ILLUSION_ALL
                | PLAYER_FLAG_CAN_SENSE_INVISIBILITY
                | PLAYER_FLAG_IGNORED_BY_MONSTERS
                | PLAYER_FLAG_NOT_GAIN_IN_FIGHT
                | PLAYER_FLAG_HAS_INFINITE_MANA
                | PLAYER_FLAG_HAS_INFINITE_SOUL
                | PLAYER_FLAG_HAS_NO_EXHAUSTION
                | PLAYER_FLAG_CAN_ALWAYS_LOGIN
                | PLAYER_FLAG_CAN_BROADCAST
                | PLAYER_FLAG_CANNOT_BE_BANNED
                | PLAYER_FLAG_CANNOT_BE_PUSHED
                | PLAYER_FLAG_HAS_INFINITE_CAPACITY
                | PLAYER_FLAG_CAN_PUSH_ALL_CREATURES
                | PLAYER_FLAG_CAN_TALK_RED_PRIVATE
                | PLAYER_FLAG_CAN_TALK_RED_CHANNEL
                | PLAYER_FLAG_TALK_ORANGE_HELP_CHANNEL
                | PLAYER_FLAG_NOT_GAIN_EXPERIENCE
                | PLAYER_FLAG_NOT_GAIN_MANA
                | PLAYER_FLAG_NOT_GAIN_HEALTH
                | PLAYER_FLAG_NOT_GAIN_SKILL
                | PLAYER_FLAG_SET_MAX_SPEED
                | PLAYER_FLAG_SPECIAL_VIP
                | PLAYER_FLAG_NOT_GENERATE_LOOT
                | PLAYER_FLAG_IGNORE_PROTECTION_ZONE
                | PLAYER_FLAG_IGNORE_SPELL_CHECK
                | PLAYER_FLAG_IGNORE_WEAPON_CHECK
                | PLAYER_FLAG_CANNOT_BE_MUTED
                | PLAYER_FLAG_IS_ALWAYS_PREMIUM
                | PLAYER_FLAG_IGNORE_YELL_CHECK
                | PLAYER_FLAG_IGNORE_SEND_PRIVATE_CHECK
        }
        _ => {
            // god (and any unknown high group)
            PLAYER_FLAG_CANNOT_ATTACK_PLAYER
                | PLAYER_FLAG_CANNOT_BE_ATTACKED
                | PLAYER_FLAG_CAN_CONVINCE_ALL
                | PLAYER_FLAG_CAN_SUMMON_ALL
                | PLAYER_FLAG_CAN_ILLUSION_ALL
                | PLAYER_FLAG_CAN_SENSE_INVISIBILITY
                | PLAYER_FLAG_IGNORED_BY_MONSTERS
                | PLAYER_FLAG_NOT_GAIN_IN_FIGHT
                | PLAYER_FLAG_HAS_INFINITE_MANA
                | PLAYER_FLAG_HAS_INFINITE_SOUL
                | PLAYER_FLAG_HAS_NO_EXHAUSTION
                | PLAYER_FLAG_CAN_ALWAYS_LOGIN
                | PLAYER_FLAG_CAN_BROADCAST
                | PLAYER_FLAG_CAN_EDIT_HOUSES
                | PLAYER_FLAG_CANNOT_BE_BANNED
                | PLAYER_FLAG_CANNOT_BE_PUSHED
                | PLAYER_FLAG_HAS_INFINITE_CAPACITY
                | PLAYER_FLAG_CAN_PUSH_ALL_CREATURES
                | PLAYER_FLAG_CAN_TALK_RED_PRIVATE
                | PLAYER_FLAG_CAN_TALK_RED_CHANNEL
                | PLAYER_FLAG_TALK_ORANGE_HELP_CHANNEL
                | PLAYER_FLAG_NOT_GAIN_EXPERIENCE
                | PLAYER_FLAG_NOT_GAIN_MANA
                | PLAYER_FLAG_NOT_GAIN_HEALTH
                | PLAYER_FLAG_NOT_GAIN_SKILL
                | PLAYER_FLAG_SET_MAX_SPEED
                | PLAYER_FLAG_SPECIAL_VIP
                | PLAYER_FLAG_IGNORE_PROTECTION_ZONE
                | PLAYER_FLAG_IGNORE_SPELL_CHECK
                | PLAYER_FLAG_IGNORE_WEAPON_CHECK
                | PLAYER_FLAG_CANNOT_BE_MUTED
                | PLAYER_FLAG_IS_ALWAYS_PREMIUM
                | PLAYER_FLAG_IGNORE_YELL_CHECK
                | PLAYER_FLAG_IGNORE_SEND_PRIVATE_CHECK
        }
    }
}

#[derive(Debug, Error)]
pub enum GroupsError {
    #[error(transparent)]
    Json5(#[from] Json5LoadError),
    #[error("invalid group flags payload")]
    InvalidFlagsPayload,
}

#[derive(Debug, Deserialize)]
struct GroupsJson5 {
    #[serde(default)]
    groups: Vec<GroupJson5>,
}

#[derive(Debug, Deserialize)]
struct GroupJson5 {
    id: u16,
    name: String,
    access: bool,
    maxdepotitems: u32,
    maxvipentries: u32,
    #[serde(default, alias = "flagsValue")]
    base_flags: Option<u64>,
    #[serde(default)]
    flags: Option<Vec<Value>>,
}

fn apply_flag_nodes(flags: &mut u64, nodes: Option<&[Value]>) -> Result<(), GroupsError> {
    let Some(nodes) = nodes else {
        return Ok(());
    };

    let parse_flags = parse_player_flag_map();
    for node in nodes {
        let Value::Object(map) = node else {
            return Err(GroupsError::InvalidFlagsPayload);
        };

        for (name, enabled) in map {
            if !enabled.as_bool().unwrap_or(false) {
                continue;
            }
            if let Some(flag) = parse_flags.get(&name.to_ascii_lowercase()) {
                *flags |= *flag;
            }
        }
    }

    Ok(())
}

fn parse_player_flag_map() -> HashMap<String, u64> {
    HashMap::from([
        (String::from("cannotusecombat"), 1 << 0),
        (String::from("cannotattackplayer"), 1 << 1),
        (String::from("cannotattackmonster"), 1 << 2),
        (String::from("cannotbeattacked"), 1 << 3),
        (String::from("canconvinceall"), 1 << 4),
        (String::from("cansummonall"), 1 << 5),
        (String::from("canillusionall"), 1 << 6),
        (String::from("cansenseinvisibility"), 1 << 7),
        (String::from("ignoredbymonsters"), 1 << 8),
        (String::from("notgaininfight"), 1 << 9),
        (String::from("hasinfinitemana"), 1 << 10),
        (String::from("hasinfinitesoul"), 1 << 11),
        (String::from("hasnoexhaustion"), 1 << 12),
        (String::from("cannotusespells"), 1 << 13),
        (String::from("cannotpickupitem"), 1 << 14),
        (String::from("canalwayslogin"), 1 << 15),
        (String::from("canbroadcast"), 1 << 16),
        (String::from("canedithouses"), 1 << 17),
        (String::from("cannotbebanned"), 1 << 18),
        (String::from("cannotbepushed"), 1 << 19),
        (String::from("hasinfinitecapacity"), 1 << 20),
        (String::from("canpushallcreatures"), 1 << 21),
        (String::from("cantalkredprivate"), 1 << 22),
        (String::from("cantalkredchannel"), 1 << 23),
        (String::from("talkorangehelpchannel"), 1 << 24),
        (String::from("notgainexperience"), 1 << 25),
        (String::from("notgainmana"), 1 << 26),
        (String::from("notgainhealth"), 1 << 27),
        (String::from("notgainskill"), 1 << 28),
        (String::from("setmaxspeed"), 1 << 29),
        (String::from("specialvip"), 1 << 30),
        (String::from("notgenerateloot"), 1u64 << 31),
        (String::from("cantalkredchannelanonymous"), 1u64 << 32),
        (String::from("ignoreprotectionzone"), 1u64 << 33),
        (String::from("ignorespellcheck"), 1u64 << 34),
        (String::from("ignoreweaponcheck"), 1u64 << 35),
        (String::from("cannotbemuted"), 1u64 << 36),
        (String::from("isalwayspremium"), 1u64 << 37),
        (String::from("ignoreyellcheck"), 1u64 << 38),
        (String::from("ignoresendprivatecheck"), 1u64 << 39),
    ])
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::Groups;

    #[test]
    fn load_from_json5_should_merge_numeric_and_named_flags() {
        let path = std::env::temp_dir().join("tfs-rust-groups.json5");
        fs::write(
            &path,
            r#"
{
  groups: [
    {
      id: 1,
      name: "Tutor",
      access: true,
      maxdepotitems: 2000,
      maxvipentries: 100,
      flagsValue: 4,
      flags: [{ canbroadcast: true, cannotbemuted: true }],
    },
  ],
}
"#,
        )
        .expect("temp groups json5 should be writable");

        let groups = Groups::load_from_json5(&path).expect("groups should load");
        let group = groups.get_group(1).expect("group should exist");
        assert_ne!(group.flags & 4, 0);
        assert_ne!(group.flags & (1 << 16), 0);
        assert_ne!(group.flags & (1u64 << 36), 0);

        fs::remove_file(path).expect("temp groups json5 should be removable");
    }
}
