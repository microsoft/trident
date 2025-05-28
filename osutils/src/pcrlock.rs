use std::{fs, path::PathBuf};

use anyhow::{Context, Error, Result};
use enumflags2::{make_bitflags, BitFlags};
use log::{debug, error, trace, warn};
use serde::Deserialize;

use trident_api::error::{ReportError, ServicingError, TridentError};

use sysdefs::tpm2::Pcr;

use crate::dependencies::Dependency;

/// Path to the pcrlock directory where .pcrlock files are stored.
#[allow(dead_code)]
const PCRLOCK_DIR: &str = "/var/lib/pcrlock.d";

/// Path to the PCR policy JSON file.
const PCR_POLICY_PATH: &str = "/var/lib/systemd/pcrlock.json";

/// Dir-s for dynamically generated .pcrlock files that might contain 1+ .pcrlock files, for the
/// current and updated images:
/// 1. /var/lib/pcrlock.d/600-gpt.pcrlock.d, where `lock-gpt` measures the GPT partition table of
///     the booted medium, as recorded to PCR 5 by the firmware,
#[allow(dead_code)]
const GPT_PCRLOCK_DIR: &str = "600-gpt.pcrlock.d";

/// 2. /var/lib/pcrlock.d/610-boot-loader-code.pcrlock.d, where Trident measures the bootx64.efi
///     binary, as recorded into PCR 4 following Microsoft's Authenticode hash spec,
#[allow(dead_code)]
const BOOT_LOADER_CODE_PCRLOCK_DIR: &str = "610-boot-loader-code.pcrlock.d";

/// 3. /var/lib/pcrlock.d/630-boot-loader-conf.pcrlock.d, where `lock-raw` measures the boot loader
///     configuration file, as recorded into PCR 5,
#[allow(dead_code)]
const BOOT_LOADER_CONF_PCRLOCK_DIR: &str = "630-boot-loader-conf.pcrlock.d";

/// 4. /var/lib/pcrlock.d/650-uki.pcrlock.d, where `lock-uki` measures the UKI binary, as recorded
///    into PCR 4,
#[allow(dead_code)]
const UKI_PCRLOCK_DIR: &str = "650-uki.pcrlock.d";

/// 5. /var/lib/pcrlock.d/710-kernel-cmdline.pcrlock.d, where `lock-kernel-cmdline` measures the
///    kernel command line, as recorded into PCR 9,
#[allow(dead_code)]
const KERNEL_CMDLINE_PCRLOCK_DIR: &str = "710-kernel-cmdline.pcrlock.d";

/// 6. /var/lib/pcrlock.d/720-kernel-initrd.pcrlock.d, where Trident measures the initrd section of
///     the UKI binary, as recorded into PCR 9.
#[allow(dead_code)]
const KERNEL_INITRD_PCRLOCK_DIR: &str = "720-kernel-initrd.pcrlock.d";

/// Valid PCRs for TPM2 policy generation, following the `systemd-pcrlock` spec.
///
/// https://www.man7.org/linux/man-pages/man8/systemd-pcrlock.8.html.
const ALLOWED_PCRS: BitFlags<Pcr> = make_bitflags!(Pcr::{Pcr0 | Pcr1 | Pcr2 | Pcr3 | Pcr4 | Pcr5 | Pcr7 | Pcr11 | Pcr12 | Pcr13 | Pcr14 | Pcr15});

#[derive(Debug, Deserialize)]
struct PcrValue {
    pcr: Pcr,
}

#[derive(Debug, Deserialize)]
struct PcrPolicy {
    #[serde(rename = "pcrValues")]
    pcr_values: Vec<PcrValue>,
}

