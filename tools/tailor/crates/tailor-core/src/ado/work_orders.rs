//! Indivisible effective image components and bounded, verified work-order encodings.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet, BinaryHeap},
    num::NonZeroUsize,
    sync::Arc,
};

use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use tailor_config::WorkOrder;

use crate::{
    BuildSchedule, Cell, CoreError, DependencyEdge, Selector, Target, orchestrator, schedule,
};

use super::{MAX_LEG_KEY_LEN, sanitize_leg_key, truncate};

const JOB_HASH_LEN: usize = 16;

/// One ADO job owns whole components and requests only their roots, not implicit producers.
#[derive(Debug, Clone)]
pub struct WorkOrderJob {
    key: String,
    work_order: WorkOrder,
    images: Vec<String>,
    cell_count: usize,
}

impl WorkOrderJob {
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn work_order(&self) -> &WorkOrder {
        &self.work_order
    }

    pub fn images(&self) -> &[String] {
        &self.images
    }

    pub fn cell_count(&self) -> usize {
        self.cell_count
    }
}

#[derive(Debug, Default)]
struct Component {
    images: BTreeSet<String>,
    requested: BTreeSet<String>,
    scheduled: BTreeSet<String>,
}

impl Component {
    fn extend(&mut self, other: Self) {
        self.images.extend(other.images);
        self.requested.extend(other.requested);
        self.scheduled.extend(other.scheduled);
    }
}

pub fn work_order_jobs(
    plan: &BuildSchedule,
    members: &[Arc<Target>],
    max_jobs: Option<NonZeroUsize>,
) -> Result<Vec<WorkOrderJob>, CoreError> {
    let components = components(plan)?;
    if components.is_empty() {
        return Err(invalid("cannot generate an empty job matrix"));
    }
    let packed = pack(components, max_jobs);
    let roots: BTreeMap<&str, &Cell> = plan
        .cells()
        .iter()
        .filter(|cell| plan.requested().contains(cell.slug.as_ref()))
        .map(|cell| (cell.slug.as_ref(), cell))
        .collect();
    let mut used = BTreeSet::new();
    let mut jobs = Vec::new();
    for component in packed {
        let images: Vec<String> = component.images.iter().cloned().collect();
        let key = job_key(&images)?;
        check_key(&key, &mut used)?;
        let work_order = encode(&component, &roots, members)?;
        jobs.push(WorkOrderJob {
            key,
            work_order,
            images,
            cell_count: component.scheduled.len(),
        });
    }
    jobs.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(jobs)
}

fn components(plan: &BuildSchedule) -> Result<Vec<Component>, CoreError> {
    let cell_images: BTreeMap<&str, &str> = plan
        .cells()
        .iter()
        .map(|cell| (cell.slug.as_ref(), cell.target.name()))
        .collect();
    let mut graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut by_image: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for cell in plan.cells() {
        graph.entry(cell.target.name().to_owned()).or_default();
        by_image
            .entry(cell.target.name().to_owned())
            .or_default()
            .insert(cell.slug.to_string());
    }
    for edge in plan.edges() {
        let (from, to) = match edge {
            DependencyEdge::Base { from_cell, to_cell }
            | DependencyEdge::Input {
                from_cell, to_cell, ..
            } => (
                *cell_images
                    .get(from_cell.as_str())
                    .ok_or_else(|| invalid("edge names an unscheduled producer cell"))?,
                *cell_images
                    .get(to_cell.as_str())
                    .ok_or_else(|| invalid("edge names an unscheduled consumer cell"))?,
            ),
            DependencyEdge::DependsOn {
                from_image,
                to_image,
            } => (from_image.as_str(), to_image.as_str()),
        };
        if !graph.contains_key(from) || !graph.contains_key(to) {
            return Err(invalid("edge names an unscheduled image"));
        }
        graph
            .entry(from.to_owned())
            .or_default()
            .insert(to.to_owned());
        graph
            .entry(to.to_owned())
            .or_default()
            .insert(from.to_owned());
    }
    let mut unseen: BTreeSet<String> = graph.keys().cloned().collect();
    let mut output = Vec::new();
    while let Some(first) = unseen.first().cloned() {
        let mut images = BTreeSet::new();
        let mut stack = vec![first];
        while let Some(name) = stack.pop() {
            if !unseen.remove(&name) {
                continue;
            }
            stack.extend(graph.get(&name).into_iter().flatten().cloned());
            images.insert(name);
        }
        let scheduled: BTreeSet<String> = images
            .iter()
            .flat_map(|image| by_image.get(image).into_iter().flatten().cloned())
            .collect();
        let requested = scheduled.intersection(plan.requested()).cloned().collect();
        output.push(Component {
            images,
            requested,
            scheduled,
        });
    }
    Ok(output)
}

