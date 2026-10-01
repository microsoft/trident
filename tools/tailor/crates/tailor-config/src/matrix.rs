use crate::{
    error::ConfigError,
    schema::{AxisValues, Selector, Selectors},
    types::OutputFormat,
};

const INCLUDE: &str = "include";
const EXCLUDE: &str = "exclude";

/// One expanded matrix cell: ordered `(axis, value)` pairs in matrix-declared order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxisTuple {
    pub values: Vec<(String, String)>,
}

impl AxisTuple {
    /// The axis-coordinate portion of a cell slug: values joined by `_`, in matrix order.
    pub fn coordinate(&self) -> String {
        self.values
            .iter()
            .map(|(_, value)| value.as_str())
            .collect::<Vec<_>>()
            .join("_")
    }

    /// The value pinned to `axis`, if this tuple includes it.
    pub fn get(&self, axis: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(name, _)| name == axis)
            .map(|(_, value)| value.as_str())
    }
}

/// The full output basename for a cell: `<image>_<axis values, in order>_<format>`
/// (`meta/docs/2026-06-22-design.md` §10).
pub fn cell_slug(image_name: &str, tuple: &AxisTuple, format: OutputFormat) -> String {
    let coordinate = tuple.coordinate();
    format!("{image_name}_{coordinate}_{format}")
}

/// Expand the `matrix:` axes into cells, then apply the `selectors:` block:
///
/// - the cartesian product of the axes (in declared order) is the candidate set;
/// - if `include` is non-empty, keep only cells matched by some `include` selector (allowlist);
/// - then drop every cell matched by some `exclude` selector (denylist — always wins).
///
/// Validates that axes are non-empty and that every selector references declared axes/values, and
/// errors if the result is empty (the product had cells but the selectors removed them all).
pub fn expand(
    axes: &AxisValues,
    selectors: Option<&Selectors>,
) -> Result<Vec<AxisTuple>, ConfigError> {
    for (axis, values) in axes {
        if values.is_empty() {
            return Err(ConfigError::EmptyAxis { axis: axis.clone() });
        }
    }
    if let Some(selectors) = selectors {
        validate_selectors(axes, selectors)?;
    }

    let product = cartesian(axes);
    let had_cells = !product.is_empty();

    let mut cells: Vec<AxisTuple> = match selectors {
        Some(s) if !s.include.is_empty() => product
            .into_iter()
            .filter(|cell| s.include.iter().any(|sel| section_matches(sel, cell)))
            .collect(),
        _ => product,
    };
    if let Some(s) = selectors {
        cells.retain(|cell| !s.exclude.iter().any(|sel| section_matches(sel, cell)));
    }

    if had_cells && cells.is_empty() {
        return Err(ConfigError::EmptySelection);
    }
    Ok(cells)
}

fn cartesian(axes: &AxisValues) -> Vec<AxisTuple> {
    let mut product: Vec<Vec<(String, String)>> = vec![Vec::new()];
    for (axis, values) in axes {
        let mut next = Vec::with_capacity(product.len() * values.len());
        for partial in &product {
            for value in values {
                let mut row = partial.clone();
                row.push((axis.clone(), value.clone()));
                next.push(row);
            }
        }
        product = next;
    }
    product
        .into_iter()
        .map(|values| AxisTuple { values })
        .collect()
}

/// A selector (sub-cube) matches a cell when every axis it pins contains the cell's value for that
/// axis; axes the selector omits match unconditionally.
fn section_matches(selector: &Selector, cell: &AxisTuple) -> bool {
    selector
        .iter()
        .all(|(axis, allowed)| cell.get(axis).is_some_and(|value| allowed.contains(value)))
}

