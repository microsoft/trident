//! Unified command classification, shared by all three of Trident's entry
//! points (CLI `Commands`, `grpc-client`'s `ClientCommands`, and the
//! daemon), so "what is this command's wire name for
//! `command`/`operation_id` telemetry", "is this a servicing command for
//! `command_error` purposes", "may this command initialize a datastore
//! that doesn't exist yet", and "does this command generate a fresh
//! servicing ID" are each answered in exactly one place instead of being
//! independently re-derived by name-matching in `main.rs`,
//! `grpc_client::mod`, `datastore.rs`, and `logging::tracestream`.
//!
//! [`CommandKind`] is constructed once per entry point, from typed
//! arguments (see [`crate::cli::Commands::kind`] /
//! [`crate::cli::ClientCommands::kind`]), or via one of the daemon-only
//! named constructors below (e.g. [`CommandKind::install_stage`]), and
//! answers every question directly against its [`Family`] (which carries
//! [`Phase`] for the families that have one) via a plain `&self` method --
//! there is no string-keyed fallback anywhere. `logging::operation_context`
//! stores the typed `CommandKind` itself (not just its rendered name) for
//! exactly this reason: it's what lets `logging::tracestream`'s
//! `merge_operation_context` -- the one remaining call site that used to
//! have nothing but a name string, read back long after the original
//! `CommandKind` was constructed -- call
//! [`CommandKind::generates_servicing_id`] directly instead of re-deriving
//! the answer from that string.

use crate::{
    cli::{to_operations, ClientCommands, Commands},
    logging::operation_context::command_name,
};
use trident_api::config::Operations;

/// Which half (if any) of a staged/finalized command a wire name
/// represents, per the suffix scheme `operation_context::command_name`
/// appends. Families with no stage/finalize split (`Commit`, `RebuildRaid`,
/// `StreamDisk`, `Other`) don't carry a `Phase` at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// No `--allowed-operations` restriction: both stage and finalize ran
    /// as one command (or the command never has a stage/finalize split).
    OneShot,
    /// `--allowed-operations stage` only.
    Stage,
    /// `--allowed-operations finalize` only: whatever the stage phase
    /// creates (datastore, servicing ID) already exists by now.
    Finalize,
    /// `--allowed-operations` with neither stage nor finalize: a no-op.
    Noop,
}

impl Phase {
    /// The wire-name suffix for this phase -- the single source of truth
    /// both `strip` (parse a suffix back out of a rendered name) and
    /// [`CommandKind::daemon_phased`] (render a name for a compile-time-
    /// fixed daemon command) build on, so the two can never drift apart
    /// the way independently-hardcoded `"_stage"`/`"_finalize"` literals
    /// could.
    fn suffix(self) -> &'static str {
        match self {
            Phase::OneShot => "",
            Phase::Stage => "_stage",
            Phase::Finalize => "_finalize",
            Phase::Noop => "_noop",
        }
    }

    /// Splits a wire name into its base command name and phase, by
    /// stripping whichever non-empty `suffix()` matches (checked before
    /// falling back to `OneShot`'s empty suffix, which would otherwise
    /// match trivially). The inverse of `phased_name`.
    fn strip(name: &str) -> (&str, Phase) {
        for phase in [Phase::Stage, Phase::Finalize, Phase::Noop] {
            if let Some(base) = name.strip_suffix(phase.suffix()) {
                return (base, phase);
            }
        }
        (name, Phase::OneShot)
    }

    /// True for the phase(s) in which the command actually creates fresh
    /// state (a datastore, a servicing ID) rather than continuing state
    /// created by an earlier stage, or doing nothing at all.
    fn creates_new_state(self) -> bool {
        matches!(self, Phase::OneShot | Phase::Stage)
    }
}

/// The semantic family a command belongs to. This -- not the wire-name
/// string -- is what every predicate method on [`CommandKind`] matches
/// against, so string rendering and classification logic can never drift
/// against each other the way the independent name-based classifiers used
/// to. Families that can be staged/finalized carry their [`Phase`]
/// directly, rather than requiring every caller that already has a typed
/// `Family` to re-parse it back out of the rendered name string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Install(Phase),
    Update(Phase),
    /// `check == true` is a read-only dry run (`rollback --check`): it
    /// never stages or finalizes anything, and is excluded from
    /// `is_servicing` and from the stage/finalize wire-name suffix,
    /// matching today's behavior exactly. `phase` is meaningless when
    /// `check` is `true` (no predicate below inspects it in that case) and
    /// is always `Phase::OneShot` there.
    Rollback {
        check: bool,
        phase: Phase,
    },
    Commit,
    RebuildRaid,
    StreamDisk,
    /// Every other command (Get, Validate, Diagnose, OfflineInitialize,
    /// StartNetwork, Daemon, Listen, Version): none of these are
    /// "servicing" for `command_error` purposes.
    Other,
}

