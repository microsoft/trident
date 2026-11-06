use std::fmt;

use serde::{Deserialize, Serialize};

/// A target architecture. Maps to the container platform `linux/<arch>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    Amd64,
    Arm64,
}

impl Arch {
    /// The token used in cell slugs and `linux/<arch>` platforms.
    pub fn as_str(self) -> &'static str {
        match self {
            Arch::Amd64 => "amd64",
            Arch::Arm64 => "arm64",
        }
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An Image Customizer output format (`--output-image-format`). See `meta/docs/2026-06-22-design.md` §10.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutputFormat {
    Cosi,
    Vhd,
    VhdFixed,
    Vhdx,
    Qcow2,
    Raw,
    Iso,
    PxeDir,
    PxeTar,
    BaremetalImage,
}

impl OutputFormat {
    /// The kebab-case token used in cell slugs (matches IC's `--output-image-format` value).
    pub fn as_str(self) -> &'static str {
        match self {
            OutputFormat::Cosi => "cosi",
            OutputFormat::Vhd => "vhd",
            OutputFormat::VhdFixed => "vhd-fixed",
            OutputFormat::Vhdx => "vhdx",
            OutputFormat::Qcow2 => "qcow2",
            OutputFormat::Raw => "raw",
            OutputFormat::Iso => "iso",
            OutputFormat::PxeDir => "pxe-dir",
            OutputFormat::PxeTar => "pxe-tar",
            OutputFormat::BaremetalImage => "baremetal-image",
        }
    }
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Post-build compression tailor applies to a cell's output artifact. Image Customizer writes the
/// raw image; tailor then streams it through the codec and publishes `<artifact>.<suffix>` (e.g.
/// `img.vhd.zst`). Not an IC feature — see `docs/reference/image-yaml.md`. Only `zstd` today; COSI's
/// own compression is separate (`cosiCompressionLevel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Compression {
    Zstd,
}

impl Compression {
    /// The lowercase codec token (also the `compression:` value).
    pub fn as_str(self) -> &'static str {
        match self {
            Compression::Zstd => "zstd",
        }
    }

    /// The filename suffix appended after the format extension (`<name>.<ext>.<suffix>`).
    pub fn suffix(self) -> &'static str {
        match self {
            Compression::Zstd => "zst",
        }
    }
}

/// A tailor preview feature — an opt-in switch for functionality that is not yet part of the stable
/// 1.0 contract. Enabled per workspace via `previewFeatures:` in `tailor.yaml` (mirroring Image
/// Customizer's own `previewFeatures`). Using a gated feature without listing it here is a hard
/// error. An unrecognized name is rejected at parse time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PreviewFeature {
    /// The signed-image pipeline (`signing:` and the `signing` workspace profiles).
    Signing,
}

impl PreviewFeature {
    /// The `previewFeatures:` token that enables this feature.
    pub fn as_str(self) -> &'static str {
        match self {
            PreviewFeature::Signing => "signing",
        }
    }
}

impl fmt::Display for Compression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The tailor-level IC operation. Default `customize` (`meta/docs/2026-06-22-design.md` §7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Operation {
    #[default]
    Customize,
    Convert,
}

/// How tailor manages an IC `output.artifacts` staging directory for a cell with no resolved
/// `signing:` profile (`meta/docs/2026-06-29-output-artifacts-staging.md` §3.3). Only consulted when the cell
/// opts into the `output-artifacts` preview feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputArtifactsPolicy {
    /// Relocate the extracted artifacts to the output directory, chown them to the caller, and keep
    /// them as a real cell output (never destroys a user-requested output).
    #[default]
    Managed,
    /// Treat the artifacts as signing scratch even when unsigned: extract, then reclaim sudo-free.
    Scratch,
    /// Drop the `output.artifacts` block so IC never extracts.
    Strip,
}

/// IC `--log-level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Panic,
    Fatal,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    /// The lowercase token passed to IC `--log-level`.
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Panic => "panic",
            LogLevel::Fatal => "fatal",
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A scalar `params` value (string | number | boolean), interpolated into `config:` scalars.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
}
