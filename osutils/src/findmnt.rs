//! Module for interacting with the `findmnt` command.
//!
//! The `findmnt` command is used to find mounted filesystems. This module
//! provides a way to run `findmnt` and parse its output into a Rust structure.
//!
//! The `FindMnt` structure represents the output of `findmnt --json` and
//! contains a list of `MountpointMetadata` structures, which represent a
//! filesystem entry with the columns defined in `FINDMNT_COLUMNS`.
//!
//! The `MountpointMetadata` structure also contains a `children` field, which
//! contains all mounts under this filesystem and it is added automatically by
//! `findmnt` when the `--json` flag is used.
//!
//! `findmnt` works by reading the `/proc/self/mountinfo` file, which contains
//! information about all mounted filesystems visible to the current process. We
//! prefer to use `findmnt` instead of reading `/proc/self/mountinfo` because it
//! can output easy-to-parse JSON and it also returns mounts in a hierarchical
//! structure, which is easier to work with.
//!
//! For example, on a simple Azl 2.0 system, `findmnt -o
//! id,target,source,fsroot,options` returns:
//!
//! ```text
//! ID TARGET                                SOURCE     FSROOT OPTIONS
//! 96 /                                     /dev/sda2  /      rw,relatime,seclabel
//! 29 ├─/tmp                                tmpfs      /      rw,nosuid,nodev,seclabel,nr_inodes=1048576
//! 33 ├─/boot/efi                           /dev/sda1  /      rw,relatime,fmask=0077,dmask=0077,codepage=437,iocharset=ascii,shortn
//! 44 ├─/dev                                devtmpfs   /      rw,nosuid,seclabel,size=4096k,nr_inodes=721910,mode=755
//! 24 │ ├─/dev/mqueue                       mqueue     /      rw,nosuid,nodev,noexec,relatime,seclabel
//! 25 │ ├─/dev/hugepages                    hugetlbfs  /      rw,nosuid,nodev,relatime,seclabel,pagesize=2M
//! 45 │ ├─/dev/shm                          tmpfs      /      rw,nosuid,nodev,seclabel
//! 46 │ └─/dev/pts                          devpts     /      rw,nosuid,noexec,relatime,seclabel,gid=5,mode=620,ptmxmode=000
//! 47 ├─/sys                                sysfs      /      rw,nosuid,nodev,noexec,relatime,seclabel
//! 22 │ ├─/sys/fs/selinux                   selinuxfs  /      rw,nosuid,noexec,relatime
//! 26 │ ├─/sys/kernel/debug                 debugfs    /      rw,nosuid,nodev,noexec,relatime,seclabel
//! 28 │ ├─/sys/kernel/tracing               tracefs    /      rw,nosuid,nodev,noexec,relatime,seclabel
//! 30 │ ├─/sys/fs/fuse/connections          fusectl    /      rw,nosuid,nodev,noexec,relatime
//! 31 │ ├─/sys/kernel/config                configfs   /      rw,nosuid,nodev,noexec,relatime
//! 48 │ ├─/sys/kernel/security              securityfs /      rw,nosuid,nodev,noexec,relatime
//! 49 │ ├─/sys/fs/cgroup                    tmpfs      /      ro,nosuid,nodev,noexec,seclabel,size=4096k,nr_inodes=1024,mode=755
//! 50 │ │ ├─/sys/fs/cgroup/systemd          cgroup     /      rw,nosuid,nodev,noexec,relatime,seclabel,xattr,release_agent=/usr/lib
//! 51 │ │ ├─/sys/fs/cgroup/cpu,cpuacct      cgroup     /      rw,nosuid,nodev,noexec,relatime,seclabel,cpu,cpuacct
//! 52 │ │ ├─/sys/fs/cgroup/blkio            cgroup     /      rw,nosuid,nodev,noexec,relatime,seclabel,blkio
//! 53 │ │ ├─/sys/fs/cgroup/net_cls,net_prio cgroup     /      rw,nosuid,nodev,noexec,relatime,seclabel,net_cls,net_prio
//! 54 │ │ ├─/sys/fs/cgroup/freezer          cgroup     /      rw,nosuid,nodev,noexec,relatime,seclabel,freezer
//! 55 │ │ ├─/sys/fs/cgroup/pids             cgroup     /      rw,nosuid,nodev,noexec,relatime,seclabel,pids
//! 56 │ │ ├─/sys/fs/cgroup/hugetlb          cgroup     /      rw,nosuid,nodev,noexec,relatime,seclabel,hugetlb
//! 57 │ │ ├─/sys/fs/cgroup/rdma             cgroup     /      rw,nosuid,nodev,noexec,relatime,seclabel,rdma
//! 58 │ │ ├─/sys/fs/cgroup/devices          cgroup     /      rw,nosuid,nodev,noexec,relatime,seclabel,devices
//! 59 │ │ ├─/sys/fs/cgroup/memory           cgroup     /      rw,nosuid,nodev,noexec,relatime,seclabel,memory
//! 60 │ │ ├─/sys/fs/cgroup/cpuset           cgroup     /      rw,nosuid,nodev,noexec,relatime,seclabel,cpuset
//! 61 │ │ └─/sys/fs/cgroup/perf_event       cgroup     /      rw,nosuid,nodev,noexec,relatime,seclabel,perf_event
//! 62 │ ├─/sys/fs/pstore                    pstore     /      rw,nosuid,nodev,noexec,relatime,seclabel
//! 63 │ ├─/sys/firmware/efi/efivars         efivarfs   /      rw,nosuid,nodev,noexec,relatime
//! 64 │ └─/sys/fs/bpf                       bpf        /      rw,nosuid,nodev,noexec,relatime,mode=700
//! 65 ├─/proc                               proc       /      rw,nosuid,nodev,noexec,relatime
//! 23 │ └─/proc/sys/fs/binfmt_misc          systemd-1  /      rw,relatime,fd=27,pgrp=1,timeout=0,minproto=5,maxproto=5,direct,pipe_
//! 66 └─/run                                tmpfs      /      rw,nosuid,nodev,seclabel,size=1159032k,nr_inodes=819200,mode=755
//! 40   └─/run/user/0                       tmpfs      /      rw,nosuid,nodev,relatime,seclabel,size=579516k,nr_inodes=144879,mode=
//! ```
//!
//! With the `--json` flag, `findmnt` outputs the same hierarchy, but in JSON,
//! where each mount point has a `children` field with all its children.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

