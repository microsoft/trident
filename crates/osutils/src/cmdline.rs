use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Error};
use linux_kernel_cmdline::utf8::{Cmdline, CmdlineOwned};

/// Reads `/proc/cmdline` using `linux-kernel-cmdline` parsing semantics.
pub fn read() -> Result<CmdlineOwned, Error> {
    CmdlineOwned::from_proc().context("Failed to read /proc/cmdline")
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
/// Repeated roots before `--` are all included. RAM roots and unsupported source
/// formats are ignored; recognized device references that cannot be resolved
/// return an error.
pub fn boot_sources() -> Result<HashSet<PathBuf>, Error> {
    boot_source_paths(&read()?)?
        .into_iter()
        .map(|path| {
            fs::canonicalize(&path)
                .with_context(|| format!("Cannot resolve boot source '{}'", path.display()))
        })
        .collect()
}

/// Converts parsed root specifications to device references without filesystem I/O.
fn boot_source_paths(parameters: &Cmdline<'_>) -> Result<HashSet<PathBuf>, Error> {
    let mut sources = HashSet::new();
    for parameter in parameters
        .iter()
        .take_while(|parameter| parameter.key() != "--".into() || parameter.value().is_some())
        .filter(|parameter| parameter.key() == "root".into())
    {
        let Some(root) = parameter.value() else {
            continue;
        };
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
    fn boot_sources_stop_at_init_arguments_but_not_separator_values() {
        for input in [
            "root=/dev/sda -- root=/dev/sdb",
            r#"root=/dev/sda "--" broken="unterminated"#,
        ] {
            assert_eq!(
                boot_source_paths(&Cmdline::from(input)).unwrap(),
                HashSet::from([PathBuf::from("/dev/sda")]),
                "{input:?}"
            );
        }
        for input in [
            "value=-- root=/dev/sdb",
            "--=value root=/dev/sdb",
            "--= root=/dev/sdb",
        ] {
            assert_eq!(
                boot_source_paths(&Cmdline::from(input)).unwrap(),
                HashSet::from([PathBuf::from("/dev/sdb")]),
                "{input:?}"
            );
        }
    }

    #[test]
    fn boot_sources_cover_device_paths_and_supported_identity_tags() {
        let parameters = Cmdline::from(
            "root=/dev/sda root=live:/dev/sdb root=UUID=fs-id \
             root=PARTUUID=part-id root=LABEL=label root=live:CDLABEL=iso \
             root=PARTLABEL=partition root=/dev/sda",
        );
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
        let parameters = Cmdline::from(
            "root=/dev/ram999999 root=/dev/zram0 \
             root=live:https://example.test/installer.iso rd.live.ram=1 \
             rd.root=/dev/sda root -- root=/dev/sdb",
        );
        assert!(boot_source_paths(&parameters).unwrap().is_empty());
    }

    #[test]
    fn boot_source_labels_support_quotes_unicode_and_encoded_names() {
        let parameters = Cmdline::from(
            "root=\"LABEL=live disk\" root=PARTLABEL=\u{6570}\u{636e}/\u{76d8} root=LABEL=live\\x20disk",
        );
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
                boot_source_paths(&Cmdline::from(input)).is_err(),
                "{input:?}"
            );
        }
    }
}
