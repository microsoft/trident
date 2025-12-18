use std::{
    pin::Pin,
    task::{Context, Poll},
};

use tokio::sync::{mpsc::UnboundedReceiver, OwnedRwLockWriteGuard};
use tokio_stream::Stream;

/// A wrapper around UnboundedReceiver that implements Stream and holds a
/// RwLockWriteGuard that is dropped when the stream is dropped.
///
/// It is equivalent to tokio_stream::wrappers::UnboundedReceiverStream but with
/// an added lock guard.
pub struct StreamWithLock<T, U: Unpin> {
    inner: UnboundedReceiver<T>,
    _rwlock: OwnedRwLockWriteGuard<U>,
}

impl<T, U: Unpin> StreamWithLock<T, U> {
    pub fn new(inner: UnboundedReceiver<T>, _rwlock: OwnedRwLockWriteGuard<U>) -> Self {
        Self { inner, _rwlock }
    }
}

impl<T, U: Unpin> Stream for StreamWithLock<T, U> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.poll_recv(cx)
    }
}

impl<T, U: Unpin> AsRef<UnboundedReceiver<T>> for StreamWithLock<T, U> {
    fn as_ref(&self) -> &UnboundedReceiver<T> {
        &self.inner
    }
}

impl<T, U: Unpin> AsMut<UnboundedReceiver<T>> for StreamWithLock<T, U> {
    fn as_mut(&mut self) -> &mut UnboundedReceiver<T> {
        &mut self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use tokio::sync::RwLock;
    use tokio_stream::StreamExt;

    #[tokio::test]
    async fn poll_next_yields_items() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<u32>();
        let lock = Arc::new(RwLock::new(()));
        let guard = lock.clone().write_owned().await;

        let mut stream = StreamWithLock::new(rx, guard);

        tx.send(10).unwrap();
        tx.send(20).unwrap();

        assert_eq!(stream.next().await, Some(10));
        assert_eq!(stream.next().await, Some(20));
    }

    #[tokio::test]
    async fn as_ref_and_as_mut_expose_inner_receiver() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<u32>();
        let lock = Arc::new(RwLock::new(()));
        let guard = lock.clone().write_owned().await;

        let mut stream = StreamWithLock::new(rx, guard);

        // AsRef
        assert!(!stream.as_ref().is_closed());

        // AsMut: closing the receiver should make the stream end.
        stream.as_mut().close();
        drop(tx);

        assert_eq!(stream.next().await, None);
    }

    #[tokio::test]
    async fn lock_is_released_when_stream_is_dropped() {
        let (_tx, rx) = tokio::sync::mpsc::unbounded_channel::<u32>();
        let lock = Arc::new(RwLock::new(0u32));
        let guard = lock.clone().write_owned().await;

        let stream = StreamWithLock::new(rx, guard);

        // While stream exists, the write lock is held.
        assert!(lock.try_write().is_err());

        drop(stream);

        // After drop, the write lock should be available again.
        assert!(lock.try_write().is_ok());
    }
}
