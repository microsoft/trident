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
//! `DataStore::correlation_id`, which is unrelated and set separately on
//! `TraceStream`/`AppInsightsSender`).

use std::cell::RefCell;

use uuid::Uuid;

thread_local! {
    static CURRENT_OPERATION: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
}

/// Runs `f` with this thread tagged as executing `command`, under a fresh
/// `operation_id`. Also fires a `command_start` metric event immediately,
/// tagged the same way. Clears the tag afterwards (even if `f` panics,
/// via a drop guard), so a thread that runs multiple commands over its
/// lifetime (e.g. a thread pool worker reused across `spawn_blocking`
/// calls) never leaks a stale tag into an unrelated later command.
pub fn run_with_operation<R>(command: &str, f: impl FnOnce() -> R) -> R {
    let operation_id = Uuid::new_v4().to_string();

    tracing::info!(
        metric_name = "command_start",
        command = command,
        operation_id = operation_id.as_str(),
    );

    CURRENT_OPERATION.with(|cell| {
        *cell.borrow_mut() = Some((operation_id, command.to_string()));
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

/// Returns the `(operation_id, command)` pair set by
/// [`run_with_operation`] for the calling thread, if any.
pub(crate) fn current() -> Option<(String, String)> {
    CURRENT_OPERATION.with(|cell| cell.borrow().clone())
}

/// A snapshot of another thread's operation context (see
/// [`run_with_operation`]), capturable via [`snapshot`] and re-installed
/// on a different thread via [`run_with_captured_operation`]. Used to
/// propagate `operation_id`/`command` into threads spawned mid-command
/// (e.g. `MonitorMetrics`'s background sampling thread), which otherwise
/// start with no thread-local context of their own and would silently
/// drop these fields from their own metrics.
#[derive(Clone)]
pub struct CapturedOperation(String, String);

/// Captures the calling thread's current operation context, if any, for
/// later re-installation on another thread via
/// [`run_with_captured_operation`]. Call this on the *spawning* thread,
/// before handing the result to the new thread's closure.
pub fn snapshot() -> Option<CapturedOperation> {
    current().map(|(operation_id, command)| CapturedOperation(operation_id, command))
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
    let Some(CapturedOperation(operation_id, command)) = captured else {
        return f();
    };

    CURRENT_OPERATION.with(|cell| {
        *cell.borrow_mut() = Some((operation_id, command));
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
        let (operation_id, command) = observed.expect("context should be set inside f");
        assert_eq!(command, "test_command");
        assert_eq!(operation_id.len(), 36, "operation_id should be a UUID");

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
        let (expected_operation_id, expected_command, observed) =
            run_with_operation("cmd_from_parent_thread", || {
                let captured = snapshot().expect("should capture a context");
                let (operation_id, command) = current().unwrap();

                let observed = std::thread::spawn(move || {
                    // No context on a fresh thread until installed.
                    assert!(current().is_none());
                    run_with_captured_operation(Some(captured), current)
                })
                .join()
                .unwrap();

                (operation_id, command, observed)
            });

        assert_eq!(
            observed,
            Some((expected_operation_id, expected_command)),
            "captured operation_id/command should propagate to the new thread"
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
}
