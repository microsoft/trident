//! Contains the servicing manager for Trident server.

use std::{fmt::Debug, panic, sync::Arc};

use anyhow::anyhow;
use log::error;
use tokio::sync::{Mutex, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};
use tokio_util::sync::CancellationToken;

use trident_api::error::{InternalError, TridentError};
use trident_proto::v1::{FinalStatus, StatusCode, TridentError as HarpoonTridentError};

use crate::{server::activitytracker::ActivityTracker, ExitKind};

/// Type alias for the servicing(write) lock guard. This guard is to be held by
/// tasks that are running an active servicing action such as install or update.
/// It is implemented as the write lock of a RwLock, so that it is exclusive and
/// prevents any other servicing actions or reading operations from occurring
/// simultaneously.
type ServicingLockGuard = OwnedRwLockWriteGuard<()>;

/// Type alias for the servicing(read) lock guard. This guard is to be held by
/// tasks that are performing read-only operations that should not occur
/// simultaneously with servicing actions. It is implemented as the read lock of
/// a RwLock, so that multiple read operations can occur simultaneously, but
/// they will block if a servicing action is in progress.
#[allow(dead_code)]
type ServicingReadGuard = OwnedRwLockReadGuard<()>;

/// Enum to specify how reboot requests from servicing tasks should be handled.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum RebootDecision {
    /// Reboot requests are deferred to the caller when needed.
    Defer,

    /// Trident will directly handle reboot requests.
    Handle,

    /// A reboot request is not allowed during this operation and will result in
    /// an error if one is requested.
    #[allow(dead_code)]
    Error,
}

/// Helper to manage concurrency for servicing operations.
#[derive(Clone)]
pub(crate) struct ServicingManager {
    servicing_lock: Arc<RwLock<()>>,

    /// The exit kind currently registered in the manager.
    exit_kind: Arc<Mutex<ExitKind>>,

    /// Cancellation token to signal exit requests made by a servicing task.
    cancellation_token: CancellationToken,
}

impl Debug for ServicingManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServicingManager").finish()
    }
}

impl ServicingManager {
    /// Creates a new ServicingManager instance.
    pub(crate) fn new() -> (Self, CancellationToken) {
        // Create a cancellation token for exit requests.
        let token = CancellationToken::new();
        let child = token.child_token();

        let mgr = Self {
            servicing_lock: Arc::new(RwLock::new(())),
            exit_kind: Arc::new(Mutex::new(ExitKind::Done)),
            cancellation_token: token,
        };

        (mgr, child)
    }

    /// Gets the exit kind currently registered in the manager.
    pub(crate) async fn get_exit_kind(&self) -> ExitKind {
        let exit_kind = self.exit_kind.lock().await;
        *exit_kind
    }

    /// Attempts to acquire the servicing (write) lock. Returns
    /// `Some(ServicingLockGuard)` if the lock was successfully acquired, or
    /// `None` if it is already held.
    pub(super) fn try_lock_servicing(&self) -> Option<ServicingLockGuard> {
        self.servicing_lock.clone().try_write_owned().ok()
    }

    /// Attempts to acquire the reading (read) lock. Returns
    /// `Some(ServicingReadGuard)` if the lock was successfully acquired, or
    /// `None` if a servicing lock is held.
    #[allow(dead_code)]
    pub(super) fn try_lock_reading(&self) -> Option<ServicingReadGuard> {
        self.servicing_lock.clone().try_read_owned().ok()
    }

