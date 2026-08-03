//! Scenario model and report types for the RP proxy runner.

use std::{collections::BTreeMap, path::Path, time::Duration};

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct Scenario {
    pub steps: Vec<ScenarioStep>,
}

impl Scenario {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read scenario {}", path.display()))?;
        serde_yaml::from_str(&contents).context("failed to parse scenario yaml")
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ScenarioStep {
    Patch {
        patch: PatchStep,
    },
    Expect {
        expect: ExpectStep,
    },
    AssertFailureReason {
        #[serde(rename = "assert-failure-reason")]
        assert_failure_reason: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct PatchStep {
    pub request: Option<String>,
    #[serde(rename = "request-id")]
    pub request_id: Option<String>,
    #[serde(rename = "target-os-image-version")]
    pub target_os_image_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExpectStep {
    pub state: String,
    #[serde(rename = "observed-request-id")]
    pub observed_request_id: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout: DurationDef,
    #[serde(default)]
    pub expect_timeout: bool,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(transparent)]
pub struct DurationDef(#[serde(deserialize_with = "deserialize_duration")] pub Duration);

impl Default for DurationDef {
    fn default() -> Self {
        Self(default_timeout().0)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ScenarioReport {
    pub passed: bool,
    pub steps: Vec<StepReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StepReport {
    pub index: usize,
    pub kind: String,
    pub passed: bool,
    pub elapsed_ms: u128,
    pub message: String,
    pub expected: Option<serde_json::Value>,
    pub actual: Option<serde_json::Value>,
}

fn default_timeout() -> DurationDef {
    DurationDef(Duration::from_secs(60))
}

fn deserialize_duration<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    humantime::parse_duration(&raw).map_err(serde::de::Error::custom)
}

pub fn labels_from_patch_step(step: &PatchStep) -> BTreeMap<String, String> {
    let mut labels = BTreeMap::new();
    if let Some(request) = &step.request {
        labels.insert(
            "kubernetes.azure.com/trident-abupdate-request".to_string(),
            request.clone(),
        );
    }
    if let Some(request_id) = &step.request_id {
        labels.insert(
            "kubernetes.azure.com/trident-abupdate-request-id".to_string(),
            request_id.clone(),
        );
    }
    if let Some(target) = &step.target_os_image_version {
        labels.insert(
            "kubernetes.azure.com/trident-abupdate-target-os-image-version".to_string(),
            target.clone(),
        );
    }
    labels
}
