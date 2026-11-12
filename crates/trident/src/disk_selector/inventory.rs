use std::{collections::HashSet, fs, os::unix::fs::FileTypeExt, path::PathBuf};

use anyhow::{Context, Error};
use log::{debug, error};

use osutils::{
    cmdline,
    lsblk::{self, BlockDevice, BlockDeviceType},
    sysfs::SysfsBlockDevice,
    udevadm,
};
use trident_api::primitives::bytes::ByteCount;

/// Builds a read-only inventory of eligible whole disks for automatic selection.
///
/// Settles udev, enumerates whole disks with lsblk, then probes [`PreCandidate`]
/// snapshots. The in-memory funnel produces [`Candidate`] entries for supported
/// NVMe, sd, vd, mmcblk, and hd names.
///
/// Skips read-only, removable, or USB-backed disks; mounted devices, active swap,
/// or block-device holders on the disk or its descendants; resolved boot-source
/// matches; non-eMMC MMC media; and logical sectors other than 512 or 4096 bytes.
/// Each snapshot passes through [`candidate_funnel`]. Probe failures are logged
/// immediately; rejections and candidates are summarized at debug level.
/// Existing partitions or filesystem signatures alone do not disqualify a disk.
///
/// Does not select or reserve disks. Callers enforce layout capacity and
/// reservations. Probe failures are distinct from known rejections and do not
/// prevent other viable disks from being returned.
/// Enumeration and boot-source failures abort discovery.
pub(super) fn discover() -> Result<Vec<Candidate>, Error> {
    udevadm::settle().context("Failed to settle disk inventory")?;
    let disks = lsblk::find(|device| device.blkdev_type == BlockDeviceType::Disk)
        .context("Failed to enumerate disks")?;

    // Resolve kernel root devices for comparison with each disk's descendant paths.
    let boot_sources = cmdline::boot_sources()?;

    // Prepare containers for viable and rejected disks.
    let mut candidates = Vec::new();
    let mut rejected = Vec::with_capacity(disks.len());

    for disk in disks.iter() {
        let path = disk.device_path();

        let precandidate = match PreCandidate::probe(disk) {
            Ok(precandidate) => precandidate,
            Err(error) => {
                // Log the probe failure and continue with the next disk.
                error!("Failed to probe {}: {}", path.display(), error);
                continue;
            }
        };

        match candidate_funnel(precandidate, &boot_sources) {
            Ok(candidate) => candidates.push(candidate),
            Err(reason) => {
                rejected.push((path, reason));
            }
        }
    }

    debug!("Disk selector discovery probed {} disks", disks.len());
    debug!("Disk selector discovery rejected disks:");
    for (path, reason) in &rejected {
        debug!("{} - rejected: {}", path.display(), reason);
    }

    debug!("Disk selector discovery accepted disks:");
    for candidate in &candidates {
        debug!(
            "{} - family: {:?}; size: {}",
            candidate.path.display(),
            candidate.order.family,
            ByteCount(candidate.size).to_human_readable_approx()
        );
    }

    Ok(candidates)
}

/// Supported kernel-name families, declared in tier-preference order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum DeviceFamily {
    /// NVMe devices, e.g., `nvme0n1`
    Nvme,

    /// SATA devices, e.g., `sda`
    Sd,

    /// Virtio devices, e.g., `vd`
    Vd,

    /// MMC devices, e.g., `mmcblk0`
    Mmcblk,

    /// PATA/IDE devices, e.g., `hd`
    Hd,
}

/// Family-priority and natural kernel-name ordering key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct DeviceOrder {
    /// Kernel-name family, ordered by selection tier.
    family: DeviceFamily,

    /// NVMe controller index, MMC device index, or alphabetic suffix length.
    first: u64,

    /// NVMe namespace index; zero for other families.
    second: u64,

    /// Alphabetic suffix for sd/vd/hd names; empty for numeric-name families.
    letters: String,
}