/// A fully-classified command. See the module-level docs for how this is
/// constructed and why it replaces the previous independent classifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandKind {
    /// Wire-format name for `command`/`operation_id` telemetry, already
    /// including the stage/finalize suffix where applicable. Byte-
    /// identical to what the pre-refactor classifiers produced -- Kusto
    /// queries depend on it.
    name: String,
    family: Family,
}

/// Wire-name literals shared by every place that renders one of these
/// families' names: the daemon-only named constructors below, and
/// `Commands::kind`/`ClientCommands::kind` further down (which pass these
/// as the `base` given to `phased_name`, instead of independently
/// deriving a name from `Commands::name()`/`ClientCommands::name()` --
/// those return the *kebab-case, user-facing* command name, e.g.
/// `"rebuild-raid"`, which is a distinct, Display-only concern from the
/// snake_case wire name Kusto queries depend on). Each literal below
/// exists exactly once in the crate; every other reference to a command's
/// wire name flows through one of these constants or the constructors
/// built on them.
const INSTALL_NAME: &str = "install";
const UPDATE_NAME: &str = "update";
const ROLLBACK_NAME: &str = "rollback";

impl CommandKind {
    fn new(name: String, family: Family) -> Self {
        Self { name, family }
    }

    pub fn as_str(&self) -> &str {
        &self.name
    }

    /// Replaces `is_servicing_client_command` and the CLI's own read-only
    /// exclusions for `command_error` eligibility.
    pub fn is_servicing(&self) -> bool {
        matches!(
            self.family,
            Family::Install(_)
                | Family::Update(_)
                | Family::Commit
                | Family::RebuildRaid
                | Family::StreamDisk
                | Family::Rollback { check: false, .. }
        )
    }

    /// Replaces `DataStore::may_initialize_datastore_for_command` for
    /// callers that already have a typed `CommandKind` (e.g. the daemon's
    /// service handlers, once constructed via the named constructors
    /// below).
    pub fn may_initialize_datastore(&self) -> bool {
        may_initialize_datastore_for_family(&self.family)
    }

    /// Replaces `command_generates_servicing_id` for callers that already
    /// have a typed `CommandKind`.
    pub fn generates_servicing_id(&self) -> bool {
        generates_servicing_id_for_family(&self.family)
    }

    /// Constructs an arbitrary, non-servicing `CommandKind` for tests
    /// elsewhere in the crate that need *some* `CommandKind` value and
    /// don't care about its classification (e.g. exercising
    /// `logging::operation_context`'s plumbing rather than
    /// `CommandKind`'s own predicates). Not part of the public API: real
    /// callers always go through a typed entry point or a named daemon
    /// constructor, never an arbitrary string.
    #[cfg(test)]
    pub(crate) fn for_test(name: &str) -> Self {
        Self::new(name.to_string(), Family::Other)
    }

    /// Shared constructor for the daemon-only named constructors below:
    /// each gRPC service handler represents a compile-time-fixed
    /// `(name, Family)` pair (unlike the CLI, which derives it at runtime
    /// from `--allowed-operations`), so there's no parsing to do -- just a
    /// literal to hardcode, once, here.
    fn daemon(name: String, family: Family) -> Self {
        Self::new(name, family)
    }

    /// Shared constructor for the daemon-only stage/finalize command
    /// families (`install`/`update`/`rollback`): derives the wire name
    /// from `base` and `phase.suffix()` -- the same suffixes `strip`
    /// parses back out -- so the base name is hardcoded exactly once per
    /// family instead of once per phase, and can never render a name that
    /// disagrees with its own classification.
    fn daemon_phased(base: &str, phase: Phase, family: impl FnOnce(Phase) -> Family) -> Self {
        Self::daemon(format!("{base}{}", phase.suffix()), family(phase))
    }

