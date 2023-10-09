use log::debug;
use std::collections::{HashMap, VecDeque};
use std::path;

use crate::commands::CommandHandler;
use crate::data::ParsedData;
use crate::errors::ToResultSetsailError;
use crate::handlers::{SectionHandler, TrashHandler, UnsuportedSectionHandler};
use crate::load;
use crate::sections::script::{ScriptHandler, ScriptType};
use crate::types::KSLine;
use crate::types::KSLineSource;
use crate::SetsailError;

pub struct Parser {
    // parsed data
    pub data: ParsedData,

    // Configuration flags
    flag_follow_include: bool,
    flag_parse_commands: bool,
    flag_include_fail_is_error: bool,
    flag_error_verbose: bool,

    // Inner state objects
    errors: Vec<SetsailError>,
    include_stack: Vec<path::PathBuf>,

    // Handlers
    section_handlers: HashMap<String, Box<dyn SectionHandler>>,
}

impl Parser {
    // Builder

    pub fn new() -> Self {
        let mut obj = Self::new_empty();
        obj.configure_default_handlers(false);
        obj
    }

    pub fn new_first_pass() -> Self {
        let mut obj = Self::new_empty();
        obj.configure_default_handlers(true);
        // Don't even bother parsing commands
        obj.parse_commands(false);
        // Don't follow includes, %pre scrips are not allowed on included files
        obj.follow_include(false);
        obj
    }

    pub fn new_empty() -> Self {
        Self {
            // parsed data
            data: ParsedData::default(),

            // configuration flags
            flag_follow_include: true,
            flag_parse_commands: true,
            flag_include_fail_is_error: true,
            flag_error_verbose: false,

            // state
            errors: Vec::new(),
            include_stack: Vec::new(),

            // handlers
            section_handlers: HashMap::new(),
        }
    }

    #[allow(dead_code)]
    pub fn follow_include(&mut self, process: bool) -> &mut Self {
        self.flag_follow_include = process;
        self
    }

    pub fn parse_commands(&mut self, parse: bool) -> &mut Self {
        self.flag_parse_commands = parse;
        self
    }

    #[allow(dead_code)]
    pub fn include_fail_is_error(&mut self, error: bool) -> &mut Self {
        self.flag_include_fail_is_error = error;
        self
    }

    pub fn verbose_errors(&mut self, verbose: bool) -> &mut Self {
        self.flag_error_verbose = verbose;
        self
    }

    // Public Parser functions

    pub fn parse(&mut self, lines: &[KSLine]) {
        // Turn this into a queue so we can easily pop off the front
        let mut buf: VecDeque<KSLine> = VecDeque::from_iter(lines.iter().cloned());
        while let Some(line) = buf.pop_front() {
            if let Err(e) = self.parse_line_internal(line, &mut buf) {
                self.push_error(e);
            }
        }
    }

    pub fn consume_errors(&mut self) -> Vec<SetsailError> {
        let mut buffer = Vec::new();
        std::mem::swap(&mut self.errors, &mut buffer);
        buffer
    }

    // Internal parser functions

    fn parse_line_internal(
        &mut self,
        line: KSLine,
        queue: &mut VecDeque<KSLine>,
    ) -> Result<(), SetsailError> {
        let tokens = shellwords::split(&line.raw).to_result_parser_error(&line)?;

        // Disregard empty lines and comments
        if tokens.is_empty() || tokens[0].starts_with('#') {
            return Ok(());
        }

        match tokens[0].as_str() {
            // We assume we have pre-proccessed the lines, but checking again just in case
            "%ksappend" => {
                debug!("Skipping %ksappend line: {}", line.raw);
            }

            // Handle include statements
            "%include" => match tokens.len() {
                2 => {
                    self.handle_include(line, &tokens[1]);
                }
                _ => {
                    return Err(SetsailError::new_syntax(
                        line,
                        "%include expects exactly 1 argument".into(),
                    ))
                }
            },

            // Invalid %end
            "%end" => {
                return Err(SetsailError::new_syntax(
                    line,
                    "%end without a matching section".into(),
                ))
            }

            // This is a section
            s if s.starts_with('%') => match self.section_handlers.get(s) {
                Some(handler) => {
                    let body = self.consume_section(&line, queue)?;
                    handler.handle(&mut self.data, line, tokens, body)?;
                }
                None => return Err(SetsailError::new_unknown_section(line, tokens[0].clone())),
            },

            // We assume this is a command
            _ if self.flag_parse_commands => {
                // Match aliases
                CommandHandler::new(tokens, line, &mut self.data).handle()?;
            }

            // Just log that we skipped something:
            _ => {
                debug!("Skipping line: {}", line);
            }
        };

        Ok(())
    }

