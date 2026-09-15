use std::{fs, io, path::PathBuf};

use serde_yaml_ng::Value;

use tailor_core::Cell;

const WORKING_COPY_PREFIX: &str = ".tailor-render";
const WORKING_COPY_SUFFIX: &str = "ic.yaml";

#[derive(Debug, thiserror::Error)]
pub enum WorkingCopyError {
    #[error("failed to serialize Image Customizer working copy: {0}")]
    Serde(#[from] serde_yaml_ng::Error),
}

/// Serialize the merged Image Customizer config verbatim for the colocated working copy.
///
/// tailor passes the user's config through unchanged — it does not inject `previewFeatures` or any
/// other IC field. The working copy exists only so IC resolves relative `files/`/`scripts/` paths
/// against the image's real directory (`meta/docs/2026-06-22-design.md` §7.6), not to edit the config.
pub fn render_working_copy(ic_config: &Value) -> Result<String, WorkingCopyError> {
    serde_yaml_ng::to_string(ic_config).map_err(WorkingCopyError::from)
}

pub(crate) fn working_copy_path(cell: &Cell, clone_index: Option<u32>) -> PathBuf {
    let slug = match clone_index {
        Some(index) => format!("{}_clone{index}", cell.slug.as_ref()),
        None => cell.slug.as_ref().to_owned(),
    };
    cell.target.dir.join(format!(
        "{WORKING_COPY_PREFIX}.{slug}.{WORKING_COPY_SUFFIX}"
    ))
}

pub fn write_working_copy(
    cell: &Cell,
    content: &str,
    clone_index: Option<u32>,
) -> io::Result<PathBuf> {
    let path = working_copy_path(cell, clone_index);
    fs::write(&path, content)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_merged_config_verbatim() {
        let config: Value = serde_yaml_ng::from_str(
            "previewFeatures:\n- uki\nos:\n  packages:\n    install:\n    - vim\n",
        )
        .unwrap();
        let rendered = render_working_copy(&config).unwrap();
        let round_tripped: Value = serde_yaml_ng::from_str(&rendered).unwrap();
        // The author's config (incl. their own previewFeatures) is passed through untouched.
        assert_eq!(round_tripped, config);
    }
}
