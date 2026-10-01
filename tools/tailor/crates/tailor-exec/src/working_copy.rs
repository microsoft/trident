use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde_yaml_ng::Value;

use tailor_core::Cell;

use crate::path_translate;

const WORKING_COPY_PREFIX: &str = ".tailor-render";
const WORKING_COPY_SUFFIX: &str = "ic.yaml";

#[derive(Debug, thiserror::Error)]
pub enum WorkingCopyError {
    #[error("failed to serialize Image Customizer working copy: {0}")]
    Serde(#[from] serde_yaml_ng::Error),
}

/// Serialize the merged Image Customizer config for the colocated working copy that tailor's own IC
/// invocation consumes.
///
/// tailor passes the user's config through unchanged, with one exception: resolved `${inputs.<name>}`
/// producer-artifact paths (`input_deps`) are host paths, but IC runs in a container where the
/// workspace is mounted under `host_root` (`/host`). The `--config-file` path and every bind are
/// already translated in `arg_builder`; the input paths *inside* the config must match, or IC fails to
/// `stat` them (they resolve on the host, not in the container). So each input path is rewritten to its
/// container-namespace form here — the same translation, applied to the same mount.
///
/// This translation is deliberately scoped to the working copy tailor feeds to its own IC run; the
/// `render`/`export` golden keeps portable host paths, since those are consumed by external pipelines
/// that run IC with their own mount layout.
pub fn render_working_copy(
    ic_config: &Value,
    input_deps: &[PathBuf],
    host_root: &Path,
) -> Result<String, WorkingCopyError> {
    let rewrites: Vec<(String, String)> = input_deps
        .iter()
        .map(|dep| {
            (
                dep.to_string_lossy().into_owned(),
                path_translate::to_container_path(dep, host_root),
            )
        })
        .collect();
    if rewrites.is_empty() {
        return serde_yaml_ng::to_string(ic_config).map_err(WorkingCopyError::from);
    }
    let mut config = ic_config.clone();
    rewrite_input_paths(&mut config, &rewrites);
    serde_yaml_ng::to_string(&config).map_err(WorkingCopyError::from)
}

/// Replace each resolved input's host path with its container-namespace path in every string scalar.
fn rewrite_input_paths(value: &mut Value, rewrites: &[(String, String)]) {
    match value {
        Value::String(text) => {
            for (host, container) in rewrites {
                if text.contains(host.as_str()) {
                    *text = text.replace(host.as_str(), container);
                }
            }
        }
        Value::Sequence(items) => items
            .iter_mut()
            .for_each(|item| rewrite_input_paths(item, rewrites)),
        Value::Mapping(map) => map
            .iter_mut()
            .for_each(|(_key, item)| rewrite_input_paths(item, rewrites)),
        _ => {}
    }
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
        let rendered = render_working_copy(&config, &[], Path::new("/host")).unwrap();
        let round_tripped: Value = serde_yaml_ng::from_str(&rendered).unwrap();
        // The author's config (incl. their own previewFeatures) is passed through untouched.
        assert_eq!(round_tripped, config);
    }

    #[test]
    fn rewrites_input_paths_into_the_container_namespace() {
        // Regression: `${inputs.*}` resolves to a host artifact path; the working copy tailor feeds
        // to its own IC run must carry the container path (mounted under host_root), or IC fails to
        // `stat` the file inside the container.
        let host_path = "/home/me/ws/artifacts/producer_amd64_cosi.cosi";
        let config: Value = serde_yaml_ng::from_str(&format!(
            "iso:\n  additionalFiles:\n    - source: \"{host_path}\"\n      destination: /images/acl.cosi\n"
        ))
        .unwrap();
        let rendered =
            render_working_copy(&config, &[PathBuf::from(host_path)], Path::new("/host")).unwrap();
        assert!(
            rendered.contains("/host/home/me/ws/artifacts/producer_amd64_cosi.cosi"),
            "input path must be container-translated, got:\n{rendered}"
        );
        assert!(
            !rendered.contains("source: /home/me/ws/artifacts"),
            "the untranslated host path must not remain, got:\n{rendered}"
        );
    }

    #[test]
    fn leaves_non_input_paths_untouched() {
        // Only resolved input paths are rewritten; unrelated host paths in the user's config stay.
        let config: Value =
            serde_yaml_ng::from_str("os:\n  additionalFiles:\n    - source: /etc/hosts\n").unwrap();
        let rendered = render_working_copy(
            &config,
            &[PathBuf::from("/home/me/ws/artifacts/x.cosi")],
            Path::new("/host"),
        )
        .unwrap();
        assert!(rendered.contains("source: /etc/hosts"), "got:\n{rendered}");
    }
}
