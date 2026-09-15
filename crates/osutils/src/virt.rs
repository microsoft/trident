//! Detects whether Trident is running inside a virtual machine, and if so,
//! which hypervisor, by reading DMI/SMBIOS sysfs files. Shared by
//! `trident::diagnostics` (the support-bundle `HostDescription`) and
//! `trident::logging::tracestream` (the `platform_info.vm`/
//! `platform_info.virt_type` telemetry fields), so the two never diverge on
//! what counts as "virtual". Also home to the unrelated `virtdeploy`
//! best-effort detection used by boot-order workarounds.
//!
//! `detect_hypervisor` only recognizes hypervisors that override the DMI
//! vendor/product strings to a known value (QEMU/KVM, Hyper-V). It does not
//! use the CPUID hypervisor-present bit, since a hardened or nested
//! hypervisor can mask both signals; this is a best-effort heuristic, not a
//! guarantee.

use anyhow::Context;

/// Path to DMI file with vendor information.
const DMI_SYS_VENDOR_FILE: &str = "/sys/class/dmi/id/sys_vendor";

/// Path to DMI file with product name.
const DMI_PRODUCT_NAME_FILE: &str = "/sys/class/dmi/id/product_name";

/// Does a best-effort check to determine whether we are running in a virtdeploy VM.
pub fn is_virtdeploy() -> bool {
    let mut index = 0;
    while let Ok(entry) =
        std::fs::read_to_string(format!("/sys/firmware/dmi/entries/11-{index}/raw"))
    {
        if entry.contains("virtdeploy:1") {
            return true;
        }
        index += 1;
    }

    false
}

/// Returns the detected hypervisor name (e.g. `"qemu"`, `"hyperv"`), or
/// `None` if the DMI vendor/product strings were read successfully but did
/// not match a known hypervisor (including physical hardware).
///
/// Returns `Err` whenever a required DMI file could not be read, so callers
/// that want to report collection failures (see
/// `trident::diagnostics::get_virtualization_info`) can distinguish
/// "unreadable" from "read fine, not virtualized". The error carries the
/// failing file's path via context, so it remains actionable after being
/// formatted/logged.
pub fn detect_hypervisor() -> Result<Option<String>, anyhow::Error> {
    let vendor = std::fs::read_to_string(DMI_SYS_VENDOR_FILE)
        .with_context(|| format!("Failed to read '{DMI_SYS_VENDOR_FILE}'"))?
        .trim()
        .to_lowercase();
    if vendor.contains("qemu") {
        return Ok(Some("qemu".to_string()));
    }

    let product = std::fs::read_to_string(DMI_PRODUCT_NAME_FILE)
        .with_context(|| format!("Failed to read '{DMI_PRODUCT_NAME_FILE}'"))?
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
