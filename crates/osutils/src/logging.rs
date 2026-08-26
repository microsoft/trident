use log::{LevelFilter, Log, Metadata, Record};

/// A `log::Log` wrapper that applies a separate level filter to a configurable
/// list of noisy "network" targets (e.g. HTTP/gRPC/watch client crates) while
/// leaving every other target at a main verbosity level.
///
/// This is useful for binaries that talk to chatty client stacks (hyper, h2,
/// tonic, kube, reqwest, ...) whose per-frame/per-request logging would
/// otherwise drown out the binary's own orchestration logs at the same
/// verbosity.
pub struct FilteredLogger<L> {
    inner: L,
    verbosity: LevelFilter,
    network_verbosity: LevelFilter,
    network_targets: &'static [&'static str],
}

impl<L: Log> FilteredLogger<L> {
    /// Builds a new [`FilteredLogger`] wrapping `inner`. Targets in
    /// `network_targets` (matched by prefix, e.g. `"hyper"` matches
    /// `hyper::client`) are filtered at `network_verbosity`; every other
    /// target is filtered at `verbosity`.
    pub fn new(
        inner: L,
        verbosity: LevelFilter,
        network_verbosity: LevelFilter,
        network_targets: &'static [&'static str],
    ) -> Self {
        Self {
            inner,
            verbosity,
            network_verbosity,
            network_targets,
        }
    }

    /// The maximum of `verbosity` and `network_verbosity`, suitable for
    /// passing to [`log::set_max_level`] so the log facade doesn't drop
    /// records before they reach this filter.
    pub fn max_level(&self) -> LevelFilter {
        self.verbosity.max(self.network_verbosity)
    }

    fn is_network_target(&self, target: &str) -> bool {
        self.network_targets.iter().any(|prefix| {
            target
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
        })
    }
}

impl<L: Log> Log for FilteredLogger<L> {
    fn enabled(&self, metadata: &Metadata) -> bool {
        let level = if self.is_network_target(metadata.target()) {
            self.network_verbosity
        } else {
            self.verbosity
        };
        metadata.level() <= level && self.inner.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            self.inner.log(record);
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};

    use log::{Level, Metadata, Record};

    #[derive(Clone)]
    struct TestLogger {
        logged: Arc<Mutex<Vec<String>>>,
    }

    impl TestLogger {
        fn new() -> Self {
            Self {
                logged: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl Log for TestLogger {
        fn enabled(&self, _metadata: &Metadata) -> bool {
            true
        }

        fn log(&self, record: &Record) {
            self.logged
                .lock()
                .unwrap()
                .push(format!("{} {}", record.target(), record.args()));
        }

        fn flush(&self) {}
    }

    /// A logger whose `enabled()` always returns `false`, to verify
    /// `FilteredLogger` honors the inner logger's own filter rather than
    /// bypassing it once its own level/target filter passes.
    struct AlwaysDisabledLogger;

    impl Log for AlwaysDisabledLogger {
        fn enabled(&self, _metadata: &Metadata) -> bool {
            false
        }

        fn log(&self, _record: &Record) {
            panic!("log() must not be called when enabled() is false");
        }

        fn flush(&self) {}
    }

    const NETWORK_TARGETS: &[&str] = &["hyper", "kube"];

    #[test]
    fn test_network_target_uses_network_verbosity() {
        let inner = TestLogger::new();
        let logged = inner.logged.clone();
        let logger = FilteredLogger::new(
            inner,
            LevelFilter::Debug,
            LevelFilter::Warn,
            NETWORK_TARGETS,
        );

        assert!(logger.enabled(
            &Metadata::builder()
                .level(Level::Warn)
                .target("hyper::client")
                .build()
        ));
        assert!(!logger.enabled(
            &Metadata::builder()
                .level(Level::Debug)
                .target("hyper::client")
                .build()
        ));
        drop(logged);
    }

    #[test]
    fn test_non_network_target_uses_verbosity() {
        let inner = TestLogger::new();
        let logger = FilteredLogger::new(
            inner,
            LevelFilter::Debug,
            LevelFilter::Warn,
            NETWORK_TARGETS,
        );

        assert!(logger.enabled(
            &Metadata::builder()
                .level(Level::Debug)
                .target("trident_acl_agent")
                .build()
        ));
        assert!(!logger.enabled(
            &Metadata::builder()
                .level(Level::Trace)
                .target("trident_acl_agent")
                .build()
        ));
    }

    #[test]
    fn test_inner_enabled_is_respected() {
        let logger = FilteredLogger::new(
            AlwaysDisabledLogger,
            LevelFilter::Debug,
            LevelFilter::Debug,
            NETWORK_TARGETS,
        );

        let metadata = Metadata::builder()
            .level(Level::Error)
            .target("trident_acl_agent")
            .build();

        // FilteredLogger's own filter passes (Error <= Debug), but the inner
        // logger's enabled() returns false, so the combined result must too.
        assert!(!logger.enabled(&metadata));

        // log() must therefore be a no-op (AlwaysDisabledLogger panics if
        // its log() is ever reached).
        logger.log(
            &Record::builder()
                .metadata(metadata)
                .args(format_args!("should not be logged"))
                .build(),
        );
    }

    #[test]
    fn test_max_level_is_max_of_both() {
        let logger = FilteredLogger::new(
            TestLogger::new(),
            LevelFilter::Warn,
            LevelFilter::Debug,
            NETWORK_TARGETS,
        );
        assert_eq!(logger.max_level(), LevelFilter::Debug);
    }
}
