use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Error};
use log::{debug, error, info};
use regex::Regex;
use serde::{Deserialize, Serialize};
use trident_api::config::RaidLevel;

use crate::{exe::RunAndCheck, lsblk};

pub const METADATA_VERSION: &str = "1.0";

pub fn create(
    raid_path: &PathBuf,
    level: &RaidLevel,
    device_paths: Vec<PathBuf>,
) -> Result<(), Error> {
    info!("Creating RAID array '{}'", &raid_path.display());

    let mut mdadm_command = Command::new("mdadm");
    mdadm_command
        .arg("--create")
        .arg(raid_path)
        .arg(format!("--level={}", &level))
        .arg(format!("--raid-devices={}", &device_paths.len()))
        .args(&device_paths)
        .arg(format!("--metadata={METADATA_VERSION}"));

    mdadm_command
        .run_and_check()
        .context("Failed to run mdadm create")
}

pub fn examine() -> Result<String, Error> {
    info!("Examining RAID arrays");

    let mut mdadm_command = Command::new("mdadm");
    mdadm_command.arg("--examine").arg("--scan");

    mdadm_command
        .output_and_check()
        .context("Failed to run mdadm examine")
}

pub fn stop(raid_array_name: impl AsRef<Path>) -> Result<(), Error> {
    info!(
        "Stopping RAID array: {}",
        raid_array_name.as_ref().display()
    );

    if let Err(e) = Command::new("mdadm")
        .arg("--stop")
        .arg(raid_array_name.as_ref())
        .run_and_check()
        .context("Failed to run mdadm stop device")
    {
        // If stop returns an error, do best effort to log what is holding the
        // block device
        let block_device = lsblk::run(raid_array_name.as_ref());
        if let Ok(block_device) = block_device {
            error!(
                "Failed to stop {}: active children: {:?}, active mount points: {:?}",
                raid_array_name.as_ref().display(),
                block_device.children,
                block_device.mountpoints
            );
        }

        // Propagate the original unmount error
        return Err(e.context(format!(
            "Failed to stop RAID array {}",
            raid_array_name.as_ref().display()
        )));
    }

    Ok(())
}

/// Adds a device to a RAID array.
///
/// This function uses `mdadm --add` to add the specified device to the given RAID array.
///
pub fn add(raid_path: impl AsRef<Path>, device: impl AsRef<Path>) -> Result<(), Error> {
    info!(
        "Adding RAID device '{}' to '{}'",
        device.as_ref().display(),
        raid_path.as_ref().display()
    );

    Command::new("mdadm")
        .arg(raid_path.as_ref())
        .arg("--add")
        .arg(device.as_ref())
        .run_and_check()
        .context("Failed to run mdadm add device")
}

/// Marks a device as failed in a RAID array.
///
/// This function uses `mdadm --fail` to mark the specified device as failed in
/// the given RAID array.
///
pub fn fail(raid_path: impl AsRef<Path>, device: impl AsRef<Path>) -> Result<(), Error> {
    info!(
        "Marking RAID device '{}' as failed for '{}'",
        device.as_ref().display(),
        raid_path.as_ref().display()
    );

    Command::new("mdadm")
        .arg(raid_path.as_ref())
        .arg("--fail")
        .arg(device.as_ref())
        .run_and_check()
        .context("Failed to run mdadm fail device")
}

/// Removes a device from a RAID array.
///
/// This function uses `mdadm --remove` to remove the specified device from the
/// given RAID array.
///
pub fn remove(raid_path: impl AsRef<Path>, device: impl AsRef<Path>) -> Result<(), Error> {
    info!(
        "Removing RAID device: '{}' from '{}'",
        device.as_ref().display(),
        raid_path.as_ref().display()
    );

    Command::new("mdadm")
        .arg(raid_path.as_ref())
        .arg("--remove")
        .arg(device.as_ref())
        .run_and_check()
        .context("Failed to run mdadm remove device")
}

