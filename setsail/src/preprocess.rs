use log::debug;

use super::errors::SetsailError;
use super::load::load_to_kslines;
use super::types::{KSLine, KSLineSource};

/// How to process %ksappend lines
#[derive(Debug, Clone, Copy)]
pub(crate) enum PreprocessMode {
    /// Do not process %ksappend lines
    Skip,
    /// Process %ksappend lines
    Process,
    /// Process %ksappend lines, but do not report missing files as errors
    ProcessNoError,
}

impl PreprocessMode {
    fn is_skip(self) -> bool {
        match self {
            Self::Skip => true,
            Self::Process | Self::ProcessNoError => false,
        }
    }

    fn missing_is_error(self) -> bool {
        match self {
            Self::Skip | Self::Process => true,
            Self::ProcessNoError => false,
        }
    }
}

pub(crate) fn preprocess(
    lines: Vec<KSLine>,
    mode: PreprocessMode,
) -> (Vec<KSLine>, Vec<SetsailError>) {
    let mut pre_processed: Vec<KSLine> = Vec::new();
    let mut errors: Vec<SetsailError> = Vec::new();
    for line in lines.into_iter() {
        // Clone the raw string so we don't borrow the line struct and we can move it
        let raw_line = line.raw.clone();
        let mut words = raw_line.split_whitespace();

        // Check the first word
        if let Some("%ksappend") = words.next() {
            if let Some(path) = words.next() {
                // Check that there are no more arguments
                if words.next().is_some() {
                    debug!("Too many arguments to %ksappend");
                    errors.push(SetsailError::new_ksappend(
                        line,
                        "Too many arguments".to_string(),
                    ));
                    continue;
                }

                if mode.is_skip() {
                    debug!("Skipping %ksappend: {}", path);
                    continue;
                }

                debug!("Processing &ksappend: {}", path);
                match load_to_kslines(path, KSLineSource::new_ksappend(path.to_string(), &line)) {
                    Ok(kslines) => {
                        debug!("Loaded {} lines from {}", kslines.len(), path);
                        pre_processed.extend(kslines);
                    }
                    Err(e) => {
                        debug!("Failed to load {}: {}", path, e);
                        if mode.missing_is_error() {
                            errors.push(SetsailError::new_ksappend(
                                line,
                                format!("Failed to load {}: {}", path, e),
                            ));
                        }
                    }
                }
            } else {
                debug!("Missing file path");
                errors.push(SetsailError::new_ksappend(
                    line,
                    "Missing file path".to_string(),
                ));
            }
        } else {
            // This is not a %ksappend line, so just pass it through
            // Preserve empty lines and comments because they can be part of a section
            pre_processed.push(line);
        }
    }

    (pre_processed, errors)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use indoc::indoc;
    use tempfile::NamedTempFile;

    use super::*;
    use crate::load_kickstart_string;

    #[test]
    fn test_passthrough() {
        let src = load_kickstart_string(indoc!(
            r#"this
            shouldn't trigger
            # the

            preprocessor
            %"#,
        ));
        let count = src.len();

        let (processed, errors) = preprocess(src, PreprocessMode::Process);
        assert_eq!(errors.len(), 0, "Assert no errors");
        assert_eq!(processed.len(), count, "Check for correct passthrough");
    }
    #[test]
    fn test_skip() {
        let src = load_kickstart_string(indoc!(
            r#"
            hello %ksappend <- this should be ignored
            # The next line should disappear
            %ksappend /tmp/ksappend
            "#,
        ));
        let count = src.len();

        let (processed, errors) = preprocess(src, PreprocessMode::Skip);

        assert_eq!(errors.len(), 0, "Assert no errors");
        assert_eq!(processed.len(), count - 1, "Check %ksappend was ignored");
        assert_eq!(
            0,
            processed
                .iter()
                .filter(|l| l.raw.trim_start().starts_with("%ksappend"))
                .count(),
            "Check %ksappend was removed"
        );
    }

    #[test]
    fn test_malformed() {
        let src = load_kickstart_string(indoc!(
            r#"
            # Without a file, a ksappend is incomplete!
            %ksappend 
            "#,
        ));
        let count = src.len();

        let (processed, errors) = preprocess(src, PreprocessMode::Process);

        assert_eq!(errors.len(), 1, "Assert one error");
        assert_eq!(
            processed.len(),
            count - 1,
            "Check %ksappend line was removed"
        );
        assert_eq!(
            0,
            processed
                .iter()
                .filter(|l| l.raw.trim_start().starts_with("%ksappend"))
                .count(),
            "Check %ksappend was removed"
        );
    }

    #[test]
    fn test_missing_file() {
        let src = load_kickstart_string(indoc!(
            r#"
            %ksappend /file/that/definitely/does/not/exist/or/at/least/I/hope
            "#,
        ));
        let count = src.len();

        let (processed, errors) = preprocess(src, PreprocessMode::Process);

        assert_eq!(errors.len(), 1, "Assert one error");
        assert_eq!(
            processed.len(),
            count - 1,
            "Check %ksappend line was removed"
        );
        assert_eq!(
            0,
            processed
                .iter()
                .filter(|l| l.raw.trim_start().starts_with("%ksappend"))
                .count(),
            "Check %ksappend was removed"
        );
    }

    #[test]
    fn test_ksappend() {
        let file = indoc! {r#"
            # This is a test file
            part something something
        "#};

        // Save that temporary file
        let mut tmpfile = NamedTempFile::new().unwrap();
        tmpfile.write_all(file.as_bytes()).unwrap();
        tmpfile.flush().unwrap();

        let src = load_kickstart_string(
            format!(
                indoc!(
                    r#"
                    # This file just has a %ksappend
                    %ksappend {}
                    "#,
                ),
                tmpfile.path().to_str().unwrap()
            )
            .as_str(),
        );
        let count = src.len();

        let (processed, errors) = preprocess(src, PreprocessMode::Process);

        assert_eq!(errors.len(), 0, "Assert no errors");
        assert_eq!(
            processed.len(),
            file.lines().count() + count - 1,
            "Check %ksappend was loaded"
        );
    }

    #[test]
    fn test_no_errors() {
        let src = load_kickstart_string(indoc!(
            r#"
            # This file just has a %ksappend
            %ksappend /tmp/ksappend
            %ksappend /some-file.ks
            "#,
        ));
        let count = src.len();

        let (processed, errors) = preprocess(src, PreprocessMode::ProcessNoError);

        assert_eq!(errors.len(), 0, "Assert no errors");
        assert_eq!(
            processed.len(),
            count - 2,
            "Check %ksappend was loaded but not removed"
        );
    }
}
