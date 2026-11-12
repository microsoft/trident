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

/// Resolves local, non-RAM `root=` references from `/proc/cmdline`.
///
/// Skips known RAM-backed/network roots and unsupported root formats. Canonicalizes
/// recognized device references, including udev UUID/label aliases, and deduplicates
/// repeated roots. Unresolvable local device references are errors.
///
/// Results identify partitions or mapper nodes, not their parent disks. Compare
/// them with disk descendants to protect the storage containing the boot root.
/// This classifies root references; it does not inspect the backing hardware.
///
/// For example, `root=live:UUID=abc` resolves `/dev/disk/by-uuid/abc`, while
/// `root=/dev/ram0` and `root=/dev/nfs` contribute no device.
pub fn local_root_devices() -> Result<HashSet<PathBuf>, Error> {
    local_root_references(&read()?)?
        .into_iter()
        .map(|path| {
            fs::canonicalize(&path)
                .with_context(|| format!("Cannot resolve boot source '{}'", path.display()))
        })
        .collect()
}

/// Converts local root references to device paths without filesystem I/O.
///
/// Leaves `/dev/...` paths unchanged and maps identity tags to udev symlinks,
/// e.g., `UUID=abc` becomes `/dev/disk/by-uuid/abc`. RAM/network roots are omitted.
fn local_root_references(parameters: &Cmdline<'_>) -> Result<HashSet<PathBuf>, Error> {
    let mut paths = HashSet::new();
    for parameter in parameters
        .iter()
        // Arguments after a standalone `--` belong to init, not the kernel.
        .take_while(|parameter| parameter.key() != "--".into() || parameter.value().is_some())
        .filter(|parameter| parameter.key() == "root".into())
    {
        let Some(root) = parameter.value() else {
            continue;
        };
        // Normalize dracut's live-root syntax before classifying the source.
        let root = root.strip_prefix("live:").unwrap_or(root);
        ensure!(!root.is_empty(), "Root device specification is empty");
        // RAM disks and NFS/CIFS pseudo-devices do not identify a disk to protect.
        if matches!(root, "/dev/nfs" | "/dev/cifs")
            || ["/dev/ram", "/dev/zram"].iter().any(|prefix| {
                root.strip_prefix(prefix).is_some_and(|suffix| {
                    !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
                })
            })
        {
            continue;
        }
        if root.starts_with("/dev/") {
            paths.insert(PathBuf::from(root));
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
                paths.insert(
                    Path::new("/dev/disk")
                        .join(directory)
                        .join(encode_udev_alias_name(value)),
                );
                break;
            }
        }
    }
    Ok(paths)
}

/// Converts a UUID or label to a udev symlink's filename, not a full path.
///
/// For example, `live disk` becomes `live\x20disk`; non-ASCII UTF-8 is retained.
fn encode_udev_alias_name(value: &str) -> String {
    let mut encoded = String::new();
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\\' {
            // Cmdline labels may already be encoded; do not escape them twice.
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
    fn local_roots_stop_at_init_arguments_but_not_separator_values() {
        for input in [
            "root=/dev/sda -- root=/dev/sdb",
            r#"root=/dev/sda "--" broken="unterminated"#,
        ] {
            assert_eq!(
                local_root_references(&Cmdline::from(input)).unwrap(),
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
                local_root_references(&Cmdline::from(input)).unwrap(),
                HashSet::from([PathBuf::from("/dev/sdb")]),
                "{input:?}"
            );
        }
    }

    #[test]
    fn local_roots_cover_device_paths_and_supported_identity_tags() {
        let parameters = Cmdline::from(
            "root=/dev/sda root=live:/dev/sdb root=UUID=fs-id \
             root=PARTUUID=part-id root=LABEL=label root=live:CDLABEL=iso \
             root=PARTLABEL=partition root=/dev/sda",
        );
        assert_eq!(
            local_root_references(&parameters).unwrap(),
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
    fn local_roots_ignore_ram_network_and_non_root_parameters() {
        let parameters = Cmdline::from(
            "root=/dev/ram999999 root=/dev/zram0 \
             root=/dev/nfs root=/dev/cifs root=live:/dev/nfs root=live:/dev/cifs \
             root=live:https://example.test/installer.iso rd.live.ram=1 \
             rd.root=/dev/sda root -- root=/dev/sdb",
        );
        assert!(local_root_references(&parameters).unwrap().is_empty());
    }

    #[test]
    fn local_root_labels_support_quotes_unicode_and_encoded_names() {
        let parameters = Cmdline::from(
            "root=\"LABEL=live disk\" root=PARTLABEL=\u{6570}\u{636e}/\u{76d8} root=LABEL=live\\x20disk",
        );
        assert_eq!(
            local_root_references(&parameters).unwrap(),
            HashSet::from([
                PathBuf::from(r"/dev/disk/by-label/live\x20disk"),
                PathBuf::from("/dev/disk/by-partlabel/\u{6570}\u{636e}\\x2f\u{76d8}"),
            ])
        );
    }

    #[test]
    fn local_roots_reject_empty_and_directory_identities() {
        for input in [
            "root=",
            "root=live:",
            "root=UUID=",
            "root=LABEL=.",
            "root=PARTLABEL=..",
        ] {
            assert!(
                local_root_references(&Cmdline::from(input)).is_err(),
                "{input:?}"
            );
        }
    }
}
