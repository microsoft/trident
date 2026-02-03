# Async LogStream Implementation

This document describes the async version of the logstream logger, implemented in `logstream_async.rs`.

## Overview

The async logstream implementation provides a non-blocking alternative to the synchronous `logstream.rs`. It uses a sidecar thread to handle all HTTP requests to the logging server, allowing the main application to continue executing without waiting for network I/O.

## Key Differences from Synchronous Version

### Architecture

**Synchronous (`logstream.rs`)**:
- Each `log()` call directly makes a blocking HTTP POST request
- The calling thread waits for the HTTP request to complete
- Simple, straightforward implementation

**Asynchronous (`logstream_async.rs`)**:
- Spawns a dedicated worker thread on creation
- `log()` calls send log entries through an in-memory channel (non-blocking)
- Worker thread reads from channel and makes HTTP requests in the background
- Main thread continues immediately after sending to channel

### Components

#### LogstreamAsync
Equivalent to `Logstream` - manages the server URL and creates loggers.

#### AsyncLogSender  
Equivalent to `LogSender` - implements the `log::Log` trait.

Key additions:
- `sender: Option<Sender<LogMessage>>` - Channel sender for log entries
- `worker_thread: Option<JoinHandle<()>>` - Handle to the worker thread
- `finish()` method - Explicitly drains the queue and shuts down the worker

#### LogMessage (Internal)
A struct that bundles a `LogEntry` with its target URL for the worker thread.

### Worker Thread Lifecycle

1. **Creation**: Worker thread spawns when `LogstreamAsync::create()` is called
2. **Operation**: Continuously reads from channel and sends HTTP requests
3. **Shutdown**: When all channel senders are dropped:
   - Channel closes
   - Worker thread drains remaining messages
   - Worker thread exits
   - `join()` waits for worker thread to complete

### Graceful Shutdown

The async version provides explicit control over shutdown via the `LogstreamAsync` struct:

```rust
let mut logstream = LogstreamAsync::create();
logstream.set_server("http://logs.example.com/api/logs".to_string())?;

let logger = logstream.make_logger();

// ... use logger ...

// Explicitly finish all pending logs
logstream.finish();
```

The `finish()` method:
1. Drops the LogstreamAsync's channel sender (signals no more logs coming)
2. Waits for the worker thread to process all queued logs
3. Joins the worker thread

If `finish()` is not called explicitly, the `Drop` implementation on `LogstreamAsync` ensures cleanup happens automatically.

## Usage Example

```rust
use trident::logging::logstream_async::LogstreamAsync;

// Create the logstream (spawns worker thread)
let mut logstream = LogstreamAsync::create();

// Set the server URL
logstream.set_server("http://logs.example.com/api/logs".to_string())?;

// Create a logger
let logger = logstream.make_logger();

// Use with the log macros (non-blocking)
log::info!("Application started");
log::error!("An error occurred");

// When shutting down, ensure all logs are sent
logstream.finish();
```

## Benefits

1. **Non-blocking**: Main thread doesn't wait for network I/O
2. **Performance**: Better throughput for high-frequency logging
3. **Resilience**: Network delays don't slow down the application
4. **Batching potential**: Worker thread could be extended to batch requests

## Trade-offs

1. **Complexity**: More complex than the synchronous version
2. **Memory**: Channel buffers log entries in memory
3. **Ordering**: Log delivery order is guaranteed, but timing is asynchronous
4. **Shutdown**: Requires explicit `finish()` or reliance on `Drop` for clean shutdown

## Testing

The async implementation includes comprehensive tests:

- `test_logstream_async`: Basic functionality and server configuration
- `test_lock`: Disabled state handling
- `test_finish_method`: Explicit shutdown mechanism
- `test_drop_cleanup`: Automatic cleanup on drop
- `test_channel_communication`: Multiple log messages through channel

## When to Use

**Use Async Version** when:
- High-frequency logging is needed
- Network latency to log server is significant
- Application performance is critical
- Logs should not block the main execution path

**Use Sync Version** when:
- Simplicity is preferred
- Logging frequency is low
- Immediate feedback on log delivery is needed
- Application is shutting down and needs guaranteed log delivery

## Implementation Notes

### Channel Type
Uses `std::sync::mpsc::channel()` - an unbounded channel. This means:
- Sending to the channel never blocks
- Memory usage grows if logs are produced faster than they can be sent
- Alternative: Could use a bounded channel with back-pressure

### Error Handling
- Serialization errors are logged to stderr, log is dropped
- HTTP errors are logged once (first failure only) to avoid spam
- Channel send errors are silently ignored (channel closed means shutdown)

### Thread Safety
- `LogstreamAsync` is `Clone` (uses `Arc<RwLock<>>` internally)
- Multiple loggers can share the same server configuration
- Each `AsyncLogSender` has its own worker thread and channel
