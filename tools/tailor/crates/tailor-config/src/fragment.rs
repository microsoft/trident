//! Fragments and conditional composition (`meta/docs/2026-06-22-image-definitions.md` §6, §9.1).
//!
//! A fragment is a partial document: tailor fields at the top level (`base`, `outputs`, `params`,
//! `rpmSources`, an inline `match`) plus an opaque `config:` delta. The recommended layout encodes
//! the match in the path — `by-<axis>/<value>.yaml` applies when that axis equals that value, and
//! `by-feature/<name>.yaml` when that feature is enabled — so single-condition fragments carry no
//! `match:` boilerplate. An inline `match:` is ANDed with the path predicate.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use indexmap::IndexMap;
use serde::Deserialize;
use serde_yaml_ng::Value;

use crate::{
    error::ConfigError,
    schema::{AxisValues, ExtraParam},
    types::ParamValue,
};

const FRAGMENT_DIR_PREFIX: &str = "by-";
const FEATURE_AXIS: &str = "feature";
const BASE_DOCUMENT: &str = "image.yaml";
const YAML_EXT: &str = "yaml";

/// The tailor-field deltas and opaque `config:` a fragment (or the base document) contributes.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Fragment {
    #[serde(default, rename = "match")]
    pub(crate) match_expr: Option<Match>,
    #[serde(default)]
    pub(crate) base: Option<Value>,
    #[serde(default)]
    pub(crate) outputs: Option<Value>,
    #[serde(default)]
    pub(crate) params: IndexMap<String, ParamValue>,
    #[serde(default)]
    pub(crate) rpm_sources: Vec<PathBuf>,
    /// Extra IC command-line flags this fragment contributes; concatenated across matched fragments
    /// (base → most-specific), mirroring `rpmSources`.
    #[serde(default)]
    pub(crate) extra_params: Vec<ExtraParam>,
    /// Drop cells this fragment applies to from bulk selection unless the run pins the fragment's
    /// value or names the cell (`meta/docs/2026-07-22-fragment-skip.md`). Mergeable last-wins; a
    /// more-specific fragment may set `false` to un-skip.
    #[serde(default)]
    pub(crate) skip: Option<bool>,
    #[serde(default)]
    pub(crate) config: Option<Value>,
}

/// A boolean condition over a cell's axis values and the image's enabled features
/// (`meta/docs/2026-06-22-image-definitions.md` §6).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum Match {
    All { all: Vec<Match> },
    Any { any: Vec<Match> },
    Not { not: Box<Match> },
    Leaf(IndexMap<String, MatchValue>),
}

/// A `match` leaf value: a single value (equality) or a list (set membership).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum MatchValue {
    One(String),
    Many(Vec<String>),
}

impl Match {
    pub(crate) fn evaluate(&self, axes: &BTreeMap<String, String>, features: &[String]) -> bool {
        match self {
            Match::All { all } => all.iter().all(|m| m.evaluate(axes, features)),
            Match::Any { any } => any.iter().any(|m| m.evaluate(axes, features)),
            Match::Not { not } => !not.evaluate(axes, features),
            Match::Leaf(leaf) => leaf
                .iter()
                .all(|(key, value)| leaf_matches(key, value, axes, features)),
        }
    }
}

fn leaf_matches(
    key: &str,
    value: &MatchValue,
    axes: &BTreeMap<String, String>,
    features: &[String],
) -> bool {
    if key == FEATURE_AXIS {
        return match value {
            MatchValue::One(name) => features.iter().any(|f| f == name),
            MatchValue::Many(names) => names.iter().any(|name| features.iter().any(|f| f == name)),
        };
    }
    let Some(actual) = axes.get(key) else {
        return false;
    };
    match value {
        MatchValue::One(expected) => actual == expected,
        MatchValue::Many(expected) => expected.iter().any(|candidate| candidate == actual),
    }
}

/// The path-derived predicate of a fragment file: a conjunction of per-axis value-sets (the cell's
/// value on each named axis must be in that axis's set), a feature flag, or the always-true base.
/// A single-axis disjunction (`by-mode/dev+test.yaml`) is one clause with several values; a multi-axis
/// composite (`by-boot+verity/uki+root.yaml`) is several clauses with one value each.
#[derive(Debug, Clone)]
enum Predicate {
    Always,
    Conjunction(Vec<AxisClause>),
    Feature { name: String },
}

/// One clause of a path predicate: the cell's value for `axis` must be one of `values`.
#[derive(Debug, Clone)]
struct AxisClause {
    axis: String,
    values: Vec<String>,
}

