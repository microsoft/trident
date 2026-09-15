//! Signing preflight — the fail-fast capability check run once before any IC build
//! (`meta/docs/2026-06-29-signing.md` §5.1).
//!
//! It verifies tailor *can* sign (tool/key/credentials present) so a signed build never customizes N
//! cells only to fail at the signing step and leave half-built, root-owned outputs around. The probe
//! is cheap and side-effect-free.
//!
//! This module is the **foundation** (config + preflight). The signing *execution* — cert minting via
//! `rcgen`, PE signing via `sbsign`, and the `inject-files` IC pass — is a later milestone
//! (`meta/docs/2026-06-29-signing.md` §11, S1-remainder). Until it lands, `tailor` refuses a signed build rather
//! than silently emit an unsigned image.

use std::{
    fmt, fs,
    io::{self, Read as _},
    path::Path,
};

use tailor_config::{SigningBackend, SigningProfile};

/// Bytes read to check for a PEM header — far more than any `-----BEGIN …-----` armor line, but
/// bounded so a mistaken path to a huge or special file (e.g. `/dev/zero`) can't hang or exhaust
/// memory during preflight.
const PEM_PROBE_BYTES: u64 = 8192;

/// A signing prerequisite that a profile could not satisfy at preflight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingPrerequisite {
    /// The profile id that failed.
    pub profile_id: String,
    /// What is missing (e.g. an unreadable key file).
    pub detail: String,
    /// The images that requested this profile (so the user sees what to fix).
    pub images: Vec<String>,
}

/// The signing feature's error. Preflight aggregates *every* unmet prerequisite so the user fixes
/// them all in one pass rather than one failed build at a time (`meta/docs/2026-06-29-signing.md` §5.1);
/// execution failures wrap the failing step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignError {
    /// Preflight found unmet prerequisites — a missing tool binary, an unreadable key, etc. Reported
    /// in aggregate, before any (slow, privileged) IC run.
    Preflight { missing: Vec<MissingPrerequisite> },
    /// A signing step failed during execution (openssl/sbsign, `inject-files.yaml` handling, or IO).
    Execution { detail: String },
}

impl fmt::Display for SignError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignError::Preflight { missing } => {
                write!(
                    formatter,
                    "signing preflight failed — fix every prerequisite below, then rebuild:"
                )?;
                for item in missing {
                    write!(
                        formatter,
                        "\n  - profile `{}` (needed by: {}): {}",
                        item.profile_id,
                        item.images.join(", "),
                        item.detail
                    )?;
                }
                Ok(())
            }
            SignError::Execution { detail } => write!(formatter, "signing failed: {detail}"),
        }
    }
}

impl std::error::Error for SignError {}

/// A distinct signing profile the selected build requires, plus the images that requested it.
#[derive(Debug, Clone)]
pub struct SigningRequirement<'a> {
    /// The resolved profile id.
    pub profile_id: String,
    /// The resolved profile.
    pub profile: &'a SigningProfile,
    /// Images (by name) that resolved to this profile.
    pub images: Vec<String>,
}

/// Probe a single profile's prerequisites — cheap, side-effect-free (`meta/docs/2026-06-29-signing.md` §5.1).
///
/// - `keypair` → the `key` and `cert` files exist, are readable, and look like PEM. Both are checked
///   so a single preflight names *every* unmet prerequisite, not just the first.
/// - `local-test-ca` → always satisfiable (pure-Rust `rcgen` mints keys at sign time).
/// - `azure-key-vault` → structural config only here; a live credential/handshake probe is a later
///   milestone (`meta/docs/2026-06-29-signing.md` §11, S2).
///
/// Relative `key`/`cert` paths resolve against `base_dir` (the workspace root, where `tailor.yaml`
/// lives). Returns every unmet prerequisite for this profile (empty ⇒ ready).
pub fn preflight_profile(profile: &SigningProfile, base_dir: &Path) -> Vec<String> {
    let mut unmet = Vec::new();
    if profile.backend == SigningBackend::Keypair {
        if let Err(detail) = check_pem(profile.key.as_deref(), "key", base_dir) {
            unmet.push(detail);
        }
        if let Err(detail) = check_pem(profile.cert.as_deref(), "cert", base_dir) {
            unmet.push(detail);
        }
    }
    unmet
}

fn check_pem(path: Option<&Path>, label: &str, base_dir: &Path) -> Result<(), String> {
    let path = path.ok_or_else(|| format!("`{label}` path is not set"))?;
    let resolved = base_dir.join(path);
    let unreadable =
        |err: io::Error| format!("cannot read `{label}` `{}`: {err}", resolved.display());
    let file = fs::File::open(&resolved).map_err(unreadable)?;
    let mut head = Vec::new();
    file.take(PEM_PROBE_BYTES)
        .read_to_end(&mut head)
        .map_err(unreadable)?;
    if !looks_like_pem(&head) {
        return Err(format!(
            "`{label}` `{}` is not a PEM file (missing a `-----BEGIN` header)",
            resolved.display()
        ));
    }
    Ok(())
}

/// A PEM file begins with a `-----BEGIN` armor line (after any leading whitespace).
fn looks_like_pem(bytes: &[u8]) -> bool {
    String::from_utf8_lossy(bytes)
        .trim_start()
        .starts_with("-----BEGIN")
}

