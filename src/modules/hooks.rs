use std::{
    collections::HashMap,
    ffi::OsString,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error, Ok};
use log::{debug, info};

use osutils::{files, scripts::ScriptRunner};
use trident_api::{
    config::{HostConfiguration, Script},
    constants::{DEFAULT_SCRIPT_INTERPRETER, ROOT_MOUNT_POINT_PATH},
    status::{HostStatus, ServicingType},
};

use crate::modules::Module;

#[derive(Debug)]
struct StagedFile {
    contents: Vec<u8>,
    mode: u32,
}

#[derive(Default, Debug)]
pub struct HooksModule {
    staged_files: HashMap<PathBuf, StagedFile>,
}
impl Module for HooksModule {
    fn name(&self) -> &'static str {
        "hooks"
    }

    fn writable_etc_overlay(&self) -> bool {
        false
    }

    fn validate_host_config(
        &self,
        _host_status: &HostStatus,
        host_config: &HostConfiguration,
        planned_servicing_type: ServicingType,
    ) -> Result<(), Error> {
        for script in host_config
            .scripts
            .post_configure
            .iter()
            .chain(&host_config.scripts.post_provision)
        {
            if let Some(ref path) = script.path {
                if script.should_run(&planned_servicing_type) && !path.exists() {
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

    fn prepare(&mut self, host_status: &mut HostStatus) -> Result<(), Error> {
        for script in host_status
            .spec
            .scripts
            .post_configure
            .iter()
            .chain(&host_status.spec.scripts.post_provision)
        {
            if let Some(ref path) = script.path {
                if let Some(ref servicing_type) = host_status.servicing_type {
                    if script.should_run(servicing_type) {
                        self.stage_file(path.to_owned())
                            .context(format!("Failed to load script '{}'", script.name,))?;
                    }
                }
            }
        }

        for file in &host_status.spec.os.additional_files {
            if let Some(ref path) = file.path {
                self.stage_file(path.to_owned()).context(format!(
                    "Failed to load additional file to be placed at '{}'",
                    file.destination.display(),
                ))?;
            }
        }

        Ok(())
    }

    fn provision(
        &mut self,
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
        mount_path: &Path,
    ) -> Result<(), Error> {
        info!("Running post-provision scripts");
        host_config
            .scripts
            .post_provision
            .iter()
            .try_for_each(|script| {
                self.run_script(
                    script,
                    host_status.servicing_type,
                    mount_path,
                    Path::new(ROOT_MOUNT_POINT_PATH),
                )?;
                Ok(())
            })?;

        Ok(())
    }

    fn configure(
        &mut self,
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
        exec_root: &Path,
    ) -> Result<(), Error> {
        info!("Adding additional files");
        for file in &host_config.os.additional_files {
            let (content, original_mode) = if let Some(ref content) = file.content {
                (content.as_bytes().to_vec(), None)
            } else if let Some(ref path) = file.path {
                let staged_file = self
                    .staged_files
                    .get(path)
                    .context(format!("Failed to find staged file '{}'", path.display()))?;
                (staged_file.contents.clone(), Some(staged_file.mode))
            } else {
                bail!(
                    "Additional file '{}' has no content or path",
                    file.destination.display()
                );
            };

            let override_mode = file
                .permissions
                .as_ref()
                .map(|p| u32::from_str_radix(p, 8))
                .transpose()
                .context("Failed to parse permissions")?;

            // If file permissions are specified in the host configuration, they override everything
            // else. Otherwise use the original file permissions or fall back to default permissions
            // of 0664.
            let mode = override_mode.or(original_mode).unwrap_or(0o664);

            files::write_file(&file.destination, mode, &content).context(format!(
                "Failed to write additional file '{}'",
                file.destination.display()
            ))?;
        }

        info!("Running post-configure scripts");
        host_config
            .scripts
            .post_configure
            .iter()
            .try_for_each(|script| {
                self.run_script(
                    script,
                    host_status.servicing_type,
                    Path::new(ROOT_MOUNT_POINT_PATH),
                    exec_root,
                )?;
                Ok(())
            })?;

        Ok(())
    }
}

impl HooksModule {
    fn stage_file(&mut self, path: PathBuf) -> Result<(), Error> {
        let contents =
            std::fs::read(&path).context(format!("Failed to read file '{}'", path.display()))?;
        let mode = std::fs::metadata(&path)
            .context(format!(
                "Failed to read metadata for file '{}'",
                path.display()
            ))?
            .permissions()
            .mode();

        self.staged_files
            .insert(path, StagedFile { contents, mode });
        Ok(())
    }

    fn run_script(
        &self,
        script: &Script,
        servicing_type: Option<ServicingType>,
        target_root: &Path,
        exec_root: &Path,
    ) -> Result<(), Error> {
        // Check if the script should be run for the current servicing type
        let servicing_type = servicing_type.context("Servicing type not set")?;
        if !script.should_run(&servicing_type) {
            debug!(
                "Skipping script {} for servicing type {:?}",
                script.name, servicing_type
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
            &self
                .staged_files
                .get(path)
                .context(format!("Failed to find staged file {}", path.display()))?
                .contents
        } else {
            bail!("Script {} has no content or path", script.name);
        };

        let mut script_runner = ScriptRunner::new_interpreter(interpreter, content);
        set_env_vars(
            &mut script_runner,
            &script.environment_variables,
            servicing_type,
            target_root,
            exec_root,
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
    servicing_type: ServicingType,
    target_root: &Path,
    exec_root: &Path,
) -> Result<(), Error> {
    for (key, value) in env_vars {
        script_runner.env_vars.insert(key.into(), value.into());
    }
    // Add default environment variables from host status that can be used
    script_runner.env_vars.insert(
        "SERVICING_TYPE".into(),
        match_servicing_type_env_var(&servicing_type),
    );
    script_runner
        .env_vars
        .insert("TARGET_ROOT".into(), target_root.into());
    script_runner
        .env_vars
        .insert("EXEC_ROOT".into(), exec_root.into());
    Ok(())
}

fn match_servicing_type_env_var(servicing_type: &ServicingType) -> OsString {
    match servicing_type {
        ServicingType::HotPatch => "hot_patch",
        ServicingType::NormalUpdate => "normal_update",
        ServicingType::UpdateAndReboot => "update_and_reboot",
        ServicingType::AbUpdate => "ab_update",
        ServicingType::CleanInstall => "clean_install",
        ServicingType::Incompatible => "incompatible",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::{collections::HashMap, path::Path};

    use super::*;
    use indoc::indoc;
    use maplit::hashmap;
    use trident_api::config::{Scripts, ServicingTypeSelection};
    use trident_api::constants;
    use trident_api::constants::ROOT_MOUNT_POINT_PATH;
    use trident_api::status::{ServicingState, ServicingType, Storage};

    #[test]
    fn test_stage_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let test_file = temp_dir.path().join("test-file");
        let test_content = "test-content";
        fs::write(&test_file, test_content).unwrap();

        let mut module = HooksModule::default();
        module
            .stage_file(test_file.clone())
            .expect("Failed to stage file");
        assert_eq!(
            module.staged_files.get(&test_file).unwrap().contents,
            test_content.as_bytes()
        );

        let mut module = HooksModule::default();
        let result = module.stage_file(PathBuf::from("non-existing-file"));
        assert!(result.is_err());

        // Cleanup
        temp_dir.close().unwrap();
    }

    #[test]
    fn test_run_script_success() {
        let temp_dir = tempfile::tempdir().unwrap();
        let test_dir = temp_dir.path().join("test-directory");

        let mut environment_variables = HashMap::new();
        environment_variables.insert("TEST_DIR".into(), test_dir.to_str().unwrap().into());
        let script = Script {
            name: "test-script".into(),
            run_on: vec![ServicingTypeSelection::CleanInstall],
            interpreter: Some("/bin/bash".into()),
            content: Some("mkdir $TEST_DIR".into()),
            environment_variables,
            ..Default::default()
        };
        HooksModule::default()
            .run_script(
                &script,
                Some(ServicingType::CleanInstall),
                Path::new("/mnt/newroot"),
                Path::new("/"),
            )
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

        let host_config = HostConfiguration {
            scripts: Scripts {
                post_provision: vec![Script {
                    name: "test-script".into(),
                    run_on: vec![ServicingTypeSelection::CleanInstall],
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
        let mut host_status = HostStatus {
            spec: host_config.clone(),
            servicing_type: Some(ServicingType::CleanInstall),
            servicing_state: ServicingState::StagingDeployment,
            storage: Storage {
                root_device_path: Some("/dev/sda".into()),
                ..Default::default()
            },
            ..Default::default()
        };

        let mut module = HooksModule::default();
        module.prepare(&mut host_status).unwrap();
        module
            .provision(
                &mut host_status,
                &host_config,
                Path::new(constants::ROOT_MOUNT_POINT_PATH),
            )
            .unwrap();

        assert!(test_dir.exists());
        // Cleanup
        temp_dir.close().unwrap();
    }

    #[test]
    fn test_run_script_from_nonexistent_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let test_dir = temp_dir.path().join("test-directory");
        let host_config = HostConfiguration {
            scripts: Scripts {
                post_provision: vec![Script {
                    name: "test-script".into(),
                    run_on: vec![ServicingTypeSelection::CleanInstall],
                    interpreter: Some("/bin/bash".into()),
                    path: Some("nonexistent-file".into()),
                    environment_variables: hashmap! {
                        "TEST_DIR".into() => test_dir.to_str().unwrap().into()
                    },
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let mut host_status = HostStatus {
            spec: host_config.clone(),
            servicing_type: Some(ServicingType::CleanInstall),
            servicing_state: ServicingState::StagingDeployment,
            storage: Storage {
                root_device_path: Some("/dev/sda".into()),
                ..Default::default()
            },
            ..Default::default()
        };

        let mut module = HooksModule::default();
        let err = module.prepare(&mut host_status);
        assert!(err.is_err());

        // Cleanup
        temp_dir.close().unwrap();
    }

    #[test]
    fn test_run_script_that_always_fails() {
        let script = Script {
            name: "test-script".into(),
            run_on: vec![ServicingTypeSelection::CleanInstall],
            interpreter: Some("/bin/bash".into()),
            content: Some("cat nonexisting.txt".into()),
            ..Default::default()
        };
        assert!(HooksModule::default()
            .run_script(
                &script,
                Some(ServicingType::CleanInstall),
                Path::new("/mnt/newroot"),
                Path::new("/")
            )
            .is_err());
    }

    #[test]
    fn test_run_script_with_non_existing_interpreter() {
        let script = Script {
            name: "test-script".into(),
            run_on: vec![ServicingTypeSelection::CleanInstall],
            interpreter: Some("/bin/nonexisting".into()),
            content: Some("mkdir test-directory".into()),
            ..Default::default()
        };
        assert!(HooksModule::default()
            .run_script(
                &script,
                Some(ServicingType::CleanInstall),
                Path::new("/mnt/newroot"),
                Path::new("/")
            )
            .is_err());
    }

    #[test]
    fn test_run_script_that_always_skips() {
        let temp_dir = tempfile::tempdir().unwrap();
        let test_dir = temp_dir.path().join("test-directory");

        let mut environment_variables = HashMap::new();
        environment_variables.insert("TEST_DIR".into(), test_dir.to_str().unwrap().into());
        let script = Script {
            name: "test-script".into(),
            run_on: vec![ServicingTypeSelection::NormalUpdate],
            interpreter: Some("/bin/bash".into()),
            content: Some("mkdir $TEST_DIR_NAME".into()),
            environment_variables,
            ..Default::default()
        };
        // Check that the test-directory does not exist since the script should not be run
        HooksModule::default()
            .run_script(
                &script,
                Some(ServicingType::CleanInstall),
                Path::new("/mnt/newroot"),
                Path::new("/"),
            )
            .unwrap();
        assert!(!test_dir.exists());
        // Cleanup
        temp_dir.close().unwrap();
    }

    #[test]
    fn test_set_env_vars() {
        let mut script_runner =
            ScriptRunner::new_interpreter(PathBuf::from("/bin/bash"), "echo $TEST_VAR".as_bytes());
        let mut env_vars = HashMap::new();
        env_vars.insert("TEST_VAR".into(), "test-value".into());
        set_env_vars(
            &mut script_runner,
            &env_vars,
            ServicingType::CleanInstall,
            Path::new("/mnt/newroot"),
            Path::new("/"),
        )
        .unwrap();
        // Check that the environment variables are set in script_runner after the function call
        let expected_env_vars = hashmap! {
            "TEST_VAR".into() => "test-value".into(),
            "SERVICING_TYPE".into() => "clean_install".into(),
            "TARGET_ROOT".into() => "/mnt/newroot".into(),
            "EXEC_ROOT".into() => "/".into()
        };
        assert_eq!(script_runner.env_vars, expected_env_vars);
    }

    #[test]
    fn test_paths_set() {
        let target_root = tempfile::tempdir().unwrap();
        let exec_root = tempfile::tempdir().unwrap();

        let script = Script {
            name: "test-script".into(),
            run_on: vec![ServicingTypeSelection::CleanInstall],
            interpreter: Some("/bin/bash".into()),
            content: Some("touch $TARGET_ROOT/a && touch $EXEC_ROOT/b".into()),
            ..Default::default()
        };
        HooksModule::default()
            .run_script(
                &script,
                Some(ServicingType::CleanInstall),
                target_root.path(),
                exec_root.path(),
            )
            .unwrap();

        assert!(target_root.path().join("a").exists());
        assert!(exec_root.path().join("b").exists());

        // Cleanup
        target_root.close().unwrap();
        exec_root.close().unwrap();
    }

    #[test]
    fn test_add_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let test_file = temp_dir.path().join("test-file");
        let test_content = "test-content";

        // Content
        let mut module = HooksModule::default();
        let host_config = HostConfiguration {
            os: trident_api::config::Os {
                additional_files: vec![trident_api::config::AdditionalFile {
                    destination: test_file.clone(),
                    content: Some(test_content.into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let mut host_status = HostStatus {
            spec: host_config.clone(),
            ..Default::default()
        };
        module.prepare(&mut host_status).unwrap();
        module
            .configure(
                &mut host_status,
                &host_config,
                Path::new(ROOT_MOUNT_POINT_PATH),
            )
            .unwrap();
        assert_eq!(fs::read_to_string(&test_file).unwrap(), test_content);
        assert_eq!(
            fs::metadata(&test_file).unwrap().permissions().mode() & 0o777,
            0o664
        );

        // Content + permissions
        let mut module = HooksModule::default();
        let host_config = HostConfiguration {
            os: trident_api::config::Os {
                additional_files: vec![trident_api::config::AdditionalFile {
                    destination: test_file.clone(),
                    content: Some(test_content.into()),
                    permissions: Some("0744".into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let mut host_status = HostStatus {
            spec: host_config.clone(),
            ..Default::default()
        };
        module.prepare(&mut host_status).unwrap();
        module
            .configure(
                &mut host_status,
                &host_config,
                Path::new(ROOT_MOUNT_POINT_PATH),
            )
            .unwrap();
        assert_eq!(fs::read_to_string(&test_file).unwrap(), test_content);
        assert_eq!(
            fs::metadata(&test_file).unwrap().permissions().mode() & 0o777,
            0o744
        );

        // File
        let source_file = temp_dir.path().join("source-file");
        fs::write(&source_file, "\u{2603}").unwrap();
        let mut module = HooksModule::default();
        let host_config = HostConfiguration {
            os: trident_api::config::Os {
                additional_files: vec![trident_api::config::AdditionalFile {
                    destination: test_file.clone(),
                    path: Some(source_file.clone()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let mut host_status = HostStatus {
            spec: host_config.clone(),
            ..Default::default()
        };
        module.prepare(&mut host_status).unwrap();
        module
            .configure(
                &mut host_status,
                &host_config,
                Path::new(ROOT_MOUNT_POINT_PATH),
            )
            .unwrap();
        assert_eq!(fs::read_to_string(&test_file).unwrap(), "\u{2603}");
        assert_eq!(
            fs::metadata(&source_file).unwrap().permissions().mode() & 0o777,
            fs::metadata(&test_file).unwrap().permissions().mode() & 0o777,
        );

        // Cleanup
        temp_dir.close().unwrap();
    }
}
