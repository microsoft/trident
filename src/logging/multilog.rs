use log::{LevelFilter, Log};

pub struct MultiLogger {
    loggers: Vec<Box<dyn Log>>,
    max_level: LevelFilter,
}

impl Default for MultiLogger {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiLogger {
    pub fn new() -> Self {
        Self {
            loggers: Vec::new(),
            max_level: LevelFilter::Trace,
        }
    }

    pub fn with_logger(mut self, logger: Box<dyn Log>) -> Self {
        self.loggers.push(logger);
        self
    }

    pub fn with_max_level(mut self, max_level: LevelFilter) -> Self {
        self.max_level = max_level;
        self
    }

    pub fn add_logger(&mut self, logger: Box<dyn Log>) {
        self.loggers.push(logger);
    }

    pub fn init(self) -> Result<(), log::SetLoggerError> {
        log::set_max_level(self.max_level);
        log::set_boxed_logger(Box::new(self))
    }
}

impl Log for MultiLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.loggers.iter().any(|l| l.enabled(metadata))
    }

    fn log(&self, record: &log::Record) {
        self.loggers
            .iter()
            .filter(|l| l.enabled(record.metadata()))
            .for_each(|l| l.log(record));
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
}