use anyhow::Context;
use serde::Deserialize;

use sysdefs::filesystems::KernelFilesystemType;
use trident_api::{config::MountOptions, constants::ROOT_MOUNT_POINT_PATH};

use crate::dependencies::{Command, Dependency};

/// String representation of the unbindable propagation type.
pub const PROPAGATION_UNBINDABLE: &str = "unbindable";

/// String with a comma-separated list of columns to be used with `findmnt
/// --json -o` to output the columns that can be deserialized into a
/// `MountpointMetadata` structure.
pub const FINDMNT_COLUMNS: &str = "id,target,source,fsroot,fstype,options,propagation";

/// Represents the output of `findmnt --json` as a Rust structure.
#[derive(Debug, Deserialize)]
pub struct FindMnt {
    pub filesystems: Vec<MountpointMetadata>,
}

/// Represents a filesystem entry from `findmnt --json` with the columns defined
/// in `FINDMNT_COLUMNS`. The `children` field contains all mounts under this
/// filesystem and it is added automatically by `findmnt` when the `--json` flag
/// is used.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub struct MountpointMetadata {
    /// Mount ID.
    pub id: u32,

    /// Mount target.
    pub target: PathBuf,

    /// Source device.
    ///
    /// This is an optional field because in some cases, a mounted filesystem will not have a
    /// specific source. E.g. When containerd and tardev-snapshotter are used for running Trident
    /// inside a container, an overlay mount will have its source reported as `null` by `findmnt`.
    pub source: Option<PathBuf>,

    /// Filesystem root.
    pub fsroot: PathBuf,

    /// Filesystem type.
    pub fstype: KernelFilesystemType,

    /// Options.
    pub options: MountOptions,

    /// Propagation type.
    pub propagation: String,

    /// Mounts under this filesystem.
    #[serde(default)]
    pub children: Vec<MountpointMetadata>,
}

