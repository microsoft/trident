//! `tailor-sign` — host-side signing backends for the [`Signer`] port
//! (`meta/docs/2026-06-29-signing.md` §6).
//!
//! External **host** tools only, no bundled crypto: `openssl` mints the per-cell CA + code-signing
//! leaf and signs verity-hash artifacts (detached CMS/DER), and `sbsign` signs PE/Authenticode boot
//! artifacts (UKIs, shim, systemd-boot). The signer reads the `inject-files.yaml` IC emits from a
//! `customize` pass and signs each listed artifact **in place**, so IC's `inject-files` pass can
//! re-inject the now-signed files.

use std::{
    env, fs,
    io::Error as IoError,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
};

use serde::Deserialize;
use tempfile::TempDir;
use tracing::debug;

use tailor_config::{SigningBackend, SigningProfile};
use tailor_core::{MissingPrerequisite, SignError, Signer, SigningPlan, SigningResult};

/// The `openssl` binary — CA/leaf minting and verity-hash CMS signing.
const OPENSSL: &str = "openssl";
/// The `sbsign` binary — PE/Authenticode signing.
const SBSIGN: &str = "sbsign";
/// Validity of the minted local test CA and leaf. Long enough that CI never trips over expiry; this
/// is explicitly **not** a production trust root (`meta/docs/2026-06-29-signing.md` §6).
const CERT_VALIDITY_DAYS: &str = "3650";

/// Build the [`Signer`] for a resolved signing profile (`meta/docs/2026-06-29-signing.md` §6). Relative
/// `keypair` `key`/`cert` paths resolve against `base_dir` (the workspace root). The returned signer's
/// [`Signer::preflight`] reports any unmet prerequisite — including an unimplemented backend — so the
/// fail-fast gate needs no special-casing.
#[must_use]
pub fn build_signer(
    profile_id: &str,
    profile: &SigningProfile,
    base_dir: &Path,
) -> Arc<dyn Signer> {
    let key_source = match profile.backend {
        SigningBackend::LocalTestCa => KeySource::LocalTestCa {
            // `publishCaCert` (resolved against the workspace root) pins a fixed publish path; absent,
            // the executor's per-image `<output_dir>/<slug>.ca_cert.pem` default is used.
            publish_override: profile
                .publish_ca_cert
                .as_deref()
                .map(|p| resolve(base_dir, Some(p))),
        },
        SigningBackend::Keypair => KeySource::Keypair {
            key: resolve(base_dir, profile.key.as_deref()),
            cert: resolve(base_dir, profile.cert.as_deref()),
        },
        SigningBackend::AzureKeyVault => KeySource::Unsupported {
            backend: SigningBackend::AzureKeyVault.as_str(),
        },
    };
    Arc::new(HostSigner {
        profile_id: profile_id.to_owned(),
        key_source,
        ca: Mutex::new(None),
    })
}

fn resolve(base_dir: &Path, path: Option<&Path>) -> PathBuf {
    base_dir.join(path.unwrap_or(Path::new("")))
}