    fn handle_include(&mut self, line: KSLine, filename: &str) {
        if !self.flag_follow_include {
            return;
        }

        debug!("Including file: {}", filename);

        let mut path = path::PathBuf::from(filename);

        // For relative paths, assume they are relative to the parent file, when possible
        // A root %include must always be an absolute path, but an included file may include
        // other files with paths relative to itself
        if path.is_relative() {
            if let Some(parent) = self.include_stack.last() {
                // Because we're saving filenames, we should always have a parent
                path = parent
                    .parent()
                    .expect("Failed to get parent path")
                    .join(path);
                debug!("Updated relative path to: {}", path.display());
            }
        }

        // Block recursive includes
        if self.include_stack.contains(&path) {
            self.push_error(SetsailError::new_include(
                line,
                format!("Recursive include: {}", path.display()),
            ));
            return;
        }

        // Path unfortunately doesn't convert safely to a string, so we have to do this
        let path_str = match path.to_str() {
            Some(path_str) => path_str,
            None => {
                self.push_error(SetsailError::new_include(
                    line,
                    "Failed to convert path to string".into(),
                ));
                return;
            }
        };

        match load::load_to_kslines(
            path_str,
            KSLineSource::new_include(path_str.to_string(), &line),
        ) {
            // We couldn't load the file :(
            Err(e) => {
                if matches!(e.kind(), std::io::ErrorKind::NotFound)
                    && !self.flag_include_fail_is_error
                {
                    debug!("Skipping missing file: {}", path.display());
                    return;
                }
                self.push_error(SetsailError::new_include(line, e.to_string()))
            }
            Ok(newlines) => {
                // Push the current file to the stack
                self.include_stack.push(path);

                // Parse the new file
                self.parse(&newlines);

                // Pop the current file from the stack
                self.include_stack.pop();
            }
        }
    }

    fn consume_section(
        &self,
        opening: &KSLine,
        queue: &mut VecDeque<KSLine>,
    ) -> Result<Vec<String>, SetsailError> {
        let mut body = Vec::new();
        loop {
            let line = queue.pop_front().ok_or(SetsailError::new_unexpected_eof(
                opening.clone(), // cloning because this does not belong to us
                "Section reached the end of the file".into(),
            ))?;

            match line.raw.split_whitespace().next() {
                // If we find the end of the section, we're done
                Some("%end") => {
                    break;
                }

                // If we find another section, we're done
                // We also want to re-queue this line so the parent can handle it
                Some(opener) if self.section_handlers.contains_key(opener) => {
                    // We need to clone the line because it is both the error and the start of the next section
                    queue.push_front(line.clone());
                    return Err(SetsailError::new_syntax(
                        line, // Cloning so we can return it to the queue
                        "Unexpected section".into(),
                    ));
                }

                // Anything else is just part of the body
                Some(_) | None => {
                    body.push(line.raw);
                }
            }
        }

        Ok(body)
    }

    // Handler management

    pub fn register_section_handler(&mut self, handler: Box<dyn SectionHandler>) {
        self.section_handlers.insert(handler.opener(), handler);
    }

    // default handlers

    fn configure_default_handlers(&mut self, first_pass: bool) {
        // Unsupported sections
        self.register_section_handler(UnsuportedSectionHandler::new_boxed("%anaconda"));
        self.register_section_handler(UnsuportedSectionHandler::new_boxed("%addon"));
        self.register_section_handler(UnsuportedSectionHandler::new_boxed("%onerror"));
        self.register_section_handler(UnsuportedSectionHandler::new_boxed("%packages"));

        // We always parse the %pre section
        self.register_section_handler(ScriptHandler::new_boxed(ScriptType::Pre));

        // If this is a first-pass parser, then we only care about %pre
        // We don't want to register any other handlers
        if first_pass {
            // Register known sections with a dummy handler
            self.ignore_section("%packages");
            self.ignore_section("%post");
            self.ignore_section("%pre-install");
            return;
        }

        // Sections
        self.register_section_handler(ScriptHandler::new_boxed(ScriptType::PreInstall));
        self.register_section_handler(ScriptHandler::new_boxed(ScriptType::Post));
    }

