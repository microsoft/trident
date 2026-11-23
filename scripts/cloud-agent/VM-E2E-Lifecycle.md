# Explicit QEMU servicing and rollback

This candidate includes the optional Storm owned-container hook and bounded
cleanup. It still needs independent review, a separately qualified new Storm
build, qualified images/keys and disposable-host execution. No VM result is
claimed.

`run-vm-e2e-lifecycle.py` runs the **existing** Storm suites, sequentially:
`servicing-qemu`, `servicing-uki`, `rollback-qemu`, `rollback-uki`.
It does not install a new OS between cases within a suite. Each suite gets
private, mutable copies of its inputs and one domain/disk chain. The original
bundle is never modified or hard-linked to mutable suite images.

## Caller and coverage

The selection follows `.pipelines/templates/e2e-template.yml` and
`stages/testing_{servicing,rollback}/{vm-testing,testing-template}.yml`:

| Lane | Existing flags/behavior |
|---|---|
| servicing-qemu | QEMU root-verity QCOW2, update A/B, Secure Boot, three update and automatic rollback iterations |
| servicing-uki | UKI `/usr`-verity QCOW2, update A/B, three update and automatic rollback iterations, no Secure Boot flag |
| rollback-qemu | Root-verity QCOW2 and update A, Secure Boot, `--uki` as the actual caller passes it, skip extensions/runtime/netplan runtime |
| rollback-uki | UKI QCOW2 and update A, `--uki`, existing sysext builder and Image Customizer preparation, full enabled runtime/manual rollback chain, no Secure Boot flag |
| rollback-qemu-grub | **Excluded/not run**, caller explicitly sets `includeQemuGrub: false` |

The root-verity `qemu` lane itself uses GRUB; the excluded lane is specifically
GRUB **without verity**, not all GRUB coverage. Azure publishing is a genuine
Storm skip. Root-verity rollback also skips `skip-to-ab-rollback`. Other required
cases must actually pass, not merely appear in a report or be skipped.

This is not ACL-agent, ARM64, bare-metal, Azure-resource, container-runtime,
scale, direct-streaming, installation/configuration or legacy E2E coverage.
No host setup, producer queueing, image injection framework, global virsh
cleanup, credential acquisition, account changes or changes to existing
ancestor permissions are included.

## Shared helper integration

The lifecycle files were authored on `fa0b19360ca925990fd91f57aa75ab5b45248f27`
and fast-forwarded to the independently approved shared-owner integration
`7642d77a5dd302c4f238391cf2422cefec42d907`, then the disjoint reviewed ACL
integration `ee894e09d45e8051f813bf4e05b2e767d9ff8d3d`. The installer reader,
artifact resolver and cancellation helpers remain unchanged. The shared-helper
base fixes **default** `command()` process-group cleanup,
including parent-exits-before-child, original 124/130 outcomes and bounded
repeated-signal handling.

Finalization reuses its existing public API:

```python
@contextmanager
def cleanup_signals():
    """Defer SIGINT/SIGTERM during bounded finalization; restore handlers on exit."""
```

The driver requires this callable before starting any VM. It uses the original
`command(argv, cwd, log, timeout, grace)`, `host_lock()`, `check_host()`,
`verify_bundle()`, `case_results()` and timeout parser. It does not duplicate
signal/process-group implementation. Lifecycle regressions exercise the real
shared guard with repeated signals while asserting that the host lock is held.

## Immutable, independently qualified cohort

Use the existing v1 C manifest verification contract documented in
[VM-E2E-Install.md](VM-E2E-Install.md), with this small additive `lifecycle`
family. The manifest pin must come from an independent trusted review channel.
The product candidate may remain
`18dd6cd99d50a4268afd936acb1ee4d2b64881ea` while the orchestration source is
`ee894e0...` plus this change. The new Storm binary needs its own source and
binary pins; it is not the old candidate tool relabelled. No image rebuild is
required for this tool-only hook or each orchestration-only revision.

The bundle must include:

- Executable `bin/storm-trident` and `bin/netlisten`, artifact `go-tools`.
  These retain the unchanged original reader contract.
- Executable `lifecycle-bin/storm-trident`, artifact `lifecycle-go-tools`,
  from the separately reviewed committed source containing the new hook.
  Only this Storm binary is staged/executed for lifecycle operations.
- `ssh/id_rsa.pub`, artifact `ssh-keys`. **No private key in the bundle or output.**
- For each flavor and role, exactly the selected file below, with the usual
  positive size, SHA256 and actual pipeline artifact name in `files`:

| Flavor | Basename | Roles | Artifact name |
|---|---|---|---|
| qemu | `trident-vm-grub-verity-testimage` | `base`, `update-a`, `update-b` | `image-qemu-<role>` |
| uki | `trident-vm-usr-verity-testimage` | `base`, `update-a`, `update-b` | `image-uki-<role>` |

