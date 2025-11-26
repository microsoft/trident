pub mod config;
pub mod constants;
pub mod error;
pub mod misc;
pub mod primitives;
pub mod status;

/// Identifier for a block device. Needs to be unique across all types of devices.
pub type BlockDeviceId = String;

/// Returns true if the given value is equal to its default value.
/// Useful for #[serde(skip_serializing_if = "default")]
pub fn is_default<T: Default + PartialEq>(t: &T) -> bool {
    *t == Default::default()
}

/// The samples module contains sample data for the API.
///
/// The samples are only used in the documentation. Therefore it is gated by a feature flag.
#[cfg(feature = "samples")]
pub mod samples;

/// The storage graph submodule.
pub use config::host::storage::storage_graph;

/// Re-export dependency so docbuilder can use the exact same version without having to manage a
/// separate dependency in docbuilder's Cargo.toml.
#[cfg(feature = "schemars")]
pub use schemars;

#[cfg(feature = "schemars")]
mod schema_helpers {
    use schemars::{
        gen::{SchemaGenerator, SchemaSettings},
        schema::{ArrayValidation, InstanceType, Schema, SchemaObject, SingleOrVec},
        JsonSchema,
    };

    pub(crate) const BLOCK_DEVICE_ID_FORMAT: &str = "Block Device ID";

    pub(crate) fn schema_generator() -> SchemaGenerator {
        SchemaSettings::draft07()
            .with(|s| {
                s.option_nullable = true;
                s.option_add_null_type = false;
            })
            .into_generator()
    }

    pub(crate) fn block_device_id_schema(gen: &mut SchemaGenerator) -> Schema {
        let mut schema = gen.subschema_for::<String>().into_object();
        schema.format = Some(BLOCK_DEVICE_ID_FORMAT.to_owned());
        Schema::Object(schema)
    }

    pub(crate) fn block_device_id_list_schema(gen: &mut SchemaGenerator) -> Schema {
        // Build an array schema and then add the block device ID schema as the item schema.
        let schema = SchemaObject {
            instance_type: Some(SingleOrVec::Single(Box::new(InstanceType::Array))),
            array: Some(Box::new(ArrayValidation {
                items: Some(SingleOrVec::Single(Box::new(block_device_id_schema(gen)))),
                ..Default::default()
            })),
            ..Default::default()
        };
        Schema::Object(schema)
    }

    /// Generate a schema for a unit enum with a single untagged variant.
    ///
    /// This exists because this pattern is currently unsupported by schemars.
    /// https://github.com/GREsau/schemars/issues/222
    pub fn unit_enum_with_untagged_variant<E, U>(generator: &mut SchemaGenerator) -> Schema
    where
        E: JsonSchema,
        U: JsonSchema,
    {
        println!(
            "unit_enum_with_untagged_variant called for <{},{}>",
            std::any::type_name::<E>(),
            std::any::type_name::<U>()
        );
        // Check if we've already added the schema for this enum to the generator's definitions.
        // If we have, we can just return a reference to it. Otherwise we'd modify it again.
        if generator.definitions().contains_key(&E::schema_name()) {
            return Schema::new_ref(format!(
                "{}{}",
                generator.settings().definitions_path,
                E::schema_name(),
            ));
        }

        // Generate a schema for the enum with a single untagged variant.
        // Because enums produce referenceable schemas, this will just be a
        // $ref.
        let base = generator.subschema_for::<E>();

        // Generate a schema for the untagged variant.
        let mut untagged_variant_schema = U::json_schema(generator).into_object();

        // Store a copy of the definitions path to use later.
        let definitions_path = generator.settings().definitions_path.clone();

        // Extract the schema for the enum from the generator's definitions.
        let schema = match base {
            Schema::Object(SchemaObject {
                reference: Some(ref key),
                ..
            }) => generator
                .definitions_mut()
                .get_mut(key.strip_prefix(&definitions_path).unwrap_or_else(|| {
                    panic!("Expected key '{key}' to start with definitions path.")
                }))
                .expect("Expected schema '{key}' to be present in definitions."),
            _ => panic!("Expected schema to be a reference."),
        };

        let Schema::Object(ref mut obj) = schema else {
            panic!("Expected schema to be an object.");
        };

        let one_of = obj
            .subschemas()
            .one_of
            .as_mut()
            .expect("Expected 'one_of' to be present");

        // Find all non-unit variants.
        let mut non_unit_variants = one_of
            .iter_mut()
            .filter(|schema| {
                let Schema::Object(ref obj) = schema else {
                    panic!("Expected subschema to be an object.");
                };

                // Unit variants are simple strings!
                if obj.instance_type == Some(SingleOrVec::Single(Box::new(InstanceType::String))) {
                    // Unit variants have a single enum value, which must be a string.
                    if let Some(enum_values) = obj.enum_values.as_ref() {
                        if enum_values.len() == 1 && enum_values[0].is_string() {
                            // This is a unit variant, so remove it.
                            return false;
                        }
                    }
                }
                // This is something else, so keep it.
                true
            })
            .collect::<Vec<_>>();

        // We are expecting a single non-unit variant.
        if non_unit_variants.len() != 1 {
            panic!("Expected to find exactly one non-unit variant.");
        }

        // Get the non-unit variant schema
        let Schema::Object(ref mut non_unit_variant) = non_unit_variants[0] else {
            panic!("Expected non-unit variant to be an object.");
        };

        let title = match (
            &non_unit_variant.metadata().title,
            &untagged_variant_schema.metadata().title,
        ) {
            (Some(title), _) => title.clone(),
            (_, Some(title)) => title.clone(),
            _ => panic!("Expected either the enum or the variant to have a title."),
        };

        let description = match (
            &non_unit_variant.metadata().description,
            &untagged_variant_schema.metadata().description,
        ) {
            (Some(d1), Some(d2)) => format!("{d1}\n\n*Details:*\n\n{d2}"),
            (Some(description), _) => description.clone(),
            (_, Some(description)) => description.clone(),
            _ => panic!("Expected either the enum or the variant to have a description."),
        };

        untagged_variant_schema.metadata().title = Some(title);
        untagged_variant_schema.metadata().description = Some(description);

        // Replace the non-unit variant with the untagged variant schema.
        *non_unit_variant = untagged_variant_schema;

        base
    }
}
