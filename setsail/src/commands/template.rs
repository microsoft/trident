use clap::{Parser, ValueEnum};

use crate::{data::ParsedData, types::KSLine, SetsailError};

use super::CommandHandler;

#[derive(Parser, Debug)]
pub struct Template {}

impl CommandHandler for Template {
    // fn handle(self, line: KSLine, data: &mut ParsedData) -> Result<(), SetsailError> {
    //     debug!("Handling {} command: {:?}", std::any::type_name::<Self>() ,self);
    //     todo!();
    // }
}
