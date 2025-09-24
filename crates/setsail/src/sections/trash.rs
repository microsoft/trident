use log::debug;

use crate::{data::ParsedData, types::KSLine, SetsailError};

use super::SectionHandler;

/// Handler for sections we want to recognize but do nothing with
/// Basically useful to avoid errors
#[derive(Debug)]
pub struct TrashHandler {
    opener: &'static str,
}

impl TrashHandler {
    pub fn new(opener: &'static str) -> Self {
        Self { opener }
    }
}

impl SectionHandler for TrashHandler {
    fn opener(&self) -> &'static str {
        self.opener
    }

    fn handle(
        &self,
        _: &mut ParsedData,
        header: KSLine,
        _: Vec<String>,
        body: Vec<String>,
    ) -> Result<(), SetsailError> {
        debug!(
            "Trash Handler invoked for {} ({} lines)",
            header,
            body.len()
        );
        Ok(())
    }
}
