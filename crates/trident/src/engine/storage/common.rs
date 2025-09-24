use std::{collections::HashSet, hash::Hash};

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub(super) enum SetRelationship {
    Disjoint,
    Overlap,
    Subset,
}

/// Given two sets, checked and reference, return what checked is in relation to reference.
///
/// It can be disjoint, overlap, or subset:
///
/// - Disjoint: checked and reference have no elements in common.
///   ```text
///   Ref       Checked
///   ┌──────┐ ┌──────┐
///   │      │ │      │
///   │      │ │      │
///   │      │ │      │
///   └──────┘ └──────┘
///   ```
/// - Overlap: checked and reference have elements in common, but checked is not a subset of reference.
///   ```text
///   Ref       Checked
///   ┌────────┐      
///   │    ┌─────────┐
///   │    │   │     │
///   │    │   │     │
///   │    │   │     │
///   │    └─────────┘
///   └────────┘
///   ```
/// - Subset: checked is a subset of reference.
///   ```text
///   Ref       Checked
///   ┌──────────────┐
///   │              │
///   │    ┌────────┐│
///   │    │        ││
///   │    │        ││
///   │    └────────┘│
///   │              │
///   └──────────────┘
///   ```
pub(super) fn subset_check<T: Hash + Eq + Clone>(
    checked_subset: &HashSet<T>,
    reference_subset: &HashSet<T>,
) -> SetRelationship {
    let symmetric_diff = checked_subset
        .symmetric_difference(reference_subset)
        .collect::<HashSet<_>>();

    if checked_subset.is_disjoint(reference_subset) {
        // There is no overlap between the two sets
        SetRelationship::Disjoint
    } else if symmetric_diff.is_empty() || checked_subset.is_subset(reference_subset) {
        // Device's underlying disks are all part of HostConfig, we can unmount and stop the RAID
        SetRelationship::Subset
    } else {
        // There is overlap between the two sets, but checked_subset is not a subset of reference_subset
        SetRelationship::Overlap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subset_check() {
        let set_a: HashSet<i32> = [1, 2, 3].iter().cloned().collect();
        let set_b: HashSet<i32> = [4, 5, 6].iter().cloned().collect();
        assert_eq!(subset_check(&set_a, &set_b), SetRelationship::Disjoint);

        let set_c: HashSet<i32> = [1, 2, 3].iter().cloned().collect();
        let set_d: HashSet<i32> = [2, 3, 4].iter().cloned().collect();
        assert_eq!(subset_check(&set_c, &set_d), SetRelationship::Overlap);

        let set_e: HashSet<i32> = [1, 2].iter().cloned().collect();
        let set_f: HashSet<i32> = [1, 2, 3].iter().cloned().collect();
        assert_eq!(subset_check(&set_e, &set_f), SetRelationship::Subset);
    }
}
