pub(super) mod arg;
pub(super) mod command;
pub(super) mod section;

use command::CommandModel;
use section::SectionModel;
use serde::Serialize;

#[derive(Default, Debug, Serialize)]
pub(super) struct DocModel {
    sections: Vec<SectionModel>,
    commands: Vec<CommandModel>,
}

impl DocModel {
    pub fn with_commands(mut self, commands: Vec<CommandModel>) -> Self {
        self.commands = commands;
        self
    }

    pub fn with_sections(mut self, sections: Vec<SectionModel>) -> Self {
        self.sections = sections;
        self
    }
}
