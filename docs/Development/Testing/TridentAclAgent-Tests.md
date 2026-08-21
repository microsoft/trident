---
sidebar_position: 9
---

# Trident ACL Agent Tests

`storm-trident run aclagent` is the single supported validation entrypoint for
the annotation-driven `trident-acl-agent` protocol. Unlike [Servicing
Tests](Servicing-Tests.md), which drive Trident's own `stage`/`finalize` gRPC
calls directly, this scenario validates `trident-acl-agent` itself: it
deploys a VM, starts fake in-process test doubles for the Kubernetes API
server and the Nebraska/Omaha update server, patches update-request
annotations onto the fake Node, and lets the real `trident-acl-agent` binary
running inside the VM drive a full A/B update (and, separately, a rollback)
against those fakes.

There is intentionally no fake `tridentd` — the scenario talks to the real
`tridentd` and real `trident-acl-agent` running inside the VM.

## What It Validates

- `trident-acl-agent` watching its own Kubernetes Node object for
  RP-authored `acl.azure.com/update-request` annotation changes (via
  `kube::runtime::watcher()`)
- Reading the target OS image version, Nebraska server/appId/track from the
  request annotation and triggering a real Trident `stage` + `finalize` A/B
  update (or `rollback` + rollback `finalize`) through the normal gRPC path
- Writing back progress/result via the `acl.azure.com/update-status` and
  `acl.azure.com/update-commit-status` annotations as the update progresses
