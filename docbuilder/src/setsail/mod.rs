mod docbuilder;
mod models;

use docbuilder::DocBuilder;

// ADD NEW COMMANDS HERE
use setsail::{
    commands::{network::Network, partition::Partition, rootpw::Rootpw, user::User},
    sections::SectionManager,
};

pub(crate) fn build_docs() -> String {
    // USE `with_command` TO ADD NEW COMMANDS TO THE DOC
    DocBuilder::new()
        .with_command::<Network>()
        .with_command::<Partition>()
        .with_command::<Rootpw>()
        .with_command::<User>()
        .with_sections(SectionManager::default())
        .build()
}
