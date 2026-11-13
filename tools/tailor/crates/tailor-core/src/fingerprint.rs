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
    /// Effective signing profile's id (`meta/docs/2026-06-29-signing.md` §4); a change -- including
    /// unsigned-to-signed or a profile swap -- rebuilds the cell. The reference fields below are
    /// hashed separately (each under its own domain-separated tag, not concatenated into this or any
    /// other single string) so that, say, a `:`-containing `certificate` value can never be
    /// misattributed to `publish_ca_cert` or vice versa.
    pub signing_profile_id: Option<&'a str>,
    /// Effective signing profile's `backend`; a change (e.g. `keypair` -> `azure-key-vault` under the
    /// same id) rebuilds the cell.
    pub signing_backend: Option<&'a str>,
    /// Effective signing profile's `vault` (azure-key-vault backend); a change rebuilds the cell.
    pub signing_vault: Option<&'a str>,
    /// Effective signing profile's `certificate` (azure-key-vault backend); a change rebuilds the
    /// cell.
    pub signing_certificate: Option<&'a str>,
    /// Effective signing profile's `publishCaCert` path (local-test-ca backend); a change -- e.g.
    /// redirecting where the enrollable CA certificate is written -- rebuilds the cell.
    pub signing_publish_ca_cert: Option<&'a str>,
    /// Content hash of the profile's local key file (XXH3-128, `deps.rs`, `keypair` backend only); a
    /// change to the file *contents* alone (same declared path) still busts the fingerprint, unlike
    /// the id or path strings above which only change when the config text itself changes. Kept
    /// separate from `signing_cert_hash` (rather than merged into one sorted set) so swapping the
    /// key and cert files changes the fingerprint instead of silently reusing a stale artifact.
    pub signing_key_hash: Option<[u8; 16]>,
    /// Content hash of the profile's local certificate file (XXH3-128, `deps.rs`, `keypair` backend
    /// only); see `signing_key_hash`.
    pub signing_cert_hash: Option<[u8; 16]>,
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
    if let Some(id) = inputs.signing_profile_id {
        field(&mut hasher, b"signing.profile.id", id.as_bytes());
    }
    if let Some(backend) = inputs.signing_backend {
        field(&mut hasher, b"signing.profile.backend", backend.as_bytes());
    }
    if let Some(vault) = inputs.signing_vault {
        field(&mut hasher, b"signing.profile.vault", vault.as_bytes());
    }
    if let Some(certificate) = inputs.signing_certificate {
        field(
            &mut hasher,
            b"signing.profile.certificate",
            certificate.as_bytes(),
        );
    }
    if let Some(publish_ca_cert) = inputs.signing_publish_ca_cert {
        field(
            &mut hasher,
            b"signing.profile.publish-ca-cert",
            publish_ca_cert.as_bytes(),
        );
    }
    if let Some(hash) = inputs.signing_key_hash {
        field(&mut hasher, b"signing.key", &hash);
    }
    if let Some(hash) = inputs.signing_cert_hash {
        field(&mut hasher, b"signing.cert", &hash);
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
            signing_profile_id: None,
            signing_backend: None,
            signing_vault: None,
            signing_certificate: None,
            signing_publish_ca_cert: None,
            signing_key_hash: None,
            signing_cert_hash: None,
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
    fn fingerprint_changes_when_signing_profile_changes() {
        let base = base();
        let cfg: Value = serde_yaml_ng::from_str("os:\n  hostname: a\n").unwrap();
        let mut unsigned = inputs("cell", &cfg, &base);
        let mut signed = inputs("cell", &cfg, &base);
        signed.signing_profile_id = Some("release");
        assert_ne!(fingerprint(&unsigned), fingerprint(&signed));

        unsigned.signing_profile_id = Some("release");
        let mut other_profile = inputs("cell", &cfg, &base);
        other_profile.signing_profile_id = Some("dev");
        assert_ne!(fingerprint(&unsigned), fingerprint(&other_profile));
    }

    #[test]
    fn fingerprint_changes_when_signing_profile_fields_are_reassigned_across_a_delimiter() {
        // Regression: concatenating profile fields with a plain `:` was not collision-free --
        // `certificate: "ignored"` + `publish_ca_cert: "a:b"` produced the same joined string as
        // `certificate: "ignored:a"` + `publish_ca_cert: "b"`. Each field must be hashed under its
        // own domain-separated tag instead.
        let base = base();
        let cfg: Value = serde_yaml_ng::from_str("os:\n  hostname: a\n").unwrap();
        let mut a = inputs("cell", &cfg, &base);
        a.signing_certificate = Some("ignored");
        a.signing_publish_ca_cert = Some("a:b");
        let mut b = inputs("cell", &cfg, &base);
        b.signing_certificate = Some("ignored:a");
        b.signing_publish_ca_cert = Some("b");
        assert_ne!(
            fingerprint(&a),
            fingerprint(&b),
            "reassigning bytes across the certificate/publish_ca_cert boundary must change the fingerprint"
        );
    }

    #[test]
    fn fingerprint_changes_when_signing_key_hash_changes() {
        // A credential edit under the *same* profile id (e.g. rotating the keypair's PEM files in
        // place) must still bust the fingerprint -- the id string alone doesn't change.
        let base = base();
        let cfg: Value = serde_yaml_ng::from_str("os:\n  hostname: a\n").unwrap();
        let mut a = inputs("cell", &cfg, &base);
        a.signing_profile_id = Some("release:keypair::");
        a.signing_key_hash = Some([1; 16]);
        a.signing_cert_hash = Some([2; 16]);
        let mut b = inputs("cell", &cfg, &base);
        b.signing_profile_id = Some("release:keypair::");
        b.signing_key_hash = Some([1; 16]);
        b.signing_cert_hash = Some([9; 16]);
        assert_ne!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn fingerprint_changes_when_key_and_cert_hashes_are_swapped() {
        // Swapping which file is the key and which is the cert must not be a no-op: merging both
        // into one sorted hash set would make this pair indistinguishable from an unswapped one.
        let base = base();
        let cfg: Value = serde_yaml_ng::from_str("os:\n  hostname: a\n").unwrap();
        let mut a = inputs("cell", &cfg, &base);
        a.signing_profile_id = Some("release:keypair::");
        a.signing_key_hash = Some([1; 16]);
        a.signing_cert_hash = Some([2; 16]);
        let mut swapped = inputs("cell", &cfg, &base);
        swapped.signing_profile_id = Some("release:keypair::");
        swapped.signing_key_hash = Some([2; 16]);
        swapped.signing_cert_hash = Some([1; 16]);
        assert_ne!(fingerprint(&a), fingerprint(&swapped));
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
