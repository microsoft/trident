use crate::markdown::table::MdTable;

use super::{get_devices, RuleDefinition};

pub(super) fn unique_field_value_constraints() -> RuleDefinition {
    let dev_kinds = get_devices();

    let table = MdTable::new(["Device Kind", "Field Name"]).with_rows(
        dev_kinds
            .iter()
            .flat_map(|kind| {
                kind.uniqueness_constraints().map(|constraints| {
                    constraints
                        .iter()
                        .map(|(field_name, _)| vec![kind.to_string(), field_name.to_string()])
                        .collect::<Vec<Vec<String>>>()
                })
            })
            .flatten(),
    );

    RuleDefinition {
        name: "Unique Field Value Constraints",
        template: "unique_field_value_constraints",
        body: table.render(),
    }
}
