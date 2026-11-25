# Explicit Storm VM/HOST install/configuration

`run-vm-e2e-install.py` is an opt-in entrypoint for the existing Storm
installation/configuration scenarios. It is not functional `full`, legacy
netlaunch/pytest E2E, QEMU servicing/rollback/UKI, ACL-agent, container, ARM64,
Azure-platform or bare-metal coverage. Source `validate.sh all` and agent
startup remain source-only.

The temporary packet-gated, single-host initial execution lane is documented in
[VM-E2E-Install-Execution.md](VM-E2E-Install-Execution.md). Its bounded selection
does not establish coverage of this runner's complete matrix.

Run only on an exclusive, disposable amd64 Ubuntu 22.04 host. Configure
KVM, host libvirt access (`qemu:///system`), sudo and the existing cloud
execution lock first. This command uses
`/var/lib/trident-cloud-agent/functional.lock`, shared with the functional
runner, not a second independent lock. It does not provision the host,
create Azure resources, change RBAC, download artifacts, or authenticate to ACR.
Storm's existing `--pipeline-run` case installs its VM dependencies.

## Required trusted bundle

Supply a previously produced bundle and its producer-generated provenance.
There is deliberately no default producer build or fallback to a baseline image.
Pin the manifest SHA256 from a trusted build/evidence channel independently of
the bundle. Computing a new pin for an arbitrary local manifest is not proof
that its assertions are true.

The v1 manifest is a **new local handoff contract**, not an existing ADO
artifact format. `prepare_vm_e2e_artifacts.py` emits it from the existing
definition 5073 producer, subject to the reviewed-evidence gate below.
It must attest that all packaged Trident binaries (installer **and** target
COSIs, including v2 images) and Storm were built from `candidateRevision`.
The runner verifies the attestation's pin, exact candidate/source SHA,
immutable numeric producer definition/build IDs, org/project, exact inventory,
file sizes and SHA256 hashes before invoking any bundle executable.
It does not inspect an ISO's RPMs to independently recover their source SHA.
Do not hand-author provenance to relabel an old image as the candidate.

Manifest fields (no sample build IDs or hashes are provided):

| Field | Required value |
|---|---|
| `schemaVersion` | `1` |
| `candidateRevision` | Full lowercase candidate commit SHA |
| `producer.organization` | `mariner-org` |
| `producer.projectId` | `2311650c-e79e-4301-b4d2-96543fdd84ff` |
| `producer.definitionId`, `producer.buildId` | Actual immutable positive integer IDs |
| `producer.sourceVersion` | Exactly the expected candidate SHA, not latest main |
| `files` | Object keyed by bundle-relative POSIX path, each with `artifact` (actual pipeline artifact name), `size` (bytes), `sha256` |
| `signingCertificate` | Optional inventoried relative certificate path, required by Storm for UKI configurations |

Layout follows `.pipelines/templates/stages/testing_common/download-test-images.yml`
and `testing_e2e/test_execution_template.yml`:

- `bin/storm-trident`, from `go-tools`, executable.
- `artifacts/iso/<installerISOName>.iso`, from the artifact named by the generated
  matrix (`trident-installer` or `trident-split-installer` for VM/HOST).
- `artifacts/test-image/`, prepared COSI contents, not a functional QCOW2.
  Use the existing `storm-trident helper prepare-images -a -- ...` pipeline
  preparation path, including real distinct v1/v2 images for A/B scenarios.
  Record each resulting file's actual source artifact in `files`.
- Optional signing certificate from `trident-usrverity-testimage`.
- The manifest may be inside the bundle; no unlisted files or symlinks are allowed.

This first handoff requires all files from one candidate producer build.
If images are built separately or candidate packages are injected, the producer
must first provide verifiable build/injection lineage, not merely assert a new SHA
on baseline images. Normal PR binaries alone are insufficient installation inputs.
No mapping from functional artifact definition 5067 to these images is assumed.

## Candidate producer resolution and preparation

Run preparation on Linux, using the caller's existing Azure CLI session.
The B downloader supplies credential acquisition, authenticated HTTPS metadata,
redirect credential stripping, ZIP member selection and extraction. There is no
login, latest-build lookup, queue, publishing, permission change or VM execution.
Only the selected six image/tool artifacts are downloaded, once per invocation.
`trident-binaries` is qualified through reviewed RPM lineage, not downloaded.

