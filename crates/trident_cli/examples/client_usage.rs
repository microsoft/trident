//! Example usage of the Trident CLI gRPC client
//!
//! This module demonstrates how to use the TridentClient to make gRPC calls
//! to the Trident core service.

fn main() {
    println!("This is an example showing how to use the Trident CLI gRPC client");
}

#[cfg(test)]
mod examples {
    use std::path::PathBuf;
    use trident_cli::cli::{AllowedOperation, Commands, GetKind};

    /// Example of creating various CLI commands that would translate to gRPC calls
    #[test]
    fn example_command_creation() {
        // Install command
        let install_cmd = Commands::Install {
            config: PathBuf::from("/etc/trident/config.yaml"),
            allowed_operations: vec![AllowedOperation::Stage, AllowedOperation::Finalize],
            status: Some(PathBuf::from("/tmp/status.json")),
            error: Some(PathBuf::from("/tmp/error.json")),
            multiboot: false,
        };
        assert_eq!(install_cmd.name(), "install");

        // Update command
        let update_cmd = Commands::Update {
            config: PathBuf::from("/etc/trident/config.yaml"),
            allowed_operations: vec![AllowedOperation::Stage],
            status: None,
            error: None,
        };
        assert_eq!(update_cmd.name(), "update");

        // Commit command
        let commit_cmd = Commands::Commit {
            status: None,
            error: None,
        };
        assert_eq!(commit_cmd.name(), "commit");

        // Get status command
        let get_status_cmd = Commands::Get {
            kind: GetKind::Status,
            outfile: Some(PathBuf::from("/tmp/status.json")),
        };
        assert_eq!(get_status_cmd.name(), "get");

        // Get configuration command
        let get_config_cmd = Commands::Get {
            kind: GetKind::Configuration,
            outfile: None,
        };
        assert_eq!(get_config_cmd.name(), "get");

        // Validate command
        let validate_cmd = Commands::Validate {
            config: PathBuf::from("/etc/trident/config.yaml"),
        };
        assert_eq!(validate_cmd.name(), "validate");

        // RebuildRaid command
        let rebuild_raid_cmd = Commands::RebuildRaid {
            config: Some(PathBuf::from("/etc/trident/config.yaml")),
            status: None,
            error: None,
        };
        assert_eq!(rebuild_raid_cmd.name(), "rebuild-raid");
    }

    /// Example showing how allowed operations work
    #[test]
    fn example_allowed_operations() {
        let stage_only = vec![AllowedOperation::Stage];
        let finalize_only = vec![AllowedOperation::Finalize];
        let both = vec![AllowedOperation::Stage, AllowedOperation::Finalize];

        // These would translate to different gRPC call patterns:
        // - Stage only: calls InstallStage or UpdateStage
        // - Finalize only: calls InstallFinalize or UpdateFinalize
        // - Both: calls Install or Update (full operation)

        assert_eq!(stage_only.len(), 1);
        assert_eq!(finalize_only.len(), 1);
        assert_eq!(both.len(), 2);
    }
}

/// Example integration showing the full flow
#[cfg(test)]
mod integration_examples {
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Example showing a typical install workflow
    #[test]
    fn example_install_workflow() {
        // 1. Create a configuration file
        let mut config_file = NamedTempFile::new().expect("Failed to create config file");
        writeln!(
            config_file,
            r#"
version: "1.0"
hostname: "test-host"
storage:
  disks:
    - device: "/dev/sda"
      partitions:
        - mount: "/"
          size: "20GB"
          filesystem: "ext4"
users:
  - name: "admin"
    groups: ["wheel"]
"#
        )
        .expect("Failed to write config");

        // 2. In a real scenario, you would:
        //    - Create a TridentClient
        //    - Call client.handle_install() with the config
        //    - Process the streaming responses
        //    - Handle any errors or reboot requirements

        // For this example, we just verify the config file exists
        assert!(config_file.path().exists());

        println!(
            "Example install workflow completed with config at: {:?}",
            config_file.path()
        );
    }

    /// Example showing error handling patterns
    #[test]
    fn example_error_handling() {
        // In the real gRPC client implementation, you would handle various error types:

        // 1. Connection errors (server unreachable)
        // 2. Authentication/authorization errors
        // 3. Validation errors (invalid configuration)
        // 4. Operation errors (disk full, hardware failure, etc.)
        // 5. Timeout errors (operation takes too long)

        // The client should gracefully handle these and provide meaningful error messages
        // to the user, potentially with suggestions for resolution.

        println!("Error handling patterns defined");
    }
}
