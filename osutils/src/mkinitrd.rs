use std::{io::Write, os::unix::fs::PermissionsExt, path::Path, process::Command};

use anyhow::{Context, Error};
use tempfile::NamedTempFile;
use trident_api::error::{ReportError, ServicingError, TridentError};

use crate::exe::RunAndCheck;

/// Verity workaround script
///
/// We rely on systemd's `Uphold=` directive to ensure that the
/// `systemd-veritysetup@root.service` service is started automatically if/when
/// the required partitions become available.
///
/// The script below scans the `BindsTo=` directive of the service file and
/// creates an override file for each partition listed in the directive.
pub const VERITY_RACE_CONDITION_WORKAROUND: &str = r#"
SERVICE_FILE=/run/systemd/generator/systemd-veritysetup\@root.service
OVERRIDE_ROOT=/etc/systemd/system

if [ -f $SERVICE_FILE ]; then
    echo "File $SERVICE_FILE exists. Injecting verity workaround..."
    SERVICE_NAME=$(basename $SERVICE_FILE)
    echo "Service name: $SERVICE_NAME"
    PARTITIONS=$(cat $SERVICE_FILE | sed -n 's/BindsTo=//p')
    for PARTITION in $PARTITIONS; do
        echo "Injecting override for partition: $PARTITION"
        mkdir -p $OVERRIDE_ROOT/$PARTITION.d/
        OVERRIDE_FILE=$OVERRIDE_ROOT/$PARTITION.d/override.conf
        cat << EOF > $OVERRIDE_FILE
[Unit]
Upholds=$SERVICE_NAME
EOF
        echo "Created '$OVERRIDE_FILE' with contents:"
        cat $OVERRIDE_FILE
        printf "\n"
    done
    systemctl daemon-reload
fi
"#;

/// Generate a new initrd image using either mkinitrd or dracut.
///
/// If mkinitrd is available, it will be used. Azl 3.0 doesn't have mkinitrd anymore, so dracut is
/// used instead.
pub fn execute() -> Result<(), TridentError> {
    if Path::new("/usr/bin/mkinitrd").exists() {
        Command::new("mkinitrd")
            .run_and_check()
            .structured(ServicingError::RegenerateInitrd)
    } else {
        run_darcut().structured(ServicingError::RegenerateInitrd)
    }
}

/// Wrapper around dracut to regenerate the initrd with specific options
fn run_darcut() -> Result<(), Error> {
    // Create a temp file
    let mut script = NamedTempFile::new().context("Failed to create temporary file")?;
    // Write the worakround script to the temp file
    script
        .write(VERITY_RACE_CONDITION_WORKAROUND.as_bytes())
        .context("Failed to write script to temporary file")?;

    // Flush the temp file
    script.flush().context("Failed to flush temporary file")?;

    // Set the permissions of the temp file to 755
    std::fs::set_permissions(script.path(), std::fs::Permissions::from_mode(0o755))
        .context("Failed to set permissions of temporary file")?;

    Command::new("dracut")
        .arg("--force")
        .arg("--regenerate-all")
        .arg("--zstd")
        .arg("--include")
        .arg("/usr/lib/locale")
        .arg("/usr/lib/locale")
        .arg("--include")
        .arg(script.path())
        .arg("/lib/dracut/hooks/cmdline/10-verity-workaround.sh")
        .run_and_check()
        .context("Failed to run dracut")
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;

    use crate::osrelease;

    #[functional_test]
    fn test_regenerate_initrd() {
        let pattern = if osrelease::is_azl3().unwrap() {
            "/boot/initramfs-*.azl3.img"
        } else {
            "/boot/initrd.img-*"
        };

        let initrd_path = glob::glob(pattern).unwrap().next();
        let original = &initrd_path;
        if let Some(initrd_path) = &initrd_path {
            std::fs::remove_file(initrd_path.as_ref().unwrap()).unwrap();
        }

        execute().unwrap();

        // Some initrd should have been created
        let initrd_path = glob::glob(pattern).unwrap().next();
        assert!(initrd_path.is_some());

        // And the filename should match the original, if it previously existed
        if let Some(original) = original {
            let initrd_path = initrd_path.unwrap().unwrap();
            assert_eq!(original.as_ref().unwrap(), &initrd_path);
        }
    }
}
