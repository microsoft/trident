use std::{fmt::Display, str::FromStr};

use anyhow::{ensure, Error};

use crate::constants::IMAGE_SHA256_CHECKSUM_IGNORED;

use super::imaging::ImageSha256;

impl FromStr for ImageSha256 {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        Ok(if s == IMAGE_SHA256_CHECKSUM_IGNORED {
            ImageSha256::Ignored
        } else {
            ensure!(
                s.len() == 64,
                "Image SHA256 checksum must be 64 characters long"
            );
            ensure!(
                s.chars().all(|c| c.is_ascii_hexdigit()),
                "Image SHA256 checksum must be a hexadecimal string"
            );

            ImageSha256::Checksum(s.to_lowercase())
        })
    }
}

impl Display for ImageSha256 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageSha256::Checksum(s) => write!(f, "{s}"),
            ImageSha256::Ignored => write!(f, "{IMAGE_SHA256_CHECKSUM_IGNORED}"),
        }
    }
}

impl<'de> serde::Deserialize<'de> for ImageSha256 {
    fn deserialize<D>(deserializer: D) -> Result<ImageSha256, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;

        ImageSha256::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl serde::Serialize for ImageSha256 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.to_string().as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_sha256_from_str() {
        assert_eq!(
            ImageSha256::from_str("ignored").unwrap(),
            ImageSha256::Ignored
        );

        assert_eq!(
            ImageSha256::from_str("  ignored  ").unwrap(),
            ImageSha256::Ignored
        );

        // From lowercase
        assert_eq!(
            ImageSha256::from_str(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            )
            .unwrap(),
            ImageSha256::Checksum(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned()
            )
        );

        // From uppercase
        assert_eq!(
            ImageSha256::from_str(
                "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF"
            )
            .unwrap(),
            ImageSha256::Checksum(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned()
            )
        );

        // From mixed case
        assert_eq!(
            ImageSha256::from_str(
                "0123456789AbCdEf0123456789aBcDeF0123456789AbCdEf0123456789aBcDeF"
            )
            .unwrap(),
            ImageSha256::Checksum(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned()
            )
        );

        // With whitespace
        assert_eq!(
            ImageSha256::from_str(
                "  0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  "
            )
            .unwrap(),
            ImageSha256::Checksum(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned()
            )
        );
    }

    #[test]
    fn test_image_sha256_from_str_errors() {
        ImageSha256::from_str("").unwrap_err();
        ImageSha256::from_str("  ").unwrap_err();
        ImageSha256::from_str("IGNORED").unwrap_err();
        ImageSha256::from_str("???????").unwrap_err();
        ImageSha256::from_str("garbage").unwrap_err();

        // right length, but not hex
        let test_str = "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz";
        assert_eq!(test_str.len(), 64);
        ImageSha256::from_str(test_str).unwrap_err();
    }

    #[test]
    fn test_roundtrip() {
        let test_str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let sha256 = ImageSha256::from_str(test_str).unwrap();
        assert_eq!(sha256.to_string(), test_str);

        let test_str = IMAGE_SHA256_CHECKSUM_IGNORED;
        let sha256 = ImageSha256::from_str(test_str).unwrap();
        assert_eq!(sha256.to_string(), test_str);
    }
}
