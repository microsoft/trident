use std::path::{Path, PathBuf};

use serde::Serialize;

/// Represents a line in a kickstart file
#[derive(Serialize, Debug, Clone, Default)]
pub struct KSLine {
    /// Origin of this kickstart line
    pub source: KSLineSource,
    /// Line number in the source
    pub lineno: usize,
    /// The parsed instruction
    pub raw: String,
}

impl KSLine {
    pub fn new(source: KSLineSource, lineno: usize, raw: String) -> Self {
        Self {
            source,
            lineno,
            raw,
        }
    }

    pub fn get_id(&self) -> String {
        match self.source.get_filename() {
            Some(filename) => format!("{}:{}", filename.display(), self.lineno),
            None => format!("unknown:{}", self.lineno),
        }
    }
}

impl std::fmt::Display for KSLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{} \"{}\"", self.source, self.lineno, self.raw)
    }
}

/// Represents the source of a line in a kickstart file
#[derive(Serialize, Debug, Clone, Default)]
pub enum KSLineSource {
    #[default]
    Unknown,
    InputString,
    File(PathBuf),
    KsAppend(PathBuf, Box<KSLine>),
    Include(PathBuf, Box<KSLine>),
}

impl KSLineSource {
    pub fn new_ksappend(file: PathBuf, line: &KSLine) -> Self {
        Self::KsAppend(file, Box::new(line.clone()))
    }

    pub fn new_include(file: PathBuf, line: &KSLine) -> Self {
        Self::Include(file, Box::new(line.clone()))
    }

    pub fn get_filename(&self) -> Option<&Path> {
        match self {
            KSLineSource::File(filename) => Some(filename),
            KSLineSource::KsAppend(file, _) => Some(file),
            KSLineSource::Include(file, _) => Some(file),
            KSLineSource::InputString | KSLineSource::Unknown => None,
        }
    }
}

impl std::fmt::Display for KSLineSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KSLineSource::Unknown => write!(f, "Unknown"),
            KSLineSource::InputString => write!(f, "InputString"),
            KSLineSource::File(filename) => write!(f, "{}", filename.display()),
            KSLineSource::KsAppend(file, line) => {
                write!(
                    f,
                    "{}:{}>ksappend({})",
                    line.source,
                    line.lineno,
                    file.display()
                )
            }
            KSLineSource::Include(file, line) => {
                write!(
                    f,
                    "{}:{}>include({})",
                    line.source,
                    line.lineno,
                    file.display()
                )
            }
        }
    }
}
