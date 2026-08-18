# Trident AKS agent storm scenario

`storm-trident aksagent` is the single supported validation entrypoint for the
label-driven trident AKS agent protocol.

## What it does

- deploys a QEMU or Azure VM using the existing storm VM helpers
- starts the fake single-node Kubernetes apiserver in-process inside the storm binary
- starts the fake Nebraska/Omaha endpoint in-process inside the storm binary
- seeds bootstrap node annotations and simulated Ready flips with an in-process kubelet helper
- talks to the real `tridentd` and real `trident-aks-agent` running inside the VM
- lets `trident-aks-agent` issue a real `systemctl reboot` on finalize, then polls SSH until it drops and comes back up to confirm the reboot actually happened

There is intentionally no fake `tridentd`.

## Test cases

- `deploy-vm`
- `check-deployment`
- `run-ab-update`
- `run-rollback`
- `collect-logs`
- `cleanup-vm`

## Expected image contents

The VM image used by this scenario must already contain:

- `tridentd.socket` installed and enabled (starts `tridentd.service` on demand)
- `trident-aks-agent` package installed, but **`trident-aks-agent.service`
  left disabled** -- it must not start before the fake kubeconfig exists
- the same SSH user/key setup expected by the existing storm servicing scenario

Both the enabled/disabled state of `trident-aks-agent.service`
(`/etc/systemd/system/multi-user.target.wants/...`) and the fake kubeconfig
at `/var/lib/kubelet/kubeconfig` live under paths that are not part of the
A/B-swapped `/usr`/root volume pair in this usr-verity layout. That makes it
safe for the scenario to deliver the kubeconfig and enable the service once,
after `deploy-vm`, rather than baking enablement into the image: the state
persists across `run-ab-update`'s finalize the same way the kubeconfig does.

`prepareVmForAksAgent` never writes a `trident-aks-agent.conf` at all - the
agent's compiled-in defaults already cover everything it needs (see the
function's own doc comment): `nebraska.app_id`/`nebraska.endpoint` are
supplied per-request via the update-request annotation's `appId`/`server`
fields instead (see `RunABUpdate`'s `PatchStep.AppId`/`PatchStep.Server`),
`kubernetes.node_name` defaults to the node's real hostname (which the VM
image's Image Customizer config sets to match `TestConfig.NodeName`), and
`kubernetes.api_server` is left unset so the fake kubeconfig's own `server:`
field is used as-is. `prepareVmForAksAgent` writes only that fake
kubeconfig, then runs `systemctl enable --now trident-aks-agent.service`.
Before that runs, the service simply isn't started -- no crash-looping, no
log noise.

## Local usage

The VM image (`make artifacts/trident-vm-aks-agent-testimage.qcow2`) bakes in
the public key from `artifacts/id_rsa.pub`, not `~/.ssh/id_rsa.pub` -- pass
`--ssh-private-key-path` pointing at `artifacts/id_rsa` (the matching private
key) or `check-deployment` will hang/fail trying to authenticate with the
wrong key.

```bash
make bin/storm-trident
./bin/storm-trident run aksagent --test-case-to-run deploy-vm \
  --artifacts-dir <artifacts> --ssh-private-key-path <artifacts>/id_rsa
./bin/storm-trident run aksagent --test-case-to-run check-deployment \
  --artifacts-dir <artifacts> --ssh-private-key-path <artifacts>/id_rsa
./bin/storm-trident run aksagent --test-case-to-run run-ab-update \
  --artifacts-dir <artifacts> --ssh-private-key-path <artifacts>/id_rsa
./bin/storm-trident run aksagent --test-case-to-run run-rollback \
  --artifacts-dir <artifacts> --ssh-private-key-path <artifacts>/id_rsa
./bin/storm-trident run aksagent --test-case-to-run collect-logs \
  --artifacts-dir <artifacts> --ssh-private-key-path <artifacts>/id_rsa
./bin/storm-trident run aksagent --test-case-to-run cleanup-vm \
  --artifacts-dir <artifacts> --ssh-private-key-path <artifacts>/id_rsa
```

Common overrides mirror other storm VM scenarios, for example:

```bash
./bin/storm-trident run aksagent --test-case-to-run run-ab-update \
  --platform qemu \
  --artifacts-dir ./artifacts \
  --output-path ./output/aksagent \
  --ssh-private-key-path ./artifacts/id_rsa \
  --api-server-port 18080 \
  --nebraska-port 18081
```

## Reboot choice

This scenario keeps the shim-based reboot interception from the old tester.
That is less realistic than a full VM reboot, but it keeps the test deterministic
and lets the storm runner hold the reverse SSH tunnels and in-process fake services
steady while the agent drives the finalize path.

## `run-rollback`

`run-rollback` exercises the `rollback` annotation end-to-end against
tridentd's stable `RollbackService` gRPC API (`RollbackStage`/
`RollbackFinalize`), followed by a real reboot and post-reboot commit -
mirroring `run-ab-update`'s stage/finalize/commit flow. It must run *after*
`run-ab-update` in the same VM lifetime, since rollback re-activates the
volume that was active before `run-ab-update`'s finalize and there is
nothing to roll back to on a freshly-deployed VM.
