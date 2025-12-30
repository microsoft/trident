use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Error};
use chrono::Utc;
use log::{debug, info};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use lsblk::BlockDevice;
use osutils::{
    dependencies::Dependency,
    files,
    findmnt::FindMnt,
    lsblk,
    pcrlock::{self, LogOutput},
};
use trident_api::{
    config::{Check, Health},
    constants::{AGENT_CONFIG_PATH, TRIDENT_DATASTORE_PATH_DEFAULT},
    error::{InternalError, ReportError, TridentError},
    status::HostStatus,
};

use crate::{
    datastore::DataStore, logging, subsystems::storage::DEFAULT_FSTAB_PATH,
    TEMPORARY_DATASTORE_PATH, TRIDENT_BACKGROUND_LOG_PATH, TRIDENT_METRICS_FILE_PATH,
    TRIDENT_VERSION,
};

/// Name of the top-level directory in the diagnostics tarball
const DIAGNOSTICS_BUNDLE_PREFIX: &str = "trident-diagnostics";

/// Path to DMI file with vendor information
const DMI_SYS_VENDOR_FILE: &str = "/sys/class/dmi/id/sys_vendor";

/// Path to DMI file with product name
const DMI_PRODUCT_NAME_FILE: &str = "/sys/class/dmi/id/product_name";

