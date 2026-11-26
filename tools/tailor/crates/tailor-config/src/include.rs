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

#[derive(PartialEq, Eq)]
struct Selection {
    file: PathBuf,
    pointer: Option<String>,
}

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
    stack: &mut Vec<Selection>,
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
    stack: &mut Vec<Selection>,
) -> Result<Value, ConfigError> {
    let (filename, pointer) = match target.find("#/") {
        Some(index) => (&target[..index], Some(&target[index + 1..])),
        None if target.ends_with('#') => (&target[..target.len() - 1], Some("")),
        None => (target, None),
    };
    if filename.is_empty() {
        return Err(ConfigError::IncludePathInvalid {
            path: path.join("."),
        });
    }
    let file = root.join(filename);
    let canonical = fs::canonicalize(&file).map_err(|source| ConfigError::Read {
        path: file.clone(),
        source,
    })?;
    let selection = Selection {
        file: canonical,
        pointer: pointer.map(str::to_owned),
    };
    if stack.contains(&selection) {
        let chain = stack
            .iter()
            .chain(std::iter::once(&selection))
            .map(|selection| {
                format!(
                    "{}{}",
                    selection.file.display(),
                    selection
                        .pointer
                        .as_ref()
                        .map(|p| format!("#{p}"))
                        .unwrap_or_default()
                )
            })
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
    if let Some(pointer) = pointer {
        included = select_node(included, pointer, target)?;
    }
    stack.push(selection);
    resolve(&mut included, root, path, stack)?;
    stack.pop();
    Ok(included)
}

fn select_node(mut value: Value, pointer: &str, target: &str) -> Result<Value, ConfigError> {
    if pointer.is_empty() {
        return Ok(value);
    }
    for encoded in pointer.strip_prefix('/').unwrap_or_default().split('/') {
        let mut key = String::new();
        let mut chars = encoded.chars();
        while let Some(character) = chars.next() {
            if character == '~' {
                key.push(match chars.next() {
                    Some('0') => '~',
                    Some('1') => '/',
                    _ => return Err(selector_error(target, "invalid JSON pointer escape")),
                });
            } else {
                key.push(character);
            }
        }
        let selected = match &value {
            Value::Mapping(map) => map.get(Value::String(key.clone())),
            Value::Sequence(items) => {
                if key.is_empty()
                    || (key.len() > 1 && key.starts_with('0'))
                    || !key.bytes().all(|b| b.is_ascii_digit())
                {
                    None
                } else {
                    key.parse::<usize>().ok().and_then(|index| items.get(index))
                }
            }
            _ => None,
        };
        value = selected
            .cloned()
            .ok_or_else(|| selector_error(target, "node does not exist"))?;
    }
    Ok(value)
}

fn selector_error(target: &str, detail: &str) -> ConfigError {
    ConfigError::InvalidField {
        slug: target.to_owned(),
        field: "$include",
        detail: detail.to_owned(),
    }
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
    fn node_selection_supports_escaped_keys_and_sequences() {
        let root = TempDir::new().unwrap();
        write(
            root.path(),
            "library.yaml",
            "'a/b':\n  '~key': [first, second]\n",
        );
        let mut value: Value =
            serde_yaml_ng::from_str("$include: library.yaml#/a~1b/~0key/1\n").unwrap();
        resolve_includes(&mut value, root.path()).unwrap();
        assert_eq!(value.as_str(), Some("second"));
        for pointer in ["/missing", "/a~2b", "/a~1b/~0key/01"] {
            let mut value: Value =
                serde_yaml_ng::from_str(&format!("$include: library.yaml#{pointer}\n")).unwrap();
            assert!(matches!(
                resolve_includes(&mut value, root.path()),
                Err(ConfigError::InvalidField { .. })
            ));
        }
    }

    #[test]
    fn different_nodes_in_one_library_can_include_each_other() {
        let root = TempDir::new().unwrap();
        write(
            root.path(),
            "library.yaml",
            "base: |\n  echo base\nselected:\n  $include: library.yaml#/base\nunused:\n  $include: missing.yaml\n",
        );
        let mut value: Value =
            serde_yaml_ng::from_str("$include: library.yaml#/selected\n").unwrap();
        resolve_includes(&mut value, root.path()).unwrap();
        assert_eq!(value.as_str(), Some("echo base\n"));
    }

    #[test]
    fn selected_node_cycles_are_detected_with_path_aliases() {
        let root = TempDir::new().unwrap();
        write(
            root.path(),
            "library.yaml",
            "a:\n  $include: ./library.yaml#/b\nb:\n  $include: library.yaml#/a\n",
        );
        let mut value: Value = serde_yaml_ng::from_str("$include: library.yaml#/a\n").unwrap();
        assert!(matches!(
            resolve_includes(&mut value, root.path()),
            Err(ConfigError::IncludeCycle { .. })
        ));
    }

    #[test]
    fn hash_characters_in_existing_filenames_remain_supported() {
        let root = TempDir::new().unwrap();
        write(root.path(), "library#v1.yaml", "value: old\n");
        let mut value: Value = serde_yaml_ng::from_str("$include: library#v1.yaml\n").unwrap();
        resolve_includes(&mut value, root.path()).unwrap();
        assert_eq!(value["value"].as_str(), Some("old"));
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
