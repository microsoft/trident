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
}
