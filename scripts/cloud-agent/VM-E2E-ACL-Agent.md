# Existing ACL-agent VM lane

`run-vm-e2e-acl-agent.py` runs **one** existing Storm `aclagent` scenario on an
exclusive disposable amd64 Ubuntu 22.04 1ES host. No source validation, startup,
pipeline or other runner calls it automatically. No Azure resource provisioning,
login, image download/pull, permission change, push or queue is implemented.

This additive driver uses the public `command()`, `cleanup_signals()`,
`host_lock()`, `verify_bundle()` and JUnit helpers from `run-vm-e2e-install.py`.
The original harness base is fa0b193; approved shared/legacy revision
`7642d77a5dd302c4f238391cf2422cefec42d907` is fast-forward integrated.
Its default-path residual process-group kill correction is required: the old
helper could return after its parent exited while a signal-ignoring child
survived. No shared runner edits or duplicate cancellation implementation are
part of this ACL-agent slice. Local tests are not VM cleanup or guest proof.

## Existing command and state contract

The caller is `.pipelines/templates/stages/testing_acl_agent/trident-acl-agent-test.yml`,
included by `e2e-template.yml`. The implementation is
`tools/storm/aclagent/{trident.go,utils/config/config.go,tests,proxies}`.
The driver preserves all eight registered cases, in order, within one process
and VM: deploy, check deployment, node resilience, node-name source, A/B update,
rollback, collect logs, cleanup. There is no separate per-case selection/reboot
driver, new test matrix or blind baseline substitution.

It uses the caller's `storm-trident run aclagent` path with `--force-cleanup`,
explicit base/update patterns, SSH key, ports, OS version and Postgres image.
Only the existing local QEMU path is selected, never `--platform azure`.
Unlike the pipeline it omits `-a` and forces `TF_BUILD=false`, so it does not
enable Storm's ADO artifact-upload integration. It omits verbose environment
logging. It uses root, as the existing caller does, without installing packages
or changing sudo/libvirt/Docker permissions.

Nebraska is **not another registry container** here. The exact
`github.com/flatcar/nebraska/backend` version in candidate `tools/go.mod`
(18dd6cd: `v0.0.0-20260806113018-30a488d8d300`) is compiled into Storm.
Its existing proxy migrates/seeds a disposable Postgres DB, serves real Omaha
over ephemeral TLS, and validates Nebraska status history and cookie behavior.
Storm's update case creates the fake API/image services and their certificates
at runtime, installs the temporary trust in the guest and tears down its
listeners synchronously. Do not supply production kubeconfigs, DB credentials,
TLS keys or Nebraska endpoints.

## Qualified inputs, not produced by build success alone

Use the SHA256-pinned **v1 bundle contract** in `VM-E2E-Install.md`. The unchanged
C verifier checks exact inventory, candidate/source equality, producer IDs and
every file's bytes. Product source may be
`18dd6cd99d50a4268afd936acb1ee4d2b64881ea` while this harness is based on
`fa0b19360ca925990fd91f57aa75ab5b45248f27` with approved helper integration
`7642d77a5dd302c4f238391cf2422cefec42d907`. Pass the product SHA explicitly.

In addition to `bin/storm-trident` (`go-tools`), inventory these exact paths:

| Bundle file | Pipeline artifact |
|---|---|
| `artifacts/acl-agent/trident-vm-acl-agent-testimage.qcow2` | `image-acl-agent-base` |
| `artifacts/test-image/trident-vm-acl-agent-update-testimage.cosi` | `image-acl-agent-update` |

Add exactly these fields under a new `aclAgent` object in the trusted manifest:

- `configRevision`: full expected product SHA. Independent qualification must
  bind the candidate base/update YAML, systemd override and baked-in hostname,
  annotation keys, UKI/usr-verity layout and installed candidate packages to
  the two selected image hashes.
- `postgresImage`: explicitly reviewed fully qualified
  `registry/repository@sha256:<64 lowercase hex>` reference, already cached in
  the host daemon as `linux/amd64`. Tags/unqualified names are not accepted.
- `targetVersion`: exact numeric `VERSION_ID` of the selected update COSI,
  in `number.number.number` form, not a harness-guessed baseline.
- `hostEndpointIP`: existing private IPv4 address of the default libvirt
  network's host interface, guest-reachable (normally `192.168.122.1`).

The manifest pin must come from an independent trusted qualification channel.
These fields are reviewed assertions, not automatic package/MIC attestations.
The install-family preparer does **not** emit this ACL-agent extension.
Do not modify/re-pin arbitrary prepared bundles and call them qualified.
No new producer, resolver or pipeline effects are added by this lane.

Producer **1266405**, definition **5073 revision 7**, ECF project
`2311650c-e79e-4301-b4d2-96543fdd84ff`, source **18dd6cd**, succeeded with
only the seven RPM/tools/installer/split-installer/regular/verity/usr-verity
producer stages. It does **not** establish ACL-agent artifact availability.
The parent's installer observation (MIC pull digest
`sha256:3e7535e520cfed39561c1040f3ca2ee1ba96dd16bfd89aa643c74fde87c9e614`,
base version `3.0.20261116`) does not qualify the ACL-agent cohort.
Additional existing prerequisites are `PrepareSSHKeys` and
`BuildImagesAclAgent` (base and update jobs), plus the matching `BuildingTools`
and `GetTridentBinaries_rpms_amd64` inputs. Those image jobs use
`build-image.yml` and the existing Make targets
`artifacts/trident-vm-acl-agent-testimage.qcow2` and
`artifacts/trident-vm-acl-agent-update-testimage.cosi`.
Qualification must prove candidate checkout, current-run RPM consumption,
installed `trident`/`trident-acl` identities (and any installed service
subpackages), exact QCOW2/COSI hashes and OS versions, and resolved immutable
Azure Linux base VHDX and MIC identities.
Any cross-build lineage needs separate reviewed proof, not relabeling.

