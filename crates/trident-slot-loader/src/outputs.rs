//! Publishes Bootengine's lowerdir file. It neither selects images nor mounts them.

use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
};

use anyhow::{ensure, Context, Error};
use log::info;

use crate::filesystem;

pub(super) struct LowerdirOutput {
    path: PathBuf,
}

impl LowerdirOutput {
    /// Validates existing state before the activation transaction starts changing runtime state.
    pub(super) fn preflight(path: PathBuf) -> Result<Self, Error> {
        let output = Self { path };
        output.check()?;
        Ok(output)
    }

    fn check(&self) -> Result<(), Error> {
        filesystem::directory_exists(self.path.parent().context("output has no parent")?)?;
        match fs::symlink_metadata(&self.path) {
            Ok(metadata) => ensure!(
                metadata.is_file(),
                "non-regular lowerdir output {}",
                self.path.display()
            ),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("inspecting lowerdir output"),
        }
        Ok(())
    }

    /// Atomic rename is the handoff; /run is ephemeral and needs no persistent-storage fsync.
    pub(super) fn publish(&self, lowerdirs: &[PathBuf]) -> Result<(), Error> {
        let value = encode(lowerdirs)?;
        self.check()?;
        if lowerdirs.is_empty() {
            let removed = filesystem::remove_regular(&self.path)?;
            info!(
                "event=lowerdirs_absent path={} stale_removed={removed}",
                self.path.display()
            );
            return Ok(());
        }
        let parent = self.path.parent().context("output has no parent")?;
        filesystem::create_directory(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary
            .write_all(value.as_bytes())
            .context("writing lowerdir handoff")?;
        temporary
            .persist(&self.path)
            .with_context(|| format!("publishing {}", self.path.display()))?;
        info!(
            "event=lowerdirs_published path={} count={}",
            self.path.display(),
            lowerdirs.len()
        );
        Ok(())
    }
}

fn encode(lowerdirs: &[PathBuf]) -> Result<String, Error> {
    lowerdirs
        .iter()
        .map(|path| {
            let value = path.to_str().context("non-UTF8 overlay lowerdir")?;
            ensure!(
                path.is_absolute()
                    && !value
                        .chars()
                        .any(|ch| ch.is_control() || matches!(ch, ':' | ',' | '\\')),
                "unsafe overlay lowerdir {}",
                path.display()
            );
            Ok(value)
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|paths| paths.join(":"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::fs::symlink;

    #[test]
    fn bad_lowerdirs_preserve_previous_publication_and_do_not_create_temporary_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("lowerdirs");
        fs::write(&path, "previous").unwrap();
        let output = LowerdirOutput::preflight(path.clone()).unwrap();
        for value in ["relative", "/run/a:b", "/run/a,b", "/run/a\\b", "/run/a\nb"] {
            output.publish(&[value.into()]).unwrap_err();
            assert_eq!(fs::read_to_string(&path).unwrap(), "previous");
        }
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
        output
            .publish(&["/run/high/etc".into(), "/run/low/etc".into()])
            .unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "/run/high/etc:/run/low/etc"
        );
    }

    #[test]
    fn stale_output_cleanup_does_not_follow_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let victim = root.path().join("victim");
        let path = root.path().join("lowerdirs");
        fs::write(&victim, "keep").unwrap();
        symlink(&victim, &path).unwrap();
        LowerdirOutput::preflight(path).err().unwrap();
        assert_eq!(fs::read_to_string(victim).unwrap(), "keep");
    }
}
