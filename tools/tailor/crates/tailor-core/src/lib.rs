//! `tailor-core` — domain model, port traits, lockfile, build stamps, and the build orchestrator
//! (`meta/docs/2026-06-22-architecture.md` §3.2). The hexagonal core: it defines the ports that `tailor-resolve`
//! and `tailor-exec` implement, and owns build orchestration. It does **not** model Image
//! Customizer's config schema or version capabilities — those are the user↔IC contract.

pub mod ado;
pub mod atomic;
pub mod catalogue;
pub mod deps;
pub mod domain;
pub mod error;
pub mod fingerprint;
pub mod hashcache;
pub mod imagedep;
pub mod lockfile;
pub mod orchestrator;
pub mod ports;
pub mod selector;
pub mod signing;
pub mod stamp;
pub mod testing;

pub use ado::{ado_matrix, is_valid_var_name};
pub use catalogue::{
    SlotOutcome, SlotReport, SlotSource, SlotSummary, download, summarize, verify,
};
pub use domain::{BuildPlan, Cell, CellSlug, Fingerprint, PlannedCell, Target};
pub use error::{CoreError, ExecError, ResolveError};
pub use hashcache::{FileHash, hash_file_cached};
pub use lockfile::{LockedBase, LockedContainer, LockedRuntime, Lockfile};
pub use orchestrator::{
    BuildOptions, BuildProgress, BuildSelection, Orchestrator, ResolvedToolchain,
    ResolvedToolsDirSource, artifact_name, cells, cells_selected, output_slug,
    published_artifact_name, runtime_config, select_node_cells, toolchain_for, toolchain_key,
    tools_dir_key,
};
pub use ports::{
    BaseImageFetcher, BaseResolver, ContainerConfig, ContainerResult, ContainerRuntime, DaemonInfo,
    ExecutionContext, ExecutionResult, Executor, FetchedBase, FilesystemOps, LocalImage, LogSource,
    ResolvedBase, RuntimeConfig, Signer, SigningPlan, SigningResult, ToolsDirPlan,
};
pub use selector::Selector;
pub use signing::{
    MissingPrerequisite, SignError, SigningRequirement, preflight, preflight_profile,
};
