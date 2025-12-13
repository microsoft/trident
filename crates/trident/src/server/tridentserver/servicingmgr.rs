use std::{fmt::Debug, sync::Arc};

use tokio::{
    sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock},
    task::JoinError,
};

// TODO: Enable once #396 is closed.
// use crate::{app_proto::Control};
use crate::server::activitytracker::ActivityTracker;

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

    // TODO: Enable once #396 is closed to turn `Control` into the final struct
    // representing the final result of a servicing operation.

    // pub(crate) async fn spawn_servicing_task<F>(
    //     _guard: ServicingLockGuard,
    //     tracker: ActivityTracker,
    //     f: F,
    // ) -> Control
    // where
    //     F: FnOnce() -> Result<(), Error> + Send + 'static,
    // {
    //     match Self::spawn_servicing_blocking_task(tracker, f).await {
    //         Ok(r) => match r {
    //             Ok(_) => Control {
    //                 status: 0,
    //                 message: "Servicing operation completed successfully".to_string(),
    //             },
    //             Err(e) => Control {
    //                 status: 1,
    //                 message: "Servicing operation failed: ".to_string() + &e.to_string(),
    //             },
    //         },
    //         Err(e) => Control {
    //             status: -1,
    //             message: "Servicing failed to complete: ".to_string() + &e.to_string(),
    //         },
    //     }
    // }

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
