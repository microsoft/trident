use std::{fmt::Display, num::ParseIntError, str::FromStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ByteCount(pub u64);

impl From<u64> for ByteCount {
    fn from(x: u64) -> Self {
        ByteCount(x)
    }
}

impl ByteCount {
    pub fn bytes(self) -> u64 {
        self.0
    }

    pub fn to_human_readable(&self) -> String {
        match self.0.trailing_zeros() {
            _ if self.0 == 0 => "0".to_owned(),
            0..=9 => format!("{}", self.0),
            10..=19 => format!("{}K", self.0 >> 10),
            20..=29 => format!("{}M", self.0 >> 20),
            30..=39 => format!("{}G", self.0 >> 30),
            _ => format!("{}T", self.0 >> 40),
        }
    }

    pub fn from_human_readable(mut s: &str) -> Result<Self, ParseIntError> {
        s = s.trim();
        let try_parse = |val: &str, shift: u8| Ok(Self(val.trim().parse::<u64>()? << shift));
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
}

impl Display for ByteCount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_human_readable())
    }
}

impl FromStr for ByteCount {
    type Err = ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_human_readable(s.trim())
    }
}

impl<'de> serde::Deserialize<'de> for ByteCount {
    fn deserialize<D>(deserializer: D) -> Result<ByteCount, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Size may be provided as a string (e.g. "1K") or as a pure number
        // (e.g. 1024). Serde forces a number when only digits are provided, so
        // we need to deserialize as a generic value and then check the type.
        let value = serde_yaml::Value::deserialize(deserializer)?;

        match value {
            serde_yaml::Value::String(s) => ByteCount::from_str(s.as_str())
                .map_err(|e| serde::de::Error::custom(format!("invalid byte count size: {e}"))),
            serde_yaml::Value::Number(n) => {
                let n = n.as_u64().ok_or_else(|| {
                    serde::de::Error::custom("invalid byte count size, expected unsigned integer")
                })?;
                Ok(ByteCount(n))
            }
            _ => Err(serde::de::Error::custom("invalid byte count size")),
        }
    }
}

impl serde::Serialize for ByteCount {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if self.0 & 0x3FF != 0 {
            // If the count is not a multiple of 1024, then we must write it as
            // a raw number. In this case, we serialize it as a number.
            serializer.serialize_u64(self.0)
        } else {
            // Serialize as a string if the value is a multiple of 1024
            serializer.serialize_str(self.to_human_readable().as_str())
        }
    }
}

#[cfg(feature = "schemars")]
mod schema_impl {
    use std::borrow::Cow;

    use schemars::{
        gen::SchemaGenerator,
        schema::{InstanceType, Schema, SingleOrVec},
        JsonSchema,
    };

    use super::ByteCount;

    impl JsonSchema for ByteCount {
        fn schema_name() -> String {
            std::any::type_name::<Self>()
                .split("::")
                .last()
                .unwrap()
                .to_string()
        }

        fn schema_id() -> Cow<'static, str> {
            Cow::Owned(format!(
                concat!(module_path!(), "::{}"),
                Self::schema_name()
            ))
        }

