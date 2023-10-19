use serde::Serialize;

use setsail::sections::SectionHandler;

use super::command::CommandModel;

#[derive(Debug, Serialize)]
pub(crate) struct SectionModel {
    pub name: String,
    pub opener: String,
    pub bare_opener: String,
    pub command: CommandModel,
}

impl SectionModel {}

impl From<&Box<dyn SectionHandler>> for SectionModel {
    fn from(handler: &Box<dyn SectionHandler>) -> Self {
        Self {
            name: handler.name(),
            opener: handler.opener().to_string(),
            bare_opener: handler.bare_opener(),
            command: CommandModel::from_cmd(
                &handler.get_clap_command().unwrap().name(handler.opener()),
                Some(include_str!("../templates/section_usage.template")),
            ),
        }
    }
}
