//! Detects whether Trident is running inside a virtual machine, and if so,
//! which hypervisor, by reading DMI/SMBIOS sysfs files. Shared by
//! `diagnostics` (the support-bundle `HostDescription`) and
//! `logging::tracestream` (the `platform_info.vm`/`platform_info.virt_type`
//! telemetry fields), so the two never diverge on what counts as "virtual".
//!
//! This only recognizes hypervisors that override the DMI vendor/product
//! strings to a known value (QEMU/KVM, Hyper-V). It does not use the CPUID
//! hypervisor-present bit, since a hardened or nested hypervisor can mask
//! both signals; this is a best-effort heuristic, not a guarantee.

use std::fs;

/// Path to DMI file with vendor information.
const DMI_SYS_VENDOR_FILE: &str = "/sys/class/dmi/id/sys_vendor";

/// Path to DMI file with product name.
const DMI_PRODUCT_NAME_FILE: &str = "/sys/class/dmi/id/product_name";

/// Returns the detected hypervisor name (e.g. `"qemu"`, `"hyperv"`), or
/// `None` if running on physical hardware, or if virtualization could not be
/// determined (DMI files unreadable, or an unrecognized vendor/product
/// combination).
///
/// Returns `Err` only when a DMI file exists but could not be read, so
/// callers that want to report collection failures (see
/// `diagnostics::get_virtualization_info`) can distinguish "unreadable" from
/// "read fine, not virtualized".
pub fn detect_hypervisor() -> Result<Option<String>, std::io::Error> {
    let vendor = fs::read_to_string(DMI_SYS_VENDOR_FILE)?
        .trim()
        .to_lowercase();
    if vendor.contains("qemu") {
        return Ok(Some("qemu".to_string()));
    }

    let product = fs::read_to_string(DMI_PRODUCT_NAME_FILE)?
        .trim()
        .to_lowercase();
    if vendor.contains("microsoft corporation") && product.contains("virtual machine") {
        return Ok(Some("hyperv".to_string()));
    }

    Ok(None)
}

/// Convenience wrapper over [`detect_hypervisor`] for callers (e.g.
/// telemetry) that only care whether this is a VM, not which hypervisor,
/// and don't need to distinguish "unreadable DMI files" from "not a VM" --
/// both cases are reported as `false`.
pub fn is_virtual() -> bool {
    detect_hypervisor().unwrap_or(None).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_virtual_matches_detect_hypervisor() {
        assert_eq!(is_virtual(), detect_hypervisor().unwrap_or(None).is_some());
    }
}
