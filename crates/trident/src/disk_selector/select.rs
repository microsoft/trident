use super::inventory::Candidate;

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

    use std::{collections::HashSet, path::PathBuf};

    use crate::disk_selector::inventory::DeviceOrder;

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
}