fn pack(mut components: Vec<Component>, cap: Option<NonZeroUsize>) -> Vec<Component> {
    let count = cap.map_or(components.len(), NonZeroUsize::get);
    if count >= components.len() {
        return components;
    }
    components.sort_by(|a, b| {
        b.scheduled
            .len()
            .cmp(&a.scheduled.len())
            .then_with(|| a.images.cmp(&b.images))
    });
    let mut jobs: Vec<Component> = (0..count).map(|_| Component::default()).collect();
    let mut loads: BinaryHeap<Reverse<(usize, usize)>> =
        (0..count).map(|index| Reverse((0, index))).collect();
    for component in components {
        let Reverse((load, index)) = loads
            .pop()
            .expect("invariant: a positive job cap provides a heap entry");
        let weight = component.scheduled.len();
        jobs[index].extend(component);
        loads.push(Reverse((load + weight, index)));
    }
    jobs
}

fn job_key(images: &[String]) -> Result<String, CoreError> {
    // JSON is an unambiguous encoding of the complete sorted image list, unlike joining with '_'.
    let digest = Sha256::digest(serialize(images)?.as_bytes());
    let suffix = &hex::encode(digest)[..JOB_HASH_LEN];
    let prefix = sanitize_leg_key(&images.join("_"));
    let prefix = truncate(&prefix, MAX_LEG_KEY_LEN - JOB_HASH_LEN - 1);
    Ok(format!("{prefix}_{suffix}"))
}

fn check_key(key: &str, used: &mut BTreeSet<String>) -> Result<(), CoreError> {
    if !used.insert(key.to_owned()) {
        return Err(CoreError::WorkOrderJobCollision {
            key: key.to_owned(),
        });
    }
    Ok(())
}

