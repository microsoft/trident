pub mod commands;
mod data;
mod errors;
mod handlers;
mod load;
mod parser;
mod preprocess;
mod sections;
mod translator;
mod types;

// use super::HostConfig;

// Republish
pub use errors::{SetsailError, SetsailErrorType};
pub use load::{load_kickstart_file, load_kickstart_string};

use log::{debug, info};

use trident_api::config::HostConfiguration;

use {parser::Parser, translator::translate, types::KSLine};

/// Main parser struct
/// This is the outward facing interface to the parser
#[derive(Debug)]
pub struct KsTranslator {
    // Behavior flags
    /// Whether to process %ksappend lines
    process_ksappend: bool,

    /// Whether missing files errors should be ignored
    error_on_missing_ksappend: bool,

    /// Run %pre scripts
    run_pre: bool,

    /// Whether to print errors and warnings as such or just print them as debug
    verbose: bool,
}

impl KsTranslator {
    pub fn new() -> Self {
        Self {
            process_ksappend: true,
            error_on_missing_ksappend: true,
            run_pre: false,
            verbose: false,
        }
    }

    pub fn process_ksappend(&mut self, process: bool) -> &mut Self {
        self.process_ksappend = process;
        self
    }

    pub fn error_on_missing_ksappend_file(&mut self, error: bool) -> &mut Self {
        self.error_on_missing_ksappend = error;
        self
    }

    pub fn run_pre_scripts(&mut self, run: bool) -> &mut Self {
        self.run_pre = run;
        self
    }

    pub fn translate(self, raw_lines: Vec<KSLine>) -> Result<HostConfiguration, Vec<SetsailError>> {
        // * * * * * * PARSING * * * * * *

        // Run preprocess, update lines and errors
        let (lines, mut errors) = preprocess::preprocess(raw_lines, self.process_ksappend);

        if self.run_pre {
            debug!("Starting parser first pass");
            let mut parser = Parser::new_first_pass();
            parser.verbose_errors(self.verbose);
            parser.parse(&lines);
            errors.extend(parser.consume_errors());

            info!("Found {} scripts", parser.data.scripts.len());

            // Run every script, record any errors
            for script in parser.data.scripts.iter() {
                if let Err(e) = script.run() {
                    errors.push(e);
                }
            }
        }

        // Do 2nd parser pass
        debug!("Starting parser second pass");
        let mut parser = Parser::new();
        parser.verbose_errors(self.verbose);
        parser.parse(&lines);
        errors.extend(parser.consume_errors());

        info!("Found {} scripts", parser.data.scripts.len());

        // * * * * * * TRANSLATING * * * * * *

        // Always attempt to translate to accumulate as many errors as possible.
        // Return the HostConfiguration if there are no errors at all.
        // Return a Vec of errors if there are any from any stage to inform the
        // caller that something went wrong and installation should NOT proceed.
        // TODO: allow setting warnings to be NOT fatal
        match translate(parser.data) {
            Ok(hc) => {
                if errors.is_empty() {
                    Ok(hc)
                } else {
                    Err(errors)
                }
            }
            Err(e) => {
                errors.extend(e);
                Err(errors)
            }
        }
    }
}

impl Default for KsTranslator {
    fn default() -> Self {
        Self::new()
    }
}
