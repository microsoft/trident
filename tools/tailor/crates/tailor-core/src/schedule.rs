//! Offline selection and dependency propagation. No resolver, fingerprint, engine or output IO.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use tailor_config::{WorkOrder, WorkOrderSelection};

use crate::{Cell, CoreError, Selector, Target, imagedep, orchestrator};

pub use crate::imagedep::DependencyEdge;

/// Dependency-complete serial execution, with requested roots distinct from required producers.
#[derive(Debug, Clone)]
pub struct BuildSchedule {
    targets: Vec<Arc<Target>>,
    cells: Vec<Cell>,
    requested: BTreeSet<String>,
    edges: Vec<DependencyEdge>,
}

impl BuildSchedule {
    /// Dependency-first targets, omitting conservative-closure nodes with no scheduled cells.
    pub fn targets(&self) -> &[Arc<Target>] {
        &self.targets
    }

    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    pub fn requested(&self) -> &BTreeSet<String> {
        &self.requested
    }

    pub fn edges(&self) -> &[DependencyEdge] {
        &self.edges
    }

    /// Exact selection of every scheduled cell, including dependency-required skip overrides.
    pub fn selector(&self) -> Result<Selector, CoreError> {
        Selector::parse(
            &[],
            &self
                .cells
                .iter()
                .map(|cell| cell.slug.to_string())
                .collect::<Vec<_>>(),
            &[],
        )
    }
}

pub fn select_work_order(
    order: &WorkOrder,
    members: &[Arc<Target>],
) -> Result<Vec<Cell>, CoreError> {
    let by_name: BTreeMap<&str, &Arc<Target>> = members.iter().map(|t| (t.name(), t)).collect();
    let mut selected = BTreeMap::new();
    for (index, clause) in order.selections().iter().enumerate() {
        let at = format!("selections[{index}]");
        let mut clause_cells = Vec::new();
        for name in clause.images() {
            let target = by_name.get(name.as_str()).ok_or_else(|| {
                invalid(format!("{at}.images"), format!("unknown image `{name}`"))
            })?;
            let all = orchestrator::cells(target)?;
            validate_filter(clause, target, &all, &at)?;
            let selector = Selector::from_axes(clause.select());
            let matched = all
                .into_iter()
                .filter(|cell| selector.matches(cell))
                .collect();
            clause_cells.extend(orchestrator::drop_skipped(matched, &selector));
        }
        if clause_cells.is_empty() {
            return Err(invalid(at, "clause selects no cells after skip rules"));
        }
        for cell in clause_cells {
            selected.insert(cell.slug.to_string(), cell);
        }
    }
    if !order.exact_slugs().is_empty() {
        let mut exact = BTreeMap::new();
        for target in members {
            for cell in orchestrator::cells(target)? {
                exact.insert(cell.slug.to_string(), cell);
            }
        }
        for (index, slug) in order.exact_slugs().iter().enumerate() {
            let cell = exact.get(slug).ok_or_else(|| {
                invalid(
                    format!("slugs[{index}]"),
                    format!("unknown cell slug `{slug}`"),
                )
            })?;
            selected.insert(slug.clone(), cell.clone());
        }
    }
    if selected.is_empty() {
        return Err(invalid("$", "order selects no cells"));
    }
    Ok(selected.into_values().collect())
}

fn validate_filter(
    clause: &WorkOrderSelection,
    target: &Target,
    all: &[Cell],
    at: &str,
) -> Result<(), CoreError> {
    let mut domain: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for cell in all {
        for (axis, value) in &cell.axes {
            domain.entry(axis).or_default().insert(value);
        }
    }
    // Manifest selectors may remove a declared coordinate; that value is still a valid name.
    if let Some(matrix) = &target.definition.matrix {
        for (axis, values) in matrix {
            domain
                .entry(axis)
                .or_default()
                .extend(values.iter().map(String::as_str));
        }
    }
    for (axis, values) in clause.select() {
        let declared = domain.get(axis.as_str()).ok_or_else(|| {
            invalid(
                format!("{at}.select.{axis}"),
                format!("unknown axis for image `{}`", target.name()),
            )
        })?;
        for value in values {
            if !declared.contains(value.as_str()) {
                return Err(invalid(
                    format!("{at}.select.{axis}"),
                    format!("unknown value `{value}` for image `{}`", target.name()),
                ));
            }
        }
    }
    Ok(())
}

