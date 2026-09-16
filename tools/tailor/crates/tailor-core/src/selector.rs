//! Cell selection — narrow a target's expanded matrix to a specific cell or an axis-delimited slice
//! (`meta/docs/2026-06-22-design.md` §11). Selecting a single cell per build is the common ADO path, so the surface
//! is deliberately small: pin some axes (`axis=value`), or name exact cells by slug.

use std::collections::BTreeMap;

use crate::{domain::Cell, error::CoreError};

const PAIR_SEPARATOR: char = ',';
const KEY_VALUE_SEPARATOR: char = '=';

/// A filter over expanded cells: per-axis allowed values (AND across axes, OR within one axis) plus
/// an optional set of exact cell slugs. An empty selector matches every cell.
#[derive(Debug, Clone, Default)]
pub struct Selector {
    axes: BTreeMap<String, Vec<String>>,
    slugs: Vec<String>,
}

impl Selector {
    /// Build a selector from CLI inputs: `select` entries (`axis=value[,axis=value…]`, repeatable),
    /// exact cell `slugs`, and any `--arch` values (folded in as the `arch` axis).
    pub fn parse(
        select: &[String],
        slugs: &[String],
        arches: &[String],
    ) -> Result<Self, CoreError> {
        let mut constraints: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for entry in select {
            for pair in entry.split(PAIR_SEPARATOR).filter(|p| !p.is_empty()) {
                let (name, value) = pair.split_once(KEY_VALUE_SEPARATOR).ok_or_else(|| {
                    CoreError::SelectorSyntax {
                        entry: pair.to_owned(),
                    }
                })?;
                if name.is_empty() || value.is_empty() {
                    return Err(CoreError::SelectorSyntax {
                        entry: pair.to_owned(),
                    });
                }
                push_unique(
                    constraints.entry(name.to_owned()).or_default(),
                    value.to_owned(),
                );
            }
        }
        for arch in arches {
            push_unique(
                constraints.entry("arch".to_owned()).or_default(),
                arch.clone(),
            );
        }
        Ok(Self {
            axes: constraints,
            slugs: slugs.to_vec(),
        })
    }

    /// Whether any constraint is set (an empty selector selects everything).
    pub fn is_empty(&self) -> bool {
        self.axes.is_empty() && self.slugs.is_empty()
    }

    /// The axis names this selector constrains (for validation against a target's declared axes).
    pub fn axis_names(&self) -> impl Iterator<Item = &str> {
        self.axes.keys().map(String::as_str)
    }

    /// Whether `cell` satisfies the selector: its slug is allowed (if slugs are given) and every
    /// constrained axis holds one of the requested values.
    pub fn matches(&self, cell: &Cell) -> bool {
        if !self.slugs.is_empty() && !self.slugs.iter().any(|slug| slug == cell.slug.as_ref()) {
            return false;
        }
        self.axes.iter().all(|(axis, values)| {
            cell.axes
                .get(axis)
                .is_some_and(|actual| values.iter().any(|v| v == actual))
        })
    }