/// Name of the trident systemd service
const TRIDENT_SERVICE_NAME: &str = "trident.service";

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticsReport {
    /// Timestamp when the report was generated
    timestamp: String,
    /// Trident version
    version: String,
    /// Information about Trident's host system
    host_description: HostDescription,
    /// Host status from the datastore
    host_status: Option<HostStatus>,
    /// Metadata for each file included in the tarball
    collected_files: Vec<FileMetadata>,
    /// Failures that occurred during the collection of diagnostics
    collection_failures: Vec<CollectionFailure>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostDescription {
    /// Whether running in a container
    is_container: bool,
    /// Whether running on a VM
    is_virtual: bool,
    /// Virtualization type (kvm, vmware, hyperv, etc.)
    virt_type: Option<String>,
    /// System info, e.g. Kernel version, OS release, total_memory...
    platform_info: BTreeMap<String, Value>,
    /// Block device information
    blockdev_info: Option<Vec<BlockDevice>>,
    /// File system information (from FindMnt)
    mount_info: Option<FindMnt>,
    /// Status of systemd services from configured health checks
    health_check_status: Option<Vec<SystemdServiceStatus>>,
    /// TPM 2.0 pcrlock log output
    pcrlock_log: Option<LogOutput>,
    /// Trident service status and journal
    trident_service: TridentServiceDiagnostics,
}

/// Status information for a systemd service from a health check
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SystemdServiceStatus {
    /// Name of the systemd service
    service: String,
    /// Whether the service is active/running
    is_active: bool,
    /// Output from systemctl status
    status_output: String,
}

/// Diagnostics for the trident.service systemd unit
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TridentServiceDiagnostics {
    /// Output from systemctl status trident.service
    status: Option<String>,
    /// Output from journalctl -u trident.service
    journal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileMetadata {
    /// Relative path in the support bundle
    path: PathBuf,
    /// Size in bytes
    size_bytes: u64,
    /// Description of what this file contains
    description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CollectionFailure {
    /// What was being collected when the failure occurred
    item: String,
    /// The error message describing what went wrong
    error: String,
}

fn record_failure(
    failures: &mut Vec<CollectionFailure>,
    item: impl Into<String>,
    error: &impl std::fmt::Debug,
) {
    failures.push(CollectionFailure {
        item: item.into(),
        error: format!("{:?}", error),
    });
}

/// Collect diagnostics information about the host system
fn collect_report() -> DiagnosticsReport {
    info!("Collecting diagnostics information");

    let mut failures = Vec::new();
    let host_status = collect_host_status(&mut failures);
    let host_description = collect_host_description(host_status.as_ref(), &mut failures);

    DiagnosticsReport {
        timestamp: Utc::now().to_rfc3339(),
        version: TRIDENT_VERSION.to_string(),
        host_description,
        host_status,
        collected_files: Vec::new(),
        collection_failures: failures,
    }
}

struct DatastorePaths {
    default: PathBuf,
    temporary: PathBuf,
    configured: Option<PathBuf>,
}

fn get_datastore_paths() -> DatastorePaths {
    let configured = fs::read_to_string(AGENT_CONFIG_PATH)
        .ok()
        .and_then(|contents| {
            contents.lines().find_map(|line| {
                line.strip_prefix("DatastorePath=")
                    .map(|p| PathBuf::from(p.trim()))
            })
        });

    DatastorePaths {
        default: PathBuf::from(TRIDENT_DATASTORE_PATH_DEFAULT),
        temporary: PathBuf::from(TEMPORARY_DATASTORE_PATH),
        configured,
    }
}

fn collect_host_description(
    host_status: Option<&HostStatus>,
    failures: &mut Vec<CollectionFailure>,
) -> HostDescription {
    let is_container = osutils::container::is_running_in_container().unwrap_or_else(|e| {
        record_failure(failures, "container environment detection", &e);
        false
    });
    let platform_info = logging::tracestream::PLATFORM_INFO.clone();
    let virt_type = get_virtualization_info(failures);
    let is_virtual = virt_type.is_some();
    let blockdev_info = lsblk::list()
        .map_err(|e| record_failure(failures, "block device info", &e))
        .ok();
    let mount_info = FindMnt::run()
        .map_err(|e| record_failure(failures, "mount info", &e))
        .ok();
    let health_check_status =
        host_status.map(|hs| collect_health_check_status(&hs.spec.health, failures));
    let pcrlock_log = collect_pcrlock_log(failures);
    let trident_service = collect_trident_service_diagnostics(failures);

    HostDescription {
        is_container,
        is_virtual,
        virt_type,
        platform_info,
        blockdev_info,
        mount_info,
        health_check_status,
        pcrlock_log,
        trident_service,
    }
}

fn collect_pcrlock_log(failures: &mut Vec<CollectionFailure>) -> Option<LogOutput> {
    debug!("Collecting pcrlock log");
    match pcrlock::log_parsed() {
        Ok(log) => Some(log),
        Err(e) => {
            record_failure(failures, "pcrlock log", &e);
            None
        }
    }
}

fn collect_trident_service_diagnostics(
    failures: &mut Vec<CollectionFailure>,
) -> TridentServiceDiagnostics {
    debug!("Collecting trident service diagnostics");
    let status = collect_service_status(TRIDENT_SERVICE_NAME, failures).map(|s| s.status_output);

    let journal = Dependency::Journalctl
        .cmd()
        .args(["--no-pager", "-u", TRIDENT_SERVICE_NAME])
        .output_and_check()
        .map_err(|e| record_failure(failures, format!("{} journal", TRIDENT_SERVICE_NAME), &e))
        .ok();

    TridentServiceDiagnostics { status, journal }
}

fn collect_full_journal(failures: &mut Vec<CollectionFailure>) -> Option<String> {
    debug!("Collecting full journal");
    Dependency::Journalctl
        .cmd()
        .args(["--no-pager"])
        .output_and_check()
        .map_err(|e| record_failure(failures, "full journal", &e))
        .ok()
}

fn collect_host_status(failures: &mut Vec<CollectionFailure>) -> Option<HostStatus> {
    debug!("Collecting host status from datastore");
    let paths = get_datastore_paths();

    let candidates = [
        paths.configured.as_ref(),
        Some(&paths.default),
        Some(&paths.temporary),
    ];

    for path in candidates.into_iter().flatten() {
        if path.exists() {
            if let Ok(datastore) = DataStore::open(path) {
                return Some(datastore.host_status().clone());
            }
        }
    }

    record_failure(failures, "host status", &anyhow!("no datastore found"));
    None
}

fn collect_health_check_status(
    health: &Health,
    failures: &mut Vec<CollectionFailure>,
) -> Vec<SystemdServiceStatus> {
    debug!("Collecting systemd health check status");
    // Get all health check systemd service names
    let services: Vec<_> = health
        .checks
        .iter()
        .filter_map(|check| match check {
            Check::SystemdCheck(sc) => Some(sc.systemd_services.iter()),
            Check::Script(_) => None,
        })
        .flatten()
        .collect();

    if services.is_empty() {
        debug!("No systemd health checks configured");
    }

    let statuses: Vec<_> = services
        .iter()
        .filter_map(|service| collect_service_status(service, failures))
        .collect();

    statuses
}

fn collect_service_status(
    service: &str,
    failures: &mut Vec<CollectionFailure>,
) -> Option<SystemdServiceStatus> {
    let output = Dependency::Systemctl
        .cmd()
        .env("SYSTEMD_IGNORE_CHROOT", "true")
        .arg("status")
        .arg(service)
        .output();

    match output {
        Ok(out) => {
            let is_active = out.success();
            let status_output = out.output_report();
            Some(SystemdServiceStatus {
                service: service.to_string(),
                is_active,
                status_output,
            })
        }
        Err(e) => {
            record_failure(failures, format!("service status for {}", service), &e);
            None
        }
    }
}

fn get_virtualization_info(failures: &mut Vec<CollectionFailure>) -> Option<String> {
    debug!("Collecting virtualization info");
    let content = match fs::read_to_string(DMI_SYS_VENDOR_FILE) {
        Ok(c) => c,
        Err(e) => {
            record_failure(failures, "virtualization info (dmi_sys vendor)", &e);
            return None;
        }
    };

    let vendor = content.trim().to_lowercase();
    if vendor.contains("qemu") {
        return Some("qemu".to_string());
    }

    let product = match fs::read_to_string(DMI_PRODUCT_NAME_FILE) {
        Ok(p) => p,
        Err(e) => {
            record_failure(failures, "virtualization info (dmi_sys product)", &e);
            return None;
        }
    };

    let product = product.trim().to_lowercase();
    if vendor.contains("microsoft corporation") && product.contains("virtual machine") {
        return Some("hyperv".to_string());
    }

    None
}

struct FileToCollect {
    src: PathBuf,
    tar_path: PathBuf,
    desc: String,
}

impl FileToCollect {
    fn new(src: impl Into<PathBuf>, tar_path: impl Into<PathBuf>, desc: impl Into<String>) -> Self {
        Self {
            src: src.into(),
            tar_path: tar_path.into(),
            desc: desc.into(),
        }
    }
}

fn collect_historical_logs(report: &mut DiagnosticsReport) -> Option<Vec<FileToCollect>> {
    debug!("Collecting historical logs");
    let Some(host_status) = report.host_status.as_ref() else {
        return None; // If no host status we've already recorded the failure
    };

    let Some(log_dir) = host_status.spec.trident.datastore_path.parent() else {
        record_failure(
            &mut report.collection_failures,
            "historical logs",
            &anyhow!(
                "unexpected datastore path when collecting historical logs {})",
                host_status.spec.trident.datastore_path.display()
            ),
        );
        return None;
    };

    let entries = match fs::read_dir(log_dir) {
        Ok(entries) => entries,
        Err(e) => {
            record_failure(
                &mut report.collection_failures,
                format!("historical logs directory {}", log_dir.display()),
                &e,
            );
            return None;
        }
    };

    entries
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .into_string()
                .ok()
                .map(|name| (entry.path(), name))
        })
        .filter(|(_, name)| {
            name.starts_with("trident-") && (name.ends_with(".log") || name.ends_with(".jsonl"))
        })
        .map(|(path, name)| {
            let desc = parse_historical_log_description(&name);
            FileToCollect::new(path, PathBuf::from("logs/historical").join(&name), desc)
        })
        .collect::<Vec<_>>()
        .into()
}

/// Parse historical log filename to extract servicing type and timestamp for description.
/// Examples:
///   trident-CleanInstallFinalized-20251230T183618Z.log
///   trident-metrics-CleanInstallFinalized-20251230T183618Z.jsonl
fn parse_historical_log_description(name: &str) -> String {
    let (base, prefix) = if name.contains("metrics") {
        ("Historical Trident metrics", "trident-metrics-")
    } else {
        ("Historical Trident log", "trident-")
    };

    name.strip_prefix(prefix)
        .and_then(|s| s.strip_suffix(".log").or_else(|| s.strip_suffix(".jsonl")))
        .and_then(|s| s.rsplit_once('-'))
        .map(|(svc, ts)| format!("{} from {} at {}", base, svc, ts))
        .unwrap_or_else(|| format!("{} from past servicing", base))
}

/// Package the diagnostics report and associated files into a compressed tarball.
fn create_diagnostics_tarball(
    report: &mut DiagnosticsReport,
    output_path: &Path,
    files_to_collect: Vec<FileToCollect>,
    collect_journal: bool,
) -> Result<(), Error> {
    debug!("Creating diagnostics tarball at {}", output_path.display());
    let mut collected_files = Vec::new();
    let file =
        files::create_file(output_path).context("failed to create diagnostics bundle file")?;
    let encoder =
        zstd::Encoder::new(file, 0).context("failed to create zstd encoder for tarball")?;
    let mut tar = tar::Builder::new(encoder);

    for file_to_collect in files_to_collect {
        if let Ok(meta) = fs::metadata(&file_to_collect.src) {
            if let Err(e) = tar.append_path_with_name(
                &file_to_collect.src,
                format!(
                    "{}/{}",
                    DIAGNOSTICS_BUNDLE_PREFIX,
                    file_to_collect.tar_path.display()
                ),
            ) {
                record_failure(
                    &mut report.collection_failures,
                    format!("file {}", file_to_collect.tar_path.display()),
                    &e,
                );
                continue; // Skip this file
            }
            collected_files.push(FileMetadata {
                path: file_to_collect.tar_path,
                size_bytes: meta.len(),
                description: file_to_collect.desc,
            });
        } else {
            record_failure(
                &mut report.collection_failures,
                format!("file {}", file_to_collect.src.display()),
                &anyhow!("file does not exist"),
            );
        }
    }

    // Collect and write full journal directly to tarball if requested
    if collect_journal {
        if let Some(journal_content) = collect_full_journal(&mut report.collection_failures) {
            write_to_tar(&mut tar, "full-journal", journal_content.as_bytes())
                .context("failed to write journal to tarball")?;
            collected_files.push(FileMetadata {
                path: PathBuf::from("full-journal"),
                size_bytes: journal_content.len() as u64,
                description: "Full system journal from current boot".to_string(),
            });
        }
    }

    report.collected_files = collected_files;
    let report_json =
        serde_json::to_string_pretty(report).context("failed to serialize diagnostics report")?;
    write_to_tar(&mut tar, "report.json", report_json.as_bytes())
        .context("failed to write report.json to tarball")?;

    tar.into_inner()
        .context("failed to finalize tar archive")?
        .finish()
        .context("failed to finish zstd compression")?;

    Ok(())
}

/// Append in-memory data to the tarball with proper header setup.
fn write_to_tar<W: std::io::Write>(
    tar: &mut tar::Builder<W>,
    path: &str,
    data: &[u8],
) -> Result<(), Error> {
    let mut header = tar::Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    );
    header.set_cksum();
    tar.append_data(
        &mut header,
        format!("{}/{}", DIAGNOSTICS_BUNDLE_PREFIX, path),
        data,
    )?;
    Ok(())
}

