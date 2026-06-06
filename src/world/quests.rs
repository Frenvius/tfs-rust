use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use serde::Deserialize;
use thiserror::Error;

use crate::util::json5::{self, Json5LoadError};

static G_QUESTS: OnceLock<Quests> = OnceLock::new();

pub fn g_quests() -> &'static Quests {
    G_QUESTS.get().expect("quests not initialized")
}

pub fn init_quests(quests: Quests) {
    G_QUESTS
        .set(quests)
        .unwrap_or_else(|_| panic!("quests already initialized"));
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mission {
    pub name: String,
    pub storage_id: u32,
    pub start_value: i32,
    pub end_value: i32,
    pub ignore_end_value: bool,
    pub main_description: String,
    pub descriptions: BTreeMap<i32, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quest {
    pub id: u16,
    pub name: String,
    pub start_storage_id: u32,
    pub start_storage_value: i32,
    pub missions: Vec<Mission>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Quests {
    quests: Vec<Quest>,
}

impl Quests {
    pub fn load_from_json5(path: impl AsRef<Path>) -> Result<Self, QuestError> {
        let data: QuestsJson5 = json5::load_from_path(path)?;
        let mut quests = Vec::with_capacity(data.quests.len());

        for (index, quest) in data.quests.into_iter().enumerate() {
            quests.push(Quest {
                id: u16::try_from(index + 1).unwrap_or(u16::MAX),
                name: quest.name,
                start_storage_id: quest.startstorageid as u32,
                start_storage_value: quest.startstoragevalue,
                missions: quest
                    .missions
                    .unwrap_or_default()
                    .into_iter()
                    .map(|mission| Mission {
                        name: mission.name,
                        storage_id: mission.storageid as u32,
                        start_value: mission.startvalue,
                        end_value: mission.endvalue,
                        ignore_end_value: mission.ignoreendvalue.unwrap_or(false),
                        main_description: mission.description.unwrap_or_default(),
                        descriptions: mission
                            .missionstates
                            .unwrap_or_default()
                            .into_iter()
                            .map(|state| (state.id, state.description))
                            .collect(),
                    })
                    .collect(),
            });
        }

        Ok(Self { quests })
    }

    pub fn get_quest_by_id(&self, id: u16) -> Option<&Quest> {
        self.quests.iter().find(|quest| quest.id == id)
    }

    pub fn is_quest_storage(&self, key: u32, value: i32, old_value: i32) -> bool {
        for quest in &self.quests {
            if quest.start_storage_id == key && quest.start_storage_value == value {
                return true;
            }

            for mission in &quest.missions {
                if mission.storage_id == key
                    && value >= mission.start_value
                    && value <= mission.end_value
                {
                    return mission.main_description.is_empty()
                        || old_value < mission.start_value
                        || old_value > mission.end_value;
                }
            }
        }

        false
    }

    pub fn get_quests(&self) -> &[Quest] {
        &self.quests
    }
}

impl Quest {
    pub fn is_started(&self, storage: &std::collections::HashMap<u32, i32>) -> bool {
        match storage.get(&self.start_storage_id) {
            Some(&v) => v >= self.start_storage_value,
            None => false,
        }
    }

    pub fn is_completed(&self, storage: &std::collections::HashMap<u32, i32>) -> bool {
        if !self.is_started(storage) {
            return false;
        }
        self.missions.iter().all(|m| m.is_completed(storage))
    }
}

impl Mission {
    pub fn is_started(&self, storage: &std::collections::HashMap<u32, i32>) -> bool {
        match storage.get(&self.storage_id) {
            Some(&v) => v >= self.start_value,
            None => false,
        }
    }

    pub fn is_completed(&self, storage: &std::collections::HashMap<u32, i32>) -> bool {
        if self.ignore_end_value {
            return false;
        }
        match storage.get(&self.storage_id) {
            Some(&v) => v >= self.end_value,
            None => false,
        }
    }

    pub fn get_description(&self, storage: &std::collections::HashMap<u32, i32>) -> &str {
        let value = storage.get(&self.storage_id).copied().unwrap_or(0);
        if let Some(desc) = self.descriptions.get(&value) {
            return desc.as_str();
        }
        for (&k, v) in self.descriptions.range(..=value).rev().take(1) {
            if k <= value {
                return v.as_str();
            }
        }
        self.main_description.as_str()
    }
}

#[derive(Debug, Error)]
pub enum QuestError {
    #[error(transparent)]
    Json5(#[from] Json5LoadError),
}

#[derive(Debug, Deserialize)]
struct QuestsJson5 {
    #[serde(default)]
    quests: Vec<QuestJson5>,
}

#[derive(Debug, Deserialize)]
struct QuestJson5 {
    name: String,
    startstorageid: i32,
    startstoragevalue: i32,
    missions: Option<Vec<MissionJson5>>,
}

#[derive(Debug, Deserialize)]
struct MissionJson5 {
    name: String,
    storageid: i32,
    startvalue: i32,
    endvalue: i32,
    ignoreendvalue: Option<bool>,
    description: Option<String>,
    missionstates: Option<Vec<MissionStateJson5>>,
}

#[derive(Debug, Deserialize)]
struct MissionStateJson5 {
    id: i32,
    description: String,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::Quests;

    #[test]
    fn load_from_json5_should_build_quest_ids_and_storage_checks() {
        let path = std::env::temp_dir().join("tfs-rust-quests.json5");
        fs::write(
            &path,
            r#"
{
  quests: [
    {
      name: "The Rookie Guard",
      startstorageid: 100,
      startstoragevalue: 1,
      missions: [
        {
          name: "Speak",
          storageid: 101,
          startvalue: 1,
          endvalue: 3,
          missionstates: [{ id: 1, description: "Hello" }],
        },
      ],
    },
  ],
}
"#,
        )
        .expect("temp quests json5 should be writable");

        let quests = Quests::load_from_json5(&path).expect("quests should load");
        assert_eq!(
            quests.get_quest_by_id(1).map(|quest| quest.name.as_str()),
            Some("The Rookie Guard")
        );
        assert!(quests.is_quest_storage(100, 1, 0));
        assert!(quests.is_quest_storage(101, 2, 0));

        fs::remove_file(path).expect("temp quests json5 should be removable");
    }
}