fn validate_selectors(axes: &AxisValues, selectors: &Selectors) -> Result<(), ConfigError> {
    let check = |selector: &Selector, label: &'static str| -> Result<(), ConfigError> {
        for (axis, allowed) in selector {
            let declared = axes
                .get(axis)
                .ok_or_else(|| ConfigError::SelectorUnknownAxis {
                    selector: label,
                    axis: axis.clone(),
                })?;
            for value in allowed {
                if !declared.contains(value) {
                    return Err(ConfigError::SelectorUnknownValue {
                        selector: label,
                        axis: axis.clone(),
                        value: value.clone(),
                    });
                }
            }
        }
        Ok(())
    };
    for selector in &selectors.include {
        check(selector, INCLUDE)?;
    }
    for selector in &selectors.exclude {
        check(selector, EXCLUDE)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use indoc::indoc;

    fn axes(yaml: &str) -> AxisValues {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

    fn selectors(yaml: &str) -> Selectors {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

    fn coords(cells: &[AxisTuple]) -> Vec<String> {
        cells.iter().map(AxisTuple::coordinate).collect()
    }

    #[test]
    fn cartesian_product_preserves_declared_order() {
        let a = axes(indoc! {"
            variant: [grub, root-verity]
            arch: [amd64, arm64]
        "});
        assert_eq!(
            coords(&expand(&a, None).unwrap()),
            [
                "grub_amd64",
                "grub_arm64",
                "root-verity_amd64",
                "root-verity_arm64"
            ]
        );
    }

    #[test]
    fn slug_is_image_axes_format() {
        let a = axes(indoc! {"
            variant: [grub]
            arch: [amd64]
            release: ['4.0']
            phase: [base]
        "});
        let cells = expand(&a, None).unwrap();
        assert_eq!(
            cell_slug("trident-vm-testimage", &cells[0], OutputFormat::Cosi),
            "trident-vm-testimage_grub_amd64_4.0_base_cosi"
        );
    }

    #[test]
    fn exclude_drops_matching_cells() {
        let a = axes("variant: [grub, usr-verity]\nrelease: ['3.0', '4.0']");
        let s = selectors(indoc! {"
            exclude:
              - { variant: usr-verity, release: '3.0' }
        "});
        let cells = expand(&a, Some(&s)).unwrap();
        assert_eq!(cells.len(), 3);
        assert!(
            !cells
                .iter()
                .any(|c| c.get("variant") == Some("usr-verity") && c.get("release") == Some("3.0"))
        );
    }

    #[test]
    fn exclude_with_a_list_value_drops_each() {
        let a = axes("arch: [amd64, arm64]\nvariant: [plain, root, usr]");
        let s = selectors(indoc! {"
            exclude:
              - { arch: arm64, variant: [root, usr] }
        "});
        // 2×3=6 minus arm64×{root,usr}=2 → 4
        let cells = expand(&a, Some(&s)).unwrap();
        assert_eq!(cells.len(), 4);
        assert!(
            !cells
                .iter()
                .any(|c| c.get("arch") == Some("arm64") && c.get("variant") != Some("plain"))
        );
    }

    #[test]
    fn include_is_an_allowlist_union_with_omitted_axes_expanding() {
        let a = axes("arch: [amd64, arm64]\nruntime: [host, container]\nvariant: [plain, verity]");
        let s = selectors(indoc! {"
            include:
              - { arch: amd64 }                         # every amd64 cell (4)
              - { arch: arm64, runtime: host, variant: plain }  # plus one arm64 cell
        "});
        let cells = expand(&a, Some(&s)).unwrap();
        assert_eq!(cells.len(), 5);
        // all amd64 present, only the host/plain arm64 present
        assert_eq!(
            cells
                .iter()
                .filter(|c| c.get("arch") == Some("amd64"))
                .count(),
            4
        );
        let arm: Vec<_> = cells
            .iter()
            .filter(|c| c.get("arch") == Some("arm64"))
            .collect();
        assert_eq!(arm.len(), 1);
        assert_eq!(arm[0].get("runtime"), Some("host"));
        assert_eq!(arm[0].get("variant"), Some("plain"));
    }

    #[test]
    fn exclude_wins_over_include() {
        let a = axes("arch: [amd64, arm64]");
        let s = selectors(indoc! {"
            include:
              - { arch: amd64 }
              - { arch: arm64 }
            exclude:
              - { arch: arm64 }
        "});
        let cells = expand(&a, Some(&s)).unwrap();
        assert_eq!(coords(&cells), ["amd64"]);
    }

    #[test]
    fn empty_axis_is_an_error() {
        let err = expand(&axes("variant: []"), None).unwrap_err();
        assert!(matches!(err, ConfigError::EmptyAxis { .. }), "got {err:?}");
    }

    #[test]
    fn selector_with_undeclared_value_is_an_error() {
        let a = axes("variant: [grub]");
        let s = selectors("exclude:\n  - { variant: nope }");
        let err = expand(&a, Some(&s)).unwrap_err();
        assert!(
            matches!(err, ConfigError::SelectorUnknownValue { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn selector_with_undeclared_axis_is_an_error() {
        let a = axes("variant: [grub]");
        let s = selectors("include:\n  - { nope: x }");
        let err = expand(&a, Some(&s)).unwrap_err();
        assert!(
            matches!(err, ConfigError::SelectorUnknownAxis { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn selecting_no_cells_is_an_error() {
        let a = axes("arch: [amd64, arm64]");
        let s = selectors("exclude:\n  - { arch: amd64 }\n  - { arch: arm64 }");
        let err = expand(&a, Some(&s)).unwrap_err();
        assert!(matches!(err, ConfigError::EmptySelection), "got {err:?}");
    }
}
