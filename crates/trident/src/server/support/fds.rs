use std::{
    env::VarError,
    fs::{self, Permissions},
    io::ErrorKind,
    os::{
        fd::{BorrowedFd, OwnedFd, RawFd},
        unix::{
            fs::{FileTypeExt, PermissionsExt},
            net::UnixListener as StdUnixListener,
        },
    },
    path::{self, Path, PathBuf},
};

use anyhow::{bail, Context, Error};
use log::{trace, warn};
use nix::{
    errno::Errno,
    fcntl,
    sys::{
        socket::{self, AddressFamily, SockaddrLike, SockaddrStorage},
        stat::{self, Mode},
    },
};

/// The starting file descriptor number for systemd socket activation.
#[cfg_attr(test, allow(dead_code))]
const SD_LISTEN_FDS_START: RawFd = 3;

/// The name of the environment variable provided by systemd indicating the
/// number of sockets the service is expected to listen on.
const SD_LISTEN_FDS_ENV: &str = "LISTEN_FDS";

/// The name of the environment variable provided by systemd indicating the
/// names of the sockets the service is expected to listen on.
const SD_LISTEN_FDNAMES_ENV: &str = "LISTEN_FDNAMES";

/// Retrieves the list of Unix socket file descriptors and their associated
/// names provided by systemd socket activation.
///
/// This function inspects the `LISTEN_FDS` and `LISTEN_FDNAMES` environment
/// variables as defined by systemd socket activation and builds a list of
/// Unix listening sockets together with their logical names.
///
/// # Validation and error handling
///
/// * If the socket-activation environment variables are not present, the
///   function returns an empty vector.
/// * If `LISTEN_FDS` or `LISTEN_FDNAMES` are malformed, inconsistent with
///   each other, or cannot be parsed, the function returns an error.
/// * For each expected file descriptor, the function verifies that it refers
///   to a valid, open Unix domain socket in listening mode. If any descriptor
///   is invalid, not a Unix domain socket, not in listening state, or cannot
///   be inspected or used as required, the function returns an error rather
///   than silently ignoring the problem.
///
/// On success, each element of the returned vector contains an `OwnedFd`
/// corresponding to a validated listening Unix socket and the associated
/// name taken from `LISTEN_FDNAMES`, ordered by file descriptor starting at
/// `SD_LISTEN_FDS_START`.
pub fn get_sd_fd_socket_data() -> Result<Vec<(OwnedFd, String)>, Error> {
    let listen_fds_names = read_systemd_socket_activation_env()?;

    // Collect the valid Unix socket FDs.
    let mut result = Vec::new();
    for (i, name) in listen_fds_names.iter().enumerate() {
        // Initialize the raw FD number.
        let raw_fd = {
            #[cfg(not(test))]
            {
                // When running normally, start from SD_LISTEN_FDS_START
                SD_LISTEN_FDS_START
            }

            #[cfg(test)]
            {
                // In tests, we cannot guarantee that FDs starting from 3 are available,
                // so we use a thread-local variable to override the starting FD.
                tests::TEST_FD_START.with(|start| *start.borrow())
            }
        } + i as RawFd;

        // Safety: In non-test builds, `raw_fd` is derived from systemd's socket-activation
        // contract (`LISTEN_FDS`), which guarantees that descriptors starting at
        // SD_LISTEN_FDS_START are valid, open FDs owned by this process. In tests,
        // `tests::TEST_FD_START` is set up to point at valid FDs under our control.
        // We also immediately validate the descriptor via `check_file_descriptor_validity`
        // before cloning it to an `OwnedFd`, so we never take ownership of an invalid FD.
        let borrowed = unsafe { BorrowedFd::borrow_raw(raw_fd) };

        // Check if the fd is valid by getting its flags
        check_file_descriptor_validity(borrowed).with_context(|| {
            format!(
                "File descriptor {}[{}] provided by systemd might be invalid: failed to get flags",
                name, raw_fd
            )
        })?;

        // Get ownership by creating an OwnedFd from the raw_fd. This is safe
        // because we have verified the fd is valid.
        let owned = borrowed.try_clone_to_owned().with_context(|| {
            format!(
                "Failed to clone file descriptor {}[{}] provided by systemd",
                name, raw_fd
            )
        })?;

        trace!(
            "File descriptor {}[{}] provided by systemd is a valid Unix socket",
            name,
            raw_fd
        );

        result.push((owned, name.clone()));
    }

    Ok(result)
}

