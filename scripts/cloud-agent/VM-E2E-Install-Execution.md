# Temporary initial plain VM install execution

This is a temporary execution harness, not a passing guest execution result.
The attempts documented below failed. The parent supplied the independently
qualified initial-cohort
packet after private review of 29 hash/line-count-verified producer logs,
source-build/current-run RPM consumption citations and offline installed payload
measurements. The packet's claims and review reference must not be rewritten.

## Admission and exact inputs

The temporary workflow `vm-e2e-install-execution.yml` admits pushes only to
`user/britel/vm-e2e-install-execution-20261125`, with the same exact job guard.
It selects the existing label `trident-github-runner-ubuntu2204-amd64`. This
follows the temporary inspector's owned-branch push admission pattern, without
assuming a branch-only workflow can already be manually dispatched. Manual
dispatch is retained if admitted by GitHub. Parent orchestration owns independent
review and publication; pushing this branch can start execution and must happen
only after that review. No ADO queue/publishing, registry operation or cloud/ADO
permission change is added.

The workflow supplies the **exact UTF-8 bytes** of the independently reviewed
v1 packet described in [VM-E2E-Install.md](VM-E2E-Install.md), from
`.github/validation/vm-e2e-install-execution/reviewed-packet-1266405.json`.
This is byte-identical to the approved session handoff
`trident-vm-e2e-initial-reviewed-packet-1266405-45697743843.json`, 21692 bytes,
SHA256 `2541fa0bfa52c9b35eae34c22b0e9fa31da57566de4e0b650b44d5991ce3190c`.
Both helper and workflow bind this fixed independently supplied pin. No newline
is appended. A task-specific LF checkout attribute preserves these bytes.
The packet is an explicit reviewed validation handoff, not synthetic test data;
the reviewer, reference and claims are unchanged.
Inputs are limited to 64 KiB. Missing bytes, pin, review fields or qualification
fail closed. There is no fallback image or inference of review approval from
inspection success. Hashing arbitrary local claims cannot replace the fixed pin.
The resolver's existing strict schema and successful timeline citation checks
remain authoritative. Explicit `null` logs on unrelated timeline rows are
treated as absent optional logs, without relaxing cited identity or ancestry.

Source images and packaged tools remain pinned to:

- Product revision `18dd6cd99d50a4268afd936acb1ee4d2b64881ea`,
  source branch `refs/heads/user/britel/cloud-vm-e2e-install`.
- ECF project `2311650c-e79e-4301-b4d2-96543fdd84ff`, definition 5073 revision 7,
  immutable producer build 1266405.
- The harness is the separately reviewed workflow commit (`github.sha`), required
  to match the checked-out HEAD and differ from the product revision.
- Inspection run 45697743843 at harness `2dde287070883543118a7c3ec5e0b01c4031ee13`
  produced report SHA256
  `260d3e130fcd21747e6c3aee212410e75b5bf6da38a0d7af52b358fdef4fc650`.
  This is supporting measurement evidence, **not** the execution packet pin.

## Same-host boundary

Use an exclusive disposable amd64 Ubuntu 22.04 1ES host with its existing mounted
`/mnt/storage` disk, passwordless sudo and preinstalled base tools (`az`, its
azure-devops extension, git, bash, mountpoint, apt-get, pip3 and runuser).
Missing prerequisites fail before dependency installs. The wrapper reuses the
inspection helper's private privileged HOME/Azure profile and pinned output FD.
It initializes only the existing UMI
`e2fd61e6-b160-4f02-a5ac-3d949a4fe01d` inside Linux, without a Windows credential
bridge, and rejects alternate ambient credential variables.

The existing preparer performs acquisition, strict qualification and image
preparation before host installation. The existing `setup-functional.sh` then
prepares only this disposable test host. The approved local effects include its
dependency installs, local libvirt group/lock/service setup and private SSH key,
plus Storm's existing `--pipeline-run` dependency case, local libvirt group update
and `SocketMode=0666` override. This approval does not cover shared hosts,
cloud identities, ADO permissions, new infrastructure or registry mutation.