    pub fn install() -> Self {
        Self::daemon_phased(INSTALL_NAME, Phase::OneShot, Family::Install)
    }

    pub fn install_stage() -> Self {
        Self::daemon_phased(INSTALL_NAME, Phase::Stage, Family::Install)
    }

    pub fn install_finalize() -> Self {
        Self::daemon_phased(INSTALL_NAME, Phase::Finalize, Family::Install)
    }

    pub fn update() -> Self {
        Self::daemon_phased(UPDATE_NAME, Phase::OneShot, Family::Update)
    }

    pub fn update_stage() -> Self {
        Self::daemon_phased(UPDATE_NAME, Phase::Stage, Family::Update)
    }

    pub fn update_finalize() -> Self {
        Self::daemon_phased(UPDATE_NAME, Phase::Finalize, Family::Update)
    }

    pub fn rollback() -> Self {
        Self::daemon_phased(ROLLBACK_NAME, Phase::OneShot, |phase| Family::Rollback {
            check: false,
            phase,
        })
    }

    pub fn rollback_stage() -> Self {
        Self::daemon_phased(ROLLBACK_NAME, Phase::Stage, |phase| Family::Rollback {
            check: false,
            phase,
        })
    }

    pub fn rollback_finalize() -> Self {
        Self::daemon_phased(ROLLBACK_NAME, Phase::Finalize, |phase| Family::Rollback {
            check: false,
            phase,
        })
    }

    /// `rollback --check`: a read-only dry run that never stages or
    /// finalizes anything (see [`Family::Rollback`]'s `check` field).
    /// Shares [`ROLLBACK_NAME`] with the other `rollback*` constructors
    /// above so its wire name can never drift from theirs, even though
    /// its classification (`check: true`) is entirely different.
    pub fn rollback_check() -> Self {
        Self::daemon_phased(ROLLBACK_NAME, Phase::OneShot, |phase| Family::Rollback {
            check: true,
            phase,
        })
    }

    pub fn commit() -> Self {
        Self::daemon("commit".to_string(), Family::Commit)
    }

    /// `check_root` is preview-only and not yet implemented server-side;
    /// classified as `Family::Other` (not servicing) to match its stub
    /// behavior today.
    pub fn check_root() -> Self {
        Self::daemon("check_root".to_string(), Family::Other)
    }

    pub fn rebuild_raid() -> Self {
        Self::daemon("rebuild_raid".to_string(), Family::RebuildRaid)
    }

    pub fn stream_disk() -> Self {
        Self::daemon("stream_disk".to_string(), Family::StreamDisk)
    }
}

/// The one true implementation behind [`CommandKind::may_initialize_datastore`].
fn may_initialize_datastore_for_family(family: &Family) -> bool {
    match family {
        Family::Install(phase) | Family::Update(phase) => phase.creates_new_state(),
        Family::StreamDisk => true,
        _ => false,
    }
}

/// The one true implementation behind [`CommandKind::generates_servicing_id`].
/// Deliberately NOT the same family set as `may_initialize_datastore_for_family`:
/// a non-check rollback generates its own servicing ID but can never
/// initialize a fresh datastore.
fn generates_servicing_id_for_family(family: &Family) -> bool {
    match family {
        Family::Install(phase) | Family::Update(phase) => phase.creates_new_state(),
        Family::Rollback {
            check: false,
            phase,
        } => phase.creates_new_state(),
        Family::StreamDisk => true,
        _ => false,
    }
}

/// Shared stage/finalize wire-name derivation, matching
/// `logging::operation_context::command_name` exactly (that function
/// remains the single implementation; this just calls it).
fn phased_name(base: &str, ops: &Operations) -> String {
    command_name(base, ops)
}

