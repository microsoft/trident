//! # Harpoon
//!
//! Harpoon is a lightweight Omaha protocol client for documents. It queries a
//! server at a given address for a specific app and track to fetch an updated
//! document.
//!
//! This crate is specifically meant to function as an Omaha client for Trident
//! to fetch updated Host Configuration documents.
//!
//! <img src="../logo.jpeg" width="200px"/>
//!

use std::{thread, time::Duration};

use anyhow::Context;
use clap::Parser;
use futures::StreamExt;
use log::{debug, error, info, trace, warn, LevelFilter};
use semver::Version;
use sha2::{Digest, Sha256};
use tonic::{transport::Endpoint, Streaming};
use trident_proto::v1::{
    commit_service_client::CommitServiceClient, servicing_response::Response as ResponseBody,
    update_service_client::UpdateServiceClient, CommitRequest, FinalizeUpdateRequest,
    HostConfiguration, LogLevel, RebootHandling, RebootManagement, ServicingResponse,
    StageUpdateRequest, StatusCode, UpdateRequest,
};
use trident_proto::v1preview::{
    status_service_client::StatusServiceClient, GetServicingStateRequest, ServicingState,
};
use url::Url;
use uuid::Uuid;

use osutils::osrelease::OsRelease;

/// Default Nebraska update endpoint (POC). Used when no URL is given.
const DEFAULT_URL: &str = "https://nebraska-poc-ep-cda8e2czfnhahxfk.b01.azurefd.net/v1/update/";

/// Default Omaha app id, used when `--appid` / `HARPOON_APPID` is not provided.
/// This is the app Paco registered in the Nebraska POC for the demo.
const DEFAULT_APPID: &str = "6d10cf97-443f-4542-8479-b9fdb44c9588";

/// Default Omaha track (a.k.a. group/channel), used when `--track` /
/// `HARPOON_TRACK` is not provided. Nebraska does not infer the group; the
/// client declares it here and it must match exactly.
const DEFAULT_TRACK: &str = "stable";

/// Value accepted by Trident's Host Configuration to skip COSI checksum
/// verification. Mirrors `trident_api::constants::IMAGE_CHECKSUM_IGNORED`.
const IMAGE_CHECKSUM_IGNORED: &str = "ignored";

/// `/etc/os-release` fields consulted, in order, to derive the current OS
/// version reported to Nebraska. `IMAGE_VERSION` is preferred because Azure
/// Linux date-stamps the per-build version there (e.g. `3.0.20260801`), while
/// `VERSION_ID` is often just `3.0` and cannot distinguish two builds.
const VERSION_FIELDS: &[&str] = &["IMAGE_VERSION", "VERSION_ID", "VERSION", "BUILD_ID"];

pub mod error;
pub mod id;
pub mod omaha;
pub mod state;

use error::HarpoonError;
use omaha::{
    event::{OmahaEvent, OmahaEventType},
    request::{AppRequest, Request},
    response::Package,
};

pub use id::IdSource;
pub use omaha::event::EventResult;

#[derive(Debug, PartialEq, Eq)]
pub struct HarpoonQueryResponse {
    pub session_id: Uuid,
    pub result: QueryResult,
}

#[derive(Debug, PartialEq, Eq)]
pub enum QueryResult {
    NoUpdate,
    /// Nebraska reports an update is already in progress for this instance
    /// (app status `error-updateInProgressOnInstance`). Expected between a
    /// download-started event and the final complete event; must be tolerated,
    /// not treated as a hard error.
    UpdateInProgress,
    NewDocument { url: Url, version: Version },
}