`--pipeline-run` is a local Storm behavior flag, not dispatch to another pool.
The existing ADO VM/HOST job uses `trident-ubuntu-1es-pool-eastus2`
(`.pipelines/templates/stages/testing_e2e/test_execution_template.yml:55-61`
at the product revision). The GitHub label's backing pool/image mapping is not
established by these files. All preparation and execution here occur on the
single selected GitHub host.

The original install runner acquires the existing
`/var/lib/trident-cloud-agent/functional.lock` for matrix/guest execution.
Pre/post libvirt inventory checks also use that lock. The exclusive-disposable
admission boundary applies to host setup and the gaps between lock acquisitions,
not just the guest process. Preflight refuses existing guests. There is no
host-wide cleanup, deletion of retained resources or attempt to repair uncertain
cleanup.

Evidence and immutable bundle contents remain in a root-private
`/mnt/storage/trident-install-*` directory until outer-host disposal. Runtime
evidence is not chowned to the GitHub job user. Only the exact sanitized report
FD is handed back, using the existing ownership/mode/inode checks. QEMU does not
need access to root-private evidence: this Storm path serves the ISO/COSIs over
HTTP and virtdeploy creates disks/NVRAM in its `/var/lib/libvirt` pools with the
existing explicit permissions. The wrapper checks ancestor search access as
`libvirt-qemu`, without widening evidence-directory permissions. It verifies
the bundle hashes again after successful execution.

## Deliberately bounded selection and results

The wrapper invokes the unchanged `run-vm-e2e-install.py` with
`--stage-type pr-e2e --scenario simple_vm-host`. This follows the exact product
matrix/configuration source:

- `tools/storm/e2e/configurations/configurations.yaml` enables `simple` VM/HOST
  at `pr-e2e`.
- `trident_configurations/simple/trident-config.yaml` uses regular COSI, no UKI,
  registry extensions or A/B storage.
- `tools/storm/e2e/matrix_script.go` supplies the regular `trident-installer`.

No existing case selections or product tools change. All registered cases in
that scenario remain selected, including Storm's own applicable skips. Other
matrix entries remain `not_run`; the summary counts unselected plain, registry,
UKI and unknown configurations separately. It does not silently drop unsupported
configurations and call the complete matrix passed. Missing/unknown/fallback
matrices still fail in the existing runner.

The report cannot claim selected-scenario success without actual passing
`setup-test-host`, `install-os`, `check-trident-ssh` and `validate-install` cases,
consistent selected/unselected results, a successful process and restored
libvirt UUID inventory. Inventory restoration is explicitly **not** proof of
complete host cleanup. Any failure, timeout, cancellation or inventory uncertainty
stops this lane; the host must be disposed of and must not run further lanes.
Successful hosts also require disposal.

### Failed-run diagnostics

The first actual attempt, run 45705918371 at
`cff74badf95ebde70c62e75090f9b757841ad7cb`, failed during the execution phase.
Its bounded report retained no case counts. Exact job 139527734553's log
contained the output redirection (line 193) and exit code 1 (line 200), not
the private runner result or Storm cases. This establishes a diagnostic gap,
not the underlying execution cause. No product or infrastructure fix follows
from that evidence.

The wrapper now collects bounded diagnostics after the runner is reaped,
including on failure, timeout and cancellation, before final publication.
It reads only the existing runner result (at most 1 MiB) and fixed runner,
matrix and selected-scenario log paths (at most 32 KiB head plus 32 KiB tail
each). Result identities must match the pinned source/manifest and selected
scenario. It exports only a bounded process exit code, fixed report/outcome
states, case counts, failed IDs from the eleven existing simple-scenario case
names, an unknown-case count and fixed error-marker labels. Unknown case
names, result error strings and log excerpts are never copied.

Missing, malformed, oversized or mismatched diagnostics are explicitly marked
unavailable/invalid, preserving the original process failure. Fixed log markers
are observations, not proof of root cause. Capturing diagnostics cannot turn
failure into success, weaken any success/packet gate, expand suite selection,
change cleanup or authorize another lane on this host. A fresh reviewed attempt
is needed to obtain execution diagnostics and may not reproduce the original
failure. Disposed-host private logs cannot be reconstructed from the first report.

