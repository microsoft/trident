use std::fmt::Write;

use log::{debug, info, warn};

use trident_api::{
    config::{HostConfiguration, OsImage as ApiOsImage},
    constants::internal_params::ENABLE_COSI_SUPPORT,
    error::{InvalidInputError, ReportError, TridentError},
};

use crate::osimage::OsImage;

/// Attempts to load an OS image based on the provided host configuration.
pub(super) fn load_os_image(
    host_config: &HostConfiguration,
) -> Result<Option<OsImage>, TridentError> {
    if !host_config.internal_params.get_flag(ENABLE_COSI_SUPPORT) {
        debug!("COSI file usage disabled");
        return Ok(None);
    }

    warn!("USING EXPERIMENTAL COSI FILE SUPPORT");
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
