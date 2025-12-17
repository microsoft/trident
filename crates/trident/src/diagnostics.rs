use anyhow::{anyhow, Error};
use chrono::Utc;
use log::{debug, info};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::{self},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use lsblk::BlockDevice;
use osutils::{dependencies::Dependency, findmnt::FindMnt, lsblk, pcrlock, pcrlock::LogOutput};
use trident_api::{
    config::{Check, Health},
    error::{InternalError, ReportError, TridentError},
    status::HostStatus,
};

use crate::{
    datastore::DataStore, logging, TRIDENT_BACKGROUND_LOG_PATH, TRIDENT_METRICS_FILE_PATH,
    TRIDENT_VERSION,
};

const DIAGNOSTICS_BUNDLE_PREFIX: &str = "trident-diagnostics";
const DMI_SYS_VENDOR_FILE: &str = "/sys/class/dmi/id/sys_vendor";
const DMI_PRODUCT_NAME_FILE: &str = "/sys/class/dmi/id/product_name";

#[derive(Debug, Serialize, Deserialize)]
pub struct DiagnosticsReport {
    /// Timestamp when the report was generated
    pub timestamp: String,
    /// Trident version
    pub version: String,
    /// Host description (VM/baremetal)
    pub host_description: HostDescription,
    /// Host status from the datastore
    pub host_status: Option<HostStatus>,
    /// Collected files metadata
    pub collected_files: Option<Vec<FileMetadata>>,
    /// Collection failures that occurred during diagnostics gathering
    pub collection_failures: Vec<CollectionFailure>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HostDescription {
    /// Whether running in a container
    pub is_container: bool,
    /// Whether running on a VM
    pub is_virtual: bool,
    /// Virtualization type (kvm, vmware, hyperv, etc.)
    pub virt_type: Option<String>,
    /// Platform information
    pub platform_info: BTreeMap<String, Value>,
    /// Block device information
    pub blockdev_info: Option<Vec<BlockDevice>>,
    /// File system information (from FindMnt)
    pub mount_info: Option<FindMnt>,
    /// Status of systemd services from configured health checks
    pub health_check_status: Option<Vec<SystemdServiceStatus>>,
    /// TPM 2.0 pcrlock log output
    pub pcrlock_log: Option<LogOutput>,
    /// Trident service status and journal
    pub trident_service: TridentServiceDiagnostics,
}

/// Status information for a systemd service from a health check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemdServiceStatus {
    /// Name of the systemd service
    pub service: String,
    /// Whether the service is active/running
    pub is_active: bool,
    /// Output from systemctl status
    pub status_output: String,
}

