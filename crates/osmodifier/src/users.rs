// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! User management — create/update users, passwords, SSH keys, groups.

use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Command,
};

use anyhow::{bail, Context, Error};
use log::{debug, info};

use crate::{
    config::{MICUser, PasswordType},
    OsModifierContext,
};

/// Add or update all configured users.
pub fn add_or_update_users(ctx: &OsModifierContext, users: &[MICUser]) -> Result<(), Error> {
    for user in users {
        add_or_update_user(ctx, user)
            .with_context(|| format!("Failed to configure user '{}'", user.name))?;
    }
    Ok(())
}

fn add_or_update_user(ctx: &OsModifierContext, user: &MICUser) -> Result<(), Error> {
    let root = ctx.root.to_str().unwrap_or("/");

    // Hash the password if needed
    let hashed_password = match &user.password {
        Some(pwd) => match pwd.password_type {
            PasswordType::PlainText => Some(hash_password(&pwd.value)?),
            PasswordType::Hashed => Some(pwd.value.clone()),
            PasswordType::Locked => None,
        },
        None => None,
    };

    let user_exists = check_user_exists(root, &user.name)?;

    if user_exists {
        debug!("User '{}' already exists, updating", user.name);
        if user.uid.is_some() {
            bail!(
                "Cannot change UID for existing user '{}'. \
                 Remove the UID field or delete the user first.",
                user.name
            );
        }
        if user.home_directory.is_some() {
            bail!(
                "Cannot change home directory for existing user '{}'. \
                 Remove the home directory field or delete the user first.",
                user.name
            );
        }

        // Update password if provided
        if let Some(ref hash) = hashed_password {
            update_user_password(ctx, &user.name, hash)?;
        }
    } else {
        info!("Creating user '{}'", user.name);
        create_user(root, user, hashed_password.as_deref())?;
    }

    // Set password expiry
    if let Some(days) = user.password_expires_days {
        set_password_expiry(ctx, &user.name, days)?;
    }

    // Update groups
    if let Some(ref primary) = user.primary_group {
        set_primary_group(root, &user.name, primary)?;
    }
    if !user.secondary_groups.is_empty() {
        set_secondary_groups(root, &user.name, &user.secondary_groups)?;
    }

    // SSH keys
    if !user.ssh_public_keys.is_empty() {
        write_ssh_keys(ctx, &user.name, &user.ssh_public_keys)?;
    }

    // Startup command
    if let Some(ref cmd) = user.startup_command {
        set_startup_command(ctx, &user.name, cmd)?;
    }

    Ok(())
}

fn check_user_exists(root: &str, username: &str) -> Result<bool, Error> {
    let status = if root == "/" {
        Command::new("id").arg("-u").arg(username).status()
    } else {
        Command::new("chroot")
            .arg(root)
            .args(["id", "-u", username])
            .status()
    }
    .with_context(|| format!("Failed to check if user '{username}' exists"))?;

    Ok(status.success())
}

fn hash_password(plaintext: &str) -> Result<String, Error> {
    // Use openssl to hash the password, matching the Go implementation
    let mut child = Command::new("openssl")
        .args(["passwd", "-6", "-stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("Failed to start openssl passwd")?;

    if let Some(ref mut stdin) = child.stdin {
        stdin
            .write_all(plaintext.as_bytes())
            .context("Failed to write password to openssl stdin")?;
    }

    let output = child
        .wait_with_output()
        .context("Failed to wait for openssl passwd")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("openssl passwd failed: {stderr}");
    }

    Ok(String::from_utf8(output.stdout)
        .context("openssl passwd produced non-UTF-8 output")?
        .trim()
        .to_string())
}

fn create_user(root: &str, user: &MICUser, hashed_password: Option<&str>) -> Result<(), Error> {
    let mut cmd = if root == "/" {
        Command::new("useradd")
    } else {
        let mut c = Command::new("chroot");
        c.arg(root).arg("useradd");
        c
    };

    cmd.arg("-m"); // Create home directory

    if let Some(ref hash) = hashed_password {
        cmd.arg("-p").arg(hash);
    }

    if let Some(uid) = user.uid {
        cmd.arg("-u").arg(uid.to_string());
    }

    if let Some(ref home) = user.home_directory {
        cmd.arg("-d").arg(home);
    }

    if let Some(ref primary_group) = user.primary_group {
        cmd.arg("-g").arg(primary_group);
    }

    cmd.arg(&user.name);

    let output = cmd
        .output()
        .with_context(|| format!("Failed to execute useradd for '{}'", user.name))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("useradd failed for '{}': {stderr}", user.name);
    }

    Ok(())
}

fn update_user_password(ctx: &OsModifierContext, username: &str, hash: &str) -> Result<(), Error> {
    debug!("Updating password for user '{username}'");
    let shadow_path = ctx.path("/etc/shadow");

    let content = fs::read_to_string(&shadow_path)
        .with_context(|| format!("Failed to read '{}'", shadow_path.display()))?;

    let mut found = false;
    let updated: Vec<String> = content
        .lines()
        .map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            if fields.len() >= 2 && fields[0] == username {
                found = true;
                let mut new_fields: Vec<String> = fields.iter().map(|f| f.to_string()).collect();
                new_fields[1] = hash.to_string();
                new_fields.join(":")
            } else {
                line.to_string()
            }
        })
        .collect();

    if !found {
        bail!("User '{username}' not found in shadow file");
    }

    let mut result = updated.join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }

    fs::write(&shadow_path, &result)
        .with_context(|| format!("Failed to write '{}'", shadow_path.display()))
}

