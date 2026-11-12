use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use anyhow::{anyhow, Context};
use log::info;
use trident_api::{
    config::{DiskSelectionStrategy, HostConfiguration},
    error::{InvalidInputError, ReportError, TridentError},
    BlockDeviceId,
};

use super::inventory::Candidate;

/// Greedily replaces selectors with eligible device paths in disk-list order.
///
/// Accepts only eligible disks from discovery and removes explicit reservations.
/// Filters available disks by layout size before applying the strategy, then
/// removes each choice before the next disk. Earlier replacements
/// remain in `spec` if a later choice fails.
pub(super) fn select(
    spec: &mut HostConfiguration,
    requirements: &HashMap<BlockDeviceId, u64>,
    candidates: &[Candidate],
    reserved: HashSet<PathBuf>,
) -> Result<(), TridentError> {
    let mut available = candidates
        .iter()
        .filter(|candidate| candidate.device_paths.is_disjoint(&reserved))
        .collect::<Vec<_>>();

    for disk in &mut spec.storage.disks {
        let Some(selector) = &disk.selector else {
            // Ignore disks without a selector.
            continue;
        };

        // Get the calculated required capacity for this disk.
        let required = *requirements
            .get(&disk.id)
            .context("Missing disk capacity requirement")
            .structured(InvalidInputError::DiskSelection {
                disk_id: disk.id.clone(),
            })?;

        let eligible = available
            .iter()
            .copied()
            .filter(|candidate| candidate.size >= required);

        let candidate = match selector.strategy {
            DiskSelectionStrategy::SmallestFit => smallest_fit(eligible),
            DiskSelectionStrategy::TieredSmallestFit => tiered_smallest_fit(eligible),
        };

        let Some(candidate) = candidate else {
            let reasons = available
                .iter()
                .map(|candidate| {
                    format!("{}: too small ({} bytes)", candidate.name, candidate.size)
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Err(anyhow!(
                "No eligible disk fits '{}' (requires {required} bytes). {}",
                disk.id,
                if reasons.is_empty() {
                    "No available whole disks remain"
                } else {
                    &reasons
                }
            ))
            .structured(InvalidInputError::DiskSelection {
                disk_id: disk.id.clone(),
            });
        };

        disk.device = candidate.path.clone();
        disk.selector = None;
        available.retain(|other| other.device_paths.is_disjoint(&candidate.device_paths));

        info!(
            "Selected '{}' for disk '{}' ({} bytes; requires {required} bytes)",
            candidate.path.display(),
            disk.id,
            candidate.size
        );
    }
    Ok(())
}

/// Returns the smallest disk, breaking ties by family and natural device order.
fn smallest_fit<'a>(candidates: impl Iterator<Item = &'a Candidate>) -> Option<&'a Candidate> {
    candidates.min_by_key(|candidate| (candidate.size, &candidate.order, &candidate.name))
}

