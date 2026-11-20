//! Full confext preparation: read-only DDI mounts, compatibility validation and mount lifetime.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Error};
use log::{debug, info};

use crate::{filesystem, release, selection::OverlayImage, tools::ImageMounter};

/// Every attempted mount is retained until commit or reverse-order cleanup, including partial failures.
pub(super) struct MountSet<'a, M: ImageMounter> {
    mounter: &'a M,
    attempted: Vec<PathBuf>,
}

impl<'a, M: ImageMounter> MountSet<'a, M> {
    pub(super) fn new(mounter: &'a M) -> Self {
        Self {
            mounter,
            attempted: Vec::new(),
        }
    }

    pub(super) fn prepare(
        &mut self,
        images: &[OverlayImage],
        host_root: &Path,
    ) -> Result<Vec<PathBuf>, Error> {
        if images.is_empty() {
            return Ok(Vec::new());
        }
        let host = release::host(host_root)?;
        let mut lowerdirs = Vec::with_capacity(images.len());
        for image in images {
            filesystem::create_directory(
                image
                    .mountpoint
                    .parent()
                    .context("mountpoint has no parent")?,
            )?;
            fs::create_dir(&image.mountpoint).with_context(|| {
                format!("creating fresh mountpoint {}", image.mountpoint.display())
            })?;
            self.attempted.push(image.mountpoint.clone());
            info!(
                "event=mount_start image={:?} source={} target={}",
                image.name,
                image.source.display(),
                image.mountpoint.display()
            );
            self.mounter
                .mount(&image.source, &image.mountpoint)
                .with_context(|| format!("mounting confext {:?}", image.name))?;
            let lower = release::validate_confext(&image.mountpoint, &image.name, &host)
                .with_context(|| format!("validating confext {:?}", image.name))?;
            info!(
                "event=mount_ready image={:?} lowerdir={}",
                image.name,
                lower.display()
            );
            lowerdirs.push(lower);
        }
        Ok(lowerdirs)
    }

    pub(super) fn rollback(&mut self) -> Vec<Error> {
        let mut errors = Vec::new();
        while let Some(path) = self.attempted.pop() {
            if let Err(error) = self.mounter.unmount(&path) {
                errors.push(error.context(format!("unmounting {}", path.display())));
                continue;
            }
            if let Err(error) = fs::remove_dir(&path) {
                errors.push(
                    Error::new(error).context(format!("removing mountpoint {}", path.display())),
                );
            } else {
                debug!("event=mount_removed target={}", path.display());
            }
        }
        errors
    }
}
