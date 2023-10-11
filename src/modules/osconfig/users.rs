use std::{collections::HashMap, io::Write};

use anyhow::{Context, Error};
use duct::cmd;
use log::{debug, info, warn};

use osutils::exe::OutputChecker;
use trident_api::config::{Password, User};

pub(super) fn set_up_users(mut users: HashMap<String, User>) -> Result<(), Error> {
    debug!("Setting up users...");
    if let Some(root_user) = users.remove("root") {
        configure_root_user(root_user).context("Failed to set up root user")?;
    }

    users
        .into_iter()
        .try_for_each(|(name, user)| configure_user(name, user))
}

fn configure_root_user(root_metadata: User) -> Result<(), Error> {
    info!("Setting root password...");
    match root_metadata.password {
        Password::DangerousPlainText(password) => {
            warn!("Using plain text password for root user");
            duct::cmd!("chpasswd")
                .stdin_bytes(format!("root:{}", password).as_bytes())
                .run()?
                .check()
                .context("Failed to set root password")?;
        }
        Password::DangerousHashed(password) => {
            warn!("Using encrypted password for root user");
            cmd!("chpasswd", "--encrypted")
                .stdin_bytes(format!("root:{}", password).as_bytes())
                .run()?
                .check()
                .context("Failed to set root password")?;
        }
        Password::Locked => {
            warn!("Locking root user");
            cmd!("passwd", "--lock", "root")
                .run()?
                .check()
                .context("Failed to set root password")?;
        }
    }

    info!("Setting root password... done");

    if !root_metadata.ssh_keys.is_empty() {
        osutils::files::create_file("/root/.ssh/authorized_keys")
            .context("Failed to create /root/.ssh/authorized_keys")?
            .write_all(root_metadata.ssh_keys.join("\n").as_bytes())
            .context("Failed to write /root/.ssh/authorized_keys")?;
    }

    Ok(())
}

fn configure_user(name: String, _user: User) -> Result<(), Error> {
    // TODO(5993): implement user configuration
    warn!("Configuring user {name}");
    todo!("user configuration");
}
