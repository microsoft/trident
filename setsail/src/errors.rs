use clap::error::ErrorKind;
use log::{debug, error, warn};
use serde::Serialize;

use super::types::KSLine;

/// External facing representation of a parser error
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct SetsailError {
    pub line: KSLine,
    pub error: SetsailErrorType,
}

/// External facing representation of a parser error type
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub enum SetsailErrorType {
    KSAppendError(String),
    MismatechedQuotes,
    SyntaxError(String),
    IncludeError(String),
    UnknownSection(String),
    UnexpectedEndOfFile(String),
    UnknownCommand(String),
    UnsupportedCommand(String),
    DisallowedCommand(String),
    UnsuportedSection(String),
    UnsuportedFeature(String),
    SemanticError(String),
    SemanticWarning(String),
    PreScriptFailed(String),
    TranslationError(String),
}

impl SetsailError {
    pub fn log(&self, verbose: bool) {
        if !verbose {
            debug!("{}", self);
        } else if self.is_warning() {
            warn!("{}", self);
        } else {
            error!("{}", self);
        }
    }

    pub fn is_warning(&self) -> bool {
        matches!(self.error, SetsailErrorType::SemanticWarning(_))
    }

    pub fn new_mismatched_quotes(line: KSLine) -> Self {
        Self {
            line,
            error: SetsailErrorType::MismatechedQuotes,
        }
    }

    pub fn new_syntax(line: KSLine, error: String) -> Self {
        Self {
            line,
            error: SetsailErrorType::SyntaxError(error),
        }
    }

    pub fn new_ksappend(line: KSLine, error: String) -> Self {
        Self {
            line,
            error: SetsailErrorType::KSAppendError(error),
        }
    }

    pub fn new_include(line: KSLine, error: String) -> Self {
        Self {
            line,
            error: SetsailErrorType::IncludeError(error),
        }
    }

    pub fn new_unknown_section(line: KSLine, error: String) -> Self {
        Self {
            line,
            error: SetsailErrorType::UnknownSection(error),
        }
    }

    pub fn new_unexpected_eof(line: KSLine, error: String) -> Self {
        Self {
            line,
            error: SetsailErrorType::UnexpectedEndOfFile(error),
        }
    }

    pub fn new_unsupported_section(line: KSLine, section: String) -> Self {
        Self {
            line,
            error: SetsailErrorType::UnsuportedSection(section),
        }
    }

    pub fn new_unsupported_command(line: KSLine, command: String) -> Self {
        Self {
            line,
            error: SetsailErrorType::UnsupportedCommand(command),
        }
    }

    pub fn new_unknown_command(line: KSLine, command: String) -> Self {
        Self {
            line,
            error: SetsailErrorType::UnknownCommand(command),
        }
    }

    pub fn new_disallowed_command(line: KSLine, command: String) -> Self {
        Self {
            line,
            error: SetsailErrorType::DisallowedCommand(command),
        }
    }

    pub fn new_unsupported_feature(line: KSLine, feature: String) -> Self {
        Self {
            line,
            error: SetsailErrorType::UnsuportedFeature(feature),
        }
    }

    pub fn new_semantic(line: KSLine, error: String) -> Self {
        Self {
            line,
            error: SetsailErrorType::SemanticError(error),
        }
    }

    pub fn new_sem_warn(line: KSLine, error: String) -> Self {
        Self {
            line,
            error: SetsailErrorType::SemanticWarning(error),
        }
    }

    pub fn new_pre_script_error(line: KSLine, error: String) -> Self {
        Self {
            line,
            error: SetsailErrorType::PreScriptFailed(error),
        }
    }

    pub fn new_translation(line: KSLine, error: String) -> Self {
        Self {
            line,
            error: SetsailErrorType::TranslationError(error),
        }
    }

    pub fn from_clap(line: KSLine, mut error: clap::Error) -> Self {
        // Suppress usage info
        error.insert(
            clap::error::ContextKind::Usage,
            clap::error::ContextValue::None,
        );

        // Suppress help info
        error.insert(
            clap::error::ContextKind::Suggested,
            clap::error::ContextValue::None,
        );

        // Get only the first line of the error
        let string = error
            .to_string()
            .replace("For more information, try '--help'.", "")
            .trim()
            .to_owned();

        Self {
            line,
            error: match error.kind() {
                ErrorKind::InvalidSubcommand => SetsailErrorType::UnknownCommand(string),
                _ => SetsailErrorType::SyntaxError(string),
            },
        }
    }
}

impl std::fmt::Display for SetsailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.error {
            SetsailErrorType::KSAppendError(e) => write!(f, "%ksappend error: {e}"),
            SetsailErrorType::MismatechedQuotes => write!(f, "Mismateched quotes"),
            SetsailErrorType::SyntaxError(e) => write!(f, "Syntax error: {e}"),
            SetsailErrorType::IncludeError(e) => write!(f, "Include error: {e}"),
            SetsailErrorType::UnknownSection(e) => write!(f, "Unknown section: {e}"),
            SetsailErrorType::UnexpectedEndOfFile(e) => write!(f, "Unexpected end of file: {e}"),
            SetsailErrorType::UnknownCommand(e) => write!(f, "Unrecognized command: \"{e}\""),
            SetsailErrorType::UnsupportedCommand(e) => write!(f, "Unsupported command: \"{e}\""),
            SetsailErrorType::DisallowedCommand(e) => write!(f, "Disallowed command: \"{e}\""),
            SetsailErrorType::UnsuportedSection(e) => write!(f, "Unsuported section: \"{e}\""),
            SetsailErrorType::SemanticError(e) => write!(f, "Semantic error: {e}"),
            SetsailErrorType::SemanticWarning(e) => write!(f, "Semantic warning: {e}"),
            SetsailErrorType::PreScriptFailed(e) => write!(f, "%pre script failed: {e}"),
            SetsailErrorType::TranslationError(e) => write!(f, "Translation error: {}", e),
            SetsailErrorType::UnsuportedFeature(s) => write!(f, "Unsuported feature: \"{}\"", s),
        }?;
        write!(
            f,
            " at {}:{}\n    {}",
            self.line.source, self.line.lineno, self.line.raw
        )
    }
}

/// A useful trait to convert any arbitrary Result into a Result<_, ParserError>
pub trait ToResultSetsailError<T> {
    fn to_result_parser_error(self, line: &KSLine) -> Result<T, SetsailError>;
}

impl<T> ToResultSetsailError<T> for Result<T, clap::Error> {
    fn to_result_parser_error(self, line: &KSLine) -> Result<T, SetsailError> {
        self.map_err(|e| SetsailError::from_clap(line.clone(), e))
    }
}

impl<T> ToResultSetsailError<T> for Result<T, shellwords::MismatchedQuotes> {
    fn to_result_parser_error(self, line: &KSLine) -> Result<T, SetsailError> {
        self.map_err(|_| SetsailError::new_mismatched_quotes(line.clone()))
    }
}

#[derive(Debug)]
pub struct SetsailErrorList(pub Vec<SetsailError>);
impl std::error::Error for SetsailErrorList {}
impl std::fmt::Display for SetsailErrorList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for error in &self.0 {
            writeln!(f, "{error}")?;
        }
        Ok(())
    }
}
