//! Integration tests for image-level `skip:` — excluding an image from bulk selection so the
//! pipeline never grabs it, while still allowing an explicit build. No Docker or network.

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

/// A scaffolded workspace (`gadget`, 4 cells) plus an experimental `exp` image marked `skip: true`.
fn workspace_with_skip() -> TempDir {
    let tmp = TempDir::new().unwrap();
    tailor_in(tmp.path())
        .args(["init", "gadget", "advanced"])
        .assert()
        .success();
    let exp = tmp.path().join("exp");
    fs::create_dir_all(&exp).unwrap();
    fs::write(
        exp.join("image.yaml"),
        "name: exp\nskip: true\nbase: { path: ./base.raw }\noutputs: [{ format: cosi }]\nconfig: {}\n",
    )
    .unwrap();
    tmp
}

#[test]
fn bulk_selection_skips_a_skip_image() {
    let ws = workspace_with_skip();
    // No image named ⇒ bulk selection ⇒ the skip image is omitted, the normal one is present.
    tailor_in(ws.path())
        .arg("slugs")
        .assert()
        .success()
        .stdout(predicate::str::contains("gadget_").and(predicate::str::contains("exp_").not()));
}

#[test]
fn naming_a_skip_image_builds_it() {
    let ws = workspace_with_skip();
    // Explicitly naming the skip image overrides the skip.
    tailor_in(ws.path())
        .args(["slugs", "exp"])
        .assert()
        .success()
        .stdout(predicate::str::contains("exp_"));
}

#[test]
fn list_marks_a_skip_image() {
    let ws = workspace_with_skip();
    tailor_in(ws.path())
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("exp").and(predicate::str::contains("(skip)")));
}

// ───────────────────────────── fragment/value-level skip ─────────────────────────────

/// A 4-cell matrix workspace whose `variant=full` fragment is marked `skip: true`.
fn workspace_with_fragment_skip() -> TempDir {
    let tmp = TempDir::new().unwrap();
    tailor_in(tmp.path())
        .args(["init", "gadget", "advanced"])
        .assert()
        .success();
    let frag = tmp.path().join("gadget/by-variant/full.yaml");
    let orig = fs::read_to_string(&frag).unwrap();
    fs::write(&frag, format!("skip: true\n{orig}")).unwrap();
    tmp
}

#[test]
fn fragment_skip_drops_matching_cells_in_bulk() {
    let ws = workspace_with_fragment_skip();
    tailor_in(ws.path()).arg("slugs").assert().success().stdout(
        predicate::str::contains("gadget_minimal_").and(predicate::str::contains("full").not()),
    );
}

#[test]
fn pinning_the_skip_value_keeps_the_cells() {
    let ws = workspace_with_fragment_skip();
    tailor_in(ws.path())
        .args(["slugs", "-s", "variant=full"])
        .assert()
        .success()
        .stdout(predicate::str::contains("gadget_full_"));
}

#[test]
fn a_non_pinning_selector_does_not_resurrect_skip_cells() {
    // `-s arch=amd64` matches the skipped cells but does not pin `variant=full`, so they stay dropped.
    let ws = workspace_with_fragment_skip();
    tailor_in(ws.path())
        .args(["slugs", "-s", "arch=amd64"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("gadget_minimal_amd64")
                .and(predicate::str::contains("full").not()),
        );
}

#[test]
fn naming_a_skip_cell_keeps_it() {
    let ws = workspace_with_fragment_skip();
    tailor_in(ws.path())
        .args(["slugs", "--cell", "gadget_full_amd64_cosi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("gadget_full_amd64_cosi"));
}
