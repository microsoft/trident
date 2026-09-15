//! Inter-image dependencies (`meta/docs/2026-09-09-inter-image-dependencies.md`).
//!
//! An image may consume another workspace image's **output** — as its `base: { image: … }`, or
//! embedded in `config:` via `${inputs.<name>}`. Three jobs live here: (1) order the images so a
//! producer builds before its consumers ([`topological_order`], with cycle detection); (2) lower
//! each `base: { image }` cell to a concrete `path` base pointing at the producer's paired-cell
//! artifact ([`lower_image_bases`]); and (3) resolve each `${inputs.<name>}` to that artifact's path,
//! substitute it into the config, and record it as a dependency ([`resolve_inputs`]). Everything
//! downstream (content-hashing, binding, arg building) then sees ordinary local-file paths.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use serde_yaml_ng::Value;

use tailor_config::{BaseSource, OutputFormat, render_image};

use crate::{
    domain::{Cell, Target},
    error::CoreError,
    orchestrator::{cells, published_artifact_name},
};

/// The dependency-closed set of `selected` images: `selected` plus every workspace image reachable
/// through `base: { image }` / `dependsOn` edges, so a consumer is never scheduled without its
/// producers. An edge to a non-member is [`CoreError::UnknownDependencyImage`].
pub fn dependency_closure(
    selected: &[Arc<Target>],
    members: &[Arc<Target>],
) -> Result<Vec<Arc<Target>>, CoreError> {
    let by_name: BTreeMap<&str, &Arc<Target>> = members.iter().map(|t| (t.name(), t)).collect();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<Arc<Target>> = Vec::new();
    let mut queue: Vec<Arc<Target>> = selected.to_vec();
    while let Some(target) = queue.pop() {
        if !seen.insert(target.name().to_owned()) {
            continue;
        }
        for dep in dependencies(&target)? {
            match by_name.get(dep.as_str()) {
                Some(producer) if !seen.contains(dep.as_str()) => {
                    queue.push(Arc::clone(producer));
                }
                Some(_) => {}
                None => {
                    return Err(CoreError::UnknownDependencyImage {
                        image: target.name().to_owned(),
                        dependency: dep,
                        known: members
                            .iter()
                            .map(|t| t.name())
                            .collect::<Vec<_>>()
                            .join(", "),
                    });
                }
            }
        }
        out.push(target);
    }
    Ok(out)
}

/// The workspace-image names `target` directly depends on: every `base: { image }` referenced by any
/// of its cells (a base may be set per-fragment), every `image` input, plus its explicit `dependsOn`.
/// Deduplicated.
pub fn dependencies(target: &Target) -> Result<Vec<String>, CoreError> {
    let mut names: BTreeSet<String> = target.definition.depends_on.iter().cloned().collect();
    for input in &target.definition.inputs {
        names.insert(input.image.clone());
    }
    for rc in render_image(&target.definition, &target.dir)? {
        if let BaseSource::Image { image, .. } = &rc.base {
            names.insert(image.clone());
        }
    }
    Ok(names.into_iter().collect())
}

/// Order `nodes` so every producer precedes its consumers. Edges are drawn only among `nodes`
/// (`nodes` must be dependency-closed — the caller adds transitive producers). A name in a
/// dependency that is not a workspace `member` is an [`CoreError::UnknownDependencyImage`]; a cycle
/// is a [`CoreError::DependencyCycle`].
pub fn topological_order(
    nodes: &[Arc<Target>],
    members: &[Arc<Target>],
) -> Result<Vec<Arc<Target>>, CoreError> {
    let member_names: BTreeSet<&str> = members.iter().map(|t| t.name()).collect();
    let node_index: BTreeMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, t)| (t.name(), i))
        .collect();

    // Adjacency (producer → consumers) restricted to the node set, plus in-degrees.
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    let mut in_degree: Vec<usize> = vec![0; nodes.len()];
    for (consumer_idx, node) in nodes.iter().enumerate() {
        for dep in dependencies(node)? {
            if !member_names.contains(dep.as_str()) {
                return Err(CoreError::UnknownDependencyImage {
                    image: node.name().to_owned(),
                    dependency: dep,
                    known: member_names.iter().copied().collect::<Vec<_>>().join(", "),
                });
            }
            if let Some(&producer_idx) = node_index.get(dep.as_str()) {
                successors[producer_idx].push(consumer_idx);
                in_degree[consumer_idx] += 1;
            }
        }
    }

    // Kahn's algorithm; ties broken by declaration order for a stable, explainable schedule.
    let mut ready: Vec<usize> = (0..nodes.len()).filter(|&i| in_degree[i] == 0).collect();
    let mut ordered = Vec::with_capacity(nodes.len());
    while let Some(idx) = ready.pop() {
        ordered.push(Arc::clone(&nodes[idx]));
        for &next in &successors[idx] {
            in_degree[next] -= 1;
            if in_degree[next] == 0 {
                ready.push(next);
            }
        }
    }

    if ordered.len() != nodes.len() {
        return Err(CoreError::DependencyCycle {
            chain: describe_cycle(nodes, &node_index),
        });
    }
    Ok(ordered)
}

