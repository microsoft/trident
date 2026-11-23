# Explicit legacy netlaunch + pytest VM/HOST phases

`run-vm-e2e-legacy.py` is additive, opt-in and sequential. It runs the existing
VM/HOST netlaunch installation and `tests/e2e_tests` validation paths, not Storm's
ported Go validations and not pytest collection presented as runtime evidence.
It does not provision hosts, queue producers, download artifacts, authenticate,
or run servicing, rollback, ACL-agent, container or bare-metal scenarios.

## Selected boundary

The explicit suite `legacy-vm-host-install-ab` selects these phases from
`.pipelines/templates/stages/testing_vm/netlaunch-testing.yml` and
`testing_common/e2e-test-run.yml`:

1. Prepare configuration with existing `edit_host_config.py`, PEM conversion,
   and `inject_uefi_fallback_validation.py` (except `rerun`).
2. Existing `tools/virt-deploy create --mem 12 --disks 32,32`, then candidate
   netlaunch with Secure Boot, provisioned-state handling and existing
   `--max-failures 1` handling for rerun/memory-constraint-combined.
3. Existing check-ssh, check-tracing and check-selinux helpers, then pytest on A.
4. Existing `helper ab-update` stages/finalizes image version 2 into B, then pytest on B.
5. The same helper stages/finalizes version 3 into A, then pytest on A.
   Each update starts a fresh one-shot netlisten, requires its bounded completion
   with exit zero after phone-home, and reaps its process group before proceeding.
   Nonzero exits and completion timeouts fail the suite. Logs/metrics are per update.
6. Reap only this suite's netlisten process group and clean only its generated
   `legacy-<random UUID>-` virt-deploy namespace, including partial creation.
   Verify absence of its exact domain, pool and network names, since the existing
   cleanup helper can log deletion errors without returning a failing exit code.

Host Configuration script hooks and health checks are preserved, not replaced.
`health-checks-install` retains `--only-print-exit-code`,
`--expect-failed-commit`, and pytest's `not-provisioned` expectation.
Configurations without `storage.abUpdate` explicitly skip the four update phases.
No VM or disk cleanup occurs between phases. A/B flags express expected state,
and `activeVolumeAfter` is recorded only after phase success.

This is **not the entire pipeline job**. Auto/manual rollback, independently
staged/finalized servicing tests, rebuild-raid, screenshots, serial-log/disk
publication and Kusto metrics upload are not selected or claimed. In particular,
the automatic rollback interlude between B and A in the full pipeline is excluded.
These omissions do not filter pytest tests: the existing matrix helper selects
VM/HOST configurations and each pytest invocation retains the requested
`buildPurpose` marker and configuration's existing `test-selection.yaml`.

## Prerequisites and artifact selection

Use the exclusive disposable amd64 Ubuntu 22.04 host and **same functional lock**
described in [VM-E2E-Install.md](VM-E2E-Install.md). Host setup remains external,
including sudo, libvirt/KVM, OVMF, swtpm, virt-firmware and Python dependencies
from `netlaunch-prep.yml`/`e2e-test-run.yml`. No dependency is installed here.
Missing dependencies fail as unavailable, not passed.

Reuse C's exact pinned v1 bundle/manifest schema and trust model, with the
following **additional explicit inventory requirements**, all from the same
candidate producer build:

- Executable `bin/netlaunch` and `bin/netlisten`, each from `go-tools`, in addition
  to `bin/storm-trident`.
- The configuration's installer ISO (`trident-split-installer` for `split`,
  otherwise `trident-installer`), with matching artifact attribution.
- Prepared `artifacts/test-image/{regular,verity,usrverity}.cosi` as actually
  referenced by each selected config. A/B configs also require `_v2.cosi` and
  `_v3.cosi` for that image kind, attributed respectively to `trident-testimage`,
  `trident-verity-testimage` or `trident-usrverity-testimage`. Use existing
  `helper prepare-images` producer preparation, not runner-created aliases or
  silent downloads. This legacy helper can intentionally create version aliases;
  they are not evidence of distinct lifecycle image production.
- UKI configs require the inventoried `signingCertificate` from
  `trident-usrverity-testimage`.

`misc` and `extensions` are explicitly unavailable until authenticated,
candidate-provenance-checked registry preparation/cleanup is supplied. They are
never silently replaced with HTTP images or removed from the matrix. Every
selected input is checked before any VM starts. The source checkout supplied by
the caller must be the intended candidate's trusted tests/helpers.

Producer definition 5073 revision 7, build 1266405, source `18dd6cd` was queued,
not successful artifact or runtime evidence. Its seven selected producer stages
do not establish availability of these extra tools, prepared aliases, certificate,
registry inputs or distinct lifecycle images. Those requirements remain explicit
handoff prerequisites. No lifecycle/runtime stage is queued by this runner.

## Invocation and evidence

```bash
python3 scripts/cloud-agent/run-vm-e2e-legacy.py \
  --suite legacy-vm-host-install-ab --build-purpose pullrequest \
  --configuration base --configuration split \
  --bundle "$BUNDLE" --manifest "$MANIFEST" \
  --manifest-sha256 "$TRUSTED_MANIFEST_SHA256" \
  --expected-revision "$CANDIDATE_SHA" \
  --timeout-seconds "$COMMAND_TIMEOUT" \
  --cleanup-grace-seconds "$CLEANUP_GRACE" --output "$NEW_OUTPUT"
```

Output must be new, outside both source and bundle, on the bundle's filesystem.
Private workspaces beside output hold copied configs and a freshly generated SSH
key, and are removed after cleanup. Do not collect these workspaces. No private
key is downloaded or copied into evidence. Bundle/source files remain unchanged.

`results.json` records provenance, the full configuration matrix, selection,
per-phase execution, expected state, actual pytest case outcomes and cleanup
status. Unselected and post-failure phases remain `not_run`. Pytest skipped cases
remain skipped; empty, all-skipped, failed or missing reports cannot pass.
Matrix selection/collection alone is never execution. Logs and JUnit remain under
output. All commands are bounded, first failure wins, and remaining configurations
stop on failure. Cancellation signals and reaps owned process groups, with repeated
signals ignored during cleanup while retaining the shared lock. Hard termination
or failed namespace cleanup means cleanup is not assured, and outer host disposal
remains the platform's responsibility. Actual runtime qualification belongs to the
parent operator; focused mocked tests are not VM qualification.

The shared command helper also tears down residual process groups on its default
C path, including when the leader exits within cleanup grace. Storm retains
leader-first graceful cancellation; its surviving descendants cannot bypass the
final group kill by outliving that leader.