/// Whether the agent reports Omaha events back to Nebraska.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum EventsMode {
    /// Do not send any events. Pure update-check polling. Nebraska self-heals
    /// the instance to Complete on the next check at the new version. This is
    /// the guaranteed-safe path and the default.
    None,
    /// Send the full Omaha event sequence to drive Nebraska's instance state
    /// machine (downloading → downloaded → installed → complete).
    Full,
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]
    #[arg(global = true, short, long, default_value_t = LevelFilter::Info)]
    pub verbosity: LevelFilter,

    /// The URL of the Nebraska server to use. Should end in `/v1/update/`
    /// (trailing slash matters for package URL composition).
    #[arg(long, env = "HARPOON_URL", default_value = DEFAULT_URL)]
    pub url: Url,

    /// Omaha app id to query for. Must match the app registered in Nebraska.
    #[arg(long, env = "HARPOON_APPID", default_value = DEFAULT_APPID)]
    pub appid: String,

    /// Omaha track (Nebraska group/channel). Nebraska does not infer the group;
    /// it must match exactly or no update is returned.
    #[arg(long, env = "HARPOON_TRACK", default_value = DEFAULT_TRACK)]
    pub track: String,

    /// Interval between polls of the Nebraska server. Accepts a bare number of
    /// seconds (e.g. `1`) or a duration string (e.g. `1s`, `500ms`).
    #[arg(long, env = "HARPOON_INTERVAL", default_value = "1s", value_parser = parse_interval)]
    pub interval: Duration,

    /// Whether to report Omaha events back to Nebraska.
    #[arg(long, env = "HARPOON_EVENTS", value_enum, default_value_t = EventsMode::None)]
    pub events: EventsMode,

    /// How to derive the machine id (instance identity) reported to Nebraska.
    #[arg(long, env = "HARPOON_ID_SOURCE", value_enum, default_value_t = IdSource::MachineIdHashed)]
    pub id_source: IdSource,

    /// Override the machine id entirely, bypassing `--id-source`. Useful as an
    /// in-the-room recovery to abandon a wedged Nebraska instance.
    #[arg(long, env = "HARPOON_MACHINE_ID")]
    pub machine_id: Option<String>,

    /// Run a single poll and exit, instead of looping.
    #[arg(long)]
    pub once: bool,

    /// Override the current OS version reported to Nebraska. When unset, it is
    /// derived from `/etc/os-release` (see `--version-field`).
    #[arg(long, env = "HARPOON_CURRENT_VERSION")]
    pub current_version: Option<String>,

    /// `/etc/os-release` field to read the current version from. When unset,
    /// the first parseable of IMAGE_VERSION, VERSION_ID, VERSION, BUILD_ID is
    /// used.
    #[arg(long, env = "HARPOON_VERSION_FIELD")]
    pub version_field: Option<String>,

    /// COSI metadata SHA384 to pass to Trident, or `ignored` to skip
    /// verification. The Omaha/Nebraska response does not carry the COSI
    /// metadata hash, so this must be supplied out of band when verification is
    /// desired.
    #[arg(long, env = "HARPOON_SHA384", default_value = IMAGE_CHECKSUM_IGNORED)]
    pub sha384: String,
}

fn main() {
    let args = Args::parse();

    init_logger(args.verbosity);

    let current_version = match resolve_current_version(&args) {
        Ok(v) => v,
        Err(e) => {
            error!("Failed to determine current OS version: {e}");
            error!(
                "Pass --current-version <semver> or --version-field <OS_RELEASE_FIELD> to override."
            );
            std::process::exit(1);
        }
    };

    // Resolve the machine id (instance identity) once, honoring an explicit
    // --machine-id override over the derived --id-source.
    let machine_id = match resolve_machine_id(&args) {
        Ok(id) => id,
        Err(e) => {
            error!("Failed to determine machine id: {e}");
            std::process::exit(1);
        }
    };

    info!(
        "Harpoon agent starting: appid='{}' track='{}' current-version=v{current_version} \
         machine-id='{machine_id}' interval={:?} events={:?} url='{}'",
        args.appid, args.track, args.interval, args.events, args.url
    );

    // Commit a finalized-but-uncommitted A/B update so it becomes permanent.
    // Trident finalizes (arms the new slot) but does not commit; without this
    // the update reverts on a later boot. Runs in BOTH events modes, and is a
    // no-op unless an A/B update is actually pending commit.
    if let Err(e) = commit_finalized_update(&current_version) {
        error!("Failed to commit finalized A/B update: {e:#}");
        error!(
            "The running update may revert on a later reboot; a manual `trident commit` may be \
             needed."
        );
    }

    // In full-events mode, the very first thing we do after a reboot is report
    // update completion to Nebraska (before any bare update-check), so a wedged
    // instance is never observed. This is a no-op if there is no pending update.
    if args.events == EventsMode::Full {
        if let Err(e) = handle_post_reboot(&args, &current_version, &machine_id) {
            // Non-fatal: fall through to polling. Nebraska can still self-heal.
            warn!("Post-reboot completion reporting failed: {e:#}");
        }
    }

    loop {
        match poll_once(&args, &current_version, &machine_id) {
            Ok(PollOutcome::NoUpdate) => {
                info!("checking for update... none available (current v{current_version})");
            }
            Ok(PollOutcome::UpdateInProgress) => {
                info!(
                    "checking for update... Nebraska reports an update already in progress for \
                     this instance; waiting (this is expected mid-update)"
                );
            }
            Ok(PollOutcome::Updated { version }) => {
                info!("update to v{version} applied; Trident is handling the reboot");
                // Trident owns the reboot, which will tear this process down.
                // Exit cleanly so a supervisor (e.g. systemd) restarts us after
                // the reboot to resume polling on the new version.
                std::process::exit(0);
            }
            Err(e) => {
                // Transient errors (Nebraska unreachable, network settling
                // after a reboot, etc.) must not kill the agent — log calmly
                // and keep polling.
                warn!("poll failed (will retry): {e:#}");
            }
        }

        if args.once {
            break;
        }
        thread::sleep(args.interval);
    }
}

fn init_logger(verbosity: LevelFilter) {
    if let Some(Ok(journal_logger)) =
        systemd_journal_logger::connected_to_journal().then(systemd_journal_logger::JournalLog::new)
    {
        journal_logger
            .install()
            .expect("Failed to install systemd journal logger");
        log::set_max_level(verbosity);
    } else {
        env_logger::builder()
            .format_timestamp(None)
            .filter_level(verbosity)
            .init();
    }
}

