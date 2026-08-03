# trident-acl-agent-tester

Practical validation harness for the label protocol in `wiki/projects/acl-aks/trident-acl-agent-label-protocol.md` §13.

## Subcommands

- `apiserver` — fake single-node Kubernetes apiserver with `GET`/`PATCH` for `/api/v1/nodes/{name}`.
- `rp-proxy` — runs a YAML scenario that patches the fake Node and waits for expected protocol states.
- `kubelet-proxy` — seeds bootstrap labels once, then flips a Ready/NotReady-equivalent annotation when the reboot shim marker appears.
- `nebraska-proxy` — serves minimal Omaha update-check responses from a YAML scenario.
- `reboot-shim` — hidden helper that writes the reboot marker and exits 0 instead of rebooting the machine.

## Notes

- The real `trident-acl-agent` currently uses the poll-loop fallback rather than a streaming Kubernetes watch, so the fake apiserver intentionally focuses on plain `GET`/`PATCH`.
- The reboot marker defaults to `trident-acl-agent-tester-reboot-signal` in the current directory instead of `/tmp/...` because this runtime forbids temporary-directory writes.
- The reboot shim is meant to be placed earlier on `PATH` than the real `reboot`/`systemctl` binaries during test runs.

## Example end-to-end flow

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

cargo run -p trident-acl-agent-tester -- apiserver --listen 127.0.0.1:18080 &
cargo run -p trident-acl-agent-tester -- nebraska-proxy --listen 127.0.0.1:18081 --scenario nebraska.yaml &
cargo run -p trident-acl-agent-tester -- kubelet-proxy \
  --apiserver-url http://127.0.0.1:18080 \
  --marker-file ./trident-acl-agent-tester-reboot-signal &

PATH="$(pwd)/target/debug:$PATH" \
trident-acl-agent --config /etc/trident-acl-agent/config.toml

cargo run -p trident-acl-agent-tester -- rp-proxy \
  --apiserver-url http://127.0.0.1:18080 \
  --scenario rp.yaml
```

In a full smoke run, an outer script should kill and restart `trident-acl-agent` after the reboot shim fires so the post-reboot commit path is exercised cleanly.

## Follow-up

A real end-to-end smoke script that includes a fake `tridentd` is still a good follow-up once the surrounding test doubles settle down.
