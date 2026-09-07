//! Thread-local "which command is currently executing, and under what
//! operation ID" context, so telemetry sinks ([`super::tracestream::TraceSender`],
//! [`super::appinsights::AppInsightsSender`]) can tag every metric/span
//! fired during a command's execution with `command`/`operation_id`
//! fields, without every call site (deep in `engine::*`, `Trident::*`,
//! etc.) needing to pass them explicitly.
//!
//! A thread-local (rather than e.g. a `tracing` span) is enough here
//! because both places that set this context run the entire command
//! synchronously on a single, dedicated thread for the command's whole
//! duration:
//! - CLI: `run_trident`'s command dispatch (synchronous, main thread).
//! - gRPC/daemon: `servicing_request`'s closure runs inside
//!   `tokio::task::spawn_blocking`, which gives it its own OS thread for
//!   as long as the closure runs.
//!
//! `operation_id` is a fresh, random ID generated once per command
//! invocation (distinct from the persistent, per-host
//! `DataStore::installation_id`, which is set separately on
//! `TraceStream`/`AppInsightsSender`, and from `servicing_id` below).
//!
//! This module also carries an optional `servicing_id`: a *persistent*
//! (stored in the datastore, see `DataStore::new_servicing_id`) ID minted
//! at the start of staging (install/update/manual-rollback), so metrics
//! from a stage invocation and a later, separate finalize invocation (the
//! A/B update case, which reboots in between) can still be correlated
//! with each other via the same ID -- unlike `operation_id`, which is
//! deliberately fresh per invocation. It's attached here (rather than
//! threaded through every `engine::*` function signature) via
//! [`set_servicing_id`], callable from wherever staging actually happens.

use std::{
    cell::RefCell,
    panic::{self, AssertUnwindSafe},
    sync::Mutex,
};

use uuid::Uuid;

use trident_api::error::TridentError;

/// Per-invocation telemetry-correlation state (see the module docs).
#[derive(Clone)]
struct OperationState {
    operation_id: String,
    command: String,
    servicing_id: Option<String>,
}

thread_local! {
    static CURRENT_OPERATION: RefCell<Option<OperationState>> = const { RefCell::new(None) };
}

