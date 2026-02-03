# Logstream Comparison Examples

This document provides side-by-side examples of using the synchronous and asynchronous logstream implementations.

## Basic Usage

### Synchronous Version (`logstream.rs`)

```rust
use crate::logging::logstream::Logstream;
use log::{info, error};

// Create and configure
let logstream = Logstream::create();
logstream.set_server("http://logs.example.com/api/logs".to_string())?;

// Create logger
let logger = logstream.make_logger();

// Log messages (blocks until HTTP request completes)
info!("Application started");  // Blocks here
error!("An error occurred");   // Blocks here

// No explicit cleanup needed
```

### Asynchronous Version (`logstream_async.rs`)

```rust
use crate::logging::logstream_async::LogstreamAsync;
use log::{info, error};

// Create and configure (worker thread spawned on create)
let mut logstream = LogstreamAsync::create();
logstream.set_server("http://logs.example.com/api/logs".to_string())?;

// Create logger
let logger = logstream.make_logger();

// Log messages (returns immediately, queued for background upload)
info!("Application started");  // Non-blocking
error!("An error occurred");   // Non-blocking

// Explicit cleanup to ensure all logs are sent
logstream.finish();
```

## Advanced Usage

### Setting Custom Log Levels

**Sync:**
```rust
let logger = logstream.make_logger_with_level(log::LevelFilter::Info);
```

**Async:**
```rust
let logger = logstream.make_logger_with_level(log::LevelFilter::Info);
```

### Disabling the Logstream

**Sync:**
```rust
let mut logstream = Logstream::create();
logstream.disable();
// Server URL changes are now ignored
```

**Async:**
```rust
let mut logstream = LogstreamAsync::create();
logstream.disable();
// Server URL changes are now ignored
```

### Clearing the Server

**Sync:**
```rust
logstream.clear_server()?;
// Logs will no longer be sent
```

**Async:**
```rust
logstream.clear_server()?;
// Logs will no longer be sent
```

## Integration with MultiLogger

Both versions can be used with the MultiLogger for sending logs to multiple destinations.

### Synchronous

```rust
use crate::logging::multilog::MultiLogger;
use crate::logging::logstream::Logstream;

let logstream = Logstream::create();
logstream.set_server("http://logs.example.com/api/logs".to_string())?;

let logger = MultiLogger::new()
    .with_logger(logstream.make_logger())
    .with_logger(Box::new(env_logger::Builder::new().build()));

logger.init()?;
```

### Asynchronous

```rust
use crate::logging::multilog::MultiLogger;
use crate::logging::logstream_async::LogstreamAsync;

let mut logstream = LogstreamAsync::create();
logstream.set_server("http://logs.example.com/api/logs".to_string())?;

let logger = MultiLogger::new()
    .with_logger(logstream.make_logger())
    .with_logger(Box::new(env_logger::Builder::new().build()));

logger.init()?;

// ... application runs ...

// Important: Call finish on logstream before shutdown
logstream.finish();
```

## Shutdown Patterns

### Synchronous Version
```rust
// No special shutdown needed
// Logs are sent immediately, so no pending queue
drop(logger);  // Safe to drop anytime
```

### Asynchronous Version

**Option 1: Explicit finish (Recommended)**
```rust
let mut logstream = LogstreamAsync::create();
// ... use logstream ...
logstream.finish();  // Blocks until all pending logs are sent
```

**Option 2: Rely on Drop**
```rust
{
    let logstream = LogstreamAsync::create();
    // ... use logstream ...
}  // Drop called here, waits for worker thread
```

**Important**: The logstream must remain in scope until you're done logging.
The `finish()` method is on the logstream, not the logger.

## Performance Comparison

### High-Frequency Logging Scenario

**Sync Version:**
```rust
// Each log call blocks for ~50ms (network latency)
for i in 0..100 {
    info!("Processing item {}", i);  // 50ms per call
}
// Total time: ~5000ms (5 seconds)
```

**Async Version:**
```rust
// Each log call takes ~1μs (channel send)
for i in 0..100 {
    info!("Processing item {}", i);  // ~1μs per call
}
logger.finish();  // Waits for all 100 to be sent in background
// Main loop time: ~100μs
// Total time: ~5000ms (same, but main thread is free)
```

## Error Handling

Both versions handle errors similarly:

- **Serialization errors**: Logged to stderr, log entry dropped
- **HTTP errors**: First error logged to stderr, subsequent errors silent
- **Channel errors** (async only): Silently ignored (indicates shutdown)

## When to Use Which Version

### Use Synchronous (`logstream.rs`) when:

1. Simplicity is a priority
2. Log frequency is low
3. Immediate feedback on log delivery is needed
4. Guaranteed delivery before function return is required
5. Memory pressure is a concern

### Use Asynchronous (`logstream_async.rs`) when:

1. High-frequency logging
2. Network latency is significant
3. Application performance is critical
4. Log calls should not block execution
5. Can handle graceful shutdown complexity

## Migration Guide

To migrate from sync to async:

1. Change import:
   ```rust
   // FROM:
   use crate::logging::logstream::Logstream;
   // TO:
   use crate::logging::logstream_async::LogstreamAsync;
   ```

2. Make logstream mutable and change struct name:
   ```rust
   // FROM:
   let logstream = Logstream::create();
   // TO:
   let mut logstream = LogstreamAsync::create();
   ```

3. Add finish call before shutdown:
   ```rust
   // Add this before application exit:
   logstream.finish();
   ```

4. Test thoroughly, especially shutdown behavior

All other APIs are identical!
