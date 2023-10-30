use std::path::PathBuf;

use status::{BlockDeviceContents, BlockDeviceInfo, Disk, Partition, RaidArray};

pub mod config;
pub mod constants;
pub mod status;

pub(crate) mod serde;

impl Disk {
    pub fn to_block_device(&self) -> BlockDeviceInfo {
        BlockDeviceInfo::new(self.path.clone(), self.capacity, self.contents.clone())
    }
}

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