Paths are
`artifacts/test-image/lifecycle/<flavor>/<role>/<basename>.<extension>`,
using `.qcow2` for base and `.cosi` for updates. Both updates must have distinct
hashes. Staging preserves the same COSI basename in `update-a` and `update-b`,
as required by the servicing code. Rollback stages only update A beside base.
Other inventoried C-installation files can coexist, but are not used or counted.

The additive `lifecycle` object has:

- `schemaVersion`: `1`.
- `review`: minimal `reviewer` and immutable `reference` identity strings.
- `stages`: exactly `PrepareSSHKeys`, `BuildImagesQemu`, `BuildImagesUKI`,
  each mapped to its reviewed, successful timeline stage UUID in the same
  immutable producer build already identified by the manifest.
- `imageCustomizerImage`: exact
  `mcr.microsoft.com/azurelinux/imagecustomizer@sha256:<64 lowercase hex>`.
  No tag, wildcard or latest fallback. Preflight requires it locally; existing
  Go preparation retains its own digest-pinned image lookup/pull behavior.
- `rollbackScriptSha256`: independently reviewed SHA256 of the repository's
  `.pipelines/templates/stages/testing_common/scripts/trigger-rollback.sh`.
  Servicing requires that relative path; the driver copies only that pinned
  existing script into its private working directory.
- `toolRevision`: full lowercase 40-hex source commit of the new Storm tool.
- `toolSha256`: full lowercase 64-hex binary digest, matching the inventory
  entry for `lifecycle-bin/storm-trident` and the independent CLI pin.
- `toolProducer`: exactly `organization`, `projectId`, positive integer
  `definitionId` and `buildId`, and `sourceVersion`. Organization/project use
  the existing ECF values; sourceVersion must equal `toolRevision`.

The independently supplied `--expected-tool-revision` and `--tool-sha256`
must match this family, separate from `--expected-revision` for product/images.
UKI rollback also checks that the pinned tool advertises the new flag.
The family review must bind this exact tool build and image cohort. A local
build of this uncommitted candidate is validation only, not a qualified tool
attestation. After clean review and parent commit, an approved committed-source
tool build and reviewed provenance are still required. Old 18dd images remain
18dd; an old 18dd Storm tool is not claimed to support the new flag.

This is an offline **trusted handoff**, not automatic producer qualification.
The existing artifact preparer does **not** emit this family. Parent-owned
qualification must bind candidate RPM/guest package identities, source checkout,
resolved base/MIC inputs, distinct filesystem UUIDs, embedded SSH public key,
update configurations and signing/certificate requirements to each exact image
hash. Stage UUIDs and a locally recomputed manifest pin are not proof.
The runner verifies hashes and cohort metadata, not installed guest packages.

Producer `1266405` has succeeded but remains unqualified for guest/package
lineage and did not produce QEMU/UKI lifecycle images or `PrepareSSHKeys`.
Parent reports producer `1266514` queued once with per-build user approval,
selecting SSH keys, RPM, QEMU, UKI and ACL stages at product source18dd with
verified template pins; the other 43 stages are skipped. No images are yet
qualified. Required lifecycle stages are `PrepareSSHKeys`, `BuildImagesQemu`
and `BuildImagesUKI`; original reader/candidate tools and RPM provenance must
also be qualified, not assumed produced by those five stages. The separately
pinned new Storm tool needs its own approved producer. No producer/stages are
queued or changed here. A missing cohort is unavailable, never a pass.

## Host and invocation

Only the explicitly named exclusive disposable amd64 Ubuntu 22.04 1ES host,
with root, KVM, libvirt, QEMU, UEFI firmware, virt-install, sudo, SSH tools,
lsof, `setpriv` and the existing functional execution lock, is supported. Ports 8000/8001
must be unused. UKI rollback additionally needs Docker access, `mksquashfs`,
and the approved digest-pinned Image Customizer already loaded in root's daemon.
Registry authentication/image acquisition and certificate qualification stay
with the parent. There is no signing-cert CLI in these existing lifecycle
suites, unlike C-installation; UKI lanes deliberately follow the caller's
non-Secure-Boot selection. Do not repair unrelated firmware drift here.

Supply the matching unencrypted private key through a separately protected
Linux path (0600 or stricter). The driver verifies its derived public key
against the inventoried public key without logging private key contents.
Do not download images or private keys to Windows.

