# mini-TAA — Trident ACL Agent for the Nebraska A/B demo

A small polling agent (a patched `trident-acl-agent` / "Harpoon") that:

1. Reads the current OS version from `/etc/os-release`.
2. Polls Nebraska every second (Omaha `updatecheck`).
3. When Nebraska offers a newer version, drives a Trident A/B update over the
   gRPC socket (`/run/trident/trident.sock`) — stage + finalize.
4. Lets Trident own the reboot. After reboot, systemd restarts the agent, it
   reads the new version, and Nebraska returns `noupdate` → quiet again.

## Build (glibc-compatible with the ACL guest)

The host (Ubuntu, glibc 2.43) is newer than the ACL guest (glibc ~2.35), so the
binary is built inside an Azure Linux 3 container. From the repo root:

```bash
DISTRO=azl3 make target/azl3/release/trident-acl-agent
# → target/azl3/release/trident-acl-agent
```

(That target uses the `azl3/trident-builder` image and the private cargo
registry; a valid ADO token / `cargo:token` credential provider is required.)

## Deploy into the VM

```bash
demo/mini-taa/deploy.sh target/azl3/release/trident-acl-agent \
    azureuser@192.168.122.77 ~/.ssh/id_rsa
```

This installs the binary at `/var/lib/trident-acl-agent/` and the unit at
`/etc/systemd/system/trident-acl-agent.service`, then enables + starts it.
Both paths are on the shared, persistent ext4 root (only `/usr` + its verity
hash are A/B-swapped), so they survive the update and reboot.

Watch it live:

```bash
ssh -i ~/.ssh/id_rsa azureuser@192.168.122.77 'journalctl -u trident-acl-agent -f'
```

## Invocation / flags

| Flag | Default | Notes |
|------|---------|-------|
| `<url>` (positional) | `https://nebraska-poc-ep-cda8e2czfnhahxfk.b01.azurefd.net/v1/update/` | Trailing slash matters for package-URL composition |
| `--appid` | `6d10cf97-443f-4542-8479-b9fdb44c9588` | Must match the Nebraska app |
| `--track` | `stable` | Must match the Nebraska group exactly |
| `--interval` | `1s` | humantime (`1s`, `500ms`, …) |
| `--events` | `none` | `none` = safe update-check-only (Nebraska self-heals to Complete). `full` = report the event sequence (not yet implemented in this build) |
| `--id-source` | `machine-id-hashed` | or `machine-id-raw`, `hostname` |
| `--machine-id` | (derived) | Override the instance id; in-the-room recovery for a wedged Nebraska instance |
| `--current-version` | (from os-release) | Override the reported version |
| `--version-field` | (auto) | Which os-release field to read (`VERSION_ID`, `VERSION`, …) |
| `--sha384` | `ignored` | COSI metadata sha384, or `ignored` to skip (Nebraska cannot supply this) |
| `--once` | off | Single poll then exit (testing) |

All flags also read from `HARPOON_*` env vars.

## Status of the patches

- **#1 poll loop** — done (`--interval`, default 1s; logs each poll).
- **#2 real current version** — done (reads `/etc/os-release`; ACL's
  `VERSION_ID=3.0.20260731` parses as semver; fails loud if unparseable).
- **#3 configurable appid/track** — done (+ `--url`, `--id-source`,
  `--machine-id`), demo defaults baked in.
- **#4 reboot + hash** — reboot handed to Trident (`TRIDENT_HANDLES_REBOOT`);
  hash passes through, defaulting to `ignored` (Nebraska only knows the COSI
  *file* sha1, not Trident's metadata sha384 — so `ignored` is the right call).
- **`--events none` kill switch** — default; guaranteed-safe path.
- **G4 tolerance** — `error-updateInProgressOnInstance` is handled as a quiet
  "update in progress" state, not a fatal error.

Not yet in this build: the full Omaha event sequence (`--events full`).
