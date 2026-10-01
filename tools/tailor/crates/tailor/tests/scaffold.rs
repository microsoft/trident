//! Scaffolding integration tests for `tailor init` and `tailor add`.
//!
//! These run the real binary in a fresh temp directory and then exercise the generated project with
//! the pure verbs (`validate`/`list`/`matrix`), so a template that is invalid YAML — or that renders
//! to a broken cell — fails CI. No Docker or network is used.

use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn tailor() -> Command {
    Command::cargo_bin("tailor").unwrap()
}

/// A `tailor` invocation rooted at `dir`.
fn tailor_in(dir: &Path) -> Command {
    let mut cmd = tailor();
    cmd.current_dir(dir);
    cmd
}

// ───────────────────────────── init: templates are valid & render ─────────────────────────────

#[test]
fn init_base_scaffolds_a_validatable_workspace() {
    let tmp = TempDir::new().unwrap();
    tailor_in(tmp.path())
        .args(["init", "myimg"])
        .assert()
        .success();

    assert!(tmp.path().join("tailor.yaml").is_file());
    assert!(tmp.path().join("myimg/image.yaml").is_file());

    tailor_in(tmp.path())
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("myimg"));
    tailor_in(tmp.path())
        .arg("validate")
        .assert()
        .success()
        .stdout(predicate::str::contains("1 cell(s) valid"));
}

#[test]
fn init_with_no_template_is_the_same_as_base() {
    // `tailor init <name>` (omitted template) must behave exactly like `tailor init <name> base`.
    let default_tmp = TempDir::new().unwrap();
    tailor_in(default_tmp.path())
        .args(["init", "x"])
        .assert()
        .success();
    let explicit_tmp = TempDir::new().unwrap();
    tailor_in(explicit_tmp.path())
        .args(["init", "x", "base"])
        .assert()
        .success();

    let read = |root: &Path, rel: &str| std::fs::read_to_string(root.join(rel)).unwrap();
    assert_eq!(
        read(default_tmp.path(), "tailor.yaml"),
        read(explicit_tmp.path(), "tailor.yaml")
    );
    assert_eq!(
        read(default_tmp.path(), "x/image.yaml"),
        read(explicit_tmp.path(), "x/image.yaml")
    );
}

#[test]
fn init_simple_scaffolds_a_standalone_image() {
    let tmp = TempDir::new().unwrap();
    tailor_in(tmp.path())
        .args(["init", "webapp", "simple"])
        .assert()
        .success();

    assert!(tmp.path().join("image.yaml").is_file());
    assert!(
        !tmp.path().join("tailor.yaml").exists(),
        "simple has no manifest"
    );

    tailor_in(tmp.path())
        .arg("validate")
        .assert()
        .success()
        .stdout(predicate::str::contains("1 cell(s) valid"));
}

#[test]
fn init_advanced_scaffolds_a_rendering_matrix() {
    let tmp = TempDir::new().unwrap();
    tailor_in(tmp.path())
        .args(["init", "gadget", "advanced"])
        .assert()
        .success();

    for rel in [
        "tailor.yaml",
        "gadget/image.yaml",
        "gadget/by-variant/minimal.yaml",
        "gadget/by-variant/full.yaml",
        "gadget/by-arch/amd64.yaml",
        "gadget/by-arch/arm64.yaml",
    ] {
        assert!(tmp.path().join(rel).is_file(), "missing {rel}");
    }

    // variant[2] × arch[2] = 4 cells, and the ${efiArch} parameter must interpolate.
    tailor_in(tmp.path())
        .arg("validate")
        .assert()
        .success()
        .stdout(predicate::str::contains("4 cell(s) valid"));
    tailor_in(tmp.path())
        .args([
            "explain",
            "gadget",
            "--with-config",
            "-s",
            "variant=full,arch=amd64",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("grub2-efi-x64") // ${efiArch} → x64
                .and(predicate::str::contains("git")), // by-variant/full delta
        );
}

#[test]
fn init_refuses_to_overwrite_existing_files() {
    let tmp = TempDir::new().unwrap();
    tailor_in(tmp.path())
        .args(["init", "dup"])
        .assert()
        .success();
    tailor_in(tmp.path())
        .args(["init", "dup"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("refusing to overwrite"));
}

#[test]
fn init_rejects_a_name_with_path_separators() {
    let tmp = TempDir::new().unwrap();
    tailor_in(tmp.path())
        .args(["init", "a/b"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid name"));
}

// ───────────────────────────── add image / add axis ─────────────────────────────

#[test]
fn add_image_scaffolds_and_registers_a_member() {
    let tmp = TempDir::new().unwrap();
    tailor_in(tmp.path())
        .args(["init", "web"])
        .assert()
        .success();
    tailor_in(tmp.path())
        .args(["add", "image", "db"])
        .assert()
        .success();

    assert!(tmp.path().join("db/image.yaml").is_file());
    // Both the original and the added image are discoverable, and the manifest now lists the member.
    tailor_in(tmp.path())
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("web").and(predicate::str::contains("db")));
    let manifest = std::fs::read_to_string(tmp.path().join("tailor.yaml")).unwrap();
    assert!(manifest.contains("- db"), "member registered: {manifest}");
    tailor_in(tmp.path())
        .arg("validate")
        .assert()
        .success()
        .stdout(predicate::str::contains("db"));
}

#[test]
fn add_image_requires_a_manifest() {
    let tmp = TempDir::new().unwrap();
    tailor_in(tmp.path())
        .args(["add", "image", "x"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no tailor.yaml"));
}

#[test]
fn add_axis_infers_the_only_image_and_creates_the_matrix() {
    let tmp = TempDir::new().unwrap();
    tailor_in(tmp.path())
        .args(["init", "solo"])
        .assert()
        .success();
    // No image argument needed when the workspace has exactly one image.
    tailor_in(tmp.path())
        .args(["add", "axis", "variant"])
        .assert()
        .success();

    assert!(tmp.path().join("solo/by-variant").is_dir());
    tailor_in(tmp.path())
        .arg("validate")
        .assert()
        .success()
        .stdout(predicate::str::contains("1 cell(s) valid"));
}

#[test]
fn add_axis_appends_to_an_existing_matrix() {
    let tmp = TempDir::new().unwrap();
    tailor_in(tmp.path())
        .args(["init", "gadget", "advanced"])
        .assert()
        .success();
    tailor_in(tmp.path())
        .args(["add", "axis", "gadget", "release"])
        .assert()
        .success();

    assert!(tmp.path().join("gadget/by-release").is_dir());
    // variant[2] × arch[2] × release[1 placeholder] = 4 cells, still valid.
    tailor_in(tmp.path())
        .args(["show", "gadget"])
        .assert()
        .success()
        .stdout(predicate::str::contains("3 axis(es)").and(predicate::str::contains("release")));
    tailor_in(tmp.path()).arg("validate").assert().success();
}

#[test]
fn add_axis_in_a_multi_image_workspace_requires_naming_the_image() {
    let tmp = TempDir::new().unwrap();
    tailor_in(tmp.path()).args(["init", "a"]).assert().success();
    tailor_in(tmp.path())
        .args(["add", "image", "b"])
        .assert()
        .success();
    tailor_in(tmp.path())
        .args(["add", "axis", "variant"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("multiple images"));
}

#[test]
fn add_axis_rejects_a_duplicate() {
    let tmp = TempDir::new().unwrap();
    tailor_in(tmp.path())
        .args(["init", "g", "advanced"])
        .assert()
        .success();
    tailor_in(tmp.path())
        .args(["add", "axis", "g", "variant"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}
