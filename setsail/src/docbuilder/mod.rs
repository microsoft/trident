use std::{env, path::PathBuf};

use clap::{Command, CommandFactory};
use tera::{Context, Tera};

use setsail::{sections::SectionHandler, sections::SectionManager};

mod models;

use models::{command::CommandModel, section::SectionModel, DocModel};

#[derive(Debug, Default)]
pub struct DocBuilder {
    commands: Vec<Command>,
    tera: Tera,
    sections: Vec<Box<dyn SectionHandler>>,
}

impl DocBuilder {
    pub fn new() -> Self {
        Self {
            tera: Tera::new(
                PathBuf::from(file!())
                    .parent()
                    .unwrap()
                    .join("templates/*")
                    .to_str()
                    .expect("Failed to get template path"),
            )
            .expect("Failed to load templates"),
            ..Self::default()
        }
    }

    pub fn with_command<T: CommandFactory>(mut self) -> Self {
        let cmd = T::command();

        // Assert it's not using the default name
        assert!(
            cmd.get_name() != env!("CARGO_PKG_NAME"),
            "Command from {} is using the default name",
            std::any::type_name::<T>()
        );

        self.commands.push(cmd);
        self
    }

    pub fn with_sections(mut self, section_manager: SectionManager) -> Self {
        self.sections = section_manager
            .into_sections()
            .into_values()
            .filter(|v| v.get_clap_command().is_some())
            .collect();
        self
    }

    pub fn build(mut self) -> String {
        // Sort commands by name
        self.commands.sort_by(|a, b| a.get_name().cmp(b.get_name()));
        self.sections.sort_by(|a, b| a.opener().cmp(b.opener()));
        let model = self.make_model();
        // println!("{}", serde_json::to_string_pretty(&model).unwrap());
        self.tera
            .render(
                "docs.md.jinja2",
                &Context::from_serialize(model).expect("Could not serialize data model"),
            )
            .expect("Failed to render template")
    }

    fn make_model(&mut self) -> DocModel {
        DocModel::default()
            .with_commands(self.commands.iter().map(CommandModel::from).collect())
            .with_sections(self.sections.iter().map(SectionModel::from).collect())
    }
}