/// Generate a diagnostics bundle (tarball) at the specified output path.
///
/// `collect_journal` is true, include the full system journal; `collect_selinux` is true, include the SELinux audit log.
pub(crate) fn generate_and_bundle(
    output_path: &Path,
    collect_journal: bool,
    collect_selinux: bool,
) -> Result<(), TridentError> {
    debug!(
        "Generating diagnostics bundle (journal={}, selinux={})",
        collect_journal, collect_selinux
    );
    let mut report = collect_report();

    let mut files = vec![
        FileToCollect::new(
            TRIDENT_BACKGROUND_LOG_PATH,
            "logs/trident-full.log",
            "Execution log",
        ),
        FileToCollect::new(
            TRIDENT_METRICS_FILE_PATH,
            "logs/trident-metrics.jsonl",
            "Metrics",
        ),
    ];

    // Collect historical metrics and logs from the datastore directory
    files.extend(collect_historical_logs(&mut report).unwrap_or_default());

    // Collect datastores
    let paths = get_datastore_paths();
    files.push(FileToCollect::new(
        paths.default,
        "datastore.sqlite",
        "Default datastore",
    ));
    files.push(FileToCollect::new(
        paths.temporary,
        "datastore-tmp.sqlite",
        "Temporary datastore",
    ));
    if let Some(configured) = paths.configured {
        files.push(FileToCollect::new(
            configured,
            "datastore-configured.sqlite",
            "Configured datastore",
        ));
    }

    files.push(FileToCollect::new(
        DEFAULT_FSTAB_PATH,
        "files/fstab",
        "File system mount configuration (/etc/fstab)",
    ));

    files.push(FileToCollect::new(
        AGENT_CONFIG_PATH,
        "files/config.yaml",
        "Trident agent configuration",
    ));

    files.push(FileToCollect::new(
        pcrlock::PCRLOCK_POLICY_JSON_PATH,
        "tpm/pcrlock.json",
        "TPM 2.0 pcrlock policy (pcrlock.json)",
    ));

    if collect_selinux {
        files.push(FileToCollect::new(
            "/var/log/audit/audit.log",
            "selinux/audit.log",
            "SELinux audit log",
        ));
    }

    create_diagnostics_tarball(&mut report, output_path, files, collect_journal).structured(
        InternalError::GenerateDiagnosticsBundle(output_path.display().to_string()),
    )?;
    info!("Diagnostics bundle created: {}", output_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_create_support_bundle() {
        // Create temporary directories for test files and output
        let test_dir = tempdir().unwrap();
        let output_dir = tempdir().unwrap();

        // Create a few test files to collect
        let file1_path = test_dir.path().join("file1");
        let file2_path = test_dir.path().join("file2");

        std::fs::write(&file1_path, "content1").unwrap();
        std::fs::write(&file2_path, "content2").unwrap();

        // Create files to collect
        let files_to_collect = vec![
            FileToCollect::new(file1_path, "subdir/file1", "First file"),
            FileToCollect::new(file2_path, "file2", "Second file"),
        ];

        // Create initial report
        let mut report = DiagnosticsReport {
            timestamp: Utc::now().to_rfc3339(),
            version: "test-version".to_string(),
            host_description: HostDescription {
                is_container: false,
                is_virtual: false,
                virt_type: Some("none".to_string()),
                platform_info: BTreeMap::new(),
                blockdev_info: None,
                mount_info: None,
                health_check_status: None,
                pcrlock_log: None,
                trident_service: TridentServiceDiagnostics {
                    status: None,
                    journal: None,
                },
            },
            host_status: None,
            collected_files: Vec::new(),
            collection_failures: Vec::new(),
        };

        // Create support bundle
        let bundle_path = output_dir.path().join("test-bundle.tar.zst");
        let result = create_diagnostics_tarball(&mut report, &bundle_path, files_to_collect, false);
        assert!(result.is_ok(), "Should create bundle successfully");

        // Verify bundle exists and is not empty
        assert!(bundle_path.exists(), "Bundle file should exist");
        assert!(
            std::fs::metadata(&bundle_path).unwrap().len() > 0,
            "Bundle should not be empty"
        );

        // Verify report has collected files metadata
        assert_eq!(
            report.collected_files.len(),
            2,
            "Should have 2 collected files"
        );
        let collected = &report.collected_files;
        assert_eq!(collected[0].path, PathBuf::from("subdir/file1"));
        assert_eq!(collected[1].path, PathBuf::from("file2"));

        // Extract and verify bundle contents
        let extract_dir = output_dir.path().join("extracted");
        std::fs::create_dir(&extract_dir).unwrap();

        let file = std::fs::File::open(&bundle_path).unwrap();
        let decoder = zstd::Decoder::new(file).unwrap();
        let mut archive = tar::Archive::new(decoder);
        archive.unpack(&extract_dir).unwrap();

        // Verify extracted files exist and have correct content
        let bundle_dir = extract_dir.join(DIAGNOSTICS_BUNDLE_PREFIX);

        let files_to_verify = [("subdir/file1", "content1"), ("file2", "content2")];

        for (path, expected_content) in files_to_verify {
            let file_path = bundle_dir.join(path);
            assert!(file_path.exists(), "File {} should exist", path);

            let content = std::fs::read_to_string(&file_path).unwrap();
            assert_eq!(content, expected_content, "File {} content mismatch", path);
        }

        assert!(
            bundle_dir.join("report.json").exists(),
            "report.json should exist"
        );

        // Verify report.json content
        let report_json_path = extract_dir
            .join(DIAGNOSTICS_BUNDLE_PREFIX)
            .join("report.json");
        let report_content = std::fs::read_to_string(&report_json_path).unwrap();
        let extracted_report: DiagnosticsReport = serde_json::from_str(&report_content).unwrap();
        assert_eq!(extracted_report.collected_files.len(), 2);
        let collected = &extracted_report.collected_files;
        assert_eq!(collected[0].path, PathBuf::from("subdir/file1"));
        assert_eq!(collected[0].description, "First file");
        assert_eq!(collected[1].path, PathBuf::from("file2"));
        assert_eq!(collected[1].description, "Second file");
    }

    #[test]
    fn test_bundle_report_json() {
        // Create a complete report with fields populated, including collection failures
        let mut report = DiagnosticsReport {
            timestamp: "2025-01-15T12:00:00Z".to_string(),
            version: "1.2.3".to_string(),
            host_description: HostDescription {
                is_container: true,
                is_virtual: true,
                virt_type: Some("kvm".to_string()),
                platform_info: {
                    let mut map = BTreeMap::new();
                    map.insert("cpu".to_string(), serde_json::json!("x86_64"));
                    map.insert("memory".to_string(), serde_json::json!(8192));
                    map
                },
                blockdev_info: Some(vec![]),
                mount_info: None,
                health_check_status: None,
                pcrlock_log: None,
                trident_service: TridentServiceDiagnostics {
                    status: None,
                    journal: None,
                },
            },
            host_status: None,
            collected_files: Vec::new(),
            collection_failures: vec![CollectionFailure {
                item: "pcrlock log".to_string(),
                error: "pcrlock not configured".to_string(),
            }],
        };

        // Create bundle
        let temp_dir = tempdir().unwrap();
        let bundle_path = temp_dir.path().join("roundtrip.tar.zst");

        create_diagnostics_tarball(&mut report, &bundle_path, vec![], false).unwrap();

        // Extract and read back report.json
        let extract_dir = temp_dir.path().join("extracted");
        std::fs::create_dir(&extract_dir).unwrap();

        let file = std::fs::File::open(&bundle_path).unwrap();
        let decoder = zstd::Decoder::new(file).unwrap();
        let mut archive = tar::Archive::new(decoder);
        archive.unpack(&extract_dir).unwrap();

        let report_json = std::fs::read_to_string(
            extract_dir
                .join(DIAGNOSTICS_BUNDLE_PREFIX)
                .join("report.json"),
        )
        .unwrap();

        let read_report: DiagnosticsReport = serde_json::from_str(&report_json).unwrap();

        // Verify all fields match expected values
        assert_eq!(read_report.timestamp, "2025-01-15T12:00:00Z");
        assert_eq!(read_report.version, "1.2.3");
        assert!(read_report.host_description.is_container);
        assert!(read_report.host_description.is_virtual);
        assert_eq!(
            read_report.host_description.virt_type,
            Some("kvm".to_string())
        );
        assert_eq!(read_report.host_description.platform_info.len(), 2);
        assert_eq!(
            read_report
                .host_description
                .platform_info
                .get("cpu")
                .unwrap(),
            &serde_json::json!("x86_64")
        );
        assert_eq!(
            read_report
                .host_description
                .platform_info
                .get("memory")
                .unwrap(),
            &serde_json::json!(8192)
        );
        assert!(read_report.host_description.mount_info.is_none());

        // Verify collection_failures survived roundtrip
        assert_eq!(read_report.collection_failures.len(), 1);
        assert_eq!(read_report.collection_failures[0].item, "pcrlock log");
    }

    #[test]
    fn test_record_failure() {
        let mut failures = Vec::new();

        let io_error = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
        record_failure(&mut failures, "test file", &io_error);

        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].item, "test file");
        assert!(failures[0].error.contains("PermissionDenied"));
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use tempfile::tempdir;

    use pytest_gen::functional_test;

    use super::*;

    #[functional_test]
    fn test_generate_and_bundle() {
        // Test depends on functional-tests running in a QEMU VM with disk sda and sdb
        let temp_dir = tempdir().unwrap();
        let bundle_path = temp_dir.path().join("test-diagnostics.tar.zst");

        let result = generate_and_bundle(&bundle_path, false, false);
        assert!(
            result.is_ok(),
            "Should generate bundle successfully: {:?}",
            result
        );

        // Verify file exists and has content
        assert!(bundle_path.exists(), "Bundle file should exist");
        let metadata = std::fs::metadata(&bundle_path).unwrap();
        assert!(metadata.len() > 0, "Bundle should not be empty");

        // Extract and verify contents
        let extract_dir = temp_dir.path().join("extracted");
        std::fs::create_dir(&extract_dir).unwrap();

        let file = std::fs::File::open(&bundle_path).unwrap();
        let decoder = zstd::Decoder::new(file).unwrap();
        let mut archive = tar::Archive::new(decoder);
        archive.unpack(&extract_dir).unwrap();

        // Verify report.json exists and is valid
        let report_path = extract_dir
            .join(DIAGNOSTICS_BUNDLE_PREFIX)
            .join("report.json");
        assert!(report_path.exists(), "report.json should exist in bundle");

        let report_content = std::fs::read_to_string(&report_path).unwrap();
        let report: DiagnosticsReport =
            serde_json::from_str(&report_content).expect("report.json should be valid JSON");

        // Verify report contents
        assert!(!report.timestamp.is_empty(), "Timestamp should be set");
        assert_eq!(report.version, TRIDENT_VERSION, "Version should match");

        // Verify host description
        assert!(
            !report.host_description.is_container,
            "Should not be running in a container"
        );
        assert!(
            report.host_description.is_virtual,
            "Should detect as virtual machine"
        );
        assert_eq!(
            report.host_description.virt_type,
            Some("qemu".to_string()),
            "Should detect QEMU virtualization"
        );
        assert!(
            !&report.host_description.platform_info.is_empty(),
            "Platform info should contain system details"
        );

        // Verify disk info is populated
        assert!(
            report.host_description.blockdev_info.is_some(),
            "Disk info should be present"
        );

        let blockdev_info = report.host_description.blockdev_info.as_ref().unwrap();
        assert!(
            blockdev_info.iter().any(|d| d.name == "sda"),
            "Should have sda in disk info"
        );
        assert!(
            blockdev_info.iter().any(|d| d.name == "sdb"),
            "Should have sdb in disk info"
        );

        // Test with full_dump=true to verify journal collection
        let bundle_path_full = temp_dir.path().join("test-diagnostics-full.tar.zst");
        generate_and_bundle(&bundle_path_full, true, false).expect("Should generate full bundle");

        let extract_dir_full = temp_dir.path().join("extracted-full");
        std::fs::create_dir(&extract_dir_full).unwrap();
        let file = std::fs::File::open(&bundle_path_full).unwrap();
        let decoder = zstd::Decoder::new(file).unwrap();
        let mut archive = tar::Archive::new(decoder);
        archive.unpack(&extract_dir_full).unwrap();

        let journal_path = extract_dir_full
            .join(DIAGNOSTICS_BUNDLE_PREFIX)
            .join("full-journal");
        assert!(
            journal_path.exists(),
            "full-journal should exist when --journal is passed"
        );
        assert!(
            !std::fs::read_to_string(&journal_path).unwrap().is_empty(),
            "full-journal should have content"
        );
    }
}