/// Merge-precedence key (ascending = applied earlier = lower precedence), per
/// `meta/docs/2026-06-29-directive-design.md` §2: more axes (`arity`) apply later; among the same axes, broader
/// value-sets apply earlier so a narrower fragment wins; `axes`/`values` give a deterministic order.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Order {
    arity: usize,
    axes: Vec<usize>,
    breadth: usize,
    values: Vec<String>,
}

impl Order {
    /// Apply order: arity asc, axis indices asc, **breadth desc**, values asc.
    fn cmp_key(&self, other: &Self) -> std::cmp::Ordering {
        self.arity
            .cmp(&other.arity)
            .then_with(|| self.axes.cmp(&other.axes))
            .then_with(|| other.breadth.cmp(&self.breadth))
            .then_with(|| self.values.cmp(&other.values))
    }
}

/// A fragment with its source label and resolved predicate, ready to test against each cell.
#[derive(Debug)]
pub(crate) struct LoadedFragment {
    pub(crate) label: String,
    predicate: Predicate,
    pub(crate) doc: Fragment,
}

impl LoadedFragment {
    /// Whether this fragment applies to a cell: its path predicate AND any inline `match`.
    pub(crate) fn applies(&self, axes: &BTreeMap<String, String>, features: &[String]) -> bool {
        let path_ok = match &self.predicate {
            Predicate::Always => true,
            Predicate::Conjunction(clauses) => clauses.iter().all(|clause| {
                axes.get(&clause.axis)
                    .is_some_and(|actual| clause.values.iter().any(|v| v == actual))
            }),
            Predicate::Feature { name } => features.iter().any(|f| f == name),
        };
        path_ok
            && self
                .doc
                .match_expr
                .as_ref()
                .is_none_or(|m| m.evaluate(axes, features))
    }

    /// The `(axis, value)` equality coordinates of this fragment's path predicate — the coordinates a
    /// `-s` selector must pin to "specifically request" a `skip`ped cell this fragment applies to
    /// (`meta/docs/2026-07-22-fragment-skip.md`). Feature/`match`-only fragments yield none.
    pub(crate) fn pins(&self) -> Vec<(String, String)> {
        match &self.predicate {
            Predicate::Conjunction(clauses) => clauses
                .iter()
                .flat_map(|clause| {
                    clause
                        .values
                        .iter()
                        .map(|value| (clause.axis.clone(), value.clone()))
                })
                .collect(),
            Predicate::Always | Predicate::Feature { .. } => Vec::new(),
        }
    }

    /// A human-readable description of why this fragment applies (for `tailor explain`).
    pub(crate) fn reason(&self) -> String {
        match &self.predicate {
            Predicate::Always => "base".to_owned(),
            Predicate::Feature { name } => format!("feature {name}"),
            Predicate::Conjunction(clauses) => clauses
                .iter()
                .map(|c| {
                    if c.values.len() == 1 {
                        format!("{}={}", c.axis, c.values[0])
                    } else {
                        format!("{} ∈ {{{}}}", c.axis, c.values.join(","))
                    }
                })
                .collect::<Vec<_>>()
                .join(" ∧ "),
        }
    }
}

/// Discover the base document and every `by-*/<value>.yaml` fragment for an image, in apply order:
/// the base document first, then fragments sorted by [`Order`] — single-axis fragments in
/// matrix axis-declaration order (broader disjunctions before narrower ones), then multi-axis
/// composites, then feature fragments. Apply order is merge precedence, so authors control it by
/// ordering axes in the matrix. Validates that every fragment axis/value is declared (closed-axis
/// check) and that composite/disjunction paths are well-formed (`meta/docs/2026-06-29-directive-design.md` §2).
pub(crate) fn discover(
    image_dir: &Path,
    matrix: Option<&AxisValues>,
    features: &[String],
) -> Result<Vec<LoadedFragment>, ConfigError> {
    let mut fragments = vec![LoadedFragment {
        label: BASE_DOCUMENT.to_owned(),
        predicate: Predicate::Always,
        doc: parse_fragment(&image_dir.join(BASE_DOCUMENT))?,
    }];

    let mut local = Vec::new();
    for axis_dir in read_dir_sorted(image_dir)? {
        let dir_name = axis_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let Some(dir_token) = dir_name.strip_prefix(FRAGMENT_DIR_PREFIX) else {
            continue;
        };
        if !axis_dir.is_dir() {
            continue;
        }
        for file in read_dir_sorted(&axis_dir)? {
            if file.extension().and_then(|e| e.to_str()) != Some(YAML_EXT) {
                continue;
            }
            let stem = file
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let label = format!(
                "{dir_name}/{}",
                file.file_name().unwrap_or_default().to_string_lossy()
            );
            let (predicate, order) = build_predicate(dir_token, &stem, &label, matrix, features)?;
            local.push((order, label, predicate, parse_fragment(&file)?));
        }
    }
    local.sort_by(|a, b| a.0.cmp_key(&b.0).then_with(|| a.1.cmp(&b.1)));
    fragments.extend(
        local
            .into_iter()
            .map(|(_, label, predicate, doc)| LoadedFragment {
                label,
                predicate,
                doc,
            }),
    );
    Ok(fragments)
}

