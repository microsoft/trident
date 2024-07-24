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
fn is_default<T: Default + PartialEq>(t: &T) -> bool {
    *t == Default::default()
}

/// The samples module contains sample data for the API.
///
/// The samples are only used in the documentation. Therefore it is gated by a
/// feature flag.
#[cfg(feature = "samples")]
pub mod samples;

/// Give docbuilder easy access to the block device graph types
#[cfg(feature = "documentation")]
pub use config::host::storage::blkdev_graph;

/// Re export dependency so docbuilder can use the exact same version without
/// having to manage a separate dependency in docbuilder's Cargo.toml.
#[cfg(feature = "schemars")]
pub use schemars;

#[cfg(feature = "schemars")]
mod schema_helpers {
    use schemars::{
        gen::{SchemaGenerator, SchemaSettings},
        schema::{ArrayValidation, InstanceType, Schema, SchemaObject, SingleOrVec},
    };

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
        schema.format = Some("Block Device ID".to_owned());
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
}