impl Commands {
    /// True if invoking this command directly (i.e. as the `trident`
    /// binary itself, not `grpc-client`) owns the local metrics file's
    /// lifecycle for a fresh run and should truncate it on startup rather
    /// than append. See `main.rs::setup_tracing`, which is the sole
    /// caller, for why append-vs-truncate matters there.
    ///
    /// Deliberately matched directly on `Commands`, not derived from
    /// `kind()`/`Family`/`is_servicing()`: those intentionally collapse
    /// `Commands::GrpcClient(_)` down to the wrapped subcommand's own
    /// family (so a CLI `install` and a `grpc-client install` render
    /// identical telemetry names), but this question needs the opposite
    /// distinction -- a `grpc-client install` must always answer `false`
    /// here (the daemon it talks to owns the file, not the short-lived
    /// grpc-client process), even though `Commands::Install`'s answer is
    /// `true`. `Commands::Daemon` is the mirror case: `is_servicing()` is
    /// `false` for it (it never emits its own `command_error`), but it
    /// still owns the file for the run it's about to service, so this
    /// answers `true`.
    pub fn owns_local_metrics_file(&self) -> bool {
        !matches!(
            self,
            Commands::GrpcClient(_)
                | Commands::Diagnose { .. }
                | Commands::Validate { .. }
                | Commands::Get { .. }
                | Commands::OfflineInitialize { .. }
                | Commands::StartNetwork { .. }
                | Commands::Rollback { check: true, .. }
        )
    }

    /// Entry point for CLI command classification. Derives `Phase` by
    /// parsing the already-rendered wire name (`Phase::strip`) rather than
    /// re-deriving it independently from `Operations` -- guaranteeing the
    /// phase can never drift from the suffix that's actually in the wire
    /// name, since it's read straight out of that same string. Uses the
    /// shared `*_NAME` constants (not `self.name()`, which renders the
    /// kebab-case Display name) as the `base` passed to `phased_name`, and
    /// delegates to the daemon-only named constructors for the families
    /// with no stage/finalize split, so a command's wire name and
    /// `Family` are never independently duplicated between the CLI and
    /// daemon sides.
    pub fn kind(&self) -> CommandKind {
        match self {
            Commands::Install {
                allowed_operations, ..
            } => {
                let name = phased_name(INSTALL_NAME, &to_operations(allowed_operations));
                let phase = Phase::strip(&name).1;
                CommandKind::new(name, Family::Install(phase))
            }
            Commands::Update {
                allowed_operations, ..
            } => {
                let name = phased_name(UPDATE_NAME, &to_operations(allowed_operations));
                let phase = Phase::strip(&name).1;
                CommandKind::new(name, Family::Update(phase))
            }
            Commands::Rollback {
                allowed_operations,
                check,
                ..
            } => {
                if *check {
                    CommandKind::rollback_check()
                } else {
                    let name = phased_name(ROLLBACK_NAME, &to_operations(allowed_operations));
                    let phase = Phase::strip(&name).1;
                    CommandKind::new(
                        name,
                        Family::Rollback {
                            check: false,
                            phase,
                        },
                    )
                }
            }
            Commands::Commit { .. } => CommandKind::commit(),
            Commands::RebuildRaid { .. } => CommandKind::rebuild_raid(),
            Commands::GrpcClient(args) => args.command.kind(),
            Commands::StartNetwork { .. }
            | Commands::Get { .. }
            | Commands::Diagnose { .. }
            | Commands::Validate { .. }
            | Commands::OfflineInitialize { .. }
            | Commands::Daemon { .. } => {
                CommandKind::new(self.name().replace('-', "_"), Family::Other)
            }
            #[cfg(feature = "pytest-generator")]
            Commands::Pytest => CommandKind::new(self.name().replace('-', "_"), Family::Other),
        }
    }
}

