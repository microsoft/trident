use std::{
    io::{Read, Result as IoResult},
    time::{Duration, Instant},
};

use log::debug;
use trident_api::primitives::bytes::ByteCount;

const RING_BUFFER_SIZE: usize = 10;

/// A `Read` wrapper that monitors download speed using a moving average over
/// the last [`RING_BUFFER_SIZE`] reads. When the speed falls below a
/// configurable threshold, it emits debug-level log messages at a
/// configurable minimum cadence. When the wrapper is dropped (i.e. the read
/// is finished, successfully or not), it emits a single `tracing` metric
/// event (picked up by the metrics pipeline) with the overall size and
/// average rate for the whole read.
pub struct ReadMonitor<R> {
    inner: R,
    /// Identifies what is being read, for metric/log context (e.g. a
    /// partition id, or "esp").
    label: String,
    /// Expected size of the complete file being read (for log/metric context).
    size: u64,
    /// Ring buffer of (bytes_read, elapsed) samples.
    samples: [(u64, Duration); RING_BUFFER_SIZE],
    /// Next write position in the ring buffer.
    sample_idx: usize,
    /// Number of samples recorded so far (capped at RING_BUFFER_SIZE).
    sample_count: usize,
    /// Speed threshold in megabits per second below which to start reporting.
    threshold_mbps: f64,
    /// Minimum time between consecutive log messages.
    report_cadence: Duration,
    /// When the last slow-speed message was emitted.
    last_report: Instant,
    /// Total bytes read through the monitor (for log/metric context).
    total_bytes: u64,
    /// When this monitor was created, used to compute the overall average
    /// rate for the whole read at drop time.
    started: Instant,
}

impl<R> ReadMonitor<R> {
    /// Creates a new download monitor wrapping `inner`.
    ///
    /// * `label` — identifies what is being read (e.g. partition id), used to
    ///   tag log messages and metric events.
    /// * `threshold_mbps` — speed in Mbps below which slow-download debug log
    ///   messages are emitted. Does not affect the summary metric emitted at
    ///   drop time, which is always recorded regardless of speed.
    /// * `report_cadence` — minimum interval between consecutive slow-download
    ///   debug log messages.
    pub fn new(
        inner: R,
        label: impl Into<String>,
        size: u64,
        threshold_mbps: f64,
        report_cadence: Duration,
    ) -> Self {
        Self {
            inner,
            label: label.into(),
            size,
            samples: [(0, Duration::ZERO); RING_BUFFER_SIZE],
            sample_idx: 0,
            sample_count: 0,
            threshold_mbps,
            report_cadence,
            last_report: Instant::now(),
            total_bytes: 0,
            started: Instant::now(),
        }
    }

    /// Computes the moving-average speed in Mbps from the ring buffer.
    fn moving_average_mbps(&self) -> Option<f64> {
        self.moving_average_bytes_per_sec()
            .map(|bps| bps * 8.0 / 1_000_000.0)
    }

    /// Computes the moving-average speed in bytes per second.
    fn moving_average_bytes_per_sec(&self) -> Option<f64> {
        if self.sample_count == 0 {
            return None;
        }

        let (total_bytes, total_dur) = self.samples[..self.sample_count]
            .iter()
            .fold((0u64, Duration::ZERO), |(b, d), (sb, sd)| (b + sb, d + *sd));

        let secs = total_dur.as_secs_f64();
        if secs <= 0.0 {
            return None;
        }

        Some(total_bytes as f64 / secs)
    }

    fn record_sample(&mut self, bytes: u64, elapsed: Duration) {
        self.samples[self.sample_idx] = (bytes, elapsed);
        self.sample_idx = (self.sample_idx + 1) % RING_BUFFER_SIZE;
        if self.sample_count < RING_BUFFER_SIZE {
            self.sample_count += 1;
        }
    }

