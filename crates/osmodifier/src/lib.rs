// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! OS modifier library — applies OS configuration changes to the filesystem.
//!
//! This crate replaces the external Go `osmodifier` binary with native Rust
//! implementations. All operations target paths under a configurable root
//! directory (defaulting to `/`).

pub mod config;
mod default_grub;
mod grub_cfg;
mod hostname;
mod modules;
mod selinux;
mod services;
mod users;

use std::path::{Path, PathBuf};

use anyhow::{Context, Error};
use log::{debug, info};

pub use config::*;

/// Execution context for OS modifier operations.
///
/// All filesystem paths are resolved relative to `root`. When trident has
/// chrooted into newroot, `root` should be `/`. When operating on an offline
/// image mounted at a specific path, set `root` accordingly.
pub struct OsModifierContext {
    /// Root directory for all filesystem operations.
    pub root: PathBuf,
}

impl Default for OsModifierContext {
    fn default() -> Self {
        Self {
            root: PathBuf::from("/"),
        }
    }
}

impl OsModifierContext {
    /// Resolve a path relative to the context root.
    pub fn path(&self, p: impl AsRef<Path>) -> PathBuf {
        if self.root == Path::new("/") {
            p.as_ref().to_path_buf()
        } else {
            let p = p.as_ref();
            let stripped = p.strip_prefix("/").unwrap_or(p);
            self.root.join(stripped)
        }
    }
}

/// Apply OS modifications: users, hostname, services, modules, kernel command
/// line, and SELinux.
///
/// This replaces the Go `osmodifier --config-file` codepath for
/// [`OSModifierConfig`].
pub fn modify_os(ctx: &OsModifierContext, config: &OSModifierConfig) -> Result<(), Error> {
    debug!("Applying OS modifications");

    if !config.users.is_empty() {
        info!("Configuring users");
        users::add_or_update_users(ctx, &config.users).context("Failed to configure users")?;
    }

    if let Some(ref name) = config.hostname {
        if !name.is_empty() {
            info!("Setting hostname to '{name}'");
            hostname::update(ctx, name).context("Failed to update hostname")?;
        }
    }

    if let Some(ref services) = config.services {
        if !services.enable.is_empty() || !services.disable.is_empty() {
            info!("Configuring services");
            services::configure(ctx, services).context("Failed to configure services")?;
        }
    }

    if !config.modules.is_empty() {
        info!("Configuring kernel modules");
        modules::configure(ctx, &config.modules).context("Failed to configure kernel modules")?;
    }

    // For UKI images, SELinux mode is set via the config file directly (not
    // via kernel cmdline). The osconfig subsystem handles this case by
    // including selinux in the OSModifierConfig.
    if let Some(ref selinux_cfg) = config.selinux {
        if let Some(ref mode) = selinux_cfg.mode {
            info!("Updating SELinux config file to mode '{mode:?}'");
            selinux::update_config_file(ctx, mode)
                .context("Failed to update SELinux config file")?;
        }
    }

    // Extra kernel command line args are appended to /etc/default/grub and
    // grub.cfg is regenerated. Note: modify_boot() also writes to
    // /etc/default/grub for boot-specific config (overlays, verity, etc.).
    if let Some(ref kcl) = config.kernel_command_line {
        if !kcl.extra_command_line.is_empty() {
            info!("Adding extra kernel command line arguments");
            default_grub::add_extra_cmdline(ctx, &kcl.extra_command_line)
                .context("Failed to add extra kernel command line")?;
            grub_cfg::run_grub_mkconfig(ctx).context("Failed to regenerate GRUB config")?;
        }
    }

    Ok(())
}

/// Sync current grub.cfg values back to /etc/default/grub and regenerate.
///
/// This replaces the Go `osmodifier --update-grub` codepath:
/// 1. Reads the generated grub.cfg
/// 2. Extracts overlayfs, verity, root, selinux, enforcing args
/// 3. Stamps those values into /etc/default/grub
/// 4. Runs grub2-mkconfig to regenerate
pub fn update_default_grub(ctx: &OsModifierContext) -> Result<(), Error> {
    info!("Syncing grub.cfg values to /etc/default/grub");

    let (args, root_device) = grub_cfg::extract_boot_args_from_grub_cfg(ctx)
        .context("Failed to extract boot args from grub.cfg")?;

    let mut default_grub = default_grub::DefaultGrub::read(ctx)?;

    default_grub.update_cmdline_args(
        &["rd.overlayfs", "roothash", "root", "selinux", "enforcing"],
        &args,
    )?;

    if let Some(root) = root_device {
        default_grub.set_variable("GRUB_DEVICE", &root);
    }

    default_grub.write()?;

    grub_cfg::run_grub_mkconfig(ctx)
        .context("Failed to regenerate GRUB config after updating defaults")?;

    info!("Successfully updated default grub");
    Ok(())
}

