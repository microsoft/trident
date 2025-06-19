use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{bail, Context, Error, Result};
use enumflags2::{make_bitflags, BitFlags};
use goblin::pe::PE;
use log::{debug, error, trace, warn};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha384, Sha512};
use tempfile::NamedTempFile;

use sysdefs::tpm2::Pcr;
use trident_api::primitives::hash::Sha256Hash;

use crate::{dependencies::Dependency, exe::RunAndCheck};

/// Path to the pcrlock directory where .pcrlock files are located.
///
/// `systemd-pcrlock` will search for .pcrlock files in a number of dir-s, but Trident will place
/// the files exclusively in this directory.
pub const PCRLOCK_DIR: &str = "/var/lib/pcrlock.d";

/// Path to the pcrlock policy JSON file.
pub const PCRLOCK_POLICY_PATH: &str = "/var/lib/systemd/pcrlock.json";

/// Sub-dirs inside PCRLOCK_DIR, i.e. `/var/lib/pcrlock.d`, for dynamically generated .pcrlock
/// files that might contain 1+ .pcrlock files, for the current and updated images:
/// 1. `/var/lib/pcrlock.d/600-gpt.pcrlock.d`, where `lock-gpt` measures the GPT partition table of
///    the booted medium, as recorded to PCR 5 by the firmware,
#[allow(dead_code)]
const GPT_PCRLOCK_DIR: &str = "600-gpt.pcrlock.d";

/// 2. `/var/lib/pcrlock.d/610-boot-loader-code.pcrlock.d`, where Trident measures the bootloader
///    PE binary, i.e., the shim EFI executable for UKI at path /EFI/BOOT/bootx64.efi, as recorded
///    into PCR 4 following Microsoft's Authenticode hash spec,
const BOOT_LOADER_CODE_PCRLOCK_DIR: &str = "610-boot-loader-code.pcrlock.d";

/// 3. `/var/lib/pcrlock.d/650-uki.pcrlock.d`, where `lock-uki` measures the UKI binary, as
///    recorded into PCR 4,
const UKI_PCRLOCK_DIR: &str = "650-uki.pcrlock.d";

/// 4. `/var/lib/pcrlock.d/710-kernel-cmdline.pcrlock.d`, where `lock-kernel-cmdline` measures the
///    kernel command line, as recorded into PCR 9,
#[allow(dead_code)]
const KERNEL_CMDLINE_PCRLOCK_DIR: &str = "710-kernel-cmdline.pcrlock.d";

/// 5. `/var/lib/pcrlock.d/720-kernel-initrd.pcrlock.d`, where Trident measures the initrd section of
///    the UKI binary, as recorded into PCR 9.
#[allow(dead_code)]
const KERNEL_INITRD_PCRLOCK_DIR: &str = "720-kernel-initrd.pcrlock.d";

/// Valid PCRs for TPM 2.0 policy generation, following the `systemd-pcrlock` spec.
///
/// https://www.man7.org/linux/man-pages/man8/systemd-pcrlock.8.html.
const VALID_PCRLOCK_PCRS: BitFlags<Pcr> = make_bitflags!(Pcr::{Pcr0 | Pcr1 | Pcr2 | Pcr3 | Pcr4 | Pcr5 | Pcr7 | Pcr11 | Pcr12 | Pcr13 | Pcr14 | Pcr15});

#[derive(Debug, Deserialize)]
struct PcrValue {
    pcr: Pcr,
}

#[derive(Debug, Deserialize)]
struct PcrPolicy {
    #[serde(rename = "pcrValues")]
    pcr_values: Vec<PcrValue>,
}

