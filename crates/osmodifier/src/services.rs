// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Service management — enable and disable systemd services.

use std::process::Command;

use anyhow::{Context, Error};
use log::{debug, warn};

use trident_api::config::Services;

use crate::OsModifierContext;

/// Enable and disable the requested systemd services.
pub fn configure(ctx: &OsModifierContext, services: &Services) -> Result<(), Error> {
    for service in &services.enable {
        enable_service(ctx, service)?;
    }

    for service in &services.disable {
        disable_service(ctx, service)?;
    }

    Ok(())
}

fn enable_service(ctx: &OsModifierContext, service: &str) -> Result<(), Error> {
    debug!("Enabling service '{service}'");
    let root = ctx.root.to_str().unwrap_or("/");

    let output = Command::new("systemctl")
        .args(["--root", root, "enable", service])
        .output()
        .with_context(|| format!("Failed to execute systemctl enable {service}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to enable service '{service}': {stderr}");
    }

    Ok(())
}

fn disable_service(ctx: &OsModifierContext, service: &str) -> Result<(), Error> {
    // Check if the service is enabled first
    let root = ctx.root.to_str().unwrap_or("/");

    let check = Command::new("systemctl")
        .args(["--root", root, "is-enabled", service])
        .output()
        .with_context(|| format!("Failed to check if service '{service}' is enabled"))?;

    if !check.status.success() {
        warn!("Service '{service}' is not enabled, skipping disable");
        return Ok(());
    }

    debug!("Disabling service '{service}'");
    let output = Command::new("systemctl")
        .args(["--root", root, "disable", service])
        .output()
        .with_context(|| format!("Failed to execute systemctl disable {service}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to disable service '{service}': {stderr}");
    }

    Ok(())
}

#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    use pytest_gen::functional_test;
    use trident_api::config::Services;

    use crate::OsModifierContext;

    /// Create a minimal systemd tree with a synthetic service unit.
    fn setup_systemd_root(tmp: &std::path::Path) {
        let unit_dir = tmp.join("usr/lib/systemd/system");
        fs::create_dir_all(&unit_dir).unwrap();

        // systemctl --root needs these directories
        fs::create_dir_all(tmp.join("etc/systemd/system/multi-user.target.wants")).unwrap();

        fs::write(
            unit_dir.join("test-osmodifier.service"),
            "[Unit]\nDescription=Test Service\n\n[Service]\nType=oneshot\nExecStart=/bin/true\n\n[Install]\nWantedBy=multi-user.target\n",
        )
        .unwrap();
    }

    #[functional_test(feature = "core")]
    fn test_enable_service() {
        let tmp = tempdir().unwrap();
        setup_systemd_root(tmp.path());

        let ctx = OsModifierContext {
            root: tmp.path().to_path_buf(),
        };

        let services = Services {
            enable: vec!["test-osmodifier.service".to_string()],
            disable: vec![],
        };

        configure(&ctx, &services).unwrap();

        // Verify the symlink was created — may be dangling since target is absolute
        let wants_dir = tmp.path().join("etc/systemd/system/multi-user.target.wants");
        let service_link = wants_dir.join("test-osmodifier.service");
        assert!(
            service_link.is_symlink(),
            "Expected service symlink at {}",
            service_link.display(),
        );
    }

    #[functional_test(feature = "core")]
    fn test_disable_service() {
        let tmp = tempdir().unwrap();
        setup_systemd_root(tmp.path());

        let ctx = OsModifierContext {
            root: tmp.path().to_path_buf(),
        };

        // Enable first
        let enable = Services {
            enable: vec!["test-osmodifier.service".to_string()],
            disable: vec![],
        };
        configure(&ctx, &enable).unwrap();

        // Then disable
        let disable = Services {
            enable: vec![],
            disable: vec!["test-osmodifier.service".to_string()],
        };
        configure(&ctx, &disable).unwrap();

        // Verify the symlink was removed
        let symlink_path = tmp
            .path()
            .join("etc/systemd/system/multi-user.target.wants/test-osmodifier.service");
        assert!(
            !symlink_path.is_symlink(),
            "Symlink should be removed after disable"
        );
    }

    #[functional_test(feature = "core")]
    fn test_disable_already_disabled_service() {
        let tmp = tempdir().unwrap();
        setup_systemd_root(tmp.path());

        let ctx = OsModifierContext {
            root: tmp.path().to_path_buf(),
        };

        // Disable without enabling first — should succeed (warn and skip)
        let services = Services {
            enable: vec![],
            disable: vec!["test-osmodifier.service".to_string()],
        };

        let result = configure(&ctx, &services);
        assert!(
            result.is_ok(),
            "Disabling an already-disabled service should succeed"
        );
    }
}
