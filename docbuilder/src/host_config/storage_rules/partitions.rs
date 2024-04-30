use trident_api::blkdev_graph::mountpoints::ValidMountpoints;

use crate::markdown::table::MdTable;

use super::{get_part_types, RuleDefinition};

pub(super) fn valid_mount_paths() -> RuleDefinition {
    let mut table = MdTable::new(vec!["Mount Path", "Valid Mount Paths"]);

    for pt in get_part_types() {
        table.add_row(vec![
            pt.to_string(),
            match pt.valid_mountpoints() {
                ValidMountpoints::None => "None".to_owned(),
                ValidMountpoints::Any => "Any path".to_owned(),
                ValidMountpoints::Specific(paths) => paths
                    .iter()
                    .map(|p| format!("`{}`", p.display()))
                    .collect::<Vec<String>>()
                    .join(" or "),
            },
        ]);
    }

    RuleDefinition {
        name: "Partition Type Valid Mounting Paths",
        template: "valid_mount_paths",
        body: table.render(),
    }
}

pub(super) fn matching_hash_partition() -> RuleDefinition {
    let mut table = MdTable::new(vec!["Partition Type", "Matching Hash Partition"]);

    for (pt, ptv) in get_part_types()
        .iter()
        .filter_map(|pt| pt.to_verity().map(|ptv| (pt, ptv)))
    {
        table.add_row(vec![pt.to_string(), ptv.to_string()]);
    }

    RuleDefinition {
        name: "Partition Type Matching Hash Partition",
        template: "matching_hash_partition",
        body: table.render(),
    }
}
