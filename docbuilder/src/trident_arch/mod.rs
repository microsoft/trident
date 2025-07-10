use std::path::PathBuf;

use anyhow::{Context, Error, Ok};

use crate::TridentArchSelection;

mod nodes;
mod render;

use nodes::Diagram;

fn get_diagram_base(selected: TridentArchSelection) -> Result<Diagram, Error> {
    let file = match selected {
        TridentArchSelection::Install => "install.yaml",
        TridentArchSelection::Update => "update.yaml",
    };

    let full_path = PathBuf::from(file!())
        .parent()
        .context("Failed to get parent directory")?
        .join("diagrams")
        .join(file);

    let yaml = std::fs::read_to_string(&full_path)
        .with_context(|| format!("Failed to read diagram file: {full_path:?}"))?;

    serde_yaml::from_str(&yaml)
        .with_context(|| format!("Failed to parse YAML for diagram '{selected:?}'"))
}

pub(super) fn build_arch_diagram(selected: TridentArchSelection) -> Result<String, Error> {
    let diag = get_diagram_base(selected).context("Failed to get diagram base")?;

    let svg = render::render(diag).context("Failed to render diagram")?;

    Ok(svg.to_string())
}