#[derive(Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq, Default)]
pub struct MdadmDetail {
    pub raid_path: PathBuf,
    pub level: String,
    pub uuid: String,
    pub devices: Vec<PathBuf>,
}

pub fn details() -> Result<Vec<MdadmDetail>, Error> {
    debug!("Getting details for all RAID arrays");

    let mut mdadm_command = Command::new("mdadm");
    mdadm_command.arg("--detail").arg("--scan").arg("--verbose");

    let output = mdadm_command
        .output_and_check()
        .context("Failed to run mdadm detail")?;

    mdadm_detail_to_struct(&output).context("Failed to parse mdadm detail")
}

pub fn detail(raid_array: &Path) -> Result<MdadmDetail, Error> {
    debug!("Getting RAID array details for '{}'", raid_array.display());

    let mut mdadm_command = Command::new("mdadm");
    mdadm_command
        .arg("--detail")
        .arg("--scan")
        .arg("--verbose")
        .arg(raid_array);

    let output = mdadm_command
        .output_and_check()
        .context("Failed to run mdadm detail")?;

    let structured_output =
        mdadm_detail_to_struct(&output).context("Failed to parse mdadm detail")?;

    if structured_output.len() != 1 {
        return Err(anyhow::anyhow!("Failed to get RAID array details"));
    }

    Ok(structured_output[0].clone())
}

