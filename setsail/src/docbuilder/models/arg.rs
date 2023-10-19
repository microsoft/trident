use serde::Serialize;

use super::command::GroupInfo;

#[derive(Debug, Serialize)]
pub(crate) struct ArgModel {
    #[serde(skip)]
    pub id: clap::Id,
    pub name: String,
    pub show_name: String,
    pub aliases: Option<Vec<String>>,
    pub help: Option<String>,
    pub defaults: Vec<String>,
    pub possible_values: Vec<PossibleValue>,
    pub value_names: Vec<String>,
    pub conflicts: Vec<ArgModel>,
    pub required: bool,
    pub positional: bool,
    pub takes_values: bool,
}

impl ArgModel {
    pub fn with_conflicts(mut self, other: Vec<&clap::Arg>) -> Self {
        self.conflicts = other.into_iter().map(ArgModel::from).collect();
        self
    }

    pub(super) fn with_groups(mut self, group_info: &GroupInfo) -> Self {
        let groups = group_info.groups.get(&self.id).cloned().unwrap_or_default();
        for mut grp in groups.into_iter().cloned() {
            // For whatever reason this needs a mutable borrow, so we clone the group
            if grp.is_multiple() {
                continue;
            }

            for arg in grp.get_args() {
                if arg != &self.id {
                    self.conflicts
                        .push(ArgModel::from(*group_info.args.get(arg).unwrap()));
                }
            }
        }
        self
    }
}

#[derive(Debug, Serialize)]
pub struct PossibleValue {
    name: String,
    help: Option<String>,
    aliases: Vec<String>,
    hide: bool,
}

impl From<&clap::builder::PossibleValue> for PossibleValue {
    fn from(value: &clap::builder::PossibleValue) -> Self {
        Self {
            name: value.get_name().to_string(),
            help: value.get_help().map(|s| s.to_string()),
            aliases: value
                .get_name_and_aliases()
                .skip(1)
                .map(|s| s.to_string())
                .collect(),
            hide: value.is_hide_set(),
        }
    }
}

impl From<&clap::Arg> for ArgModel {
    fn from(arg: &clap::Arg) -> Self {
        Self {
            id: arg.get_id().clone(),
            name: arg.get_id().to_string(),
            conflicts: vec![],
            show_name: if arg.is_positional() {
                format!(
                    "<{}>",
                    arg.get_value_names()
                        .map(|v| v.get(0).map(|v| v.to_string()).unwrap())
                        .unwrap_or(arg.get_id().to_string())
                )
            } else {
                if let Some(value_names) = arg.get_value_names() {
                    format!(
                        "--{} <{}>",
                        arg.get_id(),
                        value_names
                            .iter()
                            .map(|v| v.to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                } else {
                    format!("--{}", arg.get_id())
                }
            },
            aliases: arg
                .get_visible_aliases()
                .map(|v| v.iter().map(|s| s.to_string()).collect()),
            help: arg
                .get_long_help()
                .map(|s| s.to_string())
                .or_else(|| arg.get_help().map(|s| s.to_string())),
            defaults: arg
                .get_default_values()
                .iter()
                .map(|v| v.to_string_lossy().into_owned())
                .collect(),
            value_names: arg
                .get_value_names()
                .and_then(|v| Some(v.iter().map(|s| s.to_string()).collect()))
                .unwrap_or_default(),
            required: arg.is_required_set(),
            positional: arg.is_positional(),
            takes_values: arg
                .get_num_args()
                .and_then(|v| Some(v.takes_values()))
                .unwrap_or_default(),
            possible_values: arg
                .get_possible_values()
                .iter()
                .map(PossibleValue::from)
                .collect(),
        }
    }
}
