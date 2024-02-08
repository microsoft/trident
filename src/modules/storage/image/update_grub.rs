use std::{
    fs::{self},
    path::Path,
    process::Command,
};

use anyhow::{Context, Error};
use osutils::{exe::RunAndCheck, grub::GrubConfig};
use uuid::Uuid;

/// Updates the boot filesystem UUID on the search command inside the GRUB
/// config.
pub fn update_grub_config_esp(grub_config_path: &Path, boot_fs_uuid: &Uuid) -> Result<(), Error> {
    let mut grub_config = GrubConfig::read(grub_config_path)?;
    grub_config.update_search(boot_fs_uuid)?;
    grub_config.write()
}

/// Updates the boot filesystem UUID on the search command and the rootdevice
/// inside the GRUB config.
pub fn update_grub_config_boot(
    grub_config_path: &Path,
    boot_fs_uuid: &Uuid,
    root_device_path: &Path,
) -> Result<(), Error> {
    let mut grub_config = GrubConfig::read(grub_config_path)?;

    // TODO(6775): re-enable selinux
    grub_config.disable_selinux();

    grub_config.update_search(boot_fs_uuid)?;

    grub_config.update_rootdevice(root_device_path)?;

    grub_config.write()
}

/// Returns the UUID of a block device at the given path. To note, this will
/// fail on filesystems such as vfat, which do not use full UUID.
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
        "Failed to get UUID for path '{}', received '{}'",
        canonical_path.display(),
        output
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;
    use std::fs;
    use uuid::Uuid;

    fn get_original_grub_content() -> (&'static str, &'static str) {
        // Define original GRUB config contents on target machine
        let original_content_grub_boot = indoc! {r#"
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
                    linux $bootprefix/$mariner_linux   rd.auto=1 root=$rootdevice $mariner_cmdline lockdown=integrity sysctl.kernel.unprivileged_bpf_disabled=1 $systemd_cmdline console=tty0 console=ttyS0 $kernelopts
                    if [ -f $bootprefix/$mariner_initrd ]; then
                            initrd $bootprefix/$mariner_initrd
                    fi
            }"#};

        let original_content_grub_esp = indoc! {r#"search -n -u febfaaaa-fec4-4682-aee2-54f2d46b39ae -s

            # If '/boot' is a seperate partition, BootUUID will point directly to '/boot'.
            # In this case we should omit the '/boot' prefix from all paths.
            set bootprefix=/boot
            configfile $bootprefix/grub2/grub.cfg"#};

        (original_content_grub_boot, original_content_grub_esp)
    }

    fn get_expected_grub_content(
        random_uuid_grub_boot: String,
        root_path: Option<&Path>,
        random_uuid_grub_esp: String,
    ) -> (String, String) {
        // Define expected GRUB config contents after updating the rootfs UUID
        let (original_content_grub_boot, original_content_grub_esp) = get_original_grub_content();
        // Build the expected content with the new UUID
        let expected_content_grub_boot = original_content_grub_boot
            .replace(
                "PARTUUID=29f8eed2-3c85-4da0-b32e-480e54379766",
                root_path.unwrap().to_str().unwrap(),
            )
            .replace(
                "9e6a9d2c-b7fe-4359-ac45-18b505e29d8b",
                &random_uuid_grub_boot,
            );

        // Build the expected content with the new UUID
        let expected_content_grub_esp = original_content_grub_esp.replace(
            "febfaaaa-fec4-4682-aee2-54f2d46b39ae",
            &random_uuid_grub_esp,
        );

        (expected_content_grub_boot, expected_content_grub_esp)
    }

    #[test]
    fn test_update_grub_config_random_rootuuid() {
        let (original_content_grub_boot, original_content_grub_esp) = get_original_grub_content();

        // Create a temporary file and write the original content to it
        let temp_file_grub = tempfile::NamedTempFile::new().unwrap();
        let temp_file_path_grub = temp_file_grub.path();
        fs::write(temp_file_path_grub, original_content_grub_boot).unwrap();

        // Generate random FS UUID and root path for the partition
        let random_uuid_grub_boot = Uuid::new_v4();
        let random_uuid_grub_esp = Uuid::new_v4();
        let root_path = Path::new("/dev/sda1");

        update_grub_config_boot(temp_file_path_grub, &random_uuid_grub_boot, root_path).unwrap();

        // Read back the content of the file
        let updated_content_grub = fs::read_to_string(temp_file_path_grub).unwrap();
        let (expected_content_grub_boot, expected_content_grub_esp) = get_expected_grub_content(
            random_uuid_grub_boot.to_string(),
            Some(root_path),
            random_uuid_grub_esp.clone().to_string(),
        );

        // Assert that the updated content matches the expected content
        assert_eq!(updated_content_grub, expected_content_grub_boot);

        let temp_file_grub2 = tempfile::NamedTempFile::new().unwrap();
        let temp_file_path_grub_esp = temp_file_grub2.path();
        fs::write(temp_file_path_grub_esp, original_content_grub_esp).unwrap();

        update_grub_config_esp(temp_file_path_grub_esp, &random_uuid_grub_esp).unwrap();

        // Read back the content of the file
        let updated_content_grub_esp = fs::read_to_string(temp_file_path_grub_esp).unwrap();

        // Assert that the updated content matches the expected content
        assert_eq!(updated_content_grub_esp, expected_content_grub_esp);
    }
}
