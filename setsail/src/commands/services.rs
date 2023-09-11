use clap::Parser;

use super::CommandHandler;

#[derive(Parser, Debug)]
pub struct Services {
    #[arg(long, value_delimiter = ',')]
    enabled: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    disabled: Vec<String>,
}

impl CommandHandler for Services {
    //     fn handle(self, line: KSLine, data: &mut ParsedData) -> Result<(), ParserError> {
    //         debug!("Handling {} command: {:?}", std::any::type_name::<Self>() ,self);
    //         todo!();
    //     }
}
