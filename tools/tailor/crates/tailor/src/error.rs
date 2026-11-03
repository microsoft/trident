//! Application error type — wraps the typed library errors and adds CLI-level context.

/// Errors surfaced by the `tailor` binary.
#[derive(Debug, thiserror::Error)]
pub(crate) enum AppError {
    #[error(transparent)]
    Core(Box<tailor_core::CoreError>),

    #[error(transparent)]
    Config(#[from] tailor_config::ConfigError),

    #[error(transparent)]
    Resolve(#[from] tailor_core::ResolveError),

    #[error(transparent)]
    Exec(#[from] tailor_core::ExecError),

    #[error(transparent)]
    Sign(#[from] tailor_core::SignError),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Message(String),
}

// `CoreError` is boxed (it carries several large variants — the inter-image dependency diagnostics);
// a manual `From` keeps `?` ergonomic while keeping `AppError` small (`clippy::result_large_err`).
impl From<tailor_core::CoreError> for AppError {
    fn from(source: tailor_core::CoreError) -> Self {
        AppError::Core(Box::new(source))
    }
}

/// Process exit code for a build failure (an operational error: the engine failed, IO broke, a
/// resolution or signing step errored). The catch-all.
pub(crate) const EXIT_FAILURE: u8 = 1;
/// Process exit code for a usage error: invalid configuration or an invalid request (a bad selector,
/// an unknown image, a dependency cycle). Mirrors clap's own exit code for bad arguments.
pub(crate) const EXIT_USAGE: u8 = 2;
/// Process exit code for an interrupted run (`128 + SIGINT`), the conventional shell code for a
/// Ctrl+C-terminated program.
pub(crate) const EXIT_SIGINT: u8 = 130;

impl AppError {
    /// Map an error to its process exit code (`meta/docs` 1.0 exit-code taxonomy): `130` when the
    /// run was interrupted, `2` for a usage/configuration error, else `1` for a build failure.
    pub(crate) fn exit_code(&self) -> u8 {
        if self.is_cancelled() {
            EXIT_SIGINT
        } else if self.is_usage() {
            EXIT_USAGE
        } else {
            EXIT_FAILURE
        }
    }

    /// Whether this error is the graceful result of an interrupt (Ctrl+C / SIGTERM) cancelling a
    /// build, rather than a genuine failure.
    fn is_cancelled(&self) -> bool {
        match self {
            AppError::Exec(exec) => matches!(exec, tailor_core::ExecError::Cancelled),
            AppError::Core(core) => {
                matches!(
                    **core,
                    tailor_core::CoreError::Exec(tailor_core::ExecError::Cancelled)
                )
            }
            _ => false,
        }
    }

    /// Whether this error stems from invalid user input (configuration or request), as opposed to an
    /// operational build failure.
    fn is_usage(&self) -> bool {
        match self {
            AppError::Config(_) => true,
            AppError::Core(core) => core_is_usage(core),
            AppError::Exec(exec) => exec_is_usage(exec),
            AppError::Resolve(_) | AppError::Sign(_) | AppError::Json(_) | AppError::Message(_) => {
                false
            }
        }
    }
}

/// `CoreError` is dominated by planning/selection/configuration diagnostics (usage errors); only the
/// wrappers over operational failures are not. Unknown future variants default to usage, matching
/// the type's character.
fn core_is_usage(error: &tailor_core::CoreError) -> bool {
    use tailor_core::CoreError as C;
    match error {
        C::Resolve(_) | C::Sign(_) | C::Io { .. } | C::Serde { .. } => false,
        C::Exec(exec) => exec_is_usage(exec),
        _ => true,
    }
}

/// Execution errors are operational (a build failure) except the two that reject invalid user input
/// before the engine runs.
fn exec_is_usage(error: &tailor_core::ExecError) -> bool {
    use tailor_core::ExecError as E;
    matches!(error, E::ReservedParam { .. } | E::UnsafeDir { .. })
}

#[cfg(test)]
mod tests {
    use super::*;

    use tailor_core::{CoreError, ExecError};

    #[test]
    fn cancellation_maps_to_sigint() {
        assert_eq!(
            AppError::Exec(ExecError::Cancelled).exit_code(),
            EXIT_SIGINT
        );
        let wrapped = AppError::from(CoreError::Exec(ExecError::Cancelled));
        assert_eq!(wrapped.exit_code(), EXIT_SIGINT);
    }

    #[test]
    fn configuration_and_request_errors_map_to_usage() {
        let config = AppError::Config(tailor_config::ConfigError::EmptyAxis {
            axis: "arch".to_owned(),
        });
        assert_eq!(config.exit_code(), EXIT_USAGE);

        let selection = AppError::from(CoreError::NoCellsSelected {
            image: "img".to_owned(),
        });
        assert_eq!(selection.exit_code(), EXIT_USAGE);

        let reserved = AppError::Exec(ExecError::ReservedParam {
            param: "--output-image-file".to_owned(),
        });
        assert_eq!(reserved.exit_code(), EXIT_USAGE);
    }

    #[test]
    fn operational_failures_map_to_failure() {
        let ic = AppError::Exec(ExecError::IcFailed {
            cell: "cell".to_owned(),
            code: 1,
            dump: String::new(),
        });
        assert_eq!(ic.exit_code(), EXIT_FAILURE);

        let message = AppError::Message("another build is already running".to_owned());
        assert_eq!(message.exit_code(), EXIT_FAILURE);
    }
}
