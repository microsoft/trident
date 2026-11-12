use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Error};

/// One kernel parameter, retaining its original name and position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    /// Parameter name as written in the command line.
    pub name: String,
    /// Parameter value; `None` is a bare flag, while `Some("")` is an empty value.
    pub value: Option<String>,
}

/// Reads and parses kernel parameters from `/proc/cmdline`.
///
/// Preserves order and repeated names, stopping at the standalone `--` separator.
/// Double quotes group whitespace; single quotes and backslashes are literal,
/// not shell escapes. Read errors and malformed parameters are returned.
pub fn read() -> Result<Vec<Parameter>, Error> {
    let contents = fs::read_to_string("/proc/cmdline").context("Failed to read /proc/cmdline")?;
    parse(&contents).context("Failed to parse /proc/cmdline")
}

/// Resolves `root=` parameters from `/proc/cmdline` to boot-source device paths.
///
/// Returns a deduplicated set of canonical, absolute paths. These identify root
/// devices, which may be partitions or device-mapper nodes, not their parent disks.
///
/// Examples, assuming UUID `abc` resolves to the canonical path `/dev/sda2`:
/// - `root=/dev/sda2` returns `{ /dev/sda2 }`.
/// - `root=live:UUID=abc` returns `{ /dev/sda2 }`.
/// - `root=/dev/ram0` or `root=live:https://example.test/installer.iso` returns `{}`.
///
/// Repeated roots are all included. RAM roots and unsupported source formats are
/// ignored; recognized device references that cannot be resolved return an error.
pub fn boot_sources() -> Result<HashSet<PathBuf>, Error> {
    boot_source_paths(&read()?)?
        .into_iter()
        .map(|path| {
            fs::canonicalize(&path)
                .with_context(|| format!("Cannot resolve boot source '{}'", path.display()))
        })
        .collect()
}

/// Parses double-quote-delimited kernel parameters without shell interpretation.
///
/// Rejects empty names, NUL bytes, and unterminated quotes. Tokens after `--`
/// belong to init rather than the kernel and are not parsed.
fn parse(contents: &str) -> Result<Vec<Parameter>, Error> {
    ensure!(
        !contents.contains('\0'),
        "Kernel command line contains a NUL byte"
    );
    let mut remaining = contents;
    let mut parameters = Vec::new();
    loop {
        remaining = remaining.trim_start_matches(is_kernel_whitespace);
        if remaining.is_empty() {
            return Ok(parameters);
        }
        let mut quoted = false;
        let end = remaining
            .char_indices()
            .find_map(|(index, character)| {
                if character == '"' {
                    quoted = !quoted;
                }
                (is_kernel_whitespace(character) && !quoted).then_some(index)
            })
            .unwrap_or(remaining.len());
        ensure!(!quoted, "Unterminated quote in kernel command line");
        let parameter = parse_parameter(&remaining[..end])?;
        if parameter.name == "--" && parameter.value.is_none() {
            return Ok(parameters);
        }
        parameters.push(parameter);
        remaining = &remaining[end..];
    }
}

/// Kernel ASCII whitespace also includes vertical tab, unlike Rust's ASCII helper.
fn is_kernel_whitespace(character: char) -> bool {
    character.is_ascii_whitespace() || character == '\x0b'
}

/// Separates the first `=` and removes enclosing argument or value quotes.
///
/// Embedded quotes are retained, matching kernel parameter syntax rather than
/// shell concatenation rules.
fn parse_parameter(token: &str) -> Result<Parameter, Error> {
    let token = if let Some(inner) = token.strip_prefix('"') {
        inner.strip_suffix('"').unwrap_or(inner)
    } else {
        token
    };
    let (name, value) = match token.split_once('=') {
        Some((name, value)) => {
            let value = if let Some(inner) = value.strip_prefix('"') {
                inner.strip_suffix('"').unwrap_or(inner)
            } else {
                value
            };
            (name, Some(value.to_owned()))
        }
        None => (token, None),
    };
    ensure!(!name.is_empty(), "Kernel parameter has an empty name");
    Ok(Parameter {
        name: name.to_owned(),
        value,
    })
}

/// Converts parsed root specifications to device references without filesystem I/O.
fn boot_source_paths(parameters: &[Parameter]) -> Result<HashSet<PathBuf>, Error> {
    let mut sources = HashSet::new();
    for root in parameters
        .iter()
        .filter(|parameter| parameter.name == "root")
        .filter_map(|parameter| parameter.value.as_deref())
    {
        let root = root.strip_prefix("live:").unwrap_or(root);
        ensure!(!root.is_empty(), "Root device specification is empty");
        if ["/dev/ram", "/dev/zram"].iter().any(|prefix| {
            root.strip_prefix(prefix).is_some_and(|suffix| {
                !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
            })
        }) {
            continue;
        }
        if root.starts_with("/dev/") {
            sources.insert(PathBuf::from(root));
            continue;
        }
        for (prefix, directory) in [
            ("UUID=", "by-uuid"),
            ("PARTUUID=", "by-partuuid"),
            ("LABEL=", "by-label"),
            ("CDLABEL=", "by-label"),
            ("PARTLABEL=", "by-partlabel"),
        ] {
            if let Some(value) = root.strip_prefix(prefix) {
                ensure!(
                    !value.is_empty() && value != "." && value != "..",
                    "Invalid boot-source identity '{root}'"
                );
                sources.insert(
                    Path::new("/dev/disk")
                        .join(directory)
                        .join(alias_name(value)),
                );
                break;
            }
        }
    }
    Ok(sources)
}

