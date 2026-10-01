//! Deep-merge of Image Customizer config value trees with tailor's small directive vocabulary.
//!
//! Implements the merge semantics of `meta/docs/2026-06-29-directive-design.md`: mappings deep-merge
//! (preserving the insertion order the cell slug and goldens depend on), lists **append** by default
//! — with `$prepend`/`$append`/`$remove` for ordered edits and `$replace` for a whole-list swap — and
//! scalars **conflict** on a differing value unless overridden with `$set`. A key whose value is the
//! bare token `$unset` is removed entirely. `$include` is resolved in an earlier pass and must never
//! reach the merger; `$select` is reserved and currently errors here.

use std::mem;

use serde_yaml_ng::{Mapping, Value};

use crate::error::ConfigError;

const SET: &str = "$set";
const REPLACE: &str = "$replace";
const REMOVE: &str = "$remove";
const PREPEND: &str = "$prepend";
const APPEND: &str = "$append";
const RENAME: &str = "$rename";
const INCLUDE: &str = "$include";
const SELECT: &str = "$select";
const UNSET: &str = "$unset";

/// The list-ordering directives that may share a single mapping (`$replace`/`$set` are exclusive).
const LIST_OPS: [&str; 3] = [PREPEND, APPEND, REMOVE];

/// Threaded state for a single merge: the dotted path (for diagnostics and list policy) and the
/// label of the fragment supplying the overlay (for conflict messages).
struct Ctx<'a> {
    path: Vec<String>,
    source: &'a str,
}

impl Ctx<'_> {
    fn dotted(&self) -> String {
        self.path.join(".")
    }
}

/// Deep-merge `overlay` onto the mutable `base` mapping at the config root (strict conflicts).
///
/// `source` labels the fragment contributing `overlay`, so a conflict can name it.
pub(crate) fn merge_into(
    base: &mut Mapping,
    overlay: Mapping,
    source: &str,
) -> Result<(), ConfigError> {
    let mut ctx = Ctx {
        path: Vec::new(),
        source,
    };
    merge_mapping(base, overlay, &mut ctx)
}

/// Merge a single tailor field value (e.g. `base`, `outputs`) across fragments, honouring directives.
pub(crate) fn merge_field(
    existing: Option<Value>,
    overlay: Value,
    field: &str,
    source: &str,
) -> Result<Value, ConfigError> {
    let mut ctx = Ctx {
        path: vec![field.to_owned()],
        source,
    };
    merge_value(existing, overlay, &mut ctx)
}

fn merge_value(
    existing: Option<Value>,
    mut overlay: Value,
    ctx: &mut Ctx<'_>,
) -> Result<Value, ConfigError> {
    // A bare `$unset` only has meaning as a mapping value (handled in `merge_mapping`, which owns the
    // parent and can delete the key). Reaching here means it was used as a list item or top-level
    // value, which cannot remove anything — reject it rather than emit the literal string.
    if matches!(&overlay, Value::String(s) if s == UNSET) {
        return Err(ConfigError::DirectiveShape {
            directive: UNSET,
            path: ctx.dotted(),
            expected: "to be a mapping value (it removes that key), not a list item or bare value",
        });
    }
    match classify_directives(&mut overlay, ctx)? {
        Directive::ListOps(map) => return apply_list_ops(&map, existing, ctx),
        Directive::Single(name, inner) => return apply_directive(&name, inner, existing, ctx),
        Directive::None => {}
    }
    match (existing, overlay) {
        (Some(Value::Mapping(mut base)), Value::Mapping(over)) => {
            merge_mapping(&mut base, over, ctx)?;
            Ok(Value::Mapping(base))
        }
        (Some(Value::Sequence(base)), Value::Sequence(over)) => merge_sequence(base, over, ctx),
        (Some(base), over) => {
            if is_container(&base) || is_container(&over) {
                Err(ConfigError::TypeConflict {
                    path: ctx.dotted(),
                    existing_kind: kind(&base),
                    incoming_kind: kind(&over),
                    fragment: ctx.source.to_owned(),
                })
            } else if base == over {
                Ok(base)
            } else {
                Err(ConfigError::ScalarConflict {
                    path: ctx.dotted(),
                    existing: scalar_string(&base),
                    incoming: scalar_string(&over),
                    fragment: ctx.source.to_owned(),
                })
            }
        }
        (None, Value::Mapping(over)) => {
            let mut base = Mapping::new();
            merge_mapping(&mut base, over, ctx)?;
            Ok(Value::Mapping(base))
        }
        (None, Value::Sequence(over)) => merge_sequence(Vec::new(), over, ctx),
        (None, scalar) => Ok(scalar),
    }
}

