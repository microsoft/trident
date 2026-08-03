use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    time::Duration,
};

use reqwest::Client;
use tokio::time::sleep;
use url::Url;

use trident_acl_agent_tester::{
    apiserver, rp,
    scenario::{DurationDef, ExpectStep, PatchStep, Scenario, ScenarioStep},
};

#[tokio::test]
async fn rp_scenario_runner_observes_staged_transition() {
    let handle = apiserver::spawn(
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
        "node-a",
        BTreeMap::new(),
    )
    .await
    .unwrap();

    let store = handle.store();
    tokio::spawn(async move {
        sleep(Duration::from_millis(250)).await;
        store
            .patch_labels(BTreeMap::from([
                (
                    "kubernetes.azure.com/trident-abupdate-state".to_string(),
                    "staged".to_string(),
                ),
                (
                    "kubernetes.azure.com/trident-abupdate-observed-request-id".to_string(),
                    "R1".to_string(),
                ),
            ]))
            .await
            .unwrap();
    });

    let scenario = Scenario {
        steps: vec![
            ScenarioStep::Patch {
                patch: PatchStep {
                    request: Some("stage".to_string()),
                    request_id: Some("R1".to_string()),
                    target_os_image_version: Some("202507.28.0".to_string()),
                },
            },
            ScenarioStep::Expect {
                expect: ExpectStep {
                    state: "staged".to_string(),
                    observed_request_id: Some("R1".to_string()),
                    timeout: DurationDef(Duration::from_secs(5)),
                    expect_timeout: false,
                },
            },
        ],
    };

    let report = rp::run_scenario(
        &Client::new(),
        &Url::parse(&handle.url()).unwrap(),
        "node-a",
        &scenario,
    )
    .await
    .unwrap();

    assert!(
        report.passed,
        "report: {}",
        serde_json::to_string(&report).unwrap()
    );
    handle.shutdown().await;
}

#[tokio::test]
async fn rp_scenario_runner_checks_failure_reason() {
    let handle = apiserver::spawn(
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
        "node-b",
        BTreeMap::new(),
    )
    .await
    .unwrap();

    handle
        .store()
        .patch_labels(BTreeMap::from([(
            "kubernetes.azure.com/trident-abupdate-failure-reason".to_string(),
            "version-mismatch".to_string(),
        )]))
        .await
        .unwrap();

    let scenario = Scenario {
        steps: vec![ScenarioStep::AssertFailureReason {
            assert_failure_reason: "version-mismatch".to_string(),
        }],
    };

    let report = rp::run_scenario(
        &Client::new(),
        &Url::parse(&handle.url()).unwrap(),
        "node-b",
        &scenario,
    )
    .await
    .unwrap();

    assert!(
        report.passed,
        "report: {}",
        serde_json::to_string(&report).unwrap()
    );
    handle.shutdown().await;
}