/// Parses a poll interval, accepting either a bare number of seconds (e.g. `1`)
/// or a humantime duration string (e.g. `1s`, `500ms`).
fn parse_interval(s: &str) -> Result<Duration, String> {
    if let Ok(secs) = s.parse::<u64>() {
        return Ok(Duration::from_secs(secs));
    }
    humantime::parse_duration(s).map_err(|e| format!("invalid interval '{s}': {e}"))
}

/// Outcome of a single poll of the Nebraska server.
enum PollOutcome {
    NoUpdate,
    UpdateInProgress,
    Updated { version: Version },
}

/// Performs a single poll: query Nebraska, and if a newer version is offered,
/// drive the Trident A/B update to completion.
fn poll_once(
    args: &Args,
    current_version: &Version,
    machine_id: &str,
) -> Result<PollOutcome, anyhow::Error> {
    let response = query_and_fetch_document(
        &args.url,
        &args.appid,
        &args.track,
        current_version,
        machine_id,
    )?;

    let (url, version) = match response.result {
        QueryResult::NoUpdate => return Ok(PollOutcome::NoUpdate),
        QueryResult::UpdateInProgress => return Ok(PollOutcome::UpdateInProgress),
        QueryResult::NewDocument { url, version } => (url, version),
    };

    // Belt-and-suspenders: Nebraska should only offer newer versions given the
    // current version we send, but guard against a misconfigured channel that
    // would otherwise loop forever.
    if version <= *current_version {
        warn!("Nebraska offered v{version} which is not newer than current v{current_version}; ignoring");
        return Ok(PollOutcome::NoUpdate);
    }

    info!("UPDATE FOUND: v{current_version} -> v{version}");

    let hash = if args.sha384 == IMAGE_CHECKSUM_IGNORED {
        None
    } else {
        Some(args.sha384.clone())
    };

    // In full-events mode the agent owns the reboot so it can report the
    // download/install events in order before rebooting; in none mode Trident
    // owns the reboot.
    let reboot_handling = match args.events {
        EventsMode::Full => RebootHandling::CallerHandlesReboot,
        EventsMode::None => RebootHandling::TridentHandlesReboot,
    };

    // Report "download started" (13/1 → Downloading) before staging.
    if args.events == EventsMode::Full {
        report_event_best_effort(
            args,
            machine_id,
            current_version,
            "download started",
            OmahaEventType::UpdateDownloadStarted,
            EventResult::Success,
            None,
        );
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(trigger(&url, hash, reboot_handling))?;

    if args.events == EventsMode::Full {
        // Stage + finalize completed. Report "download finished" (14/1 →
        // Downloaded) and "installed" (800/1 → Installed) in order, then record
        // the pending update and reboot ourselves so the ordering vs. reboot is
        // guaranteed.
        report_event_best_effort(
            args,
            machine_id,
            current_version,
            "download finished",
            OmahaEventType::UpdateDownloadFinished,
            EventResult::Success,
            None,
        );
        report_event_best_effort(
            args,
            machine_id,
            current_version,
            "installed",
            OmahaEventType::EventUpdateInstalled,
            EventResult::Success,
            None,
        );

        // Persist the pre-reboot state to the shared, persistent root so the
        // restarted agent can report completion with the correct previous
        // version after the reboot.
        state::record_pending_update(&state::PendingUpdate {
            previous_version: current_version.clone(),
            target_version: version.clone(),
        })?;

        // This does not return on success — the system reboots.
        reboot_self()?;
    }

    Ok(PollOutcome::Updated { version })
}

/// Sends a single Omaha event to Nebraska, logging but not propagating failures.
/// Pre-reboot events are cosmetic (they drive the intermediate Nebraska UI
/// states); a failure must not abort the update. `label` is a human-readable
/// marker logged at INFO for the demo console.
#[allow(clippy::too_many_arguments)]
fn report_event_best_effort(
    args: &Args,
    machine_id: &str,
    version: &Version,
    label: &str,
    event_type: OmahaEventType,
    event_result: EventResult,
    previous_version: Option<&Version>,
) {
    info!("reporting to Nebraska: {label}");
    match send_update_event(
        &args.url,
        &args.appid,
        &args.track,
        machine_id,
        version,
        event_type,
        event_result,
        previous_version,
    ) {
        Ok(()) => debug!("Reported Omaha event {event_type:?}/{event_result:?} to Nebraska"),
        Err(e) => warn!("Failed to report Omaha event {event_type:?}/{event_result:?}: {e}"),
    }
}

/// Reboots the machine to boot into the freshly finalized OS slot. Used in
/// full-events mode, where the agent (not Trident) owns the reboot. Logs loudly
/// immediately before, and does not return on success.
fn reboot_self() -> Result<(), anyhow::Error> {
    warn!("Update finalized. REBOOTING NOW to boot into the updated OS...");

    let status = std::process::Command::new("systemctl")
        .arg("reboot")
        .status();
    let ok = match status {
        Ok(s) if s.success() => true,
        Ok(s) => {
            warn!("`systemctl reboot` exited with {s}; falling back to `reboot`");
            std::process::Command::new("reboot")
                .status()
                .map(|s2| s2.success())
                .unwrap_or(false)
        }
        Err(e) => {
            warn!("Failed to invoke `systemctl reboot` ({e}); falling back to `reboot`");
            std::process::Command::new("reboot")
                .status()
                .map(|s2| s2.success())
                .unwrap_or(false)
        }
    };

    if !ok {
        return Err(anyhow::anyhow!(
            "Failed to issue reboot; the system is finalized but not rebooted"
        ));
    }

    // Reboot has been requested; block until systemd tears us down, rather than
    // exiting (which would let systemd's Restart=always flap the unit during
    // shutdown).
    info!("Reboot requested; waiting for the system to go down.");
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}

/// Handles the first-request-after-reboot completion reporting in full-events
/// mode. If a pending update was recorded before the reboot and we are now
/// running the target version, report update-complete (3/2) batched with a ping
/// and an update-check as a single request, which moves the Nebraska instance
/// straight to Complete and returns a clean no-update. If the running version
/// does not match (e.g. the update did not take), report a failure (3/0) to
/// reset the instance so it can be re-offered.
fn handle_post_reboot(
    args: &Args,
    current_version: &Version,
    machine_id: &str,
) -> Result<(), anyhow::Error> {
    let Some(pending) = state::load_pending_update()? else {
        return Ok(());
    };

    if *current_version == pending.target_version {
        info!(
            "Booted into updated OS v{current_version}; reporting update-complete to Nebraska \
             (previous v{})",
            pending.previous_version
        );
        // Retry a few times: this is the request that unsticks the instance.
        let mut last_err = None;
        for attempt in 1..=5 {
            match send_complete_batched(
                &args.url,
                &args.appid,
                &args.track,
                machine_id,
                current_version,
                &pending.previous_version,
            ) {
                Ok(()) => {
                    state::clear_pending_update()?;
                    info!("Reported update-complete; Nebraska instance is now Complete.");
                    return Ok(());
                }
                Err(e) => {
                    warn!("Attempt {attempt}/5 to report update-complete failed: {e}");
                    last_err = Some(e);
                    thread::sleep(Duration::from_secs(2));
                }
            }
        }
        return Err(anyhow::Error::from(last_err.unwrap_or_else(|| {
            HarpoonError::QueryError("Failed to report update-complete".to_string())
        }))
        .context("Exhausted retries reporting update-complete"));
    }

    // We recorded an update to target_version but are not running it. Report a
    // failure so Nebraska releases the in-progress slot and can re-offer.
    warn!(
        "Expected v{} after update but running v{current_version}; reporting update failure to \
         Nebraska to reset the instance",
        pending.target_version
    );
    send_update_event(
        &args.url,
        &args.appid,
        &args.track,
        machine_id,
        current_version,
        OmahaEventType::UpdateComplete,
        EventResult::Error,
        Some(&pending.previous_version),
    )?;
    state::clear_pending_update()?;
    Ok(())
}

/// Sends a single Omaha event (no update-check) to Nebraska.
#[allow(clippy::too_many_arguments)]
fn send_update_event(
    url: &Url,
    app_id: &str,
    track: &str,
    machine_id: &str,
    version: &Version,
    event_type: OmahaEventType,
    event_result: EventResult,
    previous_version: Option<&Version>,
) -> Result<(), HarpoonError> {
    let mut app =
        AppRequest::new_with_machine_id(app_id, version, track, machine_id.to_string())
            .with_event(OmahaEvent::new(event_type, event_result));
    if let Some(prev) = previous_version {
        app = app.with_previous_version(prev);
    }
    omaha::send(url, &Request::default().with_app(app))?;
    Ok(())
}

/// Sends the batched post-reboot request: update-complete (3/2) + ping +
/// update-check, in a single Omaha request. Nebraska processes the event before
/// the update-check, so the instance moves to Complete and the same response
/// returns a clean no-update. Errors if Nebraska still reports an error status.
fn send_complete_batched(
    url: &Url,
    app_id: &str,
    track: &str,
    machine_id: &str,
    version: &Version,
    previous_version: &Version,
) -> Result<(), HarpoonError> {
    let app = AppRequest::new_with_machine_id(app_id, version, track, machine_id.to_string())
        .with_event(OmahaEvent::new(
            OmahaEventType::UpdateComplete,
            EventResult::SuccessReboot,
        ))
        .with_previous_version(previous_version)
        .with_ping()
        .with_update_check();

    let response = omaha::send(url, &Request::default().with_app(app))?;
    let app = response.apps().first().ok_or_else(|| {
        HarpoonError::InvalidResponse("Missing app in complete-event response".to_string())
    })?;

    // The instance may legitimately be Complete already; only a still-in-progress
    // status indicates the completion did not take.
    if app.status().is_update_in_progress() {
        return Err(HarpoonError::QueryError(
            "Nebraska still reports update in progress after complete event".to_string(),
        ));
    }
    Ok(())
}


fn resolve_machine_id(args: &Args) -> Result<String, anyhow::Error> {
    match &args.machine_id {
        Some(id) => Ok(id.clone()),
        None => args
            .id_source
            .produce_id()
            .map_err(|e| anyhow::anyhow!("Failed to derive machine id via {}: {e}", args.id_source)),
    }
}

/// Resolves the current OS version to report to Nebraska, from the
/// `--current-version` override or `/etc/os-release`.
fn resolve_current_version(args: &Args) -> Result<Version, anyhow::Error> {
    if let Some(raw) = &args.current_version {
        return parse_semver_loose(raw)
            .ok_or_else(|| anyhow::anyhow!("--current-version '{raw}' is not a valid version"));
    }

    let os_release =
        OsRelease::read().map_err(|e| anyhow::anyhow!("Failed to read /etc/os-release: {e}"))?;

    // If the operator named a specific field, honor it strictly.
    if let Some(field) = &args.version_field {
        let raw = os_release_field(&os_release, field).ok_or_else(|| {
            anyhow::anyhow!("os-release field '{field}' is not set or not supported")
        })?;
        let version = parse_semver_loose(&raw).ok_or_else(|| {
            anyhow::anyhow!("os-release field '{field}'='{raw}' is not a parseable version")
        })?;
        info!("Current version v{version} from os-release field '{field}' ('{raw}')");
        return Ok(version);
    }

    // Otherwise try the preferred fields in order.
    for field in VERSION_FIELDS {
        if let Some(raw) = os_release_field(&os_release, field) {
            if let Some(version) = parse_semver_loose(&raw) {
                info!("Current version v{version} from os-release field '{field}' ('{raw}')");
                return Ok(version);
            }
            debug!("os-release field '{field}'='{raw}' did not parse as a version, trying next");
        }
    }

    Err(anyhow::anyhow!(
        "No parseable version found in os-release fields {VERSION_FIELDS:?}"
    ))
}

/// Reads a named field from the parsed os-release, for the subset of fields we
/// consult for versioning.
fn os_release_field(os_release: &OsRelease, field: &str) -> Option<String> {
    match field.to_ascii_uppercase().as_str() {
        "IMAGE_VERSION" => os_release.image_version.clone(),
        "VERSION_ID" => os_release.version_id.clone(),
        "VERSION" => os_release.version.clone(),
        "BUILD_ID" => os_release.build_id.clone(),
        _ => None,
    }
}

/// Parses a version string leniently: takes the first whitespace token, strips
/// quotes, and accepts 1-3 dotted numeric components (missing components
/// default to 0). Accepts e.g. `3.0.20260801`, `3.0`, `"3.0"`,
/// `3.0.20260801 (Azure Linux)`.
fn parse_semver_loose(s: &str) -> Option<Version> {
    let token = s.trim().trim_matches('"').split_whitespace().next()?;

    if let Ok(v) = Version::parse(token) {
        return Some(v);
    }

    let mut parts = token.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts
        .next()
        .and_then(|p| p.parse::<u64>().ok())
        .unwrap_or(0);
    let patch = parts
        .next()
        .and_then(|p| p.parse::<u64>().ok())
        .unwrap_or(0);
    Some(Version::new(major, minor, patch))
}

/// Commits a finalized-but-uncommitted A/B update so it becomes permanent.
/// Trident finalizes an update (arming the new slot) but does not make it
/// permanent until committed; without this the update reverts on a later boot.
/// Idempotent: does nothing unless the servicing state is "A/B update
/// finalized". Runs in both events modes on startup.
fn commit_finalized_update(current_version: &Version) -> Result<(), anyhow::Error> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(commit_finalized_update_async(current_version))
}