```bash
python3 scripts/cloud-agent/prepare_vm_e2e_artifacts.py \
    --build-id "$APPROVED_IMMUTABLE_BUILD_ID" --expected-definition-id 5073 \
    --expected-revision "$CANDIDATE_SHA" --expected-ref "$CANDIDATE_REF" \
    --evidence "$REVIEWED_PACKET" --evidence-sha256 "$INDEPENDENT_PACKET_SHA256" \
    --timeout-seconds "$PREPARATION_TIMEOUT" --output "$NEW_PREPARATION_OUTPUT"
```

The explicit build must be completed/succeeded, with matching server-reported
project, definition, sourceVersion and sourceBranch. Its timeline must have exactly
one completed/succeeded record for each of these seven stages:
`BuildingTools`, `GetTridentBinaries_rpms_amd64`, and
`TridentTestImg_` plus each underscore-normalized installer/COSI artifact name.
Missing, skipped, failed or ambiguous stages fail closed. Queue approval and safe
stage selection remain the caller's responsibility.

Preparation verifies reviewed hashes/sizes before executing the candidate Storm
helper. It rejects unsafe ZIP paths, special entries and ambiguous basenames.
It uses the exact `download-test-images.yml` command:
`storm-trident helper prepare-images -a -- <regular-dir> <verity-dir> <usrverity-dir>
trident-testimage trident-verity-testimage trident-usrverity-testimage <target> -v 4`.
Installer artifacts each supply `<artifact>.iso`. Each COSI artifact must supply
both `<artifact>_0.cosi` and `<artifact>_1.cosi`, built separately with distinct
hashes. The existing helper maps these to `regular.cosi`, `regular_v2.cosi` (and
`verity`/`usrverity` equivalents), then hard-links v3/v4 to v1/v2. Every prepared
file is checked against its reviewed source hash and inventoried with `sourceFile`.
The five existing pipeline tools are included from `go-tools`.
No keys or installer-tools are needed. Add `--include-uki-certificate` only when
relevant; this requires reviewed `ca_cert.pem` from `trident-usrverity-testimage`.

Success publishes `bundle/provenance.json` only after the unchanged C reader
accepts the exact bundle. Stdout returns its path and SHA256. Transfer that pin
through a trusted channel, then use the existing invocation below. The output
also retains `prepare-images.log`. Failed preparation never publishes the final
manifest; discard its incomplete output and retry with a new directory.
The manifest records artifact IDs, advertised sizes, downloaded ZIP sizes/hashes,
source-to-prepared mapping, successful stage IDs and the minimal reviewed packet.
It never records download URLs or free-form timeline issues.

### Required independently reviewed evidence packet (v1)

**Remaining proof gate:** stage success, artifact names and sourceVersion alone
do not prove which candidate checkout produced an RPM, which current-run RPMs
were consumed, or which package identity was installed in every ISO/COSI clone.
There is no reliable automatic installed-package attestation established here.
An independent reviewer must bind those facts to exact output hashes, and the
resolved floating upstream Azure Linux base VHDX and MIC identities, using the
producer's qualified logs and, where logs are insufficient, package/image
inspection. If that evidence does not exist, do not approve a packet. The
resolver does not fabricate it or label reviewed claims as automatic proof.

The packet is JSON, independently SHA256-pinned after review. All objects have
exact fields; unknown fields fail. It contains **no raw producer logs**, log
excerpts, environment, request headers, tokens/prefixes, private keys or SAS URLs.
Raw logs, if needed for private parsing/review, stay ephemeral outside upload
selection and must never be printed or attached to the handoff or GitHub.
Existing producer logging includes a token-prefix echo and environment dumps;
masking is not assumed to establish safety. These producer tasks are unchanged.

Packet fields:

- `schemaVersion`: `1`; `producer`: exactly the manifest producer fields, including
  `sourceBranch`, matching all explicit caller inputs.
- `review`: `reviewer` (reviewer ID), `reference` (immutable review ID/reference).
  Obtain the packet pin through that independent review, not from arbitrary local
  assertions. Presence of these fields is not itself authentication.
- `logs`: object keyed by numeric producer log ID, each containing only `sha256`
  (digest of the privately reviewed log export) and `lineCount`. ADO log metadata
  must report the same ID/count. Log content authenticity and semantics are the
  reviewer's responsibility; content is not fetched by this resolver.
- All evidence citations are `{logId, startLine, endLine}`, using inclusive,
  positive line numbers. IDs must belong to a succeeded task/job under the
  appropriate succeeded stage in the server timeline.
