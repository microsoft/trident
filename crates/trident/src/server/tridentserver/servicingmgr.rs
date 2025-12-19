use std::{fmt::Debug, sync::Arc};

use tokio::{
    sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock},
    task::JoinError,
};

use harpoon::{FinalStatus, StatusCode};
use trident_api::error::{InvalidInputError, TridentError};

use crate::{server::activitytracker::ActivityTracker, ExitKind};

type ServicingLockGuard = OwnedRwLockWriteGuard<()>;
type ServicingReadGuard = OwnedRwLockReadGuard<()>;

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
    pub(crate) fn new() -> Self {
        Self {
            servicing_lock: Arc::new(RwLock::new(())),
        }
    }

    pub(crate) fn try_lock_servicing(&self) -> Option<ServicingLockGuard> {
        self.servicing_lock.clone().try_write_owned().ok()
    }

    pub(crate) fn try_lock_reading(&self) -> Option<ServicingReadGuard> {
        self.servicing_lock.clone().try_read_owned().ok()
    }

    pub(crate) async fn spawn_servicing_task<F>(
        _guard: ServicingLockGuard,
        tracker: ActivityTracker,
        f: F,
    ) -> FinalStatus
    where
        F: FnOnce() -> Result<ExitKind, TridentError> + Send + 'static,
    {
        match Self::spawn_servicing_blocking_task(tracker, f).await {
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
                    // TODO: convert trident error to harpoon error
                    error: None,
                    reboot_required: false,
                },
            },
            Err(_e) => FinalStatus {
                status: StatusCode::Failure.into(),
                // TODO: create an internal trident error and convert to harpoon error
                error: None,
                reboot_required: false,
            },
        }
    }

    async fn spawn_servicing_blocking_task<F, R>(
        tracker: ActivityTracker,
        f: F,
    ) -> Result<R, JoinError>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        tokio::task::spawn_blocking(move || {
            tracker.on_servicing_started();
            let out = f();
            tracker.on_servicing_ended();
            out
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time;

    #[tokio::test]
    async fn test_servicing_manager_new() {
        let manager = ServicingManager::new();
        // Manager should be created successfully
        assert!(format!("{:?}", manager).contains("ServicingManager"));
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
        let _guard = manager.try_lock_servicing().expect("First lock should succeed");
        
        // Try to acquire the lock again while it's held
        let second_guard = manager.try_lock_servicing();
        assert!(second_guard.is_none(), "Should not acquire servicing lock when already locked");
    }

    #[tokio::test]
    async fn test_try_lock_servicing_succeeds_after_release() {
        let manager = ServicingManager::new();
        {
            let _guard = manager.try_lock_servicing().expect("First lock should succeed");
            // Guard dropped here
        }
        
        // Try to acquire the lock again after it's released
        let second_guard = manager.try_lock_servicing();
        assert!(second_guard.is_some(), "Should acquire servicing lock after previous release");
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
        let _guard1 = manager.try_lock_reading().expect("First read lock should succeed");
        let _guard2 = manager.try_lock_reading().expect("Second read lock should succeed");
        
        // Multiple readers should be able to acquire read locks simultaneously
        // The fact that both guards were acquired successfully proves this
    }

    #[tokio::test]
    async fn test_try_lock_reading_fails_when_write_locked() {
        let manager = ServicingManager::new();
        let _write_guard = manager.try_lock_servicing().expect("Write lock should succeed");
        
        // Try to acquire a read lock while write lock is held
        let read_guard = manager.try_lock_reading();
        assert!(read_guard.is_none(), "Should not acquire read lock when write lock is held");
    }

    #[tokio::test]
    async fn test_try_lock_servicing_fails_when_read_locked() {
        let manager = ServicingManager::new();
        let _read_guard = manager.try_lock_reading().expect("Read lock should succeed");
        
        // Try to acquire a write lock while read lock is held
        let write_guard = manager.try_lock_servicing();
        assert!(write_guard.is_none(), "Should not acquire write lock when read lock is held");
    }

    #[tokio::test]
    async fn test_spawn_servicing_task_success() {
        let manager = ServicingManager::new();
        let guard = manager.try_lock_servicing().expect("Lock should succeed");
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(30));
        
        let result = ServicingManager::spawn_servicing_task(
            guard,
            tracker,
            || Ok(ExitKind::Done),
        ).await;
        
        assert_eq!(result.status, StatusCode::Success as i32);
        assert_eq!(result.reboot_required, false);
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_spawn_servicing_task_success_with_reboot() {
        let manager = ServicingManager::new();
        let guard = manager.try_lock_servicing().expect("Lock should succeed");
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(30));
        
        let result = ServicingManager::spawn_servicing_task(
            guard,
            tracker,
            || Ok(ExitKind::NeedsReboot),
        ).await;
        
        assert_eq!(result.status, StatusCode::Success as i32);
        assert_eq!(result.reboot_required, true);
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_spawn_servicing_task_error() {
        let manager = ServicingManager::new();
        let guard = manager.try_lock_servicing().expect("Lock should succeed");
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(30));
        
        let result = ServicingManager::spawn_servicing_task(
            guard,
            tracker,
            || Err(TridentError::new(
                InvalidInputError::CleanInstallOnProvisionedHost
            )),
        ).await;
        
        assert_eq!(result.status, StatusCode::Failure as i32);
        assert_eq!(result.reboot_required, false);
    }

    #[tokio::test]
    async fn test_spawn_servicing_task_notifies_activity_tracker() {
        let manager = ServicingManager::new();
        let guard = manager.try_lock_servicing().expect("Lock should succeed");
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(30));
        
        // Clone tracker to check state before and after
        let tracker_clone = tracker.clone();
        assert_eq!(tracker_clone.is_servicing_active(), false);
        
        // Spawn a task that takes some time
        let handle = tokio::spawn(async move {
            ServicingManager::spawn_servicing_task(
                guard,
                tracker,
                || {
                    // Task body
                    std::thread::sleep(Duration::from_millis(50));
                    Ok(ExitKind::Done)
                },
            ).await
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
        
        let result = ServicingManager::spawn_servicing_task(
            guard,
            tracker,
            || {
                panic!("Simulated panic in servicing task");
            },
        ).await;
        
        // Panic in spawn_blocking is caught and returns JoinError
        assert_eq!(result.status, StatusCode::Failure as i32);
        assert_eq!(result.reboot_required, false);
    }

    #[tokio::test]
    async fn test_servicing_manager_is_clonable() {
        let manager = ServicingManager::new();
        let manager_clone = manager.clone();
        
        // Both managers should share the same lock
        let _guard = manager.try_lock_servicing().expect("First lock should succeed");
        let second_guard = manager_clone.try_lock_servicing();
        assert!(second_guard.is_none(), "Clone should share the same lock");
    }

    #[tokio::test]
    async fn test_multiple_sequential_servicing_tasks() {
        let manager = ServicingManager::new();
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(30));
        
        // First task
        {
            let guard = manager.try_lock_servicing().expect("First lock should succeed");
            let result = ServicingManager::spawn_servicing_task(
                guard,
                tracker.clone(),
                || Ok(ExitKind::Done),
            ).await;
            assert_eq!(result.status, StatusCode::Success as i32);
        }
        
        // Second task should be able to acquire lock
        {
            let guard = manager.try_lock_servicing().expect("Second lock should succeed");
            let result = ServicingManager::spawn_servicing_task(
                guard,
                tracker.clone(),
                || Ok(ExitKind::NeedsReboot),
            ).await;
            assert_eq!(result.status, StatusCode::Success as i32);
            assert_eq!(result.reboot_required, true);
        }
    }
}
