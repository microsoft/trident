use std::{
    pin::Pin,
    task::{Context, Poll},
};

use tokio::time::Instant;
use tonic::{
    async_trait,
    body::Body,
    codegen::http::{Request, Response},
};
use tonic_middleware::{Middleware, ServiceBound};

use super::ActivityTracker;

#[derive(Clone)]
pub struct ActivityTrackerMiddleware {
    tracker: ActivityTracker,
}

impl ActivityTrackerMiddleware {
    pub(super) fn new(tracker: ActivityTracker) -> Self {
        ActivityTrackerMiddleware { tracker }
    }
}

#[async_trait]
impl<S> Middleware<S> for ActivityTrackerMiddleware
where
    S: ServiceBound,
    S::Future: Send,
{
    async fn call(&self, req: Request<Body>, mut service: S) -> Result<Response<Body>, S::Error> {
        log::debug!("New request received: '{}'", req.uri());

        // Inform the tracker of new activity
        self.tracker.on_connection_start();

        let start_time = Instant::now();
        let uri = req.uri().clone();

        let response = service.call(req).await?;

        let elapsed_time = start_time.elapsed();
        log::debug!("Request processed in {:?}: '{}'", elapsed_time, uri);

        // Wrap the response body with our tracker
        let (parts, body) = response.into_parts();
        let tracked_body = TrackedBody::new(body, self.tracker.clone());
        let response = Response::from_parts(parts, Body::new(tracked_body));

        Ok(response)
    }
}

/// Wrapper around Body that tracks when the stream is dropped
struct TrackedBody {
    inner: Body,
    tracker: ActivityTracker,
}

impl TrackedBody {
    fn new(inner: Body, tracker: ActivityTracker) -> Self {
        Self { inner, tracker }
    }
}

impl Drop for TrackedBody {
    fn drop(&mut self) {
        self.tracker.on_connection_end();
    }
}

impl hyper::body::Body for TrackedBody {
    type Data = <Body as hyper::body::Body>::Data;
    type Error = <Body as hyper::body::Body>::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.inner.size_hint()
    }
}
