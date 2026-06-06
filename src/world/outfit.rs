use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Deserializer};
use thiserror::Error;

use crate::util::json5::{self, Json5LoadError};

/// Deserialize an optional boolean that the XML→JSON5 migration may emit as a
/// bool, an integer (1/0), or a string ("yes"/"no"/"true"/"false"/"1"/"0").
fn de_yesno_opt<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<bool>, D::Error> {
    struct V;
    impl<'de> serde::de::Visitor<'de> for V {
        type Value = Option<bool>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("an optional yes/no boolean")
        }
        fn visit_none<E>(self) -> Result<Option<bool>, E> { Ok(None) }
        fn visit_unit<E>(self) -> Result<Option<bool>, E> { Ok(None) }
        fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<Option<bool>, D2::Error> {
            struct Inner;
            impl serde::de::Visitor<'_> for Inner {
                type Value = bool;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("yes/no boolean")
                }
                fn visit_bool<E>(self, v: bool) -> Result<bool, E> { Ok(v) }
                fn visit_u64<E>(self, v: u64) -> Result<bool, E> { Ok(v != 0) }
                fn visit_i64<E>(self, v: i64) -> Result<bool, E> { Ok(v != 0) }
                fn visit_str<E>(self, v: &str) -> Result<bool, E> {
                    Ok(matches!(v.trim(), "1" | "yes" | "true"))
                }
            }
            d.deserialize_any(Inner).map(Some)
        }
    }
    deserializer.deserialize_option(V)
}

static G_OUTFITS: OnceLock<Outfits> = OnceLock::new();

pub fn g_outfits() -> &'static Outfits {
    G_OUTFITS.get().expect("outfits not initialized")
}

pub fn init_outfits(outfits: Outfits) {
    G_OUTFITS
        .set(outfits)
        .unwrap_or_else(|_| panic!("outfits already initialized"));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PlayerSex {
    Female = 0,
    Male = 1,
}

impl TryFrom<u8> for PlayerSex {
    type Error = OutfitError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Female),
            1 => Ok(Self::Male),
            other => Err(OutfitError::InvalidOutfitType(other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outfit {
    pub name: String,
    pub look_type: u16,
    pub premium: bool,
    pub unlocked: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outfits {
    outfits: [Vec<Outfit>; 2],
}

impl Outfits {
    pub fn load_from_json5(path: impl AsRef<Path>) -> Result<Self, OutfitError> {
        let data: OutfitsJson5 = json5::load_from_path(path)?;
        let mut outfits = [Vec::new(), Vec::new()];

        for entry in data.outfits {
            if entry.enabled == Some(false) {
                continue;
            }

            let sex = PlayerSex::try_from(entry.outfit_type)?;
            outfits[sex as usize].push(Outfit {
                name: entry.name.unwrap_or_default(),
                look_type: entry.looktype,
                premium: entry.premium.unwrap_or(false),
                unlocked: entry.unlocked.unwrap_or(true),
            });
        }

        Ok(Self { outfits })
    }

    pub fn get_outfit_by_look_type(&self, sex: PlayerSex, look_type: u16) -> Option<&Outfit> {
        self.outfits[sex as usize]
            .iter()
            .find(|outfit| outfit.look_type == look_type)
    }

    pub fn get_outfit_by_look_type_any(&self, look_type: u16) -> Option<&Outfit> {
        self.outfits
            .iter()
            .flat_map(|outfits| outfits.iter())
            .find(|outfit| outfit.look_type == look_type)
    }

    pub fn get_outfits(&self, sex: PlayerSex) -> &[Outfit] {
        &self.outfits[sex as usize]
    }
}

#[derive(Debug, Error)]
pub enum OutfitError {
    #[error(transparent)]
    Json5(#[from] Json5LoadError),
    #[error("invalid outfit type {0}")]
    InvalidOutfitType(u8),
}

#[derive(Debug, Deserialize)]
struct OutfitsJson5 {
    #[serde(default)]
    outfits: Vec<OutfitJson5>,
}

#[derive(Debug, Deserialize)]
struct OutfitJson5 {
    #[serde(default, deserialize_with = "de_yesno_opt")]
    enabled: Option<bool>,
    #[serde(rename = "type")]
    outfit_type: u8,
    looktype: u16,
    name: Option<String>,
    #[serde(default, deserialize_with = "de_yesno_opt")]
    premium: Option<bool>,
    #[serde(default, deserialize_with = "de_yesno_opt")]
    unlocked: Option<bool>,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{Outfits, PlayerSex};

    #[test]
    fn load_from_json5_should_filter_disabled_outfits() {
        let path = std::env::temp_dir().join("tfs-rust-outfits.json5");
        fs::write(
            &path,
            r#"
{
  outfits: [
    { type: 0, looktype: 128, name: "Citizen", premium: false },
    { type: 1, looktype: 129, name: "Citizen", premium: false, unlocked: true },
    { type: 0, looktype: 130, name: "Hidden", enabled: false },
  ],
}
"#,
        )
        .expect("temp outfits json5 should be writable");

        let outfits = Outfits::load_from_json5(&path).expect("outfits should load");
        assert_eq!(outfits.get_outfits(PlayerSex::Female).len(), 1);
        assert!(outfits.get_outfit_by_look_type_any(129).is_some());

        fs::remove_file(path).expect("temp outfits json5 should be removable");
    }
}