- Resuming correctly after a real reboot (see [Reboot Choice](#reboot-choice))

## VM Image Contents

The VM image used by this scenario must already contain:

- `tridentd.socket` installed and enabled (starts `tridentd.service` on
  demand)
- `trident-acl-agent` package installed, but **`trident-acl-agent.service`
  left disabled** in the base image — it must not start before the fake
  kubeconfig exists (the harness delivers it at runtime and enables/starts
  the service itself; the update image, by contrast, bakes enablement in
  directly, since there is no test harness left to run `systemctl enable
  --now` after a real A/B update boots into it)
- the same SSH user/key setup expected by the [servicing](Servicing-Tests.md)
  scenario

The harness never writes `/etc/trident/trident-acl-agent.conf` at all —
`nebraska.endpoint`/`nebraska.app_id`/`nebraska.track` are supplied per-request
via the update-request annotation's `server`/`appId`/`track` fields instead,
and `kubernetes.node_name` defaults to the node's own real hostname (which
the image's hostname is set to match). What the harness does deliver, once
at runtime after `deploy-vm`, is a fake kubeconfig at
`/var/lib/kubelet/kubeconfig` pointing at the fake apiserver. That path lives
on its own dedicated ext4 partition (not part of the A/B-swapped `/usr`/root
volume pair in this usr-verity image layout), so it persists across
`run-ab-update`'s finalize reboot without needing to be re-delivered.
`trident-acl-agent.service`'s enablement state, by contrast, lives on the
swapped root itself and does not carry over a reboot onto the other A/B
volume — the harness re-runs its short prepare/restart step after each
reboot to reconnect the agent to that test case's fresh fake-apiserver
instance, not to re-create the kubeconfig.

## Prerequisites

- **Linux host** with root access
- **libvirt and QEMU** installed and configured
- **Docker** (for building images with Image Customizer)
- **Go 1.24+** (for building Go tools)
- **Rust** (latest stable, for building Trident and `trident-acl-agent`)

See [Dependencies](../Building/Dependencies.md) for full build dependency
details.

## Building Dependencies

### 1. Build Trident, `trident-acl-agent`, and RPMs

Always build through `make`, not a raw `cargo build`, when the RPM tarball
needs to reflect a source change — `make` injects the dev version string
(`TRIDENT_VERSION`) that the RPM spec's `%check` step verifies against. A
plain `cargo build` skips that and produces an RPM build failure.

```bash
make target/release/trident target/release/trident-acl-agent
make bin/trident-rpms.tar.gz
```

### 2. Build Go Tools

```bash
make bin/storm-trident
```

### 3. Generate SSH Keys

```bash
make artifacts/id_rsa
```

:::note
The VM images below bake in the public key from `artifacts/id_rsa.pub` (via
the `files/id_rsa.pub` Makefile rule), **not** `~/.ssh/id_rsa.pub`. Always
pass `--ssh-private-key-path artifacts/id_rsa` when running the scenario
locally — using your personal `~/.ssh/id_rsa` doesn't fail fast, it just
hangs/retries during `check-deployment`'s SSH auth.
:::

### 4. Download the qemu_guest Base Image

Same base image as the servicing tests — see [Servicing Tests, step
4](Servicing-Tests.md#4-download-the-qemu_guest-base-image) for details.

### 5. Build the Base and Update VM Images

The scenario needs two images built from the current source:

```bash
# Base image: trident-acl-agent installed but disabled
make artifacts/trident-vm-acl-agent-testimage.qcow2

# Update image: what the agent updates the VM to
make artifacts/trident-vm-acl-agent-update-testimage.cosi
```

:::caution Rebuild after any `trident-acl-agent` change
Both image targets embed the RPM built in step 1. If you only rebuild the
Rust binary and re-run the scenario without rebuilding these images, you are
still testing the **old** binary baked into the existing qcow2/cosi files —
the failure (or fix) you're trying to observe silently won't reproduce. Clear
stale artifacts first if you're not sure they're current:

```bash
rm -f artifacts/trident-vm-acl-agent-testimage.qcow2 \
      artifacts/trident-vm-acl-agent-update-testimage.cosi
```
:::

## Running the ACL Agent Scenario

The scenario requires root access for VM creation via `virt-install`:

```bash
sudo bin/storm-trident run aclagent \
    --artifacts-dir ./artifacts \
    --output-path /tmp/aclagent-output \
    --ssh-private-key-path ./artifacts/id_rsa \
    --verbose
```

### Test Cases

The scenario runs these test cases in order:

1. **deploy-vm** — Copies the base qcow2 image and creates a QEMU VM
2. **check-deployment** — Verifies the VM booted and is accessible via SSH
3. **run-ab-update** — Starts the fake apiserver and fake Nebraska/Omaha
   endpoints in-process, delivers a fake kubeconfig and restarts
   `trident-acl-agent.service`, patches the `acl.azure.com/update-request`
   annotation, and waits for `trident-acl-agent` to drive a real Trident A/B
   update to completion (including a real reboot)
4. **run-rollback** — Exercises the `rollback` annotation end-to-end against
   tridentd's `RollbackService` gRPC API, followed by a real reboot and
   post-reboot commit; must run after `run-ab-update` in the same VM
   lifetime, since it rolls back to the volume active before that update
5. **collect-logs** — Fetches `trident-acl-agent` and Trident logs from the
   VM via SSH; also runs automatically (with a `journalctl` dump for
   `trident-acl-agent.service`) if an update/rollback step times out waiting
   for the service to become active, to make crash-loops self-diagnosing
6. **cleanup-vm** — Destroys the QEMU VM

### Flags

| Flag | Description | Default |
|------|-------------|---------|
| `--artifacts-dir` | Directory containing VM images | `.` |
| `--output-path` | Output directory for logs | `./output` |
| `--platform` | `qemu` or `azure` | `qemu` |
| `--ssh-private-key-path` | Path to SSH private key | `~/.ssh/id_rsa` |
| `--api-server-port` | Port for the fake Kubernetes API server | `18080` |
| `--nebraska-port` | Port for the fake Nebraska/Omaha server | `18081` |
| `--host-endpoint-ip` | Host IP the VM reaches the fake endpoints at | `192.168.122.1` |
| `--image-path` | Real `.cosi` update image to serve during staging | first `*.cosi` found under `--artifacts-dir` |
| `--verbose` | Enable verbose logging | `false` |
| `--test-case-to-run` | Run a specific test case only | `all` |

## Reboot Choice

This scenario uses a real VM reboot rather than a shim: `trident-acl-agent`
issues a genuine `systemctl reboot` on finalize, and the scenario polls SSH
until it goes unreachable (confirming the reboot actually happened) and then
reachable again (confirming the VM came back up), exercising the agent's
real post-reboot resume logic end to end. This is slower than a shim-based
approach, but it validates the real reboot path instead of a simulation of
it.

## Debugging Failures

If `trident-acl-agent.service` gets stuck reporting `activating` and the test
times out, that almost always means a **crash-restart loop**, not a slow
start — the unit has no explicit `Type=`, so `Type=simple` (the implicit
default) is used, and systemd marks such units active immediately on
`fork`/`exec` with no readiness signal. A persistent `activating` state for
the full wait window can only mean `Restart=on-failure` (`RestartSec=5`) is
cycling the service.

`run-ab-update`'s wait-for-active check captures `journalctl -u
trident-acl-agent.service --no-pager -n 200` on timeout and includes it in
the test failure, so the actual crash reason (e.g. a panic, a fatal error
from the Kubernetes client, or a config problem) should be visible directly
in the CI log or local output without a separate log-collection step.

The fake Kubernetes API server (`tools/storm/aclagent/proxies/apiserver.go`)
is a minimal, hand-rolled HTTP handler — it does not implement the full
Kubernetes API surface. If `trident-acl-agent` is changed to make a new kind
of API call (a different verb, a new field selector, list pagination,
etc.), the fake apiserver's routing may need a corresponding update or the
call will simply 404 and (since `trident-acl-agent` treats such client
errors as fatal) crash-loop the service. For example, migrating node-watching
from polling to `kube::runtime::watcher()` introduced an initial **LIST**
call to the collection endpoint (`GET /api/v1/nodes?fieldSelector=...`) that
the fake server didn't originally route, only the singular
`/api/v1/nodes/<name>` path — surfacing as exactly this crash-loop symptom
until the collection route was added.
