# `nebraska` client module

A self-contained Rust client for the [Nebraska](https://github.com/flatcar/nebraska)
update server (Omaha protocol). Scoped strictly to the protocol — no Trident
gRPC, reboot, commit, or CLI logic — so it is reusable by any update agent.

The API encodes the protocol's silently-failing invariants in the type system:
only whitelisted events are constructible, `track` cannot be omitted, the machine
id is a validated unbraced newtype, versions are `semver::Version`, and
`error-updateInProgressOnInstance` is a normal outcome rather than an error. The
rationale for each is documented inline on the relevant type.

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
        println!("update to {} at {}", offer.version, offer.package_url);
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
    // then reboot. After the reboot, from a fresh process on the new version:
    let previous = current;
    let now_running = offer.version;
    loop {
        match client.complete_after_reboot(&previous, &now_running) {
            Ok(_) => break,
            // Retry only transient failures; a permanent one will never succeed.
            Err(e) if e.is_retryable() => continue,
            Err(e) => return Err(e.into()),
        }
    }
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

- The `Transport` is synchronous today; an async transport can be added
  alongside it without breaking this API (see the `transport` module docs).
- This module supersedes the crate's older ad-hoc `omaha` module; the agent's
  migration to it is a separate change.