/// Where the signing key/cert come from (`meta/docs/2026-06-29-signing.md` §6).
#[derive(Debug, Clone)]
enum KeySource {
    /// Mint a self-signed CA (once per signer) + a code-signing leaf (per cell) with `openssl`.
    LocalTestCa { publish_override: Option<PathBuf> },
    /// A caller-supplied PEM key + certificate.
    Keypair { key: PathBuf, cert: PathBuf },
    /// A backend whose execution is not implemented yet (e.g. `azure-key-vault`).
    Unsupported { backend: &'static str },
}

/// The minted local test CA, kept alive for the signer's lifetime so its per-cell leaves chain to one
/// trust root (`meta/docs/2026-06-29-signing.md` §7). The temp dir wipes the CA private key on drop.
#[derive(Debug)]
struct Ca {
    _dir: TempDir,
    key: PathBuf,
    cert: PathBuf,
}

/// A host-tool signer (`openssl` + `sbsign`) parameterized by its [`KeySource`].
#[derive(Debug)]
struct HostSigner {
    profile_id: String,
    key_source: KeySource,
    /// The lazily-minted per-build CA (`local-test-ca` only); `None` until the first `sign`.
    ca: Mutex<Option<Ca>>,
}

impl Signer for HostSigner {
    fn preflight(&self) -> Result<(), SignError> {
        let mut missing = Vec::new();
        let mut note = |detail: String| {
            missing.push(MissingPrerequisite {
                profile_id: self.profile_id.clone(),
                detail,
                images: Vec::new(),
            });
        };

        // Both tools are checked up front (fail-fast, `meta/docs/2026-06-29-signing.md` §5.1): `openssl` mints and
        // signs verity-hash, `sbsign` signs PE artifacts. We cannot yet know which artifact types IC
        // will emit, so require both whenever a profile signs — a missing tool fails at build start
        // rather than minutes into a customize.
        if !tool_on_path(OPENSSL) {
            note(format!(
                "`{OPENSSL}` not found on PATH (required to mint/sign)"
            ));
        }
        if !tool_on_path(SBSIGN) {
            note(format!(
                "`{SBSIGN}` not found on PATH (required to sign PE/UKI boot artifacts)"
            ));
        }
        match &self.key_source {
            KeySource::LocalTestCa { .. } => {}
            KeySource::Keypair { key, cert } => {
                if let Err(detail) = check_pem(key, "key") {
                    note(detail);
                }
                if let Err(detail) = check_pem(cert, "cert") {
                    note(detail);
                }
            }
            KeySource::Unsupported { backend } => {
                note(format!(
                    "backend `{backend}` signing is not implemented yet"
                ));
            }
        }

        if missing.is_empty() {
            Ok(())
        } else {
            Err(SignError::Preflight { missing })
        }
    }

    fn sign(&self, plan: &SigningPlan) -> Result<SigningResult, SignError> {
        let manifest = InjectManifest::load(&plan.inject_files_yaml)?;
        // `source` paths in inject-files.yaml resolve relative to the yaml file itself (IC's rule).
        let base = plan
            .inject_files_yaml
            .parent()
            .unwrap_or(&plan.artifacts_dir);

        // Resolve the concrete (key, cert) used to sign. For `local-test-ca` the leaf lives in a
        // temp dir held in `_leaf` for the duration of signing; the CA is minted once per signer.
        let _leaf: Option<TempDir>;
        let (key, cert, published_ca_cert) = match &self.key_source {
            KeySource::Keypair { key, cert } => {
                _leaf = None;
                (key.clone(), cert.clone(), None)
            }
            KeySource::Unsupported { backend } => {
                return Err(SignError::Execution {
                    detail: format!("backend `{backend}` signing is not implemented yet"),
                });
            }
            KeySource::LocalTestCa { publish_override } => {
                let (ca_key, ca_cert) = self.get_or_mint_ca()?;
                let leaf_dir = TempDir::new().map_err(|e| SignError::Execution {
                    detail: format!("create leaf temp dir: {e}"),
                })?;
                let leaf_key = leaf_dir.path().join("leaf.key");
                let leaf_cert = leaf_dir.path().join("leaf.crt");
                mint_leaf(
                    &ca_key,
                    &ca_cert,
                    &leaf_key,
                    &leaf_cert,
                    &plan.leaf_id,
                    leaf_dir.path(),
                )?;
                // Publish the CA cert to the profile override, else beside the image at the executor's
                // per-image default (`<output_dir>/<slug>.ca_cert.pem`), for firmware enrollment.
                let dest = publish_override
                    .clone()
                    .unwrap_or_else(|| plan.ca_cert_dest.clone());
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| io_err("create CA cert dir", parent, &e))?;
                }
                fs::copy(&ca_cert, &dest).map_err(|e| io_err("publish CA cert", &dest, &e))?;
                _leaf = Some(leaf_dir);
                (leaf_key, leaf_cert, Some(dest))
            }
        };