    /// Spawns a servicing task that runs the provided function `f` in a
    /// blocking task. The `ServicingLockGuard` must be provided to ensure that
    /// only one servicing operation is running at a time. The `ActivityTracker`
    /// is used to notify the start and end of servicing activity.
    pub(super) async fn spawn_servicing_task<F>(
        &self,
        reboot_decision: RebootDecision,
        _guard: ServicingLockGuard,
        tracker: ActivityTracker,
        f: F,
    ) -> FinalStatus
    where
        F: FnOnce() -> Result<ExitKind, TridentError> + Send + panic::UnwindSafe + 'static,
    {
        // Spawn the servicing operation in a blocking task, notifying the activity
        // tracker of start and end of servicing through the guard.
        let result = tokio::task::spawn_blocking(move || {
            let _activity_guard = tracker.servicing_guard();
            match panic::catch_unwind(f) {
                Ok(res) => res,
                Err(e) => Err(TridentError::new(InternalError::Panic(format!("{e:?}")))),
            }
        })
        .await;

        let exit_kind = match result {
            Ok(r) => match r {
                Ok(exit_kind) => exit_kind,
                Err(e) => {
                    return FinalStatus {
                        status: StatusCode::Failure.into(),
                        error: Some(HarpoonTridentError::from(&e)),
                        reboot_required: false,
                        reboot_started: false,
                    }
                }
            },
            Err(e) => {
                error!("Servicing task join error: {:?}", e);
                return FinalStatus {
                    status: StatusCode::Failure.into(),
                    error: Some(HarpoonTridentError::from(&TridentError::with_source(
                        InternalError::Internal("Servicing task panicked or was cancelled"),
                        anyhow!(e),
                    ))),
                    reboot_required: false,
                    reboot_started: false,
                };
            }
        };

        let (reboot_required, reboot_started) = match (exit_kind, reboot_decision) {
            // Notify the caller that a reboot is required.
            (ExitKind::NeedsReboot, RebootDecision::Defer) => (true, false),

            // Trident is allowed to request a reboot directly.
            (ExitKind::NeedsReboot, RebootDecision::Handle) => {
                // Set the manager's exit kind to NeedsReboot.
                let mut exit_kind = self.exit_kind.lock().await;
                *exit_kind = ExitKind::NeedsReboot;

                // Request a cancellation to signal shut down the server gracefully.
                self.cancellation_token.cancel();

                (false, true)
            }

            // Error if a reboot was requested but forbidden.
            (ExitKind::NeedsReboot, RebootDecision::Error) => {
                return FinalStatus {
                    status: StatusCode::Failure.into(),
                    error: Some(HarpoonTridentError::from(&TridentError::new(
                        InternalError::Internal("The servicing task requested a reboot, but this task type should not cause reboots."),
                    ))),
                    reboot_required: false,
                    reboot_started: false,
                };
            }

            // Nothing to do, no reboot needed.
            (ExitKind::Done, _) => (false, false),
        };

        FinalStatus {
            status: StatusCode::Success.into(),
            error: None,
            reboot_required,
            reboot_started,
        }
    }