/// Validates the PCR input and calls a helper function to generate the TPM 2.0 access policy.
/// Parses the output of the helper function to validate that the policy has been updated as
/// expected.
///
/// If PCRs are not specified, the command defaults to PCRs 0-5, 7, 11-15.
pub fn generate_tpm2_access_policy(pcrs: BitFlags<Pcr>) -> Result<(), TridentError> {
    debug!(
        "Generating a new TPM 2.0 access policy for the following PCRs: {:?}",
        pcrs.iter().map(|pcr| pcr.to_value()).collect::<Vec<_>>()
    );

    // Validate that all requested PCRs are allowed by systemd-pcrlock
    let filtered_pcrs = pcrs & ALLOWED_PCRS;

    if pcrs != filtered_pcrs {
        let ignored = pcrs & !filtered_pcrs;
        warn!(
            "Ignoring unsupported PCRs while generating a new TPM 2.0 access policy: {:?}",
            ignored.iter().collect::<Vec<_>>()
        );
    }

    let output = make_policy(pcrs).structured(ServicingError::GenerateTpm2AccessPolicy)?;

    // Validate that TPM 2.0 access policy has been updated
    if !output.contains("Calculated new PCR policy") || !output.contains("Updated NV index") {
        warn!("TPM 2.0 access policy has not been updated:\n{}", output);
    }

    // Log PCR policy JSON contents
    let pcrlock_policy =
        fs::read_to_string(PCR_POLICY_PATH).structured(ServicingError::GenerateTpm2AccessPolicy)?;
    trace!(
        "Contents of PCR policy JSON at '{PCR_POLICY_PATH}':\n{}",
        pcrlock_policy
    );

    // Parse the policy JSON to validate that all requested PCRs are present
    let policy: PcrPolicy = serde_json::from_str(&pcrlock_policy)
        .structured(ServicingError::GenerateTpm2AccessPolicy)?;
    // Extract PCRs from the policy, and filter for PCRs that were requested yet are missing
    // from the policy
    let policy_pcrs: Vec<Pcr> = policy.pcr_values.iter().map(|pv| pv.pcr).collect();
    let missing_pcrs: Vec<Pcr> = pcrs
        .iter()
        .filter(|pcr| !policy_pcrs.contains(pcr))
        .collect();

    if !missing_pcrs.is_empty() {
        error!(
            "Some requested PCRs are missing from the generated PCR policy: '{:?}'",
            missing_pcrs
                .iter()
                .map(|pcr| pcr.to_value())
                .collect::<Vec<_>>()
        );
        return Err(TridentError::new(ServicingError::GenerateTpm2AccessPolicy));
    }

    Ok(())
}

/// Runs `systemd-pcrlock log` command to view the combined TPM 2.0 event log matched against the
/// current PCR values, output in a tabular format.
#[allow(dead_code)]
fn log() -> Result<String, Error> {
    Dependency::SystemdPcrlock
        .cmd()
        .arg("log")
        .output_and_check()
        .context("Failed to run systemd-pcrlock log")
}

/// Runs `systemd-pcrlock make-policy` command to predict the PCR state for future boots and then
/// generate a TPM 2.0 access policy, stored in a TPM 2.0 NV index. The prediction and info about
/// the used TPM 2.0 and its NV index are written to PCR_POLICY_PATH.
fn make_policy(pcrs: BitFlags<Pcr>) -> Result<String, Error> {
    Dependency::SystemdPcrlock
        .cmd()
        .arg("make-policy")
        .arg(to_pcr_arg(pcrs))
        .output_and_check()
        .context("Failed to run systemd-pcrlock make-policy")
}

