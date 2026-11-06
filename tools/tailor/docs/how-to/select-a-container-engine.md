# Select a container engine (Docker or Podman)

tailor runs Image Customizer in a container through the Docker Engine API. Podman speaks the same API, so tailor can use either — the engine is just a different socket.

## Use Podman for the whole workspace

Set `runtime.engine` in `tailor.yaml`:

```yaml
# tailor.yaml
runtime:
  engine: podman   # docker (default) | podman | auto
```

With `engine: podman`, tailor connects to Podman's default socket: the rootless socket `$XDG_RUNTIME_DIR/podman/podman.sock` when `XDG_RUNTIME_DIR` is set, otherwise the rootful `/run/podman/podman.sock`. Start the service first if it is not socket-activated:

```bash
systemctl --user start podman.socket   # rootless
# or, ad hoc:
podman system service --time=0 &
```

## Use Podman for one build only

The global `--engine` flag overrides the manifest for a single invocation:

```bash
tailor --engine podman build app
```

## Point at a specific socket or a remote engine

Use `--host` (or `runtime.host`) for a non-default endpoint:

```bash
tailor --host unix:///run/user/1000/podman/podman.sock build app
tailor --engine podman --host tcp://builder:2375 build app
```

`--host` takes a `unix://…` socket path (a bare `/path` works too) or a `tcp://…` endpoint.

## Auto-detect

`engine: auto` (or `--engine auto`) probes the Docker socket (or `DOCKER_HOST`), then the rootless and rootful Podman sockets, and uses the first that answers:

```bash
tailor --engine auto build app
```

## Precedence

Engine and endpoint resolve as two independent axes, each highest-first:

- **engine** — `--engine` → `runtime.engine` → `docker`.
- **endpoint** — `--host` → the engine's environment variable (`DOCKER_HOST` for docker, `CONTAINER_HOST` for podman) → `runtime.host` → the engine's default socket (or, for `auto`, the probe list).

`--engine` and `--host` are complementary: `--engine` chooses the daemon, `--host` chooses where it lives. A `DOCKER_HOST` set in the environment is ignored when the resolved engine is podman.

## Fail fast

Before a build or clean starts, tailor pings the resolved engine and aborts immediately — before any image work — if it is missing or unreachable, naming the engine, the endpoint, and a fix:

```text
$ tailor --engine podman --host unix:///nonexistent.sock build app
error: container runtime error: podman engine not found: no socket at /nonexistent.sock.
Start it with `systemctl --user start podman.socket` (rootless) or `podman system service`.
```

`tailor build --dry-run` never contacts an engine, so it works without a running daemon regardless of `--engine` / `--host`.

> If you point `--engine` at an endpoint that is actually a different engine (for example `--engine podman` at a Docker socket), tailor uses the engine the socket really is and prints a warning — the live daemon is the source of truth.
