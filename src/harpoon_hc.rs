use harpoon::{EventResult, EventType, QueryResult};
use log::{debug, error};
use trident_api::{
    config::{HarpoonConfig, HostConfiguration},
    constants::internal_params::ENABLE_HARPOON_SUPPORT,
    error::{InitializationError, TridentError},
    primitives::version::SemverVersion,
};

use crate::validation;

pub(super) enum HostConfigUpdate {
    Updated {
        version: SemverVersion,
        host_config: Box<HostConfiguration>,
    },
    NoUpdate,
}

pub(crate) fn query_and_fetch_host_config(
    config: &HarpoonConfig,
) -> Result<HostConfigUpdate, TridentError> {
    let query_result = harpoon::query_and_fetch_yaml_document(
        &config.url,
        &config.app_id,
        &config.track,
        config.document_version.as_version(),
    )
    .map_err(|e| TridentError::new(InitializationError::QueryForUpdates(e.to_string())))?;

    Ok(match query_result.result {
        QueryResult::NoUpdate => HostConfigUpdate::NoUpdate,
        QueryResult::NewDocument {
            url,
            document,
            version,
        } => HostConfigUpdate::Updated {
            version: version.into(),
            host_config: Box::new(validation::parse_host_config(&document, url.as_str())?),
        },
    })
}

pub(crate) fn try_on_harpoon_enabled<E>(
    host_config: &HostConfiguration,
    f: impl FnOnce(&HarpoonConfig) -> Result<(), E>,
) -> Result<(), E> {
    if !host_config.internal_params.get_flag(ENABLE_HARPOON_SUPPORT) {
        return Ok(());
    }

    let Some(harpoon_config) = host_config.trident.harpoon.as_ref() else {
        return Ok(());
    };

    f(harpoon_config)
}

pub(crate) fn on_harpoon_enabled(host_config: &HostConfiguration, f: impl FnOnce(&HarpoonConfig)) {
    let _ = try_on_harpoon_enabled(host_config, |harpoon_config| -> Result<(), ()> {
        f(harpoon_config);
        Ok(())
    });
}

pub(crate) fn on_harpoon_enabled_event(
    host_config: &HostConfiguration,
    event_type: EventType,
    event_result: EventResult,
) {
    on_harpoon_enabled(host_config, |harpoon_config| {
        match harpoon::report_event(
            &harpoon_config.url,
            &harpoon_config.app_id,
            &harpoon_config.track,
            event_type,
            event_result,
        ) {
            Ok(()) => {
                debug!("Successfully reported '{event_type:?}:{event_result:?}' event to Harpoon")
            }
            Err(e) => error!(
                "Harpoon failed to report event '{event_type:?}:{event_result:?}' to server: {e}"
            ),
        }
    });
}
