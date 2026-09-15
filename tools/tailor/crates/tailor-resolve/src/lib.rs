//! `tailor-resolve` — base-image and toolchain digest resolution adapters.

mod azure_linux;
mod download;
mod local;
mod oci;
mod resolver;
mod toolchain;

pub use download::OciFetcher;
pub use resolver::OciResolver;
