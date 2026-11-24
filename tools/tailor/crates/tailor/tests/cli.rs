//! End-to-end CLI integration tests.
//!
//! These drive the real `tailor` binary (no Docker, no network — only the pure verbs) against the
//! synthetic fixture tree under `tests/fixtures/`. The fixtures are purpose-built test artifacts,
//! NOT real Image Customizer configs:
//!
//! - `workspace/`  — a two-image workspace (discovery, per-image toolchain override, defaults).
//! - `standalone/` — a single image with no manifest (standalone mode, built-in default toolchain).
//! - `matrix/`     — a 3-axis matrix exercising every complex render operation (fragment overlays,
//!   list append, `$set`/`$replace`/`$remove`/`$include`, parameter interpolation, base override,
//!   rpm sources, opaque IC passthrough).

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value as JsonValue;
use tempfile::TempDir;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn tailor() -> Command {
    Command::cargo_bin("tailor").unwrap()
}

/// A `tailor` invocation with the working directory set to a fixture image/workspace directory.
fn in_dir(rel: &str) -> Command {
    let mut cmd = tailor();
    cmd.current_dir(fixtures().join(rel));
    cmd
}

// ───────────────────────────── workspace: discovery, toolchains, defaults ─────────────────────────

#[test]
fn list_shows_discovered_images_and_toolchains() {
    in_dir("workspace").arg("list").assert().success().stdout(
        predicate::str::contains("app")
            .and(predicate::str::contains("db"))
            .and(predicate::str::contains("default: ic-main"))
            .and(predicate::str::contains("ic-old")),
    );
}

#[test]
fn list_via_manifest_flag_from_any_directory() {
    // `--manifest` should locate the workspace without changing the working directory.
    tailor()
        .arg("--manifest")
        .arg(fixtures().join("workspace/tailor.yaml"))
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("app").and(predicate::str::contains("db")));
}

