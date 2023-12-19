use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::{bail, Error};

/// Path to the root of the host filesystem. Expected to be mounted there when
/// running in a container.
const HOST_ROOT_PATH: &str = "/host";

/// Environment variable that is set when running in a container. Value is not
/// important.
const DOCKER_ENVIRONMENT: &str = "DOCKER_ENVIRONMENT";

/// Uses the `DOCKER_ENVIRONMENT` environment variable to determine if the
/// current process is running in a container. This variable needs to be
/// explicitly set as part of Dockerfile.
pub fn is_running_in_container() -> bool {
    env::var(DOCKER_ENVIRONMENT).is_ok()
}

/// For use only when running in a container. If running in a container, returns
/// the path to the root of the host filesystem. Host filesystem is expected to
/// be mounted at the provided input and if that path does not exist, an error
/// is returned.
fn get_host_root_path_impl(host_root_path: &Path) -> Result<PathBuf, Error> {
    if !is_running_in_container() {
        bail!("Not running in a container")
    }

    // We expect the host filesystem to be available under host_root_path
    if !host_root_path.exists() {
        bail!(
            "Running from docker container, but {} is not mounted",
            host_root_path.display()
        );
    }
    Ok(PathBuf::from(host_root_path))
}

/// For use only when running in a container. If running in a container, returns
/// the path to the root of the host filesystem. Host filesystem is expected to
/// be mounted at `/host` and if that path does not exist, an error
/// is returned.
pub fn get_host_root_path() -> Result<PathBuf, Error> {
    get_host_root_path_impl(Path::new(HOST_ROOT_PATH))
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_container() {
        // is_running_in_container tests
        env::set_var(DOCKER_ENVIRONMENT, "1");
        assert!(super::is_running_in_container());
        env::remove_var(DOCKER_ENVIRONMENT);
        assert!(!super::is_running_in_container());

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

#[cfg(all(test, feature = "functional-tests"))]
mod functional_tests {
    use super::*;

    #[test]
    fn test() {
        env::remove_var(DOCKER_ENVIRONMENT);
        assert_eq!(
            get_host_root_path().unwrap_err().root_cause().to_string(),
            "Not running in a container"
        );
        env::set_var(DOCKER_ENVIRONMENT, "true");

        let test_dir = Path::new(HOST_ROOT_PATH);
        if test_dir.exists() {
            std::fs::remove_dir(test_dir).unwrap();
        }

        assert_eq!(
            get_host_root_path().unwrap_err().root_cause().to_string(),
            "Running from docker container, but /host is not mounted"
        );

        std::fs::create_dir(test_dir).unwrap();
        assert_eq!(get_host_root_path().unwrap(), Path::new(HOST_ROOT_PATH));
    }
}
