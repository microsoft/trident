use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use crate::OS_MODIFIER_BINARY_PATH;
use anyhow::{bail, Context, Error};
use log::{debug, warn};
use osutils::exe::RunAndCheck;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use trident_api::config::{Password, SshMode, User};

const SSHD_CONFIG_FILE: &str = "/etc/ssh/sshd_config";
const SSHD_CONFIG_DIR: &str = "/etc/ssh/sshd_config.d";

/// A helper struct to convert user into MIC's user format
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct MICUser {
    pub name: String,

    #[serde(rename = "UID", skip_serializing_if = "Option::is_none")]
    pub uid: Option<i32>,

    #[serde(default)]
    pub password_hashed: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,

    #[cfg(feature = "dangerous-options")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_expires_days: Option<u64>,

    #[serde(rename = "SSHPubKeys", skip_serializing_if = "Vec::is_empty")]
    pub ssh_pub_keys: Vec<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_group: Option<String>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub secondary_groups: Vec<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_command: Option<String>,
}

impl MICUser {
    pub fn new(name: String, user: User) -> Self {
        let (password, password_hashed) = match user.password {
            #[cfg(feature = "dangerous-options")]
            Password::DangerousPlainText(s) => (s, false),
            #[cfg(feature = "dangerous-options")]
            Password::DangerousHashed(s) => (s, true),
            Password::Locked => (String::new(), true),
        };

        MICUser {
            name,
            uid: user.uid,
            password: Some(password),
            password_hashed: Some(password_hashed),
            #[cfg(feature = "dangerous-options")]
            password_expires_days: user.dangerous_password_expires_days,
            ssh_pub_keys: user.ssh_public_keys,
            primary_group: user.primary_group,
            secondary_groups: user.secondary_groups,
            startup_command: user.startup_command,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct MICSystemConfig {
    users: Vec<MICUser>,
}

pub(super) fn set_up_users(users: Vec<User>) -> Result<(), Error> {
    if Path::new(SSHD_CONFIG_FILE).exists() {
        debug!("Setting up sshd config");

        // Create sshd config dir
        osutils::files::create_dirs(SSHD_CONFIG_DIR).context("Failed to create sshd config dir")?;

        let include_dir = format!("Include {}/*.conf", SSHD_CONFIG_DIR);

        // Check if the include directive is already in the sshd config.
        // If not, add it, otherwise do nothing.
        let config =
            std::fs::read_to_string(SSHD_CONFIG_FILE).context("Failed to read sshd config")?;
        if !regex::Regex::new(&format!(r"^ *{}", include_dir))
            .context("Failed to compile regex")?
            .is_match(&config)
        {
            // Add include directive to sshd config
            osutils::files::prepend_file(
                SSHD_CONFIG_FILE,
                false,
                format!("# Trident Configuration Overrides\n{}\n\n", include_dir).as_bytes(),
            )
            .context("Failed to prepend sshd config")?;
        }

        // Set up global sshd config
        ssh_global_config(&users).context("Failed to set up global sshd config")?;
    } else {
        // sshd_config is not installed in a known location, this probably means that sshd is not installed
        // We should check whether this is a problem or not.
        // Make a list of users that have non-default ssh modes
        let users_with_ssh = users
            .iter()
            .filter_map(|user| {
                if user.ssh_mode != SshMode::Block {
                    Some(user.name.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        if !users_with_ssh.is_empty() {
            bail!(
                "sshd_config not found in {SSHD_CONFIG_FILE}, but there are users with ssh access in configuration: {}",
                users_with_ssh.join(", ")
            );
        }

        // Now, if users have ssh keys, this _could_ be a problem, but we can't be sure.
        // Maybe that's intentional. Maybe they are using them for something else?
        let users_with_ssh_keys = users
            .iter()
            .filter_map(|user| {
                if !user.ssh_public_keys.is_empty() {
                    Some(user.name.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        if !users_with_ssh_keys.is_empty() {
            warn!(
                "sshd_config not found in {SSHD_CONFIG_FILE}, but there are users with ssh keys in configuration: {}",
                users_with_ssh_keys.join(", ")
            );
        }

        // Otherwise, we still warn the user in case this is a mistake
        warn!("sshd_config not found, skipping sshd config");
    }

    debug!("Setting up users");

    let mic_users_yaml = serde_yaml::to_string(&MICSystemConfig {
        users: users
            .into_iter()
            .map(|user| MICUser::new(user.name.clone(), user))
            .collect(),
    })
    .context("Failed to serialize MIC configuration")?;

    let mut tmpfile = NamedTempFile::new().context("Failed to create a temporary file")?;
    tmpfile
        .write_all(mic_users_yaml.as_bytes())
        .context("Failed to write MIC users YAML to temporary file")?;
    tmpfile.flush().context("Failed to flush temporary file")?;

    // Invoke os modifier with the user config file
    Command::new(OS_MODIFIER_BINARY_PATH)
        .arg("--config-file")
        .arg(tmpfile.path())
        .arg("--log-level=debug")
        .run_and_check()
        .context("Failed to run OS modifier")?;

    Ok(())
}

fn ssh_global_config(users: &[User]) -> Result<(), Error> {
    let mut buffer = Vec::new();

    // Set root login mode only when root is managed by Trident
    if let Some(root_user) = users.iter().find(|u| u.name == "root") {
        buffer.push(format!(
            "PermitRootLogin {}",
            match root_user.ssh_mode {
                SshMode::Block => "no",
                SshMode::KeyOnly => "prohibit-password",
                #[cfg(feature = "dangerous-options")]
                SshMode::DangerousAllowPassword => "yes",
            }
        ));
    }

    // List of trident-managed users that are NOT allowed to login through SSH
    let denyusers = users
        .iter()
        .filter_map(|user| (user.ssh_mode == SshMode::Block).then_some(user.name.as_str()))
        .collect::<Vec<_>>();

    // If there are any users that should not be allowed to login, add a config block for them
    if !denyusers.is_empty() {
        buffer.push(format!("DenyUsers {}", denyusers.join(" ")));
    }

    #[cfg(feature = "dangerous-options")]
    {
        // List of users that are allowed to login through SSH with password
        let pwd_users = users
            .iter()
            .filter_map(|user| match user.ssh_mode {
                SshMode::Block | SshMode::KeyOnly => None,
                SshMode::DangerousAllowPassword => Some(user.name.as_str()),
            })
            .collect::<Vec<_>>();

        // If there are any users that can login with password, add a config block for them
        if !pwd_users.is_empty() {
            buffer.push(format!(
                r#"Match User {}
    PasswordAuthentication yes
    KbdInteractiveAuthentication yes
        "#,
                pwd_users.join(","),
            ));
        }
    }

    // Add a newline at the end
    buffer.push("\n".to_owned());

    // Write the config to a file
    let path = PathBuf::from(SSHD_CONFIG_DIR).join("global_user.conf");
    osutils::files::create_file(path)
        .context("Failed to create sshd config file for global config")?
        .write_all(buffer.join("\n").as_bytes())
        .context("Failed to write global user sshd config")?;

    Ok(())
}
