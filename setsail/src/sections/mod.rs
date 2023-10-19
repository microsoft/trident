use std::{collections::HashMap, fmt::Debug};

use clap::Command;

use crate::{data::ParsedData, types::KSLine, SetsailError};

pub mod script;

pub(crate) mod trash;
pub(crate) mod unsupported;

use script::{ScriptHandler, ScriptType};
use trash::TrashHandler;
use unsupported::UnsuportedSectionHandler;

pub struct SectionManager {
    handlers: HashMap<&'static str, Box<dyn SectionHandler>>,
}

impl Default for SectionManager {
    fn default() -> Self {
        Self {
            handlers: [
                // Supported Sections
                ScriptHandler::new(ScriptType::Pre).boxed(),
                ScriptHandler::new(ScriptType::PreInstall).boxed(),
                ScriptHandler::new(ScriptType::Post).boxed(),
                // Unsupported Sections
                UnsuportedSectionHandler::new("%addon").boxed(),
                UnsuportedSectionHandler::new("%anaconda").boxed(),
                UnsuportedSectionHandler::new("%onerror").boxed(),
                UnsuportedSectionHandler::new("%packages").boxed(),
            ]
            .into_iter()
            .map(|h| (h.opener(), h))
            .collect(),
        }
    }
}

impl SectionManager {
    /// Get a ref to the internal map of all known sections
    pub fn get_sections(&self) -> &HashMap<&'static str, Box<dyn SectionHandler>> {
        &self.handlers
    }

    /// Extract the underlying map of all known sections
    pub fn into_sections(self) -> HashMap<&'static str, Box<dyn SectionHandler>> {
        self.handlers
    }

    /// Ignore a specific section
    #[allow(dead_code)]
    pub(crate) fn ignore_section(&mut self, opener: &'static str) {
        self.handlers
            .insert(opener, TrashHandler::new(opener).boxed());
    }

    /// Ignore all sections except the ones specified
    pub(crate) fn ignore_all_except(&mut self, openers: &[&'static str]) {
        for handler in self.handlers.values_mut() {
            if !openers.contains(&handler.opener()) {
                *handler = TrashHandler::new(handler.opener()).boxed();
            }
        }
    }

    /// Get a handler for a specific section
    pub(crate) fn get_handler(&self, opener: &str) -> Option<&dyn SectionHandler> {
        self.handlers.get(opener).map(|h| h.as_ref())
    }

    /// Check if a section is known
    pub(crate) fn is_known_section(&self, opener: &str) -> bool {
        self.handlers.contains_key(opener)
    }
}

/// Trait to be implemented by all section handlers
pub trait SectionHandler: Debug {
    /// The verbatim opener for this section
    fn opener(&self) -> &'static str;

    /// Handle the section
    fn handle(
        &self,
        parser: &mut ParsedData,
        line: KSLine,
        tokens: Vec<String>,
        body: Vec<String>,
    ) -> Result<(), SetsailError>;

    /// Friendly name for this section
    fn name(&self) -> String {
        self.bare_opener()
    }

    /// Opener without the %
    fn bare_opener(&self) -> String {
        self.opener()
            .strip_prefix('%')
            .unwrap_or(self.opener())
            .to_string()
    }

    /// Box this handler
    fn boxed(self) -> Box<dyn SectionHandler>
    where
        Self: Sized + 'static,
    {
        Box::new(self)
    }

    /// Get the associated clap command for this section
    /// Only sections that provide a clap command will get
    /// automatically documented
    fn get_clap_command(&self) -> Option<Command> {
        None
    }
}
