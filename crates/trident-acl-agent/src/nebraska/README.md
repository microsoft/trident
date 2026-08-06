# `nebraska` client module — design and adoption notes

A self-contained Rust client for the [Nebraska](https://github.com/flatcar/nebraska)
update server (Omaha protocol), living in `crates/trident-acl-agent/src/nebraska/`.

The authoritative behavioural spec is
`knowledge/topics/nebraska-client-protocol.md` in the `pacobot` repository. This
document only covers how the module maps that spec into a Rust API and how the
existing agent would adopt it.

## Public API

| Item | Purpose |
| --- | --- |
| `Client<T = ReqwestTransport>` | A client bound to one app + track + machine id. Methods: `check_for_update`, `report_progress`, `complete_after_reboot`, `report_failure`. |
| `CheckOutcome` | `UpToDate` \| `UpdateAvailable(UpdateOffer)` \| `UpdateInProgress`. The last models `error-updateInProgressOnInstance` as an expected outcome, not an error. |
| `UpdateOffer` | `{ version: semver::Version, package_url: Url }` — the resolved package URL (codebase joined with package name). |
| `ProgressEvent` | `DownloadStarted` \| `DownloadFinished` \| `Installed`. The only publicly constructible events; they map to the whitelisted wire pairs `13/1`, `14/1`, `800/1`. |
| `MachineId` | Validated, unbraced instance id. `from_uuid` / `new`. |
| `AppStatus`, `UpdateCheckStatus` | Response statuses with an `Other(String)` catch-all so unknown values never break parsing. |
| `Transport`, `ReqwestTransport` | The HTTP seam; injectable for hermetic tests. |
| `NebraskaError` | The module error type (`thiserror`). |

Terminal events (`3/2` complete, `3/0` failure) are **not** public values — they
are emitted only through `complete_after_reboot` and `report_failure`, so they
always carry the correct request shape (e.g. the batched update-check that
completion requires).

## How the invariants are encoded

- **Whitelisted events only** — raw `(type, result)` integers are private; the
  public vocabulary (`ProgressEvent` + the terminal methods) can only produce the
  six accepted pairs. A unit test asserts this.
- **`track` mandatory** — a field of `Client`; no request can be built without it.
- **Unbraced, stable machine id** — `MachineId` rejects braced ids; `from_uuid`
  uses Rust's unbraced `Display`.
- **`error-updateInProgressOnInstance` is expected** — surfaced as
  `CheckOutcome::UpdateInProgress`; unknown statuses map to `Other`.
- **Real semver version** — the API takes `&semver::Version`, and offered
  versions are parsed as semver.
- **All-or-nothing event reporting** — see the design decision below.

## Design decision: invariant #2 (all-or-nothing) is a documented plain API, not a typestate

Sending progress events commits the caller to a terminal event, and **the
terminal event fires after a reboot — in a different process** from the progress
events. No in-process typestate or RAII guard can span that boundary; worse, an
RAII "you didn't finish" guard would fire at the drop that happens *at* reboot,
which is exactly when completion must *not* be reported. A compile-time
"started ⇒ must-finish" is therefore structurally impossible here.

Instead the property is encoded three ways that actually hold:

1. Terminal events are not free-standing values; they are dedicated `Client`
   methods, so a terminal cannot be sent in the wrong shape or context.
2. The only-whitelisted-pairs property is total, so no invalid event exists.
3. `complete_after_reboot` is the batched `3/2 + ping + updatecheck` request,
   making the safe post-reboot path (which closes the wedge window) the easy one.

Persisting "an update is in flight (previous X, target Y)" across the reboot is
the caller's responsibility — it is orchestration, deliberately out of this
module's scope — but the module makes the correct post-reboot call trivial.

## Retry of the terminal event is the caller's, but the module makes the distinction visible

`complete_after_reboot` is **not** retried internally. Retry policy is the
caller's — it lives alongside the cross-reboot state the caller must already
persist, and baking a policy into a protocol module tends to fight whatever the
caller has. But because losing the terminal event wedges the instance
permanently, the module makes the retry decision unmissable:

- `complete_after_reboot`'s rustdoc states, in plain terms, that the call must be
  retried until it succeeds and why.
- `NebraskaError::is_retryable()` classifies transient (transport/HTTP) failures
  from permanent (protocol) ones, so the caller can loop while retryable and stop
  on a permanent error — avoiding the inverse bug (spinning on a permanent
  failure) that bit the gRPC commit path.

## Blocking transport today; async is a non-breaking addition

`Transport` is synchronous, matching the current agent. A future async TAA must
not call a blocking HTTP client inside its Tokio runtime. Supporting async does
**not** require changing this API: `Client` is generic over the transport, so an
`AsyncTransport` trait plus a thin async client can be added *alongside* the sync
ones without breaking them. See the `transport` module docs for the full note.

## Adopting this in the agent

The current agent (`main.rs` + the ad-hoc `omaha` module) predates this module.
A future change would:

1. Replace `omaha::send` / `query_and_fetch_document` / `report_event` with a
   `nebraska::Client` built from the CLI args (`endpoint`, `appid`, `track`) and a
   `MachineId` derived from `IdSource`.
2. Map the poll loop's results onto `CheckOutcome` (the agent already distinguishes
   no-update / in-progress / available).
3. In `--events full`, call `report_progress` around the Trident stage/finalize,
   persist the in-flight state to `/var`, and after the reboot call
   `complete_after_reboot` (with retry) as the first request.
4. Delete the `omaha` module once nothing references it.

This module contains no Trident gRPC, reboot, commit, or CLI logic, so that
adoption is purely at the protocol seam.
