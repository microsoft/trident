use serde::Serialize;

/// Macro to implement `Deserialize`, `PartialEq`, and `as_str()` for a SHA2-family hash.
macro_rules! impl_common_sha2 {
    ($name:ident, $length:expr) => {
        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }

        impl PartialEq<String> for $name {
            fn eq(&self, other: &String) -> bool {
                self.0 == *other
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                $name(s.to_string())
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                $name(s)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let s = String::deserialize(deserializer)?;
                if s.len() != $length {
                    return Err(serde::de::Error::custom(format!(
                        "Invalid length {}, expected {}",
                        s.len(),
                        $length
                    )));
                }
                if !s.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(serde::de::Error::custom("Expected hexadecimal string"));
                }
                Ok($name(s))
            }
        }

        #[cfg(feature = "schemars")]
        impl schemars::JsonSchema for $name {
            fn schema_name() -> String {
                stringify!($name).to_string()
            }

            fn is_referenceable() -> bool {
                false
            }

            fn json_schema(
                generator: &mut schemars::gen::SchemaGenerator,
            ) -> schemars::schema::Schema {
                let mut base = String::json_schema(generator).into_object();
                base.format = Some(format!("[a-fA-F0-9]{}", $length));
                base.metadata().description = Some(format!(
                    "The {} is a {}-character hexadecimal string.",
                    stringify!($name),
                    $length
                ));
                base.into()
            }
        }
    };
}

/// The SHA256 checksum is a 64 character hexadecimal string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Sha256Hash(String);
impl_common_sha2!(Sha256Hash, 64);

/// The SHA384 checksum is a 96 character hexadecimal string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Sha384Hash(String);
impl_common_sha2!(Sha384Hash, 96);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_sha256() {
        let hash: Sha256Hash = serde_json::from_str(
            r#""0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef""#,
        )
        .unwrap();
        assert_eq!(
            hash.0,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
    }

    #[test]
    fn test_deserialize_sha256_invalid_length() {
        serde_json::from_str::<Sha256Hash>(
            r#""0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0""#,
        )
        .unwrap_err();
    }

    #[test]
    fn test_deserialize_sha256_invalid_hex() {
        serde_json::from_str::<Sha256Hash>(
            r#""0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeg""#,
        )
        .unwrap_err();
    }

    #[test]
    fn test_as_str() {
        let hash = Sha256Hash(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
        );
        assert_eq!(
            hash.as_str(),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
    }

    #[test]
    fn test_eq_str() {
        let hash = Sha256Hash(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
        );
        assert_eq!(
            hash,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
    }
}