pub fn schedule(roots: &[Cell], members: &[Arc<Target>]) -> Result<BuildSchedule, CoreError> {
    if roots.is_empty() {
        return Err(invalid("$", "cannot schedule empty requested roots"));
    }
    let by_name: BTreeMap<&str, &Arc<Target>> = members.iter().map(|t| (t.name(), t)).collect();
    let mut selected_targets = BTreeMap::new();
    let mut requested = BTreeSet::new();
    let mut required: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for root in roots {
        let name = root.target.name();
        let target = by_name
            .get(name)
            .ok_or_else(|| invalid("roots", format!("unknown image `{name}`")))?;
        selected_targets.insert(name, Arc::clone(target));
        requested.insert(root.slug.to_string());
        required
            .entry(name.to_owned())
            .or_default()
            .insert(root.slug.to_string());
    }
    let closure =
        imagedep::dependency_closure(&selected_targets.into_values().collect::<Vec<_>>(), members)?;
    // Canonical names break otherwise declaration-dependent ties in the existing topo helper.
    let mut closure = closure;
    closure.sort_by(|a, b| b.name().cmp(a.name()));
    let ordered = imagedep::topological_order(&closure, members)?;
    let mut per_image = BTreeMap::new();
    let mut edges = BTreeSet::new();
    for target in ordered.iter().rev() {
        let Some(slugs) = required.get(target.name()) else {
            continue;
        };
        let chosen: Vec<Cell> = orchestrator::cells(target)?
            .into_iter()
            .filter(|cell| slugs.contains(cell.slug.as_ref()))
            .collect();
        if chosen.len() != slugs.len() {
            return Err(invalid(
                "roots",
                format!(
                    "unknown requested or required slug for image `{}`",
                    target.name()
                ),
            ));
        }
        for cell in &chosen {
            let producers = imagedep::required_producers_with_edges(cell, members)?;
            for (producer, slug) in producers.cells {
                required.entry(producer).or_default().insert(slug);
            }
            edges.extend(producers.edges);
        }
        if !chosen.is_empty() {
            per_image.insert(target.name().to_owned(), chosen);
        }
    }
    let targets: Vec<Arc<Target>> = ordered
        .into_iter()
        .filter(|target| per_image.contains_key(target.name()))
        .collect();
    edges.retain(|edge| match edge {
        DependencyEdge::DependsOn {
            from_image,
            to_image,
        } => per_image.contains_key(from_image) && per_image.contains_key(to_image),
        DependencyEdge::Base { .. } | DependencyEdge::Input { .. } => true,
    });
    let cells = targets
        .iter()
        .flat_map(|target| per_image.remove(target.name()).unwrap_or_default())
        .collect();
    Ok(BuildSchedule {
        targets,
        cells,
        requested,
        edges: edges.into_iter().collect(),
    })
}

