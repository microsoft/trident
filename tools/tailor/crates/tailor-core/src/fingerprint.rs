//! The canonical per-cell fingerprint — a SHA-256 over every build-affecting input (`meta/docs/2026-06-22-design.md`
//! §9.1). Deterministic given a deterministic render, so it is stable across machines and runs.

use serde_yaml_ng::Value;
use sha2::{Digest, Sha256};
use tailor_config::{Compression, ExtraParam, Operation};

use crate::{domain::Fingerprint, ports::ResolvedBase};

/// Version of tailor's own build semantics. Bump when a tailor change alters the *bytes* of a
/// produced artifact for unchanged inputs (e.g. a change to how the IC invocation is assembled or
/// how outputs are post-processed) so that already-stamped artifacts are correctly rebuilt after an
/// upgrade. Patch/feature releases that do not change build semantics leave this untouched, so they
/// do not force a workspace-wide rebuild.
///
/// The IC *engine* version deliberately is **not** encoded here: it is already captured with full
/// precision by [`FingerprintInputs::toolchain_digest`], which is the digest-pinned IC image
/// reference (a different IC build is a different image digest, hence a different fingerprint).
pub const BUILD_SCHEMA_VERSION: u32 = 1;

/// All inputs that determine a cell's output. Registry digests come from resolution/the lock; local
/// hashes are computed at build time.
pub struct FingerprintInputs<'a> {
    pub slug: &'a str,
    pub toolchain_digest: &'a str,
    pub base: &'a ResolvedBase,
    pub ic_config: &'a Value,
    pub operation: Operation,
    pub tools_dir_digest: Option<&'a str>,
    /// Sorted per-file hashes of `extraDependencies` files (XXH3-128 — see `deps.rs`).
    pub extra_dependency_hashes: &'a [[u8; 16]],
    /// Sorted per-file hashes of `rpmSources` contents (excluding `repodata/`; XXH3-128).
    pub rpm_source_hashes: &'a [[u8; 16]],
    /// Sorted per-file hashes of the resolved `${inputs.*}` producer artifacts (XXH3-128).
    pub input_dep_hashes: &'a [[u8; 16]],
    /// Extra IC command-line flags, in declared (merge) order — a change to any flag rebuilds.
    pub extra_params: &'a [ExtraParam],
    /// Post-build artifact compression; a change rebuilds (the published artifact differs).
    pub compression: Option<Compression>,
    /// COSI compression level (`--cosi-compression-level`); a change rebuilds the COSI.
    pub cosi_compression_level: Option<u8>,
}

/// Compute the canonical fingerprint. Each field is domain-separated and length-prefixed so distinct
/// inputs can never collide by concatenation.
pub fn fingerprint(inputs: &FingerprintInputs<'_>) -> Fingerprint {
    let mut hasher = Sha256::new();

    field(
        &mut hasher,
        b"build-schema",
        &BUILD_SCHEMA_VERSION.to_le_bytes(),
    );
    field(&mut hasher, b"slug", inputs.slug.as_bytes());
    field(
        &mut hasher,
        b"toolchain",
        inputs.toolchain_digest.as_bytes(),
    );
    match inputs.base {
        ResolvedBase::LocalFile { content_hash, size } => {
            field(&mut hasher, b"base.local", content_hash);
            field(&mut hasher, b"base.size", &size.to_le_bytes());
        }
        ResolvedBase::Oci {
            reference,
            platform,
            digest,
        } => {
            field(&mut hasher, b"base.oci.ref", reference.as_bytes());
            field(&mut hasher, b"base.oci.platform", platform.as_bytes());
            field(&mut hasher, b"base.oci.digest", digest.as_bytes());
        }
    }
    field(&mut hasher, b"config", &canonical_config(inputs.ic_config));
    field(&mut hasher, b"operation", operation_tag(inputs.operation));
    if let Some(digest) = inputs.tools_dir_digest {
        field(&mut hasher, b"tools-dir.digest", digest.as_bytes());
    }
    for hash in inputs.extra_dependency_hashes {
        field(&mut hasher, b"dep", hash);
    }
    for hash in inputs.rpm_source_hashes {
        field(&mut hasher, b"rpm", hash);
    }
    for hash in inputs.input_dep_hashes {
        field(&mut hasher, b"input", hash);
    }
    for extra in inputs.extra_params {
        field(&mut hasher, b"extra-param.param", extra.param.as_bytes());
        field(
            &mut hasher,
            b"extra-param.value",
            extra.value.as_deref().unwrap_or_default().as_bytes(),
        );
    }
    if let Some(compression) = inputs.compression {
        field(&mut hasher, b"compression", compression.as_str().as_bytes());
    }
    if let Some(level) = inputs.cosi_compression_level {
        field(&mut hasher, b"cosi-compression-level", &[level]);
    }

    Fingerprint(hasher.finalize().into())
}

/// A deterministic byte form of the merged config (the rendered config is already deterministic).
pub fn canonical_config(config: &Value) -> Vec<u8> {
    serde_yaml_ng::to_string(config)
        .unwrap_or_default()
        .into_bytes()
}

