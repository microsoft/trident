use std::{fmt::Display, num::ParseIntError, str::FromStr};

use crate::constants::PARTITION_SIZE_GROW;

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

/// Partition size enum.
/// Serialize and Deserialize traits are implemented manually in the crate::serde module.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum PartitionSize {
    /// # Grow
    ///
    /// Grow a partition to use all available space.
    ///
    /// String equivalent is defined in constants::PARTITION_SIZE_GROW
    Grow,

    /// # Fixed
    ///
    /// Fixed size in bytes.
    Fixed(u64),
    // Not implemented yet but left as a reference for the future.
    // Min(u64),
    // Max(u64),
    // MinMax(u64, u64),
}

impl FromStr for PartitionSize {
    type Err = ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        Ok(if s == PARTITION_SIZE_GROW {
            PartitionSize::Grow
        } else {
            PartitionSize::Fixed(from_human_readable(s)?)
        })
    }
}

impl From<u64> for PartitionSize {
    fn from(n: u64) -> Self {
        PartitionSize::Fixed(n)
    }
}

impl Display for PartitionSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PartitionSize::Fixed(n) => write!(f, "{}", to_human_readable(*n)),
            PartitionSize::Grow => write!(f, "{}", PARTITION_SIZE_GROW),
        }
    }
}

fn to_human_readable(x: u64) -> String {
    match x.trailing_zeros() {
        _ if x == 0 => "0".to_owned(),
        0..=9 => format!("{}", x),
        10..=19 => format!("{}K", x >> 10),
        20..=29 => format!("{}M", x >> 20),
        30..=39 => format!("{}G", x >> 30),
        _ => format!("{}T", x >> 40),
    }
}

fn from_human_readable(mut s: &str) -> Result<u64, ParseIntError> {
    s = s.trim();
    let try_parse = |val: &str, shift: u8| Ok(val.trim().parse::<u64>()? << shift);
    if let Some(p) = s.strip_suffix('K') {
        try_parse(p, 10)
    } else if let Some(p) = s.strip_suffix('M') {
        try_parse(p, 20)
    } else if let Some(p) = s.strip_suffix('G') {
        try_parse(p, 30)
    } else if let Some(p) = s.strip_suffix('T') {
        try_parse(p, 40)
    } else {
        try_parse(s, 0)
    }
}

impl<'de> serde::Deserialize<'de> for PartitionSize {
    fn deserialize<D>(deserializer: D) -> Result<PartitionSize, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Size may be provided as a string (e.g. "1K") or as a pure number
        // (e.g. 1024). Serde forces a number when only digits are provided, so
        // we need to deserialize as a generic value and then check the type.
        let value = serde_yaml::Value::deserialize(deserializer)?;

        match value {
            serde_yaml::Value::String(s) => PartitionSize::from_str(s.as_str())
                .map_err(|e| serde::de::Error::custom(format!("invalid partition size: {e}"))),
            serde_yaml::Value::Number(n) => {
                let n = n.as_u64().ok_or_else(|| {
                    serde::de::Error::custom("invalid partition size, expected unsigned integer")
                })?;
                Ok(PartitionSize::Fixed(n))
            }
            _ => Err(serde::de::Error::custom("invalid partition size")),
        }
    }
}

impl serde::Serialize for PartitionSize {
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
    fn test_from_string() {
        // Grow
        assert_eq!(
            PartitionSize::from_str(PARTITION_SIZE_GROW).unwrap(),
            PartitionSize::Grow
        );

        // Some values
        assert_eq!(
            PartitionSize::from_str("1").unwrap(),
            PartitionSize::Fixed(1)
        );
        assert_eq!(
            PartitionSize::from_str("20K").unwrap(),
            PartitionSize::Fixed(20 * 1024)
        );
        assert_eq!(
            PartitionSize::from_str("30M").unwrap(),
            PartitionSize::Fixed(30 * 1024 * 1024)
        );
        assert_eq!(
            PartitionSize::from_str("40G").unwrap(),
            PartitionSize::Fixed(40 * 1024 * 1024 * 1024)
        );
        assert_eq!(
            PartitionSize::from_str("50T").unwrap(),
            PartitionSize::Fixed(50 * 1024 * 1024 * 1024 * 1024)
        );

        // Allowed spacing
        assert_eq!(
            PartitionSize::from_str(" 1024 ").unwrap(),
            PartitionSize::Fixed(1024)
        );
        assert_eq!(
            PartitionSize::from_str(" 1K ").unwrap(),
            PartitionSize::Fixed(1024)
        );
        assert_eq!(
            PartitionSize::from_str("1 K").unwrap(),
            PartitionSize::Fixed(1024)
        );
        assert_eq!(
            PartitionSize::from_str(" 300 K ").unwrap(),
            PartitionSize::Fixed(300 * 1024)
        );

        // Invalid numbers
        assert!(PartitionSize::from_str("1.0").is_err());
        assert!(PartitionSize::from_str("1.0K").is_err());

        // Invalid spacing
        assert!(PartitionSize::from_str("1 0K").is_err());

        // Invalid units
        assert!(PartitionSize::from_str("1.0X").is_err());

        // Invalid trailing characters
        assert!(PartitionSize::from_str("1.0KX").is_err());

        // Invalid leading characters
        assert!(PartitionSize::from_str("X10K").is_err());

        // Invalid leading and trailing characters
        assert!(PartitionSize::from_str("X10KX").is_err());

        // Garbage
        assert!(PartitionSize::from_str("X").is_err());
    }

