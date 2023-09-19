use log::{error, info, warn};
use serde::{Deserialize, Serialize};

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
}

pub struct OrchestratorConnection {
    url: String,
}
impl OrchestratorConnection {
    /// Attempt to connect to the orchestrator, and return a connection if successful.
    pub fn new(url: String) -> Option<Self> {
        info!("Reporting status to orchestrator at {}", url);
        for (i, sleep) in [100, 200, 400, 800, 1000, 1000, 1000, 0]
            .into_iter()
            .enumerate()
        {
            if reqwest::blocking::Client::new()
                .post(&url)
                .body(
                    serde_json::to_vec(&Message {
                        state: State::Started,
                        message: format!("trident started (connection attempt {i})"),
                    })
                    .unwrap(),
                )
                .send()
                .map(|r| r.status().is_success())
                .unwrap_or(false)
            {
                info!("Connected to orchestrator");
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

    pub fn report_error(&self, message: String) {
        self.send_message(Message {
            state: State::Failed,
            message,
        });
    }

    pub fn report_success(&self) {
        info!("Reporting provisioning succeeded");
        self.send_message(Message {
            state: State::Succeeded,
            message: "provisioning succeeded".to_string(),
        })
    }
}