fn invalid(field: impl Into<String>, detail: impl Into<String>) -> CoreError {
    CoreError::WorkOrder {
        field: field.into(),
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use crate::{BuildSelection, testing::WorkOrderWorkspace};

    const PATH_BASE: &str = "base: {path: ./not-read.img}";
    const ARCH_MATRIX: &str = "matrix: {arch: [amd64, arm64]}";

    fn slugs(cells: &[Cell]) -> BTreeSet<String> {
        cells.iter().map(|cell| cell.slug.to_string()).collect()
    }

    fn fragment(target: &Target, path: &str, yaml: &str) {
        let path = target.dir.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, yaml).unwrap();
    }

    fn select(input: &str, members: &[Arc<Target>]) -> Result<Vec<Cell>, CoreError> {
        select_work_order(
            &WorkOrder::parse(&format!("schemaVersion: 1\n{input}")).unwrap(),
            members,
        )
    }

    #[test]
    fn additive_union_and_validation_of_every_image_axis_value_and_slug() {
        let workspace = WorkOrderWorkspace::new();
        let image = workspace.target("img", &format!("{PATH_BASE}\n{ARCH_MATRIX}"));
        let other = workspace.target("other", PATH_BASE);
        let members = [image, other];
        let roots = select(
            "images: [img]\nselect: {arch: [amd64]}\nslugs: [img_arm64_cosi, other_amd64_cosi]",
            &members,
        )
        .unwrap();
        assert_eq!(
            slugs(&roots),
            BTreeSet::from([
                "img_amd64_cosi".to_owned(),
                "img_arm64_cosi".to_owned(),
                "other_amd64_cosi".to_owned()
            ])
        );
        for input in [
            "images: [img, typo]",
            "images: [img]\nselect: {arch: [amd64, typo]}",
            "images: [img]\nselect: {typo: [amd64]}",
            "images: [img, other]\nselect: {arch: [amd64, arm64]}",
            "images: [img]\nslugs: [img_amd64_cosi, typo]",
            "slugs: [img_amd64_cosi, typo]",
        ] {
            let error = select(input, &members).unwrap_err();
            assert!(
                matches!(error, CoreError::WorkOrder { .. }),
                "got {error:?}"
            );
        }
        let overlapping = select(
            "selections: [{images: [img]}, {images: [img], select: {arch: [amd64]}}]",
            &members,
        )
        .unwrap();
        assert_eq!(overlapping.len(), 2);
    }

    #[test]
    fn skip_coordinate_opt_in_exact_override_and_empty_clause_rejection() {
        let workspace = WorkOrderWorkspace::new();
        let image = workspace.target(
            "img",
            &format!(
                "{PATH_BASE}\nmatrix: {{variant: [normal, skipped], arch: [amd64, arm64]}}\nskip: true"
            ),
        );
        fragment(&image, "by-variant/skipped.yaml", "skip: true");
        let other = workspace.target("other", PATH_BASE);
        let members = [Arc::clone(&image), other];
        assert_eq!(select("images: [img]", &members).unwrap().len(), 2);
        let arch = select("images: [img]\nselect: {arch: [amd64]}", &members).unwrap();
        assert_eq!(arch.len(), 1);
        assert_eq!(
            select("images: [img]\nselect: {variant: [skipped]}", &members)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            select("cells: [img_skipped_arm64_cosi]", &members)
                .unwrap()
                .len(),
            1
        );
        // Valid coordinate conjunction, but workspace selectors remove both combinations.
        let excluded = workspace.target(
            "excluded",
            &format!(
                "{PATH_BASE}\nmatrix: {{a: [x, y], b: [x, y]}}\nselectors: {{exclude: [{{a: x, b: x}}]}}"
            ),
        );
        let disabled = workspace.target(
            "disabled",
            &format!("{PATH_BASE}\nmatrix: {{variant: [skipped]}}"),
        );
        fragment(&disabled, "by-variant/skipped.yaml", "skip: true");
        let members = [excluded, Arc::clone(&image), disabled];
        select(
            "selections: [{images: [excluded], select: {a: [x], b: [x]}}, {images: [img]}]",
            &members,
        )
        .unwrap_err();
        // A multi-image clause can have an empty image slice as long as the clause is nonempty.
        assert!(
            !select("images: [excluded]\nselect: {a: [x]}", &members)
                .unwrap()
                .is_empty()
        );
        select(
            "selections: [{images: [disabled]}, {images: [img]}]",
            &members,
        )
        .unwrap_err();
        assert!(
            !select("images: [disabled, img]", &members)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn transitive_diamond_pairing_all_enabled_depends_on_and_signed_prerequisites() {
        let workspace = WorkOrderWorkspace::new();
        let base = workspace.target(
            "base",
            &format!("{PATH_BASE}\n{ARCH_MATRIX}\nsigning: release"),
        );
        let left = workspace.target("left", &format!("base: {{image: base}}\n{ARCH_MATRIX}"));
        let right = workspace.target(
            "right",
            &format!(
                "{PATH_BASE}\n{ARCH_MATRIX}\ninputs: [{{name: payload, image: base}}, {{name: metadata, image: base}}]"
            ),
        );
        let leaf = workspace.target(
            "leaf",
            &format!(
                "base: {{image: left}}\n{ARCH_MATRIX}\ninputs: [{{name: content, image: right}}]\ndependsOn: [base]"
            ),
        );
        let members = [base, left, right, leaf];
        let roots = select("cells: [leaf_amd64_cosi]", &members).unwrap();
        let plan = schedule(&roots, &members).unwrap();
        assert_eq!(plan.requested(), &slugs(&roots));
        assert_eq!(
            slugs(plan.cells()),
            BTreeSet::from([
                "base_amd64_cosi".to_owned(),
                "base_arm64_cosi".to_owned(),
                "left_amd64_cosi".to_owned(),
                "right_amd64_cosi".to_owned(),
                "leaf_amd64_cosi".to_owned()
            ])
        );
        assert_eq!(plan.targets().first().unwrap().name(), "base");
        assert_eq!(plan.targets().last().unwrap().name(), "leaf");
        assert_eq!(
            plan.edges()
                .iter()
                .filter(
                    |edge| matches!(edge, DependencyEdge::Input { from_cell, to_cell, .. }
            if from_cell == "base_amd64_cosi" && to_cell == "right_amd64_cosi")
                )
                .count(),
            2
        );
        assert!(plan.edges().contains(&DependencyEdge::Base {
            from_cell: "left_amd64_cosi".to_owned(),
            to_cell: "leaf_amd64_cosi".to_owned()
        }));
        assert_eq!(
            plan.edges()
                .iter()
                .filter(|edge| matches!(edge, DependencyEdge::DependsOn { .. }))
                .count(),
            1
        );
        let selector = plan.selector().unwrap();
        let selection = BuildSelection::from_selector(&selector);
        let executed: Vec<Cell> = plan
            .targets()
            .iter()
            .flat_map(|target| selection.cells_for(target).unwrap())
            .collect();
        assert_eq!(slugs(&executed), slugs(plan.cells()));
        assert_eq!(
            executed
                .iter()
                .map(|cell| cell.slug.as_ref())
                .collect::<Vec<_>>(),
            plan.cells()
                .iter()
                .map(|cell| cell.slug.as_ref())
                .collect::<Vec<_>>()
        );
        let mut permuted = members.to_vec();
        permuted.reverse();
        let reversed = schedule(&roots, &permuted).unwrap();
        assert_eq!(
            reversed
                .cells()
                .iter()
                .map(|c| c.slug.to_string())
                .collect::<Vec<_>>(),
            plan.cells()
                .iter()
                .map(|c| c.slug.to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn conservative_graph_is_validated_but_unused_producers_are_not_scheduled() {
        let workspace = WorkOrderWorkspace::new();
        let base = workspace.target("base", &format!("{PATH_BASE}\n{ARCH_MATRIX}"));
        let leaf = workspace.target(
            "leaf",
            &format!("{PATH_BASE}\nmatrix: {{variant: [normal, skipped], arch: [amd64, arm64]}}"),
        );
        fragment(
            &leaf,
            "by-variant/skipped.yaml",
            "base: {$set: {image: base}}\nskip: true",
        );
        // Image-level inputs/dependsOn always apply; a fragment-only base is conservative.
        let members = [Arc::clone(&base), Arc::clone(&leaf)];
        let roots = select("cells: [leaf_normal_amd64_cosi]", &members).unwrap();
        let plan = schedule(&roots, &members).unwrap();
        assert_eq!(plan.targets().len(), 1);
        assert_eq!(plan.targets()[0].name(), "leaf");
        let exact = select("cells: [leaf_skipped_arm64_cosi]", &members).unwrap();
        let required = schedule(&exact, &members).unwrap();
        assert_eq!(
            slugs(required.cells()),
            BTreeSet::from([
                "base_arm64_cosi".to_owned(),
                "leaf_skipped_arm64_cosi".to_owned()
            ])
        );
        schedule(&[], &members).unwrap_err();
        let cyclic = workspace.target("cyclic", &format!("{PATH_BASE}\ndependsOn: [leaf]"));
        fragment(
            &leaf,
            "by-variant/skipped.yaml",
            "base: {$set: {image: cyclic}}\nskip: true",
        );
        let error = schedule(&roots, &[Arc::clone(&leaf), cyclic]).unwrap_err();
        assert!(
            matches!(error, CoreError::DependencyCycle { .. }),
            "got {error:?}"
        );
        let a = workspace.target("a", &format!("{PATH_BASE}\ndependsOn: [b]"));
        let b = workspace.target("b", &format!("{PATH_BASE}\ndependsOn: [a]"));
        let members = [a, b];
        let roots = select("images: [a]", &members).unwrap();
        let error = schedule(&roots, &members).unwrap_err();
        assert!(
            matches!(error, CoreError::DependencyCycle { .. }),
            "got {error:?}"
        );
    }

    #[test]
    fn paired_skipped_cells_override_skips_but_depends_on_only_adds_enabled_cells() {
        let workspace = WorkOrderWorkspace::new();
        let base = workspace.target("base", &format!(
            "{PATH_BASE}\nmatrix: {{variant: [normal, skipped], arch: [amd64, arm64]}}\nsigning: release\nskip: true"
        ));
        fragment(&base, "by-variant/skipped.yaml", "skip: true");
        let leaf = workspace.target("leaf", "base: {image: base}\nmatrix: {variant: [normal, skipped], arch: [amd64, arm64]}\ndependsOn: [base]");
        let members = [base, leaf];
        let roots = select("cells: [leaf_skipped_arm64_cosi]", &members).unwrap();
        let plan = schedule(&roots, &members).unwrap();
        assert_eq!(
            slugs(plan.cells()),
            BTreeSet::from([
                "base_normal_amd64_cosi".to_owned(),
                "base_normal_arm64_cosi".to_owned(),
                "base_skipped_arm64_cosi".to_owned(),
                "leaf_skipped_arm64_cosi".to_owned()
            ])
        );
        assert_eq!(plan.requested(), &slugs(&roots));
        let selector = plan.selector().unwrap();
        let executed: Vec<Cell> = plan
            .targets()
            .iter()
            .flat_map(|target| {
                BuildSelection::from_selector(&selector)
                    .cells_for(target)
                    .unwrap()
            })
            .collect();
        assert_eq!(
            executed
                .iter()
                .map(|cell| cell.slug.as_ref())
                .collect::<Vec<_>>(),
            plan.cells()
                .iter()
                .map(|cell| cell.slug.as_ref())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            plan.edges()
                .iter()
                .filter(|edge| matches!(edge, DependencyEdge::DependsOn { .. }))
                .count(),
            1
        );
    }
}
