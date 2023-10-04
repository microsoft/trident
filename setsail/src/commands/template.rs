use clap::{Parser, ValueEnum};

use crate::{data::ParsedData, types::KSLine, SetsailError};

use super::CommandHandler;

#[derive(Parser, Debug)]
#[command(name = "template", aliases = &["temp"])]
pub struct Template {
    // Internal
    #[clap(skip)]
    pub line: KSLine,

    /// The docstring gets transformed into the help message
    #[arg(long)]
    pub name: String,
}

impl CommandHandler for Template {
    // fn handle(self, line: KSLine, data: &mut ParsedData) -> Result<(), SetsailError> {
    //     debug!("Handling {} command: {:?}", std::any::type_name::<Self>() ,self);
    //     todo!();
    // }
}
