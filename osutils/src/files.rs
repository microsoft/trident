use std::{
    fs::{File, Permissions},
    io::{Read, Write},
    os::{linux::fs::MetadataExt, unix::fs::PermissionsExt},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error};

/// Creates a file and all parent directories if they don't exist
pub fn create_file<S>(path: S) -> Result<File, Error>
where
    S: AsRef<Path>,
{
    if let Some(parent) = path.as_ref().parent() {
        create_dirs(parent)?;
    }

    std::fs::File::create(path.as_ref()).context(format!(
        "Could not create file: {}",
        path.as_ref().display()
    ))
}

/// Creates a file and all parent directories if they don't exist, and sets the file mode
pub fn create_file_mode<S>(path: S, mode: u32) -> Result<File, Error>
where
    S: AsRef<Path>,
{
    let file = create_file(path.as_ref())?;
    std::fs::set_permissions(path.as_ref(), Permissions::from_mode(mode)).context(format!(
        "Could not set permissions {:#o} for file {}",
        mode,
        path.as_ref().display()
    ))?;
    Ok(file)
}

/// Creates all directories in a path if they don't exist
pub fn create_dirs<S>(path: S) -> Result<(), Error>
where
    S: AsRef<Path>,
{
    std::fs::create_dir_all(path.as_ref()).context(format!(
        "Could not create path: {}",
        path.as_ref().display()
    ))
}

/// Creates a file with a random name in the specified location
/// It creates all parent directories if they don't exist
pub fn create_random_file<S>(location: S) -> Result<(File, PathBuf), Error>
where
    S: AsRef<Path>,
{
    create_dirs(location.as_ref())?;
    tempfile::NamedTempFile::new_in(location)
        .context("Failed to create temporary file")?
        .keep()
        .context("Failed to persist file")
}

/// Reads the content of a file and trims it
pub fn read_file_trim<S>(file_path: &S) -> Result<String, Error>
where
    S: AsRef<Path>,
{
    let content = std::fs::read_to_string(file_path.as_ref()).context(format!(
        "Could not read file contents: {:?}",
        file_path.as_ref()
    ))?;
    Ok(content.trim().to_string())
}

/// Prepends to a file
pub fn prepend_file<S>(path: S, must_exist: bool, new_contents: &[u8]) -> Result<(), Error>
where
    S: AsRef<Path>,
{
    let mut mode = 0o600;
    let mut content = new_contents.to_owned();
    if path.as_ref().exists() {
        if !path.as_ref().is_file() {
            bail!("Path exists but is not a file: {}", path.as_ref().display());
        }

        mode = std::fs::metadata(path.as_ref())
            .context(format!(
                "Could not get metadata for {}",
                path.as_ref().display()
            ))?
            .permissions()
            .mode();

        File::open(path.as_ref())
            .context(format!("Could not open file: {}", path.as_ref().display()))?
            .read_to_end(&mut content)
            .context(format!(
                "Failed to read existing contents of {}",
                path.as_ref().display(),
            ))?;
    } else if must_exist {
        bail!("Path does not exist: {}", path.as_ref().display());
    }

    let mut file = create_file_mode(path.as_ref(), mode).context(format!(
        "Could not create file: {}",
        path.as_ref().display()
    ))?;

    file.write_all(&content).context(format!(
        "Could not write to file: {}",
        path.as_ref().display()
    ))?;

    Ok(())
}

/// Writes to a file
pub fn write_file<S>(path: S, mode: u32, contents: &[u8]) -> Result<(), Error>
where
    S: AsRef<Path>,
{
    let mut file = create_file_mode(path.as_ref(), mode).context(format!(
        "Could not create file: {}",
        path.as_ref().display()
    ))?;

    file.write_all(contents).context(format!(
        "Could not write to file: {}",
        path.as_ref().display()
    ))?;

    Ok(())
}

pub fn get_owner_uid<S>(path: S) -> Result<u32, Error>
where
    S: AsRef<Path>,
{
    Ok(path
        .as_ref()
        .metadata()
        .context(format!(
            "Failed to get metadata for {}",
            path.as_ref().display()
        ))?
        .st_uid())
}

pub fn get_owner_gid<S>(path: S) -> Result<u32, Error>
where
    S: AsRef<Path>,
{
    Ok(path
        .as_ref()
        .metadata()
        .context(format!(
            "Failed to get metadata for {}",
            path.as_ref().display()
        ))?
        .st_gid())
}