/// Lower every `base: { image }` in `cells` to a `path` base pointing at the producer's paired-cell
/// published artifact. Idempotent for non-image bases. `members` supplies producer definitions.
pub fn lower_image_bases(
    cells: &mut [Cell],
    members: &[Arc<Target>],
    output_dir: &Path,
) -> Result<(), CoreError> {
    for cell in cells.iter_mut() {
        let BaseSource::Image {
            image,
            output,
            cell: pins,
        } = cell.base.clone()
        else {
            continue;
        };
        cell.base = resolve_image_base(cell, &image, output, &pins, members, output_dir)?;
    }
    Ok(())
}

/// Resolve every image's declared `inputs:` for each of its `cells`: resolve each input to the
/// producer's paired-cell artifact path, substitute `${inputs.<name>}` occurrences in the cell's
/// `ic_config`, and record the resolved paths in `cell.input_deps` (content-hashed + already bound
/// via the output dir). A `${inputs.<name>}` referencing an undeclared input is an error.
pub fn resolve_inputs(
    cells: &mut [Cell],
    members: &[Arc<Target>],
    output_dir: &Path,
) -> Result<(), CoreError> {
    for cell in cells.iter_mut() {
        let specs = cell.target.definition.inputs.clone();
        if specs.is_empty() {
            continue;
        }
        // Resolve each declared input for this cell's coordinate (immutable borrows finish here).
        let mut resolved: BTreeMap<String, PathBuf> = BTreeMap::new();
        for spec in &specs {
            let path = resolve_image_ref(
                cell,
                &spec.image,
                spec.output,
                &spec.cell,
                members,
                output_dir,
            )?;
            resolved.insert(spec.name.clone(), path);
        }
        let image = cell.target.name().to_owned();
        substitute_inputs(&mut cell.ic_config, &resolved, &image)?;
        // Every declared input is a dependency of this cell — hashed and (via the output dir) bound,
        // whether or not `config:` references it.
        cell.input_deps = specs
            .iter()
            .filter_map(|spec| resolved.get(&spec.name).cloned())
            .collect();
    }
    Ok(())
}

/// Replace `${inputs.<name>}` in every string scalar of `value` with the resolved path. An
/// `${inputs.<name>}` whose `name` is not a declared input is [`CoreError::UnknownInput`].
fn substitute_inputs(
    value: &mut Value,
    resolved: &BTreeMap<String, PathBuf>,
    image: &str,
) -> Result<(), CoreError> {
    match value {
        Value::String(text) if text.contains(INPUT_TOKEN_OPEN) => {
            for (name, path) in resolved {
                let token = format!("{INPUT_TOKEN_OPEN}{name}}}");
                *text = text.replace(&token, &path.to_string_lossy());
            }
            if let Some(rest) = text.split(INPUT_TOKEN_OPEN).nth(1) {
                let name: String = rest.chars().take_while(|&c| c != '}').collect();
                return Err(CoreError::UnknownInput {
                    image: image.to_owned(),
                    name,
                    declared: resolved.keys().cloned().collect::<Vec<_>>().join(", "),
                });
            }
        }
        Value::Sequence(items) => {
            for item in items.iter_mut() {
                substitute_inputs(item, resolved, image)?;
            }
        }
        Value::Mapping(map) => {
            for (_key, item) in map.iter_mut() {
                substitute_inputs(item, resolved, image)?;
            }
        }
        _ => {}
    }
    Ok(())
}

