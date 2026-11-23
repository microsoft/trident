# Fixed sequential supported VM aggregate

`run-vm-e2e-aggregate.py` only composes the existing reviewed drivers. Run as
root on the explicitly named **exclusive disposable amd64 Ubuntu 22.04 1ES
outer host**, never a shared host. Host setup, artifact qualification, local
registry image preparation, credentials and final outer-host disposal belong
to the operator. This command neither downloads nor provisions them.

## Fixed order and boundary

1. Fresh B validation harness execution, including the normal
   `run-functional.sh smoke` and `full`, real candidate-binary upload evidence,
   the existing controlled failure/interruption checks and preservation checks.
2. Supported Storm VM/HOST scenarios, sorted from the actual
   `storm-trident script e2e-matrix full-validation` output. Each invokes the unchanged
   install driver with its explicit scenario.
3. Supported legacy VM/HOST configurations, sorted from the existing matrix
   helper's `validation` selection, in one existing legacy driver invocation.
4. Existing lifecycle driver, without a lane filter: servicing-qemu,
   servicing-uki, rollback-qemu, rollback-uki.
5. Existing ACL-agent driver, all eight cases.

No commands, entrypoints, ring, scenario filters or configuration files can be
supplied by the caller. Each C family takes its own existing manifest, unchanged.
All family inventories, hashes, source/cohort identities, selected matrix inputs,
matching SSH identity, local Docker images, dependencies, network ports and
host prerequisites are checked before the first suite invocation. Drivers repeat
their own preconditions under their own locks. These checks are not a reservation
against an external actor changing an exclusive host.

The shared functional lock is acquired and **released** during preflight.
It is not inherited/held across child driver invocation. Each driver retains
its existing ownership, lock, signals and cleanup. No global Docker/libvirt
cleanup, private ancestor permission changes or dependency scanner is added.
Only the two new owned aggregate/preflight directories receive the configured
nonroot QEMU group's search permission; their existing ancestors must already
permit search. The existing per-driver output/filesystem requirements apply.
The aggregate retains umask `0077`. Only its ACL child receives umask `0066`,
so the unchanged ACL driver's new directories are `0711` (search-only for
nonowners) and new evidence files remain `0600`. This lets nonroot QEMU reach
its libvirt-owned boot disk without reading private evidence or listing the
directories. Other children retain `0077`; existing files and ancestors are
never chmodded to compensate.

The summary explicitly accounts for unsupported registry-backed installation
`extensions` and legacy `misc`/`extensions` as **unavailable**, not passed.
It records excluded ARM64, bare-metal, Azure-resource provisioning, VM/container,
scale/direct-streaming and the legacy driver's documented
pipeline-only extras. GRUB-without-verity stays excluded because the actual
caller disables it. The unsigned UKI cohort is **not Secure Boot qualification**,
even though existing root-verity callers retain their Secure Boot flags.
This does not enable any of those lanes. No new Go exclusion is introduced:
exactly `TestUefiFallbackScriptMatchesLegacy` remains excluded by B, and the
full Go suite is explicitly not validated.

`supportedVmAggregatePassed` means **all fixed executable lanes actually passed
in this fresh invocation**, not that unavailable/excluded coverage passed.
Any unavailable entry in the selected installation/legacy matrices prevents
`fullAggregateOK`: otherwise successful execution reports `incomplete`, exit 1.
The current registry-backed gaps therefore prevent a full-green aggregate.
They need separately approved driver enablement, not an aggregate workaround.
Excluded external lanes and unqualified Secure Boot remain separate accounting;
`allTridentE2E` is always false, even for an aggregate `passed`.
No prior B results or caller-supplied result paths are accepted.

## Immutable inputs and pending runtime gates

- Product SOURCE root: clean HEAD exactly
  `18dd6cd99d50a4268afd936acb1ee4d2b64881ea`.
- Functional HARNESS root: a separate clean checkout at reviewed immutable
  `076bc60a1830a84b6006f68f78981e7c10d17de6` (parent `d925382...`).
  Its fixed entrypoint is
  `scripts/cloud-agent/functional-validation.py`. The parent-approved adapter
  accepts `--expected-source-revision`, retains 985/51 as default, and
  supports explicit 18dd/70 with complete candidate-label propagation.
  The caller must pass that exact reviewed harness revision explicitly.
  Do not supply the old 985-only harness or override constants.
- Installation and legacy manifests: qualified original definition 5073
  build **1266405**, product18dd. Lifecycle and ACL manifests: qualified
  definition 5073 build **1266514**, product18dd. Build success alone is not
  qualification. All four independently trusted manifest pins are required.
