use std::{env, path::PathBuf};

use clap::{Command, CommandFactory};
use tera::{Context, Tera};

mod model;

use model::{CommandModel, DocModel};

#[derive(Debug, Default)]
pub struct DocBuilder {
    commands: Vec<Command>,
    tera: Tera,
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

    pub fn build(mut self) -> String {
        // Sort commands by name
        self.commands.sort_by(|a, b| a.get_name().cmp(b.get_name()));
        let model = self.make_model();
        // println!("{}", serde_json::to_string_pretty(&model).unwrap());
        self.tera
            .render(
                "docs.md.jinja2",
                &Context::from_serialize(&model).expect("Could not serialize data model"),
            )
            .expect("Failed to render template")
    }

    pub fn make_model(&mut self) -> DocModel {
        DocModel::new(self.commands.iter().map(CommandModel::from).collect())
    }
}
