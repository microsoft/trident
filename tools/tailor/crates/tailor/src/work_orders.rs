use std::{
    collections::BTreeSet,
    env::{self, VarError},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::Serialize;

use tailor_config::{self as config, Operation, ToolConfig, WorkOrder};
use tailor_core::{BuildSchedule, Cell, DependencyEdge, Target, orchestrator, schedule};
use tailor_exec as exec;

use crate::{
    cli::{BuildArgs, PlanFormat},
    error::AppError,
};

const PLAN_SCHEMA_VERSION: u32 = 1;

pub(super) fn requested(
    args: &BuildArgs,
    members: &[Arc<Target>],
) -> Result<Option<Vec<Cell>>, AppError> {
    let (context, input) = match (&args.work_order, &args.work_order_env) {
        (Some(path), None) => (
            format!("work order {}", path.display()),
            fs::read_to_string(path).map_err(|source| {
                AppError::Message(format!("read work order {}: {source}", path.display()))
            })?,
        ),
        (None, Some(name)) => {
            let context = format!("work-order environment variable {name}");
            let input = env::var(name).map_err(|source| {
                let detail = match source {
                    VarError::NotPresent => "is missing",
                    VarError::NotUnicode(_) => "is not UTF-8",
                };
                AppError::Request(format!("{context} {detail}"))
            })?;
            if input.trim().is_empty() {
                return Err(AppError::Request(format!("{context} is empty")));
            }
            (context, input)
        }
        (None, None) => return Ok(None),
        (Some(_), Some(_)) => {
            return Err(AppError::Request(
                "work-order file and environment sources are mutually exclusive".to_owned(),
            ));
        }
    };
    let order = WorkOrder::parse(&input).map_err(|source| AppError::WorkOrderConfig {
        context: context.clone(),
        source,
    })?;
    schedule::select_work_order(&order, members)
        .map(Some)
        .map_err(|source| AppError::WorkOrderSelection {
            context,
            source: Box::new(source),
        })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlanDocument<'a> {
    schema_version: u32,
    requested: &'a BTreeSet<String>,
    cells: Vec<PlanCell<'a>>,
    edges: &'a [DependencyEdge],
    order: Vec<&'a str>,
    steps: Vec<PlanStep>,
}

#[derive(Serialize)]
struct PlanCell<'a> {
    image: &'a str,
    slug: &'a str,
    requested: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlanStep {
    number: usize,
    slug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    clone_index: Option<u32>,
    output: PathBuf,
    actions: Vec<&'static str>,
}

pub(super) fn print_plan(
    schedule: &BuildSchedule,
    tool: &ToolConfig,
    output_dir: &Path,
    clones: u32,
    format: PlanFormat,
) -> Result<(), AppError> {
    let mut steps = Vec::new();
    let clones = clones.max(1);
    for target in schedule.targets() {
        for clone in 0..clones {
            for cell in schedule
                .cells()
                .iter()
                .filter(|cell| cell.target.name() == target.name())
            {
                let clone_index = (clones > 1).then_some(clone);
                let slug = orchestrator::output_slug(cell.slug.as_ref(), clone_index);
                let signed = config::resolve_signing(
                    target.definition.signing.as_ref(),
                    tool.signing.as_ref(),
                )?
                .is_some();
                let operation = if signed {
                    Operation::Customize
                } else {
                    target.definition.operation.unwrap_or_default()
                };
                let mut actions = vec![match operation {
                    Operation::Customize => "customize",
                    Operation::Convert => "convert",
                }];
                if signed
                    || (operation == Operation::Customize
                        && exec::extracts_output_artifacts(
                            &cell.ic_config,
                            target.output_artifacts,
                        ))
                {
                    actions.push("extract");
                }
                if signed {
                    actions.extend(["sign", "inject"]);
                }
                if cell.output.compression.is_some() {
                    actions.push("compress");
                }
                actions.push("publish");
                steps.push(PlanStep {
                    number: steps.len() + 1,
                    output: output_dir.join(orchestrator::published_artifact_name(
                        &slug,
                        cell.output.format,
                        cell.output.compression,
                    )),
                    slug,
                    clone_index,
                    actions,
                });
            }
        }
    }
    let document = PlanDocument {
        schema_version: PLAN_SCHEMA_VERSION,
        requested: schedule.requested(),
        cells: schedule
            .cells()
            .iter()
            .map(|cell| PlanCell {
                image: cell.target.name(),
                slug: cell.slug.as_ref(),
                requested: schedule.requested().contains(cell.slug.as_ref()),
            })
            .collect(),
        edges: schedule.edges(),
        order: schedule
            .cells()
            .iter()
            .map(|cell| cell.slug.as_ref())
            .collect(),
        steps,
    };
    match format {
        PlanFormat::Json => println!("{}", serde_json::to_string_pretty(&document)?),
        PlanFormat::Text => {
            println!("requested:");
            for slug in document.requested {
                println!("  {slug}");
            }
            println!("required:");
            for cell in document.cells.iter().filter(|cell| !cell.requested) {
                println!("  {}", cell.slug);
            }
            println!("dependencies:");
            for edge in document.edges {
                match edge {
                    DependencyEdge::Base { from_cell, to_cell } => {
                        println!("  {from_cell} --base--> {to_cell}");
                    }
                    DependencyEdge::Input {
                        from_cell,
                        to_cell,
                        input,
                    } => println!("  {from_cell} --input:{input}--> {to_cell}"),
                    DependencyEdge::DependsOn {
                        from_image,
                        to_image,
                    } => println!("  {from_image} --dependsOn--> {to_image}"),
                }
            }
            println!("execution order (serial):");
            for step in &document.steps {
                println!(
                    "  {}  {} -> {}  [{}]",
                    step.number,
                    step.slug,
                    step.output.display(),
                    step.actions.join(" -> ")
                );
            }
        }
    }
    Ok(())
}
