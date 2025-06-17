use const_format::formatcp;
use sysdefs::arch::SystemArchitecture;

///  Pattern used to name the EFI bootloader executable
pub enum BootloaderExecutable {
    Boot,
    /// Grub executable name pattern. Also used by systemd-boot for compatibility.
    Grub,
    GrubNoPrefix,
}

impl BootloaderExecutable {
    /// Returns the current name of the EFI bootloader executable
    pub const fn current_name(&self) -> &'static str {
        match self {
            BootloaderExecutable::Boot => formatcp!(
                "boot{}.efi",
                get_arch_efi_str(SystemArchitecture::current())
            ),
            BootloaderExecutable::Grub => formatcp!(
                "grub{}.efi",
                get_arch_efi_str(SystemArchitecture::current())
            ),
            BootloaderExecutable::GrubNoPrefix => formatcp!(
                "grub{}-noprefix.efi",
                get_arch_efi_str(SystemArchitecture::current())
            ),
        }
    }
}

/// Returns the architecture-specific suffix used in EFI executables.
/// This follows the convention used in Shim, which uses "x64" for AMD64 and "aa64" for AArch64.
/// Note: Azure Linux only supports 64-bit architectures (AMD64 and AArch64).
/// References:
/// - Shim Make.defaults: https://github.com/rhboot/shim/blob/d44405e85b8560c5f41a40abe1e9f230ca704cc1/Make.defaults#L63-L98
/// - GRUB2 spec file on Azure Linux: https://github.com/microsoft/azurelinux/blob/39cc18a4a03f914e3bfdfa54bd406c740a3abafe/SPECS/grub2/grub2.spec#L400-L414
/// - Shim spec file on Azure Linux: https://github.com/microsoft/azurelinux/blob/3.0/SPECS/shim/shim.spec#L21-L29
const fn get_arch_efi_str(arch: SystemArchitecture) -> &'static str {
    match arch {
        SystemArchitecture::Amd64 => "x64",
        SystemArchitecture::Aarch64 => "aa64",
    }
}

/// Bootloader executables
pub const BOOT_EFI: &str = BootloaderExecutable::Boot.current_name();
pub const GRUB_EFI: &str = BootloaderExecutable::Grub.current_name();
pub const GRUB_NOPREFIX_EFI: &str = BootloaderExecutable::GrubNoPrefix.current_name();

#[cfg(test)]
mod tests {
    use super::*;

    use strum::IntoEnumIterator;

    #[test]
    fn test_current_name() {
        let mut expected_arch = "";
        if cfg!(target_arch = "x86_64") {
            expected_arch = "x64";
        } else if cfg!(target_arch = "aarch64") {
            expected_arch = "aa64";
        };

        let expected_bootloader_executables = [
            (
                BootloaderExecutable::Boot,
                format!("boot{expected_arch}.efi"),
            ),
            (
                BootloaderExecutable::Grub,
                format!("grub{expected_arch}.efi"),
            ),
            (
                BootloaderExecutable::GrubNoPrefix,
                format!("grub{expected_arch}-noprefix.efi"),
            ),
        ];

        for (bootloader_executable, expected_filename) in expected_bootloader_executables {
            let filename = bootloader_executable.current_name();
            assert_eq!(
                filename, expected_filename,
                "Filename {filename} does not match expected value {expected_filename}"
            );
        }
    }

    #[test]
    fn test_get_arch_efi_str() {
        for arch in SystemArchitecture::iter() {
            let expected = match arch {
                SystemArchitecture::Amd64 => "x64",
                SystemArchitecture::Aarch64 => "aa64",
            };

            assert_eq!(
                get_arch_efi_str(arch),
                expected,
                "get_arch_efi_str({arch:?}) did not return expected value {expected}"
            );
        }
    }
}
