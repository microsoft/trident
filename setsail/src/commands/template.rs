use clap::{Parser, ValueEnum};

use crate::setsail::{parser::ParsedData, types::KSLine, ParserError};

use super::CommandHandler;

#[derive(Parser, Debug)]
pub struct Template {}

impl CommandHandler for Template {
    // fn handle(self, line: KSLine, data: &mut ParsedData) -> Result<(), ParserError> {
    //     debug!("Handling {} command: {:?}", std::any::type_name::<Self>() ,self);
    //     todo!();
    // }
}
