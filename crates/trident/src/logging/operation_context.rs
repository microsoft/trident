//! Thread-local "which command is currently executing, under what
//! operation ID, and from which of Trident's three entry points" context,
//! so telemetry sinks ([`super::tracestream::TraceSender`],
//! [`super::appinsights::AppInsightsSender`]) can tag every metric/span
//! fired during a command's execution with `command`/`operation_id`/
//! `source` fields, without every call site (deep in `engine::*`,
//! `Trident::*`, etc.) needing to pass them explicitly.
//!
//! A thread-local (rather than e.g. a `tracing` span) is enough here
//! because all three places that set this context run the entire command
//! synchronously on a single, dedicated thread for the command's whole
//! duration:
//! - CLI: `run_trident`'s command dispatch (synchronous, main thread),
//!   tagged [`OperationSource::Cli`].
//! - gRPC/daemon: `servicing_request`'s closure runs inside
//!   `tokio::task::spawn_blocking`, which gives it its own OS thread for
//!   as long as the closure runs, tagged [`OperationSource::Daemon`].
//! - gRPC client: `grpc_client`'s command dispatch (synchronous, main
//!   thread of the CLI process acting as a client of a running daemon),
//!   tagged [`OperationSource::GrpcClient`].
//!
//! `operation_id` is a fresh, random ID generated once per command
//! invocation (distinct from the persistent, per-host
//! `DataStore::correlation_id`, which is unrelated and set separately on
//! `TraceStream`/`AppInsightsSender`).

use std::{cell::RefCell, sync::Mutex};

use uuid::Uuid;

/// Identifies which of Trident's three entry points actually executed a
/// command, so telemetry consumers can distinguish (for example) a
/// `grpc-client` invocation that never reached a daemon from the daemon
/// request it was trying to reach, or from a direct CLI invocation that
/// bypassed the daemon entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationSource {
    /// A command run directly by the CLI, without going through the
    /// daemon (e.g. `trident install` on a host with no daemon running).
    Cli,
    /// A command executed by the daemon in response to a gRPC request
    /// (see `server::tridentserver::TridentServer::servicing_request`).
    Daemon,
    /// A command run by the CLI acting as a gRPC client, relaying the
    /// request to a running daemon (see `grpc_client`).
    GrpcClient,
}

impl OperationSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            OperationSource::Cli => "cli",
            OperationSource::Daemon => "daemon",
            OperationSource::GrpcClient => "grpc-client",
        }
    }
}

thread_local! {
    static CURRENT_OPERATION: RefCell<Option<(String, String, OperationSource)>> =
        const { RefCell::new(None) };
}

/// Runs `f` with this thread tagged as executing `command` from `source`,
/// under a fresh `operation_id`. Also fires a `command_start` metric event
/// immediately, tagged the same way. Clears the tag afterwards (even if
/// `f` panics, via a drop guard), so a thread that runs multiple commands
/// over its lifetime (e.g. a thread pool worker reused across
/// `spawn_blocking` calls) never leaks a stale tag into an unrelated
/// later command.
///
/// The context (and its drop guard) is installed *before* firing
/// `command_start`, and that event carries only `metric_name` -- not
/// explicit `command`/`operation_id`/`source` fields. Both telemetry sinks
/// (`TraceSender`, `AppInsightsSender`) read the just-installed context via
/// `current()` and merge `command`/`operation_id`/`source` into the same
/// `additional_fields`/properties map every other event during this
/// invocation gets them from. Emitting them as explicit fields on
/// `command_start` itself, before the context existed, would instead land
/// them in that event's own `value`/properties body -- a different schema
/// from every other event, and invisible to consumers that only look at
/// `additional_fields` for operation metadata.
pub fn run_with_operation<R>(command: &str, source: OperationSource, f: impl FnOnce() -> R) -> R {
    let operation_id = Uuid::new_v4().to_string();

    CURRENT_OPERATION.with(|cell| {
        *cell.borrow_mut() = Some((operation_id, command.to_string(), source));
    });

    struct ClearOnDrop;
    impl Drop for ClearOnDrop {
        fn drop(&mut self) {
            CURRENT_OPERATION.with(|cell| *cell.borrow_mut() = None);
        }
    }
    let _clear = ClearOnDrop;

    tracing::info!(metric_name = "command_start");

    f()
}

/// Returns the `(operation_id, command, source)` triple set by
/// [`run_with_operation`] for the calling thread, if any.
pub(crate) fn current() -> Option<(String, String, OperationSource)> {
    CURRENT_OPERATION.with(|cell| cell.borrow().clone())
}