pub fn clean_directory<S>(path: S) -> Result<(), Error>
where
    S: AsRef<Path>,
{
    let path = path.as_ref();
    if !path.exists() {
        return Ok(());
    }

    if !path.is_dir() {
        bail!("Path exists but is not a directory: {}", path.display());
    }

    std::fs::read_dir(path)
        .context(format!(
            "Failed to read contents of directory {}",
            path.display()
        ))?
        .try_for_each(|entry| {
            let path = entry.context("Failed to read entry")?.path();
            if path.is_dir() {
                std::fs::remove_dir_all(&path)
                    .with_context(|| format!("Failed to remove directory: {}", path.display()))
            } else {
                std::fs::remove_file(&path)
                    .with_context(|| format!("Failed to remove file: {}", path.display()))
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::tempdir;

    #[test]
    fn test_create_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("some/path").join("test.txt");
        create_file(&path).unwrap();
        assert!(path.exists());
        assert!(path.is_file());
    }

    #[test]
    fn test_create_file_mode() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        let file = create_file_mode(&path, 0o600).unwrap();
        assert!(path.exists());
        assert!(path.is_file());
        // Bitwise AND to ignore the file type
        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn test_create_dirs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test").join("test2");
        create_dirs(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn test_prepend_file() {
        let dir = tempdir().unwrap();
        let (mut file, path) = create_random_file(&dir).unwrap();

        let heading = "hello world\n";
        let contents = indoc::indoc! {r#"
            line 1
            line 2
            line 3
        "#};
        let expected = format!("{}{}", heading, contents);

        file.write_all(contents.as_bytes()).unwrap();
        file.flush().unwrap();
        // Close file
        drop(file);

        // Check that the contents are correct
        assert_eq!(std::fs::read(&path).unwrap(), contents.as_bytes());
        // Prepend the heading
        prepend_file(&path, false, heading.as_bytes()).unwrap();
        // Check that the new contents are correct
        assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());

        // Assert that file is created when requested
        let nonexistent_path = dir.path().join("nonexistent");
        assert!(!nonexistent_path.exists());
        prepend_file(&nonexistent_path, false, heading.as_bytes()).unwrap();
        assert!(nonexistent_path.exists());

        // Assert error when file does not exist and must_exist=true
        let nonexistent_path = dir.path().join("nonexistent2");
        assert!(!nonexistent_path.exists());
        assert!(prepend_file(&nonexistent_path, true, heading.as_bytes()).is_err());
    }

    #[test]
    fn test_read_file_trim() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        let contents = indoc::indoc! {r#"

                 line 1    


        "#};
        std::fs::write(&path, contents).unwrap();
        assert_eq!(read_file_trim(&path).unwrap(), "line 1");
    }

    #[test]
    fn test_get_owner_uid() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        // Create a file, the owner should be the same as the current process
        let _ = create_file(&path).unwrap();

        // Yeah, this is silly, but it's the only way to get the current
        // process' UID without using an external crate
        assert_eq!(
            get_owner_uid(path).unwrap(),
            get_owner_uid("/proc/self").unwrap()
        );
    }

    #[test]
    fn test_get_owner_gid() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        // Create a file, the owner should be the same as the current process
        let _ = create_file(&path).unwrap();

        // Yeah, this is silly, but it's the only way to get the current
        // process' GID without using an external crate
        assert_eq!(
            get_owner_gid(&path).unwrap(),
            get_owner_gid("/proc/self").unwrap()
        );
    }

    #[test]
    fn test_clean_directory() {
        let test_dir = tempdir().unwrap();

        // Create a bunch of files in the tempdir
        let files = (0..10)
            .map(|i| test_dir.path().join(format!("test_file_{}", i)))
            .collect::<Vec<PathBuf>>();
        files.iter().for_each(|file| {
            create_file(file).unwrap();
        });

        // Create a bunch of directories in the tempdir
        let dirs = (0..10)
            .map(|i| test_dir.path().join(format!("test_dir_{}", i)))
            .collect::<Vec<PathBuf>>();
        dirs.iter().for_each(|dir| {
            create_dirs(dir).unwrap();
            create_file(dir.join("test_file")).unwrap();
        });

        clean_directory(&test_dir).unwrap();

        // Assert that the directory still exists
        assert!(test_dir.path().exists(), "Directory should still exist");

        // Assert that all files and directories are gone
        files.iter().for_each(|file| {
            assert!(!file.exists(), "File should not exist: {}", file.display());
        });

        dirs.iter().for_each(|dir| {
            assert!(
                !dir.exists(),
                "Directory should not exist: {}",
                dir.display()
            );
        });
    }
}
