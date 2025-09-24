use std::path::Path;

use anyhow::{Context, Error};

use crate::dependencies::Dependency;

/// Creates a swap space on the specified device path.
pub fn mkswap(device_path: impl AsRef<Path>) -> Result<(), Error> {
    Dependency::Mkswap
        .cmd()
        .arg("--verbose")
        .arg(device_path.as_ref())
        .run_and_check()
        .with_context(|| {
            format!(
                "Failed to execute mkswap on '{}'",
                device_path.as_ref().display()
            )
        })
}

/// Runs swapon on the specified device path.
pub fn swapon(device_path: impl AsRef<Path>) -> Result<(), Error> {
    Dependency::Swapon
        .cmd()
        .arg("--verbose")
        .arg(device_path.as_ref())
        .run_and_check()
        .with_context(|| {
            format!(
                "Failed to execute swapon on '{}'",
                device_path.as_ref().display()
            )
        })
}

/// Runs swapoff on the specified device path.
pub fn swapoff(device_path: impl AsRef<Path>) -> Result<(), Error> {
    Dependency::Swapoff
        .cmd()
        .arg("--verbose")
        .arg(device_path.as_ref())
        .run_and_check()
        .with_context(|| {
            format!(
                "Failed to execute swapoff on '{}'",
                device_path.as_ref().display()
            )
        })
}

/// Represents a swap space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwapSpace {
    pub name: String,
    pub swap_type: String,
    pub size: u64,
    pub priority: i32,
}

impl SwapSpace {
    pub fn read() -> Result<Vec<SwapSpace>, Error> {
        let output = Dependency::Swapon
            .cmd()
            .arg("--show=NAME,TYPE,SIZE,PRIO")
            .arg("--raw")
            .arg("--bytes")
            .arg("--noheadings")
            .output_and_check()
            .context("Failed to execute swapon")?;

        output
            .lines()
            .map(|line| {
                SwapSpace::from_str(line)
                    .with_context(|| format!("Failed to parse swap space line: {line}"))
            })
            .collect()
    }

    fn from_str(line: &str) -> Result<SwapSpace, Error> {
        let mut parts = line.split_whitespace();
        let name = parts
            .next()
            .context("Failed to parse swap space name")?
            .to_string();

        let swap_type = parts
            .next()
            .context("Failed to parse swap space type")?
            .to_string();

        let size = parts
            .next()
            .context("Failed to parse swap space size")?
            .parse::<u64>()
            .context("Failed to parse swap space size as integer")?;

        let priority = parts
            .next()
            .context("Failed to parse swap space priority")?
            .parse::<i32>()
            .context("Failed to parse swap space priority as integer")?;

        Ok(SwapSpace {
            name,
            swap_type,
            size,
            priority,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swap_space_parsing() {
        let swap_space = SwapSpace::from_str("/dev/sdb partition 1048576 -2").unwrap();
        assert_eq!(swap_space.name, "/dev/sdb");
        assert_eq!(swap_space.swap_type, "partition");
        assert_eq!(swap_space.size, 1048576);
        assert_eq!(swap_space.priority, -2);

        let swap_space = SwapSpace::from_str("/somefile file 1048576 500").unwrap();
        assert_eq!(swap_space.name, "/somefile");
        assert_eq!(swap_space.swap_type, "file");
        assert_eq!(swap_space.size, 1048576);
        assert_eq!(swap_space.priority, 500);
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;

    use crate::{
        filesystems::MkfsFileSystemType,
        lsblk, mkfs,
        testutils::repart::{self, TEST_DISK_DEVICE_PATH},
    };

    /// This function wipes the /dev/sdb device and ensures the /mnt
    /// directory exists.
    fn setup_test() {
        // Just zero-out the metadata so this is a fast operation.
        repart::clear_disk(Path::new(TEST_DISK_DEVICE_PATH)).unwrap();
        if !Path::new("/mnt").exists() {
            Dependency::Mkdir.cmd().arg("/mnt").run_and_check().unwrap();
        }
    }

    #[functional_test(feature = "helpers")]
    fn test_run_pass() {
        setup_test();

        // mkswap() on a zeroed block device should prepare it as a swap volume.
        mkswap(Path::new(TEST_DISK_DEVICE_PATH)).unwrap();

        // Ensure that the device is now a swap volume.
        assert_eq!(
            lsblk::get(TEST_DISK_DEVICE_PATH).unwrap().fstype,
            Some("swap".into()),
            "Expected swap filesystem type"
        );

        // Try to enable the swap.
        swapon(TEST_DISK_DEVICE_PATH).unwrap();

        // Ensure one swap space under the expected name is present.
        let swap = SwapSpace::read()
            .unwrap()
            .into_iter()
            .find(|s| s.name == TEST_DISK_DEVICE_PATH)
            .unwrap();
        assert_eq!(swap.name, TEST_DISK_DEVICE_PATH);
        assert_eq!(swap.swap_type, "partition");

        swapoff(TEST_DISK_DEVICE_PATH).unwrap();

        let err = swapoff(TEST_DISK_DEVICE_PATH).unwrap_err();
        println!("Error: {err:?}");
        assert_eq!(
            err.to_string(),
            format!("Failed to execute swapoff on '{TEST_DISK_DEVICE_PATH}'")
        );

        // run() on a formatted block device with a different filesystem
        // should reformat it as a swap.
        mkfs::run(Path::new(TEST_DISK_DEVICE_PATH), MkfsFileSystemType::Ext3).unwrap();
        assert_eq!(
            lsblk::get(TEST_DISK_DEVICE_PATH).unwrap().fstype,
            Some("ext3".into()),
            "Expected ext3 filesystem type"
        );

        mkswap(Path::new(TEST_DISK_DEVICE_PATH)).unwrap();
        assert_eq!(
            lsblk::get(TEST_DISK_DEVICE_PATH).unwrap().fstype,
            Some("swap".into()),
            "Expected swap filesystem type"
        );

        swapon(TEST_DISK_DEVICE_PATH).unwrap();
        swapoff(TEST_DISK_DEVICE_PATH).unwrap();
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_run_fail() {
        setup_test();

        // run() using device '/dev/foo' that doesn't exist should also
        // fail and again not clear the device contents.
        assert_eq!(
            mkswap(Path::new("/dev/foo")).unwrap_err().to_string(),
            "Failed to execute mkswap on '/dev/foo'"
        );

        // run() using a non-block device path should also fail.
        assert_eq!(
            mkswap(Path::new("/etc/passwd")).unwrap_err().to_string(),
            "Failed to execute mkswap on '/etc/passwd'"
        );
    }
}
