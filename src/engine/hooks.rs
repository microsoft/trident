use std::{
    collections::HashMap,
    ffi::OsString,
    ops::Not,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error};
use log::{debug, info};

use osutils::{exe::OutputChecker, files, scripts::ScriptRunner};
use trident_api::{
    config::{
        HostConfiguration, HostConfigurationDynamicValidationError,
        HostConfigurationStaticValidationError, Script,
    },
    constants::{DEFAULT_SCRIPT_INTERPRETER, ROOT_MOUNT_POINT_PATH},
    error::{InvalidInputError, ReportError, ServicingError, TridentError},
    status::ServicingType,
};

use crate::engine::Subsystem;

use super::EngineContext;

#[derive(Debug)]
struct StagedFile {
    contents: Vec<u8>,
    mode: u32,
}

#[derive(Default, Debug)]
pub struct HooksSubsystem {
    staged_files: HashMap<PathBuf, StagedFile>,
}
impl Subsystem for HooksSubsystem {
    fn name(&self) -> &'static str {
        "hooks"
    }

    fn writable_etc_overlay(&self) -> bool {
        false
    }

    fn validate_host_config(
        &self,
        ctx: &EngineContext,
        host_config: &HostConfiguration,
    ) -> Result<(), TridentError> {
        // Ensure that all scripts that should be run and have a path actually exist
        host_config
            .scripts
            .post_configure
            .iter()
            .chain(&host_config.scripts.post_provision)
            .filter(|script| script.should_run(ctx.servicing_type))
            .filter_map(|script| {
                script.path.as_ref().and_then(|path| {
                    (path.exists() && path.is_file())
                        .not()
                        .then_some(Err(TridentError::new(InvalidInputError::from(
                            HostConfigurationDynamicValidationError::InvalidScriptPath {
                                name: script.name.clone(),
                                path: path.to_string_lossy().to_string(),
                            },
                        ))))
                })
            })
            .collect::<Result<(), _>>()?;

        Ok(())
    }

    fn prepare(&mut self, ctx: &EngineContext) -> Result<(), TridentError> {
        for script in ctx
            .spec
            .scripts
            .post_configure
            .iter()
            .chain(&ctx.spec.scripts.post_provision)
        {
            if let Some(ref path) = script.path {
                if script.should_run(ctx.servicing_type) {
                    self.stage_file(path.to_owned())
                        .structured(InvalidInputError::from(
                            HostConfigurationDynamicValidationError::LoadScript {
                                name: script.name.clone(),
                                path: path.to_string_lossy().to_string(),
                            },
                        ))?;
                }
            }
        }

        for file in &ctx.spec.os.additional_files {
            if let Some(ref path) = file.source {
                self.stage_file(path.to_owned())
                    .structured(InvalidInputError::from(
                        HostConfigurationDynamicValidationError::LoadAdditionalFile {
                            name: file.destination.display().to_string(),
                            path: path.to_string_lossy().to_string(),
                        },
                    ))?;
            }
        }

        Ok(())
    }

    #[tracing::instrument(name = "hooks_provision", skip_all)]
    fn provision(&mut self, ctx: &EngineContext, mount_path: &Path) -> Result<(), TridentError> {
        info!("Running post-provision scripts");
        ctx.spec
            .scripts
            .post_provision
            .iter()
            .try_for_each(|script| {
                self.run_script(
                    script,
                    ctx.servicing_type,
                    mount_path,
                    Path::new(ROOT_MOUNT_POINT_PATH),
                )
                .structured(ServicingError::RunPostProvisionScript {
                    script_name: script.name.clone(),
                })
            })?;

        Ok(())
    }

    #[tracing::instrument(name = "hooks_configuration", skip_all)]
    fn configure(&mut self, ctx: &EngineContext, exec_root: &Path) -> Result<(), TridentError> {
        info!("Adding additional files");
        for file in &ctx.spec.os.additional_files {
            let (content, original_mode) = if let Some(ref content) = file.content {
                (content.as_bytes().to_vec(), None)
            } else if let Some(ref path) = file.source {
                let staged_file =
                    self.staged_files
                        .get(path)
                        .structured(ServicingError::FindStagedFile {
                            staged_file: file.destination.to_string_lossy().to_string(),
                        })?;
                (staged_file.contents.clone(), Some(staged_file.mode))
            } else {
                return Err(TridentError::new(InvalidInputError::from(
                    HostConfigurationStaticValidationError::AdditionalFileNoContentOrSource {
                        additional_file: file.destination.to_string_lossy().to_string(),
                    },
                )))?;
            };

            let override_mode = file
                .permissions
                .as_ref()
                .map(|p| u32::from_str_radix(p, 8))
                .transpose()
                .structured(InvalidInputError::from(
                    HostConfigurationStaticValidationError::AdditionalFileInvalidPermissions {
                        additional_file: file.destination.to_string_lossy().to_string(),
                        permissions: file.permissions.clone().unwrap_or_default(),
                    },
                ))?;

            // If file permissions are specified in the host configuration, they override everything
            // else. Otherwise use the original file permissions or fall back to default permissions
            // of 0664.
            let mode = override_mode.or(original_mode).unwrap_or(0o664);

            files::write_file(&file.destination, mode, &content).structured(
                ServicingError::WriteAdditionalFile {
                    file_name: file.destination.to_string_lossy().to_string(),
                },
            )?;
        }

        info!("Running post-configure scripts");
        ctx.spec
            .scripts
            .post_configure
            .iter()
            .try_for_each(|script| {
                self.run_script(
                    script,
                    ctx.servicing_type,
                    Path::new(ROOT_MOUNT_POINT_PATH),
                    exec_root,
                )
                .structured(ServicingError::RunPostConfigureScript {
                    script_name: script.name.clone(),
                })
            })?;

        Ok(())
    }
}

