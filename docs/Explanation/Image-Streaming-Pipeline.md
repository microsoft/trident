# Image Streaming Pipeline

Trident uses a streaming pipeline to transfer OS images from remote sources
directly to disk partitions. Rather than downloading an entire image to local
storage before writing it, data flows through a series of stages —
fetching, decompressing, hashing, and writing — in small chunks. This pipeline
is the common transfer mechanism shared across multiple servicing types
including install, A/B update, extensions, and disk streaming.

## Why Stream?

OS images can be several gigabytes in size. Downloading the full image before
writing it would require equivalent temporary storage and would significantly
increase provisioning time. Streaming solves both problems:

- **Low memory footprint** — data is processed in small buffers (4 MB) rather
  than loaded entirely into memory.
- **No temporary storage** — data flows from the network directly to the target
  disk partition.
- **Integrity verification** — a cryptographic hash (SHA-384) is computed as
  data flows through, so corruption is detected without a separate verification
  pass.
- **Network resilience** — partial failures are recovered transparently by
  resuming from the last successful byte position.

## Pipeline Stages

:::mermaid
flowchart LR
    Remote["Remote Source<br/>(HTTP / OCI)"] --> HttpSubFile["HTTP Range<br/>Reader"]
    HttpSubFile --> Hash["SHA-384<br/>Hasher"]
    Hash --> Decompress["ZSTD<br/>Decompressor"]
    Decompress --> Disk["Target<br/>Partition"]
:::

### Remote Source

Trident supports two types of remote image sources:

- **HTTP/HTTPS URLs** — standard web servers that support HTTP Range requests
  ([RFC 7233](https://datatracker.ietf.org/doc/html/rfc7233)).
- **OCI registry URLs** — container registries using the `oci://` scheme
  (e.g., `oci://registry.example.com/image:tag`). Trident authenticates with
  the registry, resolves the image manifest, and downloads the image layer as
  an HTTP blob.

### HTTP Range Requests

The core of the streaming mechanism is the HTTP Range request. Instead of
downloading the entire file in a single request, Trident requests specific byte
ranges. This provides two key benefits:

1. **Resumable downloads** — if a network error occurs mid-transfer, Trident
   resumes from the byte where the failure happened rather than restarting from
   the beginning.
2. **Partial response handling** — if a server responds with fewer bytes than
   requested, Trident transparently issues additional requests for the remaining
   bytes. This is invisible to the upper layers of the pipeline.

### Hashing

As compressed data passes through the pipeline, Trident computes a SHA-384 hash
over it. After the stream completes, the computed hash is compared against the
expected value. A mismatch causes the operation to fail.

### Decompression

Images are compressed with [ZSTD](https://facebook.github.io/zstd/). After
hashing, Trident feeds the compressed stream into a ZSTD decoder, which
produces decompressed data for writing to disk.

### Writing to Disk

Data is written to the target block device using buffered I/O (4 MB buffers) to
minimize the number of system calls and optimize throughput. After all data has
been written, Trident flushes the buffer and calls `sync` to ensure the data is
persisted to the physical disk.

## Where the Pipeline Is Used

The image streaming pipeline is the common transfer mechanism across Trident's
servicing operations:

- **Install** — root filesystem, ESP, and other partition images are streamed
  from remote sources to their target partitions.
- **A/B Update** — new OS images are streamed to the inactive volume while the
  current OS continues running.
- **Extensions** — system extensions (sysexts) and configuration extensions
  (confexts) are streamed and placed in their designated directories.
- **[Disk Streaming](./Disk-Streaming.md)** — full COSI images are streamed via
  the gRPC `StreamingService`.
