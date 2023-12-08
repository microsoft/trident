use anyhow::Error;

pub fn add_secondary_error_context(
    primary: Error,
    secondary: impl Into<Box<dyn std::error::Error + Send + Sync>>,
) -> Error {
    primary.context(format!(
        "While handling the error, an additional error was caught: \n\n{:?}\n\nThe earlier error:",
        secondary.into()
    ))
}