impl FindMnt {
    /// Runs `findmnt --json` and parses the output into a `FindMnt` structure.
    pub fn run() -> Result<Self, anyhow::Error> {
        Self::run_internal(Self::build_command())
    }

    /// Same as `run()`, but adds the `--real` flag to limit the output to real
    /// filesystems.
    pub fn run_real() -> Result<Self, anyhow::Error> {
        let mut cmd = Self::build_command();
        cmd.arg("--real");
        Self::run_internal(cmd)
    }

    /// Builds a `Command` to run `findmnt` with the common arguments.
    fn build_command() -> Command {
        let mut cmd = Dependency::Findmnt.cmd();

        // Output in JSON format
        cmd.arg("--json");

        // Query specific output columns. `--output-all` does not behave well
        // with newer versions of findmnt, like the one in Azl, as it includes
        // the `action` column, which required the `--poll` flag, and seems to
        // take a while longer to process. It's easier to just query specific
        // columns.
        cmd.arg("-o");
        cmd.arg(FINDMNT_COLUMNS);

        cmd
    }

    /// Runs a `Command` and parses the output into a `FindMnt` structure.
    fn run_internal(cmd: Command) -> Result<Self, anyhow::Error> {
        Self::from_json(&cmd.output_and_check().context("Failed to run findmnt")?)
            .context("Failed to deserialize output of findmnt")
    }

    /// Parses a JSON string into a `FindMnt` structure.
    fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Returns the root filesystem.
    /// If there is no root filesystem, returns `None`.
    pub fn root(self) -> Option<MountpointMetadata> {
        self.filesystems
            .into_iter()
            .find(|fs| fs.target == PathBuf::from(ROOT_MOUNT_POINT_PATH))
    }
}

impl MountpointMetadata {
    /// Recursively prunes all entries matching the given prefix from the
    /// `MountpointMetadata` structure.
    pub fn prune_prefix(&mut self, prefix: impl AsRef<OsStr>) {
        self.children
            .retain(|fs| !fs.target.starts_with(prefix.as_ref()));
        for child in self.children.iter_mut() {
            child.prune_prefix(prefix.as_ref());
        }
    }

    /// Returns whether a mount point with the given target exists in this mount
    /// point or any of its children.
    ///
    /// In contrast to `contains_path`, this function only checks the target of
    /// the mount point, not the path it represents. It will only return `true`
    /// if the target of the mount point or any of its children is exactly the
    /// same as the given path.
    pub fn contains_mountpoint(&self, target: impl AsRef<Path>) -> bool {
        // If the target is not contained in this mount or its children, return
        // false
        if !self.contains_path(target.as_ref()) {
            return false;
        }

        // If the target is exactly this mount point, return true
        if self.target == target.as_ref() {
            return true;
        }

        self.children
            .iter()
            .any(|child| child.contains_mountpoint(target.as_ref()))
    }