fn merge_mapping(
    base: &mut Mapping,
    overlay: Mapping,
    ctx: &mut Ctx<'_>,
) -> Result<(), ConfigError> {
    for (key, over_val) in overlay {
        ctx.path.push(key.as_str().unwrap_or_default().to_owned());
        // `$unset` (the bare token, or the tolerated `{ $unset: true }` synonym) removes the key.
        if is_unset(&over_val, ctx)? {
            base.shift_remove(&key);
            ctx.path.pop();
            continue;
        }
        if let Some(slot) = base.get_mut(&key) {
            let taken = mem::replace(slot, Value::Null);
            let merged = merge_value(Some(taken), over_val, ctx)?;
            if let Some(slot) = base.get_mut(&key) {
                *slot = merged;
            }
        } else {
            let fresh = merge_value(None, over_val, ctx)?;
            base.insert(key, fresh);
        }
        ctx.path.pop();
    }
    Ok(())
}

/// Whether `value` removes its key: the bare scalar `$unset`, or the tolerated `{ $unset: true }`
/// mapping synonym (any other `$unset:` value is a shape error — `$unset` carries no real argument).
fn is_unset(value: &Value, ctx: &Ctx<'_>) -> Result<bool, ConfigError> {
    match value {
        Value::String(s) if s == UNSET => Ok(true),
        Value::Mapping(map) if map.len() == 1 && map.contains_key(UNSET) => match map.get(UNSET) {
            Some(Value::Bool(true)) => Ok(true),
            _ => Err(ConfigError::DirectiveShape {
                directive: UNSET,
                path: ctx.dotted(),
                expected: "`true` (prefer the bare value `key: $unset`)",
            }),
        },
        _ => Ok(false),
    }
}

/// Lists **append** by default. tailor does not key lists by any Image Customizer field (that would
/// bake IC's schema into the merger); finer control — whole-list replace or item removal — is via
/// the explicit `$replace` / `$remove` directives.
fn merge_sequence(
    base: Vec<Value>,
    overlay: Vec<Value>,
    ctx: &mut Ctx<'_>,
) -> Result<Value, ConfigError> {
    let mut out = base;
    for item in overlay {
        out.push(merge_value(None, item, ctx)?);
    }
    Ok(Value::Sequence(out))
}

/// How a fragment value is interpreted: plain data, a single directive, or a combined list edit.
enum Directive {
    None,
    Single(String, Value),
    ListOps(Mapping),
}

/// Classify `overlay`'s `$`-directives. A mapping of only `$prepend`/`$append`/`$remove` is a combined
/// list edit; otherwise exactly one directive is allowed. `$`-keys may not share a mapping with data.
fn classify_directives(overlay: &mut Value, ctx: &Ctx<'_>) -> Result<Directive, ConfigError> {
    let Value::Mapping(map) = overlay else {
        return Ok(Directive::None);
    };
    let dollar: Vec<String> = map
        .keys()
        .filter_map(Value::as_str)
        .filter(|k| k.starts_with('$'))
        .map(str::to_owned)
        .collect();
    if dollar.is_empty() {
        return Ok(Directive::None);
    }
    if map.len() > dollar.len() {
        return Err(ConfigError::DirectiveNotSole {
            directive: dollar.join("`, `"),
            path: ctx.dotted(),
        });
    }
    if dollar.iter().all(|k| LIST_OPS.contains(&k.as_str())) {
        let taken = mem::replace(map, Mapping::new());
        return Ok(Directive::ListOps(taken));
    }
    match dollar.as_slice() {
        [name] => {
            let inner = map.remove(name.as_str()).unwrap_or(Value::Null);
            Ok(Directive::Single(name.clone(), inner))
        }
        many => Err(ConfigError::ConflictingDirectives {
            directives: many.join("`, `"),
            path: ctx.dotted(),
        }),
    }
}

