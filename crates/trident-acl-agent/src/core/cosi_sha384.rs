//! Resolves a Nebraska package's COSI metadata SHA-384.
//!
//! `core::nebraska` is scoped strictly to the Omaha protocol and knows
//! nothing about COSI (see that module's doc comment); this module is the
//! "reader" that does, shared by both update flows
//! ([`crate::omahaonly`] and [`crate::annotations`]) that need to turn a
//! [`PackageFile`] into the hex-encoded `image.sha384` value Trident's gRPC
//! API expects.

use std::str::FromStr;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use thiserror::Error;

use super::nebraska::PackageFile;

/// The label key under which Nebraska reports the COSI metadata SHA-384 in
/// a package's `<labels>` child element.
const COSI_SHA384_LABEL_KEY: &str = "cosi.metadata.sha384";

/// The fixed size, in bytes, of a SHA-384 digest.
const SHA384_LEN_BYTES: usize = 48;

/// Selects which source [`resolve_cosi_sha384`] reads a package's COSI
/// metadata SHA-384 from. Defaults to [`Labels`](Self::Labels); override via
/// `TRIDENT_ACL_AGENT_COSI_SHA384_SOURCE` (`labels`|`hash`).
///
/// There is deliberately no "try labels, then fall back to hash" option: an
/// ambient, automatic fallback risks silently accepting a stale/wrong hash
/// from a manifest that should carry a label but doesn't, rather than
/// failing loudly. Callers that need the legacy path opt into
/// [`Hash`](Self::Hash) explicitly instead.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CosiSha384Source {
    /// Require the `cosi.metadata.sha384` label; never read the package's
    /// legacy `hash` attribute. The default.
    #[default]
    Labels,
    /// Always use the legacy `hash` attribute; ignore labels entirely. For
    /// talking to a Nebraska deployment with no label support.
    Hash,
}

impl FromStr for CosiSha384Source {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "labels" => Ok(Self::Labels),
            "hash" => Ok(Self::Hash),
            other => Err(format!(
                "unknown cosi-sha384-source {other:?} (expected \"labels\" or \"hash\")"
            )),
        }
    }
}

/// Errors resolving a package's COSI metadata SHA-384.
#[derive(Debug, Error)]
pub enum CosiSha384Error {
    /// Neither source [`CosiSha384Source`] selects has a value for this
    /// package.
    #[error("package has no COSI metadata SHA-384 for source {0:?}")]
    Missing(CosiSha384Source),

    /// The selected source's value is not valid base64, or doesn't fit in
    /// [`SHA384_LEN_BYTES`].
    #[error(
        "COSI metadata SHA-384 is not valid base64, or does not fit in {SHA384_LEN_BYTES} bytes (SHA-384): {0}"
    )]
    InvalidBase64(base64::DecodeSliceError),

    /// The selected source's value decodes to the wrong number of bytes for
    /// a SHA-384 digest.
    #[error("COSI metadata SHA-384 decodes to {0} bytes, expected {SHA384_LEN_BYTES} (SHA-384)")]
    WrongLength(usize),
}

