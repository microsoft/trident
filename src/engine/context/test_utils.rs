#![allow(dead_code)]
//! Test utilities for the engine context. Mainly implementing the builder
//! pattern for `EngineContext` to make it easier to create test instances with
//! various configurations.

use std::path::PathBuf;

use trident_api::{config::HostConfiguration, BlockDeviceId};

use crate::osimage::{mock::MockOsImage, OsImage};

use super::EngineContext;

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