fn operation_tag(operation: Operation) -> &'static [u8] {
    match operation {
        Operation::Customize => b"customize",
        Operation::Convert => b"convert",
    }
}

fn field(hasher: &mut Sha256, label: &[u8], bytes: &[u8]) {
    hasher.update((label.len() as u64).to_le_bytes());
    hasher.update(label);
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> ResolvedBase {
        ResolvedBase::LocalFile {
            content_hash: [1; 16],
            size: 100,
        }
    }

    fn inputs<'a>(
        slug: &'a str,
        config: &'a Value,
        base: &'a ResolvedBase,
    ) -> FingerprintInputs<'a> {
        FingerprintInputs {
            slug,
            toolchain_digest: "sha256:abc",
            base,
            ic_config: config,
            operation: Operation::Customize,
            tools_dir_digest: None,
            extra_dependency_hashes: &[],
            rpm_source_hashes: &[],
            input_dep_hashes: &[],
            extra_params: &[],
            compression: None,
            cosi_compression_level: None,
        }
    }

    #[test]
    fn fingerprint_is_deterministic() {
        let base = base();
        let cfg: Value = serde_yaml_ng::from_str("os:\n  hostname: a\n").unwrap();
        let a = fingerprint(&inputs("cell", &cfg, &base));
        let b = fingerprint(&inputs("cell", &cfg, &base));
        assert_eq!(a, b);
    }

    #[test]
    fn config_change_changes_fingerprint() {
        let base = base();
        let cfg_a: Value = serde_yaml_ng::from_str("os:\n  hostname: a\n").unwrap();
        let cfg_b: Value = serde_yaml_ng::from_str("os:\n  hostname: b\n").unwrap();
        assert_ne!(
            fingerprint(&inputs("cell", &cfg_a, &base)),
            fingerprint(&inputs("cell", &cfg_b, &base))
        );
    }

    #[test]
    fn toolchain_digest_change_changes_fingerprint() {
        // The IC engine version is captured via the digest-pinned toolchain image reference, so a
        // new IC build (a different digest) invalidates the incremental fingerprint.
        let base = base();
        let cfg: Value = serde_yaml_ng::from_str("os:\n  hostname: a\n").unwrap();
        let mut a = inputs("cell", &cfg, &base);
        a.toolchain_digest = "ic@sha256:one";
        let mut b = inputs("cell", &cfg, &base);
        b.toolchain_digest = "ic@sha256:two";
        assert_ne!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn tools_dir_digest_changes_fingerprint() {
        let base = base();
        let cfg: Value = serde_yaml_ng::from_str("os:\n  hostname: a\n").unwrap();
        let mut a = inputs("cell", &cfg, &base);
        a.tools_dir_digest = Some("sha256:one");
        let mut b = inputs("cell", &cfg, &base);
        b.tools_dir_digest = Some("sha256:two");
        assert_ne!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn extra_params_change_fingerprint() {
        let base = base();
        let cfg: Value = serde_yaml_ng::from_str("os:\n  hostname: a\n").unwrap();
        let one = [ExtraParam {
            param: "--experimental".to_owned(),
            value: Some("a".to_owned()),
        }];
        let two = [ExtraParam {
            param: "--experimental".to_owned(),
            value: Some("b".to_owned()),
        }];
        let mut a = inputs("cell", &cfg, &base);
        a.extra_params = &one;
        let mut b = inputs("cell", &cfg, &base);
        b.extra_params = &two;
        assert_ne!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn compression_and_cosi_level_change_fingerprint() {
        let base = base();
        let cfg: Value = serde_yaml_ng::from_str("os:\n  hostname: a\n").unwrap();
        let plain = inputs("cell", &cfg, &base);
        let mut zstd = inputs("cell", &cfg, &base);
        zstd.compression = Some(Compression::Zstd);
        assert_ne!(fingerprint(&plain), fingerprint(&zstd));

        let mut level_one = inputs("cell", &cfg, &base);
        level_one.cosi_compression_level = Some(1);
        let mut level_two = inputs("cell", &cfg, &base);
        level_two.cosi_compression_level = Some(2);
        assert_ne!(fingerprint(&level_one), fingerprint(&level_two));
    }

    #[test]
    fn input_dep_hashes_change_the_fingerprint() {
        let base = base();
        let cfg: Value = serde_yaml_ng::from_str("os:\n  hostname: a\n").unwrap();
        let mut a = inputs("cell", &cfg, &base);
        let one = [[9u8; 16]];
        a.input_dep_hashes = &one;
        let b = inputs("cell", &cfg, &base);
        assert_ne!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn slug_is_part_of_the_fingerprint() {
        let base = base();
        let cfg: Value = serde_yaml_ng::from_str("os:\n  hostname: a\n").unwrap();
        assert_ne!(
            fingerprint(&inputs("cell-a", &cfg, &base)),
            fingerprint(&inputs("cell-b", &cfg, &base))
        );
    }
}