/// Resolves `package`'s COSI metadata SHA-384, per `source`, into the
/// hex-encoded form Trident's gRPC `image.sha384` Host Configuration field
/// expects.
///
/// This is a straight re-encode (base64 -> raw bytes -> hex), not a real
/// hash-family conversion: our Nebraska deployment already stores a SHA-384
/// digest in the selected source, just base64-encoded instead of hex.
pub fn resolve_cosi_sha384(
    package: &PackageFile,
    source: CosiSha384Source,
) -> Result<String, CosiSha384Error> {
    let raw = match source {
        CosiSha384Source::Labels => package.label(COSI_SHA384_LABEL_KEY),
        CosiSha384Source::Hash => package.hash.as_ref().map(|h| h.sha1.as_str()),
    }
    .ok_or(CosiSha384Error::Missing(source))?;

    // Decode straight into a fixed-size stack buffer instead of
    // `Engine::decode`'s allocating `Vec<u8>` output: a malicious or buggy
    // Nebraska response could otherwise send an arbitrarily large base64
    // string, forcing a large allocation before we ever get to checking the
    // decoded length below. `decode_slice` rejects any input that would
    // decode to more than `SHA384_LEN_BYTES` without allocating
    // proportionally to the (untrusted) input size.
    let mut buf = [0u8; SHA384_LEN_BYTES];
    let decoded_len = STANDARD
        .decode_slice(raw, &mut buf)
        .map_err(CosiSha384Error::InvalidBase64)?;
    if decoded_len != SHA384_LEN_BYTES {
        return Err(CosiSha384Error::WrongLength(decoded_len));
    }
    Ok(hex::encode(buf))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use url::Url;

    use super::*;
    use crate::core::nebraska::PackageHash;

    // Digest generated with:
    //   echo -n "trident-cosi-metadata-round-trip-test" | openssl dgst -sha384 -binary | openssl base64 -A
    //   echo -n "trident-cosi-metadata-round-trip-test" | openssl dgst -sha384
    const BASE64_DIGEST: &str = "8e1AenlmEPzn7npBv5uxbUi2OO2frCiT52sDgbw/RM077QgziRyh7wCIy2YcHvRx";
    const HEX_DIGEST: &str =
        "f1ed407a796610fce7ee7a41bf9bb16d48b638ed9fac2893e76b0381bc3f44cd3bed0833891ca1ef0088cb661c1ef471";

    fn package_file(hash: Option<&str>, labels: &[(&str, &str)]) -> PackageFile {
        PackageFile {
            name: "os.cosi".to_string(),
            url: Url::parse("https://updates.example.com/os.cosi").unwrap(),
            hash: hash.map(|sha1| PackageHash {
                sha1: sha1.to_string(),
                sha256: None,
            }),
            size: Some(10),
            required: true,
            labels: labels
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    #[test]
    fn resolves_from_label_with_labels_source() {
        let package = package_file(Some("LEGACY"), &[("cosi.metadata.sha384", BASE64_DIGEST)]);
        assert_eq!(
            resolve_cosi_sha384(&package, CosiSha384Source::Labels).unwrap(),
            HEX_DIGEST
        );
    }

    #[test]
    fn rejects_invalid_labels_without_legacy_fallback() {
        let oversized = "A".repeat(68);
        for (label, wrong_length) in [
            ("not-valid-base64!!", None),
            ("AAAA", Some(3)),
            (oversized.as_str(), None),
        ] {
            let package = package_file(Some(BASE64_DIGEST), &[("cosi.metadata.sha384", label)]);
            let err = resolve_cosi_sha384(&package, CosiSha384Source::Labels).unwrap_err();
            if let Some(expected) = wrong_length {
                assert!(matches!(err, CosiSha384Error::WrongLength(actual) if actual == expected));
            } else {
                assert!(matches!(err, CosiSha384Error::InvalidBase64(_)));
            }
        }
    }

    #[test]
    fn missing_label_with_labels_source_is_an_error() {
        // No implicit fallback: a legacy `hash` attribute being present
        // does not help when the configured source is Labels.
        let package = package_file(Some("LEGACY"), &[]);
        let err = resolve_cosi_sha384(&package, CosiSha384Source::Labels).unwrap_err();
        assert!(matches!(
            err,
            CosiSha384Error::Missing(CosiSha384Source::Labels)
        ));
    }

    #[test]
    fn resolves_from_hash_with_hash_source() {
        let package = package_file(Some(BASE64_DIGEST), &[]);
        assert_eq!(
            resolve_cosi_sha384(&package, CosiSha384Source::Hash).unwrap(),
            HEX_DIGEST
        );
    }

    #[test]
    fn missing_hash_with_hash_source_is_an_error() {
        // No implicit fallback the other direction either: a <labels>
        // element being present does not help when the configured source
        // is Hash.
        let package = package_file(None, &[("cosi.metadata.sha384", BASE64_DIGEST)]);
        let err = resolve_cosi_sha384(&package, CosiSha384Source::Hash).unwrap_err();
        assert!(matches!(
            err,
            CosiSha384Error::Missing(CosiSha384Source::Hash)
        ));
    }

    #[test]
    fn converts_base64_to_hex() {
        // 48 zero bytes, base64-encoded - a well-formed (if not realistic)
        // SHA-384 digest.
        let package = package_file(
            Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
            &[],
        );
        assert_eq!(
            resolve_cosi_sha384(&package, CosiSha384Source::Hash).unwrap(),
            "0".repeat(96)
        );
    }

    #[test]
    fn round_trips_real_digest() {
        // A real SHA-384 digest of a known input (not synthetic bytes),
        // confirming the base64 -> hex re-encode matches an independently
        // computed value.
        let package = package_file(Some(BASE64_DIGEST), &[]);
        assert_eq!(
            resolve_cosi_sha384(&package, CosiSha384Source::Hash).unwrap(),
            HEX_DIGEST
        );
    }

    #[test]
    fn rejects_invalid_base64() {
        let package = package_file(Some("not-valid-base64!!"), &[]);
        let err = resolve_cosi_sha384(&package, CosiSha384Source::Hash).unwrap_err();
        assert!(matches!(err, CosiSha384Error::InvalidBase64(_)));
    }

    #[test]
    fn rejects_wrong_length() {
        // Valid base64, but decodes to far fewer than the 48 bytes a SHA-384
        // digest requires - e.g. a real SHA-1 (20 bytes), confirming this
        // check would catch a Nebraska deployment that (unlike ours) puts an
        // actual SHA-1 in this field.
        let package = package_file(Some("AAAAAAAAAAAAAAAAAAAAAAAAAAA="), &[]);
        let err = resolve_cosi_sha384(&package, CosiSha384Source::Hash).unwrap_err();
        assert!(matches!(err, CosiSha384Error::WrongLength(20)));
    }

    #[test]
    fn rejects_oversized_input() {
        // Valid base64 that decodes to more than the 48 bytes a SHA-384
        // digest requires. This must be rejected via the fixed-size decode
        // buffer rather than by first allocating a `Vec` sized to the
        // (potentially attacker-controlled) input.
        // 68 base64 chars (no padding needed, 68 % 4 == 0) decode to 51
        // bytes - more than the 48 a SHA-384 digest requires.
        let package = package_file(Some(&"A".repeat(68)), &[]);
        let err = resolve_cosi_sha384(&package, CosiSha384Source::Hash).unwrap_err();
        assert!(matches!(err, CosiSha384Error::InvalidBase64(_)));
    }
}
