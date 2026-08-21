//! Determines the node's currently-running version, for comparison against
//! a request's `targetVersion` (e.g. to short-circuit to `AlreadyAtTarget`)
//! and for reporting the instance's current version to Nebraska.
//!
//! Lives in `core`, not `annotations`, because it has nothing to do with the
//! annotation wire format - it's plain env-var-configurable file probing,
//! used by both the annotation-driven orchestrator and the `omaha-only`
//! one-shot mode.

// current_active_version() reads the `VERSION_ID` key (overridable via
// TRIDENT_ACL_AGENT_CURRENT_VERSION_KEY, e.g. to `IMAGE_VERSION` for an ACL
// image that stamps its own per-build version there) out of os-release, but
// falls back to TRIDENT_ACL_AGENT_CURRENT_VERSION_FALLBACK's behavior if
// that key isn't present (e.g. a minimal dev/test os-release). Three forms
// are recognized:
//   - "always" (the default): report "0.0.0" as the current version. This
//     is a sentinel that cannot collide with a real release version
//     string, so it can never accidentally match a real requested target
//     version and cause handle_stage/handle_finalize to incorrectly
//     short-circuit to AlreadyAtTarget - useful on dev/test hosts that
//     always want to treat themselves as needing whatever update is
//     requested.
//   - "error": current_active_version() returns an error instead of
//     falling back to anything, so a misconfigured VERSION_ID/IMAGE_VERSION
//     key fails the in-flight operation loudly rather than silently
//     proceeding with a meaningless placeholder version - the right choice
//     for a production deployment that wants to catch this class of
//     misconfiguration immediately.
//   - anything else: used verbatim as the fallback "current version"
//     string, with no format validation - e.g. a specific sentinel a
//     dev/test host wants for its own purposes. This is not checked against
//     any version syntax, so it's the caller's responsibility to pick a
//     value that can't collide with a real target version if that matters
//     to them.
pub const DEFAULT_CURRENT_VERSION_FALLBACK: &str = "always";
/// Default path `current_active_version` reads. Overridable via
/// `TRIDENT_ACL_AGENT_CURRENT_VERSION_PATH` so a deployment can point the
/// agent at any file that follows the os-release format (`KEY=VALUE` lines,
/// optionally quoted, blank lines and `#` comments ignored - see
/// <https://www.freedesktop.org/software/systemd/man/latest/os-release.html>)
/// instead of the real `/etc/os-release`, e.g. a vendor-specific file that
/// carries the running image's version under a key `/etc/os-release`
/// doesn't have room for.
pub const DEFAULT_CURRENT_VERSION_PATH: &str = osutils::osrelease::OS_RELEASE_PATH;
/// Default os-release key `current_active_version` looks up for the running
/// image's version: `VERSION_ID`, a standard key every os-release carries
/// (see
/// <https://www.freedesktop.org/software/systemd/man/latest/os-release.html>).
/// Overridable via `TRIDENT_ACL_AGENT_CURRENT_VERSION_KEY` - e.g. to
/// `IMAGE_VERSION` for an ACL image that stamps its own per-build version
/// under that key instead - or point
/// `TRIDENT_ACL_AGENT_CURRENT_VERSION_PATH` at a different file entirely.
pub const DEFAULT_CURRENT_VERSION_KEY: &str = "VERSION_ID";
const ENV_CURRENT_VERSION_PATH: &str = "TRIDENT_ACL_AGENT_CURRENT_VERSION_PATH";
const ENV_CURRENT_VERSION_KEY: &str = "TRIDENT_ACL_AGENT_CURRENT_VERSION_KEY";
const ENV_CURRENT_VERSION_FALLBACK: &str = "TRIDENT_ACL_AGENT_CURRENT_VERSION_FALLBACK";
/// `TRIDENT_ACL_AGENT_CURRENT_VERSION_FALLBACK`'s "report 0.0.0" keyword.
const FALLBACK_ALWAYS: &str = "always";
/// `TRIDENT_ACL_AGENT_CURRENT_VERSION_FALLBACK`'s "fail instead" keyword.
const FALLBACK_ERROR: &str = "error";
/// What [`current_active_version`] reports for [`FALLBACK_ALWAYS`] - see its
/// docs above for why 0.0.0 is a safe sentinel here.
const FALLBACK_ALWAYS_VERSION: &str = "0.0.0";