        for artifact in &manifest.inject_files {
            let path = base.join(&artifact.source);
            match artifact.kind() {
                ArtifactKind::Pe => sign_pe(&path, &key, &cert)?,
                ArtifactKind::Verity => sign_verity(&path, &key, &cert)?,
                ArtifactKind::Skip => {
                    debug!(source = %artifact.source.display(), "skipping unrecognized artifact type");
                }
            }
        }

        Ok(SigningResult { published_ca_cert })
    }
}

impl HostSigner {
    /// Get the signer's CA, minting it (once) on first use. Kept in `self.ca` so every cell's leaf
    /// chains to one trust root; the temp dir wipes the CA private key when the signer drops.
    fn get_or_mint_ca(&self) -> Result<(PathBuf, PathBuf), SignError> {
        let mut guard = self.ca.lock().map_err(|_| SignError::Execution {
            detail: "CA state lock poisoned".to_owned(),
        })?;
        if guard.is_none() {
            let dir = TempDir::new().map_err(|e| SignError::Execution {
                detail: format!("create CA temp dir: {e}"),
            })?;
            let key = dir.path().join("ca.key");
            let cert = dir.path().join("ca_cert.pem");
            mint_ca(&key, &cert)?;
            *guard = Some(Ca {
                _dir: dir,
                key,
                cert,
            });
        }
        let ca = guard.as_ref().expect("invariant: CA minted above");
        Ok((ca.key.clone(), ca.cert.clone()))
    }
}

// ───────────────────────────── inject-files.yaml model ─────────────────────────────

/// The subset of IC's `inject-files.yaml` the signer needs: each artifact's `source` (the file to
/// sign, relative to the yaml) and its `type` (which signer to use). Other IC fields (`partition`,
/// `destination`, `previewFeatures`) are ignored — tailor signs the file and IC re-injects it.
#[derive(Debug, Deserialize)]
struct InjectManifest {
    #[serde(default, rename = "injectFiles")]
    inject_files: Vec<InjectArtifact>,
}

impl InjectManifest {
    fn load(path: &Path) -> Result<Self, SignError> {
        let text =
            fs::read_to_string(path).map_err(|e| io_err("read inject-files.yaml", path, &e))?;
        serde_yaml_ng::from_str(&text).map_err(|e| SignError::Execution {
            detail: format!("parse inject-files.yaml `{}`: {e}", path.display()),
        })
    }
}

#[derive(Debug, Deserialize)]
struct InjectArtifact {
    source: PathBuf,
    #[serde(default, rename = "type")]
    artifact_type: ArtifactType,
}

impl InjectArtifact {
    fn kind(&self) -> ArtifactKind {
        match self.artifact_type {
            ArtifactType::Ukis
            | ArtifactType::UkiAddons
            | ArtifactType::Shim
            | ArtifactType::Bootloader => ArtifactKind::Pe,
            ArtifactType::VerityHash => ArtifactKind::Verity,
            // No explicit type: infer from the filename so a `type: ""` entry still signs correctly.
            ArtifactType::Default => infer_kind(&self.source),
        }
    }
}

/// IC's `output.artifacts` item types (`imagecustomizerapi/outputartifactsitemtype.go`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ArtifactType {
    Ukis,
    UkiAddons,
    Shim,
    Bootloader,
    VerityHash,
    /// Absent / empty `type:`.
    #[serde(other)]
    #[default]
    Default,
}

/// How an artifact is signed.
#[derive(Debug, PartialEq, Eq)]
enum ArtifactKind {
    /// PE/Authenticode via `sbsign` (UKIs, shim, systemd-boot `.efi`).
    Pe,
    /// Detached CMS/DER via `openssl smime` (dm-verity root hash).
    Verity,
    /// Unrecognized — left untouched.
    Skip,
}

/// Infer the signer for a `type: ""` entry from its filename: `.efi` ⇒ PE; a name mentioning verity
/// ⇒ verity-hash; otherwise skip.
fn infer_kind(source: &Path) -> ArtifactKind {
    if source
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("efi"))
    {
        return ArtifactKind::Pe;
    }
    let name = source
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.contains("verity") || name.contains("roothash") || name.contains("root-hash") {
        ArtifactKind::Verity
    } else {
        ArtifactKind::Skip
    }
}