    #[test]
    fn test_to_human_readable() {
        // Some values
        assert_eq!(PartitionSize::Fixed(0).to_string(), "0");
        assert_eq!(PartitionSize::Fixed(1).to_string(), "1");
        assert_eq!(PartitionSize::Fixed(1023).to_string(), "1023");
        assert_eq!(PartitionSize::Fixed(1024).to_string(), "1K");
        assert_eq!(PartitionSize::Fixed(1025).to_string(), "1025");
        assert_eq!(PartitionSize::Fixed(1024 * 1024).to_string(), "1M");
        assert_eq!(PartitionSize::Fixed(1024 * 1024 + 1).to_string(), "1048577");
        assert_eq!(
            PartitionSize::Fixed(1024 * 1024 + 1024).to_string(),
            "1025K"
        );
        assert_eq!(PartitionSize::Fixed(1024 * 1024 * 1024).to_string(), "1G");
        assert_eq!(
            PartitionSize::Fixed(1024 * 1024 * 1024 + 1).to_string(),
            "1073741825"
        );
        assert_eq!(
            PartitionSize::Fixed(1024 * 1024 * 1024 * 1024).to_string(),
            "1T"
        );
        assert_eq!(
            PartitionSize::Fixed(1024 * 1024 * 1024 * 1024 + 1).to_string(),
            "1099511627777"
        );
    }

    #[test]
    fn test_roundtrip() {
        let test = |s: &str| {
            let n = PartitionSize::from_str(s).unwrap();
            let s2 = n.to_string();
            assert_eq!(s, s2);
        };

        test(PARTITION_SIZE_GROW);
        test("0");
        test("1");
        test("1023");
        test("1K");
        test("1025");
        test("1M");
        test("1025K");
        test("1G");
        test("1T");
    }

    #[test]
    fn test_serialization_roundtrip() {
        #[derive(Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
        struct TestStruct {
            size: PartitionSize,
        }

        impl TestStruct {
            fn fixed(v: u64) -> Self {
                Self { size: v.into() }
            }

            fn grow() -> Self {
                Self {
                    size: PartitionSize::Grow,
                }
            }
        }

        // Define test cases
        let test_cases = [
            ("size: grow", TestStruct::grow(), "size: grow"),
            ("size: 1", TestStruct::fixed(1), "size: '1'"),
            ("size: 512", TestStruct::fixed(512), "size: '512'"),
            ("size: 1K", TestStruct::fixed(1024), "size: 1K"),
            ("size: 1024", TestStruct::fixed(1024), "size: 1K"),
            ("size: 1M", TestStruct::fixed(1048576), "size: 1M"),
            ("size: 1048576", TestStruct::fixed(1048576), "size: 1M"),
            ("size: 1G", TestStruct::fixed(1073741824), "size: 1G"),
            (
                "size: 1073741824",
                TestStruct::fixed(1073741824),
                "size: 1G",
            ),
            ("size: 1024M", TestStruct::fixed(1073741824), "size: 1G"),
        ];

        // Test (de)serialization
        for (input_yaml, expected_struct, expected_yaml) in test_cases.iter() {
            let actual: TestStruct = serde_yaml::from_str(input_yaml).unwrap();
            assert_eq!(
                actual, *expected_struct,
                "failed to deserialize '{input_yaml}'"
            );

            let actual = serde_yaml::to_string(&actual).unwrap();
            assert_eq!(
                actual.trim(),
                *expected_yaml,
                "failed to serialize '{expected_struct:?}'"
            );
        }
    }
}