fn encode(
    component: &Component,
    roots: &BTreeMap<&str, &Cell>,
    members: &[Arc<Target>],
) -> Result<WorkOrder, CoreError> {
    let mut per_image: BTreeMap<String, Vec<&Cell>> = BTreeMap::new();
    for slug in &component.requested {
        let cell = roots
            .get(slug.as_str())
            .ok_or_else(|| invalid("requested root is missing"))?;
        per_image
            .entry(cell.target.name().to_owned())
            .or_default()
            .push(cell);
    }
    let mut inventory = BTreeMap::new();
    for name in per_image.keys() {
        let target = members
            .iter()
            .find(|target| target.name() == name)
            .ok_or_else(|| invalid("requested image is not a workspace member"))?;
        inventory.insert(name.clone(), orchestrator::cells(target)?);
    }
    let names: Vec<String> = per_image.keys().cloned().collect();
    let requested: Vec<String> = component.requested.iter().cloned().collect();
    let mut candidates = vec![
        WorkOrder::resolved(requested.clone()),
        parse(&json!({"schemaVersion": 1, "slugs": requested}))?,
        parse(&json!({"schemaVersion": 1, "images": names}))?,
    ];
    let all_roots: Vec<&Cell> = per_image.values().flatten().copied().collect();
    let common = bounding_filter(&all_roots);
    let shared = common
        .iter()
        .filter(|(axis, _)| {
            per_image
                .values()
                .all(|cells| cells.iter().all(|cell| cell.axes.contains_key(*axis)))
        })
        .map(|(axis, values)| (axis.clone(), values.clone()))
        .collect::<Filter>();
    candidates.push(parse(&order_from_slices(
        vec![slice(&names, &shared)],
        &[],
    ))?);
    let shared_reduced = shared
        .into_iter()
        .filter(|(axis, values)| {
            inventory
                .values()
                .any(|cells| domain(cells, axis) != *values)
        })
        .collect();
    candidates.push(parse(&order_from_slices(
        vec![slice(&names, &shared_reduced)],
        &[],
    ))?);

    let mut full = Vec::new();
    let mut reduced = Vec::new();
    let mut additive = Vec::new();
    let mut whole = Vec::new();
    let mut exceptions = BTreeSet::new();
    let mut whole_exceptions = BTreeSet::new();
    for (image, cells) in &per_image {
        let all = &inventory[image];
        let expected: BTreeSet<String> = cells.iter().map(|cell| cell.slug.to_string()).collect();
        let filter = bounding_filter(cells);
        let narrow: Filter = filter
            .iter()
            .filter(|(axis, values)| domain(all, axis) != **values)
            .map(|(axis, values)| (axis.clone(), values.clone()))
            .collect();
        full.push((image.clone(), filter));
        reduced.push((image.clone(), narrow));

        let enabled = select_slice(all, &Filter::new());
        let enabled_slugs = cell_slugs(&enabled);
        if !enabled_slugs.is_empty() && enabled_slugs.is_subset(&expected) {
            whole.push(image.clone());
            whole_exceptions.extend(expected.difference(&enabled_slugs).cloned());
        } else {
            whole_exceptions.extend(expected.iter().cloned());
        }
        if let Some((filter, selected)) = subset_slice(all, cells, &expected)? {
            additive.push((image.clone(), filter));
            exceptions.extend(expected.difference(&selected).cloned());
        } else {
            exceptions.extend(expected);
        }
    }
    candidates.push(parse(&order_from_slices(group_slices(full), &[]))?);
    candidates.push(parse(&order_from_slices(group_slices(reduced), &[]))?);
    if !additive.is_empty() {
        candidates.push(parse(&order_from_slices(
            group_slices(additive),
            &exceptions.into_iter().collect::<Vec<_>>(),
        ))?);
    }
    if !whole.is_empty() {
        candidates.push(parse(&order_from_slices(
            vec![slice(&whole, &Filter::new())],
            &whole_exceptions.into_iter().collect::<Vec<_>>(),
        ))?);
    }

    let mut best: Option<(usize, String, WorkOrder)> = None;
    for candidate in candidates {
        let Ok(selected) = schedule::select_work_order(&candidate, members) else {
            continue;
        };
        if cell_slugs(&selected) != component.requested {
            continue;
        }
        let verified = schedule::schedule(&selected, members)?;
        if verified.requested() != &component.requested
            || cell_slugs(verified.cells()) != component.scheduled
        {
            continue;
        }
        let serialized = serialize(&candidate)?;
        // ADO nests the order in a JSON string: account for its escaping, not just inner bytes.
        let cost = serialize(&serialized)?.len();
        if best
            .as_ref()
            .is_none_or(|(length, text, _)| (cost, &serialized) < (*length, text))
        {
            best = Some((cost, serialized, candidate));
        }
    }
    best.map(|(_, _, order)| order)
        .ok_or_else(|| invalid("no generated encoding reproduces the job"))
}

type Filter = BTreeMap<String, Vec<String>>;

fn bounding_filter(cells: &[&Cell]) -> Filter {
    let mut values: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for cell in cells {
        for (axis, value) in &cell.axes {
            values
                .entry(axis.clone())
                .or_default()
                .insert(value.clone());
        }
    }
    values
        .into_iter()
        .map(|(axis, values)| (axis, values.into_iter().collect()))
        .collect()
}