async fn commit_finalized_update_async(current_version: &Version) -> Result<(), anyhow::Error> {
    // Trident's gRPC socket may not be ready immediately after boot; retry the
    // connect/query a bounded number of times before giving up.
    const ATTEMPTS: u32 = 15;
    let mut last_err = None;
    for attempt in 1..=ATTEMPTS {
        match try_commit_if_finalized(current_version).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                debug!("Commit check attempt {attempt}/{ATTEMPTS} not ready yet: {e}");
                last_err = Some(e);
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("commit check failed")))
        .context("Trident not reachable to check/commit servicing state")
}

async fn try_commit_if_finalized(current_version: &Version) -> Result<(), anyhow::Error> {
    let channel = Endpoint::new(trident_proto::TRIDENT_DEFAULT_SOCKET_URI)?
        .connect()
        .await?;
    let state = StatusServiceClient::new(channel.clone())
        .get_servicing_state(tonic::Request::new(GetServicingStateRequest {}))
        .await?
        .into_inner()
        .state();

    if state != ServicingState::UpdateAbFinalized {
        debug!("Servicing state is {state:?}; no A/B commit needed.");
        return Ok(());
    }

    info!("A/B update is finalized but not committed; committing to make v{current_version} permanent...");
    let response = CommitServiceClient::new(channel)
        .commit(tonic::Request::new(CommitRequest {
            reboot: Some(RebootManagement {
                handling: RebootHandling::TridentHandlesReboot.into(),
            }),
        }))
        .await?
        .into_inner();
    handle_servicing_stream(response).await?;
    info!("A/B update committed; now running v{current_version} permanently.");
    Ok(())
}