Run 45709144152 at `72a5a0cff4ed918ae9c1c3fd838e026ad6e001cf` narrowed the
failure to `install-os`: four cases passed, one failed and six were not run.
Only the selected scenario log matched `missing-command-or-file`; this does
not identify a command/path or establish causality. Exact job 139538735556
again exposes redirection (line 193) and exit 1 (line 200), not private case
errors. The retained artifact contains no JUnit or private runtime evidence.
No original failure message is recoverable from that artifact or job log.

On a bound `install-os` failure only, the wrapper now reads the fixed private
`execution/simple_vm-host/storm-e2e.junit.xml` path (at most 1 MiB, regular
file, no symlink, DTD or entity declarations). It requires exactly one matching
scenario suite and failed `install-os` case/class. Storm v0.4.0-alpha3 writes
the case reason into the failure/error `message` attribute; `system-out`,
other cases and console text are not used for this classification.

The exported `installOsFailure` identifies the pinned source's failing operation
by an anchored error prefix. In particular it distinguishes the UEFI XML read,
parse, missing NVRAM node, NVRAM removal, domain redefinition, paused start,
paused stop and `sudo virt-fw-vars` invocation. Only the latter can report its
exact bounded nonzero command exit code. This maps
`tools/storm/e2e/scenario/install.go`, `tools/pkg/netlaunch/netlaunch.go` and
`tools/storm/utils/libvirt.go` at the pinned product revision, without changing
those files or running a diagnostic command.

`missingKnownPaths` can contain only exact static paths used by the selected
VM: the amd64 QEMU emulator, secure-boot OVMF code/vars, `/dev/kvm`, `/dev/pts/2`,
and its fixed serial, NVRAM and two disk paths from virtdeploy. A path is
reported only when immediately followed by `No such file or directory` in the
case error, not merely mentioned in logged configuration or XML. No arbitrary
path, failure message, XML, command argument, private output or unknown prefix
is copied. Missing/invalid evidence and unknown operations remain explicit;
this is an operation-level observation, not an independently proven root cause.
The original failure status and all existing success/cleanup gates remain
unchanged. A reviewed fresh attempt is still needed, not a product fix inferred
from the generic marker.

Run 45712867453 at `072f2c38109e9bea00789d32c3ca9caa49b83871` failed earlier,
in preparation, before any manifest, host setup or guest execution. Exact job
139551435468 exposed only redirection/exit 1, not the private preparation error.
This does not establish a transient service failure or justify a product fix.

On a prepare-phase failure, `preparationDiagnostic` now retains only the existing
preparer footer's exception class and optional HTTP status. The fixed private
`prepare.log` must be a regular non-symlink file of at most 64 KiB. Only an exact
final `Artifact preparation failed (...)` line is parsed, with a closed exception
class allowlist and a status from the standard `HTTPStatus` enumeration.
Unknown/malformed/missing evidence is explicitly `unavailable`, with null fields.
No preamble, raw footer, message or URL is copied; later-phase failures never
reuse this footer. Outcomes, cleanup and the 8 KiB report cap are unchanged.
These fields are observations from the preparer, not an automatic transient
classification or retry authorization. The earlier discarded footer cannot be
recovered from its retained report, and no surviving-host access is assumed.

### Complete bounded failure retention

Run 45715469794 at `a282b11537cec863cd6890c54d31c009c89d3839` failed during
identity, before preparation. Its report and public job log do not distinguish
the extension lookup from UMI login. They establish no identity endpoint,
authentication or permission cause. Local packet checks precede both commands;
ADO token acquisition and server qualification follow them.

The wrapper now checkpoints fixed source operation IDs through the existing
pinned publication FD. `failure` latches the first observed failure;
`cleanupFailure` separately retains the first later diagnostics, inventory,
signal-restoration or privileged-publication failure. Neither overwrites the
initiating fact. Each fact has exactly four fields:

