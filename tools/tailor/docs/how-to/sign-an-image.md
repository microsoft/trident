# Sign an image

tailor can produce **Secure Boot–signed** images by orchestrating Image Customizer's
`output.artifacts` → host-side signing → `inject-files` flow. You declare *how* to sign with a
`signing:` profile; you keep authoring *what* to extract (`output.artifacts`) in your own IC `config:`.

> **Preview feature.** Signing is not yet part of the stable 1.0 contract, so it is gated behind a
> preview opt-in: add `previewFeatures: [signing]` to `tailor.yaml`. A signed `tailor build` or
> `validate` without the opt-in stops with a clear error. Because it is a preview feature, its schema
> and behavior may change before it is promoted (see [Compatibility](https://github.com/frhuelsz/tailor/blob/main/COMPATIBILITY.md)).
>
> **Backend status.** The `local-test-ca` and `keypair` backends are implemented end-to-end: a signed
> build extracts the declared artifacts, signs them on the host, and re-injects them so the final image
> is signed. The `azure-key-vault` backend is **not yet implemented** — its configuration validates and
> preflight reports it as unavailable.

## 1. Enable the signing preview feature

Signing is gated behind a preview opt-in. Add it to `tailor.yaml`:

```yaml
# tailor.yaml
previewFeatures:
  - signing
```

## 2. Declare signing profiles

Add a `signing:` block to `tailor.yaml`. A profile names a key-source `backend` plus its settings:

```yaml
# tailor.yaml
signing:
  default: test-ca            # profile used when an image says `signing: true`
  profiles:
    test-ca:                  # self-signed CA minted per build (CI / local; not a production root)
      backend: local-test-ca
      publishCaCert: ./artifacts/ca_cert.pem
    byo:                      # bring your own Secure Boot key + cert
      backend: keypair
      key: ./secrets/db.key   # PEM private key (referenced, never imaged)
      cert: ./secrets/db.crt  # PEM certificate
    akv:                      # remote signing service (future)
      backend: azure-key-vault
      vault: https://my-vault.vault.azure.net
      certificate: secureboot-db
```

| Backend | Required fields | Use |
| --- | --- | --- |
| `local-test-ca` | none | MVP / CI. A self-signed CA + leaf minted per build. Not a production trust root. |
| `keypair` | `key`, `cert` | Bring your own Secure Boot key + certificate (PEM). |
| `azure-key-vault` | `vault`, `certificate` | Remote signing — configuration only; execution is a later milestone. |

## 3. Opt an image in

Set `signing:` on the image — `true` for the workspace default profile, or a profile id. The image
still authors its own `output.artifacts` (that is what tells IC which boot artifacts to extract):

```yaml
# image.yaml
name: appliance
signing: true            # or `signing: byo`
config:
  output:
    artifacts:
      items: [ukis, shim, systemd-boot, verity-hash]
      path: ./output-artifacts
```

Omit `signing:` (or set `signing: false`) for an unsigned image — unlike `toolchain:`, the workspace
default is **not** auto-applied, so signing is always an explicit choice.

## 4. Check readiness (fail fast)

Before any build, tailor verifies every signing prerequisite — once, up front — so a signed build
never customizes N cells only to discover a key is missing. Report readiness without building:

```bash
tailor validate appliance
# ✓ appliance                    2 cell(s) valid
# ✓ signing profile `byo` ready (image(s): appliance)
```

If a prerequisite is missing, `validate` warns; a real `build` aborts before touching IC, naming
every unmet prerequisite and the images that need it:

```text
$ tailor build appliance
error: signing preflight failed — fix every prerequisite below, then rebuild:
  - profile `byo` (needed by: appliance): cannot read `key` `./secrets/db.key`: No such file or directory
```

What the preflight checks per backend:

- **`local-test-ca`** — always ready (the CA + leaf are minted at sign time).
- **`keypair`** — the `key` and `cert` files exist, are readable, and are PEM.
- **`azure-key-vault`** — configuration completeness; preflight also reports that execution is not yet
  available for this backend.

## 5. Dry-run

`tailor build --dry-run` never contacts an engine and reports the signing plan and its readiness
without signing anything:

```bash
tailor build --dry-run appliance
# … the customize invocation …
# ✓ signing profile `byo` ready (image(s): appliance)
```

A real `tailor build appliance` (with an implemented backend) then runs the full flow: extract the
declared `output.artifacts`, sign them on the host, and re-inject them so the final image is signed.

## Notes

- Private key material is always **referenced** by path, never inlined into the manifest or written
  into an image.
- `local-test-ca` mints fresh keys each build, so its signed outputs are intentionally not
  reproducible; use `keypair` (a fixed cert identity) for reproducible production builds.
