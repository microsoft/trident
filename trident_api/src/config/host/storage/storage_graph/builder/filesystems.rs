use std::{collections::BTreeSet, path::Path};

use crate::config::{
    host::storage::storage_graph::{
        error::StorageGraphBuildError, graph::StoragePetgraph, types::FileSystemSourceKind,
    },
    FileSystem, VerityFileSystem,
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
    for fs in graph.node_weights().filter_map(|n| n.as_filesystem()) {
        check_filesystem(fs)?;
        if let Some(mount_point) = &fs.mount_point {
            check_insert_mount_point(&mount_point.path)?;
        }
    }

    let mut unique_verity_names = BTreeSet::new();

    // Iterate over all nodes that are verity filesystems and check their mount points.
    for vfs in graph
        .node_weights()
        .filter_map(|n| n.as_verity_filesystem())
    {
        check_verity_filesystem(vfs)?;
        check_insert_mount_point(&vfs.mount_point.path)?;

        if !unique_verity_names.insert(vfs.name.clone()) {
            return Err(StorageGraphBuildError::VerityFilesystemDuplicateName {
                name: vfs.name.clone(),
            });
        }
    }

    Ok(())
}

/// Checks all basic properties of a single filesystem.
fn check_filesystem(fs: &FileSystem) -> Result<(), StorageGraphBuildError> {
    // Check if we have a target.
    match (fs.device_id.is_some(), fs.fs_type.expects_block_device_id()) {
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

    if fs.mount_point.is_some() {
        // Check if this filesystem can have a mount point.
        if !fs.fs_type.can_have_mountpoint2() {
            return Err(StorageGraphBuildError::FilesystemUnexpectedMountPoint {
                fs_desc: fs.description(),
                fs_type: fs.fs_type,
            });
        }
    } else if fs.fs_type.must_have_mountpoint2() {
        // This filesystem must have a mount point, but none was provided.
        return Err(StorageGraphBuildError::FilesystemMissingMountPoint {
            fs_desc: fs.description(),
        });
    }

    // Check that the filesystem source is compatible with the filesystem type.
    {
        let fs_compatible_sources = fs.fs_type.valid_sources2();
        let fs_src_kind = FileSystemSourceKind::from(&fs.source);
        if !fs_compatible_sources.contains(fs_src_kind) {
            return Err(StorageGraphBuildError::FilesystemIncompatibleSource {
                fs_desc: fs.description(),
                fs_source: fs_src_kind,
                fs_compatible_sources,
            });
        }
    }

    Ok(())
}

/// Checks all basic properties of a single verity filesystem.
fn check_verity_filesystem(vfs: &VerityFileSystem) -> Result<(), StorageGraphBuildError> {
    if !vfs.fs_type.supports_verity() {
        return Err(StorageGraphBuildError::VerityFileSystemUnsupportedType {
            name: vfs.name.clone(),
            fs_type: vfs.fs_type,
        });
    }

    Ok(())
}