fn mdadm_detail_to_struct(mdadm_output: &str) -> Result<Vec<MdadmDetail>, Error> {
    let mut mdadm_details = Vec::new();

    let array_regex = Regex::new(r"ARRAY\s+(/dev/md\S+)").unwrap();
    let level_regex = Regex::new(r"(?:^|\s)level=(\w+)").unwrap();
    let uuid_regex = Regex::new(r"(?:^|\s)UUID=([\da-zA-Z:]+)").unwrap();
    let devices_regex = Regex::new(r"(?:^|\s)devices=([^=]+)").unwrap();

    let mut current_mdadm_detail = MdadmDetail::default();

    for line in mdadm_output.lines() {
        if let Some(captures) = array_regex.captures(line) {
            current_mdadm_detail.raid_path = PathBuf::from(
                captures
                    .get(1)
                    .context("Failed to parse RAID path from details")?
                    .as_str(),
            );
        }
        if let Some(captures) = level_regex.captures(line) {
            current_mdadm_detail.level = captures
                .get(1)
                .context("Failed to parse RAID level from details")?
                .as_str()
                .to_string();
        }
        if let Some(captures) = uuid_regex.captures(line) {
            current_mdadm_detail.uuid = captures
                .get(1)
                .context("Failed to parse RAID UUID from details")?
                .as_str()
                .to_string();
        }
        if let Some(captures) = devices_regex.captures(line) {
            current_mdadm_detail.devices = captures
                .get(1)
                .context("Failed to parse RAID devices from details")?
                .as_str()
                .split(',')
                .map(PathBuf::from)
                .collect();

            mdadm_details.push(current_mdadm_detail.clone());
            current_mdadm_detail = MdadmDetail::default();
        }
    }

    Ok(mdadm_details)
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use super::*;

    #[test]
    fn test_mdadm_detail_to_struct() {
        let mdadm_detail_output = indoc!(
            r#"
        ARRAY /dev/md/my-raid2 level=raid1 num-devices=2 metadata=1.0 name=localhost:my-raid2 UUID=6245349d:505a367b:6ceba75f:7f55c158
            devices=/dev/sda8,/dev/sda9
        ARRAY /dev/md/my-raid level=raid1 num-devices=2 metadata=1.0 name=localhost:my-raid UUID=ea381b70:20b2ab81:602edecb:cf6f2032
            devices=/dev/sda6,/dev/sda7
        "#
        );
        let details =
            mdadm_detail_to_struct(mdadm_detail_output).expect("Failed to parse mdadm detail");
        let expected_details: Vec<MdadmDetail> = [
            MdadmDetail {
                raid_path: PathBuf::from("/dev/md/my-raid2"),
                level: "raid1".to_string(),
                uuid: "6245349d:505a367b:6ceba75f:7f55c158".to_string(),
                devices: ["/dev/sda8".into(), "/dev/sda9".into()].into(),
            },
            MdadmDetail {
                raid_path: PathBuf::from("/dev/md/my-raid"),
                level: "raid1".to_string(),
                uuid: "ea381b70:20b2ab81:602edecb:cf6f2032".to_string(),
                devices: ["/dev/sda6".into(), "/dev/sda7".into()].into(),
            },
        ]
        .to_vec();

        assert_eq!(details, expected_details);

        // different raid name format
        let mdadm_detail_output = indoc!(
            r#"
        ARRAY /dev/md126 level=raid1 num-devices=2 metadata=1.0 name=localhost:my-raid2 UUID=602edecb:505a367b:6ceba75f:602edecb
            devices=/dev/sda8,/dev/sda9
        ARRAY /dev/md127 level=raid1 num-devices=2 metadata=1.0 name=localhost:my-raid UUID=as381b70:20b2ab81:602edecb:cf6f20as
            devices=/dev/sda6,/dev/sda7
        "#
        );
        let details =
            mdadm_detail_to_struct(mdadm_detail_output).expect("Failed to parse mdadm detail");
        let expected_details: Vec<MdadmDetail> = [
            MdadmDetail {
                raid_path: PathBuf::from("/dev/md126"),
                level: "raid1".to_string(),
                uuid: "602edecb:505a367b:6ceba75f:602edecb".to_string(),
                devices: ["/dev/sda8".into(), "/dev/sda9".into()].into(),
            },
            MdadmDetail {
                raid_path: PathBuf::from("/dev/md127"),
                level: "raid1".to_string(),
                uuid: "as381b70:20b2ab81:602edecb:cf6f20as".to_string(),
                devices: ["/dev/sda6".into(), "/dev/sda7".into()].into(),
            },
        ]
        .to_vec();

        assert_eq!(details, expected_details);

        // empty
        let mdadm_detail_output = r#""#;
        let details =
            mdadm_detail_to_struct(mdadm_detail_output).expect("Failed to parse mdadm detail");
        let expected_details: Vec<MdadmDetail> = [].to_vec();

        assert_eq!(details, expected_details);
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {

    use super::*;
    use pytest_gen::functional_test;
    use std::path::PathBuf;

    const NON_EXISTENT_RAID_DEVICE: &str = "/dev/md/non-existent-path";

    #[functional_test(feature = "helpers", negative = true)]
    fn test_raid_detail_failure() {
        assert_eq!(
            self::detail(&PathBuf::from(NON_EXISTENT_RAID_DEVICE))
                .unwrap_err()
                .to_string(),
            "Failed to run mdadm detail"
        );
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_raid_stop_failure() {
        assert_eq!(
            self::stop(PathBuf::from(NON_EXISTENT_RAID_DEVICE))
                .unwrap_err()
                .to_string(),
            format!("Failed to stop RAID array {}", NON_EXISTENT_RAID_DEVICE)
        );
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_raid_add_failure() {
        assert_eq!(
            self::add(
                PathBuf::from(NON_EXISTENT_RAID_DEVICE),
                PathBuf::from(NON_EXISTENT_RAID_DEVICE)
            )
            .unwrap_err()
            .to_string(),
            "Failed to run mdadm add device"
        );
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_raid_fail_failure() {
        assert_eq!(
            self::fail(
                PathBuf::from(NON_EXISTENT_RAID_DEVICE),
                PathBuf::from(NON_EXISTENT_RAID_DEVICE)
            )
            .unwrap_err()
            .to_string(),
            "Failed to run mdadm fail device"
        );
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_raid_remove_failure() {
        assert_eq!(
            self::remove(
                PathBuf::from(NON_EXISTENT_RAID_DEVICE),
                PathBuf::from(NON_EXISTENT_RAID_DEVICE)
            )
            .unwrap_err()
            .to_string(),
            "Failed to run mdadm remove device"
        );
    }
}
