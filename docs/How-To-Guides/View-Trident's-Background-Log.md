# Viewing Trident's Background Log

> **DISCLAIMER: THIS IS A PREVIEW FEATURE.**
>
> **THE FORMAT AND LOCATION OF THIS LOG FILE MAY CHANGE IN THE FUTURE, _ONLY USE
> IT FOR DEBUGGING PURPOSES!_**

In the background, Trident logs all its activities to a log file. This log file
is useful for debugging and troubleshooting. This guide explains how to view
Trident's full log.

The background log is **only generated when using the `run` subcommand**.

## Location of the Log File

By default, Trident saves these logs to the following location:

`/var/log/trident-full.log`

## Contents

This log file has similar contents to the output of Trident itself, with the
exception that this log is NOT filtered, meaning all log messages, regardless of
their level will be present in this file.

**NOTE: The file is truncated every time Trident is restarted!**

## Format

The file is a JSON stream, with each line being a JSON object. The JSON object
represents a full log line, with the following fields:

| Field     | Type   | Description                                       |
| --------- | ------ | ------------------------------------------------- |
| `level`   | String | one of `trace`, `debug`, `info`, `warn`, `error`. |
| `message` | String | the log message itself.                           |
| `target`  | String | Rust log target (generally the module path).      |
| `module`  | String | Rust module path.                                 |
| `file`    | String | Source file where the log was generated.          |
| `line`    | u32    | Line number where the log was generated.          |

## Logs from Past Servicing
In addition to the full Trident log file from the current servicing, the user
can also view the logs from any **past** servicing executed by Trident. These
logs are persisted from the MOS or old runtime OS to **the directory adjacent**
**to the datastore** in the updated runtime OS.

After each servicing, the full Trident log is persisted to a file named
`trident-<servicing_type>-<timestamp>.log`, where the timestamp corresponds to
the time when the log was persisted to the updated runtime OS. 