    /// Returns the mount containing the given path. If the path is not
    /// contained in any mount, returns `None`.
    ///
    /// The path must be a valid normalized absolute path. It does not need to
    /// exist.
    ///
    /// For example, if we have the mount points:
    ///
    /// ```text
    /// TARGET      SOURCE     OPTIONS
    /// /a          /dev/sdaA  rw
    /// ├── /a/b    /dev/sdaB  rw
    /// └── /a/c    /dev/sdaC  rw
    /// ```
    ///
    /// Represented by the structure:
    ///
    /// ```ignore
    /// use std::path::PathBuf;
    /// use osutils::findmnt::MountpointMetadata;
    ///
    /// let mnt = MountpointMetadata {
    ///     target: PathBuf::from("/a"),
    ///     children: vec![
    ///         MountpointMetadata {
    ///             target: PathBuf::from("/a/b"),
    ///             // ...
    ///         },
    ///         MountpointMetadata {
    ///             target: PathBuf::from("/a/c"),
    ///             // ...
    ///         },
    ///     ],
    ///     ..Default::default()
    /// };
    /// ```
    ///
    /// Then:
    ///
    /// * `mnt.find_mount_point_for_path("/other")` will return `None`.
    /// * `mnt.find_mount_point_for_path("/a")` will return the the
    ///   `MountpointMetadata` for `/a`.
    /// * `mnt.find_mount_point_for_path("/a/b")` will return the the
    ///   `MountpointMetadata` for `/a/b`.
    /// * `mnt.find_mount_point_for_path("/a/c")` will return the the
    ///   `MountpointMetadata` for `/a/c`.
    /// * `mnt.find_mount_point_for_path("/a/d")` will return the the
    ///   `MountpointMetadata` for `/a`.
    /// * `mnt.find_mount_point_for_path("/a/b/c")` will return the the
    ///   `MountpointMetadata` for `/a/b`.
    /// * `mnt.find_mount_point_for_path("/a/b/c/d")` will return the the
    ///   `MountpointMetadata` for `/a/b`.
    pub fn find_mount_point_for_path(&self, path: impl AsRef<Path>) -> Option<&MountpointMetadata> {
        // If the path is not contained in this mount or its children, return
        // None
        if !self.contains_path(path.as_ref()) {
            return None;
        }

        // If the path is exactly this mount, return this mount
        if self.target == path.as_ref() {
            return Some(self);
        }

        Some(
            // Iterate over all children to see if any of them contain the path.
            self.children
                .iter()
                // Try to find if a child contains the path.
                .find_map(|child| child.find_mount_point_for_path(path.as_ref()))
                // At this point we know that self contains the path. If none of
                // the children contain the path, then self is the mount point.
                .unwrap_or(self),
        )
    }

    /// Returns whether the mount point or any of its children contain the given
    /// path.
    pub fn contains_path(&self, path: impl AsRef<Path>) -> bool {
        path.as_ref().starts_with(&self.target)
    }

    /// Returns a vec with the current and all child mount points in
    /// depth-first order.
    pub fn traverse_depth(&self) -> Vec<&MountpointMetadata> {
        std::iter::once(self)
            .chain(
                self.children
                    .iter()
                    .flat_map(MountpointMetadata::traverse_depth),
            )
            .collect()
    }

    /// Returns whether this mount point is unbindable.
    pub fn is_unbindable(&self) -> bool {
        self.has_propagation_type(PROPAGATION_UNBINDABLE)
    }

    /// Returns whether the propagation type is present in the mount point.
    fn has_propagation_type(&self, propagation_type: &str) -> bool {
        self.propagation
            .split(',')
            .any(|propagation| propagation.trim() == propagation_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::vec;

    fn sample_json() -> &'static str {
        // Sample output of `findmnt` from an AzL2.0 MOS. Full command:
        //
        // findmnt --json -o "AVAIL,FREQ,FSROOT,FSTYPE,FS-OPTIONS,ID,LABEL,"\
        // "MAJ:MIN,OPTIONS,OPT-FIELDS,PARENT,PARTLABEL,PARTUUID,PASSNO,"\
        // "PROPAGATION,SIZE,SOURCE,TARGET,TID,USED,USE%,UUID,VFS-OPTIONS"
        include_str!("test_files/findmnt.json")
    }

    fn sample_json_null_source() -> &'static str {
        // Sample output of `findmnt` from a scenario with containerd and tardev-snapshotter, where
        // the source of the overlay mount is reported as `null`.
        //
        // Command:
        // findmnt --json -o "AVAIL,FREQ,FSROOT,FSTYPE,FS-OPTIONS,ID,LABEL,"\
        // "MAJ:MIN,OPTIONS,OPT-FIELDS,PARENT,PARTLABEL,PARTUUID,PASSNO,"\
        // "PROPAGATION,SIZE,SOURCE,TARGET,TID,USED,USE%,UUID,VFS-OPTIONS"
        //
        // Note: null sources have been artificially injected on some overlayfs
        include_str!("test_files/findmnt-null-source.json")
    }

