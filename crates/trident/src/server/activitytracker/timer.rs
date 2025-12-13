use std::time::Duration;

use tokio::sync::oneshot;

pub struct Timer {
    tx: oneshot::Sender<()>,
}

impl Timer {
    pub fn new<F>(duration: Duration, f: F) -> Self
    where
        F: FnOnce() + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep(duration) => {
                    (f)();
                }
                _ = rx => {
                    // Timer was cancelled
                }
            }
        });
        Timer { tx }
    }

    pub fn cancel(self) {
        let _ = self.tx.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::{sync::mpsc, time};

    #[tokio::test]
    async fn test_timer_executes_function() {
        let (tx, rx) = oneshot::channel();
        let _timer = Timer::new(Duration::from_millis(100), move || {
            let _ = tx.send(());
        });
        time::timeout(Duration::from_secs(1), rx)
            .await
            .expect("Timer did not execute the function in time")
            .expect("Error receiving from oneshot channel");
    }

    #[tokio::test]
    async fn test_timer_cancellation() {
        // Make a mpsc channel to signal if the timer function was called. Keep
        // a local copy of the sender to prevent the channel from closing.
        let (tx, mut rx) = mpsc::channel(1);
        let _tx = tx.clone();

        // Create and immediately cancel the timer
        let timer = Timer::new(Duration::from_millis(200), move || {
            let _ = tx.blocking_send(());
        });
        timer.cancel();

        // Wait to see if we receive anything on the channel. We should not
        // receive anything since the timer was cancelled.
        time::timeout(Duration::from_millis(400), rx.recv())
            .await
            .expect_err("Timer was not cancelled properly.");
    }
}