```bash
sudo python3 scripts/cloud-agent/run-vm-e2e-lifecycle.py \
    --bundle "$QUALIFIED_LINUX_BUNDLE" --manifest "$PINNED_MANIFEST" \
    --manifest-sha256 "$INDEPENDENT_MANIFEST_SHA256" \
    --expected-revision "$PRODUCT_CANDIDATE_SHA" \
    --expected-tool-revision "$COMMITTED_STORM_SOURCE_SHA" \
    --tool-sha256 "$INDEPENDENT_STORM_BINARY_SHA256" \
    --expected-hostname "$EXCLUSIVE_DISPOSABLE_1ES_HOSTNAME" \
    --qemu-user "$CONFIGURED_NONROOT_QEMU_ACCOUNT" \
    --exclusive-disposable-host --ssh-private-key "$PROTECTED_LINUX_KEY" \
    --timeout-seconds "$PER_COMMAND_BOUND" \
    --cleanup-grace-seconds "$CLEANUP_BOUND" --output "$NEW_PRIVATE_OUTPUT"
```

Optional `--lane` selects one of the four enabled lanes; other lanes remain
explicitly not run. All cohort files are verified even for a single lane.
Output must be new and outside the bundle. Named domain absence must be
positively established before execution; non-English/unknown virsh errors fail
closed rather than treating connection/permission failures as domain absence.

`--qemu-user` defaults to Ubuntu's `libvirt-qemu`; it must name the actual
existing nonroot QEMU account with a nonroot primary group. New output, lane
and artifact directories grant that group **search only** (0710). Evidence
files and staged source images remain root-private (0600), other evidence
directories remain 0700, and a restrictive umask is inherited by Storm and
restored when execution returns. Libvirt's existing DAC ownership handling
grants QEMU access to `booted.qcow2`, not to logs or source images.

The existing output parent must already be searchable by that QEMU identity,
without inherited default ACLs. Preflight probes real access with `setpriv`;
private/unowned ancestors are never chmodded or chowned. A private parent is
an unavailable prerequisite, not a reason to broaden its permissions. Root
cross-UID regressions verify actual boot-disk read/write, denied evidence reads
and directory listing, and unchanged existing ancestor permissions.

## Evidence and cleanup

`results.json` retains the pinned provenance, lane selection/execution,
commands, case outcomes, stable zero-inclusive counts, first failure and separate
cleanup failures. Per-lane directories retain suite JUnit/console/logs, mutable
base and `booted.qcow2` disks, plus locally generated sysext hashes for UKI
rollback. A missing, partial, skipped-only or malformed success report fails.
Later lanes stop on the first failure and remain not run.

The existing shared host lock covers preflight, all sequential suites and
bounded suite-owned cleanup. Both suites receive `--force-cleanup`. Rollback's
Go `CleanupVM` currently does not delete QEMU domains, so finalization additionally
runs the existing **servicing** `cleanup-vm` case for the exact unique owned
name and verifies that name is absent. It never cleans another name or deletes
the retained disks. Cleanup failure cannot replace an earlier failure.
Each cleanup/query command gets `cleanup-grace-seconds` plus the shared helper's
bounded cancellation grace. Repeated signals are deferred by the shared guard.

UKI rollback persists `container-intent.json` before the sysext producer and
suite, refuses any pre-existing exact container name, then opts into
`--image-customizer-container-name <unique-domain>-mic`. The additive Go option
defaults empty, preserving existing callers and Docker argv. Nonempty names
are validated and add only `--name` and
`--label com.microsoft.trident.lifecycle-owner=<name>`.

Under the same host lock and cleanup signal guard, finalization queries that
exact name, inspects only its immutable ID/name/ownership label (not environment
variables), removes only the verified immutable ID and checks exact-name
absence. Container and domain cleanup are attempted independently; neither
replaces the first failure. No global Docker cleanup is used.

Killing Storm/client processes cannot retract an accepted daemon create.
Empty snapshots, including repeated scans, cannot rule out late registration.
Every interrupted, failed or incomplete UKI rollback invocation therefore
records `outcome: uncertain` and `requiresHostDisposal: true`, even after
observed-ID removal and `absenceObserved: true`. Ownership/query/removal errors
also require disposal. The overall result carries that requirement, stops
later lanes and preserves original 124/130. Parent must dispose of the outer
exclusive host before reuse; this runner never provisions or deletes it.
Only a completed successful suite plus verified absence can report clean.
Independent Python fake-daemon regressions exercise survival, late registration
after empty scans, wrong ownership and removal failure using real shared
command cancellation, actual subprocesses, guard and lock. They start no
Docker daemon/container or VM and install no PATH shim.

Evidence remains private; search-only QEMU access is not an upload selection.
Do not publish raw logs,
disks, environment dumps, authentication output or unreviewed reports.
Actual firmware/guest/registry readiness also requires parent qualification on the
disposable host. No real VM/E2E result is claimed by the
mock/native process regressions in this slice.