#[test]
fn defaults_inheritance_and_architecture_override_change_cell_counts() {
    // `app` inherits the amd64-only default → 1 cell; `db` declares an arch axis → amd64 + arm64.
    in_dir("workspace")
        .args(["validate", "app"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 cell(s) valid"));
    in_dir("workspace")
        .args(["validate", "db"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 cell(s) valid"));
}

#[test]
fn per_image_toolchain_override_selects_a_different_image_customizer() {
    // `db` pins ic-old (1.0.0); `app` uses the workspace default ic-main (2.0.0). The dry-run
    // container reference reflects each image's resolved toolchain tag.
    in_dir("workspace")
        .args(["build", "--dry-run", "db"])
        .assert()
        .success()
        .stdout(predicate::str::contains("imagecustomizer:1.0.0"));
    in_dir("workspace")
        .args(["build", "--dry-run", "app"])
        .assert()
        .success()
        .stdout(predicate::str::contains("imagecustomizer:2.0.0"));
}

// ───────────────────────────── build: positional image names and cell slugs ───────────────────────

#[test]
fn build_accepts_a_cell_slug_as_a_positional() {
    // A slug uniquely identifies one cell, so `tailor build <slug>` builds just that cell without
    // naming the image.
    in_dir("workspace")
        .args(["build", "--dry-run", "db_arm64_cosi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 cell(s) (dry-run)"));
}

#[test]
fn build_rejects_an_unknown_positional_with_a_clear_error() {
    in_dir("workspace")
        .args(["build", "--dry-run", "not-a-real-slug"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no matching image or cell slug: `not-a-real-slug`",
        ));
}

fn order_only_workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("tailor.yaml"),
        "schemaVersion: 1\ntoolchains:\n  default: ic\n  entries:\n    - name: ic\n      container: registry.example/ic\n",
    )
    .unwrap();
    for (name, definition) in [
        (
            "foundation",
            "skip: true\nmatrix:\n  arch: [amd64, arm64]\n",
        ),
        ("prepare", "dependsOn: [foundation]\n"),
        (
            "consumer",
            "dependsOn: [prepare, foundation]\nmatrix:\n  channel: [stable, edge]\n",
        ),
    ] {
        let dir = tmp.path().join(name);
        fs::create_dir(&dir).unwrap();
        fs::write(
            dir.join("image.yaml"),
            format!("name: {name}\n{definition}base: {{ path: ./base.img }}\nconfig: {{}}\n"),
        )
        .unwrap();
    }
    tmp
}

fn dry_run_in(dir: &Path, args: &[&str]) -> String {
    let output = tailor()
        .current_dir(dir)
        .args(["build", "--dry-run"])
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(output).unwrap()
}

fn work_order_plan_in(dir: &Path, order: &str) -> JsonValue {
    let output = tailor()
        .current_dir(dir)
        .env("TAILOR_WORK_ORDER", order)
        .args([
            "build",
            "--work-order-env",
            "TAILOR_WORK_ORDER",
            "--plan",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&output).unwrap()
}

#[test]
fn work_order_additions_union_with_slices_and_preserve_prerequisites() {
    let ws = order_only_workspace();
    let order = "schemaVersion: 1\nimages: [consumer]\nselect: {channel: [stable]}\nslugs: [consumer_edge_amd64_cosi]\n";
    let plan = work_order_plan_in(ws.path(), order);
    assert_eq!(plan["requested"].as_array().unwrap().len(), 2);
    assert_eq!(plan["cells"].as_array().unwrap().len(), 5);
    assert_eq!(plan["steps"].as_array().unwrap().len(), 5);
    assert!(
        plan["cells"]
            .as_array()
            .unwrap()
            .iter()
            .any(|cell| { cell["slug"] == "foundation_arm64_cosi" && cell["requested"] == false })
    );

    fs::write(ws.path().join("order.yaml"), order).unwrap();
    let output = dry_run_in(ws.path(), &["-w", "order.yaml"]);
    assert!(output.contains("5 cell(s) (dry-run)"), "{output}");
    assert_eq!(output.matches("docker run").count(), 5);
}

#[test]
fn work_order_relative_path_uses_invocation_directory_with_an_external_manifest() {
    let ws = order_only_workspace();
    let invocation = TempDir::new().unwrap();
    fs::write(
        invocation.path().join("order.yaml"),
        "schemaVersion: 1\ncells: [consumer_stable_amd64_cosi]\n",
    )
    .unwrap();
    tailor()
        .current_dir(invocation.path())
        .arg("--manifest")
        .arg(ws.path().join("tailor.yaml"))
        .args(["build", "-w", "order.yaml", "--plan", "--format", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("foundation_arm64_cosi"));
}

#[test]
fn plan_is_offline_and_does_not_create_runtime_state_or_preflight_signers() {
    let ws = order_only_workspace();
    let manifest = ws.path().join("tailor.yaml");
    let original = fs::read_to_string(&manifest).unwrap();
    fs::write(
        manifest,
        format!(
            "{original}previewFeatures: [signing]\nsigning:\n  default: test-ca\n  profiles:\n    test-ca:\n      backend: local-test-ca\n"
        ),
    )
    .unwrap();
    let image = ws.path().join("prepare/image.yaml");
    let original = fs::read_to_string(&image).unwrap();
    fs::write(image, format!("{original}signing: true\n")).unwrap();
    tailor()
        .current_dir(ws.path())
        .env("PATH", "")
        .args([
            "build",
            "consumer_stable_amd64_cosi",
            "--plan",
            "--format",
            "json",
        ])
        .arg("--host")
        .arg("unix:///nonexistent-work-order-engine.sock")
        .arg("--log-dir")
        .arg(ws.path().join("logs"))
        .arg("--output-dir")
        .arg(ws.path().join("outputs"))
        .arg("--build-dir-base")
        .arg(ws.path().join("scratch"))
        .assert()
        .success()
        .stdout(predicate::str::contains("\"sign\""));
    for path in ["logs", "outputs", "scratch", ".tailor", "tailor.lock"] {
        assert!(!ws.path().join(path).exists(), "plan created {path}");
    }
}

#[test]
fn text_plan_explains_required_cells_and_typed_dependencies() {
    let ws = order_only_workspace();
    tailor()
        .current_dir(ws.path())
        .args(["build", "consumer_stable_amd64_cosi", "--plan"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("required:")
                .and(predicate::str::contains("foundation_arm64_cosi"))
                .and(predicate::str::contains(
                    "foundation --dependsOn--> prepare",
                ))
                .and(predicate::str::contains("execution order (serial):")),
        );
}

#[test]
fn plan_expands_clone_steps_without_duplicate_outputs() {
    let ws = order_only_workspace();
    let output = tailor()
        .current_dir(ws.path())
        .args([
            "build",
            "consumer_stable_amd64_cosi",
            "--clones",
            "2",
            "--plan",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let plan: JsonValue = serde_json::from_slice(&output).unwrap();
    assert_eq!(plan["cells"].as_array().unwrap().len(), 4);
    let steps = plan["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 8);
    let outputs: BTreeSet<_> = steps
        .iter()
        .map(|step| step["output"].as_str().unwrap())
        .collect();
    assert_eq!(outputs.len(), 8);
    assert_eq!(steps[0]["cloneIndex"], 0);
    assert_eq!(steps[2]["cloneIndex"], 1);
}

#[test]
fn plan_extraction_matches_policy_and_forced_signed_staging() {
    let ws = order_only_workspace();
    let manifest = ws.path().join("tailor.yaml");
    let original = fs::read_to_string(&manifest).unwrap();
    fs::write(
        manifest,
        format!(
            "{original}previewFeatures: [signing]\nsigning:\n  default: test-ca\n  profiles:\n    test-ca:\n      backend: local-test-ca\n"
        ),
    )
    .unwrap();
    let image = ws.path().join("consumer/image.yaml");
    let original = fs::read_to_string(&image).unwrap();
    for (policy, signed, feature, block, expected) in [
        ("strip", false, true, true, false),
        ("scratch", false, true, true, true),
        ("managed", false, false, true, false),
        ("managed", false, true, false, false),
        ("strip", true, true, true, true),
    ] {
        let flag = if feature {
            "  previewFeatures: [output-artifacts]\n"
        } else {
            ""
        };
        let artifacts = if block {
            "  output:\n    artifacts:\n      path: ./declared\n"
        } else {
            ""
        };
        let config = format!(
            "outputArtifacts: {policy}\nsigning: {signed}\nconfig:\n{flag}{artifacts}  os: {{hostname: test}}\n"
        );
        fs::write(&image, original.replace("config: {}", &config)).unwrap();
        let plan = work_order_plan_in(
            ws.path(),
            r#"{"schemaVersion":1,"cells":["consumer_stable_amd64_cosi"]}"#,
        );
        let step = plan["steps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|step| step["slug"] == "consumer_stable_amd64_cosi")
            .unwrap();
        assert_eq!(
            step["actions"]
                .as_array()
                .unwrap()
                .contains(&JsonValue::from("extract")),
            expected,
            "{policy}, signed={signed}, feature={feature}, block={block}"
        );
    }
}

#[test]
fn work_order_plan_uses_the_effective_convert_operation() {
    let ws = order_only_workspace();
    let manifest = ws.path().join("tailor.yaml");
    let original = fs::read_to_string(&manifest).unwrap();
    fs::write(
        manifest,
        format!(
            "{original}previewFeatures: [signing]\nsigning:\n  default: test-ca\n  profiles:\n    test-ca:\n      backend: local-test-ca\n"
        ),
    )
    .unwrap();
    let image = ws.path().join("consumer/image.yaml");
    let original = fs::read_to_string(&image).unwrap();
    for (signed, operation, extraction) in [(false, "convert", false), (true, "customize", true)] {
        let config = format!(
            "operation: convert\nsigning: {signed}\nconfig:\n  previewFeatures: [output-artifacts]\n  output:\n    artifacts:\n      path: ./declared\n"
        );
        fs::write(&image, original.replace("config: {}", &config)).unwrap();
        let plan = work_order_plan_in(
            ws.path(),
            r#"{"schemaVersion":1,"cells":["consumer_stable_amd64_cosi"]}"#,
        );
        let step = plan["steps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|step| step["slug"] == "consumer_stable_amd64_cosi")
            .unwrap();
        assert_eq!(step["actions"][0], operation);
        assert_eq!(
            step["actions"]
                .as_array()
                .unwrap()
                .contains(&JsonValue::from("extract")),
            extraction
        );
        let rendered = dry_run_in(ws.path(), &["consumer_stable_amd64_cosi"]);
        let invocation = rendered
            .split("\n\n")
            .find(|block| {
                block.contains("docker run") && block.contains("# consumer_stable_amd64_cosi")
            })
            .unwrap();
        assert!(
            invocation
                .lines()
                .any(|line| line.trim().trim_end_matches('\\').trim() == operation),
            "{invocation}"
        );
        if !signed {
            assert!(!invocation.contains("--config-file"), "{invocation}");
        }
    }
}

#[test]
fn work_order_slug_collisions_fail_before_emitting_jobs_or_builds() {
    let ws = order_only_workspace();
    fs::create_dir(ws.path().join("consumer_stable")).unwrap();
    fs::write(
        ws.path().join("consumer_stable/image.yaml"),
        "name: consumer_stable\nbase: {path: ./base.img}\nconfig: {}\n",
    )
    .unwrap();
    for args in [
        vec!["matrix", "--format", "ado"],
        vec!["matrix", "--format", "ado", "--max-jobs", "1"],
        vec!["build", "consumer", "consumer_stable", "--plan"],
    ] {
        tailor()
            .current_dir(ws.path())
            .args(args)
            .assert()
            .code(2)
            .stdout(predicate::str::diff(""))
            .stderr(predicate::str::contains("collid"));
    }
}

#[test]
fn invalid_work_orders_report_their_source_and_fail_as_usage_errors() {
    let ws = order_only_workspace();
    for order in [
        "schemaVersion: 1\nimages: [consumer, typo]\n",
        "schemaVersion: 1\nimages: [consumer]\nselect: {channel: [stable, typo]}\n",
        "schemaVersion: 1\ncells: []\n",
        "schemaVersion: 1\nimages: null\n",
    ] {
        fs::write(ws.path().join("invalid.yaml"), order).unwrap();
        tailor()
            .current_dir(ws.path())
            .args(["build", "-w", "invalid.yaml", "--plan"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("invalid.yaml"));
    }
    for value in [None, Some(" ")] {
        let mut command = tailor();
        command
            .current_dir(ws.path())
            .env_remove("MISSING_WORK_ORDER");
        if let Some(value) = value {
            command.env("MISSING_WORK_ORDER", value);
        }
        command
            .args(["build", "--work-order-env", "MISSING_WORK_ORDER", "--plan"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("MISSING_WORK_ORDER"));
    }
}

#[test]
fn ado_job_cap_packs_whole_components_and_round_trips_without_duplicate_cells() {
    let ws = order_only_workspace();
    fs::create_dir(ws.path().join("independent")).unwrap();
    fs::write(
        ws.path().join("independent/image.yaml"),
        "name: independent\nbase: {path: ./base.img}\nconfig: {}\n",
    )
    .unwrap();
    let mut uncapped_cells = BTreeSet::new();
    for (extra, expected_jobs) in [
        (vec![], 2),
        (vec!["--max-jobs", "1"], 1),
        (vec!["--max-jobs", "99"], 2),
    ] {
        let output = tailor()
            .current_dir(ws.path())
            .args(["matrix", "--format", "ado"])
            .args(&extra)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let matrix: JsonValue = serde_json::from_slice(&output).unwrap();
        assert_eq!(matrix.as_object().unwrap().len(), expected_jobs);
        let mut scheduled = BTreeSet::new();
        for variables in matrix.as_object().unwrap().values() {
            assert_eq!(variables.as_object().unwrap().len(), 1);
            let order = variables["TAILOR_WORK_ORDER"].as_str().unwrap();
            let plan = work_order_plan_in(ws.path(), order);
            for cell in plan["cells"].as_array().unwrap() {
                let slug = cell["slug"].as_str().unwrap().to_owned();
                assert!(scheduled.insert(slug.clone()), "duplicated {slug}");
            }
        }
        if extra.is_empty() {
            uncapped_cells = scheduled;
        } else {
            assert_eq!(scheduled, uncapped_cells);
        }
    }
    assert!(uncapped_cells.contains("foundation_arm64_cosi"));
    tailor()
        .current_dir(ws.path())
        .args(["matrix", "--max-jobs", "1"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--max-jobs requires"));
}

#[test]
fn depends_on_builds_transitive_prerequisites_before_a_selected_consumer_once() {
    let ws = order_only_workspace();
    for args in [
        vec!["consumer", "-s", "channel=stable"],
        vec!["consumer_stable_amd64_cosi"],
    ] {
        let output = dry_run_in(ws.path(), &args);
        assert!(output.contains("4 cell(s) (dry-run)"), "{output}");
        assert_eq!(output.matches("docker run").count(), 4, "{output}");
        let foundation = output.find("foundation_amd64_cosi.cosi").unwrap();
        let arm = output.find("foundation_arm64_cosi.cosi").unwrap();
        let prepare = output.find("prepare_amd64_cosi.cosi").unwrap();
        let consumer = output.find("consumer_stable_amd64_cosi.cosi").unwrap();
        assert!(
            foundation < prepare && arm < prepare && prepare < consumer,
            "{output}"
        );
        assert!(!output.contains("consumer_edge_amd64_cosi"), "{output}");
    }
}

#[test]
fn depends_on_keeps_a_prerequisites_signing_profile() {
    let ws = order_only_workspace();
    let manifest = ws.path().join("tailor.yaml");
    let original = fs::read_to_string(&manifest).unwrap();
    fs::write(
        manifest,
        format!(
            "{original}previewFeatures: [signing]\nsigning:\n  default: test-ca\n  profiles:\n    test-ca:\n      backend: local-test-ca\n"
        ),
    )
    .unwrap();
    let image = ws.path().join("prepare/image.yaml");
    let original = fs::read_to_string(&image).unwrap();
    fs::write(image, format!("{original}signing: true\n")).unwrap();

    let output = dry_run_in(ws.path(), &["consumer_stable_amd64_cosi"]);
    assert!(
        output.contains("# prepare_amd64_cosi — signed 3-pass"),
        "{output}"
    );
    assert_eq!(output.matches("— signed 3-pass").count(), 1, "{output}");
}

#[test]
fn depends_on_also_builds_prerequisites_of_an_input_producer() {
    let ws = order_only_workspace();
    let image = ws.path().join("consumer/image.yaml");
    let original = fs::read_to_string(&image).unwrap();
    fs::write(
        image,
        original.replace(
            "dependsOn: [prepare, foundation]",
            "inputs:\n  - name: payload\n    image: prepare",
        ),
    )
    .unwrap();

    let output = dry_run_in(ws.path(), &["consumer_stable_amd64_cosi"]);
    assert!(output.contains("4 cell(s) (dry-run)"), "{output}");
    let foundation = output.find("foundation_arm64_cosi.cosi").unwrap();
    let prepare = output.find("prepare_amd64_cosi.cosi").unwrap();
    let consumer = output.find("consumer_stable_amd64_cosi.cosi").unwrap();
    assert!(foundation < prepare && prepare < consumer, "{output}");
}

// ───────────────────────────── standalone: built-in default toolchain ─────────────────────────────
#[test]
fn standalone_image_builds_without_a_manifest() {
    in_dir("standalone")
        .arg("validate")
        .assert()
        .success()
        .stdout(predicate::str::contains("1 cell(s) valid"));
}

#[test]
fn no_arch_axis_defaults_to_a_single_amd64_cell() {
    // No `arch` axis and no workspace default ⇒ exactly one amd64 cell.
    in_dir("standalone")
        .args(["matrix", "--format", "slugs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("solo_amd64_cosi"));
}

#[test]
fn removed_per_image_architectures_field_is_rejected() {
    in_dir("legacy-architectures")
        .arg("validate")
        .assert()
        .failure()
        .stderr(predicate::str::contains("architectures"));
}

#[test]
fn removed_workspace_defaults_architectures_field_is_rejected() {
    in_dir("legacy-defaults-arch")
        .arg("validate")
        .assert()
        .failure()
        .stderr(predicate::str::contains("architectures"));
}

#[test]
fn oci_platform_mismatch_fails_validate() {
    in_dir("oci-platform-mismatch")
        .arg("validate")
        .assert()
        .failure()
        .stderr(predicate::str::contains("arm64").and(predicate::str::contains("amd64")));
}

#[test]
fn standalone_uses_the_built_in_default_image_customizer() {
    // No `toolchain:` and no `tailor.yaml` ⇒ tailor's built-in default IC image at the `latest` tag.
    in_dir("standalone")
        .args(["build", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "mcr.microsoft.com/azurelinux/imagecustomizer:latest",
        ));
}

// ───────────────────────────── matrix: expansion, slugs, dimensions ───────────────────────────────

#[test]
fn matrix_validates_every_expanded_cell() {
    // edition[2] × arch[2] × channel[2] = 8 cells.
    in_dir("matrix")
        .arg("validate")
        .assert()
        .success()
        .stdout(predicate::str::contains("8 cell(s) valid"));
}

#[test]
fn matrix_emits_json_for_every_cell() {
    in_dir("matrix").arg("matrix").assert().success().stdout(
        predicate::str::contains("gizmo_lite_amd64_stable_cosi")
            .and(predicate::str::contains("\"format\": \"raw\"")),
    );
}

#[test]
fn matrix_format_slugs_prints_one_bare_slug_per_line() {
    let assert = in_dir("matrix")
        .args(["matrix", "--format", "slugs"])
        .assert()
        .success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 8, "one slug per cell");
    assert!(lines.contains(&"gizmo_pro_arm64_edge_raw"));
    assert!(!out.contains('{'), "slugs format is not JSON");
}

#[test]
fn slugs_subcommand_matches_matrix_format_slugs() {
    let from_matrix = in_dir("matrix")
        .args(["matrix", "--format", "slugs"])
        .assert()
        .success();
    let from_slugs = in_dir("matrix").arg("slugs").assert().success();
    assert_eq!(
        from_slugs.get_output().stdout,
        from_matrix.get_output().stdout,
        "`slugs` must match `matrix --format slugs`"
    );
}

#[test]
fn matrix_ado_emits_one_setvariable_line_with_flat_scalar_legs() {
    let assert = in_dir("matrix")
        .args([
            "matrix",
            "--ado",
            "BUILD_MATRIX",
            "-s",
            "edition=lite,arch=amd64,channel=stable",
        ])
        .assert()
        .success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 1, "exactly one stdout line for the agent");
    assert!(out.starts_with("##vso[task.setvariable variable=BUILD_MATRIX;isOutput=true]{"));
    let matrix: JsonValue = serde_json::from_str(out.split_once(']').unwrap().1.trim()).unwrap();
    assert_eq!(matrix.as_object().unwrap().len(), 1);
    let variables = matrix.as_object().unwrap().values().next().unwrap();
    assert_eq!(variables.as_object().unwrap().len(), 1);
    let plan = work_order_plan_in(
        &fixtures().join("matrix"),
        variables["TAILOR_WORK_ORDER"].as_str().unwrap(),
    );
    assert_eq!(plan["requested"][0], "gizmo_lite_amd64_stable_cosi");
}

#[test]
fn matrix_format_ado_prints_the_bare_object_without_wrapper() {
    in_dir("matrix")
        .args([
            "matrix",
            "--format",
            "ado",
            "-s",
            "edition=lite,arch=amd64,channel=stable",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::starts_with("{")
                .and(predicate::str::contains("TAILOR_WORK_ORDER"))
                .and(predicate::str::contains("##vso").not()),
        );
}

#[test]
fn matrix_format_ado_of_empty_selection_fails() {
    in_dir("matrix")
        .args(["matrix", "--format", "ado", "-s", "edition=enterprise"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no cells"));
}

#[test]
fn matrix_ado_of_empty_selection_fails() {
    in_dir("matrix")
        .args([
            "matrix",
            "--ado",
            "BUILD_MATRIX",
            "-s",
            "edition=enterprise",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no cells"));
}

#[test]
fn matrix_ado_rejects_invalid_variable_name() {
    in_dir("matrix")
        .args(["matrix", "--ado", "1bad", "-s", "edition=lite"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid --ado variable name"));
}

// ───────────────────────────── base-image catalogue (baseImages) ───────────────────────────────

#[test]
fn matrix_json_exposes_base_image_for_a_catalogue_slot() {
    in_dir("catalogue")
        .arg("matrix")
        .assert()
        .success()
        .stdout(predicate::str::contains("\"baseImage\": \"baremetal\""));
}

#[test]
fn matrix_ado_orders_catalogue_cells_instead_of_carrying_per_cell_metadata() {
    in_dir("catalogue")
        .args(["matrix", "--format", "ado"])
        .assert()
        .success()
        .stdout(predicate::str::contains("TAILOR_WORK_ORDER"));
}

#[test]
fn bases_verify_fails_when_a_referenced_slot_file_is_missing() {
    // The `host` image references `baremetal`, whose file is absent in the fixture → verify fails
    // with a hint to download.
    in_dir("catalogue")
        .args(["bases", "verify"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("baremetal").and(predicate::str::contains("download")));
}

#[test]
fn show_lists_the_dimensions_and_their_values() {
    in_dir("matrix")
        .args(["show", "gizmo"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("8 cell(s) across 3 axis(es)")
                .and(predicate::str::contains("edition"))
                .and(predicate::str::contains("lite, pro"))
                .and(predicate::str::contains("channel"))
                .and(predicate::str::contains("stable, edge")),
        );
}

// ───────────────────────────── matrix: dry-run rendering & the docker prelude ─────────────────────

#[test]
fn build_dry_run_prints_the_docker_prelude_offline() {
    in_dir("matrix")
        .args(["build", "--dry-run"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("(dry-run)")
                .and(predicate::str::contains("docker run \\"))
                .and(predicate::str::contains("--privileged"))
                .and(predicate::str::contains(":ro"))
                .and(predicate::str::contains("-v /:/host").not()),
        );
}

#[test]
fn dry_run_replace_directive_changes_the_output_format() {
    // `pro` $replaces the inherited cosi output with raw.
    in_dir("matrix")
        .args(["build", "--dry-run", "--cell", "gizmo_pro_amd64_stable_raw"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("1 cell(s) (dry-run)")
                .and(predicate::str::contains("--output-image-format raw")),
        );
}

#[test]
fn dry_run_set_directive_overrides_the_base_with_an_oci_reference() {
    // `edge` $sets the base to an OCI reference with `linux/${arch}` interpolated and adds an
    // rpm-source; `stable` keeps the by-arch local path base.
    in_dir("matrix")
        .args(["build", "--dry-run", "--cell", "gizmo_lite_arm64_edge_cosi"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("--image oci:registry.example/gizmo/base:edge")
                .and(predicate::str::contains("--rpm-source"))
                .and(predicate::str::contains("repos/edge.repo")),
        );
    in_dir("matrix")
        .args([
            "build",
            "--dry-run",
            "--cell",
            "gizmo_lite_amd64_stable_cosi",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("--image-file")
                .and(predicate::str::contains("bases/gizmo-amd64.img")),
        );
}

// ───────────────────────────── matrix: rendered config (merge + interpolation) ────────────────────

#[test]
fn explain_with_config_renders_nested_interpolation_and_removed_packages() {
    // `edge` derives bootPkg = "boot-edge-${efiArch}"; on amd64 ${efiArch}=x64 → boot-edge-x64
    // (nested interpolation). `pro` $removes the `base-extra` baseline package.
    in_dir("matrix")
        .args([
            "explain",
            "gizmo",
            "--with-config",
            "-s",
            "edition=pro,arch=amd64,channel=edge",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("boot-edge-x64")
                .and(predicate::str::contains("gizmo-core"))
                .and(predicate::str::contains("base-extra").not())
                .and(predicate::str::contains("uki")), // previewFeatures passthrough
        );
}

#[test]
fn explain_with_config_resolves_includes_and_appends_lists() {
    // `pro` splices layouts/storage/pro.yaml via $include (adds a `data` partition) and appends
    // `audit=1` to the shared kernel command line; the `stable` channel pins boot-stable.
    in_dir("matrix")
        .args([
            "explain",
            "gizmo",
            "--with-config",
            "-s",
            "edition=pro,arch=amd64,channel=stable",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("/var/lib/data") // from the $included pro storage layout
                .and(predicate::str::contains("audit=1")) // appended kernel arg
                .and(predicate::str::contains("boot-stable")), // stable channel param
        );
}

#[test]
fn explain_prints_the_merge_order_with_reasons() {
    // The default `explain` (no --with-config) lists the ordered fragment files and why each applies.
    in_dir("matrix")
        .args([
            "explain",
            "gizmo",
            "-s",
            "edition=pro,arch=amd64,channel=stable",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("merge order")
                .and(predicate::str::contains("image.yaml"))
                .and(predicate::str::contains("by-edition/pro.yaml"))
                .and(predicate::str::contains("edition=pro"))
                .and(predicate::str::contains("$include")), // pro splices a storage layout
        );
}

#[test]
fn explain_accepts_a_cell_slug_as_a_positional() {
    in_dir("matrix")
        .args(["explain", "gizmo_pro_arm64_edge_raw"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("cell  gizmo_pro_arm64_edge_raw")
                .and(predicate::str::contains("by-edition+arch/pro+arm64.yaml"))
                .and(predicate::str::contains("cell  gizmo_pro_arm64_edge_cosi").not()),
        );
}

#[test]
fn explain_slug_preserves_select_and_with_config() {
    in_dir("matrix")
        .args([
            "explain",
            "gizmo_pro_arm64_edge_raw",
            "--with-config",
            "-s",
            "arch=arm64",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("cell  gizmo_pro_arm64_edge_raw")
                .and(predicate::str::contains("cell  gizmo_pro_arm64_edge_cosi").not())
                .and(predicate::str::contains("composite-only-pkg")),
        );
}

#[test]
fn explain_rejects_an_unknown_positional() {
    in_dir("matrix")
        .args(["explain", "not-a-real-slug"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no matching image or cell slug: `not-a-real-slug`",
        ));
}

#[test]
fn composite_fragment_applies_only_to_its_axis_pair() {
    // by-edition+arch/pro+arm64.yaml adds `composite-only-pkg` to the (pro, arm64) cells only.
    in_dir("matrix")
        .args([
            "explain",
            "gizmo",
            "--with-config",
            "-s",
            "edition=pro,arch=arm64,channel=stable",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("by-edition+arch/pro+arm64.yaml")
                .and(predicate::str::contains("edition=pro ∧ arch=arm64"))
                .and(predicate::str::contains("composite-only-pkg")),
        );
    // The sibling amd64 cell does not get the composite delta.
    in_dir("matrix")
        .args([
            "explain",
            "gizmo",
            "--with-config",
            "-s",
            "edition=pro,arch=amd64,channel=stable",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("composite-only-pkg").not());
}

// ───────────────────────────── selection: slices, single cells, validation ───────────────────────

#[test]
fn select_pins_a_single_cell() {
    in_dir("matrix")
        .args([
            "build",
            "--dry-run",
            "-s",
            "edition=lite,arch=amd64,channel=stable",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("1 cell(s) (dry-run)")
                .and(predicate::str::contains("gizmo_lite_amd64_stable_cosi")),
        );
}

#[test]
fn select_slice_along_one_axis() {
    // `-s arch=amd64` keeps every amd64 cell: edition[2] × channel[2] = 4.
    in_dir("matrix")
        .args(["validate", "-s", "arch=amd64"])
        .assert()
        .success()
        .stdout(predicate::str::contains("4 cell(s) valid"));
}

#[test]
fn cell_flag_selects_an_exact_slug() {
    in_dir("matrix")
        .args(["validate", "--cell", "gizmo_pro_arm64_edge_raw"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 cell(s) valid"));
}

#[test]
fn unknown_select_axis_is_rejected() {
    in_dir("matrix")
        .args(["validate", "-s", "distro=fedora"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("distro"));
}

#[test]
fn empty_selection_is_rejected() {
    // A syntactically valid axis/value that matches no cell is a hard error (catches CI typos).
    in_dir("matrix")
        .args(["validate", "-s", "edition=enterprise"])
        .assert()
        .failure();
}

// ───────────────────────────── version ────────────────────────────────────────────────────────────

#[test]
fn notice_prints_own_license_and_third_party_notices() {
    let assert = tailor().arg("notice").assert().success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout);
    // tailor's own MIT license.
    assert!(out.contains("MIT License"), "missing tailor license");
    assert!(
        out.contains("Copyright (c) 2026 tailor contributors"),
        "missing tailor copyright"
    );
    // The third-party section with real dependency license text (e.g. serde, tokio, clap).
    assert!(
        out.contains("THIRD-PARTY SOFTWARE NOTICES"),
        "missing third-party section"
    );
    for crate_name in ["serde", "tokio", "clap"] {
        assert!(
            out.contains(&format!("\n{crate_name} ")),
            "expected a notice entry for `{crate_name}`"
        );
    }
    // Full permission text must be reproduced, not just SPDX identifiers.
    assert!(
        out.contains("Permission is hereby granted"),
        "expected reproduced license text"
    );
}

#[test]
fn version_subcommand_matches_flag() {
    let from_flag = tailor().arg("--version").assert().success();
    let from_subcommand = tailor().arg("version").assert().success();
    assert_eq!(
        from_subcommand.get_output().stdout,
        from_flag.get_output().stdout,
        "`version` subcommand must match `--version`"
    );
}

#[test]
fn version_carries_cargo_version_and_build_metadata() {
    let assert = tailor().arg("version").assert().success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(out.starts_with("tailor "), "got {out:?}");
    assert!(out.contains(env!("CARGO_PKG_VERSION")), "got {out:?}");
    // SemVer build metadata is appended after a `+`.
    assert!(
        out.contains('+'),
        "expected SemVer build metadata in {out:?}"
    );
}
