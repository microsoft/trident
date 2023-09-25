use std::path::PathBuf;

use status::{BlockDeviceContents, BlockDeviceInfo, Disk, Partition};

pub mod config;
pub mod status;

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

impl BlockDeviceInfo {
    pub fn new(path: PathBuf, size: u64, contents: BlockDeviceContents) -> Self {
        Self {
            path,
            size,
            contents,
        }
    }
}
