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

use crate::{server::TridentServer, validation};

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
            return Err(Status::invalid_argument(
                "Missing host configuration in staging configuration",
            ));
        };

        let error = validation::validate_host_config_string(&host_config.config)
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