impl HooksSubsystem {
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
        servicing_type: ServicingType,
        target_root: &Path,
        exec_root: &Path,
    ) -> Result<(), Error> {
        if !script.should_run(servicing_type) {
            debug!(
                "Skipping script '{}' for servicing type '{:?}'",
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
            "Running script '{}' with interpreter '{}'",
            script.name,
            interpreter.display()
        );

        let content = if let Some(ref content) = script.content {
            content.as_bytes()
        } else if let Some(ref path) = script.path {
            &self
                .staged_files
                .get(path)
                .context(format!("Failed to find staged file '{}'", path.display()))?
                .contents
        } else {
            bail!("Script '{}' has no content or path", script.name);
        };

        let mut script_runner = ScriptRunner::new_interpreter(interpreter, content);
        set_env_vars(
            &mut script_runner,
            &script.environment_variables,
            servicing_type,
            target_root,
            exec_root,
        )
        .context(format!(
            "Failed to set environment variables for script '{}'",
            script.name
        ))?;
        let output = script_runner
            .with_logfile(script.log_file_path.as_ref())
            .output_check()
            .with_context(|| format!("Script '{}' failed", script.name))?
            .output_report();

        if output.trim().is_empty() {
            info!(
                "Script '{}' executed. (no output was captured)",
                script.name
            );
        } else {
            info!("Script '{}' executed:\n{}", script.name, output);
        }

        Ok(())
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
    // Add default environment variables from engine context that can be used
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
        ServicingType::NoActiveServicing => "none",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::{collections::HashMap, path::Path};

    use indoc::indoc;
    use maplit::hashmap;

    use trident_api::{
        config::{Scripts, ServicingTypeSelection},
        constants::ROOT_MOUNT_POINT_PATH,
        error::ErrorKind,
        status::ServicingType,
    };

    #[test]
    fn test_stage_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let test_file = temp_dir.path().join("test-file");
        let test_content = "test-content";
        fs::write(&test_file, test_content).unwrap();

        let mut subsystem = HooksSubsystem::default();
        subsystem
            .stage_file(test_file.clone())
            .expect("Failed to stage file");
        assert_eq!(
            subsystem.staged_files.get(&test_file).unwrap().contents,
            test_content.as_bytes()
        );

        let mut subsystem = HooksSubsystem::default();
        let result = subsystem.stage_file(PathBuf::from("non-existing-file"));
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
        HooksSubsystem::default()
            .run_script(
                &script,
                ServicingType::CleanInstall,
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

        let ctx = EngineContext {
            spec: HostConfiguration {
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
            },
            servicing_type: ServicingType::CleanInstall,
            ..Default::default()
        };

        let mut subsystem = HooksSubsystem::default();
        subsystem.prepare(&ctx).unwrap();
        subsystem
            .provision(&ctx, Path::new(ROOT_MOUNT_POINT_PATH))
            .unwrap();

        assert!(test_dir.exists());
        // Cleanup
        temp_dir.close().unwrap();
    }

    #[test]
    fn test_run_script_from_nonexistent_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let test_dir = temp_dir.path().join("test-directory");
        let ctx = EngineContext {
            spec: HostConfiguration {
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
            },
            servicing_type: ServicingType::CleanInstall,
            ..Default::default()
        };

        let mut subsystem = HooksSubsystem::default();
        assert_eq!(
            subsystem.prepare(&ctx).unwrap_err().kind(),
            &ErrorKind::InvalidInput(InvalidInputError::InvalidHostConfigurationDynamic {
                inner: HostConfigurationDynamicValidationError::LoadScript {
                    name: "test-script".into(),
                    path: "nonexistent-file".into()
                }
            })
        );
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
        assert!(HooksSubsystem::default()
            .run_script(
                &script,
                ServicingType::CleanInstall,
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
        assert!(HooksSubsystem::default()
            .run_script(
                &script,
                ServicingType::CleanInstall,
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
        HooksSubsystem::default()
            .run_script(
                &script,
                ServicingType::CleanInstall,
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
        HooksSubsystem::default()
            .run_script(
                &script,
                ServicingType::CleanInstall,
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
        let mut subsystem = HooksSubsystem::default();
        let ctx = EngineContext {
            spec: HostConfiguration {
                os: trident_api::config::Os {
                    additional_files: vec![trident_api::config::AdditionalFile {
                        destination: test_file.clone(),
                        content: Some(test_content.into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        subsystem.prepare(&ctx).unwrap();
        subsystem
            .configure(&ctx, Path::new(ROOT_MOUNT_POINT_PATH))
            .unwrap();
        assert_eq!(fs::read_to_string(&test_file).unwrap(), test_content);
        assert_eq!(
            fs::metadata(&test_file).unwrap().permissions().mode() & 0o777,
            0o664
        );

        // Content + permissions
        let mut subsystem = HooksSubsystem::default();
        let ctx = EngineContext {
            spec: HostConfiguration {
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
            },
            ..Default::default()
        };
        subsystem.prepare(&ctx).unwrap();
        subsystem
            .configure(&ctx, Path::new(ROOT_MOUNT_POINT_PATH))
            .unwrap();
        assert_eq!(fs::read_to_string(&test_file).unwrap(), test_content);
        assert_eq!(
            fs::metadata(&test_file).unwrap().permissions().mode() & 0o777,
            0o744
        );

        // File
        let source_file = temp_dir.path().join("source-file");
        fs::write(&source_file, "\u{2603}").unwrap();
        let mut subsystem = HooksSubsystem::default();
        let ctx = EngineContext {
            spec: HostConfiguration {
                os: trident_api::config::Os {
                    additional_files: vec![trident_api::config::AdditionalFile {
                        destination: test_file.clone(),
                        source: Some(source_file.clone()),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        subsystem.prepare(&ctx).unwrap();
        subsystem
            .configure(&ctx, Path::new(ROOT_MOUNT_POINT_PATH))
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
