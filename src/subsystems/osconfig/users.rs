use std::{
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error};
use log::{debug, warn};
use tempfile::NamedTempFile;

use osutils::osmodifier::{self, MICPassword, MICUser, MICUsers, PasswordType};
use trident_api::config::{Password, SshMode, User};

const SSHD_CONFIG_FILE: &str = "/etc/ssh/sshd_config";
const SSHD_CONFIG_DIR: &str = "/etc/ssh/sshd_config.d";
const GLOBAL_CONFIG_FILE_NAME: &str = "global_user.conf";

pub(super) fn set_up_users(users: &[User], os_modifier_path: &Path) -> Result<(), Error> {
    if Path::new(SSHD_CONFIG_FILE).exists() {
        debug!("Setting up sshd config");

        // Create sshd config dir
        osutils::files::create_dirs(SSHD_CONFIG_DIR).context("Failed to create sshd config dir")?;

        let include_dir = format!("Include {}/{}", SSHD_CONFIG_DIR, GLOBAL_CONFIG_FILE_NAME);

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
        ssh_global_config(users).context("Failed to set up global sshd config")?;
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

    let mic_users_yaml = serde_yaml::to_string(&MICUsers {
        users: users
            .iter()
            .map(|user| create_mic_user(user.clone()))
            .collect(),
    })
    .context("Failed to serialize MIC configuration")?;

    let mut tmpfile = NamedTempFile::new().context("Failed to create a temporary file")?;
    tmpfile
        .write_all(mic_users_yaml.as_bytes())
        .context("Failed to write MIC users YAML to temporary file")?;
    tmpfile.flush().context("Failed to flush temporary file")?;

    // Invoke os modifier with the user config file
    osmodifier::run(os_modifier_path, tmpfile.path())
        .context("Failed to run OS modifier to set up users")?;

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
    let path = PathBuf::from(SSHD_CONFIG_DIR).join(GLOBAL_CONFIG_FILE_NAME);
    osutils::files::create_file(path)
        .context("Failed to create sshd config file for global config")?
        .write_all(buffer.join("\n").as_bytes())
        .context("Failed to write global user sshd config")
}

fn create_mic_user(user: User) -> MICUser {
    let (password_type, password_text) = match user.password {
        #[cfg(feature = "dangerous-options")]
        Password::DangerousPlainText(ref s) => (PasswordType::PlainText, Some(s.as_str())),

        #[cfg(feature = "dangerous-options")]
        Password::DangerousHashed(ref s) => (PasswordType::Hashed, Some(s.as_str())),

        Password::Locked => (PasswordType::Locked, None::<&str>),
    };

    let password_expires_days = {
        #[cfg(feature = "dangerous-options")]
        {
            user.dangerous_password_expires_days
        }

        #[cfg(not(feature = "dangerous-options"))]
        {
            None // Do not populate if dangerous-options is not enabled
        }
    };

    let mic_password = password_text.map(|password_text| MICPassword {
        password_type,
        value: password_text.to_string(),
    });

    MICUser {
        name: user.name,
        uid: user.uid,
        password: mic_password,
        password_expires_days,
        ssh_public_keys: user.ssh_public_keys,
        primary_group: user.primary_group,
        secondary_groups: user.secondary_groups,
        startup_command: user.startup_command,
    }
}