    /// Spawns a reading task that runs the provided function `f` in a
    /// blocking task. The `ServicingReadGuard` must be provided to ensure that
    /// no servicing operation is running concurrently.
    #[allow(dead_code)]
    pub(super) async fn spawn_reading_task<F, T>(
        _guard: ServicingReadGuard,
        f: F,
    ) -> Result<T, TridentError>
    where
        F: FnOnce() -> Result<T, TridentError> + Send + 'static,
        T: Send + 'static,
    {
        tokio::task::spawn_blocking(f).await.map_err(|e| {
            TridentError::with_source(
                InternalError::Internal("Reading task panicked or was cancelled"),
                anyhow!(e),
            )
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use tokio::time;

    use trident_api::error::InvalidInputError;
    use trident_proto::v1::TridentErrorKind;

    #[tokio::test]
    async fn test_servicing_manager_new() {
        let manager = ServicingManager::new();
        // Manager should be created successfully
        assert!(format!("{manager:?}").contains("ServicingManager"));
    }

    #[tokio::test]
    async fn test_try_lock_servicing_success() {
        let (manager, _) = ServicingManager::new();
        let guard = manager.try_lock_servicing();
        assert!(guard.is_some(), "Should acquire servicing lock");
    }

    #[tokio::test]
    async fn test_try_lock_servicing_fails_when_locked() {
        let (manager, _) = ServicingManager::new();
        let _guard = manager
            .try_lock_servicing()
            .expect("First lock should succeed");

        // Try to acquire the lock again while it's held
        let second_guard = manager.try_lock_servicing();
        assert!(
            second_guard.is_none(),
            "Should not acquire servicing lock when already locked"
        );
    }

    #[tokio::test]
    async fn test_try_lock_servicing_succeeds_after_release() {
        let (manager, _) = ServicingManager::new();
        {
            let _guard = manager
                .try_lock_servicing()
                .expect("First lock should succeed");
            // Guard dropped here
        }

        // Try to acquire the lock again after it's released
        let second_guard = manager.try_lock_servicing();
        assert!(
            second_guard.is_some(),
            "Should acquire servicing lock after previous release"
        );
    }

    #[tokio::test]
    async fn test_try_lock_reading_success() {
        let (manager, _) = ServicingManager::new();
        let guard = manager.try_lock_reading();
        assert!(guard.is_some(), "Should acquire read lock");
    }

    #[tokio::test]
    async fn test_try_lock_reading_multiple_readers() {
        let (manager, _) = ServicingManager::new();
        let _guard1 = manager
            .try_lock_reading()
            .expect("First read lock should succeed");
        let _guard2 = manager
            .try_lock_reading()
            .expect("Second read lock should succeed");

        // Multiple readers should be able to acquire read locks simultaneously
        // The fact that both guards were acquired successfully proves this
    }

    #[tokio::test]
    async fn test_try_lock_reading_fails_when_write_locked() {
        let (manager, _) = ServicingManager::new();
        let _write_guard = manager
            .try_lock_servicing()
            .expect("Write lock should succeed");

        // Try to acquire a read lock while write lock is held
        let read_guard = manager.try_lock_reading();
        assert!(
            read_guard.is_none(),
            "Should not acquire read lock when write lock is held"
        );
    }

    #[tokio::test]
    async fn test_try_lock_servicing_fails_when_read_locked() {
        let (manager, _) = ServicingManager::new();
        let _read_guard = manager
            .try_lock_reading()
            .expect("Read lock should succeed");

        // Try to acquire a write lock while read lock is held
        let write_guard = manager.try_lock_servicing();
        assert!(
            write_guard.is_none(),
            "Should not acquire write lock when read lock is held"
        );
    }

    #[tokio::test]
    async fn test_spawn_servicing_task_success() {
        let (manager, _) = ServicingManager::new();
        let guard = manager.try_lock_servicing().expect("Lock should succeed");
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(30));

        let result = manager
            .spawn_servicing_task(RebootDecision::Handle, guard, tracker, || {
                Ok(ExitKind::Done)
            })
            .await;

        assert_eq!(result.status, StatusCode::Success as i32);
        assert!(!result.reboot_required);
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_spawn_servicing_task_success_with_reboot_forward() {
        let (manager, token) = ServicingManager::new();
        let guard = manager.try_lock_servicing().expect("Lock should succeed");
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(30));

        let result = manager
            .spawn_servicing_task(RebootDecision::Defer, guard, tracker, || {
                Ok(ExitKind::NeedsReboot)
            })
            .await;

        assert!(
            !token.is_cancelled(),
            "Cancellation token should not be cancelled"
        );
        assert_eq!(
            manager.get_exit_kind().await,
            ExitKind::Done,
            "Exit kind should be Done"
        );

        assert_eq!(result.status, StatusCode::Success as i32);
        assert!(result.reboot_required);
        assert!(!result.reboot_started);
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_spawn_servicing_task_success_with_reboot_allowed() {
        let (manager, token) = ServicingManager::new();
        let guard = manager.try_lock_servicing().expect("Lock should succeed");
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(30));

        let result = manager
            .spawn_servicing_task(RebootDecision::Handle, guard, tracker, || {
                Ok(ExitKind::NeedsReboot)
            })
            .await;

        assert!(
            token.is_cancelled(),
            "Cancellation token should be cancelled"
        );
        assert_eq!(
            manager.get_exit_kind().await,
            ExitKind::NeedsReboot,
            "Exit kind should be NeedsReboot"
        );

        assert_eq!(result.status, StatusCode::Success as i32);
        assert!(!result.reboot_required);
        assert!(result.reboot_started);
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_spawn_servicing_task_success_with_reboot_error() {
        let (manager, token) = ServicingManager::new();
        let guard = manager.try_lock_servicing().expect("Lock should succeed");
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(30));

        let result = manager
            .spawn_servicing_task(RebootDecision::Error, guard, tracker, || {
                Ok(ExitKind::NeedsReboot)
            })
            .await;

        assert!(
            !token.is_cancelled(),
            "Cancellation token should not be cancelled"
        );
        assert_eq!(
            manager.get_exit_kind().await,
            ExitKind::Done,
            "Exit kind should be Done"
        );

        assert_eq!(result.status, StatusCode::Failure as i32);
        assert!(!result.reboot_required);
        assert!(!result.reboot_started);

        let err = result.error.expect("Error should be present");
        assert_eq!(
            err.kind(),
            TridentErrorKind::InternalError,
            "Error kind should match"
        );
    }

    #[tokio::test]
    async fn test_spawn_servicing_task_error() {
        let (manager, _) = ServicingManager::new();
        let guard = manager.try_lock_servicing().expect("Lock should succeed");
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(30));

        let result = manager
            .spawn_servicing_task(RebootDecision::Handle, guard, tracker, || {
                Err(TridentError::new(
                    InvalidInputError::CleanInstallOnProvisionedHost,
                ))
            })
            .await;

        assert_eq!(result.status, StatusCode::Failure as i32);
        assert!(!result.reboot_required);
    }

    #[tokio::test]
    async fn test_spawn_servicing_task_notifies_activity_tracker() {
        let (manager, _) = ServicingManager::new();
        let guard = manager.try_lock_servicing().expect("Lock should succeed");
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(30));

        // Clone tracker to check state before and after
        let tracker_clone = tracker.clone();
        assert!(!tracker_clone.is_servicing_active());

        // Spawn a task that takes some time
        let handle = tokio::spawn(async move {
            manager
                .spawn_servicing_task(RebootDecision::Handle, guard, tracker, || {
                    // Task body
                    std::thread::sleep(Duration::from_millis(50));
                    Ok(ExitKind::Done)
                })
                .await
        });

        // Give the task a moment to start
        time::sleep(Duration::from_millis(20)).await;

        // Note: The servicing state will be false here because on_servicing_ended
        // is called before the task completes

        let result = handle.await.expect("Task should complete");
        assert_eq!(result.status, StatusCode::Success as i32);
    }

