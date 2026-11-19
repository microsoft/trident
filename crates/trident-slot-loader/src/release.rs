use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Error};

type Fields = HashMap<String, String>;

pub(super) fn host(root: &Path) -> Result<Fields, Error> {
    read(&root.join("usr/lib/os-release"))
}

pub(super) fn validate_confext(mount: &Path, name: &str, host: &Fields) -> Result<PathBuf, Error> {
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
    compatible(&fields, host)?;
    Ok(lower)
}

fn read(path: &Path) -> Result<Fields, Error> {
    parse(&fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?)
        .with_context(|| format!("release metadata {}", path.display()))
}

fn parse(data: &str) -> Result<Fields, Error> {
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
        "confext OS ID mismatch"
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
        (Some(expected), Some(actual), _) => ensure!(expected == actual, "CONFEXT_LEVEL mismatch"),
        (_, _, Some(version)) => {
            ensure!(value("VERSION_ID") == Some(version), "VERSION_ID mismatch")
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
