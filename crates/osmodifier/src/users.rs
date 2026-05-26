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

// Shadow file field indices (0-based, colon-delimited).
const SHADOW_FIELD_PASSWORD: usize = 1;
const SHADOW_FIELD_LAST_CHANGE: usize = 2;
const SHADOW_FIELD_MAX_AGE: usize = 4;
const SHADOW_TOTAL_FIELDS: usize = 9;

// Passwd file field indices (0-based, colon-delimited).
const PASSWD_FIELD_HOME: usize = 5;
const PASSWD_FIELD_SHELL: usize = 6;

// SSH permissions.
const SSH_DIR_MODE: u32 = 0o700;
const AUTHORIZED_KEYS_MODE: u32 = 0o600;

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
            PasswordType::Hashed => {
                validate_shadow_value(&pwd.value).context("Invalid hashed password value")?;
                Some(pwd.value.clone())
            }
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
        } else if is_locked {
            // Explicitly lock rather than relying on useradd's default shadow
            // entry (which happens to be `!!` on AZL but is not guaranteed).
            lock_user_password(ctx, &user.name)?;
        }
    }

    // Set password expiry
    if let Some(days) = user.password_expires_days {
        set_password_expiry(ctx, &user.name, days)?;
    }

    // Update groups (only run usermod -g for existing users — for new users
    // the primary group was already set via useradd -g).
    if let Some(ref primary) = user.primary_group {
        if user_exists {
            set_primary_group(&user.name, primary)?;
        }
    }
    if !user.secondary_groups.is_empty() {
        set_secondary_groups(&user.name, &user.secondary_groups)?;
    }

    // SSH keys
    if !user.ssh_public_keys.is_empty() {
        write_ssh_keys(ctx, &user.name, &user.ssh_public_keys, user_exists)?;
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
        .with_context(|| format!("Failed to run 'id' for user '{username}'"))?;

    if output.success() {
        return Ok(true);
    }

    // Go's UserExists discriminates "no such user" from real errors.
    // Only treat the expected "no such user" stderr as not-found;
    // propagate everything else (permission denied, command-not-found, etc.).
    let stderr = output.error_output().to_lowercase();
    if stderr.contains("no such user") {
        return Ok(false);
    }

    bail!(
        "Unexpected error checking if user '{username}' exists: {}",
        output.error_output()
    )
}

/// Validate that a value is safe to write into /etc/shadow or pass to chpasswd.
/// Colons would corrupt the colon-delimited format; newlines would break line parsing.
fn validate_shadow_value(value: &str) -> Result<(), Error> {
    if value.contains(':') {
        bail!("Value contains ':' which would corrupt /etc/shadow format");
    }
    if value.contains('\n') || value.contains('\r') {
        bail!("Value contains newline which would corrupt /etc/shadow format");
    }
    Ok(())
}

