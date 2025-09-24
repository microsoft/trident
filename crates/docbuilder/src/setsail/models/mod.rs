pub(crate) mod arg;
pub(crate) mod command;
pub(crate) mod section;

use command::CommandModel;
use section::SectionModel;
use serde::Serialize;

#[derive(Default, Debug, Serialize)]
pub(crate) struct DocModel {
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
