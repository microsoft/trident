use std::{
    fs::File,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Error};

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
    let file = create_file(path)?;
    file.metadata()?.permissions().set_mode(mode);
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
pub fn read_file_trim(file_path: &Path) -> Result<String, Error> {
    let content = std::fs::read_to_string(file_path)
        .context(format!("Could not read file contents: {:?}", file_path))?;
    Ok(content.trim().to_string())
}