/// Validates the PCR input and calls a helper function `systemd-pcrlock make-policy` to generate a
/// TPM 2.0 access policy. Parses the output of the helper func to validate that the policy has
/// been updated as expected.
pub fn generate_tpm2_access_policy(pcrs: BitFlags<Pcr>) -> Result<(), Error> {
    debug!(
        "Generating a new TPM 2.0 access policy with the following PCRs: {:?}",
        pcrs.iter().map(|pcr| pcr.to_num()).collect::<Vec<_>>()
    );

    // Validate that all requested PCRs are allowed by systemd-pcrlock
    let filtered_pcrs = pcrs & VALID_PCRLOCK_PCRS;

    if pcrs != filtered_pcrs {
        let ignored = pcrs & !filtered_pcrs;
        warn!(
            "Ignoring unsupported PCRs while generating a new TPM 2.0 access policy: {:?}",
            ignored.iter().collect::<Vec<_>>()
        );
    }

    // Run systemd-pcrlock make-policy helper
    let output = make_policy(pcrs).context("Failed to generate a new TPM 2.0 access policy")?;
    trace!("Output of 'systemd-pcrlock make-policy':\n{}", output);

    // Validate that TPM 2.0 access policy has been updated
    if !output.contains("Calculated new pcrlock policy") || !output.contains("Updated NV index") {
        // Only warning b/c on clean install, pcrlock policy will be created for the first time
        warn!("TPM 2.0 access policy has not been updated:\n{}", output);
    }

    // Log pcrlock policy JSON contents
    let pcrlock_policy =
        fs::read_to_string(PCRLOCK_POLICY_PATH).context("Failed to read pcrlock policy JSON")?;
    trace!(
        "Contents of pcrlock policy JSON at '{PCRLOCK_POLICY_PATH}':\n{}",
        pcrlock_policy
    );

    // Parse the policy JSON to validate that all requested PCRs are present
    let policy: PcrPolicy =
        serde_json::from_str(&pcrlock_policy).context("Failed to parse pcrlock policy JSON")?;
    // Extract PCRs from the policy, and filter for PCRs that were requested yet are missing
    // from the policy
    let policy_pcrs: Vec<Pcr> = policy.pcr_values.iter().map(|pv| pv.pcr).collect();
    let missing_pcrs: Vec<Pcr> = pcrs
        .iter()
        .filter(|pcr| !policy_pcrs.contains(pcr))
        .collect();

    // If any requested PCRs are missing from the policy, return an error
    if !missing_pcrs.is_empty() {
        error!(
            "Some requested PCRs are missing from the generated pcrlock policy: '{:?}'",
            missing_pcrs
                .iter()
                .map(|pcr| pcr.to_num())
                .collect::<Vec<_>>()
        );
        return Err(anyhow::anyhow!(
            "Failed to generate a new TPM 2.0 access policy"
        ));
    }

    Ok(())
}

