use std::{
    io::{Read, Result as IoResult},
    time::{Duration, Instant},
};

use log::warn;

use trident_api::primitives::bytes::ByteCount;

const RING_BUFFER_SIZE: usize = 10;

/// A `Read` wrapper that monitors download speed using a moving average over
/// the last [`RING_BUFFER_SIZE`] reads. When the speed falls below a
/// configurable threshold, it emits debug-level log messages at a configurable
/// minimum cadence.
pub struct HttpDownloadMonitor<R> {
    inner: R,
    /// Expected size of the complete file being read (for log context).
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
    /// Total bytes read through the monitor (for log context).
    total_bytes: u64,
}

impl<R> HttpDownloadMonitor<R> {
    /// Creates a new download monitor wrapping `inner`.
    ///
    /// * `threshold_mbps` — speed in Mbps below which debug messages are
    ///   emitted.
    /// * `report_cadence` — minimum interval between consecutive log messages.
    pub fn new(inner: R, size: u64, threshold_mbps: f64, report_cadence: Duration) -> Self {
        Self {
            inner,
            size,
            samples: [(0, Duration::ZERO); RING_BUFFER_SIZE],
            sample_idx: 0,
            sample_count: 0,
            threshold_mbps,
            report_cadence,
            last_report: Instant::now(),
            total_bytes: 0,
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
}

impl<R: Read> Read for HttpDownloadMonitor<R> {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        let start = Instant::now();
        let n = self.inner.read(buf)?;
        let elapsed = start.elapsed();

        if n > 0 {
            self.total_bytes += n as u64;
            self.record_sample(n as u64, elapsed);

            if let Some(mbps) = self.moving_average_mbps() {
                if mbps <= self.threshold_mbps && self.last_report.elapsed() >= self.report_cadence
                {
                    let pct = if self.size > 0 {
                        self.total_bytes as f64 / self.size as f64 * 100.0
                    } else {
                        0.0
                    };

                    let eta = if self.size > self.total_bytes {
                        self.moving_average_bytes_per_sec()
                            .filter(|&bps| bps > 0.0)
                            .map(|bps| {
                                let remaining = (self.size - self.total_bytes) as f64;
                                format_duration(Duration::from_secs_f64(remaining / bps))
                            })
                            .unwrap_or_else(|| "unknown".to_string())
                    } else {
                        "done".to_string()
                    };

                    warn!(
                        "Slow download: {:.2} Mbps, {:.1}% complete ({}/{}), ETA: {}",
                        mbps,
                        pct,
                        ByteCount::from(self.total_bytes).to_human_readable_approx(),
                        ByteCount::from(self.size).to_human_readable_approx(),
                        eta,
                    );
                    self.last_report = Instant::now();
                }
            }
        }

        Ok(n)
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
    use std::io::Cursor;

    #[test]
    fn test_monitor_passes_through_data() {
        let data = b"hello world";
        let len = data.len() as u64;
        let mut monitor = HttpDownloadMonitor::new(
            Cursor::new(data.as_slice()),
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
            HttpDownloadMonitor::new(Cursor::new(data), len, 10.0, Duration::from_secs(1));

        let mut buf = vec![0u8; 64];
        // Read more times than the ring buffer size.
        for _ in 0..RING_BUFFER_SIZE + 5 {
            let _ = monitor.read(&mut buf);
        }

        assert_eq!(monitor.sample_count, RING_BUFFER_SIZE);
        assert_eq!(monitor.sample_idx, 5); // wrapped around
    }
}
