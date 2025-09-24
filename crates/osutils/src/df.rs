use std::path::Path;

use anyhow::{Context, Error};

use crate::dependencies::Dependency;

pub fn available_space_in_fs(path: impl AsRef<Path>) -> Result<u64, Error> {
    let output = Dependency::Df
        .cmd()
        .arg(path.as_ref())
        .args(["-B", "1", "--output=avail"]) // Return available space in directory in bytes
        .output_and_check()
        .context("Failed to execute df")?;

    parse_df_available_space_output(output)
}

fn parse_df_available_space_output(output: String) -> Result<u64, Error> {
    output
        .lines()
        .nth(1) // Skip the header line
        .context("Failed to access available space output from df")?
        .parse::<u64>()
        .context("Failed to parse available space")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_df_available_space_output() {
        // Should succeed
        assert_eq!(
            parse_df_available_space_output("Avail\n0".to_string()).unwrap(),
            0_u64
        );
        assert_eq!(
            parse_df_available_space_output("Avail\n1".to_string()).unwrap(),
            1_u64
        );
        assert_eq!(
            parse_df_available_space_output("Avail\n107074944".to_string()).unwrap(),
            107074944_u64
        );

        // Cannot parse alphabetical characters
        assert!(parse_df_available_space_output("Avail\nzero".to_string())
            .unwrap_err()
            .to_string()
            .contains("Failed to parse available space"));

        // Expects two lines (header line followed by availability)
        assert!(parse_df_available_space_output("1".to_string())
            .unwrap_err()
            .to_string()
            .contains("Failed to access available space output from df"));

        // Expects non-negative numbers
        assert!(parse_df_available_space_output("Avail\n-1".to_string())
            .unwrap_err()
            .to_string()
            .contains("Failed to parse available space"));
    }
}
