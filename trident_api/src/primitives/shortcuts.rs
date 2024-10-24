use std::{fmt::Display, marker::PhantomData, str::FromStr};

use serde::{
    de::{self, MapAccess, Visitor},
    Deserialize, Deserializer,
};

pub(crate) fn string_or_struct<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    T: Deserialize<'de> + FromStr,
    <T as FromStr>::Err: Display,
    D: Deserializer<'de>,
{
    struct StringOrStruct<T>(PhantomData<fn() -> T>);

    impl<'de, T> Visitor<'de> for StringOrStruct<T>
    where
        T: Deserialize<'de> + FromStr,
        <T as FromStr>::Err: Display,
    {
        type Value = T;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("string or map")
        }

        fn visit_str<E>(self, value: &str) -> Result<T, E>
        where
            E: de::Error,
        {
            T::from_str(value).map_err(|e| E::custom(e.to_string()))
        }

        fn visit_map<M>(self, map: M) -> Result<T, M::Error>
        where
            M: MapAccess<'de>,
        {
            Deserialize::deserialize(de::value::MapAccessDeserializer::new(map))
        }
    }

    deserializer.deserialize_any(StringOrStruct(PhantomData))
}

pub(crate) fn opt_string_or_struct<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de> + FromStr,
    <T as FromStr>::Err: Display,
    D: Deserializer<'de>,
{
    struct OptStringOrStruct<T>(PhantomData<fn() -> T>);

    impl<'de, T> Visitor<'de> for OptStringOrStruct<T>
    where
        T: Deserialize<'de> + FromStr,
        <T as FromStr>::Err: Display,
    {
        type Value = Option<T>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("null, string, or map")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: Deserializer<'de>,
        {
            string_or_struct(deserializer).map(Some)
        }
    }

    deserializer.deserialize_option(OptStringOrStruct(PhantomData))
}

#[cfg(feature = "schemars")]
pub const STRING_SHORTCUT_EXTENSION: &str = "string_shortcut";

#[cfg(feature = "schemars")]
pub(crate) fn string_or_struct_schema<T>(
    generator: &mut schemars::gen::SchemaGenerator,
) -> schemars::schema::Schema
where
    T: schemars::JsonSchema,
{
    use schemars::schema::{SchemaObject, SubschemaValidation};
    SchemaObject {
        subschemas: Some(Box::new(SubschemaValidation {
            one_of: Some(vec![
                generator.subschema_for::<T>(),
                generator.subschema_for::<String>(),
            ]),
            ..Default::default()
        })),
        extensions: [(
            STRING_SHORTCUT_EXTENSION.into(),
            serde_json::Value::Bool(true),
        )]
        .into(),
        ..Default::default()
    }
    .into()
}

#[cfg(feature = "schemars")]
pub(crate) fn opt_string_or_struct_schema<T>(
    generator: &mut schemars::gen::SchemaGenerator,
) -> schemars::schema::Schema
where
    T: schemars::JsonSchema,
{
    // Get the base schema for T
    let mut schema = string_or_struct_schema::<T>(generator).into_object();

    // Add the null type to the schema
    schema.subschemas().one_of.iter_mut().for_each(|schema| {
        schema.push(generator.subschema_for::<()>());
    });

    // Copied from how schemars handles Option<T>
    if generator.settings().option_nullable {
        schema
            .extensions
            .insert("nullable".to_string(), serde_json::Value::Bool(true));
    };

    schema.into()
}
