use std::path::PathBuf;

use anyhow::{bail, Context, Error, Ok};
use log::{debug, info, warn};

use osutils::scripts::ScriptRunner;
use trident_api::{
    config::HostConfiguration,
    status::{HostStatus, ReconcileState},
};

use crate::modules::Module;

#[derive(Default, Debug)]
pub struct PostInstallScriptsModule;
impl Module for PostInstallScriptsModule {
    fn name(&self) -> &'static str {
        "install-scripts"
    }

    fn refresh_host_status(&mut self, _host_status: &mut HostStatus) -> Result<(), Error> {
        Ok(())
    }

    fn reconcile(
        &mut self,
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        // Skip if there are no post-install scripts
        if host_config.post_install_scripts.is_empty() {
            return Ok(());
        }

        // Limit operation to ReconcileState::CleanInstall
        if host_status.reconcile_state != ReconcileState::CleanInstall {
            warn!("Attempted to run post-installation scripts on a host that is not in the CleanInstall state. Skipping.");
            return Ok(());
        }

        // Run the scripts
        info!("Running post-installation scripts");
        host_config
            .post_install_scripts
            .iter()
            .try_for_each(|script| {
                let interpreter = match script.interpreter.as_ref() {
                    Some(i) => i.clone(),
                    None => PathBuf::from("/bin/sh"),
                };

                debug!(
                    "Running post-installation script with {}",
                    interpreter.display()
                );

                let result = ScriptRunner::new_interpreter(interpreter, &script.content)
                    .context("Failed to create script runner")?
                    .with_logfile(script.log_file_path.as_ref())
                    .context("Failed to set up logfile")?
                    .run()?;

                // Check the exit code
                // On error, we want to report the failure and bail
                if let Err(e) = result.check() {
                    bail!("Post-install {}. Captured output:\n{}", e, result.stderr());
                }

                Ok(())
            })?;

        // The script runner should clean up, but just in case...
        ScriptRunner::clear_script_dir().context("Failed to cleanup scripts directory")
    }
}