async fn trigger(
    url: &Url,
    hash: Option<String>,
    reboot_handling: RebootHandling,
) -> Result<(), anyhow::Error> {
    debug!(
        "Triggering Trident update with URL: {url}, hash: {hash:?}, reboot: {reboot_handling:?}"
    );

    let channel = Endpoint::new(trident_proto::TRIDENT_DEFAULT_SOCKET_URI)?
        .connect()
        .await?;
    let mut client = UpdateServiceClient::new(channel);

    let response = client
        .update(tonic::Request::new(UpdateRequest {
            stage: Some(StageUpdateRequest {
                config: Some(HostConfiguration {
                    // TODO: Handle escaping of URL and hash.
                    config: match hash {
                        Some(hash) => format!("image:\n  url: {url}\n  sha384: {hash}"),
                        None => {
                            format!("image:\n  url: {url}\n  sha384: ignored")
                        }
                    },
                }),
            }),
            finalize: Some(FinalizeUpdateRequest {
                reboot: Some(RebootManagement {
                    handling: reboot_handling.into(),
                }),
            }),
        }))
        .await?;

    handle_servicing_stream(response.into_inner()).await
}

async fn handle_servicing_stream(
    mut stream: Streaming<ServicingResponse>,
) -> Result<(), anyhow::Error> {
    // Iterate through the stream until we get a Completed message
    loop {
        match stream.next().await {
            Some(Ok(response)) => match response.response {
                Some(ResponseBody::Started(_)) => {
                    info!("[Trident] Install started");
                    // Continue to next message
                }
                Some(ResponseBody::Log(log)) => {
                    let msg = format!("[Trident] {}", log.message);
                    match log.level() {
                        LogLevel::Unspecified | LogLevel::Trace => trace!("{msg}"),
                        LogLevel::Debug => debug!("{msg}"),
                        LogLevel::Info => info!("{msg}"),
                        LogLevel::Warn => warn!("{msg}"),
                        LogLevel::Error => error!("{msg}"),
                    }
                }
                Some(ResponseBody::Completed(final_status)) => {
                    if final_status.status() == StatusCode::Success {
                        info!(
                            "Trident install succeeded: status={:?}",
                            final_status.status()
                        );
                        break Ok(());
                    } else {
                        error!("Trident install failed: status={:?}", final_status.status());
                        match final_status.error {
                            Some(err) => {
                                error!("Trident reported error: {}", err.message);
                                break Err(anyhow::anyhow!(err.message));
                            }
                            None => {
                                break Err(anyhow::anyhow!("Trident install failed"));
                            }
                        }
                    }
                }
                None => {
                    // Empty response, continue
                    continue;
                }
            },
            Some(Err(e)) => {
                break Err(anyhow::anyhow!("Error reading from Trident stream: {e}"));
            }
            None => {
                break Err(anyhow::anyhow!(
                    "Trident install stream ended without control message"
                ));
            }
        }
    }
}