const INPUT_TOKEN_OPEN: &str = "${inputs.";

/// Resolve one `base: { image }` reference for a single consumer cell to a concrete `path` base.
fn resolve_image_base(
    consumer: &Cell,
    image: &str,
    output: Option<OutputFormat>,
    pins: &BTreeMap<String, String>,
    members: &[Arc<Target>],
    output_dir: &Path,
) -> Result<BaseSource, CoreError> {
    let path = resolve_image_ref(consumer, image, output, pins, members, output_dir)?;
    Ok(BaseSource::Path {
        path,
        arch: Some(consumer.arch),
    })
}

/// Resolve one `image` reference (a `base: { image }` or an `${inputs}` entry) for a single consumer
/// cell to the producer's paired-cell published artifact path (§2.4). The path lives under
/// `output_dir`; whether it exists yet is the scheduler's concern (per-node ordering builds it first).
fn resolve_image_ref(
    consumer: &Cell,
    image: &str,
    output: Option<OutputFormat>,
    pins: &BTreeMap<String, String>,
    members: &[Arc<Target>],
    output_dir: &Path,
) -> Result<PathBuf, CoreError> {
    let (_producer, matched) = resolve_producer(consumer, image, output, pins, members)?;
    let name = published_artifact_name(
        matched.slug.as_ref(),
        matched.output.format,
        matched.output.compression,
    );
    Ok(output_dir.join(name))
}

/// The producer target name and matched producer cell for one `image` reference of `consumer` (a
/// `base: { image }` or an `${inputs.*}` entry). A producer cell matches when its format equals the
/// requested one and every one of its axes is satisfied by a pin (highest priority) or by the
/// consumer's value for a shared axis. An axis the producer has but neither pins nor the consumer
/// constrains is left free — the source of the ambiguity error below.
fn resolve_producer(
    consumer: &Cell,
    image: &str,
    output: Option<OutputFormat>,
    pins: &BTreeMap<String, String>,
    members: &[Arc<Target>],
) -> Result<(String, Cell), CoreError> {
    let image_name = consumer.target.name().to_owned();
    let producer = members.iter().find(|t| t.name() == image).ok_or_else(|| {
        CoreError::UnknownDependencyImage {
            image: image_name.clone(),
            dependency: image.to_owned(),
            known: members
                .iter()
                .map(|t| t.name())
                .collect::<Vec<_>>()
                .join(", "),
        }
    })?;
    let producer_cells = cells(producer)?;

    let format = resolve_output(&image_name, image, output, &producer_cells)?;
    let producer_axes = axis_domain(&producer_cells);
    validate_pins(&image_name, image, pins, &producer_axes)?;

    let matched: Vec<&Cell> = producer_cells
        .iter()
        .filter(|pc| pc.output.format == format && axes_satisfied(pc, consumer, pins))
        .collect();

    match matched.as_slice() {
        [] => Err(CoreError::MissingProducerCell {
            image: image_name,
            slug: consumer.slug.as_ref().to_owned(),
            producer: image.to_owned(),
            coord: requested_coord(consumer, pins, &producer_axes),
            available: available_coords(&producer_cells, format),
        }),
        [only] => Ok((producer.name().to_owned(), (*only).clone())),
        many => Err(ambiguous_error(&image_name, consumer, image, pins, many)),
    }
}

/// The `(producer image name, producer cell slug)` pairs that `consumer` requires — one per
/// `base: { image }` and per declared `inputs:` entry. The scheduler unions these into each
/// producer's build set so a dependency is always built even when a `-s/--cell` selection would
/// otherwise exclude it (or the producer lacks the selected axis entirely).
pub fn required_producers(
    consumer: &Cell,
    members: &[Arc<Target>],
) -> Result<Vec<(String, String)>, CoreError> {
    let mut required = Vec::new();
    if let BaseSource::Image {
        image,
        output,
        cell: pins,
    } = &consumer.base
    {
        let (producer, matched) = resolve_producer(consumer, image, *output, pins, members)?;
        required.push((producer, matched.slug.as_ref().to_owned()));
    }
    for spec in &consumer.target.definition.inputs {
        let (producer, matched) =
            resolve_producer(consumer, &spec.image, spec.output, &spec.cell, members)?;
        required.push((producer, matched.slug.as_ref().to_owned()));
    }
    Ok(required)
}