/// Run the signing preflight over every distinct required profile (`meta/docs/2026-06-29-signing.md` §5.1).
///
/// On any failure this returns a single [`SignError`] naming **all** unmet prerequisites (across all
/// profiles, and both files of a `keypair`), so the caller can abort the whole build before
/// customizing any cell. Relative key/cert paths resolve against `base_dir` (the workspace root).
pub fn preflight(
    requirements: &[SigningRequirement<'_>],
    base_dir: &Path,
) -> Result<(), SignError> {
    let mut missing = Vec::new();
    for requirement in requirements {
        for detail in preflight_profile(requirement.profile, base_dir) {
            missing.push(MissingPrerequisite {
                profile_id: requirement.profile_id.clone(),
                detail,
                images: requirement.images.clone(),
            });
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(SignError::Preflight { missing })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{fs, path::PathBuf};

    use tempfile::tempdir;

    use tailor_config::SigningProfile;

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

    #[test]
    fn local_test_ca_always_passes() {
        let dir = tempdir().unwrap();
        assert!(preflight_profile(&profile(SigningBackend::LocalTestCa), dir.path()).is_empty());
    }

    #[test]
    fn keypair_passes_with_readable_pem_files() {
        let dir = tempdir().unwrap();
        let key = dir.path().join("db.key");
        let cert = dir.path().join("db.crt");
        fs::write(
            &key,
            "-----BEGIN PRIVATE KEY-----\nMII...\n-----END PRIVATE KEY-----\n",
        )
        .unwrap();
        fs::write(
            &cert,
            "-----BEGIN CERTIFICATE-----\nMII...\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        let mut keypair = profile(SigningBackend::Keypair);
        keypair.key = Some(key);
        keypair.cert = Some(cert);
        assert!(preflight_profile(&keypair, dir.path()).is_empty());
    }

    #[test]
    fn keypair_resolves_relative_paths_against_base_dir() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("keys")).unwrap();
        fs::write(
            dir.path().join("keys/db.key"),
            "-----BEGIN PRIVATE KEY-----\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("keys/db.crt"),
            "-----BEGIN CERTIFICATE-----\n",
        )
        .unwrap();
        let mut keypair = profile(SigningBackend::Keypair);
        keypair.key = Some(PathBuf::from("./keys/db.key"));
        keypair.cert = Some(PathBuf::from("./keys/db.crt"));
        // Relative paths resolve against base_dir, not the process CWD.
        assert!(preflight_profile(&keypair, dir.path()).is_empty());
    }

    #[test]
    fn keypair_reports_both_missing_key_and_cert() {
        let dir = tempdir().unwrap();
        let mut keypair = profile(SigningBackend::Keypair);
        keypair.key = Some(dir.path().join("absent.key"));
        keypair.cert = Some(dir.path().join("absent.crt"));
        let unmet = preflight_profile(&keypair, dir.path());
        assert_eq!(unmet.len(), 2, "both key and cert should be reported");
        assert!(unmet.iter().any(|detail| detail.contains("`key`")));
        assert!(unmet.iter().any(|detail| detail.contains("`cert`")));
    }

    #[test]
    fn keypair_fails_when_file_is_not_pem() {
        let dir = tempdir().unwrap();
        let key = dir.path().join("db.key");
        let cert = dir.path().join("db.crt");
        fs::write(&key, "not a pem file").unwrap();
        fs::write(&cert, "-----BEGIN CERTIFICATE-----\n").unwrap();
        let mut keypair = profile(SigningBackend::Keypair);
        keypair.key = Some(key);
        keypair.cert = Some(cert);
        let unmet = preflight_profile(&keypair, dir.path());
        assert_eq!(unmet.len(), 1);
        assert!(unmet[0].contains("is not a PEM file"));
    }

    #[test]
    fn preflight_aggregates_every_missing_prerequisite() {
        let dir = tempdir().unwrap();
        let mut broken = profile(SigningBackend::Keypair);
        broken.key = Some(dir.path().join("absent.key"));
        broken.cert = Some(dir.path().join("absent.crt"));
        let ok = profile(SigningBackend::LocalTestCa);

        let requirements = vec![
            SigningRequirement {
                profile_id: "byo".to_owned(),
                profile: &broken,
                images: vec!["appliance".to_owned()],
            },
            SigningRequirement {
                profile_id: "test-ca".to_owned(),
                profile: &ok,
                images: vec!["demo".to_owned()],
            },
        ];

        let err = preflight(&requirements, dir.path()).unwrap_err();
        // Both the missing key and the missing cert of the `byo` profile are reported.
        let SignError::Preflight { missing } = &err else {
            panic!("expected a preflight error, got {err:?}");
        };
        assert_eq!(missing.len(), 2);
        assert!(missing.iter().all(|item| item.profile_id == "byo"));
        let rendered = err.to_string();
        assert!(rendered.contains("byo"));
        assert!(rendered.contains("appliance"));
    }

    #[test]
    fn preflight_passes_when_all_profiles_are_satisfiable() {
        let dir = tempdir().unwrap();
        let ok = profile(SigningBackend::LocalTestCa);
        let requirements = vec![SigningRequirement {
            profile_id: "test-ca".to_owned(),
            profile: &ok,
            images: vec!["demo".to_owned()],
        }];
        assert!(preflight(&requirements, dir.path()).is_ok());
    }
}
