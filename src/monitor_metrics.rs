use anyhow::{Error, Result};
use log::{error, trace};
use procfs::{net::DeviceStatus, process::Process, ticks_per_second};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64},
        Arc,
    },
    thread,
    time::Duration,
};

// This constant defines the interval at which the monitoring thread will check for updates.
const MONITORING_INTERVAL_MS: u64 = 100; // in milliseconds

/// `MonitorMetrics` is a struct with shareable fields providing methods to monitor and log CPU, memory, and network metrics.
pub struct MonitorMetrics {
    metric_count: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    join_handle: Option<thread::JoinHandle<()>>,
}

/// `CpuStat` is a struct that tracks CPU usage metrics.
/// It stores the phase, start CPU ticks, last CPU ticks, and ticks per second.
/// It provides methods to update CPU ticks and calculate CPU time.
/// It also provides a method to log the CPU time summary.
#[derive(Debug, Clone, Default)]
struct CpuStat {
    phase: String,
    start_cpu_ticks: f64,
    last_cpu_ticks: f64,
    ticks_per_second: f64,
}
impl CpuStat {
    /// Creates a new `CpuStat` instance with the given phase, start CPU ticks, and ticks per second.
    fn new(phase: String, start_cpu_ticks: f64, ticks_per_second: f64) -> Self {
        Self {
            phase,
            start_cpu_ticks,
            last_cpu_ticks: start_cpu_ticks,
            ticks_per_second,
        }
    }
    /// Updates the last CPU ticks with the given value.
    fn update(&mut self, cpu_ticks: f64) {
        self.last_cpu_ticks = cpu_ticks;
    }
    /// Calculates the CPU time based on the start and last CPU ticks.
    fn get_cpu_time(&self) -> f64 {
        (self.last_cpu_ticks - self.start_cpu_ticks) / self.ticks_per_second
    }
    /// Logs the CPU time summary.
    fn summary_trace(&mut self) {
        let total_cpu_time = self.get_cpu_time();
        tracing::info!(
            metric_name = "total_cpu_time",
            phase = &self.phase,
            total_cpu_time = total_cpu_time,
        );
        trace!("Total cpu time for {}: {}", &self.phase, total_cpu_time);
    }
}

/// `MemoryStat` is a struct that tracks memory usage metrics.
#[derive(Debug, Clone, Default)]
struct MemoryStat {
    phase: String,
    total_rss: u64,
    peak_rss: u64,
    number_measurements: u64,
}
impl MemoryStat {
    /// Creates a new `MemoryStat` instance with the given phase.
    fn new(phase: String) -> Self {
        Self {
            phase,
            total_rss: 0,
            peak_rss: 0,
            number_measurements: 0,
        }
    }
    /// Updates the memory usage metrics with the given RSS value.
    fn update(&mut self, rss: u64) {
        self.total_rss += rss;
        if rss > self.peak_rss {
            self.peak_rss = rss;
        }
        self.number_measurements += 1;
    }
    /// Calculates the average memory usage.
    fn get_average_memory_usage(&self) -> f64 {
        if self.number_measurements == 0 {
            return 0.0;
        }
        self.total_rss as f64 / self.number_measurements as f64
    }
    /// Returns the peak memory usage.
    fn get_peak_memory_usage(&self) -> u64 {
        self.peak_rss
    }
    /// Logs the memory usage summary.
    /// It logs the average and peak memory usage.
    fn summary_trace(&mut self) {
        let average_memory_usage = self.get_average_memory_usage();
        let peak_memory_usage = self.get_peak_memory_usage();
        tracing::info!(
            metric_name = "average_memory_usage",
            phase = &self.phase,
            average_memory_usage = average_memory_usage,
        );
        trace!(
            "Average memory usage for {}: {}",
            &self.phase,
            average_memory_usage
        );
        tracing::info!(
            metric_name = "peak_memory_usage",
            phase = &self.phase,
            peak_memory_usage = peak_memory_usage,
        );
        trace!(
            "Peak memory usage for {}: {}",
            &self.phase,
            peak_memory_usage
        );
    }
}