The parent owns actual image/package/base/MIC qualification and the existing
1ES download identity/network access. A usable existing SSH identity matching
the public key baked into both images is required, passed by external path.
This driver only stats that path, never reads, copies or inventories the key.
Do not download `ssh-keys` into the bundle or evidence. No credentials have been
qualified by the synthetic fixtures in the regression suite.

Pipeline Postgres defaults to the private mirror
`martimusexternal.azurecr.io/postgres:16-alpine`; local existing configuration
uses public `postgres:16-alpine`. **Neither tag is accepted here.** Independently
resolve/qualify the chosen public or private digest and arrange its presence
on the execution host through existing authorized mechanisms. Missing cache,
registry authorization, artifact, network, guest SSH identity or certificate
prerequisites must block or fail, not trigger another image or success.

## Host invocation and bounded ownership

Require existing KVM, UEFI/OVMF, libvirt default network, `sudo`, `virt-install`,
`virsh`, `qemu-img`, SSH, Docker daemon access and the shared
`/var/lib/trident-cloud-agent/functional.lock`. The runner checks both root
system/session domain views, verifies the configured network address and
availability of canonical ports 18080/18081/18082 before VM execution.
Both libvirt views must be accessible. No network/firewall changes are made.

```bash
sudo python3 scripts/cloud-agent/run-vm-e2e-acl-agent.py \
  --bundle "$QUALIFIED_ACL_BUNDLE" --manifest "$MANIFEST" \
  --manifest-sha256 "$INDEPENDENT_MANIFEST_SHA256" \
  --expected-revision "$PRODUCT_SHA" \
  --ssh-private-key-path "$EXISTING_EXTERNAL_KEY_PATH" \
  --timeout-seconds "$SUITE_TIMEOUT" \
  --cleanup-grace-seconds "$CLEANUP_BOUND" --output "$NEW_OUTPUT"
```

Use a new output directory outside the bundle, on the same filesystem.
Hard-linked suite-local image inputs preserve the bundle. The existing VM
deployer creates its writable `booted.qcow2` copy there.

The shared host lock covers preflight, the entire scenario and fallback cleanup.
A generated unique VM name and suite-local serial/disk paths replace the
legacy shared defaults. A suite-local Docker wrapper preserves the existing
proxy's loopback/ephemeral Postgres port mapping, adds a unique name/owner label,
`--pull=never` and bounded `pg_isready` Docker health checking. It only returns
the container ID to Storm after health succeeds. The existing DB ping,
migrations, TLS setup and end-to-end Omaha/status assertions remain unchanged.
Before each Docker creation request, the wrapper fsyncs its intended unique
container name in the suite-local `postgres-starts` journal. A separate receipt
is persisted only after `docker run` completes successfully with a container ID.
An interrupted or failed request without that receipt remains unresolved:
killing the CLI does not cancel a creation already accepted by the daemon.

On failure, timeout or cancellation, Storm gets its existing cleanup opportunity.
While still locked, the driver then masks repeated cancellation, removes only
containers matching its owner label (including interrupted starts), and stops/
undefines only its generated VM with the exact suite-owned boot disk, in either
libvirt view. There is no prune, global VM wipe or storage/network deletion.
Every operation is bounded. An unsuccessful cleanup is explicitly reported and
fails an otherwise successful run; it never replaces the first process failure
(including 124 timeout or 130 interruption). If cleanup cannot be verified,
`cleanupErrors` requires parent intervention/outer-host disposal, not a pass.
Unresolved starts trigger label-scoped reconciliation for at most one cleanup
grace interval, with each Docker operation limited by the remaining interval.
Late-visible owned containers are removed, but empty scans never establish that
an accepted creation was cancelled. Any unresolved start still reports explicit
uncertainty requiring outer-host disposal, even if a container was found and
removed or all scans stayed empty. The primary process exit code is retained.
The suite timeout plus sequential bounded cleanup operations is the total bound,
not a promise that cleanup finishes within one grace interval.

`results.json` distinguishes selected/observed-executed cases, pass/fail/skip,
explicit JUnit pending/not-run, and unavailable reports. Missing or partial
reports never invent passes or mark unknown execution as not-run.
The suite-level `invoked` flag records an attempted command, not case execution;
the `executed` count includes only cases observed passed/failed in JUnit.
All eight cases must pass for suite success. The original process code and first
failed case are retained. Output contains diagnostic console/JUnit/serial/disk
and test-generated service data. Treat it as sensitive, ephemeral runtime
material; do not blindly upload raw logs, guest data, generated test credentials,
Docker configuration or keys as qualification evidence.

ARM64, bare metal, Azure resources, VM/container installation matrices,
scale/direct streaming, QEMU servicing/UKI lifecycle and install/configuration
are separate lanes. None is claimed run by this entrypoint or its local tests.