    /// Computes the overall average rate for the whole read so far, in Mbps,
    /// based on total bytes read and total wall-clock time since the monitor
    /// was created (as opposed to the short moving-average window used for
    /// the slow-speed reporting).
    fn overall_avg_mbps(&self) -> Option<f64> {
        let secs = self.started.elapsed().as_secs_f64();
        if secs <= 0.0 || self.total_bytes == 0 {
            return None;
        }
        Some((self.total_bytes as f64 * 8.0) / (secs * 1_000_000.0))
    }
}

impl<R: Read> Read for ReadMonitor<R> {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        let start = Instant::now();
        let n = self.inner.read(buf)?;
        let elapsed = start.elapsed();

        // Return early:
        // - if there is no threshold configured, which disables the monitor, or
        // - report cadence is 0 or negative, which disables the monitor, or
        // - if n == 0, which naturally happens at EOF.
        if self.threshold_mbps <= 0.0 || self.report_cadence <= Duration::ZERO || n == 0 {
            return Ok(n);
        }

        self.total_bytes += n as u64;
        self.record_sample(n as u64, elapsed);

        if let Some(mbps) = self.moving_average_mbps() {
            if mbps <= self.threshold_mbps && self.last_report.elapsed() >= self.report_cadence {
                let pct = if self.size > 0 {
                    self.total_bytes as f64 / self.size as f64 * 100.0
                } else {
                    0.0
                };

                let eta_secs = if self.size > self.total_bytes {
                    self.moving_average_bytes_per_sec()
                        .filter(|&bps| bps > 0.0)
                        .map(|bps| (self.size - self.total_bytes) as f64 / bps)
                } else {
                    None
                };

                let eta = eta_secs
                    .map(|secs| format_duration(Duration::from_secs_f64(secs)))
                    .unwrap_or_else(|| {
                        if self.size > self.total_bytes {
                            "unknown".to_string()
                        } else {
                            "done".to_string()
                        }
                    });

                debug!(
                    "Slow download of '{}': {:.2} Mbps, {:.1}% complete ({}/{}), ETA: {}",
                    self.label,
                    mbps,
                    pct,
                    ByteCount::from(self.total_bytes).to_human_readable_approx(),
                    ByteCount::from(self.size).to_human_readable_approx(),
                    eta,
                );

                self.last_report = Instant::now();
            }
        }

        Ok(n)
    }
}

impl<R> Drop for ReadMonitor<R> {
    /// Emits a single summary metric once the read finishes (or the monitor
    /// is otherwise dropped), regardless of whether the slow-speed threshold
    /// was ever crossed. This gives one metric per file with the complete
    /// download rate and size, in addition to any slow-speed events above.
    fn drop(&mut self) {
        // Nothing was ever read (e.g. size-0 file, or the reader was dropped
        // before any read happened) -- skip emitting a meaningless metric.
        if self.total_bytes == 0 {
            return;
        }

        let duration_secs = self.started.elapsed().as_secs_f64();
        let avg_mbps = self.overall_avg_mbps();
        let complete = self.size == 0 || self.total_bytes >= self.size;

        tracing::info!(
            metric_name = "image_read_complete",
            label = self.label,
            bytes_read = self.total_bytes,
            total_bytes = self.size,
            duration_secs = duration_secs,
            avg_mbps = avg_mbps,
            complete = complete,
        );
    }
}

