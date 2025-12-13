use std::{
    env,
    os::{
        fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd},
        unix::net::UnixListener as StdUnixListener,
    },
};

use anyhow::{bail, Context, Error};
use nix::{
    fcntl,
    sys::socket::{self, AddressFamily, SockaddrLike, SockaddrStorage},
};
use tokio::net::UnixListener;

/// The starting file descriptor number for systemd socket activation.
const SD_LISTEN_FDS_START: RawFd = 3;

pub fn get_listener_from_fd(fd: OwnedFd) -> Result<UnixListener, Error> {
    log::info!(
        "Creating UnixListener from file descriptor {}",
        fd.as_raw_fd()
    );

    let std_listener = StdUnixListener::from(fd);

    std_listener
        .set_nonblocking(true)
        .context("Failed to set non-blocking mode on StdUnixListener")?;

    let listener = UnixListener::from_std(std_listener)
        .context("Failed to create UnixListener from StdUnixListener")?;

    Ok(listener)
}

pub fn get_sd_fd_socket_data() -> Result<Vec<(OwnedFd, String)>, Error> {
    // Try to parse LISTEN_FDS and LISTEN_FDNAMES environment variables.
    let listen_fds = env::var("LISTEN_FDS").unwrap_or_else(|_| "0".to_string());
    let listen_fds: i32 = listen_fds
        .parse()
        .map_err(|_| anyhow::anyhow!("LISTEN_FDS is not a valid integer"))?;

    let listen_fds_names: Vec<String> = env::var("LISTEN_FDNAMES")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();

    log::trace!(
        "LISTEN_FDS: {}, LISTEN_FDNAMES: '{:?}'",
        listen_fds,
        listen_fds_names
    );

    if listen_fds < 0 {
        bail!("LISTEN_FDS is negative");
    }

    if listen_fds != listen_fds_names.len() as i32 {
        bail!("LISTEN_FDS does not match number of names in LISTEN_FDNAMES");
    }

    // Collect the valid Unix socket FDs.
    let mut result = Vec::new();
    for (i, name) in listen_fds_names.iter().enumerate() {
        // Initialize the raw FD number.
        let raw_fd = SD_LISTEN_FDS_START + i as RawFd;

        // This is safe because we know the raw_fd is greater or equal to 3
        // (negative is bad). We need to check if the fd is valid before taking
        // ownership, otherwise we might close an invalid fd.
        let borrowed = unsafe { BorrowedFd::borrow_raw(raw_fd) };

        // Check if the fd is valid by getting its flags
        fcntl::fcntl(borrowed, fcntl::F_GETFD).with_context(|| {
            format!(
                "File descriptor {}[{}] provided by systemd might be invalid: failed to get flags",
                name, raw_fd
            )
        })?;

        if !is_unix_socket(borrowed.as_raw_fd()) {
            bail!(
                "File descriptor {}[{}] provided by systemd is not a Unix socket",
                name,
                raw_fd
            );
        }

        // Get ownership by creating an OwnedFd from the raw_fd. This is safe
        // because we have verified the fd is valid.
        let owned = borrowed.try_clone_to_owned().with_context(|| {
            format!(
                "Failed to clone FD File descriptor {}[{}] provided by systemd",
                name, raw_fd
            )
        })?;

        log::trace!(
            "File descriptor {}[{}] provided by systemd is a valid Unix socket",
            name,
            raw_fd
        );

        result.push((owned, name.clone()));
    }

    Ok(result)
}

fn is_unix_socket(fd: RawFd) -> bool {
    matches!(get_addr_family(fd), Some(AddressFamily::Unix))
}

fn get_addr_family(fd: RawFd) -> Option<AddressFamily> {
    match socket::getsockname::<SockaddrStorage>(fd) {
        Ok(addr) => addr.family(),
        Err(_) => None,
    }
}
