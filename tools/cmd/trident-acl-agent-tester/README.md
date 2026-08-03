# trident-acl-agent-tester

Practical validation harness for the design in `wiki/projects/acl-aks/trident-acl-agent-label-protocol.md` §13.

## Subcommands

- `apiserver` — fake single-node Kubernetes apiserver using real `corev1.Node` JSON. Supports:
  - `GET /api/v1/nodes/{name}`
  - `PATCH /api/v1/nodes/{name}` with merge-patch JSON against `metadata.labels`, `metadata.annotations`, and `status.conditions`
  - `GET /api/v1/nodes/{name}?watch=true` as newline-delimited watch events
- `rp-proxy` — plays aks-rp. Reads a YAML scenario, patches the fake Node, waits for expected state transitions, and emits human-readable or JSON reports.
- `kubelet-proxy` — plays kubelet. Seeds bootstrap labels once, then watches a reboot marker file and flips the fake Node `Ready` condition around the simulated reboot while also writing a helper annotation.
- `nebraska-proxy` — serves a minimal Omaha/Nebraska update-check endpoint driven by YAML.
- `reboot-shim.sh` — standalone bash helper, not part of the Go binary. Put it earlier on `PATH` than the real `reboot`/`systemctl` commands during a test run so a finalize step writes the reboot marker instead of rebooting the machine.

## Reboot shim

The shim lives next to this README as `reboot-shim.sh`.

Example setup:

```bash
mkdir -p ./shim-bin
ln -sf "$(pwd)/tools/cmd/trident-acl-agent-tester/reboot-shim.sh" ./shim-bin/reboot
ln -sf "$(pwd)/tools/cmd/trident-acl-agent-tester/reboot-shim.sh" ./shim-bin/systemctl
export PATH="$(pwd)/shim-bin:$PATH"
export TRIDENT_ACL_AGENT_TESTER_REBOOT_MARKER="$(pwd)/trident-acl-agent-tester-reboot-signal"
```

If invoked as `systemctl` for anything other than `systemctl reboot`, the shim forwards to the real `/usr/bin/systemctl`.

## Scenario YAML

`rp-proxy` accepts YAML like:

```yaml
steps:
  - patch:
      request: stage
      request-id: R1
      target-os-image-version: 202507.28.0
  - expect:
      state: staged
      observed-request-id: R1
      timeout: 60s
  - patch:
      request: finalize
      request-id: R1
  - expect:
      state: update-succeeded
      observed-request-id: R1
      timeout: 120s
```

`nebraska-proxy` accepts YAML like:

```yaml
available: true
version: 202507.28.0
url: https://example.invalid/images/
sha384: 111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111
```

## Example end-to-end shell flow

```bash
cat > nebraska.yaml <<'EOF'
available: true
version: 202507.28.0
url: https://example.invalid/images/
sha384: 111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111
EOF

cat > rp.yaml <<'EOF'
steps:
  - patch:
      request: stage
      request-id: R1
      target-os-image-version: 202507.28.0
  - expect:
      state: staged
      observed-request-id: R1
      timeout: 60s
  - patch:
      request: finalize
      request-id: R1
  - expect:
      state: update-succeeded
      observed-request-id: R1
      timeout: 120s
EOF

make bin/trident-acl-agent-tester
./bin/trident-acl-agent-tester apiserver --listen 127.0.0.1:18080 &
./bin/trident-acl-agent-tester nebraska-proxy --listen 127.0.0.1:18081 --scenario nebraska.yaml &
./bin/trident-acl-agent-tester kubelet-proxy \
  --apiserver-url http://127.0.0.1:18080 \
  --marker-file ./trident-acl-agent-tester-reboot-signal &

# Put reboot/systemctl shim first on PATH before starting trident-acl-agent.
mkdir -p ./shim-bin
ln -sf "$(pwd)/tools/cmd/trident-acl-agent-tester/reboot-shim.sh" ./shim-bin/reboot
ln -sf "$(pwd)/tools/cmd/trident-acl-agent-tester/reboot-shim.sh" ./shim-bin/systemctl
export PATH="$(pwd)/shim-bin:$PATH"
export TRIDENT_ACL_AGENT_TESTER_REBOOT_MARKER="$(pwd)/trident-acl-agent-tester-reboot-signal"

# Start the real Rust agent in label mode against the test doubles.
trident-acl-agent --config /etc/trident/trident-acl-agent.conf &

# Drive the protocol from the fake RP.
./bin/trident-acl-agent-tester rp-proxy \
  --apiserver-url http://127.0.0.1:18080 \
  --scenario rp.yaml

# After the reboot shim fires, kill and restart trident-acl-agent once to
# simulate the reboot boundary and exercise the post-boot commit path.
```

## Notes

- This tool is intentionally portable and CI-friendly: it uses only hand-rolled HTTP stubs, not `envtest`.
- The Rust agent may still use a poll loop instead of a Kubernetes watch; the fake apiserver supports both GET/PATCH and a best-effort watch stream.
- A fuller fake-`tridentd`-backed smoke script is still a reasonable follow-up if deterministic end-to-end CI coverage becomes necessary.
