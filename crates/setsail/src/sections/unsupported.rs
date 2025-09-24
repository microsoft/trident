use log::debug;

use crate::{data::ParsedData, types::KSLine, SetsailError};

use super::SectionHandler;

/// Handler for sections we want to recognize but do nothing with
/// Basically useful to avoid errors
#[derive(Debug)]
pub struct UnsuportedSectionHandler {
    opener: &'static str,
}

impl UnsuportedSectionHandler {
    pub fn new(opener: &'static str) -> Self {
        Self { opener }
    }
}

impl SectionHandler for UnsuportedSectionHandler {
    fn opener(&self) -> &'static str {
        self.opener
    }

    fn handle(
        &self,
        _: &mut ParsedData,
        header: KSLine,
        tokens: Vec<String>,
        body: Vec<String>,
    ) -> Result<(), SetsailError> {
        debug!("Unsupported section {} ({} lines)", header, body.len());
        Err(SetsailError::new_unsupported_section(
            header,
            tokens[0].clone(),
        ))
    }
}
