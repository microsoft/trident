use std::collections::HashMap;

use itertools::Itertools;
use serde::Serialize;

use super::arg::ArgModel;

#[derive(Debug, Serialize)]
pub(crate) struct CommandModel {
    pub name: String,
    pub aliases: Option<Vec<String>>,
    pub about: Option<String>,
    pub usage: String,
    pub args: Vec<ArgModel>,
}

impl CommandModel {
    pub fn from_cmd(cmd: &clap::Command, usage_template: Option<impl Into<String>>) -> Self {
        // Build the command
        let mut cmd = cmd
            .clone()
            .term_width(70)
            .disable_help_flag(true)
            .help_expected(true);

        if let Some(template) = usage_template {
            cmd = cmd.help_template(template.into());
        }

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
            about: cmd.get_about().map(|s| s.to_string()),
            usage,
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

impl From<&clap::Command> for CommandModel {
    fn from(cmd: &clap::Command) -> Self {
        Self::from_cmd(
            cmd,
            Some(include_str!("../templates/command_usage.template")),
        )
    }
}

/// Helper to store group information
pub(super) struct GroupInfo<'a> {
    pub groups: HashMap<&'a clap::Id, Vec<&'a clap::ArgGroup>>,
    pub args: HashMap<&'a clap::Id, &'a clap::Arg>,
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
            .flat_map(|g| g.get_args().map(move |a| (a, g)))
            .into_group_map();

        Self { groups, args }
    }
}
