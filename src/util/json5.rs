use std::fs;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Json5LoadError {
    #[error("failed to read `{path}`: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse JSON5 from `{path}`: {source}")]
    Parse {
        path: String,
        #[source]
        source: json5::Error,
    },
}

pub fn load_from_path<T>(path: impl AsRef<Path>) -> Result<T, Json5LoadError>
where
    T: DeserializeOwned,
{
    let path = path.as_ref();
    let content = fs::read_to_string(path).map_err(|source| Json5LoadError::Read {
        path: path.display().to_string(),
        source,
    })?;

    json5::from_str(&content).map_err(|source| Json5LoadError::Parse {
        path: path.display().to_string(),
        source,
    })
}

pub fn load_value_from_path(path: impl AsRef<Path>) -> Result<Value, Json5LoadError> {
    load_from_path(path)
}

pub fn deserialize_bool_or_int<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct BoolOrIntVisitor;

    impl<'de> serde::de::Visitor<'de> for BoolOrIntVisitor {
        type Value = Option<bool>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a boolean or an integer (0/1)")
        }

        fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
            Ok(Some(v))
        }

        fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(Some(v != 0))
        }

        fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(Some(v != 0))
        }

        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
    }

    deserializer.deserialize_any(BoolOrIntVisitor)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde::Deserialize;

    use super::{load_from_path, load_value_from_path};

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct Example {
        id: u32,
        name: String,
    }

    #[test]
    fn load_from_path_should_deserialize_typed_json5() {
        let path = std::env::temp_dir().join("tfs-rust-json5-load-typed.json5");
        fs::write(&path, "{ id: 42, name: 'rat', }").expect("temp JSON5 file should be writable");

        let parsed: Example = load_from_path(&path).expect("typed JSON5 should parse");

        assert_eq!(
            parsed,
            Example {
                id: 42,
                name: String::from("rat"),
            }
        );

        fs::remove_file(path).expect("temp JSON5 file should be removable");
    }

    #[test]
    fn load_value_from_path_should_support_permissive_value_mode() {
        let path = std::env::temp_dir().join("tfs-rust-json5-load-value.json5");
        fs::write(&path, "{ creature: { name: 'orc' }, }")
            .expect("temp JSON5 file should be writable");

        let parsed = load_value_from_path(&path).expect("value-mode JSON5 should parse");

        assert_eq!(parsed["creature"]["name"], "orc");

        fs::remove_file(path).expect("temp JSON5 file should be removable");
    }
}