/// Query the Omaha (Nebraska) server at the given URL for the given app and
/// track to check for an available update.
///
/// Returns the session ID and the result of the query. If an update is
/// available, the package URL and new version are returned.
pub fn query_and_fetch_document(
    url: &Url,
    app_id: &str,
    track: &str,
    document_version: &Version,
    machine_id: &str,
) -> Result<HarpoonQueryResponse, HarpoonError> {
    let request = Request::default().with_app(
        AppRequest::new_with_machine_id(app_id, document_version, track, machine_id.to_string())
            .with_update_check(),
    );

    let response = omaha::send(url, &request)?;

    debug!(
        "Received response from Omaha server at '{url}' for app '{app_id}' on track '{track}': {response:#?}",
        url = url,
        app_id = app_id,
        track = track,
        response = response
    );
    if response.apps().len() != 1 {
        return Err(HarpoonError::InvalidResponse(
            "Expected exactly one app in response".to_string(),
        ));
    }

    let app = response.apps().first().unwrap();

    if app.app_id() != app_id {
        return Err(HarpoonError::InvalidResponse(
            "Unexpected app ID in response".to_string(),
        ));
    }

    // Nebraska reports `error-updateInProgressOnInstance` on every check between
    // a download-started event and the final complete event. This is expected
    // and self-clearing (via the post-reboot complete event or Nebraska's own
    // self-heal); tolerate it quietly rather than treating it as a hard error.
    if app.status().is_update_in_progress() {
        return Ok(HarpoonQueryResponse {
            session_id: request.session_id(),
            result: QueryResult::UpdateInProgress,
        });
    }

    if app.status().is_error() {
        return Err(HarpoonError::QueryError(format!(
            "Received a non-OK app status: {0}",
            app.status()
        )));
    }

    let update_check = app.update_check().ok_or_else(|| {
        HarpoonError::InvalidResponse("Missing update check in response".to_string())
    })?;
    debug!("Received update check response: {update_check:#?}");

    if update_check.status().is_error() {
        return Err(HarpoonError::QueryError(format!(
            "Received an error status in update check: {0}",
            update_check.status()
        )));
    }

    let update_check = app.update_check().ok_or_else(|| {
        HarpoonError::InvalidResponse("Missing update check in response".to_string())
    })?;
    debug!("Received update check response: {update_check:#?}");

    if update_check.status().is_error() {
        return Err(HarpoonError::QueryError(format!(
            "Received an error status in update check: {0}",
            update_check.status()
        )));
    }

    if update_check.status().is_no_update() {
        // Successfully checked that there is no update available!
        debug!(
            "No update available for app '{}' v{}",
            app_id, document_version
        );
        return Ok(HarpoonQueryResponse {
            session_id: request.session_id(),
            result: QueryResult::NoUpdate,
        });
    }

    // If we got here, an update is available!
    let new_version = update_check.version().ok_or_else(|| {
        HarpoonError::InvalidResponse("Missing new version in update check response".to_string())
    })?;

    let update_base_url = update_check.urls().next().ok_or_else(|| {
        HarpoonError::InvalidResponse("Missing URL in update check response".to_string())
    })?;

    if update_check.packages().len() != 1 {
        return Err(HarpoonError::InvalidResponse(
            "Expected exactly one package in update check response".to_string(),
        ));
    }

    let package_url = update_base_url
        .join(&update_check.packages().first().unwrap().name)
        .map_err(|err| {
            HarpoonError::InvalidResponse(format!("Failed to join URL with package name: {err}"))
        })?;

    debug!(
        "Downloaded update for app '{}' v{} to v{}",
        app_id, document_version, new_version
    );
    debug!("Document URL: {package_url}");

    Ok(HarpoonQueryResponse {
        session_id: request.session_id(),
        result: QueryResult::NewDocument {
            url: package_url,
            version: new_version.as_version().clone(),
        },
    })
}