impl DeviceOrder {
    /// Builds a family-priority and natural-sort key from a supported disk name.
    ///
    /// Rejects unsupported names, partitions, and MMC special-area suffixes.
    /// Name parsing alone does not verify the device's existence or eligibility.
    pub(super) fn parse(name: &str) -> Option<Self> {
        let numeric = |value: &str| {
            (!value.is_empty() && value.bytes().all(|c| c.is_ascii_digit()))
                .then(|| value.parse::<u64>().ok())
                .flatten()
        };

        let (family, first, second, letters) = if let Some(value) = name.strip_prefix("nvme") {
            let (controller, namespace) = value.split_once('n')?;
            (
                DeviceFamily::Nvme,
                numeric(controller)?,
                numeric(namespace)?,
                String::new(),
            )
        } else if let Some(value) = name.strip_prefix("mmcblk") {
            (DeviceFamily::Mmcblk, numeric(value)?, 0, String::new())
        } else {
            let (family, suffix) = [
                ("sd", DeviceFamily::Sd),
                ("vd", DeviceFamily::Vd),
                ("hd", DeviceFamily::Hd),
            ]
            .into_iter()
            .find_map(|(prefix, family)| name.strip_prefix(prefix).map(|s| (family, s)))?;

            if suffix.is_empty() || !suffix.bytes().all(|c| c.is_ascii_lowercase()) {
                return None;
            }

            (family, suffix.len() as u64, 0, suffix.to_owned())
        };
        Some(Self {
            family,
            first,
            second,
            letters,
        })
    }

    /// Returns the kernel-name family used for tiering and tie-breaking.
    pub(super) fn family(&self) -> DeviceFamily {
        self.family
    }
}

/// System-derived disk properties, captured before any eligibility decisions.
#[derive(Debug, Clone)]
struct PreCandidate {
    /// Kernel device name.
    name: String,

    /// Current device path reported by lsblk.
    path: PathBuf,

    /// Canonical paths for the disk and all its descendants.
    device_paths: HashSet<PathBuf>,

    /// Total disk capacity in bytes.
    size: u64,

    /// Whether either lsblk or sysfs reports the disk as read-only.
    read_only: bool,

    /// Whether the disk or a descendant is mounted or used as active swap.
    mounted: bool,

    /// Whether the reported path refers to a block-device node.
    is_block_device: bool,

    /// Transport reported by udev, if present.
    bus: Option<String>,

    /// Kernel removable-media flag.
    removable: bool,

    /// MMC media type check; absent for other device families.
    is_emmc: Option<bool>,

    /// Logical sector size in bytes.
    logical_sector_size: u32,

    /// Whether the disk or any descendant has an active block-device holder.
    has_holders: bool,
}

impl PreCandidate {
    /// Reads all device properties; missing or malformed required data is an error.
    fn probe(disk: &BlockDevice) -> Result<Self, Error> {
        let path = disk.device_path();
        let metadata = fs::metadata(&path)
            .with_context(|| format!("Failed to inspect block device '{}'", path.display()))?;
        let properties = udevadm::properties(&path)?;
        let sysfs = SysfsBlockDevice::new(disk.sysfs_path());
        let sysfs_read_only = sysfs.is_read_only()?;
        let is_emmc = if disk.device_file_name().starts_with("mmcblk") {
            Some(sysfs.is_emmc()?)
        } else {
            None
        };

        Ok(Self {
            name: disk.device_file_name().into(),
            path: path.clone(),
            device_paths: disk.device_paths_recursive()?,
            size: disk.size,
            read_only: disk.readonly || sysfs_read_only,
            mounted: disk.is_mounted_recursive(),
            is_block_device: metadata.file_type().is_block_device(),
            bus: properties.bus,
            removable: sysfs.is_removable()?,
            is_emmc,
            logical_sector_size: sysfs.logical_sector_size()?,
            has_holders: disk.has_holders_recursive().with_context(|| {
                format!(
                    "Failed to inspect block-device holders for '{}'",
                    path.display()
                )
            })?,
        })
    }
}

/// Eligible whole disk available for layout matching and selection.
#[derive(Debug, Clone)]
pub(super) struct Candidate {
    /// Kernel device name, such as `nvme0n1` or `sda`.
    pub name: String,

    /// Current whole-disk device path reported by lsblk.
    pub path: PathBuf,

    /// Canonical paths for this disk and all its lsblk descendants.
    pub device_paths: HashSet<PathBuf>,

    /// Total disk capacity in bytes.
    pub size: u64,

    /// Family-priority and natural-name ordering key.
    pub order: DeviceOrder,
}

