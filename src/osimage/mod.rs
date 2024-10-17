use url::Url;

pub(crate) mod cosi;

use cosi::Cosi;

/// Abstract representation of an OS image.
#[derive(Debug, Clone)]
pub struct OsImage(OsImageInner);

#[derive(Debug, Clone)]
enum OsImageInner {
    /// Composable OS Image (COSI)
    Cosi(Cosi),
}

impl OsImage {
    pub(crate) fn cosi(url: &Url) -> Result<Self, anyhow::Error> {
        Ok(Self(OsImageInner::Cosi(Cosi::new(url)?)))
    }

    /// Returns the name of the OS image type.
    pub(crate) fn name(&self) -> &'static str {
        match &self.0 {
            OsImageInner::Cosi(_) => "COSI",
        }
    }

    /// Returns the source URL of the OS image.
    pub(crate) fn source(&self) -> &Url {
        match &self.0 {
            OsImageInner::Cosi(cosi) => cosi.source(),
        }
    }

    /// Returns an iterator over the available mount points provided by the OS image.
    pub(crate) fn available_mount_points(&self) -> impl Iterator<Item = &std::path::PathBuf> {
        match &self.0 {
            OsImageInner::Cosi(cosi) => cosi.available_mount_points(),
        }
    }
}