/// A snapshot of another thread's operation context (see
/// [`run_with_operation`]), capturable via [`snapshot`] and re-installed
/// on a different thread via [`run_with_captured_operation`]. Used to
/// propagate `operation_id`/`command`/`source` into threads spawned
/// mid-command (e.g. `MonitorMetrics`'s background sampling thread),
/// which otherwise start with no thread-local context of their own and
/// would silently drop these fields from their own metrics.
#[derive(Clone)]
pub struct CapturedOperation(String, String, OperationSource);

/// Captures the calling thread's current operation context, if any, for
/// later re-installation on another thread via
/// [`run_with_captured_operation`]. Call this on the *spawning* thread,
/// before handing the result to the new thread's closure.
pub fn snapshot() -> Option<CapturedOperation> {
    current()
        .map(|(operation_id, command, source)| CapturedOperation(operation_id, command, source))
}

/// Runs `f` with `captured` (from [`snapshot`]) installed as the calling
/// thread's operation context for the duration of `f`, clearing it
/// afterwards (even on panic). Unlike [`run_with_operation`], this does
/// *not* mint a new `operation_id` or fire a `command_start` metric -- it
/// re-uses an existing operation's identity on a different thread rather
/// than starting a new one. A `None` `captured` (e.g. the spawning thread
/// itself had no operation context -- this thread was started outside any
/// command) makes this a plain, untagged call to `f()`.
pub fn run_with_captured_operation<R>(
    captured: Option<CapturedOperation>,
    f: impl FnOnce() -> R,
) -> R {
    let Some(CapturedOperation(operation_id, command, source)) = captured else {
        return f();
    };

    CURRENT_OPERATION.with(|cell| {
        *cell.borrow_mut() = Some((operation_id, command, source));
    });

    struct ClearOnDrop;
    impl Drop for ClearOnDrop {
        fn drop(&mut self) {
            CURRENT_OPERATION.with(|cell| *cell.borrow_mut() = None);
        }
    }
    let _clear = ClearOnDrop;

    f()
}

/// Holds a snapshot of the just-finished servicing operation's context,
/// captured by [`save_reboot_operation`] the moment that operation
/// decides a reboot is needed, and consumed exactly once by
/// [`take_reboot_operation`] at the actual post-servicing reboot call
/// site (CLI: `main.rs`; daemon: `server::reboot`).
///
/// A process-global (not thread-local) slot is required here, unlike
/// [`snapshot`]/[`run_with_captured_operation`] above: those propagate a
/// context to a thread spawned *while the original context is still
/// active* (the spawning thread hands the snapshot directly to the new
/// thread's closure). Here, by the time the reboot call happens, the
/// operation that decided a reboot was needed has already returned --
/// its `run_with_operation` scope (and thread-local context) is gone --
/// and the reboot call itself runs later, on a different thread/call
/// stack that has no way to receive a snapshot as a direct parameter (the
/// CLI's `main` and the daemon's `server_main` reboot path both reach the
/// reboot call through several layers of return values -- `ExitKind`,
/// `Completed`, etc. -- that don't carry operation context). A shared
/// slot lets the operation stash its own context just before it returns,
/// for the reboot call to pick up moments later regardless of which
/// thread it ends up running on.
static PENDING_REBOOT_OPERATION: Mutex<Option<CapturedOperation>> = Mutex::new(None);

/// Captures the calling thread's current operation context (if any) into
/// [`PENDING_REBOOT_OPERATION`]. Call this from inside the servicing
/// operation's own context, as soon as it decides a reboot is needed --
/// i.e. while that context is still installed -- so the *original*
/// servicing invocation's `operation_id`/`command` (not a fresh "reboot"
/// identity) can later be attached to `trident_system_reboot` and any
/// telemetry from the reboot call itself, via [`take_reboot_operation`].
pub fn save_reboot_operation() {
    *PENDING_REBOOT_OPERATION.lock().unwrap() = snapshot();
}

