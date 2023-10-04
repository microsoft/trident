use std::collections::HashMap;

use itertools::Itertools;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct DocModel {
    commands: Vec<CommandModel>,
}

impl DocModel {
    pub fn new(commands: Vec<CommandModel>) -> Self {
        Self { commands }
    }
}

#[derive(Debug, Serialize)]
pub struct CommandModel {
    name: String,
    aliases: Option<Vec<String>>,
    about: Option<String>,
    usage: String,
    args: Vec<ArgModel>,
}

#[derive(Debug, Serialize)]
pub struct ArgModel {
    #[serde(skip)]
    id: clap::Id,
    name: String,
    show_name: String,
    aliases: Option<Vec<String>>,
    help: Option<String>,
    defaults: Vec<String>,
    possible_values: Vec<PossibleValue>,
    value_names: Vec<String>,
    conflicts: Vec<ArgModel>,
    required: bool,
    positional: bool,
    takes_values: bool,
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

/// Helper to store group information
struct GroupInfo<'a> {
    groups: HashMap<&'a clap::Id, Vec<&'a clap::ArgGroup>>,
    args: HashMap<&'a clap::Id, &'a clap::Arg>,
}

impl<'a> GroupInfo<'a> {
    fn new(cmd: &'a clap::Command) -> GroupInfo<'a> {
        // Make a map of Ids -> Arg
        let args = cmd
            .get_arguments()
            .map(|a| (a.get_id(), a))
            .collect::<HashMap<_, _>>();

        // Make a map of Ids -> Vec<ArgGroup>
        let groups: HashMap<&clap::Id, Vec<&clap::ArgGroup>> = cmd
            .get_groups()
            .map(|g| g.get_args().map(move |a| (a, g)))
            .flatten()
            .into_group_map();

        Self { groups, args }
    }
}

impl From<&clap::Command> for CommandModel {
    fn from(cmd: &clap::Command) -> Self {
        // Build the command
        let mut cmd = cmd.clone().term_width(70).disable_help_flag(true);
        cmd.build();

        // This is a mut borrow, so let's get it out of the way
        let usage = cmd.render_help().to_string();

        // Get the group information
        let grp_info = GroupInfo::new(&cmd);

        Self {
            name: cmd.get_name().to_string(),
            aliases: cmd
                .get_visible_aliases()
                .map(|v| Some(v.to_string()))
                .collect(),
            about: cmd.get_about().and_then(|s| Some(s.to_string())),
            usage: usage,
            args: cmd
                .get_arguments()
                .map(|arg| {
                    ArgModel::from(arg)
                        .with_conflicts(cmd.get_arg_conflicts_with(arg))
                        .with_groups(&grp_info)
                })
                .filter(|v| v.name != "help")
                .collect(),
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
            help: arg.get_long_help().map(|s| s.to_string()),
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

impl ArgModel {
    fn with_conflicts(mut self, other: Vec<&clap::Arg>) -> Self {
        self.conflicts = other.into_iter().map(ArgModel::from).collect();
        self
    }

    fn with_groups(mut self, group_info: &GroupInfo) -> Self {
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
