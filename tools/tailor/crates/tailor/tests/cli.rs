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

use std::path::PathBuf;

use assert_cmd::Command;
use predicates::prelude::*;

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
    // Wrapper, leg key (sanitised slug), reserved fields, axes as `axis_*`, raw slug verbatim.
    assert!(out.starts_with("##vso[task.setvariable variable=BUILD_MATRIX;isOutput=true]{"));
    assert!(out.contains("\"gizmo_lite_amd64_stable_cosi\":{"));
    assert!(out.contains("\"slug\":\"gizmo_lite_amd64_stable_cosi\""));
    assert!(out.contains("\"axis_edition\":\"lite\""));
    assert!(
        !out.contains("\"axes\""),
        "values are flat — no nested axes object"
    );
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
            predicate::str::starts_with("{\"gizmo_lite_amd64_stable_cosi\"")
                .and(predicate::str::contains("##vso").not()),
        );
}

#[test]
fn matrix_format_ado_of_empty_selection_prints_empty_object() {
    in_dir("matrix")
        .args(["matrix", "--format", "ado", "-s", "edition=enterprise"])
        .assert()
        .success()
        .stdout(predicate::str::diff("{}\n"));
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
fn matrix_ado_carries_base_image_as_a_reserved_scalar() {
    in_dir("catalogue")
        .args(["matrix", "--format", "ado"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"baseImage\":\"baremetal\""));
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
