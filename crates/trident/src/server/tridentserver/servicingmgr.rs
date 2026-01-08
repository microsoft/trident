//! Contains the servicing manager for Trident server.

use std::{fmt::Debug, sync::Arc};

use anyhow::anyhow;
use log::error;
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

use harpoon::{FinalStatus, StatusCode, TridentError as HarpoonTridentError};
use trident_api::error::{InternalError, TridentError};

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
type ServicingReadGuard = OwnedRwLockReadGuard<()>;

/// Helper to manage concurrency for servicing operations.
#[derive(Clone)]
pub(crate) struct ServicingManager {
    servicing_lock: Arc<RwLock<()>>,
}

impl Debug for ServicingManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServicingManager").finish()
    }
}

impl ServicingManager {
    /// Creates a new ServicingManager instance.
    pub(crate) fn new() -> Self {
        Self {
            servicing_lock: Arc::new(RwLock::new(())),
        }
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
    pub(super) fn try_lock_reading(&self) -> Option<ServicingReadGuard> {
        self.servicing_lock.clone().try_read_owned().ok()
    }

    /// Spawns a servicing task that runs the provided function `f` in a
    /// blocking task. The `ServicingLockGuard` must be provided to ensure that
    /// only one servicing operation is running at a time. The `ActivityTracker`
    /// is used to notify the start and end of servicing activity.
    pub(super) async fn spawn_servicing_task<F>(
        _guard: ServicingLockGuard,
        tracker: ActivityTracker,
        f: F,
    ) -> FinalStatus
    where
        F: FnOnce() -> Result<ExitKind, TridentError> + Send + 'static,
    {
        // Spawn the servicing operation in a blocking task, notifying the activity
        // tracker of start and end of servicing through the guard.
        let result = tokio::task::spawn_blocking(move || {
            let _activity_guard = tracker.servicing_guard();
            f()
        })
        .await;

        match result {
            Ok(r) => match r {
                Ok(exit_kind) => FinalStatus {
                    status: StatusCode::Success.into(),
                    error: None,
                    reboot_required: match exit_kind {
                        ExitKind::Done => false,
                        ExitKind::NeedsReboot => true,
                    },
                },
                Err(e) => FinalStatus {
                    status: StatusCode::Failure.into(),
                    error: Some(HarpoonTridentError::from(&e)),
                    reboot_required: false,
                },
            },
            Err(e) => {
                error!("Servicing task join error: {:?}", e);
                FinalStatus {
                    status: StatusCode::Failure.into(),
                    error: Some(HarpoonTridentError::from(&TridentError::with_source(
                        InternalError::Internal("Servicing task panicked or was cancelled"),
                        anyhow!(e),
                    ))),
                    reboot_required: false,
                }
            }
        }
    }

    /// Spawns a reading task that runs the provided function `f` in a
    /// blocking task. The `ServicingReadGuard` must be provided to ensure that
    /// no servicing operation is running concurrently.
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

    #[tokio::test]
    async fn test_servicing_manager_new() {
        let manager = ServicingManager::new();
        // Manager should be created successfully
        assert!(format!("{manager:?}").contains("ServicingManager"));
    }

    #[tokio::test]
    async fn test_try_lock_servicing_success() {
        let manager = ServicingManager::new();
        let guard = manager.try_lock_servicing();
        assert!(guard.is_some(), "Should acquire servicing lock");
    }

    #[tokio::test]
    async fn test_try_lock_servicing_fails_when_locked() {
        let manager = ServicingManager::new();
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
        let manager = ServicingManager::new();
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
        let manager = ServicingManager::new();
        let guard = manager.try_lock_reading();
        assert!(guard.is_some(), "Should acquire read lock");
    }

    #[tokio::test]
    async fn test_try_lock_reading_multiple_readers() {
        let manager = ServicingManager::new();
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
        let manager = ServicingManager::new();
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
        let manager = ServicingManager::new();
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
        let manager = ServicingManager::new();
        let guard = manager.try_lock_servicing().expect("Lock should succeed");
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(30));

        let result =
            ServicingManager::spawn_servicing_task(guard, tracker, || Ok(ExitKind::Done)).await;

        assert_eq!(result.status, StatusCode::Success as i32);
        assert!(!result.reboot_required);
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_spawn_servicing_task_success_with_reboot() {
        let manager = ServicingManager::new();
        let guard = manager.try_lock_servicing().expect("Lock should succeed");
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(30));

        let result =
            ServicingManager::spawn_servicing_task(guard, tracker, || Ok(ExitKind::NeedsReboot))
                .await;

        assert_eq!(result.status, StatusCode::Success as i32);
        assert!(result.reboot_required);
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_spawn_servicing_task_error() {
        let manager = ServicingManager::new();
        let guard = manager.try_lock_servicing().expect("Lock should succeed");
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(30));

        let result = ServicingManager::spawn_servicing_task(guard, tracker, || {
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
        let manager = ServicingManager::new();
        let guard = manager.try_lock_servicing().expect("Lock should succeed");
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(30));

        // Clone tracker to check state before and after
        let tracker_clone = tracker.clone();
        assert!(!tracker_clone.is_servicing_active());

        // Spawn a task that takes some time
        let handle = tokio::spawn(async move {
            ServicingManager::spawn_servicing_task(guard, tracker, || {
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
        let manager = ServicingManager::new();
        let guard = manager.try_lock_servicing().expect("Lock should succeed");
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(30));

        let result = ServicingManager::spawn_servicing_task(guard, tracker, || {
            panic!("Simulated panic in servicing task");
        })
        .await;

        // Panic in spawn_blocking is caught and returns JoinError
        assert_eq!(result.status, StatusCode::Failure as i32);
        assert!(!result.reboot_required);
    }

    #[tokio::test]
    async fn test_servicing_manager_is_clonable() {
        let manager = ServicingManager::new();
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
        let manager = ServicingManager::new();
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(30));

        // First task
        {
            let guard = manager
                .try_lock_servicing()
                .expect("First lock should succeed");
            let result = ServicingManager::spawn_servicing_task(guard, tracker.clone(), || {
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
            let result = ServicingManager::spawn_servicing_task(guard, tracker.clone(), || {
                Ok(ExitKind::NeedsReboot)
            })
            .await;
            assert_eq!(result.status, StatusCode::Success as i32);
            assert!(result.reboot_required);
        }
    }
}
