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
- `collect-logs`
- `cleanup-vm`

## Expected image contents

The VM image used by this scenario must already contain:

- `tridentd.service` installed and enabled
- `trident-acl-agent.service` installed and enabled
- `/etc/trident/trident-acl-agent.conf` pre-seeded to point at `localhost:<port>` endpoints that storm reverse-SSH-forwards from the test runner into the VM
- the same SSH user/key setup expected by the existing storm servicing scenario

## Local usage

```bash
make bin/storm-trident
./bin/storm-trident aclagent deploy-vm --artifacts-dir <artifacts>
./bin/storm-trident aclagent check-deployment --artifacts-dir <artifacts>
./bin/storm-trident aclagent run-ab-update --artifacts-dir <artifacts>
./bin/storm-trident aclagent collect-logs --artifacts-dir <artifacts>
./bin/storm-trident aclagent cleanup-vm --artifacts-dir <artifacts>
```

Common overrides mirror other storm VM scenarios, for example:

```bash
./bin/storm-trident aclagent run-ab-update \
  --platform qemu \
  --artifacts-dir ./artifacts \
  --output-path ./output/aclagent \
  --api-server-port 18080 \
  --nebraska-port 18081
```

## Reboot choice

This scenario keeps the shim-based reboot interception from the old tester.
That is less realistic than a full VM reboot, but it keeps the test deterministic
and lets the storm runner hold the reverse SSH tunnels and in-process fake services
steady while the agent drives the finalize path.
