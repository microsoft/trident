use std::{
    env,
    path::{Path, PathBuf},
};

use trident_api::error::{
    ContainerConfigurationError, InitializationError, InternalError, TridentError,
};

use crate::path;

/// Path to the root of the host filesystem. Expected to be mounted there when
/// running in a container.
pub const HOST_ROOT_PATH: &str = "/host";

/// Environment variable that is set when running in a container. Value is not
/// important.
pub const DOCKER_ENVIRONMENT: &str = "DOCKER_ENVIRONMENT";

/// Uses the `DOCKER_ENVIRONMENT` environment variable to determine if the
/// current process is running in a container. This variable needs to be
/// explicitly set as part of Dockerfile. Checks for other indirect ways of
/// determining if running in a container to alert users to set the environment variable.
pub fn is_running_in_container() -> Result<bool, TridentError> {
    if env::var(DOCKER_ENVIRONMENT).is_ok() {
        return Ok(true);
    }

    if Path::new("/.dockerenv").exists() {
        return Err(TridentError::new(
            InitializationError::ContainerConfiguration {
                inner: ContainerConfigurationError::DockerEnvironmentVarCheck {
                    docker_env_var: DOCKER_ENVIRONMENT.to_string(),
                },
            },
        ));
    }

    Ok(false)
}

/// For use only when running in a container. If running in a container, returns
/// the path to the root of the host filesystem. Host filesystem is expected to
/// be mounted at the provided input and if that path does not exist, an error
/// is returned.
fn get_host_root_path_impl(host_root_path: &Path) -> Result<PathBuf, TridentError> {
    if !is_running_in_container()? {
        return Err(TridentError::new(InternalError::RunInContainer));
    }

    // We expect the host filesystem to be available under host_root_path
    if !host_root_path.exists() {
        return Err(TridentError::new(
            InitializationError::ContainerConfiguration {
                inner: ContainerConfigurationError::HostRootMountCheck {
                    host_root_path: host_root_path.to_string_lossy().to_string(),
                },
            },
        ));
    }
    Ok(PathBuf::from(host_root_path))
}

/// For use only when running in a container. If running in a container, returns
/// the path to the root of the host filesystem. Host filesystem is expected to
/// be mounted at `/host` and if that path does not exist, an error
/// is returned.
pub fn get_host_root_path() -> Result<PathBuf, TridentError> {
    get_host_root_path_impl(Path::new(HOST_ROOT_PATH))
}

/// If running in a container, prepends the host root path to a path. If not
/// running in a container, returns the path as is.
pub fn get_host_relative_path(path: PathBuf) -> Result<PathBuf, TridentError> {
    Ok(if is_running_in_container()? {
        path::join_relative(get_host_root_path()?, path)
    } else {
        path
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use trident_api::error::{ContainerConfigurationError, ErrorKind};

    #[test]
    fn test_get_host_root_path() {
        // Do cleanup
        env::remove_var(DOCKER_ENVIRONMENT);

        // Test case #1: Running in a container
        env::set_var(DOCKER_ENVIRONMENT, "true");
        assert_eq!(
            super::get_host_root_path_impl(Path::new(".")).unwrap(),
            Path::new(".")
        );

        // Test case #2: Running in a container but host root path does not exist
        assert_eq!(
            super::get_host_root_path_impl(Path::new("/does-not-exist"))
                .unwrap_err()
                .kind(),
            &ErrorKind::Initialization(InitializationError::ContainerConfiguration {
                inner: ContainerConfigurationError::HostRootMountCheck {
                    host_root_path: "/does-not-exist".to_string()
                }
            })
        );

        // Test case #3: Not running in a container
        env::remove_var(DOCKER_ENVIRONMENT);
        assert_eq!(
            get_host_root_path().unwrap_err().kind(),
            &ErrorKind::Internal(InternalError::RunInContainer)
        );
        assert_eq!(
            super::get_host_root_path_impl(Path::new("."))
                .unwrap_err()
                .kind(),
            &ErrorKind::Internal(InternalError::RunInContainer)
        );

        // Test case #4: Running in a container but HOST_ROOT_PATH does not exist
        env::set_var(DOCKER_ENVIRONMENT, "true");
        let test_dir = Path::new(HOST_ROOT_PATH);
        if test_dir.exists() {
            assert_eq!(super::get_host_root_path_impl(test_dir).unwrap(), test_dir);
            std::fs::remove_dir(test_dir).unwrap();
        }
        assert_eq!(
            get_host_root_path().unwrap_err().kind(),
            &ErrorKind::Initialization(InitializationError::ContainerConfiguration {
                inner: ContainerConfigurationError::HostRootMountCheck {
                    host_root_path: HOST_ROOT_PATH.to_string()
                }
            })
        );

        // Do cleanup
        env::remove_var(DOCKER_ENVIRONMENT);
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use std::fs::File;

    use pytest_gen::functional_test;
    use trident_api::error::{ContainerConfigurationError, ErrorKind};

    #[functional_test(feature = "helpers")]
    fn test_is_running_in_container() {
        let dockerenv = Path::new("/.dockerenv");
        if dockerenv.exists() {
            std::fs::remove_file(dockerenv).unwrap();
        }

        env::set_var(DOCKER_ENVIRONMENT, "1");
        assert!(super::is_running_in_container().unwrap());
        env::remove_var(DOCKER_ENVIRONMENT);
        assert!(!super::is_running_in_container().unwrap());

        File::create(dockerenv).unwrap();
        env::set_var(DOCKER_ENVIRONMENT, "1");
        let result = super::is_running_in_container();
        env::remove_var(DOCKER_ENVIRONMENT);
        let result2 = super::is_running_in_container();

        std::fs::remove_file(dockerenv).unwrap();

        assert!(result.unwrap());
        assert_eq!(
            result2.unwrap_err().kind(),
            &ErrorKind::Initialization(InitializationError::ContainerConfiguration {
                inner: ContainerConfigurationError::DockerEnvironmentVarCheck {
                    docker_env_var: DOCKER_ENVIRONMENT.to_string()
                }
            })
        );
    }

    #[functional_test(feature = "helpers")]
    fn test_get_host_root_path_in_simulated_container() {
        env::set_var(DOCKER_ENVIRONMENT, "true");

        let test_dir = Path::new(HOST_ROOT_PATH);
        if !test_dir.exists() {
            std::fs::create_dir(test_dir).unwrap();
        }
        assert_eq!(get_host_root_path().unwrap(), Path::new(HOST_ROOT_PATH));
    }

    #[functional_test(feature = "helpers")]
    fn test_get_host_relative_path() {
        // Simulate container environment
        env::set_var(DOCKER_ENVIRONMENT, "true");
        let test_dir = Path::new(HOST_ROOT_PATH);
        if !test_dir.exists() {
            std::fs::create_dir(test_dir).unwrap();
        }

        // Test #1: Running in container; absolute path should be prefixed
        let path = PathBuf::from("/var/lib/extensions/test.raw");
        let result = get_host_relative_path(path).unwrap();
        let expected = path::join_relative(HOST_ROOT_PATH, "/var/lib/extensions/test.raw");
        assert_eq!(result, expected);

        // Cleanup
        env::remove_var(DOCKER_ENVIRONMENT);

        // Test #2; Not running in a container; path should be returned as-is
        let path = PathBuf::from("/var/lib/extensions/test.raw");
        let result = get_host_relative_path(path.clone()).unwrap();
        assert_eq!(result, path);
    }
}
