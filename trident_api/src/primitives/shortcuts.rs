use std::{fmt::Display, marker::PhantomData, str::FromStr};

use serde::{
    de::{self, MapAccess, Visitor},
    Deserialize, Deserializer,
};

#[cfg(feature = "schemars")]
pub(crate) trait StringOrStructMetadata {
    fn shorthand_format() -> &'static str;
}

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

#[allow(dead_code)]
pub(crate) fn vec_string_or_struct<'de, T, D>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    T: Deserialize<'de> + FromStr,
    <T as FromStr>::Err: Display,
    D: Deserializer<'de>,
{
    struct VecStringOrStruct<T>(PhantomData<fn() -> T>);

    impl<'de, T> Visitor<'de> for VecStringOrStruct<T>
    where
        T: Deserialize<'de> + FromStr,
        <T as FromStr>::Err: Display,
    {
        type Value = Vec<T>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("sequence of: null, string, or map")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Vec::new())
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: de::SeqAccess<'de>,
        {
            let mut vec = Vec::new();
            while let Some(value) = seq.next_element::<VecOrStringableStruct<T>>()? {
                vec.push(value.0);
            }

            Ok(vec)
        }
    }

    deserializer.deserialize_seq(VecStringOrStruct(PhantomData))
}

/// Helper struct that contains a value which can be deserialized from a string
/// or a struct. It implements `Deserialize` and defers to `string_or_struct()`
/// for deserializing the inner value.
struct VecOrStringableStruct<T>(T);

impl<'de, T> Deserialize<'de> for VecOrStringableStruct<T>
where
    T: Deserialize<'de> + FromStr,
    <T as FromStr>::Err: Display,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        string_or_struct(deserializer).map(VecOrStringableStruct)
    }
}

#[cfg(feature = "schemars")]
pub const STRING_SHORTCUT_EXTENSION: &str = "string_shortcut";

#[cfg(feature = "schemars")]
pub(crate) fn string_or_struct_schema<T>(
    generator: &mut schemars::gen::SchemaGenerator,
) -> schemars::schema::Schema
where
    T: schemars::JsonSchema + StringOrStructMetadata,
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
            serde_json::Value::String(T::shorthand_format().to_owned()),
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
    T: schemars::JsonSchema + StringOrStructMetadata,
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

#[allow(dead_code)]
#[cfg(feature = "schemars")]
pub(crate) fn vec_string_or_struct_schema<T>(
    generator: &mut schemars::gen::SchemaGenerator,
) -> schemars::schema::Schema
where
    T: schemars::JsonSchema + StringOrStructMetadata,
{
    use schemars::schema::{
        ArrayValidation, InstanceType, SchemaObject, SingleOrVec, SubschemaValidation,
    };

    SchemaObject {
        instance_type: Some(SingleOrVec::Single(Box::new(InstanceType::Array))),
        subschemas: Some(Box::new(SubschemaValidation {
            one_of: Some(vec![
                generator.subschema_for::<T>(),
                generator.subschema_for::<String>(),
            ]),
            ..Default::default()
        })),
        array: Some(Box::new(ArrayValidation {
            items: Some(SingleOrVec::Single(Box::new(string_or_struct_schema::<T>(
                generator,
            )))),
            ..Default::default()
        })),
        ..Default::default()
    }
    .into()
}
