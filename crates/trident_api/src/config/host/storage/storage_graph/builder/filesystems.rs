use std::{collections::BTreeSet, path::Path};

use petgraph::visit::IntoNodeReferences;

use crate::config::{
    host::storage::storage_graph::{error::StorageGraphBuildError, graph::StoragePetgraph},
    FileSystem,
};

/// Checks all basic properties of filesystems and ensures mount points are unique.
pub(super) fn check_filesystems(graph: &StoragePetgraph) -> Result<(), StorageGraphBuildError> {
    // Create a set of all unique mount points
    let mut unique_mount_points = BTreeSet::new();

    // Helper closure to check and insert a mount point into the set.
    let mut check_insert_mount_point = |mount_point: &Path| -> Result<(), StorageGraphBuildError> {
        // Ensure the mount point path is absolute
        if !mount_point.is_absolute() {
            return Err(StorageGraphBuildError::MountPointPathNotAbsolute(
                mount_point.to_string_lossy().into(),
            ));
        }

        // Check if the mount point is unique by inserting it into the set.
        if !unique_mount_points.insert(mount_point.to_path_buf()) {
            return Err(StorageGraphBuildError::DuplicateMountPoint(
                mount_point.to_string_lossy().into(),
            ));
        }

        Ok(())
    };

    // Iterate over all nodes that are filesystems and check their mount points.
    for (_, node) in graph.node_references() {
        let Some(fs) = node.as_filesystem() else {
            continue;
        };

        check_filesystem(fs)?;
        if let Some(mount_point) = &fs.mount_point {
            check_insert_mount_point(&mount_point.path)?;
        }
    }

    Ok(())
}

/// Checks all basic properties of a single filesystem.
fn check_filesystem(fs: &FileSystem) -> Result<(), StorageGraphBuildError> {
    // Check if we have a target.
    match (fs.device_id.is_some(), fs.source.expects_block_device_id()) {
        // We have a device ID and we expect it: OK
        (true, true) => (),
        // We don't have a device ID and we don't expect it: OK
        (false, false) => (),
        // We have a device ID but we don't expect it: ERROR
        (true, false) => {
            return Err(StorageGraphBuildError::FilesystemUnexpectedBlockDeviceId {
                fs_desc: fs.description(),
            });
        }
        // We don't have a device ID but we expect it: ERROR
        (false, true) => {
            return Err(StorageGraphBuildError::FilesystemMissingBlockDeviceId {
                fs_desc: fs.description(),
            });
        }
    }

    if fs.mount_point.is_none() && fs.source.must_have_mountpoint() {
        // This filesystem must have a mount point, but none was provided.
        return Err(StorageGraphBuildError::FilesystemMissingMountPoint {
            fs_desc: fs.description(),
        });
    }

    Ok(())
}