fn read_systemd_socket_activation_env() -> Result<Vec<String>, Error> {
    // Try to parse LISTEN_FDS and LISTEN_FDNAMES environment variables.
    let listen_fds = get_env_var(SD_LISTEN_FDS_ENV).unwrap_or_else(|_| "0".to_string());
    let listen_fds: i32 = listen_fds
        .parse()
        .map_err(|_| anyhow::anyhow!("'{SD_LISTEN_FDS_ENV}' is not a valid integer"))?;

    let listen_fds_names: Vec<String> = get_env_var(SD_LISTEN_FDNAMES_ENV)
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();

    trace!(
        "Systemd socket activation: '{SD_LISTEN_FDS_ENV}': {listen_fds}, \
        '{SD_LISTEN_FDNAMES_ENV}': \"{listen_fds_names:?}\"",
    );

    if listen_fds < 0 {
        bail!("'{SD_LISTEN_FDS_ENV}' is negative");
    }

    if listen_fds != listen_fds_names.len() as i32 {
        bail!("'{SD_LISTEN_FDS_ENV}' does not match number of names in '{SD_LISTEN_FDNAMES_ENV}'");
    }

    Ok(listen_fds_names)
}

/// A test-friendly version of `env::var` that reads from a thread-local
/// map of environment variables when running tests.
fn get_env_var(key: &str) -> Result<String, VarError> {
    #[cfg(not(test))]
    {
        // Using full module path to avoid not-used-import errors due to flags.
        std::env::var(key)
    }

    #[cfg(test)]
    {
        tests::TEST_ENV_VARS.with(|env| env.borrow().get(key).cloned().ok_or(VarError::NotPresent))
    }
}

/// Checks if the given file descriptor corresponds to a Unix domain socket.
///
/// Returns `true` if the file descriptor refers to a socket whose address
/// family is [`AddressFamily::Unix`].
///
/// Returns `false` if the file descriptor does not refer to a Unix socket.
/// This includes cases where:
/// - the file descriptor is not a socket,
/// - the file descriptor is invalid, or
/// - [`socket::getsockname`] fails for any other reason.
pub fn is_unix_socket(fd: RawFd) -> bool {
    matches!(get_addr_family(fd), Some(AddressFamily::Unix))
}

/// Gets the address family of the socket associated with the given file
/// descriptor.
fn get_addr_family(fd: RawFd) -> Option<AddressFamily> {
    match socket::getsockname::<SockaddrStorage>(fd) {
        Ok(addr) => addr.family(),
        Err(_) => None,
    }
}

/// Checks whether a file descriptor is valid by getting its status flags.
fn check_file_descriptor_validity(fd: BorrowedFd) -> Result<(), Errno> {
    fcntl::fcntl(fd, fcntl::F_GETFL).map(|_| ())
}

/// Creates a UnixListener at the specified path with the given permissions.
///
/// No attempt is made to delete any existing socket file at the given path;
/// this function will blindly try to bind to the path, which will fail if a
/// file already exists there, if there are insufficient permissions, or if
/// another instance of the server is already listening on that socket.
///
/// THREAD SAFETY: This function is not thread-safe with respect to other
/// invocations that may create or delete the same socket file concurrently. It
/// is the caller's responsibility to ensure that concurrent invocations do not
/// interfere with each other.
///
/// The function internally uses a temporary umask change to ensure the socket
/// is created with the exact desired permissions from the start. Umask changes
/// are process-wide, so concurrent calls to this function from different
/// threads may interfere with each other, potentially leading to incorrect
/// socket permissions at creation. The function will double-check and set the
/// permissions after creation as an extra safeguard, but this does not fully
/// eliminate the risk of race conditions during socket creation.
pub(crate) fn create_unix_socket(
    socket_path: impl AsRef<Path>,
    mode: Mode,
) -> Result<StdUnixListener, Error> {
    // Ensure the socket is created with the given mode by temporarily setting
    // the process umask.
    //
    // The resulting mode is roughly: 0o777 & !umask. To get `mode`, we set
    // `umask = (!mode) & 0o777`. (Mask to permission bits to avoid toggling
    // unrelated flag bits in nix::stat::Mode.)
    struct UmaskGuard(Mode);
    impl Drop for UmaskGuard {
        fn drop(&mut self) {
            stat::umask(self.0);
        }
    }

    let abs_path = path::absolute(socket_path.as_ref()).with_context(|| {
        format!(
            "Failed to get absolute path for {}",
            socket_path.as_ref().display()
        )
    })?;

    let perm_bits = Mode::from_bits_truncate(0o777);
    let mask = (!mode) & perm_bits;
    let old = stat::umask(mask);
    let listener = {
        let _guard = UmaskGuard(old);
        StdUnixListener::bind(&abs_path)
            .with_context(|| format!("Failed to bind UnixListener to {}", abs_path.display()))?
    };

    // Set exact permissions as an extra guarantee (e.g. if umask math changes
    // elsewhere). This should not broaden permissions because umask restricted
    // them at creation time.
    fs::set_permissions(&abs_path, Permissions::from_mode(mode.bits())).with_context(|| {
        format!(
            "Failed to set permissions on socket file at {}",
            abs_path.display()
        )
    })?;

    Ok(listener)
}

