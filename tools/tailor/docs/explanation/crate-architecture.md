# Architecture

tailor is split into small Rust crates with an inward dependency direction.

```mermaid
graph TD
  CLI["tailor<br/>binary CLI"] --> CORE["tailor-core"]
  CLI --> CONFIG["tailor-config"]
  CLI --> EXEC["tailor-exec"]
  CLI --> RESOLVE["tailor-resolve"]
  CLI --> SIGN["tailor-sign"]
  CORE --> CONFIG
  EXEC -. implements ports .-> CORE
  RESOLVE -. implements ports .-> CORE
  SIGN -. implements ports .-> CORE
```

| Crate | Responsibility |
| --- | --- |
| `tailor-config` | Parse `tailor.yaml`/`image.yaml`, expand matrices, merge fragments, interpolate params, render cells. |
| `tailor-core` | Domain model, build plans, lockfile/stamp logic, orchestration, and port traits. |
| `tailor-resolve` | Resolve toolchain and base image digests/hashes. |
| `tailor-exec` | Docker/Bollard execution adapter, IC arg construction, path translation, cleanup. |
| `tailor-sign` | Host-side signing backends for the `Signer` port (openssl/`sbsign`); preview-gated. |
| `tailor` | CLI parsing, command dispatch, output formatting, and composition root. |

The config-to-render path is deterministic and synchronous. Container execution is the async boundary because Docker/Bollard and log streaming are async.
