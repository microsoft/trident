use std::path::Path;

use super::types::{KSLine, KSLineSource};

// function to read file and return array of strings
fn load_to_lines(path: &Path) -> Result<Vec<String>, std::io::Error> {
    let contents = std::fs::read_to_string(path)?;
    Ok(contents.lines().map(|f| f.to_string()).collect())
}

/// Function to convert a list of strings into a list of KSLine
/// It is used to read the contents of a KS file into a list of KSLine
/// The goal is to save each line with its source for error reporting
/// and to preserve the raw string for parsing.
fn lines_to_kslines(lines: Vec<String>, source: KSLineSource) -> Vec<KSLine> {
    lines
        .into_iter()
        .enumerate()
        .map(|(i, s)| KSLine {
            lineno: i + 1,
            source: source.clone(),
            raw: s,
        })
        .collect()
}

pub fn load_to_kslines(path: &Path, source: KSLineSource) -> Result<Vec<KSLine>, std::io::Error> {
    Ok(lines_to_kslines(load_to_lines(path)?, source))
}

pub fn load_kickstart_file(filename: &Path) -> Result<Vec<KSLine>, std::io::Error> {
    load_to_kslines(filename, KSLineSource::File(filename.to_owned()))
}

pub fn load_kickstart_string(contents: &str) -> Vec<KSLine> {
    lines_to_kslines(
        contents.lines().map(|f| f.to_string()).collect(),
        KSLineSource::InputString,
    )
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use indoc::indoc;
    use tempfile::NamedTempFile;

    use super::*;

    const TEST_FILE: &str = indoc! {r#"
        this is a test

        # with blank lines
        # and comments

        %ksappend /tmp/ksappend

        # and some other things that look like
        # kickstart
        part / --fstype ext4 --size 1 --grow
        part swap --size 1024
        part /boot --fstype ext4 --size 256
        part /var --fstype ext4 --size 1024 --grow

        %include /tmp/include
        %include /tmp/include2
    "#};

    #[test]
    fn test_load_string() {
        let processed = load_kickstart_string(TEST_FILE);

        assert_eq!(
            processed.len(),
            TEST_FILE.lines().count(),
            "Check all lines were correctly loaded."
        );

        // Assert all lines have the right source
        processed.iter().for_each(|l| {
            assert!(matches!(l.source, KSLineSource::InputString));
        });
    }

    #[test]
    fn test_load_file() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(TEST_FILE.as_bytes()).unwrap();
        file.flush().unwrap();

        let processed = load_kickstart_file(file.path()).unwrap();

        assert_eq!(
            processed.len(),
            TEST_FILE.lines().count(),
            "Check all lines were correctly loaded."
        );

        // Assert all lines have the right source
        processed.iter().for_each(|l| match &l.source {
            KSLineSource::File(f) => assert_eq!(f, file.path()),
            _ => panic!("Wrong source"),
        });
    }
}
