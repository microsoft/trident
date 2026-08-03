use std::{collections::BTreeMap, net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::Context;
use clap::{Parser, Subcommand};
use reqwest::Client;
use url::Url;

use trident_acl_agent_tester::{
    apiserver, kubelet,
    nebraska::{self, NebraskaScenario},
    rp,
    scenario::Scenario,
};

#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run the fake single-node Kubernetes apiserver.
    Apiserver {
        #[arg(long, default_value = "127.0.0.1:18080")]
        listen: SocketAddr,
        #[arg(long, default_value = "trident-node")]
        node_name: String,
        #[arg(long)]
        seed_labels: Option<String>,
        #[arg(long)]
        seed_file: Option<PathBuf>,
    },
    /// Run an AKS-RP scenario against the fake apiserver.
    RpProxy {
        #[arg(long)]
        apiserver_url: Url,
        #[arg(long, default_value = "trident-node")]
        node_name: String,
        #[arg(long)]
        scenario: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Simulate kubelet bootstrap labels and reboot boundary readiness flips.
    KubeletProxy {
        #[arg(long)]
        apiserver_url: Url,
        #[arg(long, default_value = "trident-node")]
        node_name: String,
        #[arg(long)]
        bootstrap_labels: Option<String>,
        #[arg(long, default_value = "trident-acl-agent-tester-reboot-signal")]
        marker_file: PathBuf,
        #[arg(long, default_value = "30s", value_parser = parse_duration)]
        reboot_duration: Duration,
    },
    /// Run the fake Nebraska/Omaha server.
    NebraskaProxy {
        #[arg(long, default_value = "127.0.0.1:18081")]
        listen: SocketAddr,
        #[arg(long)]
        scenario: PathBuf,
    },
    /// Hidden helper for PATH-based reboot interception during tests.
    #[command(hide = true)]
    RebootShim {
        #[arg(long, default_value = "trident-acl-agent-tester-reboot-signal")]
        marker_file: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Apiserver {
            listen,
            node_name,
            seed_labels,
            seed_file,
        } => {
            let seed_labels = parse_label_map(seed_labels.as_deref())?;
            let handle = if let Some(seed_file) = seed_file {
                let contents = std::fs::read_to_string(&seed_file)
                    .with_context(|| format!("failed to read {}", seed_file.display()))?;
                let node = apiserver::node_from_seed_file(&contents)?;
                let seeded_name = node.metadata.name.clone().unwrap_or(node_name);
                let handle = apiserver::spawn(
                    listen,
                    seeded_name,
                    node.metadata.labels.clone().unwrap_or_default(),
                )
                .await?;
                if let Some(annotations) = node.metadata.annotations.clone() {
                    handle.store().patch_annotations(annotations).await?;
                }
                handle
            } else {
                apiserver::spawn(listen, node_name, seed_labels).await?
            };
            println!("fake apiserver listening at {}", handle.url());
            tokio::signal::ctrl_c().await?;
            handle.shutdown().await;
        }
        Commands::RpProxy {
            apiserver_url,
            node_name,
            scenario,
            json,
        } => {
            let client = Client::new();
            let scenario = Scenario::load(&scenario)?;
            let report = rp::run_scenario(&client, &apiserver_url, &node_name, &scenario).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("scenario passed: {}", report.passed);
                for step in &report.steps {
                    println!(
                        "step {} [{}] passed={} elapsed={}ms {}",
                        step.index, step.kind, step.passed, step.elapsed_ms, step.message
                    );
                    if let Some(expected) = &step.expected {
                        println!("  expected: {}", expected);
                    }
                    if let Some(actual) = &step.actual {
                        println!("  actual:   {}", actual);
                    }
                }
            }
            if !report.passed {
                std::process::exit(1);
            }
        }
        Commands::KubeletProxy {
            apiserver_url,
            node_name,
            bootstrap_labels,
            marker_file,
            reboot_duration,
        } => {
            let client = Client::new();
            let bootstrap_labels = parse_label_map(bootstrap_labels.as_deref())?;
            kubelet::run(
                &client,
                &apiserver_url,
                &node_name,
                bootstrap_labels,
                marker_file,
                reboot_duration,
            )
            .await?;
        }
        Commands::NebraskaProxy { listen, scenario } => {
            let scenario = NebraskaScenario::load(&scenario)?;
            let handle = nebraska::spawn(listen, scenario).await?;
            println!("fake Nebraska listening at {}", handle.url());
            tokio::signal::ctrl_c().await?;
            handle.shutdown().await;
        }
        Commands::RebootShim { marker_file } => {
            kubelet::write_reboot_marker(&marker_file)?;
        }
    }

    Ok(())
}

fn parse_label_map(raw: Option<&str>) -> Result<BTreeMap<String, String>, anyhow::Error> {
    let mut labels = BTreeMap::new();
    let Some(raw) = raw else {
        return Ok(labels);
    };

    for pair in raw.split(',').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("invalid key=value pair: {pair}"))?;
        labels.insert(key.to_string(), value.to_string());
    }
    Ok(labels)
}

fn parse_duration(raw: &str) -> Result<Duration, String> {
    humantime::parse_duration(raw).map_err(|err| err.to_string())
}