// ───────────────────────────── openssl / sbsign orchestration ─────────────────────────────

/// Mint a self-signed CA (RSA-2048) with `openssl req -x509`.
fn mint_ca(ca_key: &Path, ca_cert: &Path) -> Result<(), SignError> {
    run(
        OPENSSL,
        &[
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            &lossy(ca_key),
            "-out",
            &lossy(ca_cert),
            "-days",
            CERT_VALIDITY_DAYS,
            "-subj",
            "/CN=tailor local test CA",
        ],
    )
}

/// Mint a code-signing leaf (`extendedKeyUsage=codeSigning`) signed by the CA. The leaf CN embeds the
/// per-cell `leaf_id` so parallel cells never share a leaf.
fn mint_leaf(
    ca_key: &Path,
    ca_cert: &Path,
    leaf_key: &Path,
    leaf_cert: &Path,
    leaf_id: &str,
    work: &Path,
) -> Result<(), SignError> {
    let csr = work.join("leaf.csr");
    run(
        OPENSSL,
        &[
            "req",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            &lossy(leaf_key),
            "-out",
            &lossy(&csr),
            "-subj",
            &format!("/CN=tailor leaf {leaf_id}"),
        ],
    )?;
    // codeSigning EKU via an ext file (process substitution isn't available through `Command`).
    let ext = work.join("leaf.ext");
    fs::write(&ext, "extendedKeyUsage=codeSigning\n")
        .map_err(|e| io_err("write leaf ext", &ext, &e))?;
    run(
        OPENSSL,
        &[
            "x509",
            "-req",
            "-in",
            &lossy(&csr),
            "-CA",
            &lossy(ca_cert),
            "-CAkey",
            &lossy(ca_key),
            "-CAcreateserial",
            "-out",
            &lossy(leaf_cert),
            "-days",
            CERT_VALIDITY_DAYS,
            "-extfile",
            &lossy(&ext),
        ],
    )
}

/// PE/Authenticode-sign `file` in place with `sbsign` (writes to a temp output, then renames over).
fn sign_pe(file: &Path, key: &Path, cert: &Path) -> Result<(), SignError> {
    let signed = with_suffix(file, ".signed");
    run(
        SBSIGN,
        &[
            "--key",
            &lossy(key),
            "--cert",
            &lossy(cert),
            "--output",
            &lossy(&signed),
            &lossy(file),
        ],
    )?;
    fs::rename(&signed, file).map_err(|e| io_err("replace with signed PE", file, &e))
}

/// Sign a dm-verity root hash with a detached CMS/DER signature (`openssl smime`), replacing the
/// artifact in place — mirroring Trident's `sign.py`.
///
/// NOTE: the exact placement of the detached signature relative to the hash artifact is IC-version
/// dependent (`meta/docs/2026-06-29-signing.md` §10, open question) — validate against a real `inject-files` run.
fn sign_verity(file: &Path, key: &Path, cert: &Path) -> Result<(), SignError> {
    let sig = with_suffix(file, ".sig");
    run(
        OPENSSL,
        &[
            "smime",
            "-sign",
            "-noattr",
            "-binary",
            "-outform",
            "der",
            "-in",
            &lossy(file),
            "-out",
            &lossy(&sig),
            "-signer",
            &lossy(cert),
            "-inkey",
            &lossy(key),
        ],
    )?;
    fs::rename(&sig, file).map_err(|e| io_err("replace with verity signature", file, &e))
}

// ───────────────────────────── small helpers ─────────────────────────────

/// Is `name` an executable file on `PATH`? A cheap, side-effect-free presence probe for preflight.
fn tool_on_path(name: &str) -> bool {
    env::var_os("PATH")
        .is_some_and(|paths| env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
}

/// Run an external tool, mapping a spawn failure or non-zero exit to [`SignError::Execution`] with
/// the captured stderr.
fn run(tool: &str, args: &[&str]) -> Result<(), SignError> {
    let output = Command::new(tool)
        .args(args)
        .output()
        .map_err(|e| SignError::Execution {
            detail: format!("failed to run `{tool}`: {e}"),
        })?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(SignError::Execution {
            detail: format!("`{tool}` failed ({}): {}", output.status, stderr.trim()),
        })
    }
}

