# Logging Standards

## Revision Summary

| Revision | Date       | Comment          |
| -------- | ---------- | ---------------- |
| 1.0      | 2024-10-31 | Initial version. |

## Background

Besides the Host Status, logging is the fundamental way for Trident to
communicate the state of the host and the progress of the ongoing servicing to
the customer.

For that reason, it is important to maintain a set of clear and coherent
standards for logging in Trident. This document summarizes the role of
different [log levels](#log-levels), outlines how logs should be
structured and what information should be logged, and finally, provides
examples for developers to use while adding new logs.

## Goals

This document outlines the standards for logging in Trident, to accomplish the
following goals:

- Communicating the state of the host and servicing with clarity and sufficient
  context.
- Whenever the customers are the target audience of any log, speaking in terms
  that they will be able to understand.
- Ensuring consistency in the meaning and usage of different [log levels](#log-levels).
- Using consistent and uniform structure and language across all the logs
  produced by Trident.

## Overview

### Background Log

In addition to the output, Trident logs all its updates to a log file, which
can be used both by developers and customers for debugging and troubleshooting.
This log file has similar contents to the output, with the exception that this
log is NOT filtered, meaning that **all** log messages, regardless of their
[log levels](#log-levels), will be present in this file. Currently, Trident
saves these logs to the following location:

`/var/log/trident-full.log`

Refer to [Viewing Trident's Background Log](../../How-To-Guides/View-Trident's-Background-Log.md)
for more detailed guidelines on how to use the background log file.

### Log Levels

For each command that can be run with Trident, the customer can adjust the log
level, i.e. the verbosity level, to one of the following six values: `OFF`,
`ERROR`, the **default** `WARN`, `INFO`, `DEBUG`, and `TRACE`.

The last five correspond to the actual functions in the `log` crate that the
developers can use to write logs in Trident:

```rust
error!("...");
warn!("...");
info!("...");
debug!("...");
trace!("...");
```

Each log level has its specific purpose and must be used in a particular
context. Below, the document outlines the role of each log level and provides
example scenarios in which it can be used.

#### `ERROR`

As the name suggests, the log level `ERROR` is used when an error has occurred
in the system. When something doesn't go as expected by Trident, it could
either be:

1. A fatal error that forces Trident to shut down the ongoing servicing,
2. A non-fatal error that Trident can recover from, but that the customer
  should still be aware of.

##### `ERROR` Guidelines

The primary audience of the `ERROR` logs is **the customers**. This means that
every `ERROR` log should try to address (1) what went wrong, (2) why it might
have occurred, and (3) how the customer might potentially solve it. The message
should focus on what the customer did, rather than detail how the internal,
Trident-driven steps have failed.

However, in case of some errors, such as servicing errors or the *Example 1*
below, Trident is not able to state the cause of the failure. Then, the goal of
the log is to accurately **expose and propagate the original error** in the
system, so that the customer can address it themselves.

If possible, an `ERROR` log should follow the following structure:

```rust
error!("Failed to <PERFORM SOME ACTION> due to <POTENTIAL CAUSE THAT CAN BE ADDRESSED>");
```

##### Fatal vs Non-Fatal Errors

`ERROR` logs can be used in case of both **fatal** and **non-fatal** errors. In
the former scenario, Trident has encountered a serious failure that interrups
the ongoing servicing; otherwise, if we can recover from the failure, then the
error is non-fatal. Another type of non-fatal errors is when the failure
*would be* fatal but another failure error has already occurred, so we're
reporting any subsequent errors as non-fatal.

In case of **a fatal error**, Trident must return a `TridentError` object, which
is automatically printed as `error!(...)` in the runtime and also included in
the Host Status under the `lastError` section. Because `TridentError` already
carries the error context, the callstack, and the error type, an `ERROR` log
should **only** be printed if there is any additional context that must be
shared with the customer. Refer to
[Structured Error in Trident](Structured-Error.md) for more details on
`TridentError`.

In case of **a non-fatal error**, no `TridentError` is returned, hence, an
`ERROR` log must be printed.

##### `ERROR` Examples

*Example 1.*

The snippet below is taken from the logic in `src/lib.rs`, where Trident
validates whether the firmware correctly booted from the updated target OS after
a clean install. Here, Trident detected that the firmware failed to boot from
the expected device. Because the contents of the `CleanInstallRebootCheck` error
already summarize the failure, an `ERROR` log is *not* needed.

```rust
datastore.with_host_status(|host_status| {
    host_status.servicing_type = ServicingType::NoActiveServicing;
    host_status.servicing_state = ServicingState::NotProvisioned;
})?;

return Err(TridentError::new(ServicingError::CleanInstallRebootCheck {
    root_device_path: root_device_path.to_string_lossy().to_string(),
    expected_device_path: expected_root_device_path.to_string_lossy().to_string(),
}));
```

```rust
#[error("Clean install failed as host booted from '{root_device_path}' instead of the expected device '{expected_device_path}")]
CleanInstallRebootCheck {
    root_device_path: String,
    expected_device_path: String,
},
```

*Example 2.*

The snippet below comes from the `Prepare` step, where Trident attempts to
restart SSHD, to enable SSH for a MOS user. If Trident fails to restart SSHD,
it can still continue the servicing, so it produces an `ERROR` log to inform
the customer and then moves on to the next step, without returning a
`TridentError`. We are not able to state why the restart has failed, but we
still want to surface the original error to the customer, so that they can
address it themselves.

```rust
if let Err(err) =
    osutils::systemd::restart_unit("sshd").context("Failed to restart sshd in MOS")
{
    error!("{err:?}");
}
```

#### `WARN`

`WARN` is used to warn the customer about a potential issue that might break
the servicing in the future. In other words, `WARN` is reserved for scenarios
where some non-critical expectation is not being met. In this case, no error is
returned, and so Trident will continue the servicing.

##### `WARN` Guidelines

Similarly to `ERROR`, `WARN` logs are primarily addressed at **the customer**.
The goal of `WARN` is to warn the user about flows that:

- Technically work but may negatively affect security.
  - E.g. a Host Configuration requests to ignore the hash of an image, meaning
    that the hash validation is skipped.
- Request an unusual action that will probably work but goes against our
  expectations.
  - E.g. a Host Configuration requests to mount partitions of a certain type in
    locations where they are not expected, such as mounting the ESP partition
    at `/mnt/random-place`.

The difference between a warning and a non-fatal error might be confusing at
times. In general, we issue a `WARN` when the requested action *might* be
intentional, although we find it unsafe or unusual. On the other hand, when we
know that the problematic flow is most likely not what the customer intended
and should probably address, we issue a non-fatal error with an `ERROR` log.

##### `WARN` Examples

*Example 1.*

The code snippet below refers to a scenario where the customer is requesting to
ignore the hash of an image in the Host Configuration. Before deploying the
image onto the block device, Trident produces a `WARN` log because streaming an
unverified payload onto a block device is a potential security concern.

```rust
match image.sha256 {
    ImageSha256::Ignored => {
        warn!("Ignoring SHA256 for image from '{}'", image_url);
    }
    ...
}
```

*Example 2.*

Taken from the logic in `src/lib.rs`, the code below addresses the scenario
where the customer has updated the Host Configuration but did not include
`stage` under the allowed operations section. This means that the servicing
requested in the Host Configuration cannot be executed. While this does not
cause an error, Trident suspects that the customer might have forgotten to
include `stage` in the `--allowed-operations` option and hence, it issues a
`WARN` log. This is not a failure but a no-action scenario, so we do not
return an `ERROR` log.

```rust
if cmd.allowed_operations.has_stage() {
    engine::update(cmd, datastore).message("Failed to run update")
} else {
    warn!("Host config has been updated but allowed operations do not include 'stage'. Add 'stage' and re-run");
    Ok(())
}
```

#### `INFO`

`INFO` is reserved for the most high-level updates, such as announcing the
kickoff or successful completion of the next step in the servicing process.
The `INFO` logs are written primarily for **the developers** and some
**savvy customers** who want to understand how the servicing is progressing.

##### `INFO` Examples

*Example 1.*

Creating a RAID array is a major operation within the process of initializing
block devices. Thus, we use an `INFO` log to inform the user that a RAID array
requested in the Host Configuration is being created.

```rust
info!("Initializing '{}': creating RAID array", config.id);
mdadm::create(&config.device_path(), &config.level, device_paths)
    .context("Failed to create RAID array")?;
```

*Note:* Whenever a device is being initialized, Trident formats the log
message in the same way, to maintain consistency:

```rust
"Initializing '<BLOCK DEVICE ID>': ..."
```

*Example 2.*

The example below comes from the logic in `src/lib.rs`, where Trident has
validated that the host had correctly booted from the updated target OS. Here,
Trident uses **two** different levels of logging:

- First, an `INFO` log announces that the higher-level process, i.e. a clean
  install or an A/B update, succeeded.
- Second, a `DEBUG` log communicates to the user that the servicing state of
  the host has changed, which is a more granular update and thus, relevant
  to the developer rather than the customer. *Note:* More information about
  `DEBUG` logs will be shared in the next section.

```rust
if datastore.host_status().servicing_type == ServicingType::CleanInstall {
    info!("Clean install of target OS succeeded");
    tracing::info!(metric_name = "clean_install_success", value = true);
} else {
    info!("A/B update succeeded");
    tracing::info!(metric_name = "ab_update_success", value = true);
}
debug!(
    "Updating host's servicing state to '{:?}'",
    ServicingState::Provisioned
);
```

#### `DEBUG`

`DEBUG` logs are the primary logs, meaning that they report the individual
actions that Trident is completing during the servicing. Unlike the `ERROR` and
`WARN` logs, `DEBUG` logs are aimed at **the developers**, who can use them to
debug the code.

##### `DEBUG` Guidelines

Conceptually, `DEBUG` logs are the "normal" logs, which provide more details
than `INFO` but fewer details than `TRACE`.

Compared to `INFO` logs, `DEBUG` statements go one level lower, reporting
specific paths, device names, ids, and other granular pieces of info that are
important to the developers or some very savvy customers. On the other hand,
the `TRACE` logs provide even more detailed info, such as the exact contents of
a file/config. *Note:* More information about `TRACE` logs will be shared in
the next section.

If a major step in the servicing consists of multiple operations that might
potentially fail, we first issue an `INFO` log before Trident begins the
operation and then, we use `DEBUG` and `TRACE` logs to report the more granular
details. Finally, if the operation has completed successfully, we can issue
another `INFO` log, to confirm that the operation succeeded. (The confirmation
is optional since Trident logging the next step is a confrmation in itself.)

##### `DEBUG` Examples

*Example 1.*

The snippet below comes from the logic in `osutils/src/grub_mkconfig.rs` where
a new grub-mkconfig script is being written on the host. Here, we're using a
`DEBUG` log to report the path at which the script is being written, since it's
too low-level to be relevant to the overwhelming majority of the customers. At
the same time, it's definitely relevant to the developer. On the other hand,
seeing the contents of the script is rarely needed even by the developer, and
so we're using a `TRACE` log in this case.

```rust
debug!("Writing grub-mkconfig script to '{}'", path.display());

let content = self.render();
trace!(
    "Grub-mkconfig script content:\n{}",
    content.to_string_lossy()
);
```

*Example 2.*

The below function illustrates the difference between `INFO` and `DEBUG` log
levels. First, we're using an `INFO` log to report that Trident is starting the
`Prepare` step, which is a high-level stage of the servicing. However, knowing
what subsystem is currently being prepared is more granular and would be
helpful to the developers rather than to the customers. Thus, this info is
recorded via a `DEBUG` log.

```rust
fn prepare(subsystems: &mut [Box<dyn Subsystem>], ctx: &EngineContext) -> Result<(), TridentError> {
    info!("Starting step 'Prepare'");
    for subsystem in subsystems {
        debug!(
            "Starting step 'Prepare' for subsystem '{}'",
            subsystem.name()
        );
        subsystem.prepare(ctx).message(format!(
            "Step 'Prepare' failed for subsystem '{}'",
            subsystem.name()
        ))?;
    }
    debug!("Finished step 'Prepare'");
    Ok(())
}
```

#### `TRACE`

`TRACE` is used for logging the most granular info, which could only be useful
to the developers as they are going through the source code. Whatever actions
seem too granular for the `DEBUG` logs should be logged at the `TRACE` level.
The section below attempts to provide a guide on how to decide whether a log is
`TRACE` or `DEBUG`.

##### `TRACE` Guidelines

When trying to decide whether a piece of information should be logged at the
`TRACE` vs `DEBUG` levels, consider the following questions:

1. Are you logging an operation executed by Trident OR a generic step/"a side
   effect" of an operation? If yes, it's a `DEBUG`; if no, it's a `TRACE`.

   - E.g. announcing that Trident is creating a new mdadm config file at a path
    is a `DEBUG` log because it describes a particular operation done by
    Trident. On the other hand, printing out the contents of the config is
    showing an intermediate outcome of this operation. So, `TRACE` makes more
    sense here.

2. Does the code have any concept of the bigger picture? If yes, it's a `DEBUG`;
   if no, it's a `TRACE`.
   - E.g. the most common use case of `TRACE` is logging every sub-command that
    Trident executes. In this case, the low-level logic has no idea what
    exactly Trident is trying to achieve; it just logs that a certain command,
    with some arguments, is being executed and then prints out its output. But
    it does not have an understanding of why it is being run.

3. Is the log produced in **osutils**? If yes, it should most likely be a
  `TRACE`, unless it is a specific warning/error.

##### `TRACE` Examples

*Example 1.*

Similarly to one of the examples above, this snippet highlights the difference
between the `DEBUG` and `TRACE` log levels. While the path of the mdadm config
is a pretty fundamental piece of info for the developers, the content of the
file is a less relevant outcome of that operation, so it should be reported
with a `TRACE` log.

```rust
debug!("Creating mdadm config file '{}'", mdadm_config_file_path);
trace!("Contents:\n{}", output);
```

*Example 2.*

This example illustrates the most common use of `TRACE`: for logging the
lowest-level sub-commands that Trident executes and reporting their output.
These logs allow the developers to see what command is being run at the lowest
level, while other logs, such as `DEBUG` will provide more context on why this
command was executed in the first place.

```rust
fn run_and_check(&mut self) -> Result<(), Error> {
    let rendered_command = self.render_command();
    trace!("Executing '{rendered_command}'");
    let result = self.output();
    trace!(
        "Executed '{rendered_command}': {}. Report:\n{}",
        result.explain_exit(),
        result.output_report(),
    );
    result
        .check()
        .with_context(|| format!("Error when running: {}", self.render_command()))
}
```

### Log Structure and Contents

- The log message should concisely summarize the update or error that needs to
  be communicated to the user. If possible, summarize the info in a single
  phrase or a sentence.
- The logs must provide all the relevant information.
  - If a log is aimed at the customers, and in the case of `ERROR` and `WARN`
    logs, in particular, it must contain the block device id if applicable.
    This will allow the customer to quickly identify which device is causing
    the issue.
- On the other hand, the logs should not duplicate each other or provide
  irrelevant info that is not helpful to the target audience. *Logging too much*
  *is as bad as logging too little.*
  - E.g. `ERROR` logs should not simply duplicate the info already provided in
    the payload of the returned error.
  - E.g. the lowest-level, potentially confusing info, such as the full paths of
    the block devices in the system, should be omitted in the logs aimed at the
    customers.
- Do not end the log with a period, an exclamation mark, an ellipsis, etc. to
  use uniform formatting and maintain a neutral tone.
- Refer to the API definitions and other documentation to use correct language
  and apply Trident-specific terms consistently in the logs.
- If a series of logs informs about a similar change or step, try to maintain a
  uniform structure in the message.
  - For example, we currently use a unique statement structure to report which
    step of the servicing is being executed or which storage component is being
    initialized.