fn hash_password(plaintext: &str) -> Result<String, Error> {
    // Use Dependency::Openssl to resolve the binary path for consistent
    // detection, but use std::process::Command for stdin piping which
    // the Dependency Command wrapper doesn't yet support.
    let openssl_path = Dependency::Openssl
        .path()
        .context("openssl is required for password hashing")?;

    let mut child = Command::new(openssl_path)
        .args(["passwd", "-6", "-stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("Failed to start openssl passwd")?;

    if let Some(mut stdin) = child.stdin.take() {
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
                new_fields[SHADOW_FIELD_PASSWORD] = hash.to_string();
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
    // Use Dependency::Chpasswd to resolve the binary path for consistent
    // detection, but use std::process::Command for stdin piping which
    // the Dependency Command wrapper doesn't yet support.
    let chpasswd_path = Dependency::Chpasswd
        .path()
        .context("chpasswd is required for setting user passwords")?;

    debug!("Setting password for new user '{username}' via chpasswd");
    let input = format!("{username}:{hash}\n");

    let mut child = Command::new(chpasswd_path)
        .arg("-e")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("Failed to start chpasswd")?;

    if let Some(mut stdin) = child.stdin.take() {
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

/// Lock a user's password by writing the locked marker into /etc/shadow.
///
/// Uses `*` (not `!`) because Azure Linux's sshd is built with `UsePAM=no`,
/// where `!` means "fully disabled including SSH key login" but `*` means
/// "password disabled, SSH key login still works." Matches Go's
/// `UpdateUserPassword` behavior.
fn lock_user_password(ctx: &OsModifierContext, username: &str) -> Result<(), Error> {
    debug!("Locking password for user '{username}'");
    update_user_password(ctx, username, "*")
}

fn set_password_expiry(ctx: &OsModifierContext, username: &str, days: u64) -> Result<(), Error> {
    debug!("Setting password expiry for '{username}' to {days} days");

    // Validate range matching Go's PasswordExpiresDaysIsValid (upper bound only;
    // trident's API uses u64 so -1 "never expires" is not reachable here).
    const UPPER_BOUND: u64 = 99999;
    if days > UPPER_BOUND {
        bail!("invalid value for password_expires_days ({days}), must be <= {UPPER_BOUND}");
    }

    let shadow_path = ctx.path("/etc/shadow");

    let content = fs::read_to_string(&shadow_path)
        .with_context(|| format!("Failed to read '{}'", shadow_path.display()))?;

    // Shadow field indices (0-based):
    // 0=login, 1=password, 2=lastChange, 3=minAge, 4=maxAge,
    // 5=warnPeriod, 6=inactivity, 7=expiration, 8=reserved

    let mut found = false;
    let mut parse_err: Option<String> = None;
    let updated: Vec<String> = content
        .lines()
        .map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            if !fields.is_empty() && fields[0] == username {
                if fields.len() != SHADOW_TOTAL_FIELDS {
                    parse_err = Some(format!(
                        "invalid shadow entry for user '{}': expected {} fields, found {}",
                        username,
                        SHADOW_TOTAL_FIELDS,
                        fields.len()
                    ));
                    return line.to_string();
                }
                found = true;
                let mut new_fields: Vec<String> = fields.iter().map(|f| f.to_string()).collect();

                // Ensure lastChange field is populated so password aging
                // has a reference point for when the clock started.
                if new_fields[SHADOW_FIELD_LAST_CHANGE].is_empty() {
                    match days_since_unix_epoch() {
                        Ok(d) => new_fields[SHADOW_FIELD_LAST_CHANGE] = d.to_string(),
                        Err(e) => {
                            parse_err = Some(format!("{e:#}"));
                            return line.to_string();
                        }
                    }
                }

                // Set maxAge (field 4) = number of days the password is valid.
                // This is equivalent to `chage -M <days>`. The previous Go
                // implementation incorrectly wrote to the account expiration
                // field (field 7), which would disable the entire account
                // rather than enforce password rotation.
                new_fields[SHADOW_FIELD_MAX_AGE] = days.to_string();
                new_fields.join(":")
            } else {
                line.to_string()
            }
        })
        .collect();

    if let Some(err) = parse_err {
        bail!("{err}");
    }

    if !found {
        bail!("User '{username}' not found in shadow file for password expiry");
    }

    let mut result = updated.join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }

    atomic_write_file(&shadow_path, &result)
}

/// Return the number of days since the Unix epoch (1970-01-01).
fn days_since_unix_epoch() -> Result<i64, Error> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("System clock is set before the Unix epoch")?
        .as_secs() as i64;
    Ok(secs / 86400)
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

fn write_ssh_keys(
    ctx: &OsModifierContext,
    username: &str,
    keys: &[String],
    include_existing: bool,
) -> Result<(), Error> {
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
    fs::set_permissions(&ssh_dir, fs::Permissions::from_mode(SSH_DIR_MODE))
        .with_context(|| format!("Failed to set permissions on '{}'", ssh_dir.display()))?;

    // For existing users, preserve existing authorized_keys (matching Go's
    // ProvisionUserSSHCerts which passes userExists as includeExistingKeys).
    let mut all_keys: Vec<String> = Vec::new();
    if include_existing {
        match fs::read_to_string(&auth_keys_path) {
            Ok(existing) => {
                for line in existing.lines() {
                    if !line.is_empty() {
                        all_keys.push(line.to_string());
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // No existing keys — that's fine
            }
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("Failed to read '{}'", auth_keys_path.display()));
            }
        }
    }

    all_keys.extend(keys.iter().cloned());

    // Write authorized_keys
    let content = all_keys.join("\n") + "\n";
    fs::write(&auth_keys_path, &content)
        .with_context(|| format!("Failed to write '{}'", auth_keys_path.display()))?;

    // Set file permissions to 0600
    fs::set_permissions(
        &auth_keys_path,
        fs::Permissions::from_mode(AUTHORIZED_KEYS_MODE),
    )
    .with_context(|| {
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
            return Ok(ctx.path(fields[PASSWD_FIELD_HOME]));
        }
    }

    bail!("Could not find home directory for user '{username}' in /etc/passwd")
}

fn set_ownership(_ctx: &OsModifierContext, username: &str, path: &Path) -> Result<(), Error> {
    let path_str = path.to_str().context("Failed to convert path to string")?;

    // Use "username:" (trailing colon, no group) so chown sets the group to
    // the user's login group rather than assuming a same-named group exists.
    Dependency::Chown
        .cmd()
        .args([&format!("{username}:"), path_str])
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
    if cmd.contains('\n') || cmd.contains('\r') {
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
                new_fields[PASSWD_FIELD_SHELL] = cmd.to_string();
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
///
/// Preserves permissions and uid/gid ownership from the original file.
/// Note: SELinux labels and extended attributes are not preserved because
/// osmodifier runs inside the target root before SELinux enforcement.
fn atomic_write_file(path: &std::path::Path, content: &str) -> Result<(), Error> {
    use std::io::Write as IoWrite;
    use std::os::unix::fs::MetadataExt;

    let parent = path.parent().context("Cannot determine parent directory")?;

    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("Failed to create temp file in '{}'", parent.display()))?;

    tmp.write_all(content.as_bytes())
        .with_context(|| format!("Failed to write temp file for '{}'", path.display()))?;

    tmp.flush()
        .with_context(|| format!("Failed to flush temp file for '{}'", path.display()))?;

    // fsync the temp file before rename to ensure data is on disk. Without
    // this, a power loss between rename and dirty-page flush could leave the
    // file zero-length (e.g., /etc/shadow → locked out of all accounts).
    tmp.as_file()
        .sync_all()
        .with_context(|| format!("Failed to fsync temp file for '{}'", path.display()))?;

    // Preserve ownership and permissions from the original file if it exists.
    // Ownership must be set before permissions because chown can clear
    // setuid/setgid bits.
    if let Ok(metadata) = fs::metadata(path) {
        use std::os::fd::AsFd;
        nix::unistd::fchown(
            tmp.as_file().as_fd(),
            Some(nix::unistd::Uid::from_raw(metadata.uid())),
            Some(nix::unistd::Gid::from_raw(metadata.gid())),
        )
        .with_context(|| {
            format!(
                "Failed to set ownership on temp file for '{}'",
                path.display()
            )
        })?;

        fs::set_permissions(tmp.path(), metadata.permissions()).with_context(|| {
            format!(
                "Failed to set permissions on temp file for '{}'",
                path.display()
            )
        })?;
    }

    tmp.persist(path)
        .with_context(|| format!("Failed to atomically replace '{}'", path.display()))?;

    // Sync parent directory to ensure the rename (directory entry update) is
    // durable. Without this, the old file could reappear after power loss.
    if let Some(parent) = path.parent() {
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }

    Ok(())
}
