use std::fmt::Write;

use log::{debug, info};

use sysdefs::arch::SystemArchitecture;
use trident_api::{
    config::HostConfiguration,
    error::{InvalidInputError, ReportError, TridentError},
};

use crate::osimage::OsImage;

/// Attempts to load an OS image based on the provided host configuration.
pub(super) fn load_os_image(
    host_config: &HostConfiguration,
) -> Result<Option<OsImage>, TridentError> {
    let Some(os_image_source) = &host_config.image else {
        return Err(TridentError::new(InvalidInputError::MissingOsImage));
    };

    debug!("Loading COSI file '{}'", os_image_source.url);
    let os_image = OsImage::cosi(&os_image_source.url).structured(InvalidInputError::LoadCosi {
        url: os_image_source.url.clone(),
    })?;

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