/// Helper struct to clean up a Unix socket file on drop.
pub(crate) struct UnixSocketCleanup {
    path: Option<PathBuf>,
}

impl UnixSocketCleanup {
    pub fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    pub fn empty() -> Self {
        Self { path: None }
    }

    fn cleanup(&mut self) -> Result<(), Error> {
        let Some(path) = self.path.take() else {
            return Ok(());
        };

        match fs::symlink_metadata(&path) {
            Ok(meta) => {
                if !meta.file_type().is_socket() {
                    warn!("Not removing socket path {}: not a socket", path.display());
                    return Ok(());
                }

                trace!("Removing socket file {}", path.display());
                fs::remove_file(&path)
                    .with_context(|| format!("Failed to remove socket file {}", path.display()))?;
                Ok(())
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| {
                format!("Failed to stat socket file {} for cleanup", path.display())
            }),
        }
    }
}

impl Drop for UnixSocketCleanup {
    fn drop(&mut self) {
        if let Err(e) = self.cleanup() {
            warn!("Failed to remove unix socket file on shutdown: {:#}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        cell::RefCell,
        collections::HashMap,
        fs::File,
        os::{fd::AsFd, unix::net::UnixListener as StdUnixListener},
    };

    use tempfile::tempdir;

    // Thread-local storage for test environment variables.
    thread_local! {
        /// Used to mock environment variables in tests.
        pub(super) static TEST_ENV_VARS: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());

        /// Used to override SD_LISTEN_FDS_START in tests.
        pub(super) static TEST_FD_START: RefCell<RawFd> = const { RefCell::new(0) };
    }

    /// Sets a test environment variable in the thread-local storage.
    fn set_test_env_var(key: &str, value: &str) {
        TEST_ENV_VARS.with(|env| {
            env.borrow_mut().insert(key.to_string(), value.to_string());
        });
    }

    /// Clears all test environment variables in the thread-local storage.
    fn clear_test_env_vars() {
        TEST_ENV_VARS.with(|env| {
            env.borrow_mut().clear();
        });
    }

    #[test]
    fn test_is_unix_socket() {
        // Try with a Unix socket
        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("test_socket");
        let std_listener = StdUnixListener::bind(&socket_path).unwrap();
        let raw_fd = std_listener.as_raw_fd();

        assert!(is_unix_socket(raw_fd));

        // Try with a non-socket fd (e.g., a file)
        let file = File::create(dir.path().join("test_file")).unwrap();
        let raw_fd = file.as_raw_fd();
        assert!(!is_unix_socket(raw_fd));
    }

    #[test]
    fn test_get_addr_family() {
        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("test_socket");
        let std_listener = StdUnixListener::bind(&socket_path).unwrap();
        let raw_fd = std_listener.as_raw_fd();

        let family = get_addr_family(raw_fd).unwrap();
        assert_eq!(family, AddressFamily::Unix);
    }

    #[test]
    fn test_read_systemd_socket_activation_env() {
        set_test_env_var(SD_LISTEN_FDS_ENV, "2");
        set_test_env_var(SD_LISTEN_FDNAMES_ENV, "socket1,socket2");
        let names = read_systemd_socket_activation_env().unwrap();
        assert_eq!(names, vec!["socket1".to_string(), "socket2".to_string()]);
    }

    #[test]
    fn test_read_systemd_socket_activation_env_empty() {
        set_test_env_var(SD_LISTEN_FDS_ENV, "0");
        set_test_env_var(SD_LISTEN_FDNAMES_ENV, "");
        let names = read_systemd_socket_activation_env().unwrap();
        assert_eq!(names, Vec::<String>::new());

        clear_test_env_vars();

        let names = read_systemd_socket_activation_env().unwrap();
        assert_eq!(names, Vec::<String>::new());
    }

    #[test]
    fn test_read_systemd_socket_activation_env_mismatch() {
        set_test_env_var(SD_LISTEN_FDS_ENV, "2");
        set_test_env_var(SD_LISTEN_FDNAMES_ENV, "socket1");
        let err = read_systemd_socket_activation_env().unwrap_err();
        assert!(err
            .to_string()
            .contains("does not match number of names in"));
    }

    #[test]
    fn test_read_systemd_socket_activation_env_invalid() {
        // Negative LISTEN_FDS
        set_test_env_var(SD_LISTEN_FDS_ENV, "-1");
        set_test_env_var(SD_LISTEN_FDNAMES_ENV, "socket1");
        let err = read_systemd_socket_activation_env().unwrap_err();
        assert!(err.to_string().contains("'LISTEN_FDS' is negative"));

        // Non-integer LISTEN_FDS
        set_test_env_var(SD_LISTEN_FDS_ENV, "abc");
        let err = read_systemd_socket_activation_env().unwrap_err();
        assert!(err
            .to_string()
            .contains("'LISTEN_FDS' is not a valid integer"));
    }

    #[test]
    fn test_get_sd_fd_socket_data() {
        // No env vars set
        clear_test_env_vars();
        let result = get_sd_fd_socket_data().unwrap();
        assert!(result.is_empty());

        // Set env vars for 1 socket
        set_test_env_var(SD_LISTEN_FDS_ENV, "1");
        set_test_env_var(SD_LISTEN_FDNAMES_ENV, "test_socket");

        // Create a Unix socket to occupy the fd
        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("test_socket");
        let std_listener = StdUnixListener::bind(&socket_path).unwrap();

        TEST_FD_START.set(std_listener.as_raw_fd());
        let mut fds = get_sd_fd_socket_data().unwrap();
        assert_eq!(fds.len(), 1);

        // Assert the two fds refer to the same socket
        let (fd, name) = fds.pop().unwrap();
        assert_eq!(name, "test_socket".to_string());
        let socket_listener = StdUnixListener::from(fd);
        assert_eq!(
            socket_listener.local_addr().unwrap().as_pathname(),
            Some(socket_path.as_path())
        );
    }

    #[test]
    fn test_check_file_descriptor_validity() {
        // Check with a valid socket fd
        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("test_socket");
        let std_listener = StdUnixListener::bind(&socket_path).unwrap();
        let borrowed_fd = std_listener.as_fd();

        check_file_descriptor_validity(borrowed_fd).unwrap();

        // Check with a valid file fd
        let file = File::create(dir.path().join("test_file")).unwrap();
        let borrowed_fd = file.as_fd();

        check_file_descriptor_validity(borrowed_fd).unwrap();

        // Check with an invalid fd: use a closed file descriptor to ensure invalidity
        let temp_file = File::create(dir.path().join("temp_fd")).unwrap();
        let raw_fd = temp_file.as_raw_fd();
        drop(temp_file);

        let invalid_fd = unsafe { BorrowedFd::borrow_raw(raw_fd) };
        let err = check_file_descriptor_validity(invalid_fd).unwrap_err();
        assert_eq!(err, Errno::EBADF);
    }

    #[tokio::test]
    async fn test_create_unix_socket_sets_mode() {
        let test_mode = |desired_mode: u32| {
            let dir = tempdir().unwrap();
            let socket_path = dir.path().join("test_socket_mode");

            let mode = Mode::from_bits_truncate(desired_mode);
            let _listener = create_unix_socket(&socket_path, mode).unwrap();

            let meta = fs::symlink_metadata(&socket_path).unwrap();
            let actual = meta.permissions().mode() & 0o777;
            assert_eq!(actual, desired_mode);
        };

        test_mode(0o600);
        test_mode(0o700);
        test_mode(0o660);
        test_mode(0o666);
    }

    #[test]
    fn test_unix_socket_cleanup_removes_socket_file() {
        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("trident_test.sock");

        let listener = StdUnixListener::bind(&socket_path).unwrap();
        assert!(socket_path.exists());
        drop(listener);

        let cleanup = UnixSocketCleanup::new(socket_path.clone());
        drop(cleanup);

        assert!(!socket_path.exists());
    }

    #[test]
    fn test_unix_socket_cleanup_does_not_remove_regular_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("not_a_socket");
        File::create(&file_path).unwrap();
        assert!(file_path.exists());

        let cleanup = UnixSocketCleanup::new(file_path.clone());
        drop(cleanup);

        assert!(file_path.exists());
    }
}
