# gRPC Server

Trident includes a gRPC server that exposes a programmatic API for performing
OS servicing operations. The server runs as a daemon (`tridentd`) and
communicates over a Unix domain socket, enabling external tools and
orchestration systems to interact with Trident without relying on the CLI.

For the full design rationale and background, see
[RFC 0379: gRPC API](../Development/RFCs/0379-grpc-api.md).

## Why a gRPC Server?

While the existing CLI is well-suited for direct operator usage and simple
scripting, it has limitations when used by complex orchestrators:

- **Environment inheritance** — invoking Trident as a child process requires
  the caller to already be running with root privileges and the proper
  environment.
- **Unstructured output** — a calling agent only has raw stdout/stderr and an
  exit code to determine what Trident is doing.
- **Filesystem coupling** — passing data such as a Host Configuration file
  requires writing it to a shared filesystem location.
- **Multi-step workflows** — operations that require reading status, performing
  a servicing, and reading the result need multiple binary invocations and
  output parsing.

The gRPC server addresses these by providing:

- **A well-defined API contract** via Protocol Buffers, enabling clients in any
  language that supports gRPC.
- **Streaming progress updates** so that callers can observe operation progress
  in real time without polling.
- **Reboot management** that lets callers choose whether Trident or the caller
  handles required reboots.
- **Security by default** through Unix domain socket communication, which
  restricts access to the local machine and leverages filesystem permissions.

## Architecture

The server is implemented using [Tonic](https://github.com/hyperium/tonic), a
Rust gRPC framework built on top of the Tokio async runtime. The API is defined
in Protocol Buffer files under the `proto/` directory and compiled into Rust
types by the `trident-proto` crate.

:::mermaid
flowchart LR
    Client["gRPC Client"] -- Unix Socket --> Server["Trident Daemon\n(tridentd)"]
    Server --> Version["VersionService"]
    Server --> Streaming["StreamingService"]
:::

### Unix Domain Socket

The server listens on a Unix domain socket rather than a TCP port. By default,
the socket is created at `/run/trident/trident.sock` with permissions restricted
to the root user (`0600`). This design avoids exposing the API over the network
and leverages standard Unix file permissions for access control.

The socket file is owned by `root:root` and blocks all read/write attempts from
non-root users. This preserves Trident's existing security model where root
access is a prerequisite for any interaction.

### Systemd Integration

Trident ships with two systemd unit files for managing the daemon:

- **`tridentd.socket`** — creates and owns the Unix socket at
  `/run/trident/trident.sock` with mode `0600` owned by `root:root`. When a
  client connects, systemd activates the daemon service automatically.
- **`tridentd.service`** — runs the Trident daemon. When socket-activated,
  systemd passes the socket file descriptor to the daemon process via the
  `LISTEN_FDS` and `LISTEN_FDNAMES` environment variables.

With socket activation, the daemon is only running when there is work to do.
When the daemon shuts down due to inactivity, systemd continues to own the
socket and will reactivate the daemon on the next incoming connection.

### Connection Management

The gRPC server uses a read-write lock to manage concurrent connections. Data
retrieval operations (such as querying status) acquire a read lock, allowing
multiple simultaneous readers. Servicing operations (such as install or update)
acquire a write lock, ensuring that at most one servicing operation runs at a
time. An additional global servicing lock prevents a second servicing from
starting if a previous one is still running after the client disconnects.

### Inactivity Shutdown

The server tracks active connections and ongoing servicing operations through
middleware that wraps every service call. When there are no active connections
and no operations in progress for a configurable duration (default: 5 minutes),
the server shuts down automatically. The inactivity timer resets after every
connection finishes.

## Services

The server exposes the following stable (v1) services:

### VersionService

Returns the Trident daemon version. This is useful for clients to verify
compatibility before initiating servicing operations.

```protobuf
service VersionService {
  rpc Version(VersionRequest) returns (VersionResponse);
}
```

### StreamingService

Streams an OS image directly to the target disk. This service accepts a remote
image URL and an optional hash for integrity verification. See
[Disk Streaming](./Disk-Streaming.md) for a detailed explanation of how disk
streaming works.

```protobuf
service StreamingService {
  rpc StreamDisk(StreamDiskRequest) returns (stream ServicingResponse);
}
```

## Streaming Responses

All servicing operations return a stream of `ServicingResponse` messages. This
allows the client to observe the operation in real time. The stream contains
three types of messages:

1. **Started** — signals the beginning of the operation.
2. **Log** — carries log messages generated during the operation, including the
   log level, message body, and originating module.
3. **Completed** — the final message in the stream, indicating success or
   failure and whether a reboot is required.

This pattern allows callers to display progress to users, forward logs to
external systems, or simply wait for the final result.

## Reboot Management

Servicing operations that modify boot configuration may require a reboot to take
effect. The gRPC API allows callers to specify how reboots should be handled via
the `RebootManagement` field in the request:

| Mode | Behavior |
|------|----------|
| `TRIDENT_HANDLES_REBOOT` | Trident initiates the reboot automatically. |
| `CALLER_HANDLES_REBOOT` | The caller is responsible for rebooting the system. The `Completed` response indicates that a reboot is required. |
| `REBOOT_HANDLING_UNSPECIFIED` | Defaults to Trident handling the reboot. |

Choosing `CALLER_HANDLES_REBOOT` is useful when the caller needs to perform
additional steps (such as draining workloads) before the machine reboots.

## Running the Server

### With systemd (recommended)

Enable socket activation so the daemon starts on demand:

```bash
sudo systemctl enable --now tridentd.socket
```

The daemon will activate automatically when a client connects to the socket.

## Running in a Container

The daemon can run as the entry point of a container. A volume must be mounted
at the socket path so that an external agent can connect to the gRPC API.