/// Downloads an update package provided by the Omaha server at the given base
/// URL.
///
/// On success, returns the document as a string and the URL from which it was
/// downloaded.
///
/// The function takes care of validating the size and hash of the downloaded
/// document.
#[allow(unused)]
fn download_document(
    update_base_url: &Url,
    package: &Package,
    file_extension: &str,
) -> Result<(String, Url), HarpoonError> {
    if !package.name.ends_with(file_extension) {
        return Err(HarpoonError::ExpectedYamlDocument(package.name.clone()));
    }

    // If the package size is larger than 1MB, log a warning. This may mean that
    // we are not downloading the correct document.
    if package.size >= 1024 * 1024 {
        warn!(
            "Reported document size is larger than 1MB ({}). This may NOT be a '{}' text document.",
            package.size, file_extension
        );
    }

    let package_url = update_base_url.join(&package.name).map_err(|err| {
        HarpoonError::InvalidResponse(format!("Failed to join URL with package name: {err}"))
    })?;

    let document = reqwest::blocking::Client::new()
        .get(package_url.clone())
        .send()
        .map_err(|err| HarpoonError::FetchError(err.to_string()))?
        .text()
        .map_err(|err| HarpoonError::FetchError(err.to_string()))?;

    // Check that the downloaded document size matches the package size.
    trace!(
        "Validating document size: actual [{}] == expected [{}]",
        document.len(),
        package.size
    );
    if package.size != document.len() as u64 {
        return Err(HarpoonError::FetchError(format!(
            "Downloaded document size does not match package size: {} != {}",
            document.len(),
            package.size
        )));
    }

    // If we have a hash, validate it.
    if !package.hash.is_empty() {
        let actual = format!("{:x}", Sha256::digest(document.as_bytes()));
        let expected = package.hash.to_lowercase();
        trace!(
            "Validating document hash: actual [{}] == expected [{}]",
            actual,
            expected
        );
        if actual != expected {
            return Err(HarpoonError::FetchError(format!(
                "Downloaded document hash does not match package hash: {actual} != {expected}"
            )));
        }
    }

    Ok((document, package_url))
}

/// A wrapper to hide away the details of what Omaha events are actually
/// relevant. Trident only needs to know about Install and Update events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    Install,
    Update,
}

impl From<EventType> for OmahaEventType {
    fn from(event_type: EventType) -> Self {
        match event_type {
            EventType::Install => OmahaEventType::EventUpdateInstalled,
            EventType::Update => OmahaEventType::UpdateComplete,
        }
    }
}

