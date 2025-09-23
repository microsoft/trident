# Viewing Trident's Background Log

Trident records trace level logging information while it is performing an install or update. This
can be useful for debugging and troubleshooting issues.

## Location of the Log File

By default, Trident saves these logs to the following location:

`/var/log/trident-full.log`

## Contents

This log file has similar contents to the output of Trident itself, with the exception that this log
is NOT filtered, meaning all log messages, regardless of their level will be present in this file.

**NOTE: The file is truncated every time Trident is restarted!**

## Format

The file is a JSON stream, with each line being a JSON object. The JSON object represents a full log
line, with the following fields:

| Field     | Type   | Description                                       |
| --------- | ------ | ------------------------------------------------- |
| `level`   | String | one of `trace`, `debug`, `info`, `warn`, `error`. |
| `message` | String | the log message itself.                           |
| `target`  | String | Rust log target (generally the module path).      |
| `module`  | String | Rust module path.                                 |
| `file`    | String | Source file where the log was generated.          |
| `line`    | u32    | Line number where the log was generated.          |

## Logs from Past Servicing

In addition to the full Trident log file from the current servicing, the user can also view the logs
from any **past** servicing executed by Trident. These logs are persisted from the MOS or old
runtime OS to **the directory adjacent** **to the datastore** in the updated target OS.

After each servicing, the full Trident log is persisted to a file named
`trident-<servicing_state>-<timestamp>.log`, where the timestamp corresponds to the time when the
log was persisted to the updated target OS. Servicing state is the state that Trident was in when
the logs were copied over: e.g. the logs for the staging of an A/B update would be named
`trident-ab-update-staged-<timestamp>.log`.

Similarly, Trident also persists the metrics logs to the updated target OS, i.e.
`trident-metrics-<servicing_state>-<timestamp>.log`.
