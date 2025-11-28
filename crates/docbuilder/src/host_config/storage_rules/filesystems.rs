use crate::markdown::table::MdTable;

use super::{get_filesystems, RuleDefinition};

pub(super) fn expects_block_device_id() -> RuleDefinition {
    let mut table = MdTable::new(vec!["Filesystem Type", "Expects Block Device"]);
    for fs in get_filesystems() {
        table.add_row(vec![
            fs.to_string(),
            if fs.expects_block_device_id() {
                "Yes"
            } else {
                "No"
            }
            .to_owned(),
        ]);
    }

    RuleDefinition {
        name: "Filesystem Block Device Requirements",
        template: "filesystem_block_device",
        body: table.render(),
    }
}

pub(super) fn sources() -> RuleDefinition {
    let mut table = MdTable::new(vec!["Filesystem Type", "Valid Source Type"]);
    for fs in get_filesystems() {
        table.add_row(vec![
            fs.to_string(),
            fs.valid_sources().filter(|s| s.document()).to_string(),
        ]);
    }

    RuleDefinition {
        name: "Filesystem Source Requirements",
        template: "filesystem_sources",
        body: table.render(),
    }
}

pub(super) fn can_be_mounted() -> RuleDefinition {
    let mut table = MdTable::new(vec!["Filesystem Type", "Mount Point"]);
    for fs in get_filesystems() {
        table.add_row(vec![
            fs.to_string(),
            {
                if fs.must_have_mountpoint() {
                    "Required"
                } else {
                    "Optional"
                }
            }
            .to_owned(),
        ]);
    }

    RuleDefinition {
        name: "Filesystem Mounting",
        template: "filesystem_mounting",
        body: table.render(),
    }
}

pub(super) fn verity_support() -> RuleDefinition {
    let mut table = MdTable::new(vec!["Filesystem Type", "Supports Verity"]);

    for fs in get_filesystems() {
        table.add_row(vec![
            fs.to_string(),
            if fs.supports_verity() { "Yes" } else { "No" }.to_owned(),
        ]);
    }

    RuleDefinition {
        name: "Filesystem Verity Support",
        template: "filesystem_verity",
        body: table.render(),
    }
}