- `rpms`: `checkoutRevision`, `packages`, `evidence`. The cited successful RPM
  build must prove candidate checkout and source-built package identities, not
  download an old finished image. Each package is exactly `{name, nevra, sha256}`,
  including `trident` and the target's `trident-service` when needed.
- `artifacts`: exactly `go-tools`, `trident-installer`, `trident-split-installer`,
  `trident-testimage`, `trident-verity-testimage`, `trident-usrverity-testimage`.
  Each has `checkoutRevision`, `checkoutEvidence`, `files`.
  `files` is keyed by the exact filenames described above; each entry has positive
  `size` and `sha256`. Every non-certificate image additionally has
  `installedPackages` (the installed subset of the reviewed RPM identities) and
  `installEvidence`, tied to that exact output hash. Installers must install
  `trident`, targets must install `trident-service`.
- Each non-tool artifact additionally has `rpmBuildId` (same explicit build),
  `rpmArtifact` (`trident-binaries`), `rpmEvidence` (successful current-run download
  and consumption), `baseVhdx` and `mic`. Both resolved inputs are exactly
  `{identity, sha256, evidence}`. Record concrete base image release/content
  identity and MIC version/container digest from producer evidence, never the
  original wildcard, latest selector or signed download URL.
- Identity/version/review strings admit only letters, digits and `._/@:+-`,
  starting with a letter/digit. This intentionally excludes query strings,
  whitespace and environment/log prose. All digests are lowercase SHA256.

The manifest labels its lineage basis `independently-reviewed-pinned-evidence`.
The synthetic unit fixtures are not candidate evidence. No candidate build ID is
assumed, and another PR's successful build cannot satisfy candidate equality.

## Invocation

On the disposable host, with real values for the shell variables:

```bash
python3 scripts/cloud-agent/run-vm-e2e-install.py \
    --bundle "$BUNDLE" --manifest "$MANIFEST" \
    --manifest-sha256 "$TRUSTED_MANIFEST_SHA256" \
    --expected-revision "$CANDIDATE_SHA" \
    --stage-type pr-e2e --scenario "$SCENARIO_FROM_MATRIX" \
    --timeout-seconds "$SCENARIO_TIMEOUT" \
    --cleanup-grace-seconds "$CLEANUP_GRACE" --output "$NEW_OUTPUT"
```

The runner invokes `storm-trident script e2e-matrix <stageType>` and consumes
`TEST_MATRIX_E2E_VM_HOST`. Omit `--scenario` to select the entire generated
matrix. Missing, empty, malformed and fallback/unknown-ring matrices fail.
Unselected scenarios remain `not_run`, never passed.

Registry-backed `extensions` configurations currently fail preflight, rather
than silently dropping their registry arguments or reporting matrix success.
They still require verified OCI/COSI and sysext preparation, existing pipeline
ACR coordinates/flags and authentication, with lifecycle cleanup. No registry
mutation or image push is implemented here. Other missing scenario inputs fail
through Storm's existing setup/test cases, never through a success fallback.

Selected scenarios execute sequentially using the existing pipeline command:
`storm-trident run <SCENARIO> -o <output> -j <junit> -- --pipeline-run --iso <ISO>`.
The runner also passes explicit image directory, test ring, logstream,
tracestream and, when supplied, signing certificate paths. It does not filter
Storm's registered cases, including scenario-local A/B updates and rollback.

Use a new output directory outside the bundle, on the **same filesystem**.
Per-scenario hard-linked image directories allow Storm to create its versioned
aliases without modifying the input inventory or copying large images.
Keep the bundle immutable and do not run another VM suite concurrently.
Timeouts are operator-selected finite positive seconds per command, plus a bounded
cleanup grace. Timeout/interruption signals Storm to run its existing cleanup;
additional SIGINT/SIGTERM signals are ignored until Storm is reaped, without
restarting the cleanup grace or replacing the original failure/cancellation.
If it hangs beyond grace, the process group is killed and cleanup is not assured.
Stop after any failure. No global VM/network/disk wipe is performed, and final
disposal of the outer host remains the platform's responsibility.

`results.json` retains candidate/producer provenance, full matrix variants,
selection, process outcome and actual JUnit passed/failed/skipped/not-run cases.
Missing or inconsistent reports fail, preserving the original process failure.
`matrix.log`, `matrix.json`, `provenance.json`, per-scenario console, JUnit,
serial/artifact output and log/trace files remain for collection by the caller.
These outputs are runtime evidence only when produced by an actual execution.
