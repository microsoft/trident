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
            max_level: LevelFilter::Debug,
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
