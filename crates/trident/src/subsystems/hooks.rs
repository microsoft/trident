use std::{
    collections::HashMap,
    ffi::OsStr,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Error};
use log::{debug, info, trace};

use osutils::{dependencies::Dependency, exe::OutputChecker, files, scripts::ScriptRunner};
use trident_api::{
    config::{
        Check, HostConfigurationDynamicValidationError, HostConfigurationStaticValidationError,
        Script, ScriptSource, SystemdCheck,
    },
    constants::{
        internal_params::WRITABLE_ETC_OVERLAY_HOOKS, DEFAULT_SCRIPT_INTERPRETER,
        ROOT_MOUNT_POINT_PATH,
    },
    error::{InvalidInputError, ReportError, ServicingError, TridentError},
    status::ServicingType,
};

use crate::engine::{EngineContext, Subsystem};

#[derive(Debug)]
struct ScriptError {
    script_name: String,
    error_message: String,
}

#[derive(Clone, Debug)]
struct StagedFile {
    contents: Vec<u8>,
    mode: u32,
}

#[derive(Clone, Default, Debug)]
pub struct HooksSubsystem {
    staged_files: HashMap<PathBuf, StagedFile>,
    writable_etc_overlay: bool,
}
impl Subsystem for HooksSubsystem {
    fn name(&self) -> &'static str {
        "hooks"
    }

    fn writable_etc_overlay(&self) -> bool {
        self.writable_etc_overlay
    }

    fn validate_host_config(&self, ctx: &EngineContext) -> Result<(), TridentError> {
        // Ensure that all scripts that should be run and have a path actually exist
        for script in ctx
            .spec
            .scripts
            .post_configure
            .iter()
            .chain(&ctx.spec.scripts.post_provision)
            .filter(|script| script.should_run(ctx.servicing_type))
        {
            if let ScriptSource::Path(ref path) = script.source {
                if !path.exists() || !path.is_file() {
                    return Err(TridentError::new(InvalidInputError::from(
                        HostConfigurationDynamicValidationError::InvalidScriptPath {
                            name: script.name.clone(),
                            path: path.to_string_lossy().to_string(),
                        },
                    )));
                }
            }
        }
        Ok(())
    }

    fn prepare(&mut self, ctx: &EngineContext) -> Result<(), TridentError> {
        // Set the flag based on the internal param. This allows to mount a writable /etc overlay
        // for the hooks subsystem, if a script needs to modify /etc.
        self.writable_etc_overlay = ctx
            .spec
            .internal_params
            .get_flag(WRITABLE_ETC_OVERLAY_HOOKS);

        for script in ctx
            .spec
            .scripts
            .post_configure
            .iter()
            .chain(&ctx.spec.scripts.post_provision)
        {
            if let ScriptSource::Path(path) = &script.source {
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
        if !ctx.spec.scripts.post_provision.is_empty() {
            debug!("Running post-provision scripts");
        }
        ctx.spec
            .scripts
            .post_provision
            .iter()
            .try_for_each(|script| {
                self.run_script(script, ctx, mount_path).structured(
                    ServicingError::RunPostProvisionScript {
                        script_name: script.name.clone(),
                    },
                )
            })?;

        Ok(())
    }

    #[tracing::instrument(name = "hooks_configuration", skip_all)]
    fn configure(&mut self, ctx: &EngineContext) -> Result<(), TridentError> {
        if !ctx.spec.os.additional_files.is_empty() {
            debug!("Adding additional files");
        }
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

            // If file permissions are specified in the Host Configuration, they override everything
            // else. Otherwise use the original file permissions or fall back to default permissions
            // of 0664.
            let mode = override_mode.or(original_mode).unwrap_or(0o664);

            files::write_file(&file.destination, mode, &content).structured(
                ServicingError::WriteAdditionalFile {
                    file_name: file.destination.to_string_lossy().to_string(),
                },
            )?;
        }

        if !ctx.spec.scripts.post_configure.is_empty() {
            debug!("Running post-configure scripts");
        }
        ctx.spec
            .scripts
            .post_configure
            .iter()
            .try_for_each(|script| {
                self.run_script(script, ctx, Path::new(ROOT_MOUNT_POINT_PATH))
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

    fn run_systemd_check(&self, check: &SystemdCheck) -> Result<(), TridentError> {
        let start_time = Instant::now();
        let timeout_duration = Duration::from_secs(check.timeout_seconds as u64);
        let mut last_error = None;

        let services_list = check.systemd_services.join(" ");
        debug!("Checking status of systemd service(s) '{}'", &services_list);

        for _i in 0.. {
            if start_time.elapsed() >= timeout_duration {
                return Err(TridentError::new(ServicingError::SystemdCheckTimeout {
                    services: services_list,
                    timeout_seconds: check.timeout_seconds,
                    last_error: last_error
                        .map(|e| format!("{e:?}"))
                        .unwrap_or_else(|| "No status retrieved".into()),
                }));
            }

            let status = Dependency::Systemctl
                .cmd()
                .env("SYSTEMD_IGNORE_CHROOT", "true")
                .arg("status")
                .args(&check.systemd_services)
                .output();
            match status {
                Ok(output) => match output.check() {
                    Ok(_) => {
                        info!(
                            "Service(s) '{services_list}' are active/running: {}",
                            output.output_report()
                        );
                        break;
                    }
                    Err(e) => {
                        info!("Service(s) '{services_list}' are not active/running: {e}");
                        last_error = Some(e);
                    }
                },
                Err(e) => {
                    info!("Unable to query service(s) '{services_list}': {e}");
                    last_error = Some(e);
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }

    fn run_script(
        &self,
        script: &Script,
        ctx: &EngineContext,
        target_root: &Path,
    ) -> Result<(), Error> {
        if !script.should_run(ctx.servicing_type) {
            trace!(
                "Skipping script '{}' for servicing type '{:?}'",
                script.name,
                ctx.servicing_type
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

        let content = match &script.source {
            ScriptSource::Content(content) => content.as_bytes(),
            ScriptSource::Path(path) => {
                &self
                    .staged_files
                    .get(path)
                    .context(format!("Failed to find staged file '{}'", path.display()))?
                    .contents
            }
        };

        let mut script_runner = ScriptRunner::new_interpreter(interpreter, content);

        // Set arguments
        script_runner
            .args
            .extend(script.arguments.iter().map(OsStr::new));

        // Set environment variables
        for (key, value) in &script.environment_variables {
            script_runner
                .env_vars
                .insert(OsStr::new(key), OsStr::new(value));
        }
        // Add default environment variables from engine context that can be used for the script
        script_runner.env_vars.insert(
            OsStr::new("SERVICING_TYPE"),
            match_servicing_type_env_var(&ctx.servicing_type),
        );
        script_runner
            .env_vars
            .insert(OsStr::new("TARGET_ROOT"), target_root.as_os_str());
        if let Some(ref phonehome_url) = ctx.spec.trident.phonehome {
            script_runner
                .env_vars
                .insert(OsStr::new("PHONEHOME_URL"), OsStr::new(phonehome_url));
        }

        let output = script_runner
            .output_check()
            .with_context(|| format!("Script '{}' failed", script.name))?
            .output_report();

        info!("Script '{}' executed successfully", script.name);
        if output.trim().is_empty() {
            debug!("Script '{}' produced no output", script.name);
        } else {
            debug!("Script '{}':\n{}", script.name, output);
        }

        Ok(())
    }

    /// This function will be called outside the standard subsystem flow
    /// before Trident starts a servicing operation.
    pub fn execute_pre_servicing_scripts(
        &mut self,
        ctx: &EngineContext,
    ) -> Result<(), TridentError> {
        if !ctx.spec.scripts.pre_servicing.is_empty() {
            debug!("Running pre-servicing scripts");
        }
        ctx.spec
            .scripts
            .pre_servicing
            .iter()
            .try_for_each(|script| {
                self.run_script(script, ctx, Path::new(ROOT_MOUNT_POINT_PATH))
                    .structured(ServicingError::RunPreServicingScript {
                        script_name: script.name.clone(),
                    })
            })?;
        Ok(())
    }

    /// This function will be called outside the standard subsystem flow
    /// before Trident commits a target OS.
    pub fn execute_health_checks(&mut self, ctx: &EngineContext) -> Result<(), TridentError> {
        let health_checks = ctx
            .spec
            .health
            .checks
            .clone()
            .into_iter()
            .filter(|check| check.should_run(ctx.servicing_type))
            .collect::<Vec<_>>();
        if !health_checks.is_empty() {
            debug!("Running health check scripts");
        }

        // Shared vector to collect script errors from threads
        let health_check_errors = Arc::new(Mutex::new(Vec::new()));
        // Create parallel health-check threads within a scope, the
        // threads will all be joined before the scope ends.
        thread::scope(|s| {
            for health_check in health_checks {
                let subsystem = &self;
                let loop_script_errors = health_check_errors.clone();
                s.spawn(move || match health_check {
                    Check::SystemdCheck(systemd_check) => {
                        if let Err(err) = subsystem.run_systemd_check(&systemd_check) {
                            loop_script_errors.lock().unwrap().push(ScriptError {
                                script_name: systemd_check.name,
                                error_message: format!("{err:?}"),
                            });
                        }
                    }
                    Check::Script(inner_script) => {
                        if let Err(err) = subsystem.run_script(
                            &inner_script,
                            ctx,
                            Path::new(ROOT_MOUNT_POINT_PATH),
                        ) {
                            loop_script_errors.lock().unwrap().push(ScriptError {
                                script_name: inner_script.name,
                                error_message: format!("{err:?}"),
                            });
                        }
                    }
                });
            }
        });

        // Create error collection from individual health check failures
        let health_check_errors_message: String = health_check_errors
            .lock()
            .unwrap()
            .iter()
            .map(|e| format!("{}: {:?}", e.script_name, e.error_message))
            .collect::<Vec<String>>()
            .join("\n");
        if !health_check_errors.lock().unwrap().is_empty() {
            debug!(
                "Health checks completed with errors:\n{}",
                health_check_errors_message
            );
            return Err(TridentError::new(ServicingError::HealthChecksFailed {
                details: health_check_errors_message,
            }));
        }
        Ok(())
    }
}

fn match_servicing_type_env_var(servicing_type: &ServicingType) -> &OsStr {
    match servicing_type {
        ServicingType::HotPatch => OsStr::new("hot_patch"),
        ServicingType::NormalUpdate => OsStr::new("normal_update"),
        ServicingType::UpdateAndReboot => OsStr::new("update_and_reboot"),
        ServicingType::AbUpdate => OsStr::new("ab_update"),
        ServicingType::CleanInstall => OsStr::new("clean_install"),
        ServicingType::NoActiveServicing => OsStr::new("none"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::{collections::HashMap, path::Path};

    use indoc::indoc;
    use maplit::hashmap;

    use trident_api::config::HostConfiguration;
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
            source: ScriptSource::Content("mkdir $TEST_DIR".into()),
            environment_variables,
            ..Default::default()
        };
        HooksSubsystem::default()
            .run_script(
                &script,
                &EngineContext {
                    servicing_type: ServicingType::CleanInstall,
                    ..Default::default()
                },
                Path::new("/mnt/newroot"),
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
                        source: ScriptSource::Path(script_path),
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
                        source: ScriptSource::Path("nonexistent-file".into()),
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
            source: ScriptSource::Content("cat nonexisting.txt".into()),
            ..Default::default()
        };
        assert!(HooksSubsystem::default()
            .run_script(
                &script,
                &EngineContext {
                    servicing_type: ServicingType::CleanInstall,
                    ..Default::default()
                },
                Path::new("/mnt/newroot"),
            )
            .is_err());
    }

    #[test]
    fn test_run_script_with_non_existing_interpreter() {
        let script = Script {
            name: "test-script".into(),
            run_on: vec![ServicingTypeSelection::CleanInstall],
            interpreter: Some("/bin/nonexisting".into()),
            source: ScriptSource::Content("mkdir test-directory".into()),
            ..Default::default()
        };
        assert!(HooksSubsystem::default()
            .run_script(
                &script,
                &EngineContext {
                    servicing_type: ServicingType::CleanInstall,
                    ..Default::default()
                },
                Path::new("/mnt/newroot"),
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
            source: ScriptSource::Content("mkdir $TEST_DIR_NAME".into()),
            environment_variables,
            ..Default::default()
        };
        // Check that the test-directory does not exist since the script should not be run
        HooksSubsystem::default()
            .run_script(
                &script,
                &EngineContext {
                    servicing_type: ServicingType::CleanInstall,
                    ..Default::default()
                },
                Path::new("/mnt/newroot"),
            )
            .unwrap();
        assert!(!test_dir.exists());
        // Cleanup
        temp_dir.close().unwrap();
    }

    #[test]
    fn test_use_args() {
        let temp_dir = tempfile::tempdir().unwrap();
        let test_dir = temp_dir.path().join("test-directory");

        let script = Script {
            name: "test-script".into(),
            run_on: vec![ServicingTypeSelection::CleanInstall],
            interpreter: Some("/bin/bash".into()),
            source: ScriptSource::Content("mkdir $1".into()),
            arguments: vec![test_dir.to_str().unwrap().into()],
            ..Default::default()
        };
        HooksSubsystem::default()
            .run_script(
                &script,
                &EngineContext {
                    servicing_type: ServicingType::CleanInstall,
                    ..Default::default()
                },
                Path::new("/mnt/newroot"),
            )
            .unwrap();
        assert!(test_dir.exists(), "{}", test_dir.display());
        // Cleanup
        temp_dir.close().unwrap();
    }

    fn write_to_file(script_content: &'static str, args: Vec<String>, interpreter: PathBuf) {
        let temp_dir = tempfile::tempdir().unwrap();
        let script_path = temp_dir.path().join("test-script.sh");
        std::fs::write(&script_path, script_content).unwrap();

        let ctx = EngineContext {
            spec: HostConfiguration {
                scripts: Scripts {
                    post_provision: vec![Script {
                        name: "test-script".into(),
                        run_on: vec![ServicingTypeSelection::CleanInstall],
                        interpreter: Some(interpreter),
                        source: ScriptSource::Path(script_path),
                        arguments: args,
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
        // Cleanup
        temp_dir.close().unwrap();
    }

    #[test]
    fn test_use_args_multiline() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test-file.txt");
        write_to_file(
            indoc! {r#"
                touch $1
                cat $2 << EOF > $1
                hello $3
                EOF
            "#},
            vec![
                file_path.to_str().unwrap().into(),
                "-E".into(),
                "world".into(),
            ],
            "/bin/bash".into(),
        );

        assert!(file_path.exists());
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "hello world$\n");
        // Cleanup
        temp_dir.close().unwrap();
    }

    #[test]
    fn test_use_args_python() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test-file.txt");
        write_to_file(
            indoc! {r#"
                import sys
                file = open(sys.argv[1], "w")
                file.write(f"hello {sys.argv[2]}")
                file.close()
            "#},
            vec![file_path.to_str().unwrap().into(), "world".into()],
            "python3".into(),
        );
        assert!(file_path.exists());
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "hello world");
        // Cleanup
        temp_dir.close().unwrap();
    }

    #[test]
    fn test_paths_set() {
        let target_root = tempfile::tempdir().unwrap();

        let script = Script {
            name: "test-script".into(),
            run_on: vec![ServicingTypeSelection::CleanInstall],
            interpreter: Some("/bin/bash".into()),
            source: ScriptSource::Content("touch $TARGET_ROOT/a".into()),
            ..Default::default()
        };
        HooksSubsystem::default()
            .run_script(
                &script,
                &EngineContext {
                    servicing_type: ServicingType::CleanInstall,
                    ..Default::default()
                },
                target_root.path(),
            )
            .unwrap();

        assert!(target_root.path().join("a").exists());

        // Cleanup
        target_root.close().unwrap();
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
        subsystem.configure(&ctx).unwrap();
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
        subsystem.configure(&ctx).unwrap();
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
        subsystem.configure(&ctx).unwrap();
        assert_eq!(fs::read_to_string(&test_file).unwrap(), "\u{2603}");
        assert_eq!(
            fs::metadata(&source_file).unwrap().permissions().mode() & 0o777,
            fs::metadata(&test_file).unwrap().permissions().mode() & 0o777,
        );

        // Cleanup
        temp_dir.close().unwrap();
    }

    #[test]
    fn test_execute_pre_servicing_scripts_success() {
        let temp_dir = tempfile::tempdir().unwrap();
        let test_dir = temp_dir.path().join("test-directory");

        let mut environment_variables = HashMap::new();
        environment_variables.insert("TEST_DIR".into(), test_dir.to_str().unwrap().into());
        let ctx = EngineContext {
            spec: HostConfiguration {
                scripts: Scripts {
                    pre_servicing: vec![Script {
                        name: "test-script".into(),
                        run_on: vec![ServicingTypeSelection::CleanInstall],
                        interpreter: Some("/bin/bash".into()),
                        source: ScriptSource::Content("mkdir $TEST_DIR".into()),
                        environment_variables,
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
        subsystem.execute_pre_servicing_scripts(&ctx).unwrap();
        assert!(test_dir.exists());
        // Cleanup
        temp_dir.close().unwrap();
    }

    #[test]
    fn test_execute_pre_servicing_scripts_failure() {
        let ctx = EngineContext {
            spec: HostConfiguration {
                scripts: Scripts {
                    pre_servicing: vec![Script {
                        name: "test-script".into(),
                        run_on: vec![ServicingTypeSelection::CleanInstall],
                        interpreter: Some("/bin/bash".into()),
                        source: ScriptSource::Content("cat nonexisting.txt".into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            servicing_type: ServicingType::CleanInstall,
            ..Default::default()
        };
        let result = HooksSubsystem::default().execute_pre_servicing_scripts(&ctx);
        let error = result.unwrap_err();
        assert_eq!(
            error.kind(),
            &ErrorKind::Servicing(ServicingError::RunPreServicingScript {
                script_name: "test-script".into()
            })
        );
    }
}
