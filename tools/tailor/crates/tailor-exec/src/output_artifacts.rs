//! `output.artifacts` staging management (`meta/docs/2026-06-29-output-artifacts-staging.md`). IC's
//! `output.artifacts` feature extracts boot artifacts to a directory it resolves relative to the
//! config file — i.e. tailor's colocated working copy in the image dir — leaving root-owned droppings
//! in the source tree. This module gates on the `output-artifacts` preview flag and rewrites the
//! working-copy config so IC's scratch lands somewhere tailor owns and can reclaim sudo-free.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde_yaml_ng::Value;
use tailor_config::OutputArtifactsPolicy;

use crate::path_translate;

/// The IC preview-feature flag that gates `output.artifacts` (IC docs). Its presence is tailor's
/// single activation predicate today (§3.1); a stabilized-API detector is added alongside later.
const OUTPUT_ARTIFACTS_FEATURE: &str = "output-artifacts";
const PREVIEW_FEATURES_KEY: &str = "previewFeatures";
const OUTPUT_KEY: &str = "output";
const ARTIFACTS_KEY: &str = "artifacts";
const PATH_KEY: &str = "path";
/// Prefix of the hidden, tailor-named staging directories (so the crash sweep can recognise its own).
const STAGE_PREFIX: &str = ".tailor-stage";
/// Suffix of the enrollable CA cert dropped beside a signed image (`meta/docs/2026-06-29-signing.md` §6).
const CA_CERT_SUFFIX: &str = "ca_cert.pem";

static STAGING_SEQ: AtomicU64 = AtomicU64::new(0);

/// The per-cell published CA cert filename, `<slug>.ca_cert.pem` — paired 1:1 with the image so
/// concurrent tailor instances never clobber a shared cert (`meta/docs/2026-06-29-signing.md` §6). Used by the
/// executor to place the cert and by `tailor clean` to remove it.
pub fn ca_cert_name(slug: &str) -> String {
    format!("{slug}.{CA_CERT_SUFFIX}")
}

/// What tailor does with the staging directory after IC runs (§3.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StagingPlan {
    /// The host directory IC extracts into.
    pub(crate) dir: PathBuf,
    /// `true` ⇒ chown then reclaim (scratch); `false` ⇒ chown and keep as an output (managed).
    pub(crate) reclaim: bool,
}

/// A short identifier unique per IC invocation within and across processes (pid + a monotonic
/// counter), so staging dirs from retries, clones, and concurrent runs never collide (§7).
pub(crate) fn run_id() -> String {
    let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{}-{seq}", std::process::id())
}

/// The activation gate (§3.1): does this cell opt into the `output-artifacts` preview feature?
pub fn uses_output_artifacts(ic_config: &Value) -> bool {
    ic_config
        .get(PREVIEW_FEATURES_KEY)
        .and_then(Value::as_sequence)
        .is_some_and(|features| {
            features
                .iter()
                .any(|f| f.as_str() == Some(OUTPUT_ARTIFACTS_FEATURE))
        })
}

/// Apply the cell's policy to its working-copy IC config (§3). When the gate is off, does nothing.
/// Otherwise rewrites `output.artifacts.path` (managed/scratch) or removes the block (strip), and
/// returns the staging plan for the caller to chown/reclaim. `image_dir` is the working copy's
/// directory (IC's relative-path anchor); `output_dir` is where managed artifacts are kept.
pub(crate) fn apply(
    ic_config: &mut Value,
    policy: OutputArtifactsPolicy,
    slug: &str,
    run_id: &str,
    image_dir: &Path,
    output_dir: &Path,
    host_root: &Path,
) -> Option<StagingPlan> {
    if !uses_output_artifacts(ic_config) {
        return None;
    }
    match policy {
        OutputArtifactsPolicy::Strip => {
            strip(ic_config);
            None
        }
        // Hidden, colocated, tailor-named scratch (relative path → IC resolves it against the
        // working copy's dir); reclaimed after the run.
        OutputArtifactsPolicy::Scratch => {
            let name = format!("{STAGE_PREFIX}.{slug}.{run_id}");
            set_artifacts_path(ic_config, &format!("./{name}"));
            Some(StagingPlan {
                dir: image_dir.join(name),
                reclaim: true,
            })
        }
        // A real cell output alongside the image (absolute container path under the output dir);
        // chowned to the caller and kept.
        OutputArtifactsPolicy::Managed => {
            let dir = output_dir.join(format!("{slug}.artifacts"));
            set_artifacts_path(
                ic_config,
                &path_translate::to_container_path(&dir, host_root),
            );
            Some(StagingPlan {
                dir,
                reclaim: false,
            })
        }
    }
}

/// Stale tailor-named staging dirs for `slug` left in `image_dir` by a crashed prior run (§3.5). Only
/// ever matches tailor's own `<STAGE_PREFIX>.<slug>.*` — never a user-named directory.
pub(crate) fn stale_staging_dirs(image_dir: &Path, slug: &str) -> Vec<PathBuf> {
    let prefix = format!("{STAGE_PREFIX}.{slug}.");
    let Ok(entries) = std::fs::read_dir(image_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix))
        })
        .collect()
}

