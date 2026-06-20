use std::fs;
use std::path::Path;

use serde::de::DeserializeOwned;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum XmlLoadError {
    #[error("failed to read `{path}`: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse XML from `{path}`: {source}")]
    Parse {
        path: String,
        #[source]
        source: quick_xml::DeError,
    },
    #[error("failed to parse XML DOM from `{path}`: {source}")]
    Dom {
        path: String,
        #[source]
        source: roxmltree::Error,
    },
}

pub fn load_from_path<T>(path: impl AsRef<Path>) -> Result<T, XmlLoadError>
where
    T: DeserializeOwned,
{
    let path = path.as_ref();
    let content = fs::read_to_string(path).map_err(|source| XmlLoadError::Read {
        path: path.display().to_string(),
        source,
    })?;

    quick_xml::de::from_str(&content).map_err(|source| XmlLoadError::Parse {
        path: path.display().to_string(),
        source,
    })
}

pub fn load_dom(path: impl AsRef<Path>) -> Result<(String, roxmltree::Document<'static>), XmlLoadError> {
    let path = path.as_ref();
    let content = fs::read_to_string(path).map_err(|source| XmlLoadError::Read {
        path: path.display().to_string(),
        source,
    })?;
    let content: &'static str = Box::leak(content.into_boxed_str());
    let doc = roxmltree::Document::parse(content).map_err(|source| XmlLoadError::Dom {
        path: path.display().to_string(),
        source,
    })?;
    Ok((path.display().to_string(), doc))
}

pub fn read_to_string(path: impl AsRef<Path>) -> Result<String, XmlLoadError> {
    let path = path.as_ref();
    fs::read_to_string(path).map_err(|source| XmlLoadError::Read {
        path: path.display().to_string(),
        source,
    })
}

pub mod deser {
    use serde::{Deserialize, Deserializer};

    fn parse_tfs_bool(s: &str) -> bool {
        matches!(s.trim(), "1" | "true" | "yes" | "TRUE" | "YES" | "True" | "Yes")
    }

    pub fn tfs_bool<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(parse_tfs_bool(&s))
    }

    pub fn tfs_bool_opt<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<bool>, D::Error> {
        let s = Option::<String>::deserialize(deserializer)?;
        Ok(s.map(|v| parse_tfs_bool(&v)))
    }

    pub fn tfs_bool_default_true<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<bool, D::Error> {
        let s = Option::<String>::deserialize(deserializer)?;
        Ok(s.map(|v| parse_tfs_bool(&v)).unwrap_or(true))
    }
}
