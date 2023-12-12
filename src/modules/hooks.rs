use std::{collections::HashMap, ffi::OsString, path::PathBuf};

use anyhow::{Context, Error, Ok};
use log::{debug, info};

use osutils::scripts::ScriptRunner;
use trident_api::{
    config::{HostConfiguration, Script},
    constants::DEFAULT_SCRIPT_INTERPRETER,
    status::{HostStatus, ReconcileState, UpdateKind},
};

use crate::modules::Module;

#[derive(Default, Debug)]
pub struct HooksModule;
impl Module for HooksModule {
    fn name(&self) -> &'static str {
        "hooks"
    }

    fn refresh_host_status(&mut self, _host_status: &mut HostStatus) -> Result<(), Error> {
        Ok(())
    }

    fn provision(
        &mut self,
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
        _mount_path: &std::path::Path,
    ) -> Result<(), Error> {
        info!("Running post-provision scripts");
        host_config
            .scripts
            .post_provision
            .iter()
            .try_for_each(|script| {
                run_script(script, host_status)?;
                Ok(())
            })?;

        Ok(())
    }

    fn configure(
        &mut self,
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        info!("Running post-configure scripts");
        host_config
            .scripts
            .post_configure
            .iter()
            .try_for_each(|script| {
                run_script(script, host_status)?;
                Ok(())
            })?;

        Ok(())
    }
}

fn run_script(script: &Script, host_status: &HostStatus) -> Result<(), Error> {
    // Check if the script should be run for the current reconcile state
    if !script.should_run(&host_status.reconcile_state) {
        debug!(
            "Skipping script {} for reconcile state {:?}",
            script.name, host_status.reconcile_state
        );
        return Ok(());
    }

    let interpreter: PathBuf = script
        .interpreter
        .as_ref()
        .cloned()
        .unwrap_or(PathBuf::from(DEFAULT_SCRIPT_INTERPRETER));

    debug!(
        "Running script {} with interpreter {}",
        script.name,
        interpreter.display()
    );
    let mut script_runner = ScriptRunner::new_interpreter(interpreter, &script.content);
    set_env_vars(
        &mut script_runner,
        &script.environment_variables,
        host_status,
    )
    .context("Failed to set environment variables for script")?;
    script_runner
        .with_logfile(script.log_file_path.as_ref())
        .run_check()
        .with_context(|| format!("Script {} failed", script.name))
}

fn set_env_vars(
    script_runner: &mut ScriptRunner,
    env_vars: &HashMap<String, String>,
    host_status: &HostStatus,
) -> Result<(), Error> {
    for (key, value) in env_vars {
        script_runner.env_vars.insert(key.into(), value.into());
    }
    // Add default environment variables from host status that can be used
    script_runner.env_vars.insert(
        "UPDATE_KIND".into(),
        match_update_kind_env_var(&host_status.reconcile_state),
    );
    script_runner.env_vars.insert(
        "TARGET_ROOT".into(),
        host_status
            .storage
            .root_device_path
            .clone()
            .context("Host Status has no root device path")
            .context("Error setting host status value for TARGET_ROOT")?
            .into(),
    );
    Ok(())
}

fn match_update_kind_env_var(reconcile_state: &ReconcileState) -> OsString {
    match reconcile_state {
        ReconcileState::Ready => "ready",
        ReconcileState::CleanInstall => "clean_install",
        ReconcileState::UpdateInProgress(UpdateKind::AbUpdate) => "ab_update",
        ReconcileState::UpdateInProgress(UpdateKind::HotPatch) => "hot_patch",
        ReconcileState::UpdateInProgress(UpdateKind::NormalUpdate) => "normal_update",
        ReconcileState::UpdateInProgress(UpdateKind::UpdateAndReboot) => "update_and_reboot",
        ReconcileState::UpdateInProgress(UpdateKind::Incompatible) => "incompatible",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use trident_api::{config::ServicingType, status::Storage};

    #[test]
    fn test_run_script_success() {
        let temp_dir = tempfile::tempdir().unwrap();
        let test_dir = temp_dir.path().join("test-directory");

        let host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            storage: Storage {
                root_device_path: Some("/dev/sda".into()),
                ..Default::default()
            },
            ..Default::default()
        };

        let mut environment_variables = HashMap::new();
        environment_variables.insert("TEST_DIR".into(), test_dir.to_str().unwrap().into());
        let script = Script {
            name: "test-script".into(),
            servicing_type: vec![ServicingType::CleanInstall],
            interpreter: Some("/bin/bash".into()),
            content: "mkdir $TEST_DIR".into(),
            environment_variables,
            log_file_path: None,
        };
        run_script(&script, &host_status).unwrap();
        assert!(test_dir.exists());
        // Cleanup
        temp_dir.close().unwrap();
    }

    #[test]
    fn test_run_script_that_always_fails() {
        let host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            storage: Storage {
                root_device_path: Some("/dev/sda".into()),
                ..Default::default()
            },
            ..Default::default()
        };

        let script = Script {
            name: "test-script".into(),
            servicing_type: vec![ServicingType::CleanInstall],
            interpreter: Some("/bin/bash".into()),
            content: "cat nonexisting.txt".into(),
            environment_variables: HashMap::new(),
            log_file_path: None,
        };
        assert!(run_script(&script, &host_status).is_err());
    }

    #[test]
    fn test_run_script_with_non_existing_interpreter() {
        let host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            storage: Storage {
                root_device_path: Some("/dev/sda".into()),
                ..Default::default()
            },
            ..Default::default()
        };

        let script = Script {
            name: "test-script".into(),
            servicing_type: vec![ServicingType::CleanInstall],
            interpreter: Some("/bin/nonexisting".into()),
            content: "mkdir test-directory".into(),
            environment_variables: HashMap::new(),
            log_file_path: None,
        };
        assert!(run_script(&script, &host_status).is_err());
    }

    #[test]
    fn test_run_script_that_always_skips() {
        let temp_dir = tempfile::tempdir().unwrap();
        let test_dir = temp_dir.path().join("test-directory");

        let host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            storage: Storage {
                root_device_path: Some("/dev/sda".into()),
                ..Default::default()
            },
            ..Default::default()
        };

        let mut environment_variables = HashMap::new();
        environment_variables.insert("TEST_DIR".into(), test_dir.to_str().unwrap().into());
        let script = Script {
            name: "test-script".into(),
            servicing_type: vec![ServicingType::NormalUpdate],
            interpreter: Some("/bin/bash".into()),
            content: "mkdir $TEST_DIR_NAME".into(),
            environment_variables,
            log_file_path: None,
        };
        // Check that the test-directory does not exist since the script should not be run
        assert!(run_script(&script, &host_status).is_ok());
        assert!(!test_dir.exists());
        // Cleanup
        temp_dir.close().unwrap();
    }
}