fn resolve_output(
    image: &str,
    producer: &str,
    output: Option<OutputFormat>,
    producer_cells: &[Cell],
) -> Result<OutputFormat, CoreError> {
    let formats: BTreeSet<OutputFormat> = producer_cells.iter().map(|c| c.output.format).collect();
    match output {
        Some(format) if formats.contains(&format) => Ok(format),
        Some(format) => Err(CoreError::UnknownProducerOutput {
            image: image.to_owned(),
            producer: producer.to_owned(),
            output: format.as_str().to_owned(),
            available: join_formats(&formats),
        }),
        None if formats.len() == 1 => Ok(formats
            .into_iter()
            .next()
            .expect("invariant: formats.len() == 1")),
        None => Err(CoreError::UnknownProducerOutput {
            image: image.to_owned(),
            producer: producer.to_owned(),
            output: "<unspecified>".to_owned(),
            available: join_formats(&formats),
        }),
    }
}

/// Every axis the producer declares, mapped to the set of values it takes across its cells.
fn axis_domain(producer_cells: &[Cell]) -> BTreeMap<String, BTreeSet<String>> {
    let mut domain: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for cell in producer_cells {
        for (axis, value) in &cell.axes {
            domain
                .entry(axis.clone())
                .or_default()
                .insert(value.clone());
        }
    }
    domain
}

fn validate_pins(
    image: &str,
    producer: &str,
    pins: &BTreeMap<String, String>,
    producer_axes: &BTreeMap<String, BTreeSet<String>>,
) -> Result<(), CoreError> {
    for (axis, value) in pins {
        match producer_axes.get(axis) {
            None => {
                return Err(CoreError::BadDependencyPin {
                    image: image.to_owned(),
                    producer: producer.to_owned(),
                    axis: axis.clone(),
                    value: value.clone(),
                    detail: format!(
                        "it has no such axis (axes: {})",
                        producer_axes.keys().cloned().collect::<Vec<_>>().join(", ")
                    ),
                });
            }
            Some(values) if !values.contains(value) => {
                return Err(CoreError::BadDependencyPin {
                    image: image.to_owned(),
                    producer: producer.to_owned(),
                    axis: axis.clone(),
                    value: value.clone(),
                    detail: format!(
                        "`{axis}` has no value `{value}` ({})",
                        values.iter().cloned().collect::<Vec<_>>().join(", ")
                    ),
                });
            }
            Some(_) => {}
        }
    }
    Ok(())
}

fn axes_satisfied(producer_cell: &Cell, consumer: &Cell, pins: &BTreeMap<String, String>) -> bool {
    producer_cell.axes.iter().all(|(axis, value)| {
        if let Some(pinned) = pins.get(axis) {
            pinned == value
        } else if let Some(shared) = consumer.axes.get(axis) {
            shared == value
        } else {
            true
        }
    })
}

fn ambiguous_error(
    image: &str,
    consumer: &Cell,
    producer: &str,
    pins: &BTreeMap<String, String>,
    matched: &[&Cell],
) -> CoreError {
    // The free axis (producer-declared, neither pinned nor shared) whose variation causes ambiguity.
    let free_axis = matched
        .iter()
        .flat_map(|c| c.axes.keys())
        .find(|axis| !pins.contains_key(*axis) && !consumer.axes.contains_key(*axis))
        .cloned()
        .unwrap_or_default();
    let values: BTreeSet<String> = matched
        .iter()
        .filter_map(|c| c.axes.get(&free_axis).cloned())
        .collect();
    CoreError::AmbiguousProducerCell {
        image: image.to_owned(),
        slug: consumer.slug.as_ref().to_owned(),
        producer: producer.to_owned(),
        axis: free_axis,
        values: values.into_iter().collect::<Vec<_>>().join(", "),
    }
}

/// The coordinate the consumer asked for (shared axes it constrains + explicit pins), for errors.
fn requested_coord(
    consumer: &Cell,
    pins: &BTreeMap<String, String>,
    producer_axes: &BTreeMap<String, BTreeSet<String>>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    for axis in producer_axes.keys() {
        if let Some(value) = pins.get(axis).or_else(|| consumer.axes.get(axis)) {
            parts.push(format!("{axis}={value}"));
        }
    }
    if parts.is_empty() {
        "(single cell)".to_owned()
    } else {
        parts.join(", ")
    }
}