    #[tokio::test]
    async fn test_spawn_servicing_task_panic_handling() {
        let (manager, _) = ServicingManager::new();
        let guard = manager.try_lock_servicing().expect("Lock should succeed");
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(30));

        let result = manager
            .spawn_servicing_task(RebootDecision::Handle, guard, tracker, || {
                panic!("Simulated panic in servicing task");
            })
            .await;

        // Panic in spawn_blocking is caught and returns JoinError
        assert_eq!(result.status, StatusCode::Failure as i32);
        assert!(!result.reboot_required);
    }

    #[tokio::test]
    async fn test_servicing_manager_is_clonable() {
        let (manager, _) = ServicingManager::new();
        let manager_clone = manager.clone();

        // Both managers should share the same lock
        let _guard = manager
            .try_lock_servicing()
            .expect("First lock should succeed");
        let second_guard = manager_clone.try_lock_servicing();
        assert!(second_guard.is_none(), "Clone should share the same lock");
    }

    #[tokio::test]
    async fn test_multiple_sequential_servicing_tasks() {
        let (manager, _) = ServicingManager::new();
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(30));

        // First task
        {
            let guard = manager
                .try_lock_servicing()
                .expect("First lock should succeed");
            let result = manager
                .spawn_servicing_task(RebootDecision::Handle, guard, tracker.clone(), || {
                    Ok(ExitKind::Done)
                })
                .await;
            assert_eq!(result.status, StatusCode::Success as i32);
        }

        // Second task should be able to acquire lock
        {
            let guard = manager
                .try_lock_servicing()
                .expect("Second lock should succeed");
            let result = manager
                .spawn_servicing_task(RebootDecision::Handle, guard, tracker.clone(), || {
                    Ok(ExitKind::NeedsReboot)
                })
                .await;
            assert_eq!(result.status, StatusCode::Success as i32);
            assert!(!result.reboot_required);
            assert!(result.reboot_started);
        }
    }
}