/// Reports an Omaha event to the server at the given URL for the given app and
/// track.
fn report_omaha_event(
    url: &Url,
    app_id: &str,
    track: &str,
    event: OmahaEventType,
    result: EventResult,
    machine_id_source: IdSource,
) -> Result<(), HarpoonError> {
    omaha::send_event(
        url,
        &Request::default().with_app(
            AppRequest::new_event(app_id, track, machine_id_source)?
                .with_event(OmahaEvent::new(event, result)),
        ),
    )?;
    Ok(())
}

/// Reports a generic event to the Omaha server at the given URL for the given
/// app and track.
pub fn report_event(
    url: &Url,
    app_id: &str,
    track: &str,
    event: EventType,
    result: EventResult,
    machine_id_source: IdSource,
) -> Result<(), HarpoonError> {
    report_omaha_event(url, app_id, track, event.into(), result, machine_id_source)
}

#[cfg(test)]
mod tests {
    use mockito::Matcher;

    use super::*;

    #[test]
    fn test_parse_semver_loose() {
        // Full semver passes through.
        assert_eq!(
            parse_semver_loose("3.0.20260801"),
            Some(Version::new(3, 0, 20260801))
        );
        // Two-component (typical VERSION_ID) coerces patch to 0.
        assert_eq!(parse_semver_loose("3.0"), Some(Version::new(3, 0, 0)));
        // Single component.
        assert_eq!(parse_semver_loose("3"), Some(Version::new(3, 0, 0)));
        // Quoted and with trailing text (as os-release VERSION often is).
        assert_eq!(
            parse_semver_loose("\"3.0.20260801\""),
            Some(Version::new(3, 0, 20260801))
        );
        assert_eq!(
            parse_semver_loose("3.0.20260801 (Azure Linux)"),
            Some(Version::new(3, 0, 20260801))
        );
        // Non-numeric is rejected.
        assert_eq!(parse_semver_loose("azurelinux"), None);
        assert_eq!(parse_semver_loose(""), None);
    }

    #[test]
    fn test_download_document() {
        let mut server = mockito::Server::new();

        let data = "test document";

        let document_mock = server
            .mock("GET", "/test.yaml")
            .with_body(data)
            .with_header("content-length", &data.len().to_string())
            .with_header("content-type", "text/plain")
            .with_status(200)
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap();
        let package = Package {
            name: "test.yaml".to_string(),
            size: 13,
            hash: format!("{:x}", Sha256::digest(data.as_bytes())),
            hash_sha256: None,
            required: true,
        };

        let (document, package_url) = download_document(&url, &package, ".yaml").unwrap();

        document_mock.assert();

        assert_eq!(document, data);

        assert_eq!(
            package_url,
            Url::parse(&format!("{}/test.yaml", server.url())).unwrap()
        );
    }

    #[test]
    fn test_query_and_fetch_document() {
        let mut server = mockito::Server::new();

        let data = "test document";

        let omaha_mock = server
            .mock("POST", "/")
            .with_status(200)
            .match_body(Matcher::Regex(".*<updatecheck.*".to_string()))
            .with_body(format!(
                indoc::indoc! {r#"
                <?xml version="1.0" encoding="UTF-8"?>
                <response protocol="3.0" server="mock">
                    <daystart elapsed_seconds="0"/>
                    <app appid="test" status="ok">
                        <updatecheck status="ok">
                            <urls>
                                <url codebase="{}"/>
                            </urls>
                            <manifest version="1.0.0">
                                <packages>
                                    <package hash="{:x}" name="test.yaml" size="{}" required="true"/>
                                </packages>
                            </manifest>
                        </updatecheck>
                    </app>
                </response>"#},
                server.url(),
                Sha256::digest(data.as_bytes()),
                data.len()
            ))
            .expect(1)
            .create();

        // let omaha_event_mock = server
        //     .mock("POST", "/")
        //     .with_status(200)
        //     .match_body(Matcher::Regex(".*<event.*".to_string()))
        //     .with_body(indoc::indoc! {r#"
        //         <?xml version="1.0" encoding="UTF-8"?>
        //         <response protocol="3.0" server="mock">
        //             <daystart elapsed_seconds="0"/>
        //             <app appid="test" status="ok">
        //                 <event status="ok"/>
        //             </app>
        //         </response>"#})
        //     .expect(1)
        //     .create();

        let response = query_and_fetch_document(
            &Url::parse(&server.url()).unwrap(),
            "test",
            "track",
            &Version::new(0, 1, 0),
            "test-machine-id",
        )
        .unwrap();

        omaha_mock.assert();
        // omaha_event_mock.assert();

        assert_eq!(
            response,
            HarpoonQueryResponse {
                session_id: response.session_id,
                result: QueryResult::NewDocument {
                    url: Url::parse(&format!("{}/test.yaml", server.url())).unwrap(),
                    version: Version::new(1, 0, 0),
                }
            }
        );
    }
}