    #[test]
    fn test_findmnt() {
        FindMnt::from_json(sample_json()).unwrap();

        FindMnt::from_json(sample_json_null_source()).unwrap();
    }

    #[test]
    fn test_prune_prefix() {
        // Get a reference to the root filesystem
        let mut root = FindMnt::from_json(sample_json()).unwrap().root().unwrap();

        // Assert that the root filesystem contains a mount point on `/dev`
        assert!(root.contains_mountpoint("/dev"));

        // Prune the `/dev` mount point
        root.prune_prefix("/dev");

        // Assert that the root filesystem no longer contains a mount point on `/dev`
        assert!(!root.contains_mountpoint("/dev"));

        // Assert that there is a mount point on `/sys/kernel/tracing` and `/sys/kernel/debug`
        assert!(root.contains_mountpoint("/sys/kernel/tracing"));
        assert!(root.contains_mountpoint("/sys/kernel/debug"));

        // Assert there is a mount point on `/sys/fs/cgroup`
        assert!(root.contains_mountpoint("/sys/fs/cgroup"));

        // Prune everything under `/sys/kernel`
        root.prune_prefix("/sys/kernel");

        // Assert that there is still a mount point on `/sys`
        assert!(root.contains_mountpoint("/sys"));

        // Assert there is still a mount point on `/sys/fs/cgroup`
        assert!(root.contains_mountpoint("/sys/fs/cgroup"));

        // Assert that there is no longer a mount point on `/sys/kernel/tracing` and `/sys/kernel/debug`
        assert!(!root.contains_mountpoint("/sys/kernel/tracing"));
        assert!(!root.contains_mountpoint("/sys/kernel/debug"));
    }

    #[test]
    fn test_traverse_depth() {
        let root = FindMnt::from_json(sample_json()).unwrap().root().unwrap();

        // Make a stack to validate we are traversing in depth-first order
        let mut stack = vec![(&root, root.children.len())];
        let mut mountpoints = root.traverse_depth().into_iter();

        assert_eq!(
            mountpoints.next().unwrap(),
            stack.last().unwrap().0,
            "Expected root mount to be on top of the stack"
        );

        for next_mp in mountpoints {
            let (top, remaining_children) = {
                // While the top mount point has no remaining children, pop it from the stack
                while stack.last().unwrap().1 == 0 {
                    stack.pop();
                }

                stack.last_mut().unwrap()
            };

            assert!(
                top.children.contains(next_mp),
                "Expected {} to be a child of {}",
                next_mp.target.display(),
                top.target.display()
            );

            // Decrement the remaining children of the top mount point
            *remaining_children -= 1;

            // Push the next mount point and its children count to the stack
            stack.push((next_mp, next_mp.children.len()));
        }

        assert_eq!(
            stack[0].0.target,
            Path::new(ROOT_MOUNT_POINT_PATH),
            "Expected stack to only contain the root mountpoint after traversing all mount points, got:\n{stack:#?}"
        );
    }

    #[test]
    fn test_contains_mountpoint() {
        let root = FindMnt::from_json(sample_json()).unwrap().root().unwrap();

        root.traverse_depth().iter().for_each(|fs| {
            assert!(
                root.contains_mountpoint(&fs.target),
                "Expected root mount to contain submount {}",
                fs.target.display()
            )
        });

        assert!(
            !root.contains_mountpoint("/nonexistent"),
            "Expected root mount to not contain /nonexistent"
        );

        assert!(
            !root.contains_mountpoint("/sys/kernel"),
            "Expected root mount to not contain /sys/kernel"
        );
    }

