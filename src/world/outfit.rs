use std::path::Path;
use std::sync::OnceLock;

use serde::Deserialize;
use thiserror::Error;

use crate::util::xml::{self, XmlLoadError};
use crate::util::xml::deser::tfs_bool_opt;

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
    pub fn load_from_xml(path: impl AsRef<Path>) -> Result<Self, OutfitError> {
        let data: OutfitsXml = xml::load_from_path(path)?;
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
    Xml(#[from] XmlLoadError),
    #[error("invalid outfit type {0}")]
    InvalidOutfitType(u8),
}

#[derive(Debug, Deserialize)]
struct OutfitsXml {
    #[serde(rename = "outfit", default)]
    outfits: Vec<OutfitXml>,
}

#[derive(Debug, Deserialize)]
struct OutfitXml {
    #[serde(rename = "@enabled", default, deserialize_with = "tfs_bool_opt")]
    enabled: Option<bool>,
    #[serde(rename = "@type")]
    outfit_type: u8,
    #[serde(rename = "@looktype")]
    looktype: u16,
    #[serde(rename = "@name")]
    name: Option<String>,
    #[serde(rename = "@premium", default, deserialize_with = "tfs_bool_opt")]
    premium: Option<bool>,
    #[serde(rename = "@unlocked", default, deserialize_with = "tfs_bool_opt")]
    unlocked: Option<bool>,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{Outfits, PlayerSex};

    #[test]
    fn load_from_xml_should_filter_disabled_outfits() {
        let path = std::env::temp_dir().join("tfs-rust-outfits.xml");
        fs::write(
            &path,
            r#"
<outfits>
    <outfit type="0" looktype="128" name="Citizen" premium="no" />
    <outfit type="1" looktype="129" name="Citizen" premium="no" unlocked="yes" />
    <outfit type="0" looktype="130" name="Hidden" enabled="no" />
</outfits>
"#,
        )
        .expect("temp outfits xml should be writable");

        let outfits = Outfits::load_from_xml(&path).expect("outfits should load");
        assert_eq!(outfits.get_outfits(PlayerSex::Female).len(), 1);
        assert!(outfits.get_outfit_by_look_type_any(129).is_some());

        fs::remove_file(path).expect("temp outfits xml should be removable");
    }
}