/// Apply a combined list edit to the inherited list: `$prepend` items, then the inherited list with
/// `$remove` matches dropped, then `$append` items. New (prepended/appended) items are themselves
/// merged so nested directives resolve consistently with plain list append.
fn apply_list_ops(
    map: &Mapping,
    existing: Option<Value>,
    ctx: &mut Ctx<'_>,
) -> Result<Value, ConfigError> {
    let base: Vec<Value> = match existing {
        Some(Value::Sequence(items)) => items,
        None => Vec::new(),
        Some(other) => {
            return Err(ConfigError::TypeConflict {
                path: ctx.dotted(),
                existing_kind: kind(&other),
                incoming_kind: "list",
                fragment: ctx.source.to_owned(),
            });
        }
    };
    let remaining: Vec<Value> = match map.get(REMOVE) {
        Some(Value::Sequence(drop)) => base.into_iter().filter(|it| !drop.contains(it)).collect(),
        Some(_) => {
            return Err(ConfigError::DirectiveShape {
                directive: REMOVE,
                path: ctx.dotted(),
                expected: "a list",
            });
        }
        None => base,
    };
    let prepend = directive_list(map, PREPEND, ctx)?;
    let append = directive_list(map, APPEND, ctx)?;
    let mut out = Vec::with_capacity(prepend.len() + remaining.len() + append.len());
    for item in prepend {
        out.push(merge_value(None, item, ctx)?);
    }
    out.extend(remaining);
    for item in append {
        out.push(merge_value(None, item, ctx)?);
    }
    Ok(Value::Sequence(out))
}

/// The list body of a list-ordering directive (`None` → empty); a non-list body is a shape error.
fn directive_list(
    map: &Mapping,
    name: &'static str,
    ctx: &Ctx<'_>,
) -> Result<Vec<Value>, ConfigError> {
    match map.get(name) {
        Some(Value::Sequence(items)) => Ok(items.clone()),
        Some(_) => Err(ConfigError::DirectiveShape {
            directive: name,
            path: ctx.dotted(),
            expected: "a list",
        }),
        None => Ok(Vec::new()),
    }
}

fn apply_directive(
    name: &str,
    inner: Value,
    _existing: Option<Value>,
    ctx: &mut Ctx<'_>,
) -> Result<Value, ConfigError> {
    match name {
        SET => merge_value(None, inner, ctx),
        REPLACE => {
            if !matches!(inner, Value::Sequence(_)) {
                return Err(ConfigError::DirectiveShape {
                    directive: REPLACE,
                    path: ctx.dotted(),
                    expected: "a list",
                });
            }
            merge_value(None, inner, ctx)
        }
        UNSET => Err(ConfigError::DirectiveShape {
            directive: UNSET,
            path: ctx.dotted(),
            expected: "to be a mapping value (it removes that key), not a list item or bare value",
        }),
        RENAME => Err(ConfigError::UnsupportedDirective {
            directive: RENAME,
            path: ctx.dotted(),
        }),
        INCLUDE => Err(ConfigError::UnresolvedDirective {
            directive: INCLUDE,
            path: ctx.dotted(),
        }),
        SELECT => Err(ConfigError::ReservedDirective {
            directive: SELECT,
            path: ctx.dotted(),
        }),
        other => Err(ConfigError::UnknownDirective {
            directive: other.to_owned(),
            path: ctx.dotted(),
        }),
    }
}

fn is_container(value: &Value) -> bool {
    matches!(value, Value::Mapping(_) | Value::Sequence(_))
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Mapping(_) => "mapping",
        Value::Sequence(_) => "list",
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
        Value::Null => "null",
        Value::Tagged(_) => "tagged value",
    }
}