/// Encodes literal identity characters for udev aliases, preserving `\xNN` escapes.
fn alias_name(value: &str) -> String {
    let mut encoded = String::new();
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\\' {
            let mut lookahead = characters.clone();
            if lookahead.next() == Some('x')
                && lookahead.next().is_some_and(|c| c.is_ascii_hexdigit())
                && lookahead.next().is_some_and(|c| c.is_ascii_hexdigit())
            {
                encoded.push('\\');
                encoded.extend(characters.by_ref().take(3));
                continue;
            }
        }
        if character.is_ascii()
            && !character.is_ascii_alphanumeric()
            && !"#+-.:=@_".contains(character)
        {
            encoded.push_str(&format!("\\x{:02x}", u32::from(character)));
        } else {
            encoded.push(character);
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flags_values_duplicates_and_empty_values() {
        for (input, expected) in [
            ("", vec![]),
            (" \t\n\r\u{b}\u{c}", vec![]),
            ("quiet ro", vec![("quiet", None), ("ro", None)]),
            ("quiet empty=", vec![("quiet", None), ("empty", Some(""))]),
            (
                "console=tty0 console=ttyS0 root=UUID=a=b",
                vec![
                    ("console", Some("tty0")),
                    ("console", Some("ttyS0")),
                    ("root", Some("UUID=a=b")),
                ],
            ),
            (
                "\troot=/dev/sda\nrw\rdebug=yes ",
                vec![
                    ("root", Some("/dev/sda")),
                    ("rw", None),
                    ("debug", Some("yes")),
                ],
            ),
        ] {
            let actual = parse(input).unwrap();
            let actual = actual
                .iter()
                .map(|parameter| (parameter.name.as_str(), parameter.value.as_deref()))
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "{input:?}");
        }
    }

    #[test]
    fn preserves_kernel_quoting_without_shell_expansion() {
        for (input, name, value) in [
            (r#"root="LABEL=live disk""#, "root", Some("LABEL=live disk")),
            (r#""root=LABEL=live disk""#, "root", Some("LABEL=live disk")),
            (r#"empty="""#, "empty", Some("")),
            (r#""quiet""#, "quiet", None),
            (r#"value=ab" cd""#, "value", Some(r#"ab" cd""#)),
            (r"value=$HOME\name", "value", Some(r"$HOME\name")),
            (
                "label=\u{5b89}\u{88c5}\u{a0}\u{76d8}",
                "label",
                Some("\u{5b89}\u{88c5}\u{a0}\u{76d8}"),
            ),
            ("#not-a-comment", "#not-a-comment", None),
        ] {
            assert_eq!(
                parse(input).unwrap(),
                vec![Parameter {
                    name: name.into(),
                    value: value.map(str::to_owned),
                }],
                "{input:?}"
            );
        }
        assert_eq!(
            parse("value='two words'").unwrap(),
            vec![
                Parameter {
                    name: "value".into(),
                    value: Some("'two".into())
                },
                Parameter {
                    name: "words'".into(),
                    value: None
                },
            ]
        );
    }

    #[test]
    fn stops_at_init_arguments_but_not_separator_values() {
        for input in [
            "root=/dev/sda -- root=/dev/sdb",
            r#"root=/dev/sda "--" broken="unterminated"#,
        ] {
            assert_eq!(parse(input).unwrap(), parse("root=/dev/sda").unwrap());
        }
        assert_eq!(parse("value=--").unwrap()[0].value.as_deref(), Some("--"));
    }

    #[test]
    fn rejects_malformed_parameters() {
        for input in [
            "=value",
            r#""""#,
            r#"root="unterminated"#,
            "quiet\0root=/dev/sda",
        ] {
            assert!(parse(input).is_err(), "{input:?}");
        }
    }

    #[test]
    fn boot_sources_cover_device_paths_and_supported_identity_tags() {
        let parameters = parse(
            "root=/dev/sda root=live:/dev/sdb root=UUID=fs-id \
             root=PARTUUID=part-id root=LABEL=label root=live:CDLABEL=iso \
             root=PARTLABEL=partition root=/dev/sda",
        )
        .unwrap();
        assert_eq!(
            boot_source_paths(&parameters).unwrap(),
            [
                "/dev/sda",
                "/dev/sdb",
                "/dev/disk/by-uuid/fs-id",
                "/dev/disk/by-partuuid/part-id",
                "/dev/disk/by-label/label",
                "/dev/disk/by-label/iso",
                "/dev/disk/by-partlabel/partition",
            ]
            .into_iter()
            .map(PathBuf::from)
            .collect()
        );
    }

    #[test]
    fn boot_sources_ignore_ram_network_and_non_root_parameters() {
        let parameters = parse(
            "root=/dev/ram999999 root=/dev/zram0 \
             root=live:https://example.test/installer.iso rd.live.ram=1 \
             rd.root=/dev/sda root -- root=/dev/sdb",
        )
        .unwrap();
        assert!(boot_source_paths(&parameters).unwrap().is_empty());
    }

    #[test]
    fn boot_source_labels_support_quotes_unicode_and_encoded_names() {
        let parameters = parse(
            "root=\"LABEL=live disk\" root=PARTLABEL=\u{6570}\u{636e}/\u{76d8} root=LABEL=live\\x20disk",
        )
        .unwrap();
        assert_eq!(
            boot_source_paths(&parameters).unwrap(),
            HashSet::from([
                PathBuf::from(r"/dev/disk/by-label/live\x20disk"),
                PathBuf::from("/dev/disk/by-partlabel/\u{6570}\u{636e}\\x2f\u{76d8}"),
            ])
        );
    }

    #[test]
    fn boot_sources_reject_empty_and_directory_identities() {
        for input in [
            "root=",
            "root=live:",
            "root=UUID=",
            "root=LABEL=.",
            "root=PARTLABEL=..",
        ] {
            assert!(
                boot_source_paths(&parse(input).unwrap()).is_err(),
                "{input:?}"
            );
        }
    }
}