/// `NetworkStat` is a struct that tracks network usage metrics.
#[derive(Debug, Clone, Default)]
struct NetworkStat {
    phase: String,
    iface_start_bytes: HashMap<String, (u64, u64)>,
    iface_bytes: HashMap<String, (u64, u64)>,
}
impl NetworkStat {
    /// Creates a new `NetworkStat` instance with the given phase and initial network statistics.
    fn new(phase: String, init_stats: HashMap<String, DeviceStatus>) -> Self {
        let mut iface_start_bytes = HashMap::new();
        for stat in init_stats.values() {
            iface_start_bytes.insert(stat.name.clone(), (stat.recv_bytes, stat.sent_bytes));
        }

        Self {
            phase: phase.clone(),
            iface_start_bytes,
            iface_bytes: HashMap::new(),
        }
    }
    /// Updates the network usage metrics with the given interface name and received/sent bytes.
    /// It calculates the difference from the initial values and stores it in `iface_bytes`.
    /// If the interface name is not found in `iface_start_bytes`, it simply stores the received/sent bytes.
    fn update(&mut self, name: String, recv_bytes: u64, sent_bytes: u64) {
        let mut trace_measurement = (recv_bytes, sent_bytes);
        if let Some(start_bytes) = self.iface_start_bytes.get(&name) {
            trace_measurement = (recv_bytes - start_bytes.0, sent_bytes - start_bytes.1);
        }
        self.iface_bytes.insert(name.clone(), trace_measurement);
    }
    /// Logs the network usage summary for each interface.
    fn summary_trace(&mut self) {
        for (name, trace_measurement) in &self.iface_bytes {
            tracing::info!(
                metric_name = "total_network_usage",
                phase = &self.phase,
                iface_name = &name,
                rx_bytes = trace_measurement.0,
                tx_bytes = trace_measurement.1,
            );
            trace!(
                "Total network usage for {}: iface: {}, rx_bytes: {}, tx_bytes: {}",
                &self.phase,
                &name,
                trace_measurement.0,
                trace_measurement.1,
            );
        }
    }
}

impl MonitorMetrics {
    /// Creates a new `MonitorMetrics` instance that starts monitoring process and network metrics.
    /// The `phase` parameter is used to tag the logs with the current phase of the application.
    pub fn new(phase: String) -> Result<Self, Error> {
        let mut monitor = MonitorMetrics {
            stop: Arc::new(AtomicBool::new(false)),
            metric_count: Arc::new(AtomicU64::new(0)),
            join_handle: None,
        };
        monitor.start_monitoring(phase.clone())?;
        Ok(monitor)
    }

    /// Stops the monitoring thread by setting stop.
    /// This method is called when the `MonitorMetrics` instance is dropped.
    /// It ensures that the thread is stopped gracefully.
    pub fn stop(&mut self) -> Result<(), Error> {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    /// Joins the monitoring thread.
    #[allow(dead_code)]
    pub fn join(&mut self) -> thread::Result<()> {
        if let Some(h) = self.join_handle.take() {
            h.join()?;
        } else {
            error!("No monitoring thread to join");
        }
        Ok(())
    }

    /// Creates and starts the monitoring thread and returns the thread handle.
    /// The thread will run in a loop, updating the metrics at regular intervals.
    /// It will stop when stop is set.
    fn start_monitoring(&mut self, phase: String) -> Result<(), Error> {
        let polling_interval = Duration::from_millis(MONITORING_INTERVAL_MS);

        let local_stop = self.stop.clone();
        let local_metric_count = self.metric_count.clone();

        let process = Process::myself()?;
        let init_process_stats = process.stat()?;
        let init_net_stats = procfs::net::dev_status()?;

        let mut cpu_stat = CpuStat::new(
            phase.clone(),
            (init_process_stats.utime + init_process_stats.stime) as f64,
            ticks_per_second() as f64,
        );
        let mut memory_stat = MemoryStat::new(phase.clone());
        let mut network_stat = NetworkStat::new(phase.clone(), init_net_stats);

        let join_handle = thread::spawn(move || {
            loop {
                // Update CPU and memory statistics
                if let Ok(process) = Process::myself() {
                    if let Ok(stat) = process.stat() {
                        cpu_stat.update((stat.utime + stat.stime) as f64);
                        memory_stat.update(stat.rss);
                    }
                }
                // Update network statistics
                if let Ok(dev_stats) = procfs::net::dev_status() {
                    let stats: Vec<_> = dev_stats.values().collect();
                    for stat in stats {
                        network_stat.update(stat.name.clone(), stat.recv_bytes, stat.sent_bytes);
                    }
                }

                local_metric_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

                if local_stop.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }

                // Sleep for the polling interval
                thread::sleep(polling_interval);
            }
            // Perform summary trace for CPU, memory, and network metrics
            // after the monitoring thread is stopped.
            cpu_stat.summary_trace();
            memory_stat.summary_trace();
            network_stat.summary_trace();
        });

        self.join_handle = Some(join_handle);
        Ok(())
    }
}