fn scalar_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => "null".to_owned(),
        _ => "<complex>".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use indoc::indoc;

    fn yaml(text: &str) -> Mapping {
        serde_yaml_ng::from_str(text).unwrap()
    }

    fn merged(base: &str, overlay: &str) -> Mapping {
        let mut acc = yaml(base);
        merge_into(&mut acc, yaml(overlay), "overlay").unwrap();
        acc
    }

    fn to_yaml(map: &Mapping) -> String {
        serde_yaml_ng::to_string(map).unwrap()
    }

    #[test]
    fn maps_deep_merge_and_preserve_insertion_order() {
        let out = merged(
            indoc! {"
                os:
                  a: 1
                  b: 2
            "},
            indoc! {"
                os:
                  b: 2
                  c: 3
            "},
        );
        // `c` appends after the existing keys; `a`/`b` keep their positions.
        assert_eq!(to_yaml(&out), "os:\n  a: 1\n  b: 2\n  c: 3\n");
    }

    #[test]
    fn lists_append_by_default() {
        let out = merged(
            "os:\n  packages:\n    install: [a, b]\n",
            "os:\n  packages:\n    install: [c]\n",
        );
        let install = out["os"]["packages"]["install"].as_sequence().unwrap();
        let names: Vec<&str> = install.iter().filter_map(Value::as_str).collect();
        assert_eq!(names, ["a", "b", "c"]);
    }

    #[test]
    fn replace_directive_swaps_the_inherited_list() {
        let out = merged(
            "outputs: [cosi]\n",
            "outputs:\n  $replace:\n    - vhd-fixed\n",
        );
        let outputs = out["outputs"].as_sequence().unwrap();
        let names: Vec<&str> = outputs.iter().filter_map(Value::as_str).collect();
        assert_eq!(names, ["vhd-fixed"]);
    }

    #[test]
    fn remove_directive_drops_inherited_items() {
        let out = merged(
            "os:\n  packages:\n    install: [a, b, c]\n",
            "os:\n  packages:\n    install:\n      $remove: [b]\n",
        );
        let install = out["os"]["packages"]["install"].as_sequence().unwrap();
        let names: Vec<&str> = install.iter().filter_map(Value::as_str).collect();
        assert_eq!(names, ["a", "c"]);
    }

    #[test]
    fn set_directive_overrides_without_conflict() {
        let out = merged(
            "os:\n  selinux:\n    mode: disabled\n",
            "os:\n  selinux:\n    mode:\n      $set: enforcing\n",
        );
        assert_eq!(out["os"]["selinux"]["mode"].as_str(), Some("enforcing"));
    }

    #[test]
    fn differing_scalar_without_set_is_a_conflict() {
        let mut acc = yaml("os:\n  hostname: a\n");
        let err = merge_into(&mut acc, yaml("os:\n  hostname: b\n"), "frag-b").unwrap_err();
        assert!(
            matches!(err, ConfigError::ScalarConflict { .. }),
            "got {err:?}"
        );
        assert!(err.to_string().contains("frag-b"), "got {err}");
    }

    #[test]
    fn equal_scalar_set_twice_is_fine() {
        let out = merged("os:\n  hostname: a\n", "os:\n  hostname: a\n");
        assert_eq!(out["os"]["hostname"].as_str(), Some("a"));
    }

    #[test]
    fn storage_lists_append_generically_without_keying() {
        // No IC-specific keyed merge: a second `partitions` list appends rather than merging by id.
        let out = merged(
            indoc! {"
                storage:
                  disks:
                    - partitions:
                        - id: esp
                          size: 8M
            "},
            indoc! {"
                storage:
                  disks:
                    - partitions:
                        - id: esp
                          size: 16M
            "},
        );
        // `disks` is a list → append; the two single-disk entries are both kept (generic behavior).
        assert_eq!(out["storage"]["disks"].as_sequence().unwrap().len(), 2);
    }

    #[test]
    fn directive_mixed_with_siblings_is_rejected() {
        let mut acc = yaml("x: [a]\n");
        let err = merge_into(&mut acc, yaml("x:\n  $replace: [b]\n  extra: 1\n"), "f").unwrap_err();
        assert!(
            matches!(err, ConfigError::DirectiveNotSole { .. }),
            "got {err:?}"
        );
    }

    // ───────────────────────────── $unset ─────────────────────────────

    #[test]
    fn unset_bare_value_removes_inherited_key() {
        let out = merged(
            "os:\n  hostname: h\n  selinux:\n    mode: disabled\n",
            "os:\n  selinux: $unset\n",
        );
        assert_eq!(out["os"]["hostname"].as_str(), Some("h"));
        assert!(!out["os"].as_mapping().unwrap().contains_key("selinux"));
    }

    #[test]
    fn unset_mapping_synonym_true_removes_key() {
        let out = merged(
            "os:\n  overlays: [a]\n",
            "os:\n  overlays:\n    $unset: true\n",
        );
        assert!(!out["os"].as_mapping().unwrap().contains_key("overlays"));
    }

    #[test]
    fn unset_synonym_false_is_a_shape_error() {
        let mut acc = yaml("os:\n  selinux:\n    mode: x\n");
        let err =
            merge_into(&mut acc, yaml("os:\n  selinux:\n    $unset: false\n"), "f").unwrap_err();
        assert!(
            matches!(
                err,
                ConfigError::DirectiveShape {
                    directive: "$unset",
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn unset_absent_key_is_a_noop() {
        let out = merged("os:\n  hostname: h\n", "os:\n  selinux: $unset\n");
        assert_eq!(out["os"]["hostname"].as_str(), Some("h"));
        assert!(!out["os"].as_mapping().unwrap().contains_key("selinux"));
    }

    #[test]
    fn later_fragment_can_re_add_an_unset_key() {
        let mut acc = yaml("os:\n  selinux:\n    mode: disabled\n");
        merge_into(&mut acc, yaml("os:\n  selinux: $unset\n"), "drop").unwrap();
        merge_into(
            &mut acc,
            yaml("os:\n  selinux:\n    mode: enforcing\n"),
            "re-add",
        )
        .unwrap();
        assert_eq!(acc["os"]["selinux"]["mode"].as_str(), Some("enforcing"));
    }

    #[test]
    fn unset_as_a_list_item_is_an_error() {
        let mut acc = yaml("items: [a]\n");
        let err = merge_into(&mut acc, yaml("items:\n  - $unset\n"), "f").unwrap_err();
        assert!(
            matches!(
                err,
                ConfigError::DirectiveShape {
                    directive: "$unset",
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    // ───────────────────────────── $prepend / $append ─────────────────────────────

    fn names(value: &Value) -> Vec<String> {
        value
            .as_sequence()
            .unwrap()
            .iter()
            .map(|v| {
                v["path"]
                    .as_str()
                    .or_else(|| v.as_str())
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn prepend_puts_items_before_the_inherited_list() {
        let out = merged(
            "scripts:\n  post: [b, c]\n",
            "scripts:\n  post:\n    $prepend: [a]\n",
        );
        assert_eq!(names(&out["scripts"]["post"]), ["a", "b", "c"]);
    }

    #[test]
    fn append_alone_equals_the_default_append() {
        let out = merged(
            "scripts:\n  post: [a]\n",
            "scripts:\n  post:\n    $append: [b]\n",
        );
        assert_eq!(names(&out["scripts"]["post"]), ["a", "b"]);
    }

    #[test]
    fn prepend_and_append_touch_both_ends_in_one_fragment() {
        let out = merged(
            "scripts:\n  post: [a, b]\n",
            "scripts:\n  post:\n    $prepend: [x]\n    $append: [y]\n",
        );
        assert_eq!(names(&out["scripts"]["post"]), ["x", "a", "b", "y"]);
    }

    #[test]
    fn later_fragment_prepend_lands_in_front() {
        let mut acc = yaml("scripts:\n  post: [a]\n");
        merge_into(
            &mut acc,
            yaml("scripts:\n  post:\n    $prepend: [b]\n"),
            "f1",
        )
        .unwrap();
        merge_into(
            &mut acc,
            yaml("scripts:\n  post:\n    $prepend: [c]\n"),
            "f2",
        )
        .unwrap();
        assert_eq!(names(&acc["scripts"]["post"]), ["c", "b", "a"]);
    }

    #[test]
    fn list_ops_combine_remove_with_prepend_and_append() {
        let out = merged(
            "os:\n  packages:\n    install: [a, b, c]\n",
            "os:\n  packages:\n    install:\n      $remove: [b]\n      $prepend: [x]\n      $append: [y]\n",
        );
        assert_eq!(
            names(&out["os"]["packages"]["install"]),
            ["x", "a", "c", "y"]
        );
    }

    #[test]
    fn replace_combined_with_prepend_is_conflicting() {
        let mut acc = yaml("x: [a]\n");
        let err = merge_into(
            &mut acc,
            yaml("x:\n  $replace: [b]\n  $prepend: [c]\n"),
            "f",
        )
        .unwrap_err();
        assert!(
            matches!(err, ConfigError::ConflictingDirectives { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn prepend_with_a_non_list_body_is_a_shape_error() {
        let mut acc = yaml("x: [a]\n");
        let err = merge_into(&mut acc, yaml("x:\n  $prepend: scalar\n"), "f").unwrap_err();
        assert!(
            matches!(
                err,
                ConfigError::DirectiveShape {
                    directive: "$prepend",
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    // ───────────────────────────── $select ─────────────────────────────

    #[test]
    fn select_is_a_reserved_directive_error() {
        let mut acc = yaml("base: {}\n");
        let err = merge_into(
            &mut acc,
            yaml("base:\n  $select:\n    arch:\n      amd64: x\n"),
            "f",
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                ConfigError::ReservedDirective {
                    directive: "$select",
                    ..
                }
            ),
            "got {err:?}"
        );
    }
}
