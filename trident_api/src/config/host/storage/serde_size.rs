use std::{fmt::Display, num::ParseIntError, str::FromStr};

use crate::constants::PARTITION_SIZE_GROW;

use super::PartitionSize;

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
        let s = String::deserialize(deserializer)?;

        PartitionSize::from_str(s.as_str())
            .map_err(|e| serde::de::Error::custom(format!("invalid partition size: {e}")))
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
}