fn domain(cells: &[Cell], axis: &str) -> Vec<String> {
    cells
        .iter()
        .filter_map(|cell| cell.axes.get(axis).cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn select_slice(cells: &[Cell], filter: &Filter) -> Vec<Cell> {
    let selector = Selector::from_axes(filter);
    orchestrator::drop_skipped(
        cells
            .iter()
            .filter(|cell| selector.matches(cell))
            .cloned()
            .collect(),
        &selector,
    )
}

fn cell_slugs(cells: &[Cell]) -> BTreeSet<String> {
    cells.iter().map(|cell| cell.slug.to_string()).collect()
}

fn subset_slice(
    all: &[Cell],
    roots: &[&Cell],
    expected: &BTreeSet<String>,
) -> Result<Option<(Filter, BTreeSet<String>)>, CoreError> {
    // Linear bounded alternatives: whole image, bounding rectangle and single coordinates.
    // This is deliberately not a search over combinations of arbitrary sub-cubes.
    let bounds = bounding_filter(roots);
    let mut filters = vec![Filter::new(), bounds.clone()];
    for (axis, values) in bounds {
        for value in values {
            filters.push(BTreeMap::from([(axis.clone(), vec![value])]));
        }
    }
    let mut best: Option<(usize, usize, String, Filter, BTreeSet<String>)> = None;
    for filter in filters {
        let selected = cell_slugs(&select_slice(all, &filter));
        if selected.is_empty() || !selected.is_subset(expected) {
            continue;
        }
        let text = serialize(&filter)?;
        let rank = (Reverse(selected.len()), text.len(), &text);
        if best
            .as_ref()
            .is_none_or(|(count, cost, previous, _, _)| rank < (Reverse(*count), *cost, previous))
        {
            best = Some((selected.len(), text.len(), text, filter, selected));
        }
    }
    Ok(best.map(|(_, _, _, filter, selected)| (filter, selected)))
}

fn slice(images: &[String], filter: &Filter) -> Value {
    if filter.is_empty() {
        json!({"images": images})
    } else {
        json!({"images": images, "select": filter})
    }
}

fn group_slices(slices: Vec<(String, Filter)>) -> Vec<Value> {
    let mut groups: BTreeMap<Filter, Vec<String>> = BTreeMap::new();
    for (image, filter) in slices {
        groups.entry(filter).or_default().push(image);
    }
    groups
        .into_iter()
        .map(|(filter, images)| slice(&images, &filter))
        .collect()
}

fn order_from_slices(mut slices: Vec<Value>, exceptions: &[String]) -> Value {
    let mut order = if slices.len() == 1 {
        slices.pop().expect("invariant: exactly one slice")
    } else {
        json!({"selections": slices})
    };
    order["schemaVersion"] = json!(1);
    if !exceptions.is_empty() {
        order["slugs"] = json!(exceptions);
    }
    order
}

fn parse(value: &Value) -> Result<WorkOrder, CoreError> {
    Ok(WorkOrder::parse(&serialize(value)?)?)
}

fn serialize(value: &(impl Serialize + ?Sized)) -> Result<String, CoreError> {
    serde_json::to_string(value)
        .map_err(|error| invalid(format!("cannot serialize generated work order: {error}")))
}

fn invalid(detail: impl Into<String>) -> CoreError {
    CoreError::WorkOrder {
        field: "jobs".to_owned(),
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testing::WorkOrderWorkspace;

    const PATH_BASE: &str = "base: {path: ./not-read.img}";

    fn selected(input: &str, members: &[Arc<Target>]) -> BuildSchedule {
        let order = WorkOrder::parse(&format!("schemaVersion: 1\n{input}")).unwrap();
        schedule::schedule(
            &schedule::select_work_order(&order, members).unwrap(),
            members,
        )
        .unwrap()
    }

    fn round_trip(jobs: &[WorkOrderJob], original: &BuildSchedule, members: &[Arc<Target>]) {
        let mut requested = BTreeSet::new();
        let mut scheduled = BTreeSet::new();
        for job in jobs {
            let json = serialize(job.work_order()).unwrap();
            let order = WorkOrder::parse(&json).unwrap();
            let roots = schedule::select_work_order(&order, members).unwrap();
            let plan = schedule::schedule(&roots, members).unwrap();
            assert_eq!(plan.cells().len(), job.cell_count());
            assert_eq!(
                plan.targets()
                    .iter()
                    .map(|t| t.name().to_owned())
                    .collect::<BTreeSet<_>>(),
                job.images().iter().cloned().collect()
            );
            for slug in plan.requested() {
                assert!(
                    requested.insert(slug.clone()),
                    "duplicate requested root {slug}"
                );
            }
            for cell in plan.cells() {
                assert!(
                    scheduled.insert(cell.slug.to_string()),
                    "duplicate scheduled cell {}",
                    cell.slug
                );
            }
        }
        assert_eq!(&requested, original.requested());
        assert_eq!(scheduled, cell_slugs(original.cells()));
    }

    fn weighted(workspace: &WorkOrderWorkspace, name: &str, count: usize) -> Arc<Target> {
        let values = (0..count)
            .map(|index| format!("v{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        workspace.target(
            name,
            &format!("{PATH_BASE}\nmatrix: {{variant: [{values}]}}"),
        )
    }

    #[test]
    fn shared_producer_and_all_cells_of_an_image_are_indivisible() {
        let workspace = WorkOrderWorkspace::new();
        let base = workspace.target(
            "base",
            &format!("{PATH_BASE}\nmatrix: {{arch: [amd64, arm64]}}"),
        );
        let left = workspace.target(
            "left",
            "base: {image: base}\nmatrix: {arch: [amd64, arm64]}",
        );
        let right = workspace.target("right", &format!(
            "{PATH_BASE}\nmatrix: {{arch: [amd64, arm64]}}\ninputs: [{{name: data, image: base}}]"
        ));
        let independent = workspace.target("independent", PATH_BASE);
        let members = [base, left, right, independent];
        let plan = selected(
            "cells: [left_amd64_cosi, right_arm64_cosi, independent_amd64_cosi]",
            &members,
        );
        let jobs = work_order_jobs(&plan, &members, None).unwrap();
        assert_eq!(jobs.len(), 2);
        let shared = jobs
            .iter()
            .find(|job| job.images().contains(&"base".to_owned()))
            .unwrap();
        assert_eq!(shared.images(), ["base", "left", "right"]);
        assert_eq!(shared.cell_count(), 4);
        round_trip(&jobs, &plan, &members);
        let packed = work_order_jobs(&plan, &members, NonZeroUsize::new(1)).unwrap();
        assert_eq!(packed.len(), 1);
        round_trip(&packed, &plan, &members);
    }

    #[test]
    fn lpt_caps_no_padding_and_stable_ties() {
        let workspace = WorkOrderWorkspace::new();
        let members: Vec<Arc<Target>> = [("a", 8), ("b", 7), ("c", 6), ("d", 5), ("e", 4)]
            .into_iter()
            .map(|(name, count)| weighted(&workspace, name, count))
            .collect();
        let plan = selected("images: [a, b, c, d, e]", &members);
        for cap in [None, NonZeroUsize::new(5), NonZeroUsize::new(9)] {
            let jobs = work_order_jobs(&plan, &members, cap).unwrap();
            assert_eq!(jobs.len(), 5);
            round_trip(&jobs, &plan, &members);
        }
        let jobs = work_order_jobs(&plan, &members, NonZeroUsize::new(3)).unwrap();
        assert_eq!(
            jobs.iter()
                .map(|job| job.images().to_vec())
                .collect::<Vec<_>>(),
            [vec!["a"], vec!["b", "e"], vec!["c", "d"]]
        );
        assert_eq!(
            jobs.iter()
                .map(WorkOrderJob::cell_count)
                .collect::<Vec<_>>(),
            [8, 11, 11]
        );
        round_trip(&jobs, &plan, &members);
        let mut reverse = members.clone();
        reverse.reverse();
        let reversed = selected("images: [e, d, c, b, a]", &reverse);
        let reversed = work_order_jobs(&reversed, &reverse, NonZeroUsize::new(3)).unwrap();
        assert_eq!(
            jobs.iter()
                .map(|job| (job.key(), serialize(job.work_order()).unwrap()))
                .collect::<Vec<_>>(),
            reversed
                .iter()
                .map(|job| (job.key(), serialize(job.work_order()).unwrap()))
                .collect::<Vec<_>>()
        );
        let ties: Vec<Arc<Target>> = ["f", "g", "h", "i"]
            .into_iter()
            .map(|name| weighted(&workspace, name, 2))
            .collect();
        let equal_plan = selected("images: [i, h, g, f]", &ties);
        let tied_jobs = work_order_jobs(&equal_plan, &ties, NonZeroUsize::new(2)).unwrap();
        assert_eq!(
            tied_jobs
                .iter()
                .map(|job| job.images().to_vec())
                .collect::<Vec<_>>(),
            [vec!["f", "h"], vec!["g", "i"]]
        );
        round_trip(&tied_jobs, &equal_plan, &ties);
    }

    #[test]
    fn shortest_tested_encodings_are_verified_with_no_extra_roots_or_cells() {
        let workspace = WorkOrderWorkspace::new();
        let name = "long_image_name_for_compact_filtered_orders";
        let image = workspace.target(name, &format!(
            "{PATH_BASE}\nmatrix: {{variant: [one, two, three, four], arch: [amd64, arm64]}}\noutputs: [{{format: cosi}}, {{format: vhd}}]"
        ));
        let members = [image];
        let plan = selected(&format!("images: [{name}]"), &members);
        let jobs = work_order_jobs(&plan, &members, None).unwrap();
        let encoded = serialize(jobs[0].work_order()).unwrap();
        assert!(encoded.contains("\"images\""));
        assert!(!encoded.contains("\"select\""));
        round_trip(&jobs, &plan, &members);
        let filtered = selected(
            &format!("images: [{name}]\nselect: {{arch: [amd64]}}"),
            &members,
        );
        let jobs = work_order_jobs(&filtered, &members, None).unwrap();
        assert!(
            serialize(jobs[0].work_order())
                .unwrap()
                .contains("\"select\"")
        );
        round_trip(&jobs, &filtered, &members);
        let exact = selected(&format!("cells: [{name}_one_amd64_vhd]"), &members);
        let jobs = work_order_jobs(&exact, &members, None).unwrap();
        assert!(
            serialize(jobs[0].work_order())
                .unwrap()
                .contains("\"cells\"")
        );
        round_trip(&jobs, &exact, &members);
        let additive = selected(
            &format!("images: [{name}]\nselect: {{arch: [amd64]}}\nslugs: [{name}_one_arm64_cosi]"),
            &members,
        );
        let jobs = work_order_jobs(&additive, &members, None).unwrap();
        assert!(
            serialize(jobs[0].work_order())
                .unwrap()
                .contains("\"slugs\"")
        );
        round_trip(&jobs, &additive, &members);
    }

    #[test]
    fn names_are_ascii_bounded_unambiguous_and_collisions_are_errors() {
        let first = job_key(&["a-b".to_owned()]).unwrap();
        let second = job_key(&["a.b".to_owned()]).unwrap();
        assert_ne!(first, second);
        assert_ne!(
            job_key(&["a_b".to_owned(), "c".to_owned()]).unwrap(),
            job_key(&["a".to_owned(), "b_c".to_owned()]).unwrap()
        );
        let long = job_key(&[format!("9_{}", "名".repeat(130))]).unwrap();
        assert!(long.len() <= MAX_LEG_KEY_LEN);
        assert!(long.starts_with(|c: char| c.is_ascii_alphabetic()));
        assert!(long.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        let mut used = BTreeSet::new();
        check_key(&first, &mut used).unwrap();
        let error = check_key(&first, &mut used).unwrap_err();
        assert!(
            matches!(error, CoreError::WorkOrderJobCollision { .. }),
            "got {error:?}"
        );
    }
}
