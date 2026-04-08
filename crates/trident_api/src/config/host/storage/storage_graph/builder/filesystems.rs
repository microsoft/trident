use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use petgraph::visit::IntoNodeReferences;

use crate::config::{
    host::storage::storage_graph::{error::StorageGraphBuildError, graph::StoragePetgraph},
    FileSystem, FileSystemSource,
};

/// Checks all basic properties of filesystems and ensures mount points are unique.
pub(super) fn check_filesystems(
    graph: &StoragePetgraph,
) -> Result<PathBuf, StorageGraphBuildError> {
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

    // Keep track of the ESP filesystem and its mount point (if any) to perform
    // additional checks later.
    let mut esp_filesystem: Option<(&FileSystem, &Path)> = None;

    // Iterate over all nodes that are filesystems and check their mount points.
    for (_, node) in graph.node_references() {
        let Some(fs) = node.as_filesystem() else {
            continue;
        };

        check_filesystem(fs)?;
        match (&fs.mount_point, fs.is_esp) {
            (None, true) => {
                // A filesystem without mount point CANNOT be the ESP.
                return Err(StorageGraphBuildError::FilesystemEspWithoutMountPoint {
                    fs_desc: fs.description(),
                });
            }

            (None, false) => {
                // Nothing to check.
            }

            (Some(mount_point), is_esp) => {
                // Check if the mount point is unique.
                check_insert_mount_point(&mount_point.path)?;

                // If this filesystem is the ESP, we need to check if the mount
                // point is valid.
                match (esp_filesystem.as_ref(), is_esp) {
                    (None, true) => {
                        // This is the first ESP we've seen, so we store it for
                        // later checks.
                        esp_filesystem = Some((fs, mount_point.path.as_path()));
                    }
                    (Some((other_fs, _)), true) => {
                        // We already have one ESP defined, throw a multiple ESP
                        // error.
                        return Err(StorageGraphBuildError::FilesystemEspMultiple {
                            fs_desc_a: other_fs.description(),
                            fs_desc_b: fs.description(),
                        });
                    }

                    (_, false) => {
                        // This filesystem is not the ESP, so we don't need to
                        // check its mount point against the ESP.
                    }
                }
            }
        }
    }

    // Extract the ESP FS data for additional checks.
    let Some((esp_fs, esp_mount_path)) = esp_filesystem else {
        return Err(StorageGraphBuildError::FilesystemEspNotFound);
    };

    // The ESP must be backed by an image.
    if esp_fs.source != FileSystemSource::Image {
        return Err(StorageGraphBuildError::FilesystemEspNotBackedByImage {
            fs_desc: esp_fs.description(),
        });
    }

    Ok(esp_mount_path.to_path_buf())
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
