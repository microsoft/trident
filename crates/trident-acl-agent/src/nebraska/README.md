# `nebraska` client module

A self-contained Rust client for the [Nebraska](https://github.com/flatcar/nebraska)
update server (Omaha protocol). Scoped strictly to the protocol — no Trident
gRPC, reboot, commit, or CLI logic — so it is reusable by any update agent.

The API encodes the protocol's silently-failing invariants in the type system:
only whitelisted events are constructible, `track` cannot be omitted, the machine
id is a validated unbraced newtype, versions are `semver::Version`, and
`error-updateInProgressOnInstance` is a normal outcome rather than an error. The
rationale for each is documented inline on the relevant type.

## Protocol compatibility

This is a generic **Omaha v3** client (`protocol="3.0"`), not tied to any
particular Nebraska release. It relies on a handful of behaviours that are
stable across Nebraska versions rather than on a specific build:

- an event and an update check batched in one request are processed
  events-first, which is what makes the post-reboot completion a single round
  trip;
- `error-updateInProgressOnInstance` is reported for an instance between its
  first progress event and its terminal event;
- the app and the group (resolved from `track`) are looked up *before* events
  are processed, so an unresolvable app or track is reported as an app-level
  error status while the events are dropped.

Unknown status strings are preserved rather than rejected, so a server that adds
new ones does not break parsing.

## Usage

### Poll for an update

```rust,no_run
use semver::Version;
use url::Url;
use trident_acl_agent::nebraska::{Client, CheckOutcome, MachineId};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let client = Client::new(
    Url::parse("https://updates.example.com/v1/update/")?, // trailing slash matters
    "example-app",
    "stable",
    MachineId::from_uuid(uuid::Uuid::new_v4()),
);

let current = Version::new(1, 0, 0);
match client.check_for_update(&current)? {
    CheckOutcome::UpToDate => println!("no update"),
    CheckOutcome::UpdateInProgress => println!("update already in progress"),
    CheckOutcome::UpdateAvailable(offer) => {
        println!("update to {} at {}", offer.version, offer.primary.url);
        // The file's hash (base64 SHA-1, plus SHA-256 when present) is
        // available for integrity-checking the downloaded artifact:
        if let Some(hash) = &offer.primary.hash {
            println!("expected file sha1: {}", hash.sha1);
        }
        // An update may also carry extra files (e.g. a detached signature):
        for extra in &offer.extra_files {
            println!("extra file: {} at {}", extra.name, extra.url);
        }
    }
}
# Ok(())
# }
```

### Report the full update sequence (with events)

Sending progress events is a **commitment** to send a terminal event: leaving an
instance in a progress state wedges it permanently. The terminal event is sent
*after the reboot*, from a fresh process, so the caller must persist the
in-flight state across the reboot and retry the completion until it lands.

```rust,no_run
use semver::Version;
use url::Url;
use trident_acl_agent::nebraska::{Client, CheckOutcome, MachineId, ProgressEvent};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let client = Client::new(
#     Url::parse("https://updates.example.com/v1/update/")?,
#     "example-app", "stable", MachineId::from_uuid(uuid::Uuid::new_v4()));
let current = Version::new(1, 0, 0);

if let CheckOutcome::UpdateAvailable(offer) = client.check_for_update(&current)? {
    // Before/after each stage of the update (driven elsewhere), report progress:
    client.report_progress(&current, ProgressEvent::DownloadStarted)?;
    // ... stage the update ...
    client.report_progress(&current, ProgressEvent::DownloadFinished)?;
    // ... finalize ...
    client.report_progress(&current, ProgressEvent::Installed)?;

    // Persist { previous: current, target: offer.version } somewhere durable,
    // then reboot. After the reboot, from a fresh process on the new version,
    // report completion. This blocks while it retries transient failures (a
    // reboot's first network call often fails while DNS settles) and while the
    // server still reports the update as in progress; losing this event would
    // wedge the instance permanently, which is why it retries by default and
    // fails loudly (`CompletionNotAcknowledged`) rather than returning success.
    let previous = current;
    let now_running = offer.version;
    client.complete_after_reboot(&previous, &now_running)?;
    // A caller with its own scheduler can use `try_complete_after_reboot`
    // instead and drive the retry on its own cadence.
}
# Ok(())
# }
```

### Recover a wedged instance

If completion cannot be reported, `report_failure` moves the instance to Error
and re-arms it so a later check can grant again:

```rust,no_run
# use semver::Version;
# use url::Url;
# use trident_acl_agent::nebraska::{Client, MachineId};
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let client = Client::new(
#     Url::parse("https://updates.example.com/v1/update/")?,
#     "example-app", "stable", MachineId::from_uuid(uuid::Uuid::new_v4()));
client.report_failure(&Version::new(1, 0, 0), &Version::new(2, 0, 0))?;
# Ok(())
# }
```

## Testing

`Transport` abstracts the HTTP round-trip, so the client is testable without a
network by injecting a canned implementation via `Client::with_transport`.

## Notes

- Every request carries a per-request timeout (10s by default, overridable with
  `ReqwestTransport::with_timeout`), so `complete_after_reboot`'s retry window is
  bounded in wall-clock time and a hung connection cannot stall startup.
- The update endpoint may carry an Omaha secret, so it is never logged in full
  and is stripped from transport errors; only its scheme and host appear.
- The `Transport` is synchronous today; an async transport can be added
  alongside it without breaking this API (see the `transport` module docs).
- This module supersedes the crate's older ad-hoc `omaha` module; the agent's
  migration to it is a separate change.