- Lifecycle Storm: separately built from committed source
  `928feb75a262344a41639b600d5c6174b1ae4b1b`, with the independently supplied
  binary SHA256 and existing `lifecycle.toolProducer` provenance. The existing
  lifecycle reader verifies these pins and the container-ownership hook.
  Native checks or an old 18dd tool cannot substitute for this build.
- Lifecycle and ACL must identify the same producer/cohort. One external
  protected private key is passed to both; preflight derives its public key
  and matches the inventoried lifecycle public key. Independent image
  qualification must bind that key to the ACL and lifecycle images.
- Qualification must also cover installed candidate packages, base/MIC inputs,
  certificates, guest configuration, and already-local Postgres/MIC digests as
  required by the existing driver documents. No new manifest metadata is added.
- The B harness retains its own real functional image resolution/cache provenance
  checks, separately from C images. It must supply fresh smoke/full evidence.
  Historical run 45252937219 is not aggregate evidence.

On the prepared host, using real approved inputs:

```bash
export EXCLUSIVE_DISPOSABLE_1ES=true
# GITHUB_ACTIONS, GITHUB_RUN_ID and GITHUB_RUN_ATTEMPT identify the actual 1ES job.
python3 scripts/cloud-agent/run-vm-e2e-aggregate.py \
  --source "$PRODUCT_SOURCE_ROOT" \
  --functional-harness "$REVIEWED_ADAPTER_ROOT" \
  --functional-harness-revision 076bc60a1830a84b6006f68f78981e7c10d17de6 \
  --install-bundle "$INSTALL_BUNDLE" --install-manifest "$INSTALL_MANIFEST" \
  --install-manifest-sha256 "$INSTALL_PIN" \
  --legacy-bundle "$LEGACY_BUNDLE" --legacy-manifest "$LEGACY_MANIFEST" \
  --legacy-manifest-sha256 "$LEGACY_PIN" \
  --lifecycle-bundle "$LIFECYCLE_BUNDLE" --lifecycle-manifest "$LIFECYCLE_MANIFEST" \
  --lifecycle-manifest-sha256 "$LIFECYCLE_PIN" \
  --acl-agent-bundle "$ACL_BUNDLE" --acl-agent-manifest "$ACL_MANIFEST" \
  --acl-agent-manifest-sha256 "$ACL_PIN" \
  --expected-tool-revision 928feb75a262344a41639b600d5c6174b1ae4b1b \
  --tool-sha256 "$QUALIFIED_NEW_STORM_SHA256" \
  --ssh-private-key "$PROTECTED_MATCHING_COHORT_KEY" \
  --expected-hostname "$EXCLUSIVE_1ES_HOSTNAME" --exclusive-disposable-host \
  --qemu-user "$EXISTING_QEMU_ACCOUNT" \
  --timeout-seconds "$PER_DRIVER_COMMAND_BOUND" \
  --cleanup-grace-seconds "$PER_DRIVER_CLEANUP_BOUND" \
  --lane-timeout-seconds "$WHOLE_CHILD_INVOCATION_BOUND" \
  --shutdown-grace-seconds "$WHOLE_CHILD_CLEANUP_ALLOWANCE" \
  --output "$NEW_OUTPUT_UNDER_PREPARED_SEARCHABLE_PARENT"
```

The shutdown allowance must accommodate the existing driver's sequential bounded
cleanup operations, not just one per-command grace. A lane timeout is 124;
aggregate cancellation retains its first 128+signal exit. C drivers receive
leader-only cancellation and clean their own nested sessions. The B harness
receives cancellation in its aggregate-owned process group, since it and its
normal functional children inherit that group and B's handler only records
interruption. This lets the existing functional EXIT cleanup run. Repeated
signals do not reset grace. Bounded force or any failed/unknown cleanup requires
outer-host disposal; the aggregate never asserts that killing a Docker client
settles an asynchronous daemon request. Empty container scans are not proof.
SIGKILL, power loss and independently created process sessions are outside the
cleanup guarantee. The platform must dispose of the outer host.

## Evidence and checks

Stop on the first failure, including missing/mismatched fresh reports. Preserve
the primary exit and first failing lane; later lanes stay `not_run`.
Failed all-input preflight reports required families as `unavailable`.
The aggregate summary contains only bounded JSON identities, status/counts and
fixed coverage reasons, not commands, input paths, private bytes or raw errors.
Per-lane driver output and preflight logs remain **private runtime material**,
not an upload selection. Only the bounded sanitized `summary.json` is suitable
for review after operator inspection.

Focused Python unit tests use short-lived synthetic processes and temporary
files, never VM/Docker execution. Run on native Linux `/tmp`, not a Windows
mount with different permission semantics. They are not E2E proof. Actual
per-family qualification and the fresh sequential 1ES batch remain pending.
