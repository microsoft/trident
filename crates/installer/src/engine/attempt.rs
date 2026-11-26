//! Coordinates one live installation attempt independently of presentation.

use std::path::PathBuf;

use anyhow::Error;
use tokio::{
    sync::mpsc,
    task::{self, JoinHandle},
};

use crate::{
    client::{self, AttemptOutcome, Event},
    config::Config,
    ipmi::LiveAttemptReporter,
    source::Request,
};

/// Starts one live attempt and returns the task that owns its lifecycle.
///
/// The task reloads installer configuration, emits best-effort IPMI start and terminal events,
/// and forwards operation events through `tx`. Configuration and servicing errors produce handled
/// [`AttemptOutcome::Failed`] results. Exiting without a handled outcome causes the IPMI reporter
/// to emit `Failed` when the task future is dropped.
pub(super) fn spawn(
    config_path: PathBuf,
    request: Request,
    uncertain: bool,
    tx: mpsc::Sender<Event>,
) -> JoinHandle<Result<AttemptOutcome, Error>> {
    task::spawn(async move {
        let mut reporter = LiveAttemptReporter::new();
        reporter.start();
        match Config::read(config_path) {
            Ok(settings) => {
                client::execute(settings, request, uncertain, tx, |outcome| {
                    reporter.finish(outcome);
                })
                .await
            }
            Err(error) => {
                reporter.finish(AttemptOutcome::Failed);
                client::send(
                    &tx,
                    Event::Error {
                        details: format!("{error:#}"),
                        uncertain,
                    },
                )
                .await?;
                Ok(AttemptOutcome::Failed)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use tempfile::TempDir;

    /// Invalid settings must fail before source selection, rather than fall back to defaults.
    #[tokio::test]
    async fn reloads_configuration_without_using_defaults_on_error() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("installer.toml");
        for (text, diagnostic) in [("", "mode"), ("mode = 'not-a-mode'", "not-a-mode")] {
            fs::write(&path, text).unwrap();
            let (tx, mut rx) = mpsc::channel(1);
            let worker = spawn(path.clone(), Request::Autorun, false, tx);
            let Event::Error { details, uncertain } = rx.recv().await.unwrap() else {
                panic!("Invalid settings must fail before preparing a source");
            };
            assert!(!uncertain);
            assert!(details.contains(diagnostic), "{details}");
            assert_eq!(worker.await.unwrap().unwrap(), AttemptOutcome::Failed);
            rx.try_recv().unwrap_err();
        }
    }
}