fn check_pem(path: &Path, label: &str) -> Result<(), String> {
    let bytes =
        fs::read(path).map_err(|e| format!("cannot read `{label}` `{}`: {e}", path.display()))?;
    if String::from_utf8_lossy(&bytes)
        .trim_start()
        .starts_with("-----BEGIN")
    {
        Ok(())
    } else {
        Err(format!(
            "`{label}` `{}` is not a PEM file (missing a `-----BEGIN` header)",
            path.display()
        ))
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn lossy(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn io_err(context: &str, path: &Path, source: &IoError) -> SignError {
    SignError::Execution {
        detail: format!("{context} `{}`: {source}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use tailor_config::{SigningBackend, SigningProfile};
    use tempfile::tempdir;

    fn profile(backend: SigningBackend) -> SigningProfile {
        SigningProfile {
            backend,
            key: None,
            cert: None,
            publish_ca_cert: None,
            vault: None,
            certificate: None,
        }
    }

    fn openssl_available() -> bool {
        tool_on_path(OPENSSL)
    }

    #[test]
    fn artifact_type_dispatch_covers_every_kind() {
        let pe = |t: ArtifactType| InjectArtifact {
            source: PathBuf::from("boot.efi"),
            artifact_type: t,
        };
        assert_eq!(pe(ArtifactType::Ukis).kind(), ArtifactKind::Pe);
        assert_eq!(pe(ArtifactType::Shim).kind(), ArtifactKind::Pe);
        assert_eq!(pe(ArtifactType::Bootloader).kind(), ArtifactKind::Pe);
        assert_eq!(pe(ArtifactType::UkiAddons).kind(), ArtifactKind::Pe);
        assert_eq!(
            InjectArtifact {
                source: PathBuf::from("verity"),
                artifact_type: ArtifactType::VerityHash,
            }
            .kind(),
            ArtifactKind::Verity
        );
    }

    #[test]
    fn default_type_is_inferred_from_filename() {
        assert_eq!(infer_kind(Path::new("vmlinuz.efi")), ArtifactKind::Pe);
        assert_eq!(
            infer_kind(Path::new("root-verity.hash")),
            ArtifactKind::Verity
        );
        assert_eq!(infer_kind(Path::new("readme.txt")), ArtifactKind::Skip);
    }

    #[test]
    fn manifest_parses_ic_inject_files_shape() {
        let dir = tempdir().unwrap();
        let yaml = dir.path().join("inject-files.yaml");
        fs::write(
            &yaml,
            "previewFeatures: [inject-files]\n\
             injectFiles:\n\
             - partition:\n    idType: part-label\n    id: esp\n  destination: /EFI/BOOT/bootx64.efi\n  source: ./bootx64.efi\n  type: shim\n\
             - partition:\n    idType: part-label\n    id: esp\n  destination: /root.hash\n  source: ./root.hash\n  type: verity-hash\n",
        )
        .unwrap();
        let manifest = InjectManifest::load(&yaml).unwrap();
        assert_eq!(manifest.inject_files.len(), 2);
        assert_eq!(manifest.inject_files[0].kind(), ArtifactKind::Pe);
        assert_eq!(manifest.inject_files[1].kind(), ArtifactKind::Verity);
    }

    #[test]
    fn keypair_preflight_reports_missing_key_and_cert() {
        let dir = tempdir().unwrap();
        let mut byo = profile(SigningBackend::Keypair);
        byo.key = Some(PathBuf::from("absent.key"));
        byo.cert = Some(PathBuf::from("absent.crt"));
        let signer = build_signer("byo", &byo, dir.path());
        let SignError::Preflight { missing } = signer.preflight().unwrap_err() else {
            panic!("expected a preflight error");
        };
        // openssl/sbsign presence varies by host, so assert on the key/cert prerequisites we control.
        assert!(missing.iter().any(|m| m.detail.contains("`key`")));
        assert!(missing.iter().any(|m| m.detail.contains("`cert`")));
    }

    #[test]
    fn azure_key_vault_preflight_reports_unimplemented() {
        let dir = tempdir().unwrap();
        let signer = build_signer("kv", &profile(SigningBackend::AzureKeyVault), dir.path());
        let SignError::Preflight { missing } = signer.preflight().unwrap_err() else {
            panic!("expected a preflight error");
        };
        assert!(missing.iter().any(|m| m.detail.contains("not implemented")));
    }

    #[test]
    fn local_test_ca_mints_ca_and_leaf_and_signs_verity() {
        if !openssl_available() {
            eprintln!("skipping: openssl not on PATH");
            return;
        }
        let dir = tempdir().unwrap();
        let artifacts = dir.path().join("artifacts");
        fs::create_dir_all(&artifacts).unwrap();
        // A verity-hash artifact (openssl can sign arbitrary bytes; sbsign needs a real PE, so this
        // test exercises the openssl path end to end without a PE fixture).
        fs::write(artifacts.join("root.hash"), b"deadbeef root hash bytes").unwrap();
        fs::write(
            artifacts.join("inject-files.yaml"),
            "injectFiles:\n- partition:\n    idType: part-label\n    id: esp\n  destination: /root.hash\n  source: ./root.hash\n  type: verity-hash\n",
        )
        .unwrap();

        let signer = build_signer("test-ca", &profile(SigningBackend::LocalTestCa), dir.path());
        signer.preflight().unwrap();
        let ca_dest = dir.path().join("image.ca_cert.pem");
        let plan = SigningPlan {
            inject_files_yaml: artifacts.join("inject-files.yaml"),
            artifacts_dir: artifacts.clone(),
            leaf_id: "solo_amd64_cosi".to_owned(),
            ca_cert_dest: ca_dest.clone(),
        };
        let result = signer.sign(&plan).unwrap();
        // CA published, and the verity artifact was replaced by a non-empty DER signature.
        assert_eq!(result.published_ca_cert.as_deref(), Some(ca_dest.as_path()));
        let ca_pem = fs::read_to_string(&ca_dest).unwrap();
        assert!(ca_pem.contains("BEGIN CERTIFICATE"));
        let signed_bytes = fs::read(artifacts.join("root.hash")).unwrap();
        assert!(!signed_bytes.is_empty());
        assert_ne!(
            signed_bytes, b"deadbeef root hash bytes",
            "artifact should be re-signed in place"
        );
    }

    #[test]
    fn local_test_ca_honors_publish_override() {
        if !openssl_available() {
            eprintln!("skipping: openssl not on PATH");
            return;
        }
        let dir = tempdir().unwrap();
        let artifacts = dir.path().join("artifacts");
        fs::create_dir_all(&artifacts).unwrap();
        fs::write(artifacts.join("root.hash"), b"hash bytes").unwrap();
        fs::write(
            artifacts.join("inject-files.yaml"),
            "injectFiles:\n- partition:\n    idType: part-label\n    id: esp\n  destination: /root.hash\n  source: ./root.hash\n  type: verity-hash\n",
        )
        .unwrap();

        let mut profile = profile(SigningBackend::LocalTestCa);
        profile.publish_ca_cert = Some(PathBuf::from("pinned/ca.pem"));
        let signer = build_signer("test-ca", &profile, dir.path());
        let plan = SigningPlan {
            inject_files_yaml: artifacts.join("inject-files.yaml"),
            artifacts_dir: artifacts.clone(),
            leaf_id: "solo_amd64_cosi".to_owned(),
            // The per-image default, which the override must win over.
            ca_cert_dest: dir.path().join("image.ca_cert.pem"),
        };
        let result = signer.sign(&plan).unwrap();
        let pinned = dir.path().join("pinned/ca.pem");
        assert_eq!(result.published_ca_cert.as_deref(), Some(pinned.as_path()));
        assert!(pinned.exists(), "override path should receive the CA");
        assert!(
            !dir.path().join("image.ca_cert.pem").exists(),
            "default path should be unused when the override is set"
        );
    }
}