/// Reason a probed disk cannot become a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
enum RefusalReason {
    #[error("unsupported device name")]
    UnsupportedDeviceName,
    #[error("read-only")]
    ReadOnly,
    #[error("mounted disk or descendant, including active swap")]
    MountedOrSwap,
    #[error("not a block device")]
    NotBlockDevice,
    #[error("USB-backed")]
    UsbBacked,
    #[error("removable")]
    Removable,
    #[error("not an eMMC user-area disk")]
    NotEmmcUserArea,
    #[error("unsupported logical sector size")]
    UnsupportedLogicalSectorSize,
    #[error("active block-device holder")]
    ActiveBlockDeviceHolder,
    #[error("installation-media backing device")]
    InstallationMediaBackingDevice,
}

/// Narrows a snapshot through eligibility checks, stopping at the first rejection.
///
/// Uses only in-memory data. Boot-source paths must already be canonical.
fn candidate_funnel(
    disk: PreCandidate,
    boot_sources: &HashSet<PathBuf>,
) -> Result<Candidate, RefusalReason> {
    let order = DeviceOrder::parse(&disk.name).ok_or(RefusalReason::UnsupportedDeviceName)?;

    if disk.read_only {
        return Err(RefusalReason::ReadOnly);
    }

    if disk.mounted {
        return Err(RefusalReason::MountedOrSwap);
    }

    if !disk.is_block_device {
        return Err(RefusalReason::NotBlockDevice);
    }

    if disk.bus.as_deref() == Some("usb") {
        return Err(RefusalReason::UsbBacked);
    }

    if disk.removable {
        return Err(RefusalReason::Removable);
    }

    // MMC and SD cards share the mmcblk name family.
    if order.family == DeviceFamily::Mmcblk && disk.is_emmc != Some(true) {
        return Err(RefusalReason::NotEmmcUserArea);
    }

    if !matches!(disk.logical_sector_size, 512 | 4096) {
        return Err(RefusalReason::UnsupportedLogicalSectorSize);
    }

    if disk.has_holders {
        return Err(RefusalReason::ActiveBlockDeviceHolder);
    }

    if !disk.device_paths.is_disjoint(boot_sources) {
        return Err(RefusalReason::InstallationMediaBackingDevice);
    }

    Ok(Candidate {
        name: disk.name,
        path: disk.path,
        device_paths: disk.device_paths,
        size: disk.size,
        order,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Models a fully probed disk without accessing device nodes or sysfs.
    fn precandidate(name: &str) -> PreCandidate {
        let path: PathBuf = format!("/dev/{name}").into();
        PreCandidate {
            name: name.into(),
            path: path.clone(),
            device_paths: HashSet::from([path]),
            size: 1_000_000,
            read_only: false,
            mounted: false,
            is_block_device: true,
            bus: None,
            removable: false,
            is_emmc: None,
            logical_sector_size: 512,
            has_holders: false,
        }
    }

    #[test]
    fn whole_disk_names_have_natural_family_order() {
        let names = [
            "nvme2n2", "nvme2n10", "nvme10n1", "sdz", "sdaa", "vda", "mmcblk2", "mmcblk10", "hda",
        ];
        let keys = names.map(|name| DeviceOrder::parse(name).unwrap());
        assert!(keys.windows(2).all(|pair| pair[0] < pair[1]));
        for name in [
            "sda1",
            "nvme0n1p1",
            "mmcblk0p1",
            "mmcblk0boot0",
            "mmcblk0boot1",
            "mmcblk0rpmb",
            "xvda",
        ] {
            assert!(DeviceOrder::parse(name).is_none(), "{name}");
            assert_eq!(
                candidate_funnel(precandidate(name), &HashSet::new()).unwrap_err(),
                RefusalReason::UnsupportedDeviceName,
                "{name}"
            );
        }
    }

    #[test]
    fn only_viable_snapshots_become_candidates() {
        let mut candidates = Vec::new();
        for (disk, rejection) in [
            (precandidate("nvme0n1"), None),
            (
                PreCandidate {
                    read_only: true,
                    ..precandidate("sda")
                },
                Some(RefusalReason::ReadOnly),
            ),
            (
                PreCandidate {
                    mounted: true,
                    ..precandidate("sdb")
                },
                Some(RefusalReason::MountedOrSwap),
            ),
            (
                PreCandidate {
                    is_block_device: false,
                    ..precandidate("sdc")
                },
                Some(RefusalReason::NotBlockDevice),
            ),
            (
                PreCandidate {
                    bus: Some("usb".into()),
                    ..precandidate("sdd")
                },
                Some(RefusalReason::UsbBacked),
            ),
            (
                PreCandidate {
                    removable: true,
                    ..precandidate("sde")
                },
                Some(RefusalReason::Removable),
            ),
            (
                PreCandidate {
                    is_emmc: Some(false),
                    ..precandidate("mmcblk0")
                },
                Some(RefusalReason::NotEmmcUserArea),
            ),
            (
                precandidate("mmcblk1"),
                Some(RefusalReason::NotEmmcUserArea),
            ),
            (
                PreCandidate {
                    logical_sector_size: 8192,
                    ..precandidate("sdf")
                },
                Some(RefusalReason::UnsupportedLogicalSectorSize),
            ),
            (
                PreCandidate {
                    logical_sector_size: 0,
                    ..precandidate("sdg")
                },
                Some(RefusalReason::UnsupportedLogicalSectorSize),
            ),
            (
                PreCandidate {
                    has_holders: true,
                    ..precandidate("sdh")
                },
                Some(RefusalReason::ActiveBlockDeviceHolder),
            ),
            (
                precandidate("xvda"),
                Some(RefusalReason::UnsupportedDeviceName),
            ),
            (
                PreCandidate {
                    is_emmc: Some(true),
                    ..precandidate("mmcblk2")
                },
                None,
            ),
            (precandidate("vda"), None),
        ] {
            let result = candidate_funnel(disk, &HashSet::new());
            match rejection {
                Some(reason) => assert_eq!(result.unwrap_err(), reason),
                None => candidates.push(result.unwrap().name),
            }
        }
        assert_eq!(candidates, ["nvme0n1", "mmcblk2", "vda"]);
    }

    #[test]
    fn supported_families_and_sector_sizes_preserve_candidate_metadata() {
        for (name, bus, is_emmc) in [
            ("nvme0n1", None, None),
            ("sda", Some("ata"), None),
            ("vda", Some("virtio"), None),
            ("mmcblk0", Some("mmc"), Some(true)),
            ("hda", Some("ata"), None),
        ] {
            for logical_sector_size in [512, 4096] {
                let mut disk = PreCandidate {
                    bus: bus.map(str::to_owned),
                    is_emmc,
                    logical_sector_size,
                    ..precandidate(name)
                };
                disk.device_paths
                    .insert(format!("/dev/{name}-child").into());
                let candidate = candidate_funnel(disk.clone(), &HashSet::new()).unwrap();
                assert_eq!(candidate.name, disk.name);
                assert_eq!(candidate.path, disk.path);
                assert_eq!(candidate.device_paths, disk.device_paths);
                assert_eq!(candidate.size, disk.size);
                assert_eq!(candidate.order, DeviceOrder::parse(name).unwrap());
            }
        }
    }

    #[test]
    fn boot_sources_exclude_whole_disks_and_descendants() {
        let mut disk = precandidate("sda");
        disk.device_paths.insert("/dev/sda1".into());
        for source in ["/dev/sda", "/dev/sda1"] {
            assert_eq!(
                candidate_funnel(disk.clone(), &HashSet::from([source.into()])).unwrap_err(),
                RefusalReason::InstallationMediaBackingDevice
            );
        }
        assert!(candidate_funnel(disk, &HashSet::from(["/dev/sdb1".into()])).is_ok());
    }

    #[test]
    fn funnel_stops_at_the_first_rejection() {
        let mut disk = PreCandidate {
            read_only: true,
            mounted: true,
            bus: Some("usb".into()),
            ..precandidate("sda")
        };
        assert_eq!(
            candidate_funnel(disk.clone(), &HashSet::new()).unwrap_err(),
            RefusalReason::ReadOnly
        );
        disk.read_only = false;
        assert_eq!(
            candidate_funnel(disk.clone(), &HashSet::new()).unwrap_err(),
            RefusalReason::MountedOrSwap
        );
        disk.mounted = false;
        assert_eq!(
            candidate_funnel(disk, &HashSet::new()).unwrap_err(),
            RefusalReason::UsbBacked
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;

    #[functional_test(feature = "helpers")]
    fn test_discover() {
        let candidates = discover().unwrap();
        // An in-use host may legitimately have no eligible disks.
        for candidate in candidates {
            assert!(candidate.device_paths.contains(&candidate.path));
        }
    }
}