/// Returns the smallest disk in the highest-priority available family.
fn tiered_smallest_fit<'a>(
    candidates: impl Iterator<Item = &'a Candidate>,
) -> Option<&'a Candidate> {
    candidates.min_by_key(|candidate| {
        (
            candidate.order.family(),
            candidate.size,
            &candidate.order,
            &candidate.name,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use trident_api::{
        config::{Disk, DiskSelector},
        error::ErrorKind,
    };

    use crate::disk_selector::inventory::DeviceOrder;

    /// Models an eligible disk without accessing the host block-device inventory.
    fn candidate(name: &str, size: u64) -> Candidate {
        let path: PathBuf = format!("/dev/{name}").into();
        Candidate {
            name: name.into(),
            path: path.clone(),
            device_paths: HashSet::from([path]),
            size,
            order: DeviceOrder::parse(name).unwrap(),
        }
    }

    /// Builds selector input for tests that exercise assignment without validating a layout.
    fn spec(strategy: DiskSelectionStrategy) -> HostConfiguration {
        HostConfiguration {
            storage: trident_api::config::Storage {
                disks: vec![Disk {
                    id: "os".into(),
                    selector: Some(DiskSelector { strategy }),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn strategies_prioritize_size_and_family_differently() {
        let disks = [candidate("nvme0n1", 200), candidate("sda", 100)];
        assert_eq!(smallest_fit(disks.iter()).unwrap().path, disks[1].path);
        assert_eq!(
            tiered_smallest_fit(disks.iter()).unwrap().path,
            disks[0].path
        );
    }

    #[test]
    fn equal_capacity_uses_family_and_natural_order() {
        let disks = [
            candidate("sda", 100),
            candidate("nvme10n1", 100),
            candidate("nvme2n10", 100),
            candidate("nvme2n2", 100),
        ];
        assert_eq!(smallest_fit(disks.iter()).unwrap().path, disks[3].path);
        assert_eq!(
            tiered_smallest_fit(disks.iter()).unwrap().path,
            disks[3].path
        );
    }

    #[test]
    fn empty_input_has_no_selection() {
        assert!(smallest_fit(std::iter::empty()).is_none());
        assert!(tiered_smallest_fit(std::iter::empty()).is_none());
    }

    #[test]
    fn capacity_first_and_tier_first_are_distinct() {
        let disks = [candidate("nvme0n1", 200), candidate("sda", 100)];
        let sizes = HashMap::from([("os".into(), 50)]);
        for (strategy, expected) in [
            (DiskSelectionStrategy::SmallestFit, "sda"),
            (DiskSelectionStrategy::TieredSmallestFit, "nvme0n1"),
        ] {
            let mut input = spec(strategy);
            select(&mut input, &sizes, &disks, HashSet::new()).unwrap();
            assert_eq!(
                input.storage.disks[0].device,
                disks.iter().find(|d| d.name == expected).unwrap().path
            );
            assert!(input.storage.disks[0].selector.is_none());
            assert!(!serde_yaml::to_string(&input).unwrap().contains("selector"));
        }
    }

    #[test]
    fn selects_current_device_paths_without_stable_identity_aliases() {
        let disks = [candidate("sda", 100)];
        for strategy in [
            DiskSelectionStrategy::SmallestFit,
            DiskSelectionStrategy::TieredSmallestFit,
        ] {
            let mut input = spec(strategy);
            select(
                &mut input,
                &HashMap::from([("os".into(), 50)]),
                &disks,
                HashSet::new(),
            )
            .unwrap();
            assert_eq!(input.storage.disks[0].device, PathBuf::from("/dev/sda"));
            assert!(input.storage.disks[0].selector.is_none());
        }
    }

    #[test]
    fn equal_capacity_candidates_use_family_priority() {
        let disks = [
            candidate("hda", 100),
            candidate("mmcblk0", 100),
            candidate("vda", 100),
            candidate("sda", 100),
            candidate("nvme0n1", 100),
        ];
        let sizes = HashMap::from([("os".into(), 50)]);
        for strategy in [
            DiskSelectionStrategy::SmallestFit,
            DiskSelectionStrategy::TieredSmallestFit,
        ] {
            let mut input = spec(strategy);
            select(&mut input, &sizes, &disks, HashSet::new()).unwrap();
            assert_eq!(input.storage.disks[0].device, disks[4].path);
        }
    }

    #[test]
    fn undersized_tiers_are_skipped() {
        let disks = [
            candidate("nvme0n1", 40),
            candidate("sda", 40),
            candidate("vda", 100),
        ];
        let mut input = spec(DiskSelectionStrategy::TieredSmallestFit);
        select(
            &mut input,
            &HashMap::from([("os".into(), 50)]),
            &disks,
            HashSet::new(),
        )
        .unwrap();
        assert_eq!(input.storage.disks[0].device, disks[2].path);
    }

    #[test]
    fn explicit_reservations_precede_greedy_selector_order() {
        let mut disks = [
            candidate("sda", 100),
            candidate("sdb", 200),
            candidate("sdc", 300),
        ];
        let reserved_partition = PathBuf::from("/dev/sda1");
        disks[0].device_paths.insert(reserved_partition.clone());
        let mut input = spec(DiskSelectionStrategy::SmallestFit);
        input.storage.disks.push(Disk {
            id: "second".into(),
            selector: Some(DiskSelector::default()),
            ..Default::default()
        });
        let sizes = HashMap::from([("os".into(), 50), ("second".into(), 50)]);
        select(
            &mut input,
            &sizes,
            &disks,
            HashSet::from([reserved_partition]),
        )
        .unwrap();
        assert_eq!(input.storage.disks[0].device, disks[1].path);
        assert_eq!(input.storage.disks[1].device, disks[2].path);
    }

    #[test]
    fn undersized_candidates_remain_available_for_later_disks() {
        let disks = [candidate("sda", 100), candidate("sdb", 200)];
        let mut input = spec(DiskSelectionStrategy::SmallestFit);
        input.storage.disks.push(Disk {
            id: "second".into(),
            selector: Some(DiskSelector::default()),
            ..Default::default()
        });
        select(
            &mut input,
            &HashMap::from([("os".into(), 150), ("second".into(), 100)]),
            &disks,
            HashSet::new(),
        )
        .unwrap();
        assert_eq!(input.storage.disks[0].device, disks[1].path);
        assert_eq!(input.storage.disks[1].device, disks[0].path);
    }

    #[test]
    fn selected_devices_are_removed_from_available_candidates() {
        let mut disks = [
            candidate("sda", 100),
            candidate("sdb", 100),
            candidate("sdc", 200),
        ];
        disks[1].device_paths = disks[0].device_paths.clone();
        let mut input = spec(DiskSelectionStrategy::SmallestFit);
        input.storage.disks.push(Disk {
            id: "second".into(),
            selector: Some(DiskSelector::default()),
            ..Default::default()
        });
        select(
            &mut input,
            &HashMap::from([("os".into(), 100), ("second".into(), 100)]),
            &disks,
            HashSet::new(),
        )
        .unwrap();
        assert_eq!(input.storage.disks[0].device, disks[0].path);
        assert_eq!(input.storage.disks[1].device, disks[2].path);
    }

    #[test]
    fn greedy_assignment_does_not_backtrack() {
        let disks = [candidate("nvme0n1", 200), candidate("sda", 100)];
        let mut input = spec(DiskSelectionStrategy::TieredSmallestFit);
        input.storage.disks.push(Disk {
            id: "second".into(),
            selector: Some(DiskSelector::default()),
            ..Default::default()
        });
        let error = select(
            &mut input,
            &HashMap::from([("os".into(), 50), ("second".into(), 150)]),
            &disks,
            HashSet::new(),
        )
        .unwrap_err();
        assert_eq!(
            error.kind(),
            &ErrorKind::InvalidInput(InvalidInputError::DiskSelection {
                disk_id: "second".into(),
            })
        );
        assert!(format!("{error:?}").contains("requires 150 bytes"));
        assert_eq!(input.storage.disks[0].device, disks[0].path);
    }

    #[test]
    fn missing_capacity_reports_the_current_disk() {
        let disks = [candidate("sda", 100), candidate("sdb", 200)];
        let mut input = spec(DiskSelectionStrategy::SmallestFit);
        input.storage.disks.push(Disk {
            id: "second".into(),
            selector: Some(DiskSelector::default()),
            ..Default::default()
        });
        let error = select(
            &mut input,
            &HashMap::from([("os".into(), 50)]),
            &disks,
            HashSet::new(),
        )
        .unwrap_err();
        assert_eq!(
            error.kind(),
            &ErrorKind::InvalidInput(InvalidInputError::DiskSelection {
                disk_id: "second".into(),
            })
        );
        assert_eq!(input.storage.disks[0].device, disks[0].path);
    }
}