/// Converts the provided PCR bitflags into the `--pcr=` argument for `systemd-pcrlock`. Returns a
/// string with the PCR indices separated by `,`.
fn to_pcr_arg(pcrs: BitFlags<Pcr>) -> String {
    format!(
        "--pcr={}",
        pcrs.iter()
            .map(|flag| flag.to_value().to_string())
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// Represents the `systemd-pcrlock lock-*` commands. Each command generates or removes specific
/// .pcrlock files based on the TPM 2.0 event log of the current/next boot covering all records for
/// a specific set of PCRs.
///
/// For more info, see the official documentation for the `systemd-pcrlock` tool:
/// https://www.man7.org/linux/man-pages/man8/systemd-pcrlock.8.html.
enum LockCommand {
    /// Generates .pcrlock files covering all records for PCRs 0 ("platform-code") and 2
    /// ("external-code"). Allows locking the boot process to the current version of the firmware
    /// of the system and its extension cards.
    FirmwareCode,

    /// Locks down the firmware configuration, i.e. PCRs 1 ("platform-config") and 3
    /// ("external-config").
    FirmwareConfig,

    /// Generates a .pcrlock file based on the SecureBoot policy currently enforced. Looks at
    /// SecureBoot, PK, KEK, db, dbx, dbt, dbr EFI variables and predicts their measurements to PCR
    /// 7 ("secure-boot-policy") on the next boot.
    SecureBootPolicy,

    /// Generates a .pcrlock file based on the SecureBoot authorities used to validate the boot
    /// path. Uses relevant measurements on PCR 7 ("secure-boot-policy").
    SecureBootAuthority,

    /// Generates a .pcrlock file based on the GPT partition table of the specified disk. If no
    /// disk is specified automatically determines the block device backing the root file system.
    /// Locks the state of the disk partitioning, which firmware measures to PCR 5
    /// ("boot-loader-config").
    Gpt {
        path: Option<PathBuf>,
        pcrlock_file: PathBuf,
    },

    /// Generates a .pcrlock file based on the specified PE binary. Useful for predicting
    /// measurements the firmware makes to PCR 4 ("boot-loader-code") if the specified
    /// binary is part of the UEFI boot process.
    ///
    /// Used for non-UKI images only; UKI binaries must be locked with lock-uki.
    #[allow(dead_code)]
    Pe {
        path: PathBuf,
        pcrlock_file: PathBuf,
    },

    /// Generates a .pcrlock file based on the specified UKI PE binary. Useful for predicting
    /// measurements the firmware makes to PCR 4 ("boot-loader-code"), and systemd-stub makes to
    /// PCR 11 ("kernel-boot"). Used for UKI images only; non-UKI binaries must be locked with
    /// lock-pe.
    Uki {
        path: PathBuf,
        pcrlock_file: PathBuf,
    },

    /// Generates a .pcrlock file based on /etc/machine-id. Useful for predicting measurements
    /// systemd-pcrmachine.service makes to PCR 15 ("system-identity").
    MachineId,

    /// Generates a .pcrlock file based on file system identity. Useful for predicting measurements
    /// systemd-pcrfs@.service makes to PCR 15 ("system-identity") for the root and var
    /// filesystems.
    FileSystem,

    /// Generates a .pcrlock file based on /proc/cmdline (or the specified file if given). Useful
    /// for predicting measurements the Linux kernel makes to PCR 9 ("kernel-initrd").
    KernelCmdline {
        path: Option<PathBuf>,
        pcrlock_file: PathBuf,
    },

    /// Generates a .pcrlock file based on a kernel initrd cpio archive. Useful for predicting
    /// measurements the Linux kernel makes to PCR 9 ("kernel-initrd"). Should not be used for
    /// systemd-stub UKIs, as the initrd is combined dynamically from various sources and hence
    /// does not take a single input, like this command.
    #[allow(dead_code)]
    KernelInitrd {
        path: PathBuf,
        pcrlock_file: PathBuf,
    },

    /// Generates/removes a .pcrlock file based on raw binary data. The data is either read from
    /// the specified file or from STDIN. Requires that --pcrs= is specified. The generated
    /// .pcrlock file is written to the file specified via --pcrlock=.
    Raw {
        path: PathBuf,
        pcrs: BitFlags<Pcr>,
        pcrlock_file: PathBuf,
    },
}

impl LockCommand {
    /// Returns the name of the subcommand for the `systemd-pcrlock` tool.
    fn subcmd_name(&self) -> &'static str {
        match self {
            Self::FirmwareCode => "lock-firmware-code",
            Self::FirmwareConfig => "lock-firmware-config",
            Self::SecureBootPolicy => "lock-secureboot-policy",
            Self::SecureBootAuthority => "lock-secureboot-authority",
            Self::MachineId => "lock-machine-id",
            Self::FileSystem => "lock-file-system",
            Self::Gpt { .. } => "lock-gpt",
            Self::Pe { .. } => "lock-pe",
            Self::Uki { .. } => "lock-uki",
            Self::KernelCmdline { .. } => "lock-kernel-cmdline",
            Self::KernelInitrd { .. } => "lock-kernel-initrd",
            Self::Raw { .. } => "lock-raw",
        }
    }

    /// Runs a `systemd-pcrlock` command.
    ///
    /// Primarily designed for running the `lock-*` commands.
    fn run(&self) -> Result<(), Error> {
        let (path, pcrlock_file, pcrs) = {
            let mut cmd_path: Option<PathBuf> = None;
            let mut cmd_pcrlock_file: Option<PathBuf> = None;
            let mut cmd_pcrs: Option<BitFlags<Pcr>> = None;

            match self {
                Self::FirmwareCode
                | Self::FirmwareConfig
                | Self::SecureBootPolicy
                | Self::SecureBootAuthority
                | Self::MachineId
                | Self::FileSystem => (),

                Self::Gpt { path, pcrlock_file } | Self::KernelCmdline { path, pcrlock_file } => {
                    cmd_path = path.clone();
                    cmd_pcrlock_file = Some(pcrlock_file.clone());
                }

                Self::Pe { path, pcrlock_file }
                | Self::Uki { path, pcrlock_file }
                | Self::KernelInitrd { path, pcrlock_file } => {
                    cmd_path = Some(path.clone());
                    cmd_pcrlock_file = Some(pcrlock_file.clone());
                }

                Self::Raw {
                    path,
                    pcrs: raw_pcrs,
                    pcrlock_file,
                } => {
                    cmd_path = Some(path.clone());
                    cmd_pcrlock_file = Some(pcrlock_file.clone());
                    cmd_pcrs = Some(*raw_pcrs);
                }
            }

            (cmd_path, cmd_pcrlock_file, cmd_pcrs)
        };

        let mut cmd = Dependency::SystemdPcrlock.cmd();
        cmd.arg(self.subcmd_name());

        if let Some(path) = path {
            cmd.arg(path);
        }

        if let Some(pcrs) = pcrs {
            cmd.arg(to_pcr_arg(pcrs));
        }

        if let Some(pcrlock_file) = pcrlock_file {
            cmd.arg(format!("--pcrlock={}", pcrlock_file.display()));
        }

        cmd.run_and_check()
            .with_context(|| format!("Failed to run systemd-pcrlock {}", self.subcmd_name()))
    }
}

/// Generates dynamically defined .pcrlock files for either (1) the current boot only or (2) the
/// current and the next boots. Calls the `systemd-pcrlock lock-*` commands to generate the
/// .pcrlock files, as well as native logic to generate the remaining .pcrlock files.
pub fn generate_pcrlock_files(
    // lock-gpt -> path of partitioned disk, pcrlock_file to write to
    gpt_disks: Vec<(Option<PathBuf>, PathBuf)>,
    // lock-pe -> path of PE binary, pcrlock_file to write to
    _pe_binaries: Vec<(PathBuf, PathBuf)>,
    // lock-uki -> path of UKI PE binary, pcrlock_file to write to
    uki_binaries: Vec<(PathBuf, PathBuf)>,
    // lock-kernel-cmdline -> path of kernel cmdline, pcrlock_file to write to
    kernel_cmdlines: Vec<(Option<PathBuf>, PathBuf)>,
    // lock-kernel-initrd -> path, pcrlock_file to write to
    _kernel_initrds: Vec<(PathBuf, PathBuf)>,
    // lock-raw -> path, pcrs, pcrlock_file to write to
    raw_binaries: Vec<(PathBuf, BitFlags<Pcr>, PathBuf)>,
) -> Result<()> {
    let basic_cmds: Vec<LockCommand> = vec![
        LockCommand::FirmwareCode,
        LockCommand::FirmwareConfig,
        LockCommand::SecureBootPolicy,
        LockCommand::SecureBootAuthority,
        LockCommand::MachineId,
        LockCommand::FileSystem,
    ];

    for cmd in basic_cmds {
        cmd.run()?;
    }

    for (path, pcrlock_file) in gpt_disks {
        LockCommand::Gpt { path, pcrlock_file }.run()?;
    }

    for (path, pcrlock_file) in uki_binaries {
        LockCommand::Uki { path, pcrlock_file }.run()?;
    }

    for (path, pcrlock_file) in kernel_cmdlines {
        LockCommand::KernelCmdline { path, pcrlock_file }.run()?;
    }

    // For now, needed to generate 630-boot-loader-conf.pcrlock.d, which measures the raw binary of
    // /boot/efi/loader/loader.conf into PCR 5.
    for (path, pcrs, pcrlock_file) in raw_binaries {
        LockCommand::Raw {
            path,
            pcrs,
            pcrlock_file,
        }
        .run()?;
    }

    // TODO: Run helpers to generate remaining .pcrlock files, which cannot be generated via the
    // lock-* commands.

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use enumflags2::make_bitflags;

    #[test]
    fn test_to_pcr_arg() {
        let pcrs = make_bitflags!(Pcr::{Pcr1 | Pcr4});
        assert_eq!(to_pcr_arg(pcrs), "--pcr=1,4".to_string());

        let single_pcr = make_bitflags!(Pcr::{Pcr7});
        assert_eq!(to_pcr_arg(single_pcr), "--pcr=7".to_string());

        let all_pcrs = BitFlags::<Pcr>::all();
        assert_eq!(
            to_pcr_arg(all_pcrs),
            "--pcr=0,1,2,3,4,5,7,9,10,11,12,13,14,15,16,23".to_string()
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;

    use trident_api::error::ErrorKind;

    #[functional_test(feature = "helpers")]
    fn test_generate_tpm2_access_policy() {
        // Test case #0. Since no pcrlock files have been generated yet, only 0-valued PCRs can be
        // used to generate a TPM 2.0 access policy.
        let zero_pcrs = make_bitflags!(Pcr::{Pcr11 | Pcr12 | Pcr13});
        generate_tpm2_access_policy(zero_pcrs).unwrap();

        // Test case #1. Try to generate a TPM 2.0 access policy with all PCRs; should return an
        // error since no pcrlock files have been generated yet.
        let pcrs = BitFlags::<Pcr>::all();
        assert_eq!(
            generate_tpm2_access_policy(pcrs).unwrap_err().kind(),
            &ErrorKind::Servicing(ServicingError::GenerateTpm2AccessPolicy)
        );

        // TODO: Add other/more test cases once helpers are implemented and statically defined
        // pcrlock files have been added.
    }
}
