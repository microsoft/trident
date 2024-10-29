use std::collections::HashMap;

use anyhow::{Context, Error};
use log::warn;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_yaml::Value;

/// Struct to hold free-form preview parameters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InternalParams(HashMap<String, Value>);

type Parameter<T> = Option<Result<T, Error>>;

impl InternalParams {
    /// Get the value of a key as a generic type.
    pub fn get<T>(&self, key: impl AsRef<str>) -> Parameter<T>
    where
        T: DeserializeOwned,
    {
        self.0.get(key.as_ref()).map(|v| {
            warn!(
                "USING INTERNAL OVERRIDE PARAMETER '{}':\n{:#?}",
                key.as_ref(),
                v
            );
            serde_yaml::from_value(v.clone())
                .with_context(|| format!("Failed to parse as '{}'", std::any::type_name::<T>()))
        })
    }

    /// Get the value of a key as a string.
    pub fn get_string(&self, key: impl AsRef<str>) -> Parameter<String> {
        self.get(key)
    }

    /// Get the value of a key as a vector of strings.
    pub fn get_vec_string(&self, key: impl AsRef<str>) -> Parameter<Vec<String>> {
        self.get(key)
    }

    /// Get the value of a key as a boolean.
    pub fn get_flag(&self, key: impl AsRef<str>) -> bool {
        self.get(key).transpose().ok().flatten().unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_string() {
        let params: InternalParams = serde_yaml::from_str(
            r#"
            key: value
        "#,
        )
        .unwrap();

        // Assert we get string correctly
        assert_eq!(params.get_string("key").unwrap().unwrap().as_str(), "value");

        // Assert we get None for missing key
        assert!(params.get_string("missing").is_none());
    }

    #[test]
    fn test_get_vec_string() {
        let params: InternalParams = serde_yaml::from_str(
            r#"
            key:
              - value1
              - value2
            myString: myValue
            myBadList:
              - value1
              - 1
              - -500
        "#,
        )
        .unwrap();

        // Assert we get list correctly
        assert_eq!(
            params.get_vec_string("key").unwrap().unwrap(),
            ["value1".to_string(), "value2".to_string()]
        );

        // Assert we get an error for a non-string value
        params.get_vec_string("myBadList").unwrap().unwrap_err();

        // Assert we get None for missing key
        assert!(params.get_vec_string("missing").is_none());
    }

    #[test]
    fn test_get_flag() {
        let params: InternalParams = serde_yaml::from_str(
            r#"
            key:
              - value1
              - value2
            myString: true
            x: false
        "#,
        )
        .unwrap();

        assert!(!params.get_flag("key"));
        assert!(params.get_flag("myString"));
        assert!(!params.get_flag("x"));
        assert!(!params.get_flag("missing"));
    }
}