fn available_coords(producer_cells: &[Cell], format: OutputFormat) -> String {
    let coords: BTreeSet<String> = producer_cells
        .iter()
        .filter(|c| c.output.format == format)
        .map(|c| {
            c.axes
                .iter()
                .map(|(a, v)| format!("{a}={v}"))
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect();
    coords.into_iter().collect::<Vec<_>>().join("; ")
}

fn join_formats(formats: &BTreeSet<OutputFormat>) -> String {
    formats
        .iter()
        .map(|f| f.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Find and render a dependency cycle among the not-yet-emitted nodes, for the error message.
fn describe_cycle(nodes: &[Arc<Target>], node_index: &BTreeMap<&str, usize>) -> String {
    let mut path: Vec<usize> = Vec::new();
    let mut on_path = vec![false; nodes.len()];
    let mut visited = vec![false; nodes.len()];

    for start in 0..nodes.len() {
        if !visited[start]
            && let Some(cycle) = find_cycle(
                start,
                nodes,
                node_index,
                &mut path,
                &mut on_path,
                &mut visited,
            )
        {
            return cycle
                .iter()
                .map(|&i| nodes[i].name().to_owned())
                .collect::<Vec<_>>()
                .join(" → ");
        }
    }
    // Unreachable in practice (we only call this when a cycle exists), but degrade gracefully.
    "<cycle>".to_owned()
}

/// DFS from `idx` following in-set dependency edges; returns the first back-edge cycle (node indices)
/// it finds. Used only to render a [`CoreError::DependencyCycle`] message.
fn find_cycle(
    idx: usize,
    nodes: &[Arc<Target>],
    node_index: &BTreeMap<&str, usize>,
    path: &mut Vec<usize>,
    on_path: &mut [bool],
    visited: &mut [bool],
) -> Option<Vec<usize>> {
    visited[idx] = true;
    on_path[idx] = true;
    path.push(idx);
    for dep in dependencies(&nodes[idx]).unwrap_or_default() {
        if let Some(&producer) = node_index.get(dep.as_str()) {
            if on_path[producer] {
                let start = path.iter().position(|&p| p == producer).unwrap_or(0);
                let mut cycle = path[start..].to_vec();
                cycle.push(producer);
                return Some(cycle);
            }
            if !visited[producer]
                && let Some(cycle) = find_cycle(producer, nodes, node_index, path, on_path, visited)
            {
                return Some(cycle);
            }
        }
    }
    on_path[idx] = false;
    path.pop();
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use tempfile::TempDir;

    use tailor_config::{
        BaseImageCatalogue, OutputArtifactsPolicy, OutputFormat, OutputSpec, load_image,
    };

    /// Build a `Target` from an on-disk `image.yaml` (rendering reads fragments + the base document
    /// from disk, so the directory must exist). Default output is a single `cosi`.
    fn target(root: &Path, name: &str, yaml: &str) -> Arc<Target> {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("image.yaml"), yaml).unwrap();
        let definition = load_image(dir.join("image.yaml")).unwrap();
        Arc::new(Target {
            definition,
            dir,
            default_outputs: vec![OutputSpec {
                format: OutputFormat::Cosi,
                cosi_compression_level: None,
                compression: None,
                name: None,
            }],
            output_artifacts: OutputArtifactsPolicy::default(),
            root: root.to_path_buf(),
            base_images: BaseImageCatalogue::default(),
            tools_dir_sources: Vec::new(),
        })
    }

    const MATRIX_ARCH: &str = "matrix:\n  arch: [amd64, arm64]\n";

    fn producer(root: &Path, name: &str, matrix: bool) -> Arc<Target> {
        let matrix = if matrix { MATRIX_ARCH } else { "" };
        target(
            root,
            name,
            &format!(
                "name: {name}\nbase:\n  path: ./b.img\n{matrix}config:\n  os: {{ hostname: {name} }}\n"
            ),
        )
    }

    fn consumer_on(root: &Path, name: &str, base: &str) -> Arc<Target> {
        target(
            root,
            name,
            &format!(
                "name: {name}\n{MATRIX_ARCH}base:\n{base}config:\n  os: {{ hostname: {name} }}\n"
            ),
        )
    }

    #[test]
    fn resolve_inputs_substitutes_and_records_the_producer_path() {
        let tmp = TempDir::new().unwrap();
        let payload = producer(tmp.path(), "payload", true);
        // A consumer with a path base and a `${inputs.payload}` reference in its config.
        let iso = target(
            tmp.path(),
            "iso",
            &format!(
                "name: iso\n{MATRIX_ARCH}base:\n  path: ./iso.img\ninputs:\n  - name: payload\n    \
                 image: payload\n    output: cosi\nconfig:\n  os:\n    additionalFiles:\n      \
                 - source: \"${{inputs.payload}}\"\n        destination: /images/payload.cosi\n"
            ),
        );
        let members = vec![Arc::clone(&payload), Arc::clone(&iso)];
        let output_dir = tmp.path().join("artifacts");

        let mut cells = cells(&iso).unwrap();
        resolve_inputs(&mut cells, &members, &output_dir).unwrap();

        for cell in &cells {
            let arch = cell.axes.get("arch").unwrap();
            let expected = output_dir.join(format!("payload_{arch}_cosi.cosi"));
            // The token is replaced with the arch-paired producer artifact path...
            let source = cell.ic_config["os"]["additionalFiles"][0]["source"]
                .as_str()
                .unwrap();
            assert_eq!(source, expected.to_string_lossy());
            // ...and the path is recorded as a content-hashed dependency.
            assert_eq!(cell.input_deps, vec![expected]);
        }
    }

    #[test]
    fn an_unknown_input_reference_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let payload = producer(tmp.path(), "payload", false);
        let iso = target(
            tmp.path(),
            "iso",
            "name: iso\nbase:\n  path: ./iso.img\ninputs:\n  - name: payload\n    image: payload\n\
             config:\n  os:\n    additionalFiles:\n      - source: \"${inputs.missing}\"\n        \
             destination: /x\n",
        );
        let members = vec![Arc::clone(&payload), Arc::clone(&iso)];
        let mut cells = cells(&iso).unwrap();
        let err = resolve_inputs(&mut cells, &members, tmp.path()).unwrap_err();
        assert!(
            matches!(err, CoreError::UnknownInput { ref name, .. } if name == "missing"),
            "got {err:?}"
        );
    }

    #[test]
    fn an_image_input_creates_a_dependency_edge() {
        let tmp = TempDir::new().unwrap();
        let payload = producer(tmp.path(), "payload", false);
        // `iso` references `payload` only via `inputs`, with an unrelated path base.
        let iso = target(
            tmp.path(),
            "iso",
            "name: iso\nbase:\n  path: ./iso.img\ninputs:\n  - name: payload\n    image: payload\n\
             config:\n  os: { hostname: iso }\n",
        );
        let members = vec![Arc::clone(&payload), Arc::clone(&iso)];
        let ordered = topological_order(
            &dependency_closure(&[Arc::clone(&iso)], &members).unwrap(),
            &members,
        )
        .unwrap();
        let names: Vec<&str> = ordered.iter().map(|t| t.name()).collect();
        assert_eq!(
            names,
            ["payload", "iso"],
            "input producer must precede consumer"
        );
    }

    #[test]
    fn topological_order_places_producer_before_consumer() {
        let tmp = TempDir::new().unwrap();
        let base = producer(tmp.path(), "base", true);
        let derived = consumer_on(tmp.path(), "derived", "  image: base\n");
        let members = vec![Arc::clone(&base), Arc::clone(&derived)];

        let closure = dependency_closure(&[Arc::clone(&derived)], &members).unwrap();
        let ordered = topological_order(&closure, &members).unwrap();
        let names: Vec<&str> = ordered.iter().map(|t| t.name()).collect();
        assert_eq!(names, ["base", "derived"], "producer must precede consumer");
    }

    #[test]
    fn a_cycle_is_rejected() {
        let tmp = TempDir::new().unwrap();
        // a bases on b, b depends on a → cycle.
        let a = consumer_on(tmp.path(), "a", "  image: b\n");
        let b = target(
            tmp.path(),
            "b",
            &format!(
                "name: b\n{MATRIX_ARCH}base:\n  path: ./b.img\ndependsOn: [a]\nconfig:\n  os: {{ hostname: b }}\n"
            ),
        );
        let members = vec![Arc::clone(&a), Arc::clone(&b)];
        let err = topological_order(&members, &members).unwrap_err();
        assert!(
            matches!(err, CoreError::DependencyCycle { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn unknown_dependency_image_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let derived = consumer_on(tmp.path(), "derived", "  image: missing\n");
        let members = vec![Arc::clone(&derived)];
        let err = dependency_closure(&[derived], &members).unwrap_err();
        assert!(
            matches!(err, CoreError::UnknownDependencyImage { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn lower_pairs_by_arch_to_the_producer_artifact() {
        let tmp = TempDir::new().unwrap();
        let base = producer(tmp.path(), "base", true);
        let derived = consumer_on(tmp.path(), "derived", "  image: base\n");
        let members = vec![Arc::clone(&base), Arc::clone(&derived)];
        let output_dir = tmp.path().join("artifacts");

        let mut cells = cells(&derived).unwrap();
        lower_image_bases(&mut cells, &members, &output_dir).unwrap();

        for cell in &cells {
            let arch = cell.axes.get("arch").unwrap();
            let BaseSource::Path { path, .. } = &cell.base else {
                panic!("base not lowered to a path: {:?}", cell.base);
            };
            assert_eq!(path, &output_dir.join(format!("base_{arch}_cosi.cosi")));
        }
    }

    #[test]
    fn a_producer_only_axis_without_a_pin_is_ambiguous() {
        let tmp = TempDir::new().unwrap();
        // producer has arch + flavor; consumer has only arch and no pin.
        let base = target(
            tmp.path(),
            "base",
            "name: base\nbase:\n  path: ./b.img\nmatrix:\n  arch: [amd64, arm64]\n  flavor: [min, net]\nconfig:\n  os: { hostname: base }\n",
        );
        let derived = consumer_on(tmp.path(), "derived", "  image: base\n");
        let members = vec![Arc::clone(&base), Arc::clone(&derived)];

        let mut cells = cells(&derived).unwrap();
        let err = lower_image_bases(&mut cells, &members, tmp.path()).unwrap_err();
        assert!(
            matches!(err, CoreError::AmbiguousProducerCell { ref axis, .. } if axis == "flavor"),
            "got {err:?}"
        );
    }

    #[test]
    fn required_producers_pairs_the_consumer_to_the_matching_producer_cell() {
        // The scheduler uses this to build a producer's paired cell even when a `-s`/`--cell`
        // selection meant for the consumer would exclude it. Each arch-paired consumer cell must
        // require exactly its arch-matched producer cell.
        let tmp = TempDir::new().unwrap();
        let base = producer(tmp.path(), "base", true);
        let derived = consumer_on(tmp.path(), "derived", "  image: base\n");
        let members = vec![Arc::clone(&base), Arc::clone(&derived)];

        for cell in cells(&derived).unwrap() {
            let arch = cell.axes.get("arch").unwrap();
            let required = required_producers(&cell, &members).unwrap();
            assert_eq!(
                required,
                vec![("base".to_owned(), format!("base_{arch}_cosi"))],
                "consumer {} must require its arch-paired producer cell",
                cell.slug.as_ref()
            );
        }
    }

    #[test]
    fn required_producers_covers_both_base_and_inputs() {
        let tmp = TempDir::new().unwrap();
        let base = producer(tmp.path(), "base", true);
        let payload = producer(tmp.path(), "payload", true);
        // `derived` both bases on `base` and embeds `payload` as an input.
        let derived = target(
            tmp.path(),
            "derived",
            &format!(
                "name: derived\n{MATRIX_ARCH}base:\n  image: base\ninputs:\n  - name: payload\n    \
                 image: payload\n    output: cosi\nconfig:\n  os: {{ hostname: derived }}\n"
            ),
        );
        let members = vec![
            Arc::clone(&base),
            Arc::clone(&payload),
            Arc::clone(&derived),
        ];

        for cell in cells(&derived).unwrap() {
            let arch = cell.axes.get("arch").unwrap();
            let required = required_producers(&cell, &members).unwrap();
            assert_eq!(
                required,
                vec![
                    ("base".to_owned(), format!("base_{arch}_cosi")),
                    ("payload".to_owned(), format!("payload_{arch}_cosi")),
                ],
                "consumer must require both its base and input producer cells"
            );
        }
    }
}
