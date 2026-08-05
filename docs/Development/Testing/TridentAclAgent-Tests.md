---
sidebar_position: 9
---

# Trident ACL Agent Tests

`storm-trident run aclagent` is the single supported validation entrypoint for
the label-driven `trident-acl-agent` protocol described in the ACL AKS
node-label design. Unlike [Servicing Tests](Servicing-Tests.md), which drive
Trident's own `stage`/`finalize` gRPC calls directly, this scenario validates
`trident-acl-agent` itself: it deploys a VM, starts fake in-process test
doubles for the Kubernetes API server and the Nebraska/Omaha update server,
seeds bootstrap node labels, and lets the real `trident-acl-agent` binary
running inside the VM drive a full A/B update against those fakes.

There is intentionally no fake `tridentd` — the scenario talks to the real
`tridentd` and real `trident-acl-agent` running inside the VM.

## What It Validates

- `trident-acl-agent` watching its own Kubernetes Node object for
  RP-authored label changes (via `kube::runtime::watcher()`)
- Reading the update image URL/hash from labels and triggering a real
  Trident `stage` + `finalize` A/B update through the normal gRPC path
- Patching back observed-state labels/annotations as the update progresses
- Resuming correctly after a simulated reboot (shim-intercepted, not a real
  VM reboot — see [Reboot Choice](#reboot-choice))

## VM Image Contents

The VM image used by this scenario must already contain:

- `tridentd.socket` installed and enabled (starts `tridentd.service` on
  demand)
- `trident-acl-agent` package installed, but **`trident-acl-agent.service`
  left disabled** — it must not start before a config file exists
- the same SSH user/key setup expected by the [servicing](Servicing-Tests.md)
  scenario

Both the enabled/disabled state of `trident-acl-agent.service` and
`/etc/trident/trident-acl-agent.conf` live under `/etc`, which is not part of
the A/B-swapped `/usr`/root volume pair in this usr-verity image layout. That
makes it safe for the scenario to write the config and enable the service
once, after `deploy-vm`, rather than baking enablement into the image — the
state persists across `run-ab-update`'s finalize the same way the config file
does.

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
2. **check-deployment** — Verifies the VM booted and is accessible via SSH;
   writes `/etc/trident/trident-acl-agent.conf` pointing at the
   `localhost:<port>` endpoints storm reverse-SSH-forwards into the VM, then
   runs `systemctl enable --now trident-acl-agent.service`
3. **run-ab-update** — Starts the fake apiserver and fake Nebraska/Omaha
   endpoints in-process, seeds bootstrap node labels, patches the desired
   update-image label, and waits for `trident-acl-agent` to drive a real
   Trident A/B update to completion (including the shim-based simulated
   reboot)
4. **collect-logs** — Fetches `trident-acl-agent` and Trident logs from the
   VM via SSH; also runs automatically (with a `journalctl` dump for
   `trident-acl-agent.service`) if `run-ab-update` times out waiting for the
   service to become active, to make crash-loops self-diagnosing
5. **cleanup-vm** — Destroys the QEMU VM

### Flags

| Flag | Description | Default |
|------|-------------|---------|
| `--artifacts-dir` | Directory containing VM images | `/tmp` |
| `--output-path` | Output directory for logs | `./output` |
| `--platform` | `qemu` or `azure` | `qemu` |
| `--ssh-private-key-path` | Path to SSH private key | `~/.ssh/id_rsa` |
| `--api-server-port` | Port for the fake Kubernetes API server | `18080` |
| `--nebraska-port` | Port for the fake Nebraska/Omaha server | `18081` |
| `--verbose` | Enable verbose logging | `false` |
| `--test-case-to-run` | Run a specific test case only | `all` |

## Reboot Choice

This scenario uses shim-based reboot interception rather than a full VM
reboot: a `reboot`/`systemctl reboot` shim on `PATH` inside the VM signals the
scenario's controller and exits the agent process instead of actually
rebooting. The scenario then restarts `trident-acl-agent` fresh, exercising
its post-reboot resume logic without tearing down the SSH session or the
in-process fake services. This is less realistic than a full reboot, but it
keeps the test deterministic and fast.

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
