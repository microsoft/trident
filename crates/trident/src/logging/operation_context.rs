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

use std::cell::RefCell;

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

/// Like [`run_with_operation`], but specifically for the
/// `Result<T, TridentError>` shape both places that run a command
/// actually use (CLI dispatch, gRPC's `servicing_request`): additionally
/// fires a `command_error` metric -- breaking the error down into `kind`,
/// `subkind`, and `location` -- if `f` returns `Err`, while the
/// operation_id/command context is still active (so it's correlated the
/// same way `command_start` is).
pub fn run_command<T>(
    command: &str,
    f: impl FnOnce() -> Result<T, TridentError>,
) -> Result<T, TridentError> {
    run_with_operation(command, || {
        let result = f();
        if let Err(ref error) = result {
            report_command_error(error);
        }
        result
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
            Some((expected_operation_id, expected_command, expected_servicing_id)),
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