/// Formats a duration as a human-readable string (e.g., "2h 15m", "3m 42s", "17s").
fn format_duration(d: Duration) -> String {
    let total_secs = d.as_secs();
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;

    if hours > 0 {
        format!("{hours}h {mins:02}m")
    } else if mins > 0 {
        format!("{mins}m {secs:02}s")
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Cursor,
        sync::{Arc, Mutex},
    };
    use tracing::{
        field::{Field, Visit},
        span, Event, Subscriber,
    };
    use tracing_subscriber::{layer::Layer, layer::SubscriberExt, registry::LookupSpan};

    #[test]
    fn test_monitor_passes_through_data() {
        let data = b"hello world";
        let len = data.len() as u64;
        let mut monitor = ReadMonitor::new(
            Cursor::new(data.as_slice()),
            "test",
            len,
            10.0,
            Duration::from_secs(1),
        );

        let mut buf = vec![0u8; 32];
        let n = monitor.read(&mut buf).unwrap();
        assert_eq!(n, data.len());
        assert_eq!(&buf[..n], data);
    }

    #[test]
    fn test_ring_buffer_wraps() {
        let data = vec![0u8; 1024];
        let len = data.len() as u64;
        let mut monitor =
            ReadMonitor::new(Cursor::new(data), "test", len, 10.0, Duration::from_secs(1));

        let mut buf = vec![0u8; 64];
        // Read more times than the ring buffer size.
        for _ in 0..RING_BUFFER_SIZE + 5 {
            let _ = monitor.read(&mut buf);
        }

        assert_eq!(monitor.sample_count, RING_BUFFER_SIZE);
        assert_eq!(monitor.sample_idx, 5); // wrapped around
    }

    /// A minimal tracing `Layer` that records the field values of every
    /// event carrying a `metric_name` field, keyed by metric name -> field
    /// name -> stringified value. Used to assert that `ReadMonitor` emits
    /// the expected metrics, mirroring the pattern used by
    /// `logging::tracestream`'s own tests.
    #[derive(Default, Clone)]
    struct RecordingLayer {
        events: Arc<Mutex<Vec<(String, std::collections::BTreeMap<String, String>)>>>,
    }

    #[derive(Default)]
    struct FieldVisitor(std::collections::BTreeMap<String, String>);
    impl Visit for FieldVisitor {
        fn record_i64(&mut self, field: &Field, value: i64) {
            self.0.insert(field.name().to_string(), value.to_string());
        }
        fn record_f64(&mut self, field: &Field, value: f64) {
            self.0.insert(field.name().to_string(), value.to_string());
        }
        fn record_u64(&mut self, field: &Field, value: u64) {
            self.0.insert(field.name().to_string(), value.to_string());
        }
        fn record_bool(&mut self, field: &Field, value: bool) {
            self.0.insert(field.name().to_string(), value.to_string());
        }
        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.insert(field.name().to_string(), value.to_string());
        }
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0
                .insert(field.name().to_string(), format!("{:?}", value));
        }
    }

    impl<S> Layer<S> for RecordingLayer
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        fn on_event(&self, event: &Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
            let mut visitor = FieldVisitor::default();
            event.record(&mut visitor);
            if let Some(name) = visitor.0.get("metric_name").cloned() {
                self.events.lock().unwrap().push((name, visitor.0));
            }
        }
    }

    #[test]
    fn test_emits_complete_metric_on_drop() {
        let layer = RecordingLayer::default();
        let events = layer.events.clone();
        let subscriber = tracing_subscriber::Registry::default().with(layer);

        let data = vec![0u8; 4096];
        let len = data.len() as u64;

        tracing::subscriber::with_default(subscriber, || {
            let mut monitor = ReadMonitor::new(
                Cursor::new(data),
                "test-partition",
                len,
                // Very high threshold so the slow-read metric never fires;
                // we only want to see the completion metric here.
                f64::MAX,
                Duration::from_secs(1),
            );
            let mut buf = vec![0u8; 4096];
            monitor.read(&mut buf).unwrap();
            // Drop explicitly to trigger the summary metric.
            drop(monitor);
        });

        let recorded = events.lock().unwrap();
        let complete = recorded
            .iter()
            .find(|(name, _)| name == "image_read_complete")
            .expect("expected an image_read_complete metric event");
        assert_eq!(complete.1.get("label").unwrap(), "test-partition");
        assert_eq!(complete.1.get("bytes_read").unwrap(), &len.to_string());
        assert_eq!(complete.1.get("total_bytes").unwrap(), &len.to_string());
        assert_eq!(complete.1.get("complete").unwrap(), "true");
    }
}
