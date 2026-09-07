use log::info;
use tonic::{async_trait, Request, Response, Status};

use trident_api::error::{InternalError, TridentError};
use trident_proto::{
    v1::TridentError as ProtoTridentError,
    v1preview::{
        validation_service_server::ValidationService, GetRequiredServicingTypeRequest,
        GetRequiredServicingTypeResponse, ValidateHostConfigurationRequest,
        ValidateHostConfigurationResponse,
    },
};

use crate::{logging::operation_context, server::TridentServer, validation};

#[async_trait]
impl ValidationService for TridentServer {
    async fn validate_host_configuration(
        &self,
        request: Request<ValidateHostConfigurationRequest>,
    ) -> Result<Response<ValidateHostConfigurationResponse>, Status> {
        // Validate is different because it only acts upon the input and does
        // not read or modify state in any way, so we are free to run this
        // whenever without doing any lock checks.
        info!("Received Host Configuration validation request");
        let Some(host_config) = request.into_inner().config else {
            return Err(self.reject_invalid_argument(
                "validate_host_configuration",
                "config",
                "Missing host configuration in staging configuration",
            ));
        };

        self.refresh_correlation_id("validate_host_configuration");

        // A semantically invalid Host Configuration is reported back to the
        // caller as a normal (ok: false) response, not a gRPC error status --
        // this is a real, successful validation outcome, not a failed RPC.
        // But it's still a genuine, classified `TridentError`, and without
        // wrapping it in `run_command`, this -- the most common
        // validation-failure case -- fired no `command_start`/`command_error`
        // telemetry at all, unlike the missing-config rejection above (via
        // `reject_invalid_argument`), which always fires both. See the
        // `block_in_place` comment on `reject_invalid_argument` for why this
        // needs `block_in_place` too: `run_command` synchronously fires
        // tracing events, and a configured remote telemetry sender does a
        // blocking POST from inside that same call.
        let error = tokio::task::block_in_place(|| {
            operation_context::run_command("validate_host_configuration", || {
                validation::validate_host_config_string(&host_config.config)
            })
        })
        .err()
        .map(ProtoTridentError::from);

        Ok(Response::new(ValidateHostConfigurationResponse {
            ok: error.is_none(),
            error,
        }))
    }

    async fn get_required_servicing_type(
        &self,
        _request: Request<GetRequiredServicingTypeRequest>,
    ) -> Result<Response<GetRequiredServicingTypeResponse>, Status> {
        self.reading_request("get_required_servicing_type", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: get_required_servicing_type",
            )))
        })
        .await
    }
}