    pub fn ignore_section(&mut self, section: &str) {
        self.register_section_handler(TrashHandler::new_boxed(section));
    }

    // Internal utils

    fn push_error(&mut self, error: SetsailError) {
        error.log(self.flag_error_verbose);
        self.errors.push(error);
    }
}

#[cfg(test)]
mod tests {
    use std::include_str;
    use std::io::Write;

    use indoc::indoc;
    use tempfile::NamedTempFile;

    use super::*;
    use crate::{load_kickstart_string, SetsailErrorType};

    #[test]
    fn test_first_pass() {
        let lines = load_kickstart_string(include_str!("test_files/scripts.ks"));
        let mut parser = Parser::new_first_pass();
        parser.parse(&lines);
        assert!(parser.errors.is_empty(), "Assert no errors");
        assert!(
            !parser.data.scripts.is_empty(),
            "Assert we grabbed a script"
        );

        for script in parser.data.scripts.iter() {
            assert!(
                matches!(script.script_type, ScriptType::Pre),
                "Assert script is %pre"
            );
        }

        assert!(parser.data.partitions.is_empty(), "Assert no partitions");
        assert!(parser.data.users.is_empty(), "Assert no users");
        assert!(parser.data.root.is_none(), "Assert no root");
    }

    #[test]
    fn test_include_simple() {
        let file = indoc! {r#"
            # This is a test file
            part /boot --fstype=ext4 --size=1024
        "#};

        // Save that temporary file
        let mut tmpfile = NamedTempFile::new().unwrap();
        tmpfile.write_all(file.as_bytes()).unwrap();
        tmpfile.flush().unwrap();

        let src = load_kickstart_string(
            format!("%include {}", tmpfile.path().to_str().unwrap()).as_str(),
        );

        let mut parser = Parser::new();
        parser.parse(&src);
        assert!(parser.errors.is_empty(), "Assert no errors");
        assert_eq!(
            parser.data.partitions.len(),
            1,
            "Assert we grabbed ONE partition"
        );
    }

    #[test]
    fn test_include_nested() {
        // We want to test nested %include statements with relative paths.
        // When we include a file, we assume that the path is relative to the parent file, when possible.
        // We set up this "structure":
        // File(src):
        //     %include /tmp/file1
        // File(tmp/file1):
        //     %include file2
        // File(tmp/file2):
        //     <contents>

        // Create a file with just 1 partition
        let file2 = indoc! {r#"
            # This is a test file
            part /boot --fstype=ext4 --size=1024
        "#};

        // Save that temporary file
        let mut tmpfile2 = NamedTempFile::new().unwrap();
        tmpfile2.write_all(file2.as_bytes()).unwrap();
        tmpfile2.flush().unwrap();

        // Create a file with a _relative_ include to the second file, hence we only use its name
        let file1 = format!(
            "%include {}",
            tmpfile2.path().file_name().unwrap().to_str().unwrap()
        );

        // Save that temporary file
        let mut tmpfile1 = NamedTempFile::new().unwrap();
        tmpfile1.write_all(file1.as_bytes()).unwrap();
        tmpfile1.flush().unwrap();

        // Now create a file with an _absolute_ include to the second file
        let src = load_kickstart_string(
            format!("%include {}", tmpfile1.path().to_str().unwrap()).as_str(),
        );

        let mut parser = Parser::new();
        parser.parse(&src);
        assert!(parser.errors.is_empty(), "Assert no errors");
        assert_eq!(
            parser.data.partitions.len(),
            1,
            "Assert we grabbed ONE partition"
        );
    }

    #[test]
    fn test_include_recursive() {
        let mut tmpfile = NamedTempFile::new().unwrap();

        // This file will include itself
        let file = format!(
            indoc! {r#"
            # This is a test file
            %include {}
            part /boot --fstype=ext4 --size=1024
        "#},
            tmpfile.path().to_str().unwrap()
        );

        tmpfile.write_all(file.as_bytes()).unwrap();
        tmpfile.flush().unwrap();

        let src = load_kickstart_string(
            format!("%include {}", tmpfile.path().to_str().unwrap()).as_str(),
        );

        let mut parser = Parser::new();
        parser.parse(&src);
        assert_eq!(parser.errors.len(), 1, "Assert ONE error");
        assert!(
            matches!(parser.errors[0].error, SetsailErrorType::IncludeError(_)),
            "Assert error is IncludeError"
        );

        // Despite the error, we should recover gracefully
        // and correctly parse the part command
        assert_eq!(
            parser.data.partitions.len(),
            1,
            "Assert we grabbed ONE partitions"
        );
    }

    #[test]
    fn test_sections() {
        let lines = load_kickstart_string(include_str!("test_files/scripts.ks"));
        let mut parser = Parser::new();
        parser.parse(&lines);
        assert!(parser.errors.is_empty(), "Assert no errors");
        assert_eq!(
            parser.data.scripts.len(),
            4,
            "Assert we grabbed FOUR scripts"
        );

        assert!(matches!(
            parser.data.scripts[0].script_type,
            ScriptType::Pre
        ));

        assert!(matches!(
            parser.data.scripts[1].script_type,
            ScriptType::Pre
        ));

        assert!(matches!(
            parser.data.scripts[2].script_type,
            ScriptType::PreInstall
        ));

        assert!(matches!(
            parser.data.scripts[3].script_type,
            ScriptType::Post
        ));
    }

    #[test]
    fn test_section_eof() {
        // Section is missing an %end
        let lines = load_kickstart_string(indoc!(
            r#"
            %pre
            echo "Hello World"
            "#,
        ));

        let mut parser = Parser::new();
        parser.parse(&lines);
        assert_eq!(parser.errors.len(), 1, "Assert ONE error");
        assert!(
            matches!(
                parser.errors[0].error,
                SetsailErrorType::UnexpectedEndOfFile(_)
            ),
            "Assert error is UnexpectedEndOfFile"
        );
    }

    #[test]
    fn test_section_not_closed() {
        // Section is missing an %end and another section opens
        let lines = load_kickstart_string(indoc!(
            r#"
            %pre
            echo "Hello World"

            %post
            # do something
            %end
            "#,
        ));

        let mut parser = Parser::new();
        parser.parse(&lines);
        assert_eq!(parser.errors.len(), 1, "Assert ONE error");
        assert!(
            matches!(parser.errors[0].error, SetsailErrorType::SyntaxError(_)),
            "Assert error is SyntaxError"
        );

        assert_eq!(parser.data.scripts.len(), 1, "Assert we grabbed ONE script");
    }

    #[test]
    fn test_unrecognized_section() {
        // Section is missing an %end and another section opens
        let lines = load_kickstart_string(indoc!(
            r#"
            %pre
            echo "Hello World"
            %end

            %unrecognized
            # do something
            %end

            %post
            # do something
            %end
            "#,
        ));

        let mut parser = Parser::new();
        parser.parse(&lines);
        println!("{:?}", parser.errors);
        // Because the unknown section is not consumed, we get TWO errors
        // One for the %unrecognized, and one for the unmatched %end
        // Anything inside would generate an error too if it's not valid kickstart
        assert_eq!(parser.errors.len(), 2, "Assert TWO error");
        assert!(
            matches!(parser.errors[0].error, SetsailErrorType::UnknownSection(_)),
            "Assert error is UnknownSection"
        );

        assert!(
            matches!(parser.errors[1].error, SetsailErrorType::SyntaxError(_)),
            "Assert error is UnknownSection"
        );

        assert_eq!(parser.data.scripts.len(), 2, "Assert we grabbed TWO script");
    }

    #[test]
    fn test_disable_include() {
        let lines = load_kickstart_string(indoc!(
            r#"
            %include /some/file/1
            %include /some/file/2
            %include /some/file/3
            "#,
        ));

        let mut parser = Parser::new();
        parser.follow_include(false);
        parser.parse(&lines);
        assert!(parser.errors.is_empty(), "Assert no errors");
    }

    #[test]
    fn test_include_no_error() {
        let lines = load_kickstart_string(indoc!(
            r#"
            %include /some/file/1
            %include /some/file/2
            %include /some/file/3
            "#,
        ));

        let mut parser = Parser::new();
        parser.include_fail_is_error(false);
        parser.parse(&lines);
        assert!(parser.errors.is_empty(), "Assert no errors");
    }
}