/// Apply boot-specific modifications: SELinux, overlays, verity, root device.
///
/// This replaces the Go `osmodifier --config-file` codepath for
/// [`BootConfig`]. Updates /etc/default/grub and regenerates via
/// grub2-mkconfig.
pub fn modify_boot(ctx: &OsModifierContext, config: &BootConfig) -> Result<(), Error> {
    info!("Applying boot configuration modifications");

    let mut default_grub = default_grub::DefaultGrub::read(ctx)?;
    let mut changed = false;

    if let Some(ref selinux_cfg) = config.selinux {
        if let Some(ref mode) = selinux_cfg.mode {
            debug!("Updating SELinux in boot config");
            selinux::update_grub_cmdline(ctx, &mut default_grub, mode)?;
            selinux::update_config_file(ctx, mode)
                .context("Failed to update SELinux config file")?;
            changed = true;
        }
    }

    if !config.overlays.is_empty() {
        debug!("Updating overlays in boot config");
        let mut overlay_configs = Vec::new();
        for overlay in &config.overlays {
            overlay_configs.push(format!(
                "{},{},{},{}",
                overlay.lower_dir, overlay.upper_dir, overlay.work_dir, overlay.partition.id,
            ));
        }
        let concatenated = overlay_configs.join(" ");
        default_grub
            .update_cmdline_args(&["rd.overlayfs"], &[format!("rd.overlayfs={concatenated}")])?;
        changed = true;
    }

    if let Some(ref verity) = config.verity {
        debug!("Updating verity in boot config");
        let corruption_opt = verity
            .corruption_option
            .as_ref()
            .map(format_corruption_option)
            .unwrap_or_default();

        let new_args = vec![
            "rd.systemd.verity=1".to_string(),
            format!("systemd.verity_root_data={}", verity.data_device),
            format!("systemd.verity_root_hash={}", verity.hash_device),
            format!("systemd.verity_root_options={corruption_opt}"),
        ];
        default_grub.update_cmdline_args(
            &[
                "rd.systemd.verity",
                "systemd.verity_root_data",
                "systemd.verity_root_hash",
                "systemd.verity_root_options",
            ],
            &new_args,
        )?;
        changed = true;
    }

    if let Some(ref root_device) = config.root_device {
        debug!("Setting root device to '{root_device}'");
        default_grub.set_variable("GRUB_DEVICE", root_device);
        changed = true;
    }

    if changed {
        default_grub.write()?;
        grub_cfg::run_grub_mkconfig(ctx)
            .context("Failed to regenerate GRUB config after boot modifications")?;
    }

    Ok(())
}

fn format_corruption_option(opt: &CorruptionOption) -> String {
    match opt {
        CorruptionOption::IoError => String::new(),
        CorruptionOption::Ignore => "ignore-corruption".to_string(),
        CorruptionOption::Panic => "panic-on-corruption".to_string(),
        CorruptionOption::Restart => "restart-on-corruption".to_string(),
    }
}

#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use tempfile::tempdir;

    use pytest_gen::functional_test;
    use trident_api::config::{LoadMode, Module, Services};

    #[functional_test(feature = "core")]
    fn test_modify_os_hostname_and_modules() {
        let tmp = tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("etc")).unwrap();

        let ctx = OsModifierContext {
            root: tmp.path().to_path_buf(),
        };

        let config = OSModifierConfig {
            hostname: Some("integration-test-host".to_string()),
            modules: vec![Module {
                name: "vfio_pci".to_string(),
                load_mode: LoadMode::Always,
                options: HashMap::new(),
            }],
            ..Default::default()
        };

        modify_os(&ctx, &config).unwrap();

        // Verify hostname
        let hostname = fs::read_to_string(tmp.path().join("etc/hostname")).unwrap();
        assert_eq!(hostname.trim(), "integration-test-host");

        // Verify module loaded
        let load_conf =
            fs::read_to_string(tmp.path().join("etc/modules-load.d/modules-load.conf")).unwrap();
        assert!(
            load_conf.contains("vfio_pci"),
            "Expected vfio_pci in modules-load.conf"
        );
    }

    #[functional_test(feature = "core")]
    fn test_modify_os_empty_config() {
        let tmp = tempdir().unwrap();
        let ctx = OsModifierContext {
            root: tmp.path().to_path_buf(),
        };

        let config = OSModifierConfig::default();

        // Empty config should be a no-op
        modify_os(&ctx, &config).unwrap();
    }

    #[functional_test(feature = "core")]
    fn test_modify_os_with_services() {
        let tmp = tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("etc")).unwrap();

        // Create a minimal systemd tree with a synthetic service
        let unit_dir = tmp.path().join("usr/lib/systemd/system");
        fs::create_dir_all(&unit_dir).unwrap();
        fs::create_dir_all(tmp.path().join("etc/systemd/system/multi-user.target.wants")).unwrap();
        fs::write(
            unit_dir.join("test-integration.service"),
            "[Unit]\nDescription=Test\n\n[Service]\nType=oneshot\nExecStart=/bin/true\n\n[Install]\nWantedBy=multi-user.target\n",
        )
        .unwrap();

        let ctx = OsModifierContext {
            root: tmp.path().to_path_buf(),
        };

        let config = OSModifierConfig {
            hostname: Some("svc-test-host".to_string()),
            services: Some(Services {
                enable: vec!["test-integration.service".to_string()],
                disable: vec![],
            }),
            ..Default::default()
        };

        modify_os(&ctx, &config).unwrap();

        // Verify hostname
        let hostname = fs::read_to_string(tmp.path().join("etc/hostname")).unwrap();
        assert_eq!(hostname.trim(), "svc-test-host");

        // Verify service enabled — symlink may be dangling (target is absolute /usr/...
        // but only exists under the temp root), so check is_symlink() not exists()
        let symlink = tmp
            .path()
            .join("etc/systemd/system/multi-user.target.wants/test-integration.service");
        assert!(
            symlink.is_symlink(),
            "Service should be enabled (symlink at {})",
            symlink.display()
        );
    }
}