fn set_password_expiry(ctx: &OsModifierContext, username: &str, days: u64) -> Result<(), Error> {
    debug!("Setting password expiry for '{username}' to {days} days");
    let shadow_path = ctx.path("/etc/shadow");

    let content = fs::read_to_string(&shadow_path)
        .with_context(|| format!("Failed to read '{}'", shadow_path.display()))?;

    let updated: Vec<String> = content
        .lines()
        .map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            if fields.len() >= 5 && fields[0] == username {
                let mut new_fields: Vec<String> = fields.iter().map(|f| f.to_string()).collect();
                // Field index 4 is the maximum password age
                while new_fields.len() < 5 {
                    new_fields.push(String::new());
                }
                new_fields[4] = days.to_string();
                new_fields.join(":")
            } else {
                line.to_string()
            }
        })
        .collect();

    let mut result = updated.join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }

    fs::write(&shadow_path, &result)
        .with_context(|| format!("Failed to write '{}'", shadow_path.display()))
}

fn set_primary_group(root: &str, username: &str, group: &str) -> Result<(), Error> {
    debug!("Setting primary group for '{username}' to '{group}'");
    let output = if root == "/" {
        Command::new("usermod")
            .args(["-g", group, username])
            .output()
    } else {
        Command::new("chroot")
            .arg(root)
            .args(["usermod", "-g", group, username])
            .output()
    }
    .with_context(|| format!("Failed to set primary group for '{username}'"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("usermod -g failed for '{username}': {stderr}");
    }
    Ok(())
}

fn set_secondary_groups(root: &str, username: &str, groups: &[String]) -> Result<(), Error> {
    let groups_str = groups.join(",");
    debug!("Setting secondary groups for '{username}' to '{groups_str}'");
    let output = if root == "/" {
        Command::new("usermod")
            .args(["-a", "-G", &groups_str, username])
            .output()
    } else {
        Command::new("chroot")
            .arg(root)
            .args(["usermod", "-a", "-G", &groups_str, username])
            .output()
    }
    .with_context(|| format!("Failed to set secondary groups for '{username}'"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("usermod -a -G failed for '{username}': {stderr}");
    }
    Ok(())
}

fn write_ssh_keys(ctx: &OsModifierContext, username: &str, keys: &[String]) -> Result<(), Error> {
    // Determine home directory
    let home = get_home_dir(ctx, username)?;
    let ssh_dir = home.join(".ssh");
    let auth_keys_path = ssh_dir.join("authorized_keys");

    debug!(
        "Writing {} SSH key(s) for '{username}' to '{}'",
        keys.len(),
        auth_keys_path.display()
    );

    // Create .ssh directory
    fs::create_dir_all(&ssh_dir)
        .with_context(|| format!("Failed to create '{}'", ssh_dir.display()))?;

    // Set directory permissions to 0700
    fs::set_permissions(&ssh_dir, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("Failed to set permissions on '{}'", ssh_dir.display()))?;

    // Write authorized_keys
    let content = keys.join("\n") + "\n";
    fs::write(&auth_keys_path, &content)
        .with_context(|| format!("Failed to write '{}'", auth_keys_path.display()))?;

    // Set file permissions to 0600
    fs::set_permissions(&auth_keys_path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("Failed to set permissions on '{}'", auth_keys_path.display()))?;

    // Set ownership to the user
    set_ownership(ctx, username, &ssh_dir)?;
    set_ownership(ctx, username, &auth_keys_path)?;

    Ok(())
}

fn get_home_dir(ctx: &OsModifierContext, username: &str) -> Result<std::path::PathBuf, Error> {
    let passwd_path = ctx.path("/etc/passwd");
    let content = fs::read_to_string(&passwd_path)
        .with_context(|| format!("Failed to read '{}'", passwd_path.display()))?;

    for line in content.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() >= 6 && fields[0] == username {
            return Ok(ctx.path(fields[5]));
        }
    }

    bail!("Could not find home directory for user '{username}' in /etc/passwd")
}

fn set_ownership(ctx: &OsModifierContext, username: &str, path: &Path) -> Result<(), Error> {
    let root = ctx.root.to_str().unwrap_or("/");
    let path_str = path
        .to_str()
        .context("Failed to convert path to string")?;

    let output = if root == "/" {
        Command::new("chown")
            .args([&format!("{username}:{username}"), path_str])
            .output()
    } else {
        // For non-root context, strip the root prefix for chroot
        let relative = path.strip_prefix(&ctx.root).unwrap_or(path);
        let rel_str = relative.to_str().context("path to string")?;
        Command::new("chroot")
            .arg(root)
            .args(["chown", &format!("{username}:{username}"), rel_str])
            .output()
    }
    .with_context(|| format!("Failed to chown '{}'", path.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("chown failed for '{}': {stderr}", path.display());
    }
    Ok(())
}

fn set_startup_command(ctx: &OsModifierContext, username: &str, cmd: &str) -> Result<(), Error> {
    debug!("Setting startup command for '{username}' to '{cmd}'");
    let passwd_path = ctx.path("/etc/passwd");

    let content = fs::read_to_string(&passwd_path)
        .with_context(|| format!("Failed to read '{}'", passwd_path.display()))?;

    let mut found = false;
    let updated: Vec<String> = content
        .lines()
        .map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            if fields.len() >= 7 && fields[0] == username {
                found = true;
                let mut new_fields: Vec<String> = fields.iter().map(|f| f.to_string()).collect();
                new_fields[6] = cmd.to_string();
                new_fields.join(":")
            } else {
                line.to_string()
            }
        })
        .collect();

    if !found {
        bail!("User '{username}' not found in /etc/passwd");
    }

    let mut result = updated.join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }

    fs::write(&passwd_path, &result)
        .with_context(|| format!("Failed to write '{}'", passwd_path.display()))
}
