//! Integration tests for `tailor export` / `tailor export --check` (configs-only render-ahead).
//!
//! Each test scaffolds a real 4-cell matrix workspace via `tailor init … advanced`, so it runs
//! against a valid rendering workspace without hard-coding the schema. No Docker or network.

use std::{fs, path::Path};

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn tailor() -> Command {
    Command::cargo_bin("tailor").unwrap()
}

fn tailor_in(dir: &Path) -> Command {
    let mut cmd = tailor();
    cmd.current_dir(dir);
    cmd
}

/// A scaffolded 4-cell matrix workspace (`variant × arch`).
fn workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    tailor_in(tmp.path())
        .args(["init", "gadget", "advanced"])
        .assert()
        .success();
    tmp
}

/// Sorted `*.yaml` file names directly under `dir`.
fn yaml_files(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("yaml") {
            names.push(path.file_name().unwrap().to_str().unwrap().to_owned());
        }
    }
    names.sort();
    names
}

#[test]
fn export_writes_one_yaml_per_cell_and_check_passes() {
    let ws = workspace();
    let out = ws.path().join("rendered");

    tailor_in(ws.path())
        .args(["export", "--output-dir", "rendered"])
        .assert()
        .success();
    // The advanced template is a variant[2] × arch[2] = 4-cell matrix.
    assert_eq!(yaml_files(&out).len(), 4, "one <slug>.yaml per cell");

    tailor_in(ws.path())
        .args(["export", "--check", "--output-dir", "rendered"])
        .assert()
        .success()
        .stdout(predicate::str::contains("up to date"));
}

#[test]
fn exported_config_matches_the_render_golden_bytes() {
    let ws = workspace();
    tailor_in(ws.path())
        .args(["export", "--output-dir", "rendered"])
        .assert()
        .success();
    // `tailor render` writes the same golden per cell under the image dir's `.rendered/`.
    tailor_in(ws.path()).arg("render").assert().success();

    for name in yaml_files(&ws.path().join("rendered")) {
        let exported = fs::read(ws.path().join("rendered").join(&name)).unwrap();
        let golden = fs::read(ws.path().join("gadget/.rendered").join(&name)).unwrap();
        assert_eq!(
            exported, golden,
            "export must match the render golden for {name}"
        );
    }
}

#[test]
fn check_detects_changed_missing_and_extra_drift() {
    let ws = workspace();
    let out = ws.path().join("rendered");
    tailor_in(ws.path())
        .args(["export", "--output-dir", "rendered"])
        .assert()
        .success();
    let first = out.join(&yaml_files(&out)[0]);

    // changed
    let mut content = fs::read(&first).unwrap();
    content.extend_from_slice(b"# drift\n");
    fs::write(&first, &content).unwrap();
    tailor_in(ws.path())
        .args(["export", "--check", "--output-dir", "rendered"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("changed").and(predicate::str::contains("out of date")));

    // re-export fixes the drift
    tailor_in(ws.path())
        .args(["export", "--output-dir", "rendered"])
        .assert()
        .success();
    tailor_in(ws.path())
        .args(["export", "--check", "--output-dir", "rendered"])
        .assert()
        .success();

    // extra (stale) file
    fs::write(out.join("stale_cell.yaml"), "x: 1\n").unwrap();
    tailor_in(ws.path())
        .args(["export", "--check", "--output-dir", "rendered"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("extra"));

    // re-export prunes the stale file
    tailor_in(ws.path())
        .args(["export", "--output-dir", "rendered"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pruned"));
    assert!(!out.join("stale_cell.yaml").exists(), "stale file pruned");

    // missing file
    fs::remove_file(&first).unwrap();
    tailor_in(ws.path())
        .args(["export", "--check", "--output-dir", "rendered"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("missing"));
}

#[test]
fn export_block_makes_the_command_zero_arg() {
    let ws = workspace();
    // A declarative `export:` block (scope omitted → configsOnly) makes `tailor export` and
    // `tailor export --check` argument-free — the pre-commit/CI shape.
    let manifest = ws.path().join("tailor.yaml");
    let mut yaml = fs::read_to_string(&manifest).unwrap();
    yaml.push_str("\nexport:\n  outputDir: rendered\n");
    fs::write(&manifest, yaml).unwrap();

    tailor_in(ws.path()).arg("export").assert().success();
    assert_eq!(yaml_files(&ws.path().join("rendered")).len(), 4);

    tailor_in(ws.path())
        .args(["export", "--check"])
        .assert()
        .success()
        .stdout(predicate::str::contains("up to date"));
}

#[test]
fn export_without_a_dir_or_config_is_an_error() {
    let ws = workspace();
    tailor_in(ws.path())
        .arg("export")
        .assert()
        .failure()
        .stderr(predicate::str::contains("outputDir"));
}

#[test]
fn build_help_lists_the_build_dir_base_override() {
    tailor()
        .args(["build", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--build-dir-base"));
}
