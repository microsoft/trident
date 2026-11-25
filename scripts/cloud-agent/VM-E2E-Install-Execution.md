# Temporary initial plain VM install execution

This is execution scaffolding, not an execution result. No guest has been run
by this slice. The parent supplied the independently qualified initial-cohort
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

Preparation is bounded to 1200 seconds (its image helper to 600), host setup to
900, matrix and selected scenario to 1800 each, and their outer install-runner
process to 3780. Existing leader-first Storm cleanup gets 120 seconds internally
and 180 seconds in the wrapper. Preparation/setup use process-group cleanup.
The whole root context is bounded by 125 minutes plus 300 seconds before force
kill, inside larger workflow limits. Cancellation signaling and reaping reuse
the reviewed process helpers. A forced kill is never treated as proven cleanup.

Only `trident-install-report/execution.json`, at most 8 KiB, is uploaded. Its
allowlist contains fixed scope/outcome/phase values, exact source/harness/pin
identities and bounded counts/booleans. It never includes raw console, errors,
environment, command arguments, keys, certificates, images, packet/log excerpts
or authentication/download URLs. Preparation logs, full runner results, JUnit,
serial output and trace evidence remain private on the disposable host.

Other plain configurations, registry, UKI, legacy, lifecycle, ACL-agent,
container, ARM64, Azure-platform and bare-metal coverage are explicitly not run.
This first actual-install lane is not the eventual all-VM-compatible test plan.
