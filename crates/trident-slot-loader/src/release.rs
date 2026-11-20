//! Confext metadata compatibility. No image mounting, publication or shell evaluation occurs here.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Error};

use crate::filesystem;

const MAX_RELEASE_BYTES: u64 = 64 * 1024;
type Fields = HashMap<String, String>;

/// Factory metadata from the booted OS, never the initrd's own os-release.
pub(super) struct HostRelease(Fields);

pub(super) fn host(root: &Path) -> Result<HostRelease, Error> {
    let root = root.canonicalize().context("resolving target root")?;
    let path = root
        .join("usr/lib/os-release")
        .canonicalize()
        .context("resolving factory os-release")?;
    ensure!(
        path.starts_with(root.join("usr")),
        "factory os-release escapes target /usr"
    );
    Ok(HostRelease(read(&path)?))
}

pub(super) fn validate_confext(
    mount: &Path,
    name: &str,
    host: &HostRelease,
) -> Result<PathBuf, Error> {
    let lower = mount
        .join("etc")
        .canonicalize()
        .context("resolving confext /etc")?;
    let mounted_root = mount.canonicalize()?;
    ensure!(
        lower.starts_with(&mounted_root),
        "confext /etc escapes its mount"
    );
    ensure!(lower.is_dir(), "confext has no /etc directory");
    let release = lower
        .join("extension-release.d")
        .join(format!("extension-release.{name}"));
    let release = release
        .canonicalize()
        .with_context(|| format!("resolving {}", release.display()))?;
    ensure!(
        release.starts_with(&mounted_root),
        "extension-release escapes its mount"
    );
    let fields = read(&release)?;
    compatible(&fields, &host.0)?;
    Ok(lower)
}

fn read(path: &Path) -> Result<Fields, Error> {
    parse(&filesystem::read_bounded(path, MAX_RELEASE_BYTES)?)
        .with_context(|| format!("release metadata {}", path.display()))
}

fn parse(data: &str) -> Result<Fields, Error> {
    ensure!(!data.contains('\0'), "NUL in release metadata");
    let mut fields = Fields::new();
    for line in data
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let (key, value) = line.split_once('=').context("expected KEY=VALUE")?;
        ensure!(
            !key.is_empty()
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
            "invalid release key {key:?}"
        );
        fields.insert(key.to_string(), unquote(value)?);
    }
    Ok(fields)
}

fn unquote(value: &str) -> Result<String, Error> {
    let (value, double_quoted) =
        if let Some(first) = value.chars().next().filter(|ch| matches!(ch, '\'' | '"')) {
            ensure!(
                value.len() >= 2 && value.ends_with(first),
                "unterminated release value"
            );
            let inner = &value[1..value.len() - 1];
            if first == '\'' {
                ensure!(
                    !inner.contains('\''),
                    "concatenated release strings are unsupported"
                );
                return Ok(inner.to_string());
            }
            (inner, true)
        } else {
            (value, false)
        };
    let mut output = String::new();
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            let escaped = characters.next().context("trailing release escape")?;
            if double_quoted && !matches!(escaped, '$' | '`' | '"' | '\\') {
                output.push('\\');
            }
            output.push(escaped);
        } else {
            ensure!(
                character != '"' && (double_quoted || character != '\''),
                "unexpected quote in release value"
            );
            output.push(character);
        }
    }
    Ok(output)
}