#[derive(Debug, Deserialize)]
struct LogEntry {
    pcr: Pcr,
    pcrname: Option<String>,
    event: Option<String>,
    sha256: Option<Sha256Hash>,
    component: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LogOutput {
    log: Vec<LogEntry>,
}

/// Runs `systemd-pcrlock log` to get the combined TPM 2.0 event log, output as a "pretty" JSON.
/// Parses the output and validates that every log entry related to a required PCR has been matched
/// to a recognized boot component. Currently, required PCRs are: 4, 7, and 11.
///
/// If a log entry has a null `component`, it means that there is no .pcrlock file that records
/// that specific measurement extended into the given PCR, for any boot process component. For that
/// reason, .pcrlock files are known as boot component definition files. If a log entry for a PCR
/// has its component missing, then the value of that PCR cannot be predicted and so the PCR cannot
/// be included in a pcrlock policy. Thus, this validation ensures that all .pcrlock files have
/// been added & generated, so that a valid TPM 2.0 access policy can be generated.
/// Please refer to `systemd-pcrlock` doc for additional info:
/// https://www.man7.org/linux/man-pages/man8/systemd-pcrlock.8.html.
fn validate_log() -> Result<(), Error> {
    debug!("Validating systemd-pcrlock log output");

    let output = Dependency::SystemdPcrlock
        .cmd()
        .arg("log")
        .arg("--json=pretty")
        .output_and_check()
        .context("Failed to run systemd-pcrlock log")?;

    let parsed: LogOutput =
        serde_json::from_str(&output).context("Failed to parse systemd-pcrlock log output")?;

    // Collect all entries that have a null component AND record measurements into PCRs 4, 7, or 11
    let unrecognized: Vec<_> = parsed
        .log
        .iter()
        .filter(|entry| {
            entry.component.is_none() && matches!(entry.pcr, Pcr::Pcr4 | Pcr::Pcr7 | Pcr::Pcr11)
        })
        .collect();

    if unrecognized.is_empty() {
        return Ok(());
    }

    let entries: Vec<String> = unrecognized
        .into_iter()
        .map(|entry| {
            format!(
                "pcr='{}', pcrname='{}', event='{}', sha256='{}', description='{}'",
                entry.pcr.to_num(),
                entry.pcrname.as_deref().unwrap_or("null"),
                entry.event.as_deref().unwrap_or("null"),
                entry.sha256.as_ref().map(|h| h.as_str()).unwrap_or("null"),
                entry.description.as_deref().unwrap_or("null"),
            )
        })
        .collect();

    bail!(
        "Failed to validate systemd-pcrlock log output as some log entries cannot be matched \
            to recognized components:\n{}",
        entries.join("\n")
    );
}

/// Runs `systemd-pcrlock make-policy` command to predict the PCR state for future boots and then
/// generate a TPM 2.0 access policy, stored in a TPM 2.0 NV index. The prediction and info about
/// the used TPM 2.0 and its NV index are written to PCRLOCK_POLICY_PATH.
fn make_policy(pcrs: BitFlags<Pcr>) -> Result<String, Error> {
    debug!(
        "Generating a new pcrlock policy via 'systemd-pcrlock make-policy' \
        with the following PCRs: {:?}",
        pcrs.iter().map(|pcr| pcr.to_num()).collect::<Vec<_>>()
    );

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
            .map(|flag| flag.to_num().to_string())
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
    ///
    /// Currently not used since Trident might possibly change the GPT disk partitioning.
    #[allow(dead_code)]
    Gpt {
        path: Option<PathBuf>,
        pcrlock_file: PathBuf,
    },

    /// Generates a .pcrlock file based on the specified PE binary. Useful for predicting
    /// measurements the firmware makes to PCR 4 ("boot-loader-code") if the specified
    /// binary is part of the UEFI boot process.
    ///
    /// Used for non-UKI images only; UKI binaries must be locked with `lock-uki`.
    #[allow(dead_code)]
    Pe {
        path: PathBuf,
        pcrlock_file: PathBuf,
    },

    /// Generates a .pcrlock file based on the specified UKI PE binary. Useful for predicting
    /// measurements the firmware makes to PCR 4 ("boot-loader-code"), and `systemd-stub` makes to
    /// PCR 11 ("kernel-boot"). Used for UKI images only; non-UKI binaries must be locked with
    /// `lock-pe`.
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
    #[allow(dead_code)]
    KernelCmdline {
        path: Option<PathBuf>,
        pcrlock_file: PathBuf,
    },

    /// Generates a .pcrlock file based on a kernel initrd cpio archive. Useful for predicting
    /// measurements the Linux kernel makes to PCR 9 ("kernel-initrd"). Should not be used for
    /// `systemd-stub` UKIs, as the initrd is combined dynamically from various sources and hence
    /// does not take a single input, like this command.
    #[allow(dead_code)]
    KernelInitrd {
        path: PathBuf,
        pcrlock_file: PathBuf,
    },

    /// Generates/removes a .pcrlock file based on raw binary data. The data is either read from
    /// the specified file or from STDIN. Requires that `--pcrs=` is specified. The generated
    /// .pcrlock file is written to the file specified via `--pcrlock=.
    #[allow(dead_code)]
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
        debug!("Running systemd-pcrlock {}", self.subcmd_name());
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

        cmd.run_and_check().context(format!(
            "Failed to run systemd-pcrlock {}",
            self.subcmd_name()
        ))
    }
}

