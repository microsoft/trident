#![allow(dead_code)]
//! Test utilities for the engine context. Mainly implementing the builder
//! pattern for `EngineContext` to make it easier to create test instances with
//! various configurations.

use std::path::PathBuf;

use trident_api::{config::HostConfiguration, BlockDeviceId};

use crate::osimage::{mock::MockOsImage, OsImage};

use super::EngineContext;
use std::path::Path;

impl EngineContext {
    /// Adds a spec to the context and builds the graph for it.
    pub(crate) fn with_spec(mut self, spec: HostConfiguration) -> Self {
        self.storage_graph = spec.storage.build_graph().unwrap();
        self.spec = spec;
        self
    }

    /// Sets the mock image for the context.
    pub(crate) fn with_image(mut self, img: MockOsImage) -> Self {
        self.image = Some(OsImage::mock(img));
        self
    }

    /// Populates filesystem data.
    /// Needs both spec and image to be set first!
    pub(crate) fn with_filesystem_data(mut self) -> Self {
        self.populate_filesystems()
            .expect("Failed to populate filesystems");
        self
    }

    /// Inserts a partition path to the context.
    pub(crate) fn with_partition_path<I, P>(mut self, block_device_id: I, partition_path: P) -> Self
    where
        I: Into<BlockDeviceId>,
        P: Into<PathBuf>,
    {
        self.partition_paths
            .insert(block_device_id.into(), partition_path.into());
        self
    }

    /// Inserts multiple partition paths to the context.
    pub(crate) fn with_partition_paths<I, P>(mut self, paths: impl Iterator<Item = (I, P)>) -> Self
    where
        I: Into<BlockDeviceId>,
        P: Into<PathBuf>,
    {
        self.partition_paths
            .extend(paths.map(|(id, path)| (id.into(), path.into())));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_with_partition_path() {
        let ctx = EngineContext::default().with_partition_path("dev1", "/tmp/partition1");

        let id = BlockDeviceId::from("dev1");
        let expected_path = Path::new("/tmp/partition1");

        assert_eq!(
            ctx.partition_paths.get(&id).map(|p| p.as_path()),
            Some(expected_path)
        );
    }

    #[test]
    fn test_with_partition_paths() {
        use super::*;

        let paths = vec![("dev1", "/tmp/partition1"), ("dev2", "/tmp/partition2")];

        let ctx = EngineContext::default().with_partition_paths(paths.iter().cloned());

        for (dev, path) in &paths {
            let id = BlockDeviceId::from(*dev);
            let expected_path = Path::new(*path);
            assert_eq!(
                ctx.partition_paths.get(&id).map(|p| p.as_path()),
                Some(expected_path)
            );
        }
    }
}
