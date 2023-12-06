use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error, Ok};
use duct::cmd;
use log::{debug, warn};

use osutils::exe::OutputChecker;
use trident_api::config::{Password, SshMode, User};

const SSHD_CONFIG_FILE: &str = "/etc/ssh/sshd_config";
const SSHD_CONFIG_DIR: &str = "/etc/ssh/sshd_config.d";

pub(super) fn set_up_users(mut users: HashMap<String, User>) -> Result<(), Error> {
    if Path::new(SSHD_CONFIG_FILE).exists() {
        debug!("Setting up sshd config...");

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
            .filter_map(|(name, data)| {
                if data.ssh_mode != SshMode::Block {
                    Some(name.as_str())
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
            .filter_map(|(name, data)| {
                if !data.ssh_keys.is_empty() {
                    Some(name.as_str())
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

    debug!("Setting up users...");

    // Take care of root separately
    if let Some(root_user) = users.remove("root") {
        configure_root_user(root_user).context("Failed to set up root user")?;
    }

    // Set up all other users
    users.into_iter().try_for_each(|(name, user)| {
        configure_user(&name, user).context(format!("Failed to set up user {}", name))
    })
}

fn ssh_global_config(users: &HashMap<String, User>) -> Result<(), Error> {
    let mut buffer = Vec::new();

    // Globally block passwords
    buffer.push("PasswordAuthentication no".to_owned());
    buffer.push("KbdInteractiveAuthentication no".to_owned());

    // Set root login mode
    if let Some(root_user) = users.get("root") {
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

    // List of users that are allowed to login through SSH
    let allowusers = users
        .iter()
        .filter_map(|(name, user)| {
            if user.ssh_mode == SshMode::Block {
                None
            } else {
                Some(name.as_str())
            }
        })
        .collect::<Vec<_>>();

    // If there are any users that can login, add a config block for them
    if !allowusers.is_empty() {
        buffer.push(format!("AllowUsers {}", allowusers.join(" ")));
    } else {
        // If there are no users that can login, block all users
        buffer.push("DenyUsers *".to_owned());
    }

    #[cfg(feature = "dangerous-options")]
    {
        // List of users that are allowed to login through SSH with password
        let pwd_users = users
            .iter()
            .filter_map(|(name, user)| match user.ssh_mode {
                SshMode::Block | SshMode::KeyOnly => None,
                SshMode::DangerousAllowPassword => Some(name.as_str()),
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

fn set_password(name: &str, password: Password) -> Result<(), Error> {
    match password {
        #[cfg(feature = "dangerous-options")]
        Password::DangerousPlainText(password) => {
            warn!("Using plain text password for user {}", name);
            cmd!("chpasswd")
                .stdin_bytes(format!("{}:{}", name, password).as_bytes())
                .run()?
                .check()
                .context(format!("Failed to set password for user {}", name))?;
            // Run mkinitrd to successfully set the password
            // Not doing this does not reflect the password successfully to login
            osutils::exe::RunAndCheck::run_and_check(&mut std::process::Command::new("mkinitrd"))?;
        }
        #[cfg(feature = "dangerous-options")]
        Password::DangerousHashed(password) => {
            warn!("Using encrypted password for user {}", name);
            cmd!("chpasswd", "--encrypted")
                .stdin_bytes(format!("{}:{}", name, password).as_bytes())
                .run()?
                .check()
                .context(format!("Failed to set password for user {}", name))?;
        }
        Password::Locked => {
            warn!("Locking user {}", name);
            cmd!("passwd", "--lock", name)
                .run()?
                .check()
                .context(format!("Failed to lock password for user {}", name))?;
        }
    }

    Ok(())
}

fn set_ssh_keys<P>(home: P, name: &str, keys: Vec<String>) -> Result<(), Error>
where
    P: AsRef<Path>,
{
    if keys.is_empty() {
        // Nothing to do
        return Ok(());
    }

    // Save the UID of the home dir
    let uid = osutils::files::get_owner_uid(&home).context(format!(
        "Failed to get UID of user with homedir {}",
        home.as_ref().display()
    ))?;

    // Save the GID of the home dir
    let gid = osutils::files::get_owner_gid(&home).context(format!(
        "Failed to get GID of user with homedir {}",
        home.as_ref().display()
    ))?;

    // Create the .ssh dir
    let ssh_dir = home.as_ref().join(".ssh");
    // Create the authorized_keys file and write the keys to it
    let auth_keys = ssh_dir.join("authorized_keys");
    osutils::files::create_file_mode(auth_keys, 0o600)
        .context(format!(
            "Failed to create authorized_keys file for user {}",
            name
        ))?
        .write_all(keys.join("\n").as_bytes())
        .context(format!("Failed to write authorized_keys for user {}", name))?;

    // Make sure the .ssh dir and authorized_keys file are owned by the user

    // TODO: use this api when we use Rust >= 1.73
    // std::os::unix::fs::chown(ssh_dir, Some(uid), Some(gid)).context("Failed to chown .ssh dir")?;
    // std::os::unix::fs::chown(auth_keys, Some(uid), Some(gid))
    //     .context("Failed to chown authorized_keys")?;

    // For now we have to use this :(
    cmd!("chown", "-R", format!("{}:{}", uid, gid), &ssh_dir)
        .run()?
        .check()
        .context("Failed to chown .ssh dir")?;

    Ok(())
}

fn configure_root_user(root_metadata: User) -> Result<(), Error> {
    set_password("root", root_metadata.password)?;
    set_ssh_keys("/root", "root", root_metadata.ssh_keys)
        .context("Failed to set ssh keys for user root")?;

    Ok(())
}

fn configure_user(name: &str, user: User) -> Result<(), Error> {
    warn!("Configuring user {name}");

    // Set homedir to be /home/<username>
    let homedir = PathBuf::from("/home").join(name);

    // Create the user
    let result = cmd!("useradd", "-m", "-s", "/bin/bash", "-d", &homedir, name).run()?;
    // Proceed if user is created successfully or if the user already exists
    if !result.status.success() && result.status.code() != Some(9) {
        result
            .check()
            .context(format!("Failed to create user {}", name))?;
    }

    set_password(name, user.password)?;

    // Add the user to the groups
    for group in user.groups {
        let args = vec!["-a", "-G", &group, name];
        duct::cmd("usermod", args)
            .run()?
            .check()
            .context(format!("Failed to add user {} to group {}", name, group))?;
    }

    set_ssh_keys(&homedir, name, user.ssh_keys)
        .context(format!("Failed to set ssh keys for user {}", name))?;

    Ok(())
}
