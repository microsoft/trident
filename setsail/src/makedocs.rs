use clap::Parser;

mod docbuilder;
use docbuilder::DocBuilder;

// ADD NEW COMMANDS HERE
use setsail::{
    commands::{network::Network, partition::Partition, rootpw::Rootpw, user::User},
    sections::SectionManager,
};

#[derive(Debug, clap::Parser)]
struct Args {
    #[clap(short = 'o')]
    output_dir: String,
}

fn main() {
    let args = Args::parse();

    // USE `with_command` TO ADD NEW COMMANDS TO THE DOC
    let doc = DocBuilder::new()
        .with_command::<Network>()
        .with_command::<Partition>()
        .with_command::<Rootpw>()
        .with_command::<User>()
        .with_sections(SectionManager::default())
        .build();

    std::fs::write(
        format!(
            "{}/setsail-{}.md",
            args.output_dir,
            env!("CARGO_PKG_VERSION")
        ),
        doc,
    )
    .expect("Failed to write output");
}