    /// Whether this selector **specifically requests** a `skip`ped cell, so it should be kept despite
    /// its `skip` (`meta/docs/2026-07-22-fragment-skip.md`): either it names the cell by slug
    /// (`--cell`), or it pins (via `-s`) one of the cell's `skip_pins` `(axis, value)` coordinates. A
    /// selector that merely *matches* the cell without pinning a skip coordinate (e.g. `-s arch=…`)
    /// does **not** count.
    pub fn requests_skip_cell(&self, cell: &Cell) -> bool {
        if self.slugs.iter().any(|slug| slug == cell.slug.as_ref()) {
            return true;
        }
        cell.skip_pins.iter().any(|(axis, value)| {
            self.axes
                .get(axis)
                .is_some_and(|values| values.iter().any(|v| v == value))
        })
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use indexmap::IndexMap;
    use serde_yaml_ng::Value;

    use tailor_config::{
        Arch, BaseImageCatalogue, BaseSource, ImageDefinition, OutputArtifactsPolicy, OutputFormat,
        OutputSpec,
    };

    use crate::domain::{CellSlug, Target};

    fn cell(slug: &str, axes: &[(&str, &str)]) -> Cell {
        let definition = ImageDefinition {
            name: "img".to_owned(),
            toolchain: None,
            skip: false,
            matrix: Some(IndexMap::new()),
            selectors: None,
            outputs: None,
            base: None,
            features: vec![],
            params: IndexMap::new(),
            rpm_sources: vec![],
            tools_dir: None,
            operation: None,
            output_artifacts: None,
            signing: None,
            extra_dependencies: vec![],
            depends_on: vec![],
            inputs: vec![],
            extra_params: vec![],
            config: None,
        };
        let target = Arc::new(Target {
            definition,
            dir: ".".into(),
            default_outputs: vec![],
            output_artifacts: OutputArtifactsPolicy::default(),
            root: ".".into(),
            base_images: BaseImageCatalogue::default(),
            tools_dir_sources: Vec::new(),
        });
        Cell {
            target,
            axes: axes
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            arch: Arch::Amd64,
            output: OutputSpec {
                format: OutputFormat::Cosi,
                cosi_compression_level: None,
                compression: None,
                name: None,
            },
            slug: CellSlug(slug.to_owned()),
            ic_config: Value::Null,
            base: BaseSource::Path {
                path: "b".into(),
                arch: None,
            },
            base_image: None,
            rpm_sources: vec![],
            extra_params: vec![],
            input_deps: vec![],
            tools_dir: None,
            skip: false,
            skip_pins: Vec::new(),
        }
    }

    #[test]
    fn empty_selector_matches_everything() {
        let sel = Selector::default();
        assert!(sel.is_empty());
        assert!(sel.matches(&cell("img_grub_amd64_cosi", &[("variant", "grub")])));
    }

    #[test]
    fn pinning_all_axes_selects_one_cell() {
        let sel = Selector::parse(&["variant=grub,arch=amd64".to_owned()], &[], &[]).unwrap();
        assert!(sel.matches(&cell("a", &[("variant", "grub"), ("arch", "amd64")])));
        assert!(!sel.matches(&cell("b", &[("variant", "grub"), ("arch", "arm64")])));
    }

    #[test]
    fn pinning_one_axis_selects_a_slice() {
        let sel = Selector::parse(&["arch=amd64".to_owned()], &[], &[]).unwrap();
        assert!(sel.matches(&cell("a", &[("variant", "grub"), ("arch", "amd64")])));
        assert!(sel.matches(&cell("b", &[("variant", "vm-img"), ("arch", "amd64")])));
        assert!(!sel.matches(&cell("c", &[("variant", "grub"), ("arch", "arm64")])));
    }

    #[test]
    fn multiple_values_for_one_axis_are_ored() {
        let sel = Selector::parse(&["release=3.0,release=4.0".to_owned()], &[], &[]).unwrap();
        assert!(sel.matches(&cell("a", &[("release", "3.0")])));
        assert!(sel.matches(&cell("b", &[("release", "4.0")])));
        assert!(!sel.matches(&cell("c", &[("release", "5.0")])));
    }

    #[test]
    fn slug_selection_is_exact() {
        let sel = Selector::parse(&[], &["img_grub_amd64_cosi".to_owned()], &[]).unwrap();
        assert!(sel.matches(&cell("img_grub_amd64_cosi", &[("variant", "grub")])));
        assert!(!sel.matches(&cell("img_grub_arm64_cosi", &[("variant", "grub")])));
    }

    #[test]
    fn arch_flag_folds_into_the_arch_axis() {
        let sel = Selector::parse(&[], &[], &["amd64".to_owned()]).unwrap();
        assert!(sel.matches(&cell("a", &[("arch", "amd64")])));
        assert!(!sel.matches(&cell("b", &[("arch", "arm64")])));
    }

    #[test]
    fn malformed_pair_is_an_error() {
        assert!(Selector::parse(&["variant".to_owned()], &[], &[]).is_err());
        assert!(Selector::parse(&["=grub".to_owned()], &[], &[]).is_err());
        assert!(Selector::parse(&["variant=".to_owned()], &[], &[]).is_err());
    }
}
