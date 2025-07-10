use std::{collections::HashSet, vec};

use documented::DocumentedVariants;

use trident_api::storage_graph::containers::AllowBlockList;

use crate::markdown::table::MdTable;

use super::{get_devices, get_part_types, get_referrers, RuleDefinition};

pub(super) fn referrer_description_table() -> RuleDefinition {
    let mut table = MdTable::new(vec!["Referrer kind", "Description"]);

    for referrer_kind in get_referrers() {
        table.add_row(vec![
            referrer_kind.to_string(),
            referrer_kind
                .get_variant_docs()
                .unwrap_or("No description available")
                .replace("\n\n", "<br>")
                .replace("\n", " "),
        ]);
    }

    RuleDefinition {
        name: "Referrer Description",
        template: "referrer_description",
        body: table.render(),
    }
}

pub(super) fn valid_targets_table() -> RuleDefinition {
    let dev_kinds = get_devices();

    let mut table = MdTable::new(
        vec!["Referrer ╲ Device".to_owned()]
            .into_iter()
            .chain(dev_kinds.iter().map(|k| k.to_string())),
    );

    for referrer_kind in get_referrers() {
        let mut row = vec![referrer_kind.to_string()];
        let compatible_kinds = referrer_kind.compatible_kinds();
        for dev_kind in dev_kinds.iter() {
            let is_compatible = compatible_kinds.contains(dev_kind.as_flag());
            row.push(if is_compatible { "Yes" } else { "No" }.to_owned());
        }
        table.add_row(row);
    }

    RuleDefinition {
        name: "Reference Validity",
        template: "valid_references",
        body: table.render(),
    }
}

pub(super) fn reference_count_table() -> RuleDefinition {
    let mut table = MdTable::new(vec!["Referrer type", "Min", "Max"]);

    for referrer_kind in get_referrers() {
        let cardinality = referrer_kind.valid_target_count();
        table.add_row(vec![
            referrer_kind.to_string(),
            match cardinality.min() {
                None => "0".to_owned(),
                Some(min) => min.to_string(),
            },
            match cardinality.max() {
                None => "∞".to_owned(),
                Some(max) => max.to_string(),
            },
        ]);
    }

    RuleDefinition {
        name: "Reference Count",
        template: "reference_count",
        body: table.render(),
    }
}

pub(super) fn reference_sharing_table() -> RuleDefinition {
    let mut table = MdTable::new(vec!["Referrer type", "Valid sharing peers"]);

    for referrer_kind in get_referrers() {
        table.add_row(vec![
            referrer_kind.to_string(),
            referrer_kind.valid_sharing_peers().to_string(),
        ]);
    }

    RuleDefinition {
        name: "Reference Sharing",
        template: "reference_sharing",
        body: table.render(),
    }
}

pub(super) fn homogeneous_references() -> RuleDefinition {
    let mut list: Vec<String> = Vec::new();

    for referrer_kind in get_referrers()
        .into_iter()
        .filter(|r| r.enforce_homogeneous_reference_kinds())
    {
        list.push(format!("- {referrer_kind}"));
    }

    RuleDefinition {
        name: "Homogeneous References",
        template: "homogeneous_references",
        body: list.join("\n"),
    }
}

pub(super) fn homogeneous_partition_types() -> RuleDefinition {
    let mut list: Vec<String> = Vec::new();

    for referrer_kind in get_referrers()
        .into_iter()
        .filter(|r| r.enforce_homogeneous_partition_types())
    {
        list.push(format!("- {referrer_kind}"));
    }

    RuleDefinition {
        name: "Homogeneous Partition Types",
        template: "homogeneous_partition_types",
        body: list.join("\n"),
    }
}

pub(super) fn homogeneous_partition_sizes() -> RuleDefinition {
    let mut list: Vec<String> = Vec::new();

    for referrer_kind in get_referrers()
        .into_iter()
        .filter(|r| r.enforce_homogeneous_partition_sizes())
    {
        list.push(format!("- {referrer_kind}"));
    }

    RuleDefinition {
        name: "Homogeneous Partition Sizes",
        template: "homogeneous_partition_sizes",
        body: list.join("\n"),
    }
}

pub(super) fn allowed_partition_types() -> RuleDefinition {
    let mut table = MdTable::new(vec!["Referrer type", "Allowed partition types"]);

    // Collect all partition types that are to be used in the documentation.
    let doc_partition_types = get_part_types().into_iter().collect::<HashSet<_>>();

    for referrer_kind in get_referrers() {
        // Keep only elements that are in the documentation.
        let mut allowed_types = referrer_kind.allowed_partition_types();
        match &mut allowed_types {
            // No need to add anything
            AllowBlockList::None | AllowBlockList::Any => (),

            // Keep only the types that are in the documentation and remove the
            // rest. If empty, set to he opposite.
            AllowBlockList::Allow(types) => {
                types.retain(|t| doc_partition_types.contains(t));
                if types.is_empty() {
                    allowed_types = AllowBlockList::None;
                }
            }

            AllowBlockList::Block(types) => {
                types.retain(|t| doc_partition_types.contains(t));
                if types.is_empty() {
                    allowed_types = AllowBlockList::Any;
                }
            }
        }

        table.add_row(vec![referrer_kind.to_string(), allowed_types.to_string()]);
    }

    RuleDefinition {
        name: "Allowed Partition Types",
        template: "allowed_partition_types",
        body: table.render(),
    }
}

pub(super) fn allowed_raid_levels() -> RuleDefinition {
    let mut table = MdTable::new(vec!["Referrer type", "Allowed RAID levels"]);

    for referrer_kind in get_referrers() {
        table.add_row(vec![
            referrer_kind.to_string(),
            referrer_kind
                .allowed_raid_levels()
                .map_or("May not refer to a RAID array".to_string(), |levels| {
                    levels.to_string()
                }),
        ]);
    }

    RuleDefinition {
        name: "Allowed RAID Levels",
        template: "allowed_raid_levels",
        body: table.render(),
    }
}
