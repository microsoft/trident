use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::anyhow;
use trident_api::error::{InitializationError, InternalError, ReportError, TridentError};

/// Path to the root of the host filesystem. Expected to be mounted there when
/// running in a container.
const HOST_ROOT_PATH: &str = "/host";

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
        return Err(anyhow!(
            "Running from docker container, but {DOCKER_ENVIRONMENT} environment variable is not set"
        ))
        .structured(InitializationError::ContainerConfigurationCheck);
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
        return Err(anyhow!(
            "Running from docker container, but {} is not mounted",
            host_root_path.display()
        ))
        .structured(InitializationError::ContainerConfigurationCheck);
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

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_container() {
        // get_host_root_path_impl tests
        env::set_var(DOCKER_ENVIRONMENT, "true");
        assert_eq!(
            super::get_host_root_path_impl(Path::new(".")).unwrap(),
            Path::new(".")
        );
        super::get_host_root_path_impl(Path::new("/does-not-exist")).unwrap_err();

        env::remove_var(DOCKER_ENVIRONMENT);
        super::get_host_root_path_impl(Path::new(".")).unwrap_err();
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use std::fs::File;

    use pytest_gen::functional_test;
    use trident_api::error::ErrorKind;

    use super::*;

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
            result2
                .unwrap_err()
                .unstructured("")
                .root_cause()
                .to_string(),
            "Running from docker container, but DOCKER_ENVIRONMENT environment variable is not set"
        );
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_get_host_root_path_fails_outside_container() {
        env::remove_var(DOCKER_ENVIRONMENT);
        assert_eq!(
            get_host_root_path().unwrap_err().kind(),
            &ErrorKind::Internal(InternalError::RunInContainer)
        );
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_get_host_root_path_fails_in_simulated_container_without_host_mount() {
        env::set_var(DOCKER_ENVIRONMENT, "true");

        let test_dir = Path::new(HOST_ROOT_PATH);
        if test_dir.exists() {
            std::fs::remove_dir(test_dir).unwrap();
        }

        assert_eq!(
            get_host_root_path()
                .unwrap_err()
                .unstructured("")
                .root_cause()
                .to_string(),
            "Running from docker container, but /host is not mounted"
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
}