/// Generates dynamically defined .pcrlock files for either (1) the current boot only or (2) the
/// current and the next boots. Calls the `systemd-pcrlock lock-*` commands to generate the
/// .pcrlock files, as well as helpers to generate the remaining .pcrlock files.
pub fn generate_pcrlock_files(
    // Vector containing paths of UKI binaries to measure via lock-uki,
    uki_binaries: Vec<PathBuf>,
    // Vector containing paths of bootloader binaries, i.e. shim EFI executables for UKI, to be
    // measured by Trident,
    bootloader_binaries: Vec<PathBuf>,
) -> Result<(), Error> {
    debug!("Generating .pcrlock files");

    let basic_cmds: Vec<LockCommand> = vec![
        LockCommand::FirmwareCode,
        LockCommand::FirmwareConfig,
        LockCommand::SecureBootPolicy,
        LockCommand::SecureBootAuthority,
        LockCommand::MachineId,
        LockCommand::FileSystem,
    ];

    for cmd in basic_cmds {
        cmd.run().context(format!(
            "Failed to generate .pcrlock file via '{}'",
            cmd.subcmd_name()
        ))?;
    }

    // lock-uki
    for (id, uki_path) in uki_binaries.clone().into_iter().enumerate() {
        let pcrlock_file = generate_pcrlock_output_path(UKI_PCRLOCK_DIR, id);
        let cmd = LockCommand::Uki {
            path: uki_path.clone(),
            pcrlock_file: pcrlock_file.clone(),
        };
        cmd.run().context(format!(
            "Failed to generate .pcrlock file via '{}' for UKI at path '{}'",
            cmd.subcmd_name(),
            uki_path.display()
        ))?;
    }

    for (id, bootloader_path) in bootloader_binaries.into_iter().enumerate() {
        let pcrlock_file = generate_pcrlock_output_path(BOOT_LOADER_CODE_PCRLOCK_DIR, id);
        debug!(
            "Manually generating .pcrlock file at path '{}' for bootloader at path '{}'",
            pcrlock_file.display(),
            bootloader_path.display()
        );
        generate_610_boot_loader_code_pcrlock(bootloader_path, pcrlock_file.clone()).context(
            format!(
                "Failed to manually generate .pcrlock file at path '{}'",
                pcrlock_file.display()
            ),
        )?;
    }

    // Parse the systemd-pcrlock log output to validate that every log entry has been matched to a
    // recognized boot component, and thus that all necessary .pcrlock files have been added or
    // generated
    validate_log().context(
        "Failed to validate pcrlock log to confirm all required .pcrlock files have been generated",
    )?;

    Ok(())
}

/// Represents a single digest entry in a .pcrlock file.
#[derive(Serialize)]
struct DigestEntry<'a> {
    hash_alg: &'a str,
    digest: String,
}

/// Represents a single record in a .pcrlock file.
#[derive(Serialize)]
struct Record<'a> {
    pcr: u8,
    digests: Vec<DigestEntry<'a>>,
}

/// Represents a .pcrlock file.
#[derive(Serialize)]
struct PcrLock<'a> {
    records: Vec<Record<'a>>,
}

/// Generates a full .pcrlock file path under PCRLOCK_DIR, i.e. /var/lib/pcrlock.d, given the
/// sub-dir, e.g. 600-gpt, and the index of the .pcrlock file. This is needed so that each image,
/// current and update, gets its own .pcrlock file.
fn generate_pcrlock_output_path(pcrlock_subdir: &str, index: usize) -> PathBuf {
    let base = Path::new(PCRLOCK_DIR).join(pcrlock_subdir);
    base.join(format!("generated-{index}.pcrlock"))
}

/// Generates .pcrlock files under /var/lib/pcrlock.d/610-boot-loader-code.pcrlock.d, where Trident
/// measures the bootloader PE binary, i.e., the shim EFI executable for UKI at path
/// /EFI/BOOT/bootx64.efi, as recorded into PCR 4 following Microsoft's Authenticode hash spec for
/// measuring Windows PE binaries:
/// https://reversea.me/index.php/authenticode-i-understanding-windows-authenticode/.
fn generate_610_boot_loader_code_pcrlock(
    bootloader_path: PathBuf,
    pcrlock_file: PathBuf,
) -> Result<()> {
    // Read the entire file into memory
    let buffer = fs::read(&bootloader_path)
        .with_context(|| format!("Failed to read PE binary at {}", bootloader_path.display()))?;

    // Parse PE
    let pe = PE::parse(&buffer)
        .with_context(|| format!("Failed to parse PE binary at {}", bootloader_path.display()))?;

    // Initialize hashers
    let mut sha256 = Sha256::new();
    let mut sha384 = Sha384::new();
    let mut sha512 = Sha512::new();

    for slice in pe.authenticode_ranges() {
        sha256.update(slice);
        sha384.update(slice);
        sha512.update(slice);
    }

    let digests = vec![
        DigestEntry {
            hash_alg: "sha256",
            digest: format!("{:x}", sha256.finalize()),
        },
        DigestEntry {
            hash_alg: "sha384",
            digest: format!("{:x}", sha384.finalize()),
        },
        DigestEntry {
            hash_alg: "sha512",
            digest: format!("{:x}", sha512.finalize()),
        },
    ];

    // Build PcrLock structure with PCR 4
    let pcrlock = PcrLock {
        records: vec![Record { pcr: 4, digests }],
    };

    if let Some(parent) = pcrlock_file.parent() {
        fs::create_dir_all(parent).context(format!(
            "Failed to create directory for .pcrlock file at {}",
            pcrlock_file.display()
        ))?;
    }

    let json = serde_json::to_string(&pcrlock).context(format!(
        "Failed to serialize .pcrlock file {} as JSON",
        pcrlock_file.display()
    ))?;
    fs::write(&pcrlock_file, json).context(format!(
        "Failed to write .pcrlock file at {}",
        pcrlock_file.display()
    ))?;

    Ok(())
}

