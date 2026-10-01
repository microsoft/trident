//! `tailor-config` — parse, merge, and render tailor's manifest and image definitions.
//!
//! See `meta/docs/2026-06-22-architecture.md` §3.1. The content under an image's `config:` key is Image Customizer
//! configuration and is kept here as an opaque value tree — it is **never** modeled against IC's
//! schema. The merge engine is generic (deep-merge maps, append lists, `$set`/`$replace`/`$remove`
//! directives); tailor validates only its own inputs (matrix axes, exactly-one base), leaving IC
//! config validation to IC.

mod error;
mod fragment;
mod include;
mod interpolate;
mod loader;
mod matrix;
mod merge;
mod path;
mod render;
mod schema;
mod types;
mod workspace;

pub mod defaults;

pub use error::ConfigError;
pub use loader::{load_image, load_tool_config};
pub use matrix::{AxisTuple, cell_slug, expand};
pub use path::absolutize;
pub use render::{MergeStep, RenderedCell, merge_plan, render_image, write_golden};
pub use schema::*;
pub use types::*;
pub use workspace::{DiscoveredImage, Workspace, discover, find_manifest};
