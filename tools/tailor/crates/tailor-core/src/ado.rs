//! The Azure DevOps matrix object for `tailor matrix --ado`/`--format ado` (`meta/docs/2026-06-29-ado-matrix.md`).
//! Each selected cell becomes one ADO leg: a sanitised key mapping to scalar-string variables a build
//! job keys off. tailor only knows about images, so the shape is fixed (slug → image fields) — no
//! projection config.

use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use crate::Cell;

/// Axis variables are prefixed so they can never collide with a reserved field; flat because ADO
/// rejects nested matrix values (`meta/docs/2026-06-29-ado-matrix.md` §4).
const AXIS_VAR_PREFIX: &str = "axis_";
/// ADO leg names allow only `[A-Za-z0-9_]` and must start with a letter; cap at 100 chars.
const MAX_LEG_KEY_LEN: usize = 100;
/// Lowercase-hex hash length appended to break sanitisation collisions.
const COLLISION_HASH_LEN: usize = 6;
/// Letter prepended when a sanitised key would not start with one (ADO requires a leading letter).
const LEG_KEY_LEAD: char = 'g';

/// A single ADO leg's variables: the slug-as-leg-key mapped to flat string variables.
type LegVars = BTreeMap<String, String>;

/// Build the ADO matrix object — `{ <legKey>: { var: string, … }, … }`, one leg per cell. Empty input
/// yields an empty object (the bare-`{}` case is `--format ado`; `--ado` rejects it upstream).
pub fn ado_matrix(cells: &[Cell]) -> BTreeMap<String, LegVars> {
    let mut legs = BTreeMap::new();
    let mut used = BTreeSet::new();
    for cell in cells {
        let slug = cell.slug.as_ref();
        let key = unique_leg_key(slug, &mut used);
        legs.insert(
            key,
            leg_vars(
                cell.target.name(),
                slug,
                cell.output.format.as_str(),
                cell.base_image.as_deref(),
                &cell.axes,
            ),
        );
    }
    legs
}

/// The reserved fields plus each axis as `axis_<name>` — all scalar strings, prefix-safe (§4). A cell
/// bound to a catalogue slot adds the reserved `baseImage` scalar so jobs can key off the slot name.
fn leg_vars(
    image: &str,
    slug: &str,
    format: &str,
    base_image: Option<&str>,
    axes: &BTreeMap<String, String>,
) -> LegVars {
    let mut vars = BTreeMap::new();
    vars.insert("image".to_owned(), image.to_owned());
    vars.insert("slug".to_owned(), slug.to_owned());
    vars.insert("format".to_owned(), format.to_owned());
    if let Some(name) = base_image {
        vars.insert("baseImage".to_owned(), name.to_owned());
    }
    for (axis, value) in axes {
        vars.insert(format!("{AXIS_VAR_PREFIX}{axis}"), value.clone());
    }
    vars
}

/// Sanitise then disambiguate: a collision (two slugs sharing a sanitised key) gets a short hash of the
/// raw slug, so leg keys stay unique and ADO-valid while `slug` keeps the verbatim value.
fn unique_leg_key(slug: &str, used: &mut BTreeSet<String>) -> String {
    let base = sanitize_leg_key(slug);
    if used.insert(base.clone()) {
        return base;
    }
    let suffix = short_hash(slug);
    let trimmed = truncate(&base, MAX_LEG_KEY_LEN - COLLISION_HASH_LEN - 1);
    let key = format!("{trimmed}_{suffix}");
    used.insert(key.clone());
    key
}

/// Replace every non-`[A-Za-z0-9_]` char with `_`, guarantee a leading letter, and cap the length.
fn sanitize_leg_key(slug: &str) -> String {
    let mut key: String = slug
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if !key.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        key.insert(0, LEG_KEY_LEAD);
    }
    truncate(&key, MAX_LEG_KEY_LEN)
}

fn truncate(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn short_hash(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    hex::encode(digest)[..COLLISION_HASH_LEN].to_owned()
}

/// Is `name` a valid ADO output variable name (`[A-Za-z_][A-Za-z0-9_]*`)? Keeps
/// `outputs['emit.<NAME>']` references clean (`meta/docs/2026-06-29-ado-matrix.md` §7).
pub fn is_valid_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_dash_and_dot_with_underscore() {
        assert_eq!(
            sanitize_leg_key("trident-mos_host_amd64.iso"),
            "trident_mos_host_amd64_iso"
        );
    }

    #[test]
    fn sanitize_prepends_letter_when_not_starting_with_one() {
        assert_eq!(sanitize_leg_key("3-arch"), "g3_arch");
        assert_eq!(sanitize_leg_key("_x"), "g_x");
    }

    #[test]
    fn sanitize_caps_length_at_100() {
        let long = "a".repeat(150);
        assert_eq!(sanitize_leg_key(&long).len(), MAX_LEG_KEY_LEN);
    }

    #[test]
    fn collisions_get_a_hash_suffix_and_stay_unique() {
        let mut used = BTreeSet::new();
        let first = unique_leg_key("a-b", &mut used);
        let second = unique_leg_key("a.b", &mut used);
        assert_eq!(first, "a_b");
        assert_ne!(first, second);
        assert!(second.starts_with("a_b_"));
    }

    #[test]
    fn leg_vars_are_flat_scalar_strings_with_prefixed_axes() {
        let axes = BTreeMap::from([
            ("runtime".to_owned(), "host".to_owned()),
            ("arch".to_owned(), "amd64".to_owned()),
        ]);
        let vars = leg_vars(
            "trident-mos",
            "trident-mos_host_amd64_iso",
            "iso",
            None,
            &axes,
        );
        assert_eq!(vars.get("image").unwrap(), "trident-mos");
        assert_eq!(vars.get("slug").unwrap(), "trident-mos_host_amd64_iso");
        assert_eq!(vars.get("format").unwrap(), "iso");
        assert_eq!(vars.get("axis_runtime").unwrap(), "host");
        assert_eq!(vars.get("axis_arch").unwrap(), "amd64");
        assert!(!vars.contains_key("axes"));
        assert!(!vars.contains_key("baseImage"));
    }

    #[test]
    fn leg_vars_include_base_image_only_when_bound_to_a_slot() {
        let no_axes = BTreeMap::new();
        let bound = leg_vars(
            "trident-mos",
            "trident-mos_iso",
            "iso",
            Some("baremetal"),
            &no_axes,
        );
        assert_eq!(bound.get("baseImage").unwrap(), "baremetal");
    }

    #[test]
    fn var_name_validation_matches_ado_rules() {
        assert!(is_valid_var_name("BUILD_MATRIX"));
        assert!(is_valid_var_name("_x1"));
        assert!(!is_valid_var_name(""));
        assert!(!is_valid_var_name("1bad"));
        assert!(!is_valid_var_name("has space"));
        assert!(!is_valid_var_name("has.dot"));
    }
}