/// Parse and validate one `by-<dir_token>/<stem>.yaml` fragment path into its predicate and [`Order`].
/// `dir_token` is the part after `by-` (one axis, several `+`-joined axes, or `feature`); `stem` is the
/// file name without extension (the axis value(s), `+`-joined).
fn build_predicate(
    dir_token: &str,
    stem: &str,
    label: &str,
    matrix: Option<&AxisValues>,
    features: &[String],
) -> Result<(Predicate, Order), ConfigError> {
    let axis_tokens: Vec<&str> = dir_token.split('+').collect();

    if axis_tokens.as_slice() == [FEATURE_AXIS] {
        let Some(feature_index) = features.iter().position(|f| f == stem) else {
            return Err(ConfigError::UnknownFragmentValue {
                axis: FEATURE_AXIS.to_owned(),
                value: stem.to_owned(),
                file: label.to_owned(),
            });
        };
        let axis_count = matrix.map_or(0, IndexMap::len);
        return Ok((
            Predicate::Feature {
                name: stem.to_owned(),
            },
            Order {
                arity: 1,
                axes: vec![axis_count + feature_index],
                breadth: 1,
                values: vec![stem.to_owned()],
            },
        ));
    }
    if axis_tokens.contains(&FEATURE_AXIS) {
        return Err(ConfigError::InvalidFragmentPath {
            file: label.to_owned(),
            reason: "the `feature` axis cannot be combined with other axes".to_owned(),
        });
    }

    let value_tokens: Vec<&str> = stem.split('+').collect();

    if axis_tokens.len() == 1 {
        // Single axis, one or more values (a disjunction when several).
        let axis = axis_tokens[0];
        let (axis_index, _, declared) = matrix.and_then(|m| m.get_full(axis)).ok_or_else(|| {
            ConfigError::UnknownFragmentAxis {
                axis: axis.to_owned(),
            }
        })?;
        let mut last = None;
        for value in &value_tokens {
            let Some(decl_index) = declared.iter().position(|d| d == value) else {
                return Err(ConfigError::UnknownFragmentValue {
                    axis: axis.to_owned(),
                    value: (*value).to_owned(),
                    file: label.to_owned(),
                });
            };
            if last.is_some_and(|prev| decl_index <= prev) {
                return Err(ConfigError::InvalidFragmentPath {
                    file: label.to_owned(),
                    reason: format!(
                        "values must be distinct and in `{axis}`'s declared order (e.g. `{}`)",
                        canonical_values(&value_tokens, declared)
                    ),
                });
            }
            last = Some(decl_index);
        }
        let values: Vec<String> = value_tokens.iter().map(|s| (*s).to_owned()).collect();
        Ok((
            Predicate::Conjunction(vec![AxisClause {
                axis: axis.to_owned(),
                values: values.clone(),
            }]),
            Order {
                arity: 1,
                axes: vec![axis_index],
                breadth: values.len(),
                values,
            },
        ))
    } else {
        // Multi-axis conjunction: exactly one value per axis, positionally.
        if axis_tokens.len() != value_tokens.len() {
            return Err(ConfigError::InvalidFragmentPath {
                file: label.to_owned(),
                reason: format!(
                    "{} axes (`{dir_token}`) but {} values (`{stem}`); a composite path names exactly one value per axis",
                    axis_tokens.len(),
                    value_tokens.len()
                ),
            });
        }
        let mut clauses = Vec::with_capacity(axis_tokens.len());
        let mut axes = Vec::with_capacity(axis_tokens.len());
        let mut last = None;
        for (axis, value) in axis_tokens.iter().zip(&value_tokens) {
            let (axis_index, _, declared) =
                matrix.and_then(|m| m.get_full(*axis)).ok_or_else(|| {
                    ConfigError::UnknownFragmentAxis {
                        axis: (*axis).to_owned(),
                    }
                })?;
            if last.is_some_and(|prev| axis_index <= prev) {
                return Err(ConfigError::InvalidFragmentPath {
                    file: label.to_owned(),
                    reason: "axes must be distinct and in the matrix's declared order".to_owned(),
                });
            }
            last = Some(axis_index);
            if !declared.iter().any(|d| d == value) {
                return Err(ConfigError::UnknownFragmentValue {
                    axis: (*axis).to_owned(),
                    value: (*value).to_owned(),
                    file: label.to_owned(),
                });
            }
            clauses.push(AxisClause {
                axis: (*axis).to_owned(),
                values: vec![(*value).to_owned()],
            });
            axes.push(axis_index);
        }
        let arity = axis_tokens.len();
        Ok((
            Predicate::Conjunction(clauses),
            Order {
                arity,
                axes,
                breadth: arity,
                values: value_tokens.iter().map(|s| (*s).to_owned()).collect(),
            },
        ))
    }
}