impl ClientCommands {
    /// Entry point for `grpc-client` command classification. Like
    /// `Commands::kind`, uses the shared `*_NAME` constants and the
    /// daemon-only named constructors for families with a fixed name,
    /// rather than `self.name()` (the kebab-case, `"client-"`-prefixed
    /// Display name -- irrelevant here). Only the catch-all `Family::Other`
    /// arm still needs a name derived from `self.name()`, so the
    /// `"client-"` strip (see its own `// TODO: remove "client-" prefix`
    /// comment) is computed there, once, rather than for every arm.
    pub fn kind(&self) -> CommandKind {
        match self {
            ClientCommands::Install {
                allowed_operations, ..
            } => {
                let name = phased_name(INSTALL_NAME, &to_operations(allowed_operations));
                let phase = Phase::strip(&name).1;
                CommandKind::new(name, Family::Install(phase))
            }
            ClientCommands::Update {
                allowed_operations, ..
            } => {
                let name = phased_name(UPDATE_NAME, &to_operations(allowed_operations));
                let phase = Phase::strip(&name).1;
                CommandKind::new(name, Family::Update(phase))
            }
            ClientCommands::Rollback {
                allowed_operations,
                check,
                ..
            } => {
                if *check {
                    CommandKind::rollback_check()
                } else {
                    let name = phased_name(ROLLBACK_NAME, &to_operations(allowed_operations));
                    let phase = Phase::strip(&name).1;
                    CommandKind::new(
                        name,
                        Family::Rollback {
                            check: false,
                            phase,
                        },
                    )
                }
            }
            ClientCommands::Commit => CommandKind::commit(),
            ClientCommands::RebuildRaid { .. } => CommandKind::rebuild_raid(),
            ClientCommands::StreamDisk { .. } => CommandKind::stream_disk(),
            ClientCommands::Listen { .. }
            | ClientCommands::StartNetwork { .. }
            | ClientCommands::Get { .. }
            | ClientCommands::Validate { .. }
            | ClientCommands::Version => {
                let base = self.name().trim_start_matches("client-");
                CommandKind::new(base.replace('-', "_"), Family::Other)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, ClientArgs};
    use clap::Parser;

    /// `ClientArgs` derives `clap::Args`, not `Parser` -- it's only ever
    /// parsed as a nested subcommand of the real `Cli`/`Commands::GrpcClient`.
    /// This thin wrapper gives the test helper below a standalone
    /// `Parser` entry point without changing production code.
    #[derive(Parser, Debug)]
    struct TestClientArgs {
        #[clap(flatten)]
        inner: ClientArgs,
    }

    /// Parity test: for every CLI/grpc-client subcommand this crate
    /// supports, `CommandKind` must classify exactly as the pre-refactor
    /// classifiers did.
    fn cli(args: &[&str]) -> Commands {
        let mut full = vec!["trident"];
        full.extend_from_slice(args);
        Cli::parse_from(full).command
    }

    fn client(args: &[&str]) -> ClientCommands {
        let mut full = vec!["trident-client"];
        full.extend_from_slice(args);
        TestClientArgs::parse_from(full).inner.command
    }

    /// `INSTALL_NAME`/`UPDATE_NAME`/`ROLLBACK_NAME` are the base names
    /// `Commands::kind`/`ClientCommands::kind` pass to `phased_name`, and
    /// what the daemon-only named constructors hardcode -- but
    /// `Commands::name`/`ClientCommands::name` (used for CLI `Display`
    /// output, `owns_local_metrics_file`, etc.) hardcode their own,
    /// entirely independent `"install"`/`"client-install"` literals in
    /// `cli/mod.rs`/`cli/grpc_client.rs`. Nothing in the type system ties
    /// the two together, so this test is the only thing that would catch
    /// someone renaming one without the other.
    #[test]
    fn shared_name_constants_match_commands_name() {
        let install = cli(&["install", "--allowed-operations", "stage,finalize"]);
        let update = cli(&["update", "--allowed-operations", "stage,finalize"]);
        let rollback = cli(&["rollback", "--allowed-operations", "stage,finalize"]);
        assert_eq!(install.name(), INSTALL_NAME);
        assert_eq!(update.name(), UPDATE_NAME);
        assert_eq!(rollback.name(), ROLLBACK_NAME);

        // `ClientCommands::name()` always carries a `"client-"` prefix
        // (see its own `// TODO: remove "client-" prefix` comment); strip
        // it the same way `ClientCommands::kind`'s catch-all arm does
        // before comparing against the unprefixed shared constants.
        let client_install = client(&["install", "--allowed-operations", "stage,finalize"]);
        let client_update = client(&["update", "--allowed-operations", "stage,finalize"]);
        let client_rollback = client(&["rollback", "--allowed-operations", "stage,finalize"]);
        assert_eq!(
            client_install.name().trim_start_matches("client-"),
            INSTALL_NAME
        );
        assert_eq!(
            client_update.name().trim_start_matches("client-"),
            UPDATE_NAME
        );
        assert_eq!(
            client_rollback.name().trim_start_matches("client-"),
            ROLLBACK_NAME
        );
    }

    #[test]
    fn install_update_rollback_phase_suffixes() {
        assert_eq!(
            cli(&["install", "--allowed-operations", "stage,finalize"])
                .kind()
                .as_str(),
            "install"
        );
        assert_eq!(
            cli(&["install", "--allowed-operations", "stage"])
                .kind()
                .as_str(),
            "install_stage"
        );
        assert_eq!(
            cli(&["install", "--allowed-operations", "finalize"])
                .kind()
                .as_str(),
            "install_finalize"
        );
        assert_eq!(
            cli(&["install", "--allowed-operations"]).kind().as_str(),
            "install_noop"
        );
        assert_eq!(
            cli(&["update", "--allowed-operations", "stage"])
                .kind()
                .as_str(),
            "update_stage"
        );
        assert_eq!(
            cli(&["rollback", "--allowed-operations", "stage"])
                .kind()
                .as_str(),
            "rollback_stage"
        );
    }

    #[test]
    fn rollback_check_ignores_phase_suffix() {
        assert_eq!(
            cli(&["rollback", "--check", "--allowed-operations", "stage"])
                .kind()
                .as_str(),
            "rollback"
        );
    }

    #[test]
    fn non_phase_commands_use_bare_name() {
        assert_eq!(cli(&["commit"]).kind().as_str(), "commit");
        assert_eq!(cli(&["rebuild-raid"]).kind().as_str(), "rebuild_raid");
        assert_eq!(cli(&["get"]).kind().as_str(), "get");
        assert_eq!(cli(&["validate"]).kind().as_str(), "validate");
        assert_eq!(cli(&["start-network"]).kind().as_str(), "start_network");
    }

    #[test]
    fn grpc_client_strips_prefix_and_matches_cli_naming() {
        assert_eq!(
            client(&["install", "--allowed-operations", "stage"])
                .kind()
                .as_str(),
            "install_stage"
        );
        assert_eq!(
            client(&["stream-disk", "http://x/img"]).kind().as_str(),
            "stream_disk"
        );
        assert_eq!(client(&["commit"]).kind().as_str(), "commit");
        assert_eq!(client(&["rebuild-raid"]).kind().as_str(), "rebuild_raid");
        assert_eq!(
            client(&["rollback", "--check", "--allowed-operations", "stage"])
                .kind()
                .as_str(),
            "rollback"
        );
    }

    #[test]
    fn is_servicing_excludes_check_rollback_and_read_only_commands() {
        assert!(cli(&["install", "--allowed-operations", "stage"])
            .kind()
            .is_servicing());
        assert!(cli(&["rollback", "--allowed-operations", "stage"])
            .kind()
            .is_servicing());
        assert!(
            !cli(&["rollback", "--check", "--allowed-operations", "stage"])
                .kind()
                .is_servicing()
        );
        assert!(!cli(&["get"]).kind().is_servicing());
        assert!(!cli(&["validate"]).kind().is_servicing());
    }

    #[test]
    fn owns_local_metrics_file_matches_truncate_set() {
        // Owns (truncate): a fresh servicing run, or the daemon that will
        // service one.
        assert!(
            cli(&["install", "--allowed-operations", "stage,finalize"]).owns_local_metrics_file()
        );
        assert!(
            cli(&["update", "--allowed-operations", "stage,finalize"]).owns_local_metrics_file()
        );
        assert!(cli(&["commit"]).owns_local_metrics_file());
        assert!(cli(&["rebuild-raid"]).owns_local_metrics_file());
        assert!(
            cli(&["rollback", "--allowed-operations", "stage,finalize"]).owns_local_metrics_file()
        );
        assert!(cli(&["daemon"]).owns_local_metrics_file());

        // Does not own (append): read-only/fast commands, a checked
        // rollback, and every grpc-client subcommand -- including ones
        // that otherwise start a servicing run, since the daemon they
        // talk to is the file's real owner.
        assert!(!cli(&["validate"]).owns_local_metrics_file());
        assert!(!cli(&["get"]).owns_local_metrics_file());
        assert!(!cli(&["diagnose", "--output", "/tmp/out.json"]).owns_local_metrics_file());
        assert!(!cli(&["start-network"]).owns_local_metrics_file());
        assert!(
            !cli(&["rollback", "--check", "--allowed-operations", "stage"])
                .owns_local_metrics_file()
        );
        assert!(
            !cli(&["grpc-client", "install", "--allowed-operations", "stage"])
                .owns_local_metrics_file()
        );
        assert!(!cli(&["grpc-client", "get"]).owns_local_metrics_file());
    }

    #[test]
    fn phase_strip_round_trips_every_suffix() {
        assert_eq!(Phase::strip("install"), ("install", Phase::OneShot));
        assert_eq!(Phase::strip("install_stage"), ("install", Phase::Stage));
        assert_eq!(
            Phase::strip("install_finalize"),
            ("install", Phase::Finalize)
        );
        assert_eq!(Phase::strip("install_noop"), ("install", Phase::Noop));
    }

    /// Locks in the exact classification of every daemon named
    /// constructor -- name, `is_servicing`, `may_initialize_datastore`,
    /// and `generates_servicing_id` -- so a future edit to `Family`'s
    /// match arms is caught by CI rather than only noticed in a running
    /// system. Formerly split across two independent test suites (this
    /// one, and `logging::tracestream`'s now-removed
    /// `command_generates_servicing_id` lock-in test); consolidated here
    /// since both were locking the same underlying `*_for_family` logic.
    #[test]
    fn daemon_constructors_match_expected_names_and_classification() {
        assert_eq!(CommandKind::install().as_str(), "install");
        assert!(CommandKind::install().is_servicing());
        assert!(CommandKind::install().may_initialize_datastore());
        assert!(CommandKind::install().generates_servicing_id());

        assert_eq!(CommandKind::install_stage().as_str(), "install_stage");
        assert!(CommandKind::install_stage().is_servicing());
        assert!(CommandKind::install_stage().may_initialize_datastore());
        assert!(CommandKind::install_stage().generates_servicing_id());

        assert_eq!(CommandKind::install_finalize().as_str(), "install_finalize");
        assert!(CommandKind::install_finalize().is_servicing());
        assert!(!CommandKind::install_finalize().may_initialize_datastore());
        assert!(!CommandKind::install_finalize().generates_servicing_id());

        assert_eq!(CommandKind::update().as_str(), "update");
        assert!(CommandKind::update().is_servicing());
        assert!(CommandKind::update().may_initialize_datastore());
        assert!(CommandKind::update().generates_servicing_id());

        assert_eq!(CommandKind::update_stage().as_str(), "update_stage");
        assert!(CommandKind::update_stage().is_servicing());
        assert!(CommandKind::update_stage().may_initialize_datastore());
        assert!(CommandKind::update_stage().generates_servicing_id());

        assert_eq!(CommandKind::update_finalize().as_str(), "update_finalize");
        assert!(CommandKind::update_finalize().is_servicing());
        assert!(!CommandKind::update_finalize().may_initialize_datastore());
        assert!(!CommandKind::update_finalize().generates_servicing_id());

        assert_eq!(CommandKind::rollback().as_str(), "rollback");
        assert!(CommandKind::rollback().is_servicing());
        assert!(!CommandKind::rollback().may_initialize_datastore());
        assert!(CommandKind::rollback().generates_servicing_id());

        assert_eq!(CommandKind::rollback_stage().as_str(), "rollback_stage");
        assert!(CommandKind::rollback_stage().is_servicing());
        assert!(!CommandKind::rollback_stage().may_initialize_datastore());
        assert!(CommandKind::rollback_stage().generates_servicing_id());

        assert_eq!(
            CommandKind::rollback_finalize().as_str(),
            "rollback_finalize"
        );
        assert!(CommandKind::rollback_finalize().is_servicing());
        assert!(!CommandKind::rollback_finalize().may_initialize_datastore());
        assert!(!CommandKind::rollback_finalize().generates_servicing_id());

        assert_eq!(CommandKind::commit().as_str(), "commit");
        assert!(CommandKind::commit().is_servicing());
        assert!(!CommandKind::commit().may_initialize_datastore());
        assert!(!CommandKind::commit().generates_servicing_id());

        assert_eq!(CommandKind::check_root().as_str(), "check_root");
        assert!(!CommandKind::check_root().is_servicing());
        assert!(!CommandKind::check_root().may_initialize_datastore());
        assert!(!CommandKind::check_root().generates_servicing_id());

        assert_eq!(CommandKind::rebuild_raid().as_str(), "rebuild_raid");
        assert!(CommandKind::rebuild_raid().is_servicing());
        assert!(!CommandKind::rebuild_raid().may_initialize_datastore());
        assert!(!CommandKind::rebuild_raid().generates_servicing_id());

        assert_eq!(CommandKind::stream_disk().as_str(), "stream_disk");
        assert!(CommandKind::stream_disk().is_servicing());
        assert!(CommandKind::stream_disk().may_initialize_datastore());
        assert!(CommandKind::stream_disk().generates_servicing_id());
    }
}
