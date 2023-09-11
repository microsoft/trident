use clap::Parser;

use super::CommandHandler;

#[derive(Parser, Debug)]
pub struct LiveImg {
    #[arg(long, required = true)]
    url: String,

    #[arg(long)]
    proxy: Option<String>,

    #[arg(long)]
    noverifyssl: bool,

    #[arg(long)]
    checksum: Option<String>,
}

impl CommandHandler for LiveImg {
    // fn handle(self, line: KSLine, data: &mut ParsedData) -> Result<(), ParserError> {
    //     debug!("Handling {} command: {:?}", std::any::type_name::<Self>() ,self);
    //     todo!();
    // }
}
