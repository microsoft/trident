use std::{
    fs::{self},
    path::Path,
    process::Command,
};

use anyhow::{bail, Context, Error};
use osutils::exe::RunAndCheck;
use regex::Regex;
use uuid::Uuid;

/// The path to the GRUB configuration on a volume.
pub const GRUB_BOOT_CONFIG_PATH: &str = "boot/grub2/grub.cfg";

/// Updates the root filesystem UUID inside the GRUB config.
pub fn update_grub_config(
    grub_config: &Path,
    root_fs_uuid: &str,
    root_device_path: Option<&Path>,
) -> Result<(), Error> {
    // Read the GRUB config file as a string
    let grub_config_path = Path::new(grub_config);

    if !grub_config_path.exists() {
        bail!(
            "GRUB config does not exist at path: {}",
            grub_config.display()
        );
    }
    let mut file_content = fs::read_to_string(grub_config)
        .context("Failed to read the GRUB config file '{grub_config}'")?;

    let re_uuid = Regex::new(r"search -n -u [\w-]+ -s").unwrap();
    let re_partuuid = Regex::new(r"set rootdevice=PARTUUID=[\w-]+").unwrap();

    // Update the grub content
    file_content = re_uuid
        .replace(
            &file_content,
            &format!("search -n -u {} -s", root_fs_uuid.trim()),
        )
        .to_string();
    if let Some(root_device_path) = root_device_path {
        file_content = re_partuuid
            .replace(
                &file_content,
                &format!(
                    "set rootdevice={}",
                    root_device_path
                        .to_str()
                        .context("Failed to convert root device path to string")?
                        .trim()
                ),
            )
            .to_string()
    }
    fs::write(grub_config, file_content).context("failed to write the updated grub content")
}

/// Returns the UUID of a block device at the given path.
pub fn get_uuid_from_path(block_device_path: &Path) -> Result<Uuid, Error> {
    // Canonicalize the path
    let canonical_path = fs::canonicalize(block_device_path).with_context(|| {
        format!(
            "Failed to canonicalize the path '{}'",
            block_device_path.display()
        )
    })?;

    // Run the blkid command to fetch block devices
    let output = Command::new("blkid")
        .arg("-o")
        .arg("value")
        .arg("-s")
        .arg("UUID")
        .arg(&canonical_path)
        .output_and_check()
        .context("failed to run blkid command to fetch block devices")?;

    Uuid::parse_str(output.trim()).context(format!(
        "Failed to get UUID for path '{}'",
        canonical_path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use uuid::Uuid;
    #[test]
    fn test_update_grub_config() {
        // Define original GRUB config contents on target machine
        let original_content_grub = r#"
            set timeout=0
            set bootprefix=/boot
            search -n -u 9e6a9d2c-b7fe-4359-ac45-18b505e29d8b -s

            load_env -f $bootprefix/mariner.cfg
            if [ -f  $bootprefix/systemd.cfg ]; then
                    load_env -f $bootprefix/systemd.cfg
            else
                    set systemd_cmdline=net.ifnames=0
            fi
            if [ -f $bootprefix/grub2/grubenv ]; then
                    load_env -f $bootprefix/grub2/grubenv
            fi

            set rootdevice=PARTUUID=29f8eed2-3c85-4da0-b32e-480e54379766

            menuentry "CBL-Mariner" {
                    linux $bootprefix/$mariner_linux security=selinux selinux=1 rd.auto=1 root=$rootdevice $mariner_cmdline lockdown=integrity sysctl.kernel.unprivileged_bpf_disabled=1 $systemd_cmdline console=tty0 console=ttyS0 $kernelopts
                    if [ -f $bootprefix/$mariner_initrd ]; then
                            initrd $bootprefix/$mariner_initrd
                    fi
            }"#;

        // Create a temporary file and write the original content to it
        let temp_file_grub = tempfile::NamedTempFile::new().unwrap();
        let temp_file_path_grub = temp_file_grub.path();

        fs::write(temp_file_path_grub, original_content_grub).unwrap();

        // Generate random FS UUID and PARTUUID for the partition
        let random_uuid_grub = Uuid::new_v4().to_string();
        let root_path = Path::new("/dev/sda1");

        // Call update_grub_rootfs()
        update_grub_config(temp_file_path_grub, &random_uuid_grub, Some(root_path)).unwrap();
        // Read back the content of the file
        let updated_content_grub = fs::read_to_string(temp_file_path_grub).unwrap();

        // Build the expected content with the new UUID
        let expected_content_grub = original_content_grub
            .replace(
                "PARTUUID=29f8eed2-3c85-4da0-b32e-480e54379766",
                root_path.to_str().unwrap(),
            )
            .replace("9e6a9d2c-b7fe-4359-ac45-18b505e29d8b", &random_uuid_grub);

        // Assert that the updated content matches the expected content
        assert_eq!(updated_content_grub, expected_content_grub);

        let original_content_grub2 = r#"search -n -u febfaaaa-fec4-4682-aee2-54f2d46b39ae -s

            # If '/boot' is a seperate partition, BootUUID will point directly to '/boot'.
            # In this case we should omit the '/boot' prefix from all paths.
            set bootprefix=/boot
            configfile $bootprefix/grub2/grub.cfg"#;

        let temp_file_grub2 = tempfile::NamedTempFile::new().unwrap();
        let temp_file_path_grub2 = temp_file_grub2.path();

        fs::write(temp_file_path_grub2, original_content_grub2).unwrap();

        // Generate a random UUID for the partition
        let random_uuid_grub2 = Uuid::new_v4().to_string();

        // Call update_grub_rootfs() with None as 2nd arg since no need to update
        // PARTUUID of root partition
        update_grub_config(temp_file_path_grub2, &random_uuid_grub2, None).unwrap();

        // Read back the content of the file
        let updated_content_grub2 = fs::read_to_string(temp_file_path_grub2).unwrap();

        // Build the expected content with the new UUID
        let expected_content_grub2 = original_content_grub2
            .replace("febfaaaa-fec4-4682-aee2-54f2d46b39ae", &random_uuid_grub2);

        // Assert that the updated content matches the expected content
        assert_eq!(updated_content_grub2, expected_content_grub2);
    }
}
