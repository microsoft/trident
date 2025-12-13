use std::{
    pin::Pin,
    task::{Context, Poll},
};

use tokio::sync::{mpsc::UnboundedReceiver, OwnedRwLockWriteGuard};
use tokio_stream::Stream;

/// A wrapper around UnboundedReceiver that implements Stream and holds a
/// RwLockWriteGuard that is dropped when the stream is closed.
///
/// It is equivalent to tokio_stream::wrappers::UnboundedReceiverStream but with
/// an added lock guard.
pub struct StreamWithLock<T, U: Unpin> {
    inner: UnboundedReceiver<T>,
    _rwlock: OwnedRwLockWriteGuard<U>,
}

impl<T, U: Unpin> StreamWithLock<T, U> {
    fn new(inner: UnboundedReceiver<T>, _rwlock: OwnedRwLockWriteGuard<U>) -> Self {
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
