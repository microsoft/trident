use clap::Parser;

use super::CommandHandler;

#[derive(Parser, Debug)]
pub struct Timezone {
    #[arg(long, visible_alias = "isUtc")]
    utc: bool,
    #[arg(long, group = "ntp")]
    nontp: bool,
    #[arg(long, group = "ntp", value_delimiter = ',')]
    ntpservers: Vec<String>,
    timezone: Option<String>,
}

impl CommandHandler for Timezone {
    // fn handle(self, line: KSLine, data: &mut ParsedData) -> Result<(), ParserError> {
    //     debug!("Handling {} command: {:?}", std::any::type_name::<Self>() ,self);
    //     todo!();
    // }
}
