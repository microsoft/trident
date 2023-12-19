use std::{collections::HashMap, ffi::OsString, path::PathBuf};

use anyhow::{bail, Context, Error, Ok};
use log::{debug, info};

use osutils::scripts::ScriptRunner;
use trident_api::{
    config::{HostConfiguration, Script},
    constants::DEFAULT_SCRIPT_INTERPRETER,
    status::{HostStatus, ReconcileState, UpdateKind},
};

use crate::modules::Module;

#[derive(Default, Debug)]
pub struct HooksModule {
    staged_files: HashMap<PathBuf, Vec<u8>>,
}
impl Module for HooksModule {
    fn name(&self) -> &'static str {
        "hooks"
    }

    fn refresh_host_status(&mut self, _host_status: &mut HostStatus) -> Result<(), Error> {
        Ok(())
    }

    fn validate_host_config(
        &self,
        _host_status: &HostStatus,
        host_config: &HostConfiguration,
        planned_update: ReconcileState,
    ) -> Result<(), Error> {
        for script in host_config
            .scripts
            .post_configure
            .iter()
            .chain(&host_config.scripts.post_provision)
        {
            if let Some(ref path) = script.path {
                if script.should_run(&planned_update) && !path.exists() {
                    bail!(
                        "Script '{}' not found at '{}' on host system",
                        script.name,
                        path.display()
                    );
                }
            }
        }
        Ok(())
    }

    fn prepare(
        &mut self,
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        for script in host_config
            .scripts
            .post_configure
            .iter()
            .chain(&host_config.scripts.post_provision)
        {
            if let Some(ref path) = script.path {
                if script.should_run(&host_status.reconcile_state) {
                    let content = std::fs::read(path).context(format!(
                        "Failed to load script '{}' from path '{}'",
                        script.name,
                        path.display()
                    ))?;
                    self.staged_files.insert(path.to_owned(), content);
                }
            }
        }

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
                self.run_script(script, host_status)?;
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
                self.run_script(script, host_status)?;
                Ok(())
            })?;

        Ok(())
    }
}

impl HooksModule {
    fn run_script(&self, script: &Script, host_status: &HostStatus) -> Result<(), Error> {
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

        let content = if let Some(ref content) = script.content {
            content.as_bytes()
        } else if let Some(ref path) = script.path {
            self.staged_files
                .get(path)
                .context(format!("Failed to find staged file {}", path.display()))?
        } else {
            bail!("Script {} has no content or path", script.name);
        };

        let mut script_runner = ScriptRunner::new_interpreter(interpreter, content);
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
    use std::{collections::HashMap, path::Path};

    use super::*;
    use indoc::indoc;
    use maplit::hashmap;
    use trident_api::config::{Scripts, ServicingType};
    use trident_api::status::Storage;

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
            content: Some("mkdir $TEST_DIR".into()),
            environment_variables,
            ..Default::default()
        };
        HooksModule::default()
            .run_script(&script, &host_status)
            .unwrap();
        assert!(test_dir.exists());
        // Cleanup
        temp_dir.close().unwrap();
    }

    #[test]
    fn test_run_script_from_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let test_dir = temp_dir.path().join("test-directory");
        let script_path = temp_dir.path().join("test-script.sh");
        std::fs::write(
            &script_path,
            indoc! {r#"
                #!/bin/bash
                mkdir $TEST_DIR
            "#},
        )
        .unwrap();

        let mut host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            storage: Storage {
                root_device_path: Some("/dev/sda".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let host_config = HostConfiguration {
            scripts: Scripts {
                post_provision: vec![Script {
                    name: "test-script".into(),
                    servicing_type: vec![ServicingType::CleanInstall],
                    interpreter: Some("/bin/bash".into()),
                    path: Some(script_path),
                    environment_variables: hashmap! {
                        "TEST_DIR".into() => test_dir.to_str().unwrap().into()
                    },
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        let mut module = HooksModule::default();
        module.prepare(&mut host_status, &host_config).unwrap();
        module
            .provision(&mut host_status, &host_config, Path::new("/"))
            .unwrap();

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
            content: Some("cat nonexisting.txt".into()),
            ..Default::default()
        };
        assert!(HooksModule::default()
            .run_script(&script, &host_status)
            .is_err());
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
            content: Some("mkdir test-directory".into()),
            ..Default::default()
        };
        assert!(HooksModule::default()
            .run_script(&script, &host_status)
            .is_err());
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
            content: Some("mkdir $TEST_DIR_NAME".into()),
            environment_variables,
            ..Default::default()
        };
        // Check that the test-directory does not exist since the script should not be run
        HooksModule::default()
            .run_script(&script, &host_status)
            .unwrap();
        assert!(!test_dir.exists());
        // Cleanup
        temp_dir.close().unwrap();
    }
}