/// Reads `name`, treating both "unset" and "set to the empty string" as
/// absent, matching `config::env_raw`'s convention: a drop-in override that
/// clears a variable to `""` should fall back to the default, not try to use
/// an empty value.
fn env_override(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

pub fn current_active_version() -> Result<String, anyhow::Error> {
    let path = env_override(ENV_CURRENT_VERSION_PATH)
        .unwrap_or_else(|| DEFAULT_CURRENT_VERSION_PATH.to_string());
    let key = env_override(ENV_CURRENT_VERSION_KEY)
        .unwrap_or_else(|| DEFAULT_CURRENT_VERSION_KEY.to_string());
    if let Some(value) = read_os_release_value(&path, &key) {
        return Ok(value);
    }
    let fallback = env_override(ENV_CURRENT_VERSION_FALLBACK)
        .unwrap_or_else(|| DEFAULT_CURRENT_VERSION_FALLBACK.to_string());
    match fallback.as_str() {
        FALLBACK_ERROR => Err(anyhow::anyhow!(
            "{key} not found in {path}, and {ENV_CURRENT_VERSION_FALLBACK} is set to \"error\""
        )),
        FALLBACK_ALWAYS => {
            log::warn!(
                "{key} not found in {path}; falling back to \"always\" (reporting {FALLBACK_ALWAYS_VERSION} as the current version)"
            );
            Ok(FALLBACK_ALWAYS_VERSION.to_string())
        }
        _ => {
            log::warn!(
                "{key} not found in {path}; falling back to configured current version {fallback:?}"
            );
            Ok(fallback)
        }
    }
}

/// Reads `path` (an os-release-formatted file: `KEY=VALUE` lines, blank
/// lines and `#` comments ignored, values optionally single- or
/// double-quoted - see
/// <https://www.freedesktop.org/software/systemd/man/latest/os-release.html>)
/// and returns the trimmed, unquoted value for `key`, or `None` if the file
/// can't be read, `key` isn't present, or its value is empty - all of which
/// `current_active_version` treats identically: fall back to the stub.
/// Split out from `current_active_version` so tests can point it at a temp
/// file instead of the real os-release.
fn read_os_release_value(path: &str, key: &str) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((line_key, raw_value)) = line.split_once('=') else {
            continue;
        };
        if line_key.trim() != key {
            continue;
        }
        let value = raw_value.trim().trim_matches('"').trim_matches('\'').trim();
        if value.is_empty() {
            return None;
        }
        return Some(value.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    #[test]
    fn read_os_release_value_returns_none_for_missing_file() {
        assert_eq!(
            read_os_release_value(
                "/nonexistent/path/does-not-exist-os-release",
                DEFAULT_CURRENT_VERSION_KEY
            ),
            None
        );
    }

    #[test]
    fn read_os_release_value_finds_requested_key() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("os-release-test-{}", Uuid::new_v4()));
        std::fs::write(
            &path,
            "NAME=\"Azure Linux\"\nIMAGE_VERSION=202608.6.0\nVERSION_ID=3.0\n",
        )
        .unwrap();
        let result = read_os_release_value(path.to_str().unwrap(), "IMAGE_VERSION");
        std::fs::remove_file(&path).ok();
        assert_eq!(result.as_deref(), Some("202608.6.0"));
    }

    #[test]
    fn read_os_release_value_trims_quotes_and_whitespace() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("os-release-test-quoted-{}", Uuid::new_v4()));
        std::fs::write(&path, "  IMAGE_VERSION = \"202608.6.0\" \n").unwrap();
        let result = read_os_release_value(path.to_str().unwrap(), "IMAGE_VERSION");
        std::fs::remove_file(&path).ok();
        assert_eq!(result.as_deref(), Some("202608.6.0"));
    }

    #[test]
    fn read_os_release_value_returns_none_for_missing_key() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("os-release-test-missing-key-{}", Uuid::new_v4()));
        std::fs::write(&path, "NAME=\"Azure Linux\"\nVERSION_ID=3.0\n").unwrap();
        let result = read_os_release_value(path.to_str().unwrap(), "IMAGE_VERSION");
        std::fs::remove_file(&path).ok();
        assert_eq!(result, None);
    }

    #[test]
    fn read_os_release_value_returns_none_for_empty_value() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("os-release-test-empty-value-{}", Uuid::new_v4()));
        std::fs::write(&path, "IMAGE_VERSION=\n").unwrap();
        let result = read_os_release_value(path.to_str().unwrap(), "IMAGE_VERSION");
        std::fs::remove_file(&path).ok();
        assert_eq!(result, None);
    }

    #[test]
    fn read_os_release_value_skips_comments_and_blank_lines() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("os-release-test-comments-{}", Uuid::new_v4()));
        std::fs::write(
            &path,
            "# a comment\n\n# IMAGE_VERSION=should-be-ignored\nIMAGE_VERSION=202608.6.0\n",
        )
        .unwrap();
        let result = read_os_release_value(path.to_str().unwrap(), "IMAGE_VERSION");
        std::fs::remove_file(&path).ok();
        assert_eq!(result.as_deref(), Some("202608.6.0"));
    }

    /// Clears all three env vars `current_active_version` reads. Environment
    /// mutation is process-global and `std::env::remove_var`/`set_var` are
    /// `unsafe` (not thread-safe against concurrent reads elsewhere in the
    /// process), so the defaults/overrides/read-path cases below are
    /// intentionally folded into one sequential `#[test]` rather than
    /// several separate ones that `cargo test` could run in parallel
    /// against the same variables.
    fn clear_current_version_env() {
        // SAFETY: single-threaded within this test function; no other test
        // in this crate reads or writes these three variables.
        unsafe {
            std::env::remove_var(ENV_CURRENT_VERSION_PATH);
            std::env::remove_var(ENV_CURRENT_VERSION_KEY);
            std::env::remove_var(ENV_CURRENT_VERSION_FALLBACK);
        }
    }

    #[test]
    fn current_active_version_path_key_and_fallback_are_overridable_via_env() {
        clear_current_version_env();
        assert_eq!(env_override(ENV_CURRENT_VERSION_PATH), None);
        assert_eq!(env_override(ENV_CURRENT_VERSION_KEY), None);

        // SAFETY: see clear_current_version_env's doc comment.
        unsafe {
            std::env::set_var(ENV_CURRENT_VERSION_PATH, "/custom/os-release");
        }
        assert_eq!(
            env_override(ENV_CURRENT_VERSION_PATH).as_deref(),
            Some("/custom/os-release")
        );

        // SAFETY: see clear_current_version_env's doc comment.
        unsafe {
            std::env::set_var(ENV_CURRENT_VERSION_KEY, "CUSTOM_VERSION_KEY");
        }
        assert_eq!(
            env_override(ENV_CURRENT_VERSION_KEY).as_deref(),
            Some("CUSTOM_VERSION_KEY")
        );

        // SAFETY: see clear_current_version_env's doc comment.
        unsafe {
            std::env::set_var(ENV_CURRENT_VERSION_FALLBACK, "custom-fallback");
        }
        assert_eq!(
            env_override(ENV_CURRENT_VERSION_FALLBACK).as_deref(),
            Some("custom-fallback")
        );

        // An empty override is treated the same as unset.
        // SAFETY: see clear_current_version_env's doc comment.
        unsafe {
            std::env::set_var(ENV_CURRENT_VERSION_PATH, "");
            std::env::set_var(ENV_CURRENT_VERSION_KEY, "");
        }
        assert_eq!(env_override(ENV_CURRENT_VERSION_PATH), None);
        assert_eq!(env_override(ENV_CURRENT_VERSION_KEY), None);

        clear_current_version_env();

        // current_active_version() itself honors TRIDENT_ACL_AGENT_CURRENT_VERSION_PATH,
        // pointing it at an arbitrary os-release-formatted file instead of the
        // real /etc/os-release.
        let dir = std::env::temp_dir();
        let found_path = dir.join(format!(
            "os-release-test-current-version-{}",
            Uuid::new_v4()
        ));
        std::fs::write(
            &found_path,
            "NAME=\"Contoso Linux\"\nVERSION_ID=202608.6.0\n",
        )
        .unwrap();
        // SAFETY: see clear_current_version_env's doc comment.
        unsafe {
            std::env::set_var(ENV_CURRENT_VERSION_PATH, found_path.to_str().unwrap());
            std::env::set_var(ENV_CURRENT_VERSION_KEY, "VERSION_ID");
        }
        assert_eq!(current_active_version().unwrap(), "202608.6.0");
        std::fs::remove_file(&found_path).ok();
        clear_current_version_env();

        // When the configured key isn't present at the configured path, and
        // no fallback override is set, it defaults to "always" - reporting
        // FALLBACK_ALWAYS_VERSION ("0.0.0") as the current version.
        let missing_path = dir.join(format!(
            "os-release-test-current-version-missing-{}",
            Uuid::new_v4()
        ));
        std::fs::write(&missing_path, "NAME=\"Contoso Linux\"\n").unwrap();
        // SAFETY: see clear_current_version_env's doc comment.
        unsafe {
            std::env::set_var(ENV_CURRENT_VERSION_PATH, missing_path.to_str().unwrap());
            std::env::set_var(ENV_CURRENT_VERSION_KEY, "VERSION_ID");
        }
        assert_eq!(current_active_version().unwrap(), FALLBACK_ALWAYS_VERSION);

        // TRIDENT_ACL_AGENT_CURRENT_VERSION_FALLBACK="error" turns a missing
        // key into a hard error instead of a placeholder version.
        // SAFETY: see clear_current_version_env's doc comment.
        unsafe {
            std::env::set_var(ENV_CURRENT_VERSION_FALLBACK, FALLBACK_ERROR);
        }
        assert!(current_active_version().is_err());

        // Any other TRIDENT_ACL_AGENT_CURRENT_VERSION_FALLBACK value is used
        // verbatim, with no format validation.
        // SAFETY: see clear_current_version_env's doc comment.
        unsafe {
            std::env::set_var(
                ENV_CURRENT_VERSION_FALLBACK,
                "custom-fallback-for-missing-key",
            );
        }
        assert_eq!(
            current_active_version().unwrap(),
            "custom-fallback-for-missing-key"
        );

        std::fs::remove_file(&missing_path).ok();
        clear_current_version_env();
    }
}
