// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! User management — create/update users, passwords, SSH keys, groups.

use std::{fs, io::Write, os::unix::fs::PermissionsExt, path::Path, process::Command};

use anyhow::{bail, Context, Error};
use log::{debug, info};
use osutils::dependencies::Dependency;

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
    // Hash the password if needed
    let hashed_password = match &user.password {
        Some(pwd) => match pwd.password_type {
            PasswordType::PlainText => Some(hash_password(&pwd.value)?),
            PasswordType::Hashed => Some(pwd.value.clone()),
            PasswordType::Locked => None,
        },
        None => None,
    };

    let is_locked = user
        .password
        .as_ref()
        .is_some_and(|p| p.password_type == PasswordType::Locked);

    let user_exists = check_user_exists(&user.name)?;

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
        } else if is_locked {
            // Lock the account by writing a locked marker to /etc/shadow
            lock_user_password(ctx, &user.name)?;
        }
    } else {
        info!("Creating user '{}'", user.name);
        create_user(user)?;

        // Set password after creation via chpasswd (avoids leaking hash in
        // /proc/cmdline that useradd -p would cause).
        if let Some(ref hash) = hashed_password {
            set_password_via_chpasswd(&user.name, hash)?;
        }
    }

    // Set password expiry
    if let Some(days) = user.password_expires_days {
        set_password_expiry(ctx, &user.name, days)?;
    }

    // Update groups
    if let Some(ref primary) = user.primary_group {
        set_primary_group(&user.name, primary)?;
    }
    if !user.secondary_groups.is_empty() {
        set_secondary_groups(&user.name, &user.secondary_groups)?;
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

fn check_user_exists(username: &str) -> Result<bool, Error> {
    let output = Dependency::Id
        .cmd()
        .args(["-u", username])
        .output()
        .with_context(|| format!("Failed to check if user '{username}' exists"))?;

    Ok(output.success())
}

fn hash_password(plaintext: &str) -> Result<String, Error> {
    // TODO: Convert to Dependency::Openssl once the Command wrapper supports
    // stdin piping. Currently uses std::process::Command directly because
    // openssl passwd reads the password from stdin.
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

fn create_user(user: &MICUser) -> Result<(), Error> {
    let mut cmd = Dependency::Useradd.cmd();

    cmd.arg("-m"); // Create home directory

    // Password is set separately via chpasswd to avoid leaking the hash
    // through /proc/cmdline (useradd -p is world-readable).

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

    cmd.run_and_check()
        .with_context(|| format!("Failed to create user '{}'", user.name))?;

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

    atomic_write_file(&shadow_path, &result)
}

/// Set password on a newly created user via chpasswd -e (stdin), avoiding
/// leaking the hash through /proc/cmdline.
fn set_password_via_chpasswd(username: &str, hash: &str) -> Result<(), Error> {
    // TODO: Convert to Dependency::Chpasswd once the Command wrapper supports
    // stdin piping. chpasswd reads username:hash from stdin.
    debug!("Setting password for new user '{username}' via chpasswd");
    let input = format!("{username}:{hash}\n");

    let mut child = Command::new("chpasswd")
        .arg("-e")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("Failed to start chpasswd")?;

    if let Some(ref mut stdin) = child.stdin {
        stdin
            .write_all(input.as_bytes())
            .context("Failed to write to chpasswd stdin")?;
    }

    let output = child
        .wait_with_output()
        .context("Failed to wait for chpasswd")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("chpasswd failed for '{username}': {stderr}");
    }
    Ok(())
}

/// Lock a user's password by writing the locked marker '!' into /etc/shadow.
fn lock_user_password(ctx: &OsModifierContext, username: &str) -> Result<(), Error> {
    debug!("Locking password for user '{username}'");
    update_user_password(ctx, username, "!")
}

fn set_password_expiry(ctx: &OsModifierContext, username: &str, days: u64) -> Result<(), Error> {
    debug!("Setting password expiry for '{username}' to {days} days");
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
                // Shadow fields: login:password:lastChange:minAge:maxAge:warn:inactive:expire:reserved
                // Field index 4 (0-based) is the maximum password age.
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

    if !found {
        bail!("User '{username}' not found in shadow file for password expiry");
    }

    let mut result = updated.join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }

    atomic_write_file(&shadow_path, &result)
}

fn set_primary_group(username: &str, group: &str) -> Result<(), Error> {
    debug!("Setting primary group for '{username}' to '{group}'");
    Dependency::Usermod
        .cmd()
        .args(["-g", group, username])
        .run_and_check()
        .with_context(|| format!("Failed to set primary group for '{username}'"))?;

    Ok(())
}

fn set_secondary_groups(username: &str, groups: &[String]) -> Result<(), Error> {
    let groups_str = groups.join(",");
    debug!("Setting secondary groups for '{username}' to '{groups_str}'");
    Dependency::Usermod
        .cmd()
        .args(["-a", "-G", &groups_str, username])
        .run_and_check()
        .with_context(|| format!("Failed to set secondary groups for '{username}'"))?;

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
    fs::set_permissions(&auth_keys_path, fs::Permissions::from_mode(0o600)).with_context(|| {
        format!(
            "Failed to set permissions on '{}'",
            auth_keys_path.display()
        )
    })?;

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
    let path_str = path.to_str().context("Failed to convert path to string")?;

    Dependency::Chown
        .cmd()
        .args([&format!("{username}:{username}"), path_str])
        .run_and_check()
        .with_context(|| format!("Failed to chown '{}'", path.display()))?;

    Ok(())
}

fn set_startup_command(ctx: &OsModifierContext, username: &str, cmd: &str) -> Result<(), Error> {
    debug!("Setting startup command for '{username}' to '{cmd}'");

    // Validate: colons would corrupt the colon-delimited /etc/passwd format
    if cmd.contains(':') {
        bail!("Startup command for user '{username}' contains ':' which would corrupt /etc/passwd");
    }
    if cmd.contains('\n') {
        bail!("Startup command for user '{username}' contains a newline");
    }

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

    atomic_write_file(&passwd_path, &result)
}

/// Atomically write a file by writing to a temp file and renaming.
/// This prevents corruption from crashes mid-write.
fn atomic_write_file(path: &std::path::Path, content: &str) -> Result<(), Error> {
    use std::io::Write as IoWrite;

    let parent = path.parent().context("Cannot determine parent directory")?;

    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("Failed to create temp file in '{}'", parent.display()))?;

    tmp.write_all(content.as_bytes())
        .with_context(|| format!("Failed to write temp file for '{}'", path.display()))?;

    tmp.flush()
        .with_context(|| format!("Failed to flush temp file for '{}'", path.display()))?;

    // Preserve permissions from the original file if it exists
    if let Ok(metadata) = fs::metadata(path) {
        fs::set_permissions(tmp.path(), metadata.permissions()).with_context(|| {
            format!(
                "Failed to set permissions on temp file for '{}'",
                path.display()
            )
        })?;
    }

    tmp.persist(path)
        .with_context(|| format!("Failed to atomically replace '{}'", path.display()))?;

    Ok(())
}