- `operation`: a closed source-defined enum, never a command or arbitrary path.
- `exitCode`: an observed bounded process/helper status, otherwise null.
- `exceptionClass`: a closed class-name allowlist, otherwise null.
- `httpStatus`: a standard status from a typed `HTTPError` in a bounded explicit
  cause chain, otherwise null. CLI stderr and exception messages are not parsed.

Coverage includes these boundaries, without adding commands:

| Boundary | Fixed operation coverage |
| --- | --- |
| Admission | Signals, exclusive host, each prerequisite, ambient credentials, harness argument/read/binding, storage mount, private runtime |
| Packet and identity | Read, byte pin, schema, producer, review, private copy, extension lookup, UMI login |
| Acquisition | Arguments, token, build, timeline, stages, log metadata, packet/log/RPM/artifact qualification, artifact list and each known artifact validation/download/extraction |
| Bundle preparation | Output creation, tools, installers, image helper and verification, optional certificate, staging cleanup, manifest write/verification/promotion |
| Host and guest | Host setup/lock, platform/release/KVM/tools/libvirt gates, each inventory and validation, empty-host guard, each fixed QEMU ancestor, selected runner process/results, bundle recheck |
| Completion | Private diagnostics, final lock/inventories/comparison, signal restoration, final report and privileged ownership handoff |

`lastStartedOperation` has the explicit
`operationBasis=last-started-not-proof-of-cause-or-completion` qualification.
Later cleanup checkpoints can advance it. It is not a failing operation
assertion. Intermediate checkpoints never claim selected-scenario success.
Observed failure facts, including child cleanup failures, prevent final success.

The opt-in preparer handoff is a create-once, owner-matching 0600 regular file
inside the 0700 private runtime directory. Writes use its pinned FD; reads reject
symlinks, hardlinks, oversized files and unknown schemas/operations. Its maximum
size is 2 KiB. Only validated `state`, four-field `fact` and `cleanupFailure`
are forwarded in `preparationHandoff`. The parent polls during the existing
bounded process wait and after reaping; a `started` handoff preserves the last
child boundary on interruption without inventing a cause. Missing or invalid
handoffs are explicitly unavailable. The original acquisition error is recorded
before staging cleanup can replace it.

Shared command observers, host checkpoints and secondary privileged-publication
callbacks are opt-in. Default inspector, preparer and install-runner callers keep
their existing behavior. Native output remains private, and observer failures
still reap the child process group. No identity, permission, packet, product,
guest case selection or retry-policy change follows from this instrumentation.
Timeout/cancellation is latched before post-reap observers. An opt-in integer
status or the original raised exception privately retains an observer exception;
the wrapper exports it only as a separate closed cleanup fact. A failed observer
cannot replace status 124/130 or the initiating exception.

There is no unsafe publication fallback. Failure before the protected FD exists,
an unwritable report, SIGKILL or host loss can prevent a complete final record.
The safe sentinel or latest successful checkpoint can survive, not a fabricated
cause or proof of cleanup. Such hosts still require disposal.

Preparation is bounded to 1200 seconds (its image helper to 600), host setup to
900, matrix and selected scenario to 1800 each, and their outer install-runner
process to 3780. Existing leader-first Storm cleanup gets 120 seconds internally
and 180 seconds in the wrapper. Preparation/setup use process-group cleanup.
The whole root context is bounded by 125 minutes plus 300 seconds before force
kill, inside larger workflow limits. Cancellation signaling and reaping reuse
the reviewed process helpers. A forced kill is never treated as proven cleanup.

Only `trident-install-report/execution.json`, strictly below 8 KiB, is uploaded. Its
allowlist contains fixed scope/outcome/phase/diagnostic values, exact
source/harness/pin identities, fixed operation facts and bounded
counts/booleans/exit code. It never
includes raw console, errors,
environment, command arguments, keys, certificates, images, packet/log excerpts
or authentication/download URLs. Preparation logs, full runner results, JUnit,
serial output and trace evidence remain private on the disposable host.

Other plain configurations, registry, UKI, legacy, lifecycle, ACL-agent,
container, ARM64, Azure-platform and bare-metal coverage are explicitly not run.
This first actual-install lane is not the eventual all-VM-compatible test plan.
