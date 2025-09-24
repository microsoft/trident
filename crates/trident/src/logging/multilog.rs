use log::{LevelFilter, Log};

pub struct MultiLogger {
    loggers: Vec<Box<dyn Log>>,
    max_level: LevelFilter,
    global_filter: Vec<(String, LevelFilter)>,
}

impl Default for MultiLogger {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiLogger {
    /// Create a new MultiLogger.
    pub fn new() -> Self {
        Self {
            loggers: Vec::new(),
            max_level: LevelFilter::Trace,
            global_filter: Vec::new(),
        }
    }

    /// Add a logger to the MultiLogger.
    pub fn with_logger(mut self, logger: Box<dyn Log>) -> Self {
        self.loggers.push(logger);
        self
    }

    /// Set the max log level for the MultiLogger.
    pub fn with_max_level(mut self, max_level: LevelFilter) -> Self {
        self.max_level = max_level;
        self
    }

    /// Add a global filter to the logger.
    ///
    /// The filter will be applied to all loggers that have a target that starts
    /// with the given string. The logs will be dropped if the log level is
    /// more verbose than the given max level.
    pub fn with_global_filter(mut self, target: impl Into<String>, max_level: LevelFilter) -> Self {
        self.global_filter.push((target.into(), max_level));
        self
    }

    /// Add a logger to the MultiLogger.
    pub fn add_logger(&mut self, logger: Box<dyn Log>) {
        self.loggers.push(logger);
    }

    /// Initialize the multi logger by setting it as the global logger.
    pub fn init(self) -> Result<(), log::SetLoggerError> {
        log::set_max_level(self.max_level);
        log::set_boxed_logger(Box::new(self))
    }

    /// Returns whether a specific log should be dropped based on the global
    /// filters.
    fn should_drop(&self, metadata: &log::Metadata) -> bool {
        self.global_filter.iter().any(|(target, max_level)| {
            // Check if target matches a filter and the log level is more
            // verbose than the max level.
            metadata.target().starts_with(target) && metadata.level() > *max_level
        })
    }
}

impl Log for MultiLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        // Check if the log is filtered out by the global filter.
        if self.should_drop(metadata) {
            // Target matched a filter and the log level is more verbose than
            // the max level so the log should be dropped.
            return false;
        }

        // Check if any of the loggers are enabled for this record.
        self.loggers.iter().any(|l| l.enabled(metadata))
    }

    fn log(&self, record: &log::Record) {
        if !self.should_drop(record.metadata()) {
            self.loggers
                .iter()
                .filter(|l| l.enabled(record.metadata()))
                .for_each(|l| l.log(record));
        }
    }

    fn flush(&self) {
        self.loggers.iter().for_each(|l| l.flush());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    use log::Level;

    #[derive(Default)]
    struct TestLogger {
        enabled: bool,
        got_logs: Arc<AtomicBool>,
    }

    impl Log for TestLogger {
        fn enabled(&self, _: &log::Metadata) -> bool {
            self.enabled
        }

        fn log(&self, _: &log::Record) {
            self.got_logs.store(true, Ordering::Relaxed);
        }

        fn flush(&self) {
            // No-op
        }
    }

    #[test]
    fn test_enabled() {
        let logger1 = Box::new(TestLogger {
            enabled: false,
            ..Default::default()
        });
        let logger2 = Box::new(TestLogger {
            enabled: false,
            ..Default::default()
        });

        let multi_logger = MultiLogger::new().with_logger(logger1).with_logger(logger2);

        assert!(
            !multi_logger.enabled(&log::Metadata::builder().level(Level::Error).build()),
            "Logger should not be enabled"
        );

        let logger1 = Box::new(TestLogger {
            enabled: false,
            ..Default::default()
        });
        let logger2 = Box::new(TestLogger {
            enabled: true,
            ..Default::default()
        });

        let multi_logger = MultiLogger::new().with_logger(logger1).with_logger(logger2);

        assert!(
            multi_logger.enabled(&log::Metadata::builder().level(Level::Error).build()),
            "Logger should be enabled"
        );
    }

    #[test]
    fn test_filter() {
        let logger1 = Box::new(TestLogger {
            enabled: true,
            ..Default::default()
        });
        let logger1_state = logger1.got_logs.clone();
        let logger2 = Box::new(TestLogger {
            enabled: false,
            ..Default::default()
        });
        let logger2_state = logger2.got_logs.clone();

        let multi_logger = MultiLogger::new()
            .with_logger(logger1)
            .with_logger(logger2)
            .with_max_level(LevelFilter::Info);

        multi_logger.log(&log::Record::builder().build());

        assert!(
            logger1_state.load(Ordering::Relaxed),
            "Logger 1 should have received the log"
        );
        assert!(
            !logger2_state.load(Ordering::Relaxed),
            "Logger 2 should not have received the log"
        );
    }

    #[test]
    fn test_global_filter() {
        let logger = Box::new(TestLogger {
            enabled: true,
            ..Default::default()
        });
        let logger_state = logger.got_logs.clone();

        let multi_logger = MultiLogger::new()
            .with_logger(logger)
            .with_global_filter("myModule", LevelFilter::Info);

        // Send some other logs
        multi_logger.log(
            &log::Record::builder()
                .target("other::module")
                .level(Level::Trace)
                .build(),
        );

        assert!(
            logger_state.load(Ordering::Relaxed),
            "Logger should have received the other log"
        );

        // Reset the state
        logger_state.store(false, Ordering::Relaxed);

        // Trace should be blocked by the global filter
        multi_logger.log(
            &log::Record::builder()
                .target("myModule::module")
                .level(Level::Trace)
                .build(),
        );

        assert!(
            !logger_state.load(Ordering::Relaxed),
            "Logger should not have received the trace log"
        );

        // Debug should be blocked by the global filter
        multi_logger.log(
            &log::Record::builder()
                .target("myModule::module")
                .level(Level::Debug)
                .build(),
        );

        assert!(
            !logger_state.load(Ordering::Relaxed),
            "Logger should not have received the debug log"
        );

        // Info should be allowed by the global filter
        multi_logger.log(
            &log::Record::builder()
                .target("myModule::module")
                .level(Level::Info)
                .build(),
        );

        assert!(
            logger_state.load(Ordering::Relaxed),
            "Logger should have received the info log"
        );
    }
}