/// Diagnostics for the trident.service systemd unit
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TridentServiceDiagnostics {
    /// Output from systemctl status trident.service
    pub status: Option<String>,
    /// Output from journalctl -u trident.service
    pub journal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadata {
    /// Relative path in the support bundle
    pub path: PathBuf,
    /// Size in bytes
    pub size_bytes: u64,
    /// Description of what this file contains
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionFailure {
    /// What was being collected when the failure occurred
    pub item: String,
    /// The error message describing what went wrong
    pub error: String,
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
        collected_files: None,
        collection_failures: failures,
    }
}

struct DatastorePaths {
    default: PathBuf,
    temporary: PathBuf,
    configured: Option<PathBuf>,
}

fn get_datastore_paths() -> DatastorePaths {
    use trident_api::constants::{AGENT_CONFIG_PATH, TRIDENT_DATASTORE_PATH_DEFAULT};

    let configured = std::fs::read_to_string(AGENT_CONFIG_PATH)
        .ok()
        .and_then(|contents| {
            contents.lines().find_map(|line| {
                line.strip_prefix("DatastorePath=")
                    .map(|p| PathBuf::from(p.trim()))
            })
        });

    DatastorePaths {
        default: PathBuf::from(TRIDENT_DATASTORE_PATH_DEFAULT),
        temporary: PathBuf::from(crate::TEMPORARY_DATASTORE_PATH),
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
    let status = collect_service_status("trident.service", failures).map(|s| s.status_output);

    let journal = Dependency::Journalctl
        .cmd()
        .args(["--no-pager", "-u", "trident.service"])
        .output()
        .map(|out| out.output_report())
        .map_err(|e| record_failure(failures, "trident.service journal", &e))
        .ok();

    TridentServiceDiagnostics { status, journal }
}

fn collect_full_journal(failures: &mut Vec<CollectionFailure>) -> Option<String> {
    Dependency::Journalctl
        .cmd()
        .args(["--no-pager"])
        .output()
        .map(|out| out.output_report())
        .map_err(|e| record_failure(failures, "full journal", &e))
        .ok()
}

fn collect_host_status(failures: &mut Vec<CollectionFailure>) -> Option<HostStatus> {
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

/// Package the diagnostics report and associated files into a compressed tarball
fn create_support_bundle(
    report: &mut DiagnosticsReport,
    output_path: &Path,
    files_to_collect: Vec<FileToCollect>,
    full_dump: bool,
) -> Result<(), Error> {
    let mut collected_files = Vec::new();
    let file = osutils::files::create_file(output_path)?;
    let encoder = zstd::Encoder::new(file, 0)?;
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
    if full_dump {
        if let Some(journal_content) = collect_full_journal(&mut report.collection_failures) {
            let mut header = tar::Header::new_gnu();
            header.set_size(journal_content.len() as u64);
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
                format!("{}/full-journal", DIAGNOSTICS_BUNDLE_PREFIX),
                journal_content.as_bytes(),
            )?;
            collected_files.push(FileMetadata {
                path: PathBuf::from("full-journal"),
                size_bytes: journal_content.len() as u64,
                description: "Full system journal from current boot".to_string(),
            });
        }
    }

    report.collected_files = Some(collected_files);
    let report_json = serde_json::to_string_pretty(report)?;

    let mut report_file_header = tar::Header::new_gnu();
    report_file_header.set_size(report_json.len() as u64);
    report_file_header.set_mode(0o644);
    report_file_header.set_mtime(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    );
    report_file_header.set_cksum();
    tar.append_data(
        &mut report_file_header,
        format!("{}/report.json", DIAGNOSTICS_BUNDLE_PREFIX),
        report_json.as_bytes(),
    )?;

    tar.into_inner()?.finish()?;

    Ok(())
}

pub(crate) fn generate_and_bundle(
    output_path: &Path,
    full_dump: bool,
    selinux: bool,
) -> Result<(), TridentError> {
    let mut report = collect_report();

    let mut files_to_collect = vec![
        FileToCollect {
            src: PathBuf::from(TRIDENT_BACKGROUND_LOG_PATH),
            tar_path: PathBuf::from("logs/trident-full.log"),
            desc: "Trident execution log".to_string(),
        },
        FileToCollect {
            src: PathBuf::from(TRIDENT_METRICS_FILE_PATH),
            tar_path: PathBuf::from("logs/trident-metrics.jsonl"),
            desc: "Trident metrics".to_string(),
        },
    ];

    // Collect historical metrics and logs from the datastore directory
    if let Some(log_dir) = report
        .host_status
        .as_ref()
        .and_then(|hs| hs.spec.trident.datastore_path.parent())
    {
        if let Ok(entries) = fs::read_dir(log_dir) {
            for entry in entries.flatten() {
                if let Ok(file_name) = entry.file_name().into_string() {
                    if file_name.starts_with("trident-")
                        && (file_name.ends_with(".log") || file_name.ends_with(".jsonl"))
                    {
                        let desc = if file_name.contains("metrics") {
                            "Historical Trident metrics from past servicing".to_string()
                        } else {
                            "Historical Trident log from past servicing".to_string()
                        };
                        files_to_collect.push(FileToCollect {
                            src: entry.path(),
                            tar_path: PathBuf::from("logs/historical").join(&file_name),
                            desc,
                        });
                    }
                }
            }
        }
    }

    // Collect datastores
    let paths = get_datastore_paths();
    files_to_collect.push(FileToCollect {
        src: paths.default,
        tar_path: PathBuf::from("datastore.sqlite"),
        desc: "Default datastore".to_string(),
    });
    files_to_collect.push(FileToCollect {
        src: paths.temporary,
        tar_path: PathBuf::from("datastore-tmp.sqlite"),
        desc: "Temporary datastore".to_string(),
    });
    if let Some(configured) = paths.configured {
        files_to_collect.push(FileToCollect {
            src: configured,
            tar_path: PathBuf::from("datastore-configured.sqlite"),
            desc: "Configured datastore".to_string(),
        });
    }

    files_to_collect.push(FileToCollect {
        src: PathBuf::from("/etc/fstab"),
        tar_path: PathBuf::from("files/fstab"),
        desc: "File system mount configuration (/etc/fstab)".to_string(),
    });

    files_to_collect.push(FileToCollect {
        src: PathBuf::from(pcrlock::PCRLOCK_POLICY_JSON_PATH),
        tar_path: PathBuf::from("tpm/pcrlock.json"),
        desc: "TPM 2.0 pcrlock policy (pcrlock.json)".to_string(),
    });

    if selinux {
        files_to_collect.push(FileToCollect {
            src: PathBuf::from("/var/log/audit/audit.log"),
            tar_path: PathBuf::from("selinux/audit.log"),
            desc: "SELinux audit log".to_string(),
        });
    }

    create_support_bundle(&mut report, output_path, files_to_collect, full_dump)
        .structured(InternalError::DiagnosticBundleGeneration)?;
    info!("Diagnostics bundle created: {}", output_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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
            FileToCollect {
                src: file1_path,
                tar_path: PathBuf::from("subdir/file1"),
                desc: "First file".to_string(),
            },
            FileToCollect {
                src: file2_path,
                tar_path: PathBuf::from("file2"),
                desc: "Second file".to_string(),
            },
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
            collected_files: None,
            collection_failures: Vec::new(),
        };

        // Create support bundle
        let bundle_path = output_dir.path().join("test-bundle.tar.zst");
        let result = create_support_bundle(&mut report, &bundle_path, files_to_collect, false);
        assert!(result.is_ok(), "Should create bundle successfully");

        // Verify bundle exists and is not empty
        assert!(bundle_path.exists(), "Bundle file should exist");
        assert!(
            std::fs::metadata(&bundle_path).unwrap().len() > 0,
            "Bundle should not be empty"
        );

        // Verify report has collected files metadata
        let collected = report.collected_files.as_ref().unwrap();
        assert_eq!(collected.len(), 2, "Should have 2 collected files");
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
        let collected = extracted_report.collected_files.as_ref().unwrap();
        assert_eq!(collected.len(), 2);
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
            collected_files: None,
            collection_failures: vec![CollectionFailure {
                item: "pcrlock log".to_string(),
                error: "pcrlock not configured".to_string(),
            }],
        };

        // Create bundle
        let temp_dir = tempdir().unwrap();
        let bundle_path = temp_dir.path().join("roundtrip.tar.zst");

        create_support_bundle(&mut report, &bundle_path, vec![], false).unwrap();

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
    use super::*;

    use pytest_gen::functional_test;

    use tempfile::tempdir;

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
            "full-journal should exist when --full is passed"
        );
        assert!(
            !std::fs::read_to_string(&journal_path).unwrap().is_empty(),
            "full-journal should have content"
        );
    }
}
