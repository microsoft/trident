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

// Republish
pub use errors::{SetsailError, SetsailErrorType};
pub use load::{load_kickstart_file, load_kickstart_string};

use log::{debug, info};

use trident_api::config::HostConfiguration;

use {parser::Parser, preprocess::PreprocessMode, translator::translate, types::KSLine};

/// Main parser struct
/// This is the outward facing interface to the parser
#[derive(Debug)]
pub struct KsTranslator {
    // Behavior flags
    /// Whether to process %ksappend lines
    flag_process_ksappend: bool,

    /// Whether missing files errors should be ignored
    flag_missing_ksappend_is_error: bool,

    /// Whether missing %include files should be ignored
    flag_include_fail_is_error: bool,

    /// Run %pre scripts
    flag_run_pre: bool,

    /// Whether to print errors and warnings as such or just print them as debug
    flag_verbose: bool,
}

impl KsTranslator {
    pub fn new() -> Self {
        Self {
            flag_process_ksappend: true,
            flag_missing_ksappend_is_error: true,
            flag_include_fail_is_error: true,
            flag_run_pre: false,
            flag_verbose: false,
        }
    }

    pub fn process_ksappend(mut self, process: bool) -> Self {
        self.flag_process_ksappend = process;
        self
    }

    pub fn error_on_missing_ksappend_file(mut self, error: bool) -> Self {
        self.flag_missing_ksappend_is_error = error;
        self
    }

    pub fn include_fail_is_error(mut self, error: bool) -> Self {
        self.flag_include_fail_is_error = error;
        self
    }

    pub fn run_pre_scripts(mut self, run: bool) -> Self {
        self.flag_run_pre = run;
        self
    }

    pub fn translate(self, raw_lines: Vec<KSLine>) -> Result<HostConfiguration, Vec<SetsailError>> {
        // * * * * * * PARSING * * * * * *

        // Run preprocess, update lines and errors
        let preprocess_mode = if !self.flag_process_ksappend {
            PreprocessMode::Skip
        } else if self.flag_missing_ksappend_is_error {
            PreprocessMode::Process
        } else {
            PreprocessMode::ProcessNoError
        };

        let (lines, mut errors) = preprocess::preprocess(raw_lines, preprocess_mode);

        if self.flag_run_pre {
            debug!("Starting parser first pass");
            let mut parser = Parser::new_first_pass();
            parser.verbose_errors(self.flag_verbose);
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
        parser.include_fail_is_error(self.flag_include_fail_is_error);
        parser.verbose_errors(self.flag_verbose);
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