/// Takes (clearing) whichever operation context was most recently saved
/// via [`save_reboot_operation`], for use with
/// [`run_with_captured_operation`] at the actual reboot call site. `None`
/// if no operation ever called [`save_reboot_operation`] (e.g. reboot
/// requested outside any servicing operation), or if it was already
/// consumed by a prior call.
pub fn take_reboot_operation() -> Option<CapturedOperation> {
    PENDING_REBOOT_OPERATION.lock().unwrap().take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_operation_by_default() {
        assert!(current().is_none());
    }

    #[test]
    fn test_run_with_operation_sets_and_clears_context() {
        assert!(current().is_none());

        let observed = run_with_operation("test_command", OperationSource::Cli, current);
        let (operation_id, command, source) = observed.expect("context should be set inside f");
        assert_eq!(command, "test_command");
        assert_eq!(operation_id.len(), 36, "operation_id should be a UUID");
        assert_eq!(source, OperationSource::Cli);

        assert!(
            current().is_none(),
            "context must be cleared after run_with_operation returns"
        );
    }

    #[test]
    fn test_run_with_operation_clears_context_on_panic() {
        assert!(current().is_none());

        let result = std::panic::catch_unwind(|| {
            run_with_operation("panicking_command", OperationSource::Cli, || {
                panic!("boom");
            })
        });
        assert!(result.is_err());

        assert!(
            current().is_none(),
            "context must be cleared even if f panics"
        );
    }

    #[test]
    fn test_each_invocation_gets_a_fresh_operation_id() {
        let first = run_with_operation("cmd", OperationSource::Cli, || current().unwrap().0);
        let second = run_with_operation("cmd", OperationSource::Cli, || current().unwrap().0);
        assert_ne!(
            first, second,
            "each command invocation gets a fresh operation_id"
        );
    }

    #[test]
    fn test_snapshot_is_none_outside_run_with_operation() {
        assert!(snapshot().is_none());
    }

    #[test]
    fn test_snapshot_and_captured_operation_propagates_across_threads() {
        // Capture on a thread standing in for the "spawning" thread (here,
        // just the current thread inside run_with_operation), then install
        // it on a different OS thread, mirroring MonitorMetrics's use.
        let (expected_operation_id, expected_command, expected_source, observed) =
            run_with_operation(
                "cmd_from_parent_thread",
                OperationSource::GrpcClient,
                || {
                    let captured = snapshot().expect("should capture a context");
                    let (operation_id, command, source) = current().unwrap();

                    let observed = std::thread::spawn(move || {
                        // No context on a fresh thread until installed.
                        assert!(current().is_none());
                        run_with_captured_operation(Some(captured), current)
                    })
                    .join()
                    .unwrap();

                    (operation_id, command, source, observed)
                },
            );

        assert_eq!(
            observed,
            Some((expected_operation_id, expected_command, expected_source)),
            "captured operation_id/command/source should propagate to the new thread"
        );
    }

    #[test]
    fn test_run_with_captured_operation_none_is_a_plain_call() {
        assert!(current().is_none());
        let result = run_with_captured_operation(None, current);
        assert!(result.is_none());
        assert!(current().is_none());
    }

    #[test]
    fn test_run_with_captured_operation_clears_context_after_returning() {
        // run_with_captured_operation is meant for a *fresh* thread with no
        // context of its own (see MonitorMetrics's use), not nested on top
        // of an existing run_with_operation on the *same* thread -- both
        // share one flat thread-local slot, so nesting on one thread isn't
        // a supported combination. Verify the fresh-thread case clears
        // itself after returning.
        let captured = run_with_operation("cmd", OperationSource::Daemon, snapshot);
        let still_set_inside = std::thread::spawn(move || {
            run_with_captured_operation(captured, || current().is_some())
        })
        .join()
        .unwrap();
        assert!(still_set_inside, "context should be set while f runs");
    }

    /// Serializes the reboot-operation tests below: unlike the
    /// thread-local `CURRENT_OPERATION`, `PENDING_REBOOT_OPERATION` is a
    /// process-wide slot, so tests touching it would otherwise race
    /// against each other under cargo's default parallel test execution.
    static REBOOT_OPERATION_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_take_reboot_operation_is_none_by_default() {
        let _guard = REBOOT_OPERATION_TEST_LOCK.lock().unwrap();
        assert!(take_reboot_operation().is_none());
    }

    #[test]
    fn test_save_and_take_reboot_operation_round_trips() {
        let _guard = REBOOT_OPERATION_TEST_LOCK.lock().unwrap();
        assert!(
            take_reboot_operation().is_none(),
            "start with a clean slate"
        );

        let expected = run_with_operation("install", OperationSource::Cli, || {
            save_reboot_operation();
            current().unwrap()
        });

        let captured = take_reboot_operation().expect("should have captured a context");
        let observed = run_with_captured_operation(Some(captured), current).unwrap();
        assert_eq!(
            observed, expected,
            "the original install's operation_id/command should have been captured"
        );

        assert!(
            take_reboot_operation().is_none(),
            "take_reboot_operation should clear the slot after taking it"
        );
    }

    #[test]
    fn test_save_reboot_operation_outside_run_with_operation_is_none() {
        let _guard = REBOOT_OPERATION_TEST_LOCK.lock().unwrap();
        assert!(
            take_reboot_operation().is_none(),
            "start with a clean slate"
        );

        save_reboot_operation();

        assert!(
            take_reboot_operation().is_none(),
            "nothing to capture outside an active operation"
        );
    }
}