/// Overwrite `output.artifacts.path` in place. No-op if the cell declared no `output.artifacts`
/// mapping (a `previewFeatures` flag without a block — IC would extract nothing).
fn set_artifacts_path(ic_config: &mut Value, path: &str) {
    if let Some(artifacts) = ic_config
        .get_mut(OUTPUT_KEY)
        .and_then(|output| output.get_mut(ARTIFACTS_KEY))
        .and_then(Value::as_mapping_mut)
    {
        artifacts.insert(Value::from(PATH_KEY), Value::from(path));
    }
}

/// Remove the `output.artifacts` block so IC never extracts, plus the now-unused `output-artifacts`
/// preview flag (safe: that flag gates only `output.artifacts`). Prunes `output`/`previewFeatures`
/// if they become empty.
fn strip(ic_config: &mut Value) {
    let Some(root) = ic_config.as_mapping_mut() else {
        return;
    };
    let output_now_empty = match root.get_mut(OUTPUT_KEY).and_then(Value::as_mapping_mut) {
        Some(output) => {
            output.remove(ARTIFACTS_KEY);
            output.is_empty()
        }
        None => false,
    };
    if output_now_empty {
        root.remove(OUTPUT_KEY);
    }
    let flags_now_empty = match root
        .get_mut(PREVIEW_FEATURES_KEY)
        .and_then(Value::as_sequence_mut)
    {
        Some(features) => {
            features.retain(|f| f.as_str() != Some(OUTPUT_ARTIFACTS_FEATURE));
            features.is_empty()
        }
        None => false,
    };
    if flags_now_empty {
        root.remove(PREVIEW_FEATURES_KEY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(yaml: &str) -> Value {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

    const GATED: &str = "previewFeatures:\n  - uki\n  - output-artifacts\noutput:\n  artifacts:\n    items: [ukis]\n    path: ./output\n";

    #[test]
    fn gate_detects_the_preview_flag() {
        assert!(uses_output_artifacts(&config(GATED)));
        assert!(!uses_output_artifacts(&config(
            "previewFeatures: [uki]\noutput:\n  artifacts:\n    path: ./output\n"
        )));
        assert!(!uses_output_artifacts(&config("os:\n  hostname: x\n")));
    }

    #[test]
    fn gate_off_is_a_noop() {
        let mut cfg = config("os:\n  hostname: x\n");
        let before = cfg.clone();
        let plan = apply(
            &mut cfg,
            OutputArtifactsPolicy::Managed,
            "slug",
            "1",
            Path::new("/img"),
            Path::new("/out"),
            Path::new("/"),
        );
        assert!(plan.is_none());
        assert_eq!(cfg, before);
    }

    #[test]
    fn scratch_relocates_to_a_hidden_colocated_dir_and_reclaims() {
        let mut cfg = config(GATED);
        let plan = apply(
            &mut cfg,
            OutputArtifactsPolicy::Scratch,
            "img_amd64_cosi",
            "42-0",
            Path::new("/img"),
            Path::new("/out"),
            Path::new("/"),
        )
        .unwrap();
        assert_eq!(
            plan.dir,
            PathBuf::from("/img/.tailor-stage.img_amd64_cosi.42-0")
        );
        assert!(plan.reclaim);
        assert_eq!(
            cfg["output"]["artifacts"]["path"].as_str(),
            Some("./.tailor-stage.img_amd64_cosi.42-0")
        );
        // The user's intent (items) is untouched.
        assert_eq!(
            cfg["output"]["artifacts"]["items"][0].as_str(),
            Some("ukis")
        );
    }

    #[test]
    fn managed_relocates_to_the_output_dir_and_keeps() {
        let mut cfg = config(GATED);
        let plan = apply(
            &mut cfg,
            OutputArtifactsPolicy::Managed,
            "img_amd64_cosi",
            "42-0",
            Path::new("/img"),
            Path::new("/home/u/out"),
            Path::new("/host"),
        )
        .unwrap();
        assert_eq!(
            plan.dir,
            PathBuf::from("/home/u/out/img_amd64_cosi.artifacts")
        );
        assert!(!plan.reclaim);
        assert_eq!(
            cfg["output"]["artifacts"]["path"].as_str(),
            Some("/host/home/u/out/img_amd64_cosi.artifacts")
        );
    }

    #[test]
    fn strip_removes_the_block_and_the_flag() {
        let mut cfg = config(GATED);
        let plan = apply(
            &mut cfg,
            OutputArtifactsPolicy::Strip,
            "slug",
            "1",
            Path::new("/img"),
            Path::new("/out"),
            Path::new("/"),
        );
        assert!(plan.is_none());
        assert!(cfg.get("output").is_none(), "output pruned: {cfg:?}");
        // The other preview feature survives; only output-artifacts is dropped.
        let features = cfg["previewFeatures"].as_sequence().unwrap();
        assert_eq!(features.len(), 1);
        assert_eq!(features[0].as_str(), Some("uki"));
    }
}
