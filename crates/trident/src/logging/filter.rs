use log::{LevelFilter, Log};

pub struct LogFilter<T>
where
    T: Log,
{
    inner: T,
    max_level: LevelFilter,
    module_filters: Vec<(String, LevelFilter)>,
}

impl<T> LogFilter<T>
where
    T: Log,
{
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            max_level: LevelFilter::Trace,
            module_filters: Vec::new(),
        }
    }

    #[allow(dead_code)]
    pub fn with_max_level(mut self, max_level: LevelFilter) -> Self {
        self.max_level = max_level;
        self
    }

    #[allow(dead_code)]
    pub fn into_inner(self) -> T {
        self.inner
    }

    pub fn with_global_filter(mut self, target: impl Into<String>, max_level: LevelFilter) -> Self {
        self.module_filters.push((target.into(), max_level));
        self
    }

    pub fn into_logger(self) -> Box<Self> {
        Box::new(self)
    }

    /// Returns whether a specific log should be dropped based on the global
    /// filters.
    fn should_drop(&self, metadata: &log::Metadata) -> bool {
        metadata.level() > self.max_level
            || self.module_filters.iter().any(|(target, max_level)| {
                // Check if target matches a filter and the log level is more
                // verbose than the max level.
                metadata.target().starts_with(target) && metadata.level() > *max_level
            })
    }
}

impl<T> Log for LogFilter<T>
where
    T: Log,
{
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        if self.should_drop(metadata) {
            return false;
        }

        self.inner.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        if self.should_drop(record.metadata()) {
            return;
        }

        self.inner.log(record);
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};

    use log::{Level, LevelFilter, Metadata, Record};

    #[derive(Clone)]
    struct TestLogger {
        logged_records: Arc<Mutex<Vec<String>>>,
        flushed: Arc<Mutex<bool>>,
    }
    impl TestLogger {
        fn new() -> Self {
            Self {
                logged_records: Arc::new(Mutex::new(Vec::new())),
                flushed: Arc::new(Mutex::new(false)),
            }
        }

        fn get_last_record(&self) -> Option<String> {
            self.logged_records.lock().unwrap().last().cloned()
        }

        fn is_flushed(&self) -> bool {
            *self.flushed.lock().unwrap()
        }

        fn clear(&mut self) {
            self.logged_records.lock().unwrap().clear();
            *self.flushed.lock().unwrap() = false;
        }
    }
    impl Log for TestLogger {
        fn enabled(&self, _metadata: &Metadata) -> bool {
            true
        }

        fn log(&self, record: &Record) {
            self.logged_records.lock().unwrap().push(format!(
                "{} - {}",
                record.level(),
                record.args()
            ));
        }

        fn flush(&self) {
            *self.flushed.lock().unwrap() = true;
        }
    }

    #[test]
    fn test_log_filter() {
        let mut test_logger = TestLogger::new();
        let log_filter = LogFilter::new(test_logger.clone())
            .with_max_level(LevelFilter::Info)
            .with_global_filter("filtered_module", LevelFilter::Warn)
            .into_logger();

        // Ensure flush works
        assert!(!test_logger.is_flushed());
        log_filter.flush();
        assert!(test_logger.is_flushed());
        test_logger.clear();

        // Log that should pass the filter
        let record_info = Record::builder()
            .args(format_args!("Info message"))
            .level(Level::Info)
            .target("mu_module")
            .build();

        assert!(
            log_filter.enabled(record_info.metadata()),
            "Info level should be enabled"
        );
        log_filter.log(&record_info);
        assert_eq!(
            test_logger.get_last_record(),
            Some("INFO - Info message".to_string())
        );
        test_logger.clear();

        // Log that should be filtered out by the global max level
        let record_debug = Record::builder()
            .args(format_args!("Debug message"))
            .level(Level::Debug)
            .target("my_module")
            .build();
        assert!(
            !log_filter.enabled(record_debug.metadata()),
            "Debug level should be filtered out by max level"
        );
        log_filter.log(&record_debug);
        assert_eq!(
            test_logger.get_last_record(),
            None,
            "Debug message should be filtered out"
        );

        // Log that should be filtered out by the module-specific filter
        let record_info_filtered = Record::builder()
            .args(format_args!("Info message in filtered module"))
            .level(Level::Info)
            .target("filtered_module::submodule")
            .build();
        assert!(
            !log_filter.enabled(record_info_filtered.metadata()),
            "Info level in filtered module should be filtered out"
        );
        log_filter.log(&record_info_filtered);
        assert_eq!(
            test_logger.get_last_record(),
            None,
            "Info message in filtered module should be filtered out"
        );

        // Log that should pass the module-specific filter
        let record_warn_filtered = Record::builder()
            .args(format_args!("Warn message in filtered module"))
            .level(Level::Warn)
            .target("filtered_module::submodule")
            .build();
        assert!(
            log_filter.enabled(record_warn_filtered.metadata()),
            "Warn level in filtered module should be enabled"
        );
        log_filter.log(&record_warn_filtered);
        assert_eq!(
            test_logger.get_last_record(),
            Some("WARN - Warn message in filtered module".to_string())
        );
        test_logger.clear();

        // Check that logger conversion works ok
        let logger = log_filter.into_logger();

        // Log that should pass the filter
        let record_error = Record::builder()
            .args(format_args!("Error message"))
            .level(Level::Error)
            .target("any_module")
            .build();
        assert!(
            logger.enabled(record_error.metadata()),
            "Error level should be enabled"
        );
        logger.log(&record_error);
        assert_eq!(
            test_logger.get_last_record(),
            Some("ERROR - Error message".to_string())
        );

        drop(logger);
    }
}