/// Generates .pcrlock files under /var/lib/pcrlock.d/720-kernel-initrd.pcrlock.d, where Trident
/// measures the initrd section of the UKI binary, as recorded into PCR 9.
#[allow(dead_code)]
fn generate_720_kernel_initrd_pcrlock(uki_path: PathBuf, pcrlock_file: PathBuf) -> Result<()> {
    // Copy UKI to a temp file
    let uki_temp = NamedTempFile::new().context("Failed to create temporary UKI file")?;
    fs::copy(&uki_path, uki_temp.path())
        .with_context(|| format!("Failed to copy UKI from {}", uki_path.display()))?;

    // Extract .initrd
    let initrd_temp = NamedTempFile::new().context("Failed to create temporary initrd file")?;
    let initrd_path = initrd_temp.path().to_path_buf();
    Command::new("objcopy")
        .arg("--dump-section")
        .arg(format!(".initrd={}", initrd_path.display()))
        .arg(uki_temp.path())
        .run_and_check()
        .context(format!(
            "Failed to execute objcopy to extract initrd section from UKI at '{}'",
            uki_temp.path().display()
        ))?;

    // Read extracted initrd and compute hashes
    let buffer = fs::read(&initrd_path).with_context(|| {
        format!(
            "Failed to read extracted initrd at {}",
            initrd_path.display()
        )
    })?;

    let digests = vec![
        DigestEntry {
            hash_alg: "sha256",
            digest: hex::encode(Sha256::digest(&buffer)),
        },
        DigestEntry {
            hash_alg: "sha384",
            digest: hex::encode(Sha384::digest(&buffer)),
        },
        DigestEntry {
            hash_alg: "sha512",
            digest: hex::encode(Sha512::digest(&buffer)),
        },
    ];

    // Write .pcrlock file
    if let Some(parent) = pcrlock_file.parent() {
        fs::create_dir_all(parent).context(format!(
            "Failed to create directory for .pcrlock file at {}",
            pcrlock_file.display()
        ))?;
    }

    let pcrlock = PcrLock {
        records: vec![Record { pcr: 9, digests }],
    };

    let json = serde_json::to_string(&pcrlock).context(format!(
        "Failed to serialize .pcrlock file {} as JSON",
        pcrlock_file.display()
    ))?;
    fs::write(&pcrlock_file, json).context(format!(
        "Failed to write .pcrlock file at {}",
        pcrlock_file.display()
    ))?;

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

    #[test]
    fn test_generate_pcrlock_output_path() {
        let index = 0;
        let expected_path = Path::new(PCRLOCK_DIR)
            .join(GPT_PCRLOCK_DIR)
            .join(format!("generated-{index}.pcrlock"));
        assert_eq!(
            generate_pcrlock_output_path(GPT_PCRLOCK_DIR, index),
            expected_path
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;

    #[functional_test(feature = "helpers")]
    fn test_generate_tpm2_access_policy() {
        // Test case #0. Since no .pcrlock files have been generated yet, only 0-valued PCRs can be
        // used to generate a TPM 2.0 access policy.
        let zero_pcrs = Pcr::Pcr11 | Pcr::Pcr12 | Pcr::Pcr13;
        generate_tpm2_access_policy(zero_pcrs).unwrap();

        // Test case #1. Try to generate a TPM 2.0 access policy with all PCRs; should return an
        // error since no .pcrlock files have been generated yet.
        let pcrs = BitFlags::<Pcr>::all();
        assert_eq!(
            generate_tpm2_access_policy(pcrs)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to generate a new TPM 2.0 access policy"
        );

        // TODO: Add other/more test cases once helpers are implemented and statically defined
        // .pcrlock files have been added.
    }

    #[functional_test(feature = "helpers")]
    fn test_validate_log() {
        // TODO: This test will fail for now since .pcrlock files have not been generated/added
        // yet. Once static .pcrlock files are added and dynamic files are generated, the test
        // should pass.
        validate_log().unwrap_err();
    }
}
