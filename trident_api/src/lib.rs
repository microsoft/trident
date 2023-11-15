use std::path::PathBuf;

use status::{BlockDeviceContents, BlockDeviceInfo, Partition, RaidArray};

pub mod config;
pub mod constants;
pub mod status;

/// Identifier for a block device. Needs to be unique across all types of devices.
pub type BlockDeviceId = String;

impl Partition {
    pub fn to_block_device(&self) -> BlockDeviceInfo {
        BlockDeviceInfo::new(
            self.path.clone(),
            self.end - self.start,
            self.contents.clone(),
        )
    }
}

impl RaidArray {
    pub fn to_block_device(&self) -> BlockDeviceInfo {
        BlockDeviceInfo::new(self.path.clone(), self.array_size, self.contents.clone())
    }
}

impl BlockDeviceInfo {
    pub fn new(path: PathBuf, size: u64, contents: BlockDeviceContents) -> Self {
        Self {
            path,
            size,
            contents,
        }
    }
}

/// Returns true if the given value is equal to its default value.
/// Useful for #[serde(skip_serializing_if = "default")]
fn is_default<T: Default + PartialEq>(t: &T) -> bool {
    *t == Default::default()
}

#[cfg(feature = "schemars")]
mod schema_helpers {
    use schemars::{
        gen::SchemaGenerator,
        schema::{ArrayValidation, InstanceType, Schema, SchemaObject, SingleOrVec},
    };

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
