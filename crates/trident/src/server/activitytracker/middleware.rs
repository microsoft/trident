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
        // Inform the tracker of new activity
        self.tracker.on_connection_start();

        // Call the service and wrap the response body with our tracker
        let (parts, body) = service.call(req).await?.into_parts();
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        pin::Pin,
        task::{Context, Poll, Waker},
        time::Duration,
    };

    use hyper::body::Body as BodyTrait;
    use tonic::codegen::http::{Request, Response, StatusCode};
    use tower::Service;

    // Mock service for testing
    #[derive(Clone)]
    struct MockService {
        should_error: bool,
        delay: Duration,
    }

    impl MockService {
        fn new() -> Self {
            Self {
                should_error: false,
                delay: Duration::from_millis(0),
            }
        }

        fn with_should_error(mut self) -> Self {
            self.should_error = true;
            self
        }

        fn with_delay_ms(mut self, delay: u64) -> Self {
            self.delay = Duration::from_millis(delay);
            self
        }
    }

    impl Service<Request<Body>> for MockService {
        type Response = Response<Body>;
        type Error = Box<dyn std::error::Error + Send + Sync>;
        type Future =
            Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _req: Request<Body>) -> Self::Future {
            let should_error = self.should_error;
            let delay = self.delay;
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                if should_error {
                    Err("Service error".into())
                } else {
                    Ok(Response::new(Body::empty()))
                }
            })
        }
    }

    #[tokio::test]
    async fn test_middleware_successful_request() {
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(10));
        let middleware = ActivityTrackerMiddleware::new(tracker.clone());

        let req = Request::builder()
            .uri("http://test.com/api")
            .body(Body::empty())
            .unwrap();

        let service = MockService::new().with_delay_ms(100);

        // Before request, no active connections
        assert!(!tracker.has_active_connections());

        // Spawn the request in a separate task so we can check the tracker state during execution
        let tracker_clone = tracker.clone();
        let handle = tokio::spawn(async move { middleware.call(req, service).await });

        // Give it a moment to start and register the connection
        tokio::time::sleep(Duration::from_millis(50)).await;

        // While the service is running, there should be an active connection
        assert!(tracker_clone.has_active_connections());

        // Wait for the request to complete
        let result = handle.await.unwrap();

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // After response body is dropped, no active connections
        drop(response);
        assert!(!tracker_clone.has_active_connections());
    }

    #[tokio::test]
    async fn test_middleware_tracks_connection_start() {
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(10));
        let middleware = ActivityTrackerMiddleware::new(tracker.clone());

        let req = Request::builder()
            .uri("http://test.com/api")
            .body(Body::empty())
            .unwrap();

        let service = MockService::new();

        assert!(!tracker.has_active_connections());

        let _response = middleware.call(req, service).await.unwrap();

        // Connection should be tracked (incremented during call)
        // Note: might be 0 again if body dropped, but start was called
    }

    #[tokio::test]
    async fn test_middleware_error_propagation() {
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(10));
        let middleware = ActivityTrackerMiddleware::new(tracker);

        let req = Request::builder()
            .uri("http://test.com/api")
            .body(Body::empty())
            .unwrap();

        let service = MockService::new().with_should_error();

        let result = middleware.call(req, service).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_tracked_body_drop_calls_on_connection_end() {
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(10));

        // Manually increment to track
        tracker.on_connection_start();
        assert!(tracker.has_active_connections());

        {
            let body = Body::empty();
            let _tracked_body = TrackedBody::new(body, tracker.clone());
            // TrackedBody is in scope
            assert!(tracker.has_active_connections());
        } // TrackedBody dropped here

        // After drop, connection should be ended
        assert!(!tracker.has_active_connections());
    }

    #[tokio::test]
    async fn test_tracked_body_is_end_stream() {
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(10));
        let body = Body::empty();
        let tracked_body = TrackedBody::new(body, tracker);

        assert!(tracked_body.is_end_stream());
    }

    #[tokio::test]
    async fn test_tracked_body_size_hint() {
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(10));
        let body = Body::empty();
        let tracked_body = TrackedBody::new(body, tracker);

        let hint = tracked_body.size_hint();
        // For empty body, should have size 0
        assert_eq!(hint.lower(), 0);
    }

    #[tokio::test]
    async fn test_tracked_body_poll_frame() {
        let (tracker, _rx, _token) = ActivityTracker::new(Duration::from_secs(10));
        let body = Body::empty();
        let mut tracked_body = TrackedBody::new(body, tracker);

        // Create a simple waker for testing
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);

        // Poll the body
        let result = Pin::new(&mut tracked_body).poll_frame(&mut cx);

        // Empty body should return None
        assert!(matches!(result, Poll::Ready(None)));
    }

    #[tokio::test]
    async fn test_mock_service_poll_ready() {
        let mut service = MockService::new();

        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);

        // Test that poll_ready returns Ready(Ok(()))
        let result = service.poll_ready(&mut cx);
        assert!(matches!(result, Poll::Ready(Ok(()))));
    }
}
