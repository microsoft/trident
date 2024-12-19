use log::{debug, error, warn};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Timeout in seconds for connecting to the orchestrator.
pub const ORCHESTRATOR_CONNECTION_TIMEOUT_SECONDS: u16 = 20;

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
enum State {
    Started,
    Failed,
    Succeeded,
}

#[derive(Serialize, Deserialize, Debug)]
struct Message {
    state: State,
    message: String,
    host_status: Option<String>,
}

pub struct OrchestratorConnection {
    url: String,
}
impl OrchestratorConnection {
    /// Attempt to connect to the orchestrator, and return a connection if successful.
    pub fn new(url: String, connection_timeout_secs: Option<u16>) -> Option<Self> {
        let timeout_duration = Duration::from_secs(
            connection_timeout_secs
                .unwrap_or(ORCHESTRATOR_CONNECTION_TIMEOUT_SECONDS)
                .into(),
        );
        let start_time = Instant::now();
        let sleep = 100;

        debug!(
            "Reporting status to orchestrator at {}, attempt connection for {} seconds",
            url,
            timeout_duration.as_secs()
        );
        for i in 0.. {
            if start_time.elapsed() >= timeout_duration {
                break;
            }

            if reqwest::blocking::Client::new()
                .post(&url)
                .body(
                    serde_json::to_vec(&Message {
                        state: State::Started,
                        message: format!("Trident started (connection attempt {i})"),
                        host_status: None,
                    })
                    .unwrap(),
                )
                .send()
                .map(|r| r.status().is_success())
                .unwrap_or(false)
            {
                debug!("Connected to orchestrator");
                return Some(Self { url });
            }
            std::thread::sleep(std::time::Duration::from_millis(sleep));
        }

        warn!("Failed to connect to orchestrator");
        None
    }

    fn send_message(&self, message: Message) {
        if let Err(e) = reqwest::blocking::Client::new()
            .post(&self.url)
            .body(serde_json::to_vec(&message).unwrap())
            .send()
        {
            error!("Orchestrator connection lost: {}", e);
        }
    }

    pub fn report_error(&self, error: String, host_status: Option<String>) {
        self.send_message(Message {
            state: State::Failed,
            message: error,
            host_status,
        });
    }

    pub fn report_success(&self, host_status: Option<String>) {
        debug!("Reporting provisioning succeeded");
        self.send_message(Message {
            state: State::Succeeded,
            message: "provisioning succeeded".to_string(),
            host_status,
        })
    }
}
