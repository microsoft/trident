use log::debug;

use super::errors::SetsailError;
use super::parser::ParsedData;
use super::types::KSLine;

pub trait SectionHandler {
    fn opener(&self) -> String;
    fn handle(
        &self,
        parser: &mut ParsedData,
        line: KSLine,
        tokens: Vec<String>,
        body: Vec<String>,
    ) -> Result<(), SetsailError>;
}

/// Handler for sections we want to recognize but do nothing with
/// Basically useful to avoid errors
pub struct TrashHandler {
    opener: String,
}

impl TrashHandler {
    pub fn new_boxed(opener: &str) -> Box<dyn SectionHandler> {
        Box::new(Self {
            opener: String::from(opener),
        })
    }
}

impl SectionHandler for TrashHandler {
    fn opener(&self) -> String {
        self.opener.clone()
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

/// Handler for sections we want to recognize but do nothing with
/// Basically useful to avoid errors
pub struct UnsuportedSectionHandler {
    opener: String,
}

impl UnsuportedSectionHandler {
    pub fn new_boxed(opener: &str) -> Box<dyn SectionHandler> {
        Box::new(Self {
            opener: String::from(opener),
        })
    }
}

impl SectionHandler for UnsuportedSectionHandler {
    fn opener(&self) -> String {
        self.opener.clone()
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
