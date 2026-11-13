use trident_api::config::Operations;

/// Maps a base command name plus its requested `Operations` to the same
/// naming convention gRPC's `servicing_request` already uses for
/// stage/finalize granularity (e.g. `"install"` vs `"install_stage"` vs
/// `"install_finalize"`), so `command`/`operation_id` telemetry is
/// consistent regardless of whether the command came from the CLI or from
/// gRPC/daemon.
pub fn command_name(base: &str, ops: &Operations) -> String {
    match (ops.has_stage(), ops.has_finalize()) {
        (true, true) => base.to_string(),
        (true, false) => format!("{base}_stage"),
        (false, true) => format!("{base}_finalize"),
        // Neither stage nor finalize was requested (an empty
        // `--allowed-operations` list); nothing can actually be staged or
        // finalized, so name this like the existing no-op naming convention
        // rather than a full install/update.
        (false, false) => format!("{base}_noop"),
    }
}
