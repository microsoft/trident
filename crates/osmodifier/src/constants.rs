// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Centralized constants for the osmodifier crate.
//!
//! GRUB variable names and kernel command-line arg names are defined here
//! once to avoid magic-literal duplication across modules.

/// GRUB variable name for the primary kernel command line.
pub(crate) const GRUB_VAR_CMDLINE_LINUX: &str = "GRUB_CMDLINE_LINUX";

/// GRUB variable name for the default (non-recovery) kernel command line.
pub(crate) const GRUB_VAR_CMDLINE_LINUX_DEFAULT: &str = "GRUB_CMDLINE_LINUX_DEFAULT";

/// GRUB variable name for the boot device.
pub(crate) const GRUB_VAR_DEVICE: &str = "GRUB_DEVICE";

/// Kernel args to extract from grub.cfg and sync back to /etc/default/grub.
///
/// Used by both `grub_cfg::extract_boot_args_from_grub_cfg` (to pick which
/// args to capture) and `update_default_grub` (to specify which existing
/// args to replace).
pub(crate) const SYNC_ARG_NAMES: &[&str] = &[
    "rd.overlayfs",
    "roothash",
    "root",
    "security",
    "selinux",
    "enforcing",
];

/// Kernel command-line arg names managed by SELinux configuration.
///
/// Used when updating or replacing SELinux-related boot args in
/// GRUB_CMDLINE_LINUX.
pub(crate) const SELINUX_CMDLINE_ARG_NAMES: &[&str] = &["security", "selinux", "enforcing"];
