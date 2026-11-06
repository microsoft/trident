//! `${…}` interpolation of axis values and parameters into scalar strings.
//!
//! `meta/docs/2026-06-22-image-definitions.md` §8: `params` are a convenience for interpolating axis values into
//! scalars and naming a derived constant. There is exactly one construct — `${name}` substitution
//! into a string — and it only ever produces values, never structure. A name resolves to a matrix
//! axis value or to another parameter (which may itself interpolate, resolved here with cycle
//! detection).

use std::collections::BTreeMap;

use indexmap::IndexMap;
use serde_yaml_ng::Value;

use crate::error::ConfigError;

const OPEN: &str = "${";
const CLOSE: char = '}';

/// Interpolation names under this namespace are **deferred**: `interpolate` leaves `${inputs.<name>}`
/// verbatim (rather than erroring on an undefined var) so a later workspace-aware pass can resolve
/// them to producer artifact paths (`meta/docs/2026-09-09-inter-image-dependencies.md` §2.1).
const DEFERRED_NAMESPACE: &str = "inputs.";

/// A flat lookup of every interpolation name: matrix axis values plus fully-resolved parameters.
pub(crate) type Context = BTreeMap<String, String>;

/// Resolve `params` (raw strings that may reference axes or other params) against the cell's `axes`,
/// returning a context usable to interpolate the merged config tree.
pub(crate) fn build_context(
    axes: &BTreeMap<String, String>,
    params: &IndexMap<String, String>,
) -> Result<Context, ConfigError> {
    let mut resolved = axes.clone();
    let mut stack = Vec::new();
    for name in params.keys() {
        resolve_param(name, params, &mut resolved, &mut stack)?;
    }
    Ok(resolved)
}

/// Interpolate every `${…}` occurrence in every string scalar of `value`, in place.
pub(crate) fn interpolate_tree(value: &mut Value, context: &Context) -> Result<(), ConfigError> {
    match value {
        Value::String(text) if text.contains(OPEN) => {
            *text = interpolate(text, &mut |name| context.get(name).cloned())?;
        }
        Value::Sequence(items) => {
            for item in items.iter_mut() {
                interpolate_tree(item, context)?;
            }
        }
        Value::Mapping(map) => {
            for (_key, item) in map.iter_mut() {
                interpolate_tree(item, context)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn resolve_param(
    name: &str,
    params: &IndexMap<String, String>,
    resolved: &mut Context,
    stack: &mut Vec<String>,
) -> Result<(), ConfigError> {
    if resolved.contains_key(name) || !params.contains_key(name) {
        return Ok(());
    }
    if stack.iter().any(|seen| seen == name) {
        stack.push(name.to_owned());
        return Err(ConfigError::ParamCycle {
            chain: stack.join(" → "),
        });
    }
    let raw = params.get(name).cloned().unwrap_or_default();
    stack.push(name.to_owned());
    for referenced in references(&raw) {
        resolve_param(&referenced, params, resolved, stack)?;
    }
    let value = interpolate(&raw, &mut |lookup| resolved.get(lookup).cloned())?;
    stack.pop();
    resolved.insert(name.to_owned(), value);
    Ok(())
}

/// Replace every `${name}` in `input`, resolving each name through `lookup`. An unresolved name is a
/// hard error; an unterminated `${` is rejected (it would silently swallow the rest of the string).
fn interpolate(
    input: &str,
    lookup: &mut impl FnMut(&str) -> Option<String>,
) -> Result<String, ConfigError> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        let after = &rest[start + OPEN.len()..];
        let Some(end) = after.find(CLOSE) else {
            return Err(ConfigError::UnterminatedInterpolation {
                text: input.to_owned(),
            });
        };
        let name = &after[..end];
        match lookup(name) {
            Some(value) => out.push_str(&value),
            // Deferred namespace: re-emit `${inputs.<name>}` verbatim for a later resolution pass.
            None if name.starts_with(DEFERRED_NAMESPACE) => {
                out.push_str(OPEN);
                out.push_str(name);
                out.push(CLOSE);
            }
            None => {
                return Err(ConfigError::UndefinedVar {
                    name: name.to_owned(),
                    at: input.to_owned(),
                });
            }
        }
        rest = &after[end + CLOSE.len_utf8()..];
    }
    out.push_str(rest);
    Ok(out)
}

fn references(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = input;
    while let Some(start) = rest.find(OPEN) {
        let after = &rest[start + OPEN.len()..];
        let Some(end) = after.find(CLOSE) else { break };
        out.push(after[..end].to_owned());
        rest = &after[end + CLOSE.len_utf8()..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axes(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn params(pairs: &[(&str, &str)]) -> IndexMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn resolves_axis_value() {
        let ctx = build_context(&axes(&[("arch", "amd64")]), &params(&[])).unwrap();
        assert_eq!(ctx.get("arch").map(String::as_str), Some("amd64"));
    }

    #[test]
    fn resolves_param_referencing_another_param() {
        let ctx = build_context(
            &axes(&[]),
            &params(&[("grubEfiPkg", "grub2-efi-${efiArch}"), ("efiArch", "x64")]),
        )
        .unwrap();
        assert_eq!(
            ctx.get("grubEfiPkg").map(String::as_str),
            Some("grub2-efi-x64")
        );
    }

    #[test]
    fn resolves_param_referencing_axes() {
        let ctx = build_context(
            &axes(&[("release", "3.0"), ("variant", "grub")]),
            &params(&[("osTag", "${release}-${variant}")]),
        )
        .unwrap();
        assert_eq!(ctx.get("osTag").map(String::as_str), Some("3.0-grub"));
    }

    #[test]
    fn defers_the_inputs_namespace() {
        // `${inputs.*}` must survive interpolation verbatim for the later resolution pass, rather
        // than erroring as an undefined var.
        let ctx = build_context(&axes(&[("arch", "amd64")]), &params(&[])).unwrap();
        let mut value: Value =
            serde_yaml_ng::from_str("source: \"${inputs.payload}\"\nplatform: linux/${arch}\n")
                .unwrap();
        interpolate_tree(&mut value, &ctx).unwrap();
        assert_eq!(value["source"].as_str(), Some("${inputs.payload}"));
        assert_eq!(value["platform"].as_str(), Some("linux/amd64"));
    }

    #[test]
    fn undefined_variable_is_an_error() {
        let err = build_context(&axes(&[]), &params(&[("p", "${missing}")])).unwrap_err();
        assert!(
            matches!(err, ConfigError::UndefinedVar { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn parameter_cycle_is_detected() {
        let err = build_context(&axes(&[]), &params(&[("a", "${b}"), ("b", "${a}")])).unwrap_err();
        assert!(matches!(err, ConfigError::ParamCycle { .. }), "got {err:?}");
    }

    #[test]
    fn interpolates_a_value_tree() {
        let ctx = build_context(&axes(&[("arch", "amd64")]), &params(&[])).unwrap();
        let mut value: Value =
            serde_yaml_ng::from_str("platform: linux/${arch}\nplain: keep\n").unwrap();
        interpolate_tree(&mut value, &ctx).unwrap();
        assert_eq!(value["platform"].as_str(), Some("linux/amd64"));
        assert_eq!(value["plain"].as_str(), Some("keep"));
    }
}
