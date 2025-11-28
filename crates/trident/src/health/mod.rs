use std::{
    path::Path,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use log::{debug, error, info};

use osutils::dependencies::Dependency;
use trident_api::{
    config::{Check, SystemdCheck},
    constants::ROOT_MOUNT_POINT_PATH,
    error::{HealthChecksError, ServicingError, TridentError},
};

use crate::{engine::EngineContext, subsystems::hooks};

#[derive(Debug)]
struct ScriptError {
    script_name: String,
    error_message: String,
}

/// This function will be called outside the standard subsystem flow
/// before Trident commits a target OS.
pub fn execute_health_checks(ctx: &EngineContext) -> Result<(), TridentError> {
    let hooks_subsystem = hooks::HooksSubsystem::new_for_local_scripts();
    let health_checks = ctx
        .spec
        .health
        .checks
        .clone()
        .into_iter()
        .filter(|check| check.should_run(ctx.servicing_type))
        .collect::<Vec<_>>();
    if !health_checks.is_empty() {
        debug!("Running health check(s)");
    }

    // Channel to collect script errors from threads
    let (tx, rx) = mpsc::channel();
    // Create parallel health check threads within a scope, the
    // threads will all be joined before the scope ends.
    thread::scope(|s| {
        for health_check in health_checks {
            let inner_subsystem = &hooks_subsystem;
            let inner_tx = tx.clone();
            s.spawn(move || {
                match health_check {
                    Check::SystemdCheck(systemd_check) => {
                        if let Err(err) = run_systemd_check(&systemd_check) {
                            if let Err(e) = inner_tx.send(ScriptError {
                                script_name: systemd_check.name,
                                error_message: format!("{err:?}"),
                            }) {
                                error!("Failed to send systemd check error: {e:?}");
                            }
                        }
                    }
                    Check::Script(inner_script) => {
                        if let Err(err) = inner_subsystem.run_script(
                            &inner_script,
                            ctx,
                            Path::new(ROOT_MOUNT_POINT_PATH),
                        ) {
                            if let Err(e) = inner_tx.send(ScriptError {
                                script_name: inner_script.name,
                                error_message: format!("{err:?}"),
                            }) {
                                error!("Failed to send script error: {e:?}");
                            }
                        }
                    }
                };
                drop(inner_tx);
            });
        }
        drop(tx);
    });

    // Collect messages from the channel
    let mut health_check_errors = Vec::new();
    while let Ok(script_error) = rx.recv() {
        health_check_errors.push(script_error);
    }

    // Create error collection from individual health check failures
    let health_check_errors_message: String = health_check_errors
        .iter()
        .map(|e| format!("{}: {:?}", e.script_name, e.error_message))
        .collect::<Vec<String>>()
        .join("\n");
    if !health_check_errors.is_empty() {
        error!(
            "Health checks completed with errors:\n{}",
            health_check_errors_message
        );
        return Err(TridentError::new(HealthChecksError::HealthChecksFailed {
            details: health_check_errors_message,
            servicing_type: format!("{:?}", ctx.servicing_type),
        }));
    }
    Ok(())
}

/// This function will be called outside the standard subsystem flow
/// by execute_health_checks.
///
/// It checks that the specified systemd service(s), when queried with
/// systemctl status, are in good running state. If not, the function
/// will retry until the specified timeout is reached. On timeout, the
/// last error will be returned.
fn run_systemd_check(check: &SystemdCheck) -> Result<(), TridentError> {
    let start_time = Instant::now();
    let timeout_duration = Duration::from_secs(check.timeout_seconds as u64);

    let services_list = check.systemd_services.join(" ");
    debug!("Checking status of systemd service(s) '{}'", &services_list);

    loop {
        let status = Dependency::Systemctl
            .cmd()
            .env("SYSTEMD_IGNORE_CHROOT", "true")
            .arg("status")
            .args(&check.systemd_services)
            .output();
        let error = match status {
            Ok(output) => match output.check() {
                Ok(_) => {
                    info!(
                        "Service(s) '{services_list}' are active/running: {}",
                        output.output_report()
                    );
                    return Ok(());
                }
                Err(e) => {
                    info!("Service(s) '{services_list}' are not active/running: {e}");
                    Some(e)
                }
            },
            Err(e) => {
                info!("Unable to query service(s) '{services_list}': {e}");
                Some(e)
            }
        };
        thread::sleep(Duration::from_millis(100));
        if start_time.elapsed() >= timeout_duration {
            return Err(TridentError::new(ServicingError::SystemdCheckTimeout {
                services: services_list,
                timeout_seconds: check.timeout_seconds,
                last_error: error
                    .map(|e| format!("{e:?}"))
                    .unwrap_or_else(|| "No status retrieved".into()),
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_systemd_check() {
        let mut check = SystemdCheck {
            name: "test-check".into(),
            systemd_services: vec!["nonexistent-service".into()],
            timeout_seconds: 0,
            run_on: vec![],
        };

        let result = run_systemd_check(&check);
        assert!(result.is_err());

        let zero_timeout_error_string = format!("{:?}", result.unwrap_err());
        let zero_timeout_errors = zero_timeout_error_string
            .matches("Unit nonexistent-service.service could not be found")
            .collect::<Vec<_>>();
        assert!(
            !zero_timeout_errors.is_empty(),
            "Expected error message to contain 'Unit could not be found' error"
        );

        check.timeout_seconds = 1;
        let result = run_systemd_check(&check);
        assert!(result.is_err());

        let nonzero_timeout_error_string = format!("{:?}", result.unwrap_err());
        let nonzero_timeout_errors = nonzero_timeout_error_string
            .matches("Unit nonexistent-service.service could not be found")
            .collect::<Vec<_>>();
        assert!(
            !nonzero_timeout_errors.is_empty(),
            "Expected error message to contain 'Unit could not be found' error"
        );
    }
}