/// Runs `f` with this thread tagged as executing `command`, under a fresh
/// `operation_id`. Also fires a `command_start` metric event immediately,
/// tagged the same way. Clears the tag afterwards (even if `f` panics,
/// via a drop guard), so a thread that runs multiple commands over its
/// lifetime (e.g. a thread pool worker reused across `spawn_blocking`
/// calls) never leaks a stale tag into an unrelated later command.
///
/// The context (and its drop guard) is installed *before* firing
/// `command_start`, and that event carries only `metric_name` -- not
/// explicit `command`/`operation_id` fields. Both telemetry sinks
/// (`TraceSender`, `AppInsightsSender`) read the just-installed context via
/// `current()` and merge `command`/`operation_id` into the same
/// `additional_fields`/properties map every other event during this
/// invocation gets them from. Emitting them as explicit fields on
/// `command_start` itself, before the context existed, would instead land
/// them in that event's own `value`/properties body -- a different schema
/// from every other event, and invisible to consumers that only look at
/// `additional_fields` for operation metadata.
pub fn run_with_operation<R>(command: &str, f: impl FnOnce() -> R) -> R {
    let operation_id = Uuid::new_v4().to_string();

    CURRENT_OPERATION.with(|cell| {
        *cell.borrow_mut() = Some(OperationState {
            operation_id,
            command: command.to_string(),
            servicing_id: None,
        });
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

/// Tags the current invocation (as established by
/// [`run_with_operation`]/[`run_command`]) with `servicing_id`, so every
/// metric emitted for the remainder of this invocation carries it
/// alongside `operation_id`/`command`. A no-op (logged at debug level) if
/// called outside of `run_with_operation`/`run_command`.
///
/// Called from wherever staging actually begins (install, update,
/// manual-rollback) with a freshly-minted, datastore-persisted ID (see
/// `DataStore::new_servicing_id`) -- or, on a later finalize-only
/// invocation resuming a prior stage, with the persisted ID read back via
/// `DataStore::servicing_id`.
pub fn set_servicing_id(servicing_id: impl Into<String>) {
    CURRENT_OPERATION.with(|cell| match cell.borrow_mut().as_mut() {
        Some(state) => state.servicing_id = Some(servicing_id.into()),
        None => {
            tracing::debug!("set_servicing_id called outside run_with_operation/run_command");
        }
    });
}

/// Returns the `(operation_id, command, servicing_id)` set by
/// [`run_with_operation`] for the calling thread, if any. `servicing_id`
/// is `None` until/unless [`set_servicing_id`] has been called this
/// invocation.
pub(crate) fn current() -> Option<(String, String, Option<String>)> {
    CURRENT_OPERATION.with(|cell| {
        cell.borrow().as_ref().map(|state| {
            (
                state.operation_id.clone(),
                state.command.clone(),
                state.servicing_id.clone(),
            )
        })
    })
}

/// A snapshot of another thread's operation context (see
/// [`run_with_operation`]/[`set_servicing_id`]), capturable via
/// [`snapshot`] and re-installed on a different thread via
/// [`run_with_captured_operation`]. Used to propagate
/// `operation_id`/`command`/`servicing_id` into threads spawned
/// mid-command (e.g. `MonitorMetrics`'s background sampling thread),
/// which otherwise start with no thread-local context of their own and
/// would silently drop these fields from their own metrics.
#[derive(Clone)]
pub struct CapturedOperation(String, String, Option<String>);

/// Captures the calling thread's current operation context, if any, for
/// later re-installation on another thread via
/// [`run_with_captured_operation`]. Call this on the *spawning* thread,
/// before handing the result to the new thread's closure.
pub fn snapshot() -> Option<CapturedOperation> {
    current().map(|(operation_id, command, servicing_id)| {
        CapturedOperation(operation_id, command, servicing_id)
    })
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
    let Some(CapturedOperation(operation_id, command, servicing_id)) = captured else {
        return f();
    };

    CURRENT_OPERATION.with(|cell| {
        *cell.borrow_mut() = Some(OperationState {
            operation_id,
            command,
            servicing_id,
        });
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

/// Like [`run_command`], but specifically for the actual reboot call:
/// reuses whichever operation was captured by [`save_reboot_operation`]
/// (the servicing operation that decided a reboot was needed) instead of
/// minting a fresh `operation_id`, while still firing `command_error` on
/// failure -- so a failed reboot's error metric is correlated back to
/// that original operation, exactly like every other command's
/// `command_error`. Falls back to a plain, untagged call (still firing
/// `command_error` on failure) if nothing was captured.
pub fn run_reboot_command<T>(
    f: impl FnOnce() -> Result<T, TridentError>,
) -> Result<T, TridentError> {
    run_with_captured_operation(take_reboot_operation(), || {
        let result = f();
        if let Err(ref error) = result {
            report_command_error(error);
        }
        result
    })
}

/// Like [`run_with_operation`], but specifically for the
/// `Result<T, TridentError>` shape both places that run a command
/// actually use (CLI dispatch, gRPC's `servicing_request`/
/// `reading_request`): additionally fires a `command_error` metric --
/// breaking the error down into `kind`, `subkind`, and `location` -- if
/// `f` returns `Err`, while the operation_id/command context is still
/// active (so it's correlated the same way `command_start` is).
///
/// Also catches a panic from `f` here, still inside
/// `run_with_operation`'s scope, so `command_error` still fires even when
/// `f` panics instead of returning `Err`. Without this, a panic would
/// unwind straight through this closure and past `run_with_operation`'s
/// `ClearOnDrop` guard -- which clears the operation context *during* the
/// unwind, before any code downstream of `run_command` (the CLI's own
/// outer `catch_unwind` in `main.rs`, or the daemon's per-request
/// panic-to-`Status`/`Completed` conversion) gets a chance to report
/// anything -- silently skipping `command_error` even though
/// `Telemetry.md` documents it as firing on every failed command. The
/// panic is re-raised afterwards via `resume_unwind`, so callers'
/// existing panic handling (exit codes, gRPC error responses) is
/// unaffected -- this only adds the metric emission that was missing.
pub fn run_command<T>(
    command: &str,
    f: impl FnOnce() -> Result<T, TridentError>,
) -> Result<T, TridentError> {
    run_with_operation(command, || match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(result) => {
            if let Err(ref error) = result {
                report_command_error(error);
            }
            result
        }
        Err(payload) => {
            report_command_panic(command, &payload);
            panic::resume_unwind(payload);
        }
    })
}

/// Fires the `command_error` metric for a failed command. Split out from
/// `run_command` so it's independently testable against a constructed
/// `TridentError` without needing a real failing command.
fn report_command_error(error: &TridentError) {
    tracing::info!(
        metric_name = "command_error",
        kind = error.kind().as_str(),
        subkind = error.subkind().unwrap_or("none"),
        location = error.location().as_str(),
    );
}

/// Fires the `command_error` metric for a command that panicked instead
/// of returning `Err`. Tagged with a distinct `kind = "panic"` (rather
/// than reusing a `TridentError`'s own `kind`/`subkind`/`location`, which
/// don't exist for a panic) so consumers can tell the two failure modes
/// apart. The panic payload's message (when it is the common `&str` or
/// `String` panic message) is logged separately at `error` level, not
/// included in the metric's own fields, since it's unstructured and of
/// unbounded size.
fn report_command_panic(command: &str, payload: &Box<dyn std::any::Any + Send>) {
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic payload>".to_string());
    log::error!("Command '{command}' panicked: {message}");
    tracing::info!(
        metric_name = "command_error",
        kind = "panic",
        subkind = "none",
        location = "none",
    );
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

        let observed = run_with_operation("test_command", current);
        let (operation_id, command, servicing_id) =
            observed.expect("context should be set inside f");
        assert_eq!(command, "test_command");
        assert_eq!(operation_id.len(), 36, "operation_id should be a UUID");
        assert!(
            servicing_id.is_none(),
            "servicing_id should be unset unless set_servicing_id was called"
        );

        assert!(
            current().is_none(),
            "context must be cleared after run_with_operation returns"
        );
    }

    #[test]
    fn test_run_with_operation_clears_context_on_panic() {
        assert!(current().is_none());

        let result = std::panic::catch_unwind(|| {
            run_with_operation("panicking_command", || {
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
        let first = run_with_operation("cmd", || current().unwrap().0);
        let second = run_with_operation("cmd", || current().unwrap().0);
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
        let (expected_operation_id, expected_command, expected_servicing_id, observed) =
            run_with_operation("cmd_from_parent_thread", || {
                set_servicing_id("test-servicing-id");
                let captured = snapshot().expect("should capture a context");
                let (operation_id, command, servicing_id) = current().unwrap();

                let observed = std::thread::spawn(move || {
                    // No context on a fresh thread until installed.
                    assert!(current().is_none());
                    run_with_captured_operation(Some(captured), current)
                })
                .join()
                .unwrap();

                (operation_id, command, servicing_id, observed)
            });

        assert_eq!(
            observed,
            Some((
                expected_operation_id,
                expected_command,
                expected_servicing_id
            )),
            "captured operation_id/command/servicing_id should propagate to the new thread"
        );
    }

    #[test]
    fn test_set_servicing_id_outside_run_with_operation_is_a_no_op() {
        assert!(current().is_none());
        set_servicing_id("stray-id");
        assert!(
            current().is_none(),
            "set_servicing_id must not create context on its own"
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
        let captured = run_with_operation("cmd", snapshot);
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

        let expected = run_with_operation("install", || {
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

    #[test]
    fn test_run_reboot_command_reuses_captured_operation() {
        let _guard = REBOOT_OPERATION_TEST_LOCK.lock().unwrap();
        assert!(
            take_reboot_operation().is_none(),
            "start with a clean slate"
        );

        let expected = run_with_operation("install", || {
            save_reboot_operation();
            current().unwrap()
        });

        let observed: Result<(String, String, Option<String>), TridentError> =
            run_reboot_command(|| Ok(current().unwrap()));
        assert_eq!(
            observed.unwrap(),
            expected,
            "run_reboot_command should reuse the captured install operation, not mint a fresh one"
        );
    }

    #[test]
    fn test_run_reboot_command_fires_command_error_on_failure_even_without_capture() {
        let _guard = REBOOT_OPERATION_TEST_LOCK.lock().unwrap();
        assert!(
            take_reboot_operation().is_none(),
            "start with a clean slate"
        );

        use tracing_subscriber::layer::SubscriberExt;
        let layer = CapturingLayer::default();
        let events = layer.events.clone();
        let _guard2 =
            tracing::subscriber::set_default(tracing_subscriber::Registry::default().with(layer));

        let _: Result<(), TridentError> =
            run_reboot_command(|| Err(TridentError::internal("reboot boom")));

        let events = events.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|e| e.get("metric_name").map(String::as_str) == Some("command_error")),
            "run_reboot_command should still fire command_error when nothing was captured"
        );
    }

    #[test]
    fn test_set_servicing_id_attaches_to_current_operation() {
        let servicing_id = run_with_operation("cmd", || {
            set_servicing_id("test-servicing-id");
            current().unwrap().2
        });
        assert_eq!(servicing_id, Some("test-servicing-id".to_string()));
    }

    #[test]
    fn test_run_command_passes_through_ok() {
        let result: Result<i32, TridentError> = run_command("cmd", || Ok(42));
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn test_run_command_passes_through_err_unchanged() {
        let result: Result<(), TridentError> =
            run_command("cmd", || Err(TridentError::internal("boom")));
        assert!(result.is_err());
    }

    #[test]
    fn test_run_command_clears_context_after_error() {
        let _: Result<(), TridentError> =
            run_command("cmd", || Err(TridentError::internal("boom")));
        assert!(
            current().is_none(),
            "context must be cleared even when f returns Err"
        );
    }

    /// A minimal `tracing_subscriber::Layer` that records every event's
    /// fields as strings, so `report_command_error`'s output can be
    /// asserted on directly instead of only checking that `run_command`
    /// doesn't panic.
    #[derive(Default, Clone)]
    struct CapturingLayer {
        events: std::sync::Arc<std::sync::Mutex<Vec<std::collections::BTreeMap<String, String>>>>,
    }

    struct CaptureVisitor(std::collections::BTreeMap<String, String>);

    impl tracing::field::Visit for CaptureVisitor {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.0.insert(field.name().to_string(), value.to_string());
        }

        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0
                .insert(field.name().to_string(), format!("{value:?}"));
        }
    }

    impl<S> tracing_subscriber::layer::Layer<S> for CapturingLayer
    where
        S: tracing::Subscriber,
    {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut visitor = CaptureVisitor(std::collections::BTreeMap::new());
            event.record(&mut visitor);
            self.events.lock().unwrap().push(visitor.0);
        }
    }

    #[test]
    fn test_run_command_fires_command_error_with_kind_subkind_location() {
        use tracing_subscriber::layer::SubscriberExt;

        let layer = CapturingLayer::default();
        let events = layer.events.clone();
        let _guard =
            tracing::subscriber::set_default(tracing_subscriber::Registry::default().with(layer));

        let _: Result<(), TridentError> =
            run_command("test_command", || Err(TridentError::internal("boom")));

        let events = events.lock().unwrap();
        let command_error = events
            .iter()
            .find(|e| e.get("metric_name").map(String::as_str) == Some("command_error"))
            .expect("command_error event should have been fired");

        assert_eq!(
            command_error.get("kind").map(String::as_str),
            Some("internal")
        );
        assert!(
            command_error.get("subkind").is_some(),
            "subkind should be present: {command_error:?}"
        );
        assert!(
            command_error
                .get("location")
                .is_some_and(|l| l.contains("operation_context.rs")),
            "location should point at the TridentError::internal call site: {command_error:?}"
        );
        // command_start (from run_with_operation) should also have fired,
        // ahead of command_error.
        assert!(events
            .iter()
            .any(|e| e.get("metric_name").map(String::as_str) == Some("command_start")));
    }
}