impl Drop for MonitorMetrics {
    fn drop(&mut self) {
        if let Err(e) = self.stop() {
            trace!("Failed to stop monitoring threads: {:?}", e);
        }
    }
}

#[cfg(test)]
mod stat_tests {
    use super::*;
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };
    use tracing_subscriber::{layer::SubscriberExt, Registry};

    #[derive(Debug, Clone, Default)]
    struct TestTraceWriter {
        logs: Arc<Mutex<Vec<String>>>,
    }

    impl std::io::Write for TestTraceWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let s = String::from_utf8_lossy(buf).to_string();
            self.logs.lock().unwrap().push(s);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn init_monitor_metrics_tracing_validation_for_thread(
    ) -> (Arc<Mutex<Vec<String>>>, tracing::subscriber::DefaultGuard) {
        let logs = Arc::new(Mutex::new(Vec::new()));
        let writer = TestTraceWriter { logs: logs.clone() };
        let guard = tracing::subscriber::set_default(
            Registry::default().with(
                tracing_subscriber::fmt::layer()
                    .with_writer(move || writer.clone())
                    .with_ansi(false)
                    .with_target(false)
                    .with_level(true),
            ),
        );
        (logs, guard)
    }

    #[test]
    fn test_cpu_stat_update() {
        let phase = "test_phase".to_string();
        let start_cpu_ticks = 40.0;
        let ticks_per_second = 70.0;

        let mut cpu_stat = CpuStat::new(phase, start_cpu_ticks, ticks_per_second);
        assert_eq!(cpu_stat.last_cpu_ticks, start_cpu_ticks);
        let first_update = 200.0;
        cpu_stat.update(first_update);
        assert_eq!(cpu_stat.last_cpu_ticks, first_update);

        let last_update = 600.0;
        cpu_stat.update(last_update);
        assert_eq!(cpu_stat.last_cpu_ticks, last_update);

        assert_eq!(cpu_stat.start_cpu_ticks, start_cpu_ticks);
        assert_eq!(cpu_stat.last_cpu_ticks, last_update);
        assert_eq!(cpu_stat.ticks_per_second, ticks_per_second);

        assert_eq!(
            cpu_stat.get_cpu_time(),
            (last_update - start_cpu_ticks) / ticks_per_second
        );

        let (trace_logs, _guard) = init_monitor_metrics_tracing_validation_for_thread();
        cpu_stat.summary_trace();

        let logs = trace_logs.lock().unwrap().join("");
        println!("Trace logs: {}", logs);
        let expected_log = format!(
            "total_cpu_time={}",
            (last_update - start_cpu_ticks) / ticks_per_second
        );
        assert!(logs.contains(&expected_log));
    }

    #[test]
    fn test_memory_stat_initialization() {
        let phase = "test_phase".to_string();

        let memory_stat = MemoryStat::new(phase.clone());

        assert_eq!(memory_stat.phase, phase);
        assert_eq!(memory_stat.total_rss, 0);
        assert_eq!(memory_stat.peak_rss, 0);
        assert_eq!(memory_stat.number_measurements, 0);
    }

    #[test]
    fn test_memory_stat_update() {
        let phase = "test_phase".to_string();
        let mut memory_stat = MemoryStat::new(phase);

        let first_rss = 2048;
        memory_stat.update(first_rss);
        assert_eq!(memory_stat.total_rss, first_rss);
        assert_eq!(memory_stat.peak_rss, first_rss);
        assert_eq!(memory_stat.number_measurements, 1);

        let second_rss = 1024;
        memory_stat.update(second_rss);
        assert_eq!(memory_stat.total_rss, first_rss + second_rss);
        assert_eq!(memory_stat.peak_rss, first_rss);
        assert_eq!(memory_stat.number_measurements, 2);

        assert_eq!(
            memory_stat.get_average_memory_usage(),
            (first_rss + second_rss) as f64 / 2.0
        );
        assert_eq!(memory_stat.get_peak_memory_usage(), first_rss);

        let (trace_logs, _guard) = init_monitor_metrics_tracing_validation_for_thread();
        memory_stat.summary_trace();

        let logs = trace_logs.lock().unwrap().join("");
        println!("Trace logs: {}", logs);
        let expected_log = format!(
            "average_memory_usage={}",
            (first_rss + second_rss) as f64 / 2.0
        );
        assert!(logs.contains(&expected_log));
        let expected_log = format!("peak_memory_usage={}", first_rss);
        assert!(logs.contains(&expected_log));
    }

    fn create_mock_device_status(name: &str, recv_bytes: u64, sent_bytes: u64) -> DeviceStatus {
        DeviceStatus {
            name: name.to_string(),
            recv_bytes,
            sent_bytes,
            recv_packets: 0,
            recv_errs: 0,
            recv_drop: 0,
            recv_fifo: 0,
            recv_frame: 0,
            recv_compressed: 0,
            recv_multicast: 0,
            sent_packets: 0,
            sent_errs: 0,
            sent_drop: 0,
            sent_fifo: 0,
            sent_colls: 0,
            sent_carrier: 0,
            sent_compressed: 0,
        }
    }

    #[test]
    fn test_network_stat_initialization() {
        let phase = "test_phase".to_string();
        let mut init_stats = HashMap::new();
        let mock_device_status = create_mock_device_status("eth0", 1000, 2000);
        init_stats.insert("eth0".to_string(), mock_device_status);

        let network_stat = NetworkStat::new(phase.clone(), init_stats);

        assert_eq!(network_stat.phase, phase);
        assert!(network_stat.iface_start_bytes.contains_key("eth0"));
        assert_eq!(network_stat.iface_start_bytes["eth0"], (1000, 2000));
    }

    #[test]
    fn test_network_stat_update() {
        let phase = "test_phase".to_string();
        let mut init_stats = HashMap::new();
        let mock_device_status = create_mock_device_status("eth0", 1000, 2000);
        init_stats.insert("eth0".to_string(), mock_device_status);

        let mut network_stat = NetworkStat::new(phase, init_stats);
        network_stat.update("eth0".to_string(), 3000, 4000);
        assert!(network_stat.iface_bytes.contains_key("eth0"));
        assert_eq!(network_stat.iface_bytes["eth0"], (2000, 2000));
        network_stat.update("eth1".to_string(), 5000, 6000);
        assert!(network_stat.iface_bytes.contains_key("eth1"));
        assert_eq!(network_stat.iface_bytes["eth1"], (5000, 6000));
        assert!(network_stat.iface_bytes.contains_key("eth0"));
        assert_eq!(network_stat.iface_bytes["eth0"], (2000, 2000));

        let (trace_logs, _guard) = init_monitor_metrics_tracing_validation_for_thread();
        network_stat.summary_trace();

        let logs = trace_logs.lock().unwrap().join("");
        println!("Trace logs: {}", logs);
        let expected_log = "iface_name=\"eth0\" rx_bytes=2000 tx_bytes=2000".to_string();
        assert!(logs.contains(&expected_log));
        let expected_log = "iface_name=\"eth1\" rx_bytes=5000 tx_bytes=6000".to_string();
        assert!(logs.contains(&expected_log));
    }

    #[test]
    fn test_stop() {
        let monitor = MonitorMetrics {
            stop: Arc::new(AtomicBool::new(false)),
            metric_count: Arc::new(AtomicU64::new(0)),
            join_handle: None,
        };
        assert!(!monitor.stop.load(std::sync::atomic::Ordering::SeqCst));
        monitor
            .stop
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(monitor.stop.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            monitor
                .metric_count
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }

    #[test]
    fn test_join() {
        let mut monitor = MonitorMetrics {
            stop: Arc::new(AtomicBool::new(false)),
            metric_count: Arc::new(AtomicU64::new(0)),
            join_handle: None,
        };
        assert!(monitor.join_handle.is_none());
        // Validate that join without handle returns Ok
        assert!(monitor.join().is_ok());

        // Create thread and validate that join works
        let started = Arc::new(AtomicBool::new(false));
        let ended = Arc::new(AtomicBool::new(false));
        let thread_started = started.clone();
        let thread_ended = ended.clone();
        monitor.join_handle = Some(thread::spawn(move || {
            thread_started.store(true, std::sync::atomic::Ordering::SeqCst);
            // Simulate some work
            thread::sleep(Duration::from_millis(100));
            thread_ended.store(true, std::sync::atomic::Ordering::SeqCst);
        }));
        // Validate that join with handle returns Ok
        assert!(monitor.join().is_ok());
        assert!(started.load(std::sync::atomic::Ordering::SeqCst));
        assert!(ended.load(std::sync::atomic::Ordering::SeqCst));
        assert!(monitor.join_handle.is_none());
        // Validate that join without handle returns Ok
        assert!(monitor.join().is_ok());
    }

    #[test]
    fn test_start_monitoring() {
        let mut monitor = MonitorMetrics {
            // Initialize monitor with stop=true, forcing thead to
            // collect 1 set of metrics and exit
            stop: Arc::new(AtomicBool::new(true)),
            metric_count: Arc::new(AtomicU64::new(0)),
            join_handle: None,
        };

        monitor.start_monitoring("test-phase".to_string()).unwrap();
        assert!(monitor.join_handle.is_some());

        // Validate that join with handle returns Ok
        assert!(monitor.join().is_ok());
        assert!(monitor.join_handle.is_none());
        // Validate that join without handle returns Ok
        assert!(monitor.join().is_ok());
        assert_eq!(
            monitor
                .metric_count
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::{layer::SubscriberExt, Registry};

    #[derive(Debug, Clone, Default)]
    struct TestTraceWriter {
        logs: Arc<Mutex<Vec<String>>>,
    }

    impl std::io::Write for TestTraceWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let s = String::from_utf8_lossy(buf).to_string();
            self.logs.lock().unwrap().push(s);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn init_monitor_metrics_tracing_validation_global() -> Arc<Mutex<Vec<String>>> {
        let logs = Arc::new(Mutex::new(Vec::new()));
        let writer = TestTraceWriter { logs: logs.clone() };
        assert!(tracing::subscriber::set_global_default(
            Registry::default().with(
                tracing_subscriber::fmt::layer()
                    .with_writer(move || writer.clone())
                    .with_ansi(false)
                    .with_target(false)
                    .with_level(true),
            ),
        )
        .is_ok());
        logs
    }

    #[functional_test]
    fn test_monitor_metrics() {
        let trace_logs = init_monitor_metrics_tracing_validation_global();

        let mut test_metrics = MonitorMetrics::new("test_phase".to_string()).unwrap();

        // Wait for a while to allow the thread to run and collect metrics
        let sleep_ms = 1000;
        thread::sleep(Duration::from_millis(sleep_ms));

        // Tell monitor loop to stop
        test_metrics.stop().unwrap();

        // Join the thread to wait monitor loop to end
        test_metrics.join().unwrap();

        // Loop is stopped after X ms, each iteration waits Y ms, check
        // that metric count is roughly (maybe within 20%) between 0 and
        // (X / Y)
        assert_ne!(
            test_metrics
                .metric_count
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert!(
            test_metrics
                .metric_count
                .load(std::sync::atomic::Ordering::SeqCst)
                <= (1.2 * sleep_ms as f64 / MONITORING_INTERVAL_MS as f64) as u64
        );

        // Validate that tracing output contains expected metrics
        let logs = trace_logs.lock().unwrap().join("");
        assert!(logs.contains("total_cpu_time"));
        assert!(logs.contains("average_memory_usage"));
        assert!(logs.contains("peak_memory_usage"));
        assert!(logs.contains("total_network_usage"));
    }
}
