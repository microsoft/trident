//! End-to-end CLI integration tests for the signing foundation (`meta/docs/2026-06-29-signing.md`).
//!
//! These drive the real `tailor` binary against the synthetic `tests/fixtures/signing/` workspace.
//! They cover the `signing:` config surface, profile resolution, the fail-fast preflight, and the
//! signed dry-run rendering the three-pass. No Docker or network is involved: every signing path
//! resolves before the engine, and the dry-run renders the passes without running them.
//!
//! The `keys/db.{key,crt}` fixtures are PEM-armored stubs (header only), not real key material —
//! enough for the preflight's PEM-shape check.

use std::path::PathBuf;

use assert_cmd::Command;
use predicates::prelude::*;

fn signing_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/signing")
}

fn tailor() -> Command {
    let mut cmd = Command::cargo_bin("tailor").unwrap();
    cmd.current_dir(signing_dir());
    cmd
}

#[test]
fn validate_reports_a_ready_signing_profile_non_fatally() {
    tailor()
        .args(["validate", "ok-default"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("ok-default")
                .and(predicate::str::contains("signing profile `test-ca` ready")),
        );
}

#[test]
fn validate_reports_a_missing_prerequisite_without_failing() {
    // `missing-key` uses the `broken` keypair profile whose key path does not exist.
    tailor()
        .args(["validate", "missing-key"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("not ready")
                .and(predicate::str::contains("broken"))
                .and(predicate::str::contains("cannot read `key`")),
        );
}

#[test]
fn build_fails_fast_when_a_signing_key_is_missing() {
    // Fail-fast preflight (§5.1): aborts before any IC run, naming the unmet prerequisite.
    tailor()
        .args(["build", "missing-key"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("signing preflight failed")
                .and(predicate::str::contains("cannot read `key`"))
                .and(predicate::str::contains("missing-key")),
        );
}

#[test]
fn dry_run_byo_renders_the_signed_pass_after_preflight() {
    // `ok-byo` references the committed PEM stubs, so the keypair preflight passes; a dry-run then
    // renders the signed pass daemon-free (no real signing happens, so the stub keys are fine).
    tailor()
        .args(["build", "--dry-run", "ok-byo"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("signing profile `byo` ready")
                .and(predicate::str::contains("inject-files")),
        );
}

#[test]
fn dry_run_of_a_signed_image_is_daemon_free_and_renders_the_three_pass() {
    // Signing is implemented: a signed dry-run renders the real three-pass (customize → raw
    // intermediate, sign, inject-files) without contacting any engine.
    tailor()
        .args(["build", "--dry-run", "ok-default"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("(dry-run)")
                .and(predicate::str::contains("signing profile `test-ca` ready"))
                .and(predicate::str::contains("--output-image-format raw"))
                .and(predicate::str::contains("inject-files"))
                .and(predicate::str::contains("ca_cert.pem")),
        );
}

#[test]
fn an_unknown_signing_profile_is_a_clear_error() {
    tailor()
        .args(["validate", "bad-profile"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown signing profile `nope`"));
}

#[test]
fn an_unsigned_image_is_unaffected_by_signing() {
    // No `signing:` ⇒ no signing report, and the dry-run renders as usual.
    tailor()
        .args(["build", "--dry-run", "unsigned"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("(dry-run)")
                .and(predicate::str::contains("signing profile").not()),
        );
}