fn compatible(extension: &Fields, host: &Fields) -> Result<(), Error> {
    let value = |key| {
        extension
            .get(key)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    };
    ensure!(value("CONFEXT_ID").is_some(), "missing CONFEXT_ID");
    ensure!(
        value("CONFEXT_SCOPE")
            .unwrap_or("system portable")
            .split_ascii_whitespace()
            .any(|scope| scope == "system"),
        "confext does not allow system scope"
    );
    if let Some(architecture) = value("ARCHITECTURE").filter(|value| *value != "_any") {
        let native = match std::env::consts::ARCH {
            "x86_64" => "x86-64",
            "aarch64" => "arm64",
            other => bail!("unsupported loader architecture {other}"),
        };
        ensure!(
            architecture == native,
            "confext architecture {architecture} differs from {native}"
        );
    }
    let host_id = host
        .get("ID")
        .filter(|value| !value.is_empty())
        .context("target OS has no ID")?;
    if value("ID") == Some("_any") {
        return Ok(());
    }
    ensure!(
        value("ID") == Some(host_id.as_str()),
        "confext OS ID mismatch: expected {host_id:?}, got {:?}",
        value("ID")
    );
    let host_value = |key| {
        host.get(key)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    };
    match (
        host_value("CONFEXT_LEVEL"),
        value("CONFEXT_LEVEL"),
        host_value("VERSION_ID"),
    ) {
        (Some(expected), Some(actual), _) => ensure!(
            expected == actual,
            "CONFEXT_LEVEL mismatch: expected {expected:?}, got {actual:?}"
        ),
        (_, _, Some(version)) => {
            ensure!(
                value("VERSION_ID") == Some(version),
                "VERSION_ID mismatch: expected {version:?}, got {:?}",
                value("VERSION_ID")
            )
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{fs, os::unix::fs::symlink};

    #[test]
    fn factory_metadata_cannot_resolve_to_the_initrd_or_another_root() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("usr/lib")).unwrap();
        fs::write(outside.path().join("os-release"), "ID=wrong").unwrap();
        symlink(
            outside.path().join("os-release"),
            root.path().join("usr/lib/os-release"),
        )
        .unwrap();
        assert!(host(root.path())
            .err()
            .unwrap()
            .to_string()
            .contains("escapes"));
    }

    #[test]
    fn confext_lowerdirs_and_release_metadata_cannot_escape_the_image_mount() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let host = HostRelease(parse("ID=acl\nVERSION_ID=3.0").unwrap());
        symlink(outside.path(), root.path().join("etc")).unwrap();
        validate_confext(root.path(), "defaults", &host).unwrap_err();
        fs::remove_file(root.path().join("etc")).unwrap();
        fs::create_dir_all(root.path().join("etc/extension-release.d")).unwrap();
        fs::write(
            outside.path().join("release"),
            "ID=_any\nCONFEXT_ID=defaults",
        )
        .unwrap();
        symlink(
            outside.path().join("release"),
            root.path()
                .join("etc/extension-release.d/extension-release.defaults"),
        )
        .unwrap();
        validate_confext(root.path(), "defaults", &host).unwrap_err();
    }

    #[test]
    fn oversized_or_nul_release_metadata_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("release");
        fs::write(&path, "x".repeat(MAX_RELEASE_BYTES as usize + 1)).unwrap();
        read(&path).unwrap_err();
        parse("ID=acl\0garbage").unwrap_err();
    }

    #[test]
    fn release_values_are_data_not_shell() {
        let fields = parse(indoc::indoc! {r#"
            # Comment
            ID=old
            ID="acl"
            CONFEXT_ID='defaults'
            QUOTED="a\"b"
            LITERAL='$(not-executed)'
            PRETTY_NAME="Vendor's OS"
        "#})
        .unwrap();
        assert_eq!(fields["ID"], "acl");
        assert_eq!(fields["QUOTED"], "a\"b");
        assert_eq!(fields["LITERAL"], "$(not-executed)");
        assert_eq!(fields["PRETTY_NAME"], "Vendor's OS");
        parse("ID=\"unfinished").unwrap_err();
        parse("ID='a'\"b\"").unwrap_err();
    }

    #[test]
    fn compatibility_accepts_any_or_matching_version_or_level() {
        let host = parse("ID=acl\nVERSION_ID=3.0\nCONFEXT_LEVEL=7").unwrap();
        for release in [
            "ID=_any\nCONFEXT_ID=defaults",
            "ID=acl\nVERSION_ID=3.0\nCONFEXT_ID=defaults",
            "ID=acl\nVERSION_ID=other\nCONFEXT_LEVEL=7\nCONFEXT_ID=defaults",
        ] {
            compatible(&parse(release).unwrap(), &host).unwrap();
        }
        for release in [
            "ID=other\nVERSION_ID=3.0\nCONFEXT_ID=defaults",
            "ID=acl\nVERSION_ID=old\nCONFEXT_ID=defaults",
            "ID=acl\nVERSION_ID=3.0\nCONFEXT_LEVEL=8\nCONFEXT_ID=defaults",
            "ID=_any\nCONFEXT_ID=defaults\nCONFEXT_SCOPE=initrd",
            "ID=_any\nCONFEXT_ID=defaults\nARCHITECTURE=wrong",
            "ID=_any",
        ] {
            compatible(&parse(release).unwrap(), &host).unwrap_err();
        }
    }
}
