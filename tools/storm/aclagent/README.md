# Trident ACL agent storm scenario

`storm-trident aclagent` is the single supported validation entrypoint for the
label-driven trident ACL agent protocol.

## What it does

- deploys a QEMU or Azure VM using the existing storm VM helpers
- starts the fake single-node Kubernetes apiserver in-process inside the storm binary
- starts the fake Nebraska/Omaha endpoint in-process inside the storm binary
- seeds bootstrap node labels and simulated Ready flips with an in-process kubelet helper
- talks to the real `tridentd` and real `trident-acl-agent` running inside the VM
- intercepts `reboot` / `systemctl reboot` with a shim so the finalize path can be exercised without tearing down the SSH session

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
- `trident-acl-agent` package installed, but **`trident-acl-agent.service`
  left disabled** -- it must not start before a config exists
- the same SSH user/key setup expected by the existing storm servicing scenario

Both the enabled/disabled state of `trident-acl-agent.service`
(`/etc/systemd/system/multi-user.target.wants/...`) and
`/etc/trident/trident-acl-agent.conf` live under `/etc`, which is not part of
the A/B-swapped `/usr`/root volume pair in this usr-verity layout. That makes
it safe for the scenario to write the config and enable the service once,
after `deploy-vm`, rather than baking enablement into the image: the state
persists across `run-ab-update`'s finalize the same way the config file does.

`prepareVmForAclAgent` writes `/etc/trident/trident-acl-agent.conf` pointing
at the `localhost:<port>` endpoints storm reverse-SSH-forwards into the VM,
then runs `systemctl enable --now trident-acl-agent.service`. Before that
runs, the service simply isn't started -- no crash-looping, no log noise.

## Local usage

The VM image (`make artifacts/trident-vm-acl-agent-testimage.qcow2`) bakes in
the public key from `artifacts/id_rsa.pub`, not `~/.ssh/id_rsa.pub` -- pass
`--ssh-private-key-path` pointing at `artifacts/id_rsa` (the matching private
key) or `check-deployment` will hang/fail trying to authenticate with the
wrong key.

```bash
make bin/storm-trident
./bin/storm-trident run aclagent deploy-vm \
  --artifacts-dir <artifacts> --ssh-private-key-path <artifacts>/id_rsa
./bin/storm-trident run aclagent check-deployment \
  --artifacts-dir <artifacts> --ssh-private-key-path <artifacts>/id_rsa
./bin/storm-trident run aclagent run-ab-update \
  --artifacts-dir <artifacts> --ssh-private-key-path <artifacts>/id_rsa
./bin/storm-trident run aclagent run-rollback \
  --artifacts-dir <artifacts> --ssh-private-key-path <artifacts>/id_rsa
./bin/storm-trident run aclagent collect-logs \
  --artifacts-dir <artifacts> --ssh-private-key-path <artifacts>/id_rsa
./bin/storm-trident run aclagent cleanup-vm \
  --artifacts-dir <artifacts> --ssh-private-key-path <artifacts>/id_rsa
```

Common overrides mirror other storm VM scenarios, for example:

```bash
./bin/storm-trident run aclagent run-ab-update \
  --platform qemu \
  --artifacts-dir ./artifacts \
  --output-path ./output/aclagent \
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
