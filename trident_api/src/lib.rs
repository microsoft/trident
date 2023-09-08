use std::path::PathBuf;

use status::{BlockDeviceInfo, Disk, Partition};

pub mod config;
pub mod status;

impl Disk {
    pub fn to_block_device(&self) -> BlockDeviceInfo {
        BlockDeviceInfo::new(self.path.clone(), self.capacity.unwrap_or(0))
    }
}

impl Partition {
    pub fn to_block_device(&self) -> BlockDeviceInfo {
        BlockDeviceInfo::new(self.path.clone(), self.end - self.start)
    }
}

impl BlockDeviceInfo {
    pub fn new(path: PathBuf, size: u64) -> Self {
        Self { path, size }
    }
}
