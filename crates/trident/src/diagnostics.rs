use std::{
    fs::{self},
    path::{Path, PathBuf},
};

use anyhow::Error;
use log::{debug, info, warn};
use osutils::lsblk;
use serde::{Deserialize, Serialize};
use trident_api::{
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

/// The diagnostics report contains all collected information
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostDescription {
    /// Whether running in a container
    pub is_container: bool,
    /// Whether running on a VM
    pub is_virtual: bool,
    /// Virtualization type (kvm, vmware, hyperv, etc.)
    pub virt_type: String,
    /// Platform information
    pub platform_info: std::collections::BTreeMap<String, serde_json::Value>,
    /// Disk information
    pub disk_info: Option<Vec<lsblk::BlockDevice>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadata {
    /// Relative path in the support bundle
    pub path: String,
    /// Size in bytes
    pub size_bytes: u64,
    /// Description of what this file contains
    pub description: String,
}

fn collect_report() -> Result<DiagnosticsReport, TridentError> {
    info!("Collecting diagnostics information");

    let timestamp = chrono::Utc::now().to_rfc3339();
    let version = TRIDENT_VERSION.to_string();

    let host_description = collect_host_description();
    let host_status = collect_host_status();

    Ok(DiagnosticsReport {
        timestamp,
        version,
        host_description,
        host_status,
        collected_files: None,
    })
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

fn collect_host_description() -> HostDescription {
    let is_container = osutils::container::is_running_in_container().unwrap_or_else(|e| {
        warn!("Container environment detection failed: {:?}", e);
        false
    });
    let platform_info = logging::tracestream::PLATFORM_INFO.clone();
    let (is_virtual, virt_type) = get_virtualization_info();
    let disk_info = lsblk::list().ok();

    HostDescription {
        is_container,
        is_virtual,
        virt_type,
        platform_info,
        disk_info,
    }
}

fn collect_host_status() -> Option<HostStatus> {
    let paths = get_datastore_paths();

    let candidates = [
        paths.configured.as_ref(),
        Some(&paths.default),
        Some(&paths.temporary),
    ];

    for path in candidates.into_iter().flatten() {
        if path.exists() {
            match DataStore::open(path) {
                Ok(datastore) => {
                    return Some(datastore.host_status().clone());
                }
                Err(e) => {
                    warn!("Failed to open datastore at {}: {:?}", path.display(), e);
                }
            }
        }
    }
    debug!("No valid datastore found to collect host status");
    None
}

fn get_virtualization_info() -> (bool, String) {
    if let Ok(content) = fs::read_to_string(DMI_SYS_VENDOR_FILE) {
        let vendor = content.trim().to_lowercase();
        if vendor.contains("qemu") {
            return (true, "qemu".to_string());
        }
        if let Ok(product) = fs::read_to_string(DMI_PRODUCT_NAME_FILE) {
            let product = product.trim().to_lowercase();
            if vendor.contains("microsoft corporation") && product.contains("virtual machine") {
                return (true, "hyperv".to_string());
            }
        }
    }

    (false, "none detected".to_string())
}

struct FileToCollect {
    src: PathBuf,
    tar_path: String,
    desc: String,
}

/// Package the diagnostics report and associated files into a compressed tarball
fn create_support_bundle(
    report: &mut DiagnosticsReport,
    output_path: &Path,
    files_to_collect: Vec<FileToCollect>,
) -> Result<PathBuf, Error> {
    let mut collected_files = Vec::new();
    let file = osutils::files::create_file(output_path)?;
    let encoder = zstd::Encoder::new(file, 0)?;
    let mut tar = tar::Builder::new(encoder);

    for file_to_collect in files_to_collect {
        if let Ok(meta) = fs::metadata(&file_to_collect.src) {
            if let Err(e) = tar.append_path_with_name(
                &file_to_collect.src,
                format!("{}/{}", DIAGNOSTICS_BUNDLE_PREFIX, file_to_collect.tar_path),
            ) {
                warn!(
                    "Failed to add {} to diagnostics bundle: {}",
                    file_to_collect.tar_path, e
                );
                continue; // Skip this file
            }
            collected_files.push(FileMetadata {
                path: file_to_collect.tar_path,
                size_bytes: meta.len(),
                description: file_to_collect.desc,
            });
        } else {
            debug!(
                "File {} does not exist, skipping",
                file_to_collect.src.display()
            );
        }
    }

    report.collected_files = Some(collected_files);
    let report_json = serde_json::to_string_pretty(report)?;

    let mut report_file_header = tar::Header::new_gnu();
    report_file_header.set_size(report_json.len() as u64);
    report_file_header.set_mode(0o644);
    report_file_header.set_mtime(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
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

    Ok(output_path.to_path_buf())
}

pub(crate) fn generate_and_bundle(output_path: &Path) -> Result<(), TridentError> {
    let mut report = collect_report()?;

    let mut files_to_collect = vec![
        FileToCollect {
            src: PathBuf::from(TRIDENT_BACKGROUND_LOG_PATH),
            tar_path: "logs/trident-full.log".to_string(),
            desc: "Trident execution log".to_string(),
        },
        FileToCollect {
            src: PathBuf::from(TRIDENT_METRICS_FILE_PATH),
            tar_path: "logs/trident-metrics.jsonl".to_string(),
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
                    if file_name.starts_with("trident-") && file_name.ends_with(".log") {
                        let desc = if file_name.contains("metrics") {
                            "Historical Trident metrics from past servicing".to_string()
                        } else {
                            "Historical Trident log from past servicing".to_string()
                        };
                        files_to_collect.push(FileToCollect {
                            src: entry.path(),
                            tar_path: format!("logs/historical/{}", file_name),
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
        tar_path: "datastore.sqlite".to_string(),
        desc: "Default datastore".to_string(),
    });
    files_to_collect.push(FileToCollect {
        src: paths.temporary,
        tar_path: "datastore-tmp.sqlite".to_string(),
        desc: "Temporary datastore".to_string(),
    });
    if let Some(configured) = paths.configured {
        files_to_collect.push(FileToCollect {
            src: configured,
            tar_path: "datastore-configured.sqlite".to_string(),
            desc: "Configured datastore".to_string(),
        });
    }

    let bundle_path = create_support_bundle(&mut report, output_path, files_to_collect)
        .structured(InternalError::DiagnosticBundleGeneration)?;
    info!("Diagnostics bundle created: {}", bundle_path.display());
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
                tar_path: "subdir/file1".to_string(),
                desc: "First file".to_string(),
            },
            FileToCollect {
                src: file2_path,
                tar_path: "file2".to_string(),
                desc: "Second file".to_string(),
            },
        ];

        // Create initial report
        let mut report = DiagnosticsReport {
            timestamp: chrono::Utc::now().to_rfc3339(),
            version: "test-version".to_string(),
            host_description: HostDescription {
                is_container: false,
                is_virtual: false,
                virt_type: "none".to_string(),
                platform_info: std::collections::BTreeMap::new(),
                disk_info: None,
            },
            host_status: None,
            collected_files: None,
        };

        // Create support bundle
        let bundle_path = output_dir.path().join("test-bundle.tar.zst");
        let result = create_support_bundle(&mut report, &bundle_path, files_to_collect);
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
        assert_eq!(collected[0].path, "subdir/file1");
        assert_eq!(collected[1].path, "file2");

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
        assert_eq!(collected[0].path, "subdir/file1");
        assert_eq!(collected[0].description, "First file");
        assert_eq!(collected[1].path, "file2");
        assert_eq!(collected[1].description, "Second file");
    }

    #[test]
    fn test_bundle_report_json() {
        // Create a complete report with fields populated
        let original_report = DiagnosticsReport {
            timestamp: "2025-01-15T12:00:00Z".to_string(),
            version: "1.2.3".to_string(),
            host_description: HostDescription {
                is_container: true,
                is_virtual: true,
                virt_type: "kvm".to_string(),
                platform_info: {
                    let mut map = std::collections::BTreeMap::new();
                    map.insert("cpu".to_string(), serde_json::json!("x86_64"));
                    map.insert("memory".to_string(), serde_json::json!(8192));
                    map
                },
                disk_info: Some(vec![]),
            },
            host_status: None,
            collected_files: None,
        };

        // Create bundle
        let temp_dir = tempdir().unwrap();
        let bundle_path = temp_dir.path().join("roundtrip.tar.zst");

        let mut report_copy = original_report.clone();
        create_support_bundle(&mut report_copy, &bundle_path, vec![]).unwrap();

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

        // Verify all fields match
        assert_eq!(read_report.timestamp, original_report.timestamp);
        assert_eq!(read_report.version, original_report.version);
        assert_eq!(
            read_report.host_description.is_container,
            original_report.host_description.is_container
        );
        assert_eq!(
            read_report.host_description.is_virtual,
            original_report.host_description.is_virtual
        );
        assert_eq!(
            read_report.host_description.virt_type,
            original_report.host_description.virt_type
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
        assert!(read_report.host_description.disk_info.is_some());
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

        let result = generate_and_bundle(&bundle_path);
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
            report.host_description.virt_type, "qemu",
            "Should detect QEMU virtualization"
        );
        assert!(
            !&report.host_description.platform_info.is_empty(),
            "Platform info should contain system details"
        );

        // Verify disk info is populated
        assert!(
            report.host_description.disk_info.is_some(),
            "Disk info should be present"
        );

        let disk_info = report.host_description.disk_info.as_ref().unwrap();
        assert!(
            disk_info.iter().any(|d| d.name == "sda"),
            "Should have sda in disk info"
        );
        assert!(
            disk_info.iter().any(|d| d.name == "sdb"),
            "Should have sdb in disk info"
        );
    }
}