        fn json_schema(gen: &mut SchemaGenerator) -> Schema {
            let mut schema = gen.subschema_for::<String>().into_object();
            schema.format = Some(r"\d+\s*[KMGT]?".to_owned());
            schema.instance_type = Some(SingleOrVec::Vec(vec![
                InstanceType::String,
                InstanceType::Number,
            ]));
            let metadata = schema.metadata();
            metadata.description = Some(
                "A byte count with an optional suffix (K, M, G, T, to the base of 1024)."
                    .to_owned(),
            );
            metadata.examples = vec![
                serde_json::json!(0),
                serde_json::json!(1),
                serde_json::json!(102),
                serde_json::json!(104576),
                serde_json::json!("1K"),
                serde_json::json!("1M"),
                serde_json::json!("5G"),
                serde_json::json!("4T"),
            ];

            Schema::Object(schema)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_string() {
        // Some values
        assert_eq!(ByteCount::from_str("1").unwrap(), ByteCount(1));
        assert_eq!(ByteCount::from_str("20K").unwrap(), ByteCount(20 * 1024));
        assert_eq!(
            ByteCount::from_str("30M").unwrap(),
            ByteCount(30 * 1024 * 1024)
        );
        assert_eq!(
            ByteCount::from_str("40G").unwrap(),
            ByteCount(40 * 1024 * 1024 * 1024)
        );
        assert_eq!(
            ByteCount::from_str("50T").unwrap(),
            ByteCount(50 * 1024 * 1024 * 1024 * 1024)
        );

        // Allowed spacing
        assert_eq!(ByteCount::from_str(" 1024 ").unwrap(), ByteCount(1024));
        assert_eq!(ByteCount::from_str(" 1K ").unwrap(), ByteCount(1024));
        assert_eq!(ByteCount::from_str("1 K").unwrap(), ByteCount(1024));
        assert_eq!(
            ByteCount::from_str(" 300 K ").unwrap(),
            ByteCount(300 * 1024)
        );

        // Invalid numbers
        assert!(ByteCount::from_str("1.0").is_err());
        assert!(ByteCount::from_str("1.0K").is_err());

        // Invalid spacing
        assert!(ByteCount::from_str("1 0K").is_err());

        // Invalid units
        assert!(ByteCount::from_str("1.0X").is_err());

        // Invalid trailing characters
        assert!(ByteCount::from_str("1.0KX").is_err());

        // Invalid leading characters
        assert!(ByteCount::from_str("X10K").is_err());

        // Invalid leading and trailing characters
        assert!(ByteCount::from_str("X10KX").is_err());

        // Garbage
        assert!(ByteCount::from_str("X").is_err());
    }

    #[test]
    fn test_to_human_readable() {
        // Some values
        assert_eq!(ByteCount(0).to_string(), "0");
        assert_eq!(ByteCount(1).to_string(), "1");
        assert_eq!(ByteCount(1023).to_string(), "1023");
        assert_eq!(ByteCount(1024).to_string(), "1K");
        assert_eq!(ByteCount(1025).to_string(), "1025");
        assert_eq!(ByteCount(1024 * 1024).to_string(), "1M");
        assert_eq!(ByteCount(1024 * 1024 + 1).to_string(), "1048577");
        assert_eq!(ByteCount(1024 * 1024 + 1024).to_string(), "1025K");
        assert_eq!(ByteCount(1024 * 1024 * 1024).to_string(), "1G");
        assert_eq!(ByteCount(1024 * 1024 * 1024 + 1).to_string(), "1073741825");
        assert_eq!(ByteCount(1024 * 1024 * 1024 * 1024).to_string(), "1T");
        assert_eq!(
            ByteCount(1024 * 1024 * 1024 * 1024 + 1).to_string(),
            "1099511627777"
        );
    }

    #[test]
    fn test_roundtrip() {
        let test = |s: &str| {
            let n = ByteCount::from_str(s).unwrap();
            let s2 = n.to_string();
            assert_eq!(s, s2);
        };

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
            size: ByteCount,
        }

        impl TestStruct {
            fn size(v: u64) -> Self {
                Self { size: v.into() }
            }
        }

        // Define test cases
        let test_cases = [
            ("size: 1", TestStruct::size(1), "size: 1"),
            ("size: 512", TestStruct::size(512), "size: 512"),
            ("size: 1K", TestStruct::size(1024), "size: 1K"),
            ("size: 1024", TestStruct::size(1024), "size: 1K"),
            ("size: 1025", TestStruct::size(1025), "size: 1025"),
            ("size: 1536", TestStruct::size(1536), "size: 1536"),
            ("size: 1M", TestStruct::size(1048576), "size: 1M"),
            ("size: 1048576", TestStruct::size(1048576), "size: 1M"),
            ("size: 1G", TestStruct::size(1073741824), "size: 1G"),
            ("size: 1073741824", TestStruct::size(1073741824), "size: 1G"),
            ("size: 1024M", TestStruct::size(1073741824), "size: 1G"),
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