/// The canonical `+`-joined spelling of `values` (the axis's declared order), for error suggestions.
fn canonical_values(values: &[&str], declared: &[String]) -> String {
    let mut sorted: Vec<&str> = values
        .iter()
        .copied()
        .filter(|v| declared.iter().any(|d| d == v))
        .collect();
    sorted.sort_by_key(|v| declared.iter().position(|d| d == v).unwrap_or(usize::MAX));
    sorted.dedup();
    sorted.join("+")
}

fn parse_fragment(path: &Path) -> Result<Fragment, ConfigError> {
    let text = fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    serde_yaml_ng::from_str(&text).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

fn read_dir_sorted(dir: &Path) -> Result<Vec<PathBuf>, ConfigError> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir).map_err(|source| ConfigError::Read {
        path: dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| ConfigError::Read {
            path: dir.to_path_buf(),
            source,
        })?;
        entries.push(entry.path());
    }
    entries.sort();
    Ok(entries)
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

    fn parse_match(yaml: &str) -> Match {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

    #[test]
    fn equality_leaf_matches_axis_value() {
        let m = parse_match("release: '4.0'");
        assert!(m.evaluate(&axes(&[("release", "4.0")]), &[]));
        assert!(!m.evaluate(&axes(&[("release", "3.0")]), &[]));
    }

    #[test]
    fn set_membership_leaf() {
        let m = parse_match("variant: [root-verity, usr-verity]");
        assert!(m.evaluate(&axes(&[("variant", "usr-verity")]), &[]));
        assert!(!m.evaluate(&axes(&[("variant", "grub")]), &[]));
    }

    #[test]
    fn all_any_not_combinators() {
        let all = parse_match("all: [{release: '3.0'}, {variant: grub}]");
        assert!(all.evaluate(&axes(&[("release", "3.0"), ("variant", "grub")]), &[]));
        assert!(!all.evaluate(&axes(&[("release", "3.0"), ("variant", "vm-img")]), &[]));

        let any = parse_match("any: [{variant: grub}, {phase: update}]");
        assert!(any.evaluate(&axes(&[("variant", "grub"), ("phase", "base")]), &[]));

        let not = parse_match("not: {release: '4.0'}");
        assert!(not.evaluate(&axes(&[("release", "3.0")]), &[]));
        assert!(!not.evaluate(&axes(&[("release", "4.0")]), &[]));
    }

    #[test]
    fn feature_leaf_consults_enabled_features() {
        let m = parse_match("feature: pcrlock-static-files");
        assert!(m.evaluate(&axes(&[]), &["pcrlock-static-files".to_owned()]));
        assert!(!m.evaluate(&axes(&[]), &[]));
    }

    #[test]
    fn compound_feature_and_release() {
        let m = parse_match("all: [{feature: pcrlock-static-files}, {release: '3.0'}]");
        let feats = ["pcrlock-static-files".to_owned()];
        assert!(m.evaluate(&axes(&[("release", "3.0")]), &feats));
        assert!(!m.evaluate(&axes(&[("release", "4.0")]), &feats));
    }

    // ───────────────────────── composite / disjunction fragment paths ─────────────────────────

    use tempfile::TempDir;

    const MATRIX: &str = "arch: [amd64, arm64]\nboot: [grub, uki]\nverity: [none, root, usr]\nmode: [dev, test, prod]\n";

    fn axisvals(yaml: &str) -> AxisValues {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

    /// Build a temp image dir containing `image.yaml` plus the given `by-*` fragment files.
    fn image(files: &[&str]) -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(BASE_DOCUMENT), "config: {}\n").unwrap();
        for rel in files {
            let path = dir.path().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "config: {}\n").unwrap();
        }
        dir
    }

    fn discover_labels(dir: &TempDir) -> Vec<String> {
        let frags = discover(dir.path(), Some(&axisvals(MATRIX)), &[]).unwrap();
        frags.into_iter().skip(1).map(|f| f.label).collect()
    }

    fn find<'a>(frags: &'a [LoadedFragment], label: &str) -> &'a LoadedFragment {
        frags.iter().find(|f| f.label == label).unwrap()
    }

    #[test]
    fn composite_conjunction_applies_to_the_pair_only() {
        let dir = image(&["by-boot+verity/uki+root.yaml"]);
        let frags = discover(dir.path(), Some(&axisvals(MATRIX)), &[]).unwrap();
        let frag = find(&frags, "by-boot+verity/uki+root.yaml");
        assert!(frag.applies(&axes(&[("boot", "uki"), ("verity", "root")]), &[]));
        assert!(!frag.applies(&axes(&[("boot", "uki"), ("verity", "usr")]), &[]));
        assert!(!frag.applies(&axes(&[("boot", "grub"), ("verity", "root")]), &[]));
    }

    #[test]
    fn single_axis_disjunction_applies_to_any_listed_value() {
        let dir = image(&["by-mode/dev+test.yaml"]);
        let frags = discover(dir.path(), Some(&axisvals(MATRIX)), &[]).unwrap();
        let frag = find(&frags, "by-mode/dev+test.yaml");
        assert!(frag.applies(&axes(&[("mode", "dev")]), &[]));
        assert!(frag.applies(&axes(&[("mode", "test")]), &[]));
        assert!(!frag.applies(&axes(&[("mode", "prod")]), &[]));
    }

    #[test]
    fn precedence_orders_by_arity_then_axis_then_breadth() {
        let dir = image(&[
            "by-arch/amd64.yaml",
            "by-boot/grub.yaml",
            "by-boot/uki.yaml",
            "by-verity/root.yaml",
            "by-mode/dev.yaml",
            "by-mode/dev+test.yaml",
            "by-boot+verity/uki+root.yaml",
        ]);
        assert_eq!(
            discover_labels(&dir),
            [
                "by-arch/amd64.yaml", // axis 0
                "by-boot/grub.yaml",  // axis 1
                "by-boot/uki.yaml",
                "by-verity/root.yaml",          // axis 2
                "by-mode/dev+test.yaml",        // axis 3, broader (breadth 2) first
                "by-mode/dev.yaml",             // axis 3, narrower (breadth 1) → wins
                "by-boot+verity/uki+root.yaml"  // arity 2, after every single-axis fragment
            ]
        );
    }

    #[test]
    fn unknown_fragment_axis_is_rejected() {
        let dir = image(&["by-nope/x.yaml"]);
        let err = discover(dir.path(), Some(&axisvals(MATRIX)), &[]).unwrap_err();
        assert!(
            matches!(err, ConfigError::UnknownFragmentAxis { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn undeclared_value_is_rejected() {
        let dir = image(&["by-arch/x86.yaml"]);
        let err = discover(dir.path(), Some(&axisvals(MATRIX)), &[]).unwrap_err();
        assert!(
            matches!(err, ConfigError::UnknownFragmentValue { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn composite_arity_mismatch_is_rejected() {
        let dir = image(&["by-boot+verity/uki.yaml"]);
        let err = discover(dir.path(), Some(&axisvals(MATRIX)), &[]).unwrap_err();
        assert!(
            matches!(&err, ConfigError::InvalidFragmentPath { reason, .. } if reason.contains("one value per axis")),
            "got {err:?}"
        );
    }

    #[test]
    fn composite_axes_out_of_declared_order_is_rejected() {
        let dir = image(&["by-verity+boot/root+uki.yaml"]);
        let err = discover(dir.path(), Some(&axisvals(MATRIX)), &[]).unwrap_err();
        assert!(
            matches!(&err, ConfigError::InvalidFragmentPath { reason, .. } if reason.contains("declared order")),
            "got {err:?}"
        );
    }

    #[test]
    fn disjunction_values_out_of_declared_order_is_rejected() {
        let dir = image(&["by-mode/test+dev.yaml"]);
        let err = discover(dir.path(), Some(&axisvals(MATRIX)), &[]).unwrap_err();
        assert!(
            matches!(&err, ConfigError::InvalidFragmentPath { reason, .. } if reason.contains("dev+test")),
            "got {err:?}"
        );
    }
}
