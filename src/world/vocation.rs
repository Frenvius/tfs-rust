use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use serde::Deserialize;
use thiserror::Error;

use crate::util::json5::{self, Json5LoadError};

const VOCATION_NONE: u32 = 0;
const SKILL_LAST: usize = 6;
const MINIMUM_SKILL_LEVEL: u16 = 10;
const SKILL_BASE: [u32; SKILL_LAST + 1] = [50, 50, 50, 50, 30, 100, 20];

static G_VOCATIONS: OnceLock<Vocations> = OnceLock::new();

pub fn g_vocations() -> &'static Vocations {
    G_VOCATIONS.get().expect("vocations not initialized")
}

pub fn init_vocations(vocations: Vocations) {
    G_VOCATIONS
        .set(vocations)
        .unwrap_or_else(|_| panic!("vocations already initialized"));
}

#[derive(Debug, Clone, PartialEq)]
pub struct Vocation {
    pub id: u16,
    pub name: String,
    pub description: String,
    pub client_id: u16,
    pub gain_hp: u32,
    pub gain_mana: u32,
    pub gain_cap: u32,
    pub gain_health_ticks: u32,
    pub gain_health_amount: u32,
    pub gain_mana_ticks: u32,
    pub gain_mana_amount: u32,
    pub gain_soul_ticks: u16,
    pub soul_max: u8,
    pub from_vocation: u32,
    pub attack_speed: u32,
    pub base_speed: u32,
    pub no_pong_kick_time: u32,
    pub allow_pvp: bool,
    pub melee_damage_multiplier: f32,
    pub dist_damage_multiplier: f32,
    pub defense_multiplier: f32,
    pub armor_multiplier: f32,
    pub skill_multipliers: [f64; SKILL_LAST + 1],
    pub mana_multiplier: f32,
}

impl Default for Vocation {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::from("none"),
            description: String::new(),
            client_id: 0,
            gain_hp: 5,
            gain_mana: 5,
            gain_cap: 500,
            gain_health_ticks: 6,
            gain_health_amount: 1,
            gain_mana_ticks: 6,
            gain_mana_amount: 1,
            gain_soul_ticks: 120,
            soul_max: 100,
            from_vocation: VOCATION_NONE,
            attack_speed: 1500,
            base_speed: 220,
            no_pong_kick_time: 60000,
            allow_pvp: true,
            melee_damage_multiplier: 1.0,
            dist_damage_multiplier: 1.0,
            defense_multiplier: 1.0,
            armor_multiplier: 1.0,
            skill_multipliers: [1.5, 2.0, 2.0, 2.0, 2.0, 1.5, 1.1],
            mana_multiplier: 4.0,
        }
    }
}

impl Vocation {
    pub fn get_req_skill_tries(&self, skill: u8, level: u16) -> u64 {
        let skill = usize::from(skill);
        if skill > SKILL_LAST {
            return 0;
        }

        u64::from(SKILL_BASE[skill])
            * self.skill_multipliers[skill]
                .powi(i32::from(level.saturating_sub(MINIMUM_SKILL_LEVEL + 1)))
                .round() as u64
    }

