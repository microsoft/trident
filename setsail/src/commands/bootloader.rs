use clap::{Parser, ValueEnum};

use super::HandleCommand;

#[derive(Parser, Debug)]
pub struct Bootloader {
    #[arg(long)]
    append: Option<String>,

    #[arg(long)]
    location: BootloaderLocation,
}

#[derive(ValueEnum, Debug, Clone, Default)]
pub enum BootloaderLocation {
    #[default]
    Mbr,
    Partition,
    None,
    Boot,
}

impl HandleCommand for Bootloader {
    // fn handle(self, line: KSLine, data: &mut ParsedData) -> Result<(), ParserError> {
    //     debug!("Handling {} command: {:?}", std::any::type_name::<Self>() ,self);
    //     todo!();
    // }
}
