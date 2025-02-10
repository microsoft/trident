use std::fmt::Write;

use log::{debug, info};

use osutils::arch::SystemArchitecture;
use trident_api::{
    config::{HostConfiguration, OsImage as ApiOsImage},
    error::{InvalidInputError, ReportError, TridentError},
};

use crate::osimage::OsImage;

/// Attempts to load an OS image based on the provided host configuration.
pub(super) fn load_os_image(
    host_config: &HostConfiguration,
) -> Result<Option<OsImage>, TridentError> {
    // Skip when using the old API
    if host_config
        .storage
        .filesystems
        .iter()
        .any(|fs| fs.source.is_old_api())
    {
        debug!("Skipping OS image loading because the old API is being used");
        return Ok(None);
    }

    let Some(os_image_source) = &host_config.os_image else {
        return Err(TridentError::new(InvalidInputError::MissingOsImage));
    };

    let os_image = match os_image_source {
        ApiOsImage::Cosi(cosi_file) => {
            debug!("Loading COSI file '{}'", cosi_file.url);
            OsImage::cosi(&cosi_file.url).structured(InvalidInputError::LoadCosi {
                url: cosi_file.url.clone(),
            })?
        }
    };

    info!(
        "Successfully loaded OS image of type '{}' from '{}'",
        os_image.name(),
        os_image.source()
    );

    // Ensure the OS image architecture matches the current system architecture
    if SystemArchitecture::current() != os_image.architecture() {
        return Err(TridentError::new(
            InvalidInputError::MismatchedArchitecture {
                expected: SystemArchitecture::current().into(),
                actual: os_image.architecture().into(),
            },
        ));
    }

    debug!(
        "OS image provides the following mount points:\n{}",
        os_image
            .available_mount_points()
            .fold(String::new(), |mut acc, p| {
                let _ = writeln!(acc, "  - {}", p.display());
                acc
            })
    );

    Ok(Some(os_image))
}