    pub fn get_req_mana(&self, magic_level: u32) -> u64 {
        if magic_level == 0 {
            return 0;
        }

        (1600.0f64 * f64::from(self.mana_multiplier).powi((magic_level - 1) as i32)).round() as u64
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Vocations {
    vocations: BTreeMap<u16, Vocation>,
}

impl Vocations {
    pub fn load_from_json5(path: impl AsRef<Path>) -> Result<Self, VocationError> {
        let data: VocationsJson5 = json5::load_from_path(path)?;
        let mut vocations = BTreeMap::new();

        for entry in data.vocations {
            let mut vocation = Vocation {
                id: entry.id,
                name: entry.name.unwrap_or_else(|| String::from("none")),
                description: entry.description.unwrap_or_default(),
                client_id: entry.clientid.unwrap_or(0),
                gain_hp: entry.gainhp.unwrap_or(5),
                gain_mana: entry.gainmana.unwrap_or(5),
                gain_cap: entry.gaincap.unwrap_or(5) * 100,
                gain_health_ticks: entry.gainhpticks.unwrap_or(6),
                gain_health_amount: entry.gainhpamount.unwrap_or(1),
                gain_mana_ticks: entry.gainmanaticks.unwrap_or(6),
                gain_mana_amount: entry.gainmanaamount.unwrap_or(1),
                gain_soul_ticks: entry.gainsoulticks.unwrap_or(120),
                soul_max: entry.soulmax.unwrap_or(100),
                from_vocation: entry.fromvoc.unwrap_or(VOCATION_NONE),
                attack_speed: entry.attackspeed.unwrap_or(1500),
                base_speed: entry.basespeed.unwrap_or(220),
                no_pong_kick_time: entry.nopongkicktime.unwrap_or(60) * 1000,
                allow_pvp: entry.allowpvp.unwrap_or(true),
                mana_multiplier: entry.manamultiplier.unwrap_or(4.0),
                ..Vocation::default()
            };

            for skill in entry.skills.unwrap_or_default() {
                let skill_id = usize::from(skill.id);
                if skill_id <= SKILL_LAST {
                    vocation.skill_multipliers[skill_id] = skill.multiplier;
                }
            }

            if let Some(formula) = entry.formula {
                if let Some(value) = formula.melee_damage {
                    vocation.melee_damage_multiplier = value;
                }
                if let Some(value) = formula.dist_damage {
                    vocation.dist_damage_multiplier = value;
                }
                if let Some(value) = formula.defense {
                    vocation.defense_multiplier = value;
                }
                if let Some(value) = formula.armor {
                    vocation.armor_multiplier = value;
                }
            }

            vocations.insert(vocation.id, vocation);
        }

        Ok(Self { vocations })
    }

    pub fn get_vocation(&self, id: u16) -> Option<&Vocation> {
        self.vocations.get(&id)
    }

    pub fn get_vocation_id(&self, name: &str) -> Option<u16> {
        self.vocations
            .iter()
            .find(|(_, vocation)| vocation.name.eq_ignore_ascii_case(name))
            .map(|(id, _)| *id)
    }

    pub fn get_promoted_vocation(&self, id: u16) -> u16 {
        self.vocations
            .iter()
            .find(|(other_id, vocation)| {
                vocation.from_vocation == u32::from(id) && **other_id != id
            })
            .map(|(other_id, _)| *other_id)
            .unwrap_or(0)
    }
}

#[derive(Debug, Error)]
pub enum VocationError {
    #[error(transparent)]
    Json5(#[from] Json5LoadError),
}

#[derive(Debug, Deserialize)]
struct VocationsJson5 {
    #[serde(default)]
    vocations: Vec<VocationJson5>,
}

#[derive(Debug, Deserialize)]
struct VocationJson5 {
    id: u16,
    name: Option<String>,
    allowpvp: Option<bool>,
    clientid: Option<u16>,
    description: Option<String>,
    gaincap: Option<u32>,
    gainhp: Option<u32>,
    gainmana: Option<u32>,
    gainhpticks: Option<u32>,
    gainhpamount: Option<u32>,
    gainmanaticks: Option<u32>,
    gainmanaamount: Option<u32>,
    manamultiplier: Option<f32>,
    attackspeed: Option<u32>,
    basespeed: Option<u32>,
    soulmax: Option<u8>,
    gainsoulticks: Option<u16>,
    fromvoc: Option<u32>,
    nopongkicktime: Option<u32>,
    skills: Option<Vec<VocationSkillJson5>>,
    formula: Option<VocationFormulaJson5>,
}

#[derive(Debug, Deserialize)]
struct VocationSkillJson5 {
    id: u8,
    multiplier: f64,
}

#[derive(Debug, Deserialize)]
struct VocationFormulaJson5 {
    #[serde(rename = "meleeDamage")]
    melee_damage: Option<f32>,
    #[serde(rename = "distDamage")]
    dist_damage: Option<f32>,
    defense: Option<f32>,
    armor: Option<f32>,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::Vocations;

    #[test]
    fn load_from_json5_should_load_vocations_and_lookups() {
        let path = std::env::temp_dir().join("tfs-rust-vocations.json5");
        fs::write(
            &path,
            r#"
{
  vocations: [
    {
      id: 1,
      name: "Sorcerer",
      gaincap: 10,
      fromvoc: 0,
      skills: [{ id: 0, multiplier: 1.7 }],
      formula: { meleeDamage: 1.1, distDamage: 1.2, defense: 1.3, armor: 1.4 },
    },
    {
      id: 5,
      name: "Master Sorcerer",
      fromvoc: 1,
    },
  ],
}
"#,
        )
        .expect("temp vocations json5 should be writable");

        let vocations = Vocations::load_from_json5(&path).expect("vocations should load");
        let vocation = vocations.get_vocation(1).expect("vocation should exist");

        assert_eq!(vocation.gain_cap, 1000);
        assert_eq!(vocations.get_vocation_id("sorcerer"), Some(1));
        assert_eq!(vocations.get_promoted_vocation(1), 5);
        assert_eq!(vocation.get_req_mana(2), 6400);

        fs::remove_file(path).expect("temp vocations json5 should be removable");
    }
}
