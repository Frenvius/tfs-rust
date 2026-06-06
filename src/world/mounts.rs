use std::path::Path;

use serde::Deserialize;
use thiserror::Error;

use crate::util::json5::{self, Json5LoadError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub id: u8,
    pub client_id: u16,
    pub name: String,
    pub speed: i32,
    pub premium: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mounts {
    mounts: Vec<Mount>,
}

impl Mounts {
    pub fn load_from_json5(path: impl AsRef<Path>) -> Result<Self, MountError> {
        let data: MountsJson5 = json5::load_from_path(path)?;
        Ok(Self {
            mounts: data
                .mounts
                .into_iter()
                .map(|mount| Mount {
                    id: mount.id,
                    client_id: mount.clientid,
                    name: mount.name,
                    speed: mount.speed,
                    premium: mount.premium.unwrap_or(false),
                })
                .collect(),
        })
    }

    pub fn get_mount_by_id(&self, id: u8) -> Option<&Mount> {
        self.mounts.iter().find(|mount| mount.id == id)
    }

    pub fn get_mount_by_name(&self, name: &str) -> Option<&Mount> {
        self.mounts
            .iter()
            .find(|mount| mount.name.eq_ignore_ascii_case(name))
    }

    pub fn get_mount_by_client_id(&self, client_id: u16) -> Option<&Mount> {
        self.mounts
            .iter()
            .find(|mount| mount.client_id == client_id)
    }

    pub fn get_mounts(&self) -> &[Mount] {
        &self.mounts
    }
}

#[derive(Debug, Error)]
pub enum MountError {
    #[error(transparent)]
    Json5(#[from] Json5LoadError),
}

#[derive(Debug, Deserialize)]
struct MountsJson5 {
    #[serde(default)]
    mounts: Vec<MountJson5>,
}

#[derive(Debug, Deserialize)]
struct MountJson5 {
    id: u8,
    clientid: u16,
    name: String,
    speed: i32,
    premium: Option<bool>,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::Mounts;

    #[test]
    fn load_from_json5_should_support_mount_lookups() {
        let path = std::env::temp_dir().join("tfs-rust-mounts.json5");
        fs::write(
            &path,
            r#"
{
  mounts: [
    { id: 1, clientid: 368, name: "Widow Queen", speed: 20, premium: true },
  ],
}
"#,
        )
        .expect("temp mounts json5 should be writable");

        let mounts = Mounts::load_from_json5(&path).expect("mounts should load");
        assert!(mounts.get_mount_by_name("widow queen").is_some());
        assert_eq!(
            mounts.get_mount_by_client_id(368).map(|mount| mount.id),
            Some(1)
        );

        fs::remove_file(path).expect("temp mounts json5 should be removable");
    }
}
