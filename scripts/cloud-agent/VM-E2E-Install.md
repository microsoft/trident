# Explicit Storm VM/HOST install/configuration

`run-vm-e2e-install.py` is an opt-in entrypoint for the existing Storm
installation/configuration scenarios. It is not functional `full`, legacy
netlaunch/pytest E2E, QEMU servicing/rollback/UKI, ACL-agent, container, ARM64,
Azure-platform or bare-metal coverage. Source `validate.sh all` and agent
startup remain source-only.

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
artifact format. The resolver/build integration that emits it is still required.
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
