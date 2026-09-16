//! `$include` resolution — splicing shared library files into a config tree before merge.
//!
//! `meta/docs/2026-06-22-image-definitions.md` §7: `$include` resolves to the parsed content of a repo-root-relative
//! file and substitutes it **as the value at its position**. As a mapping value the key becomes the
//! file's content (a bare subtree, not a re-stated key); as a list element a list file is spliced
//! (flattened) in place. `$include` must be the sole key of its mapping and may itself contain
//! `$include` (resolved recursively; cycles are an error).

use std::{
    fs, mem,
    path::{Path, PathBuf},
};

use serde_yaml_ng::Value;

use crate::error::ConfigError;

const INCLUDE: &str = "$include";

/// Resolve every `$include` in `value`, reading library files relative to `root`.
pub(crate) fn resolve_includes(value: &mut Value, root: &Path) -> Result<(), ConfigError> {
    let mut stack = Vec::new();
    let mut path = Vec::new();
    resolve(value, root, &mut path, &mut stack)
}

fn resolve(
    value: &mut Value,
    root: &Path,
    path: &mut Vec<String>,
    stack: &mut Vec<PathBuf>,
) -> Result<(), ConfigError> {
    if let Some(target) = include_target(value, path)? {
        // A node that is itself an `$include` (a mapping value or a whole library file). `load`
        // fully resolves the loaded content, including any chained or cyclic includes.
        *value = load(root, &target, path, stack)?;
        return Ok(());
    }
    match value {
        Value::Mapping(map) => {
            for (key, child) in map.iter_mut() {
                path.push(key.as_str().unwrap_or_default().to_owned());
                resolve(child, root, path, stack)?;
                path.pop();
            }
        }
        Value::Sequence(items) => {
            let mut out = Vec::with_capacity(items.len());
            for mut item in mem::take(items) {
                if let Some(target) = include_target(&item, path)? {
                    match load(root, &target, path, stack)? {
                        Value::Sequence(spliced) => out.extend(spliced),
                        other => out.push(other),
                    }
                } else {
                    resolve(&mut item, root, path, stack)?;
                    out.push(item);
                }
            }
            *items = out;
        }
        _ => {}
    }
    Ok(())
}

/// If `value` is a `{ $include: <path> }` mapping, return the (string) target; otherwise `None`.
/// Rejects a non-string target and an `$include` mixed with sibling keys.
fn include_target(value: &Value, path: &[String]) -> Result<Option<String>, ConfigError> {
    let Value::Mapping(map) = value else {
        return Ok(None);
    };
    if !map.contains_key(INCLUDE) {
        return Ok(None);
    }
    if map.len() > 1 {
        return Err(ConfigError::DirectiveNotSole {
            directive: INCLUDE.to_owned(),
            path: path.join("."),
        });
    }
    match map.get(INCLUDE).and_then(Value::as_str) {
        Some(target) => Ok(Some(target.to_owned())),
        None => Err(ConfigError::IncludePathInvalid {
            path: path.join("."),
        }),
    }
}

fn load(
    root: &Path,
    target: &str,
    path: &mut Vec<String>,
    stack: &mut Vec<PathBuf>,
) -> Result<Value, ConfigError> {
    let file = root.join(target);
    if stack.contains(&file) {
        let chain = stack
            .iter()
            .map(|p| p.display().to_string())
            .chain(std::iter::once(file.display().to_string()))
            .collect::<Vec<_>>()
            .join(" → ");
        return Err(ConfigError::IncludeCycle { chain });
    }
    let text = fs::read_to_string(&file).map_err(|source| ConfigError::Read {
        path: file.clone(),
        source,
    })?;
    let mut included: Value =
        serde_yaml_ng::from_str(&text).map_err(|source| ConfigError::Parse {
            path: file.clone(),
            source,
        })?;
    stack.push(file);
    resolve(&mut included, root, path, stack)?;
    stack.pop();
    Ok(included)
}

#[cfg(test)]
mod tests {
    use super::*;

    use indoc::indoc;
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[test]
    fn include_as_mapping_value_becomes_file_content() {
        let root = TempDir::new().unwrap();
        write(
            root.path(),
            "layouts/storage/grub.yaml",
            "bootType: efi\ndisks: []\n",
        );
        let mut value: Value = serde_yaml_ng::from_str(indoc! {"
            storage:
              $include: layouts/storage/grub.yaml
        "})
        .unwrap();
        resolve_includes(&mut value, root.path()).unwrap();
        assert_eq!(value["storage"]["bootType"].as_str(), Some("efi"));
        assert!(value["storage"]["disks"].is_sequence());
    }

    #[test]
    fn include_as_list_element_is_spliced() {
        let root = TempDir::new().unwrap();
        write(root.path(), "shared/files.yaml", "- a\n- b\n");
        let mut value: Value = serde_yaml_ng::from_str(indoc! {"
            items:
              - first
              - $include: shared/files.yaml
              - last
        "})
        .unwrap();
        resolve_includes(&mut value, root.path()).unwrap();
        let items: Vec<&str> = value["items"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(items, ["first", "a", "b", "last"]);
    }

    #[test]
    fn nested_includes_resolve_recursively() {
        let root = TempDir::new().unwrap();
        write(root.path(), "a.yaml", "inner:\n  $include: b.yaml\n");
        write(root.path(), "b.yaml", "leaf: 1\n");
        let mut value: Value = serde_yaml_ng::from_str("outer:\n  $include: a.yaml\n").unwrap();
        resolve_includes(&mut value, root.path()).unwrap();
        assert_eq!(value["outer"]["inner"]["leaf"].as_i64(), Some(1));
    }

    #[test]
    fn include_cycle_is_detected() {
        let root = TempDir::new().unwrap();
        write(root.path(), "a.yaml", "$include: b.yaml\n");
        write(root.path(), "b.yaml", "$include: a.yaml\n");
        let mut value: Value = serde_yaml_ng::from_str("x:\n  $include: a.yaml\n").unwrap();
        let err = resolve_includes(&mut value, root.path()).unwrap_err();
        assert!(
            matches!(err, ConfigError::IncludeCycle { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn include_with_siblings_is_rejected() {
        let root = TempDir::new().unwrap();
        write(root.path(), "x.yaml", "k: v\n");
        let mut value: Value =
            serde_yaml_ng::from_str("storage:\n  $include: x.yaml\n  extra: 1\n").unwrap();
        let err = resolve_includes(&mut value, root.path()).unwrap_err();
        assert!(
            matches!(err, ConfigError::DirectiveNotSole { .. }),
            "got {err:?}"
        );
    }
}