    #[test]
    fn test_contains_path() {
        let root = FindMnt::from_json(sample_json()).unwrap().root().unwrap();

        root.traverse_depth().iter().for_each(|fs| {
            assert!(
                fs.contains_path(&fs.target),
                "Expected {} to contain itself",
                fs.target.display()
            )
        });

        assert!(
            root.contains_path("/"),
            "Expected root mount to contain itself"
        );

        assert!(
            root.contains_path("/sys"),
            "Expected root mount to contain /sys"
        );

        assert!(
            root.contains_path("/sys/fs/cgroup"),
            "Expected root mount to contain /sys/fs/cgroup"
        );

        // This is out of the root FS
        assert!(
            !root.contains_path("C:\\nonexistent"),
            "Expected root mount to not contain C:\\nonexistent"
        );
    }

    #[test]
    fn test_get_mount_point_for_path() {
        let root = FindMnt::from_json(sample_json()).unwrap().root().unwrap();

        root.traverse_depth().iter().for_each(|fs| {
            // First assert that the mount point for each mount point is itself
            assert_eq!(
                root.find_mount_point_for_path(&fs.target).unwrap().target,
                fs.target,
                "The mount point for {} should be itself",
                fs.target.display()
            );

            // Then add some arbitrary path to each mount point and assert that
            // the mount point for that path is the mount point itself
            let arbitrary_child = fs.target.join("arbitrary-path");
            assert_eq!(
                root.find_mount_point_for_path(&arbitrary_child)
                    .unwrap()
                    .target,
                fs.target,
                "The mount point for {} should be {}",
                arbitrary_child.display(),
                fs.target.display()
            );
        });
    }

    #[test]
    fn test_propagation_parsing() {
        let root = FindMnt::from_json(sample_json()).unwrap().root().unwrap();

        root.traverse_depth().iter().for_each(|fs| {
            let mut unbindable: bool = false;

            fs.propagation.split(',').for_each(|propagation| {
                if propagation == PROPAGATION_UNBINDABLE {
                    unbindable = true;
                }
            });

            assert_eq!(fs.is_unbindable(), unbindable);
        });
    }
}

/// Functional tests for the `findmnt` module.
#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_tests {
    use std::path::Path;

    use pytest_gen::functional_test;
    use trident_api::constants::ROOT_MOUNT_POINT_PATH;

    use super::FindMnt;

    #[functional_test(feature = "helpers")]
    fn test_findmnt_real() {
        let findmnt = FindMnt::run_real().unwrap();
        assert!(!findmnt.filesystems.is_empty());

        // On the funtional test VM we're expecting to have a root filesystem
        // backed by a partition, so it should appear in the output of `findmnt
        // --real`.
        let root = findmnt.root().unwrap();

        // All real filesystems on the functional test VM should be backed by a
        // block device in /dev.
        for mount in root.traverse_depth() {
            let source = mount.source.as_ref().unwrap();

            assert!(
                source.starts_with("/dev/"),
                "Mount {} is not backed by a block device in /dev",
                mount.target.display()
            );
        }
    }

    #[functional_test(feature = "helpers")]
    fn test_findmnt_run() {
        let findmnt = FindMnt::run().unwrap();
        let mut root = findmnt.root().unwrap();
        assert_eq!(root.target, Path::new(ROOT_MOUNT_POINT_PATH));

        let assert_mountpoint_exists = |target: &str| {
            if !root.contains_mountpoint(target) {
                panic!("Mount point {target} not found")
            }
        };

        // Check multiple common mount points
        assert_mountpoint_exists("/dev");
        assert_mountpoint_exists("/sys");
        assert_mountpoint_exists("/proc");
        assert_mountpoint_exists("/run");
        assert_mountpoint_exists("/boot/efi");

        root.prune_prefix("/dev");

        // Check that the mount point was pruned
        assert!(
            !root.contains_mountpoint("/dev"),
            "Mount point /dev should have been pruned"
        );
    }
}
