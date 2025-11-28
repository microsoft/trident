use anyhow::{anyhow, Context, Error};

/// Returns the hostname of the system.
pub fn read() -> Result<String, Error> {
    hostname::get()
        .context("Failed to get hostname")?
        .into_string()
        .map_err(|err| {
            anyhow!(
                "Failed to convert hostname to string: {}",
                err.to_string_lossy()
            )
        })
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use crate::exe::RunAndCheck;

    use super::*;

    #[test]
    fn test_read() {
        let hostname = read().unwrap();
        assert!(!hostname.is_empty());

        let expected_hostname = Command::new("hostname")
            .output_and_check()
            .unwrap()
            .trim()
            .to_owned();

        assert_eq!(hostname, expected_hostname);
    }
}
