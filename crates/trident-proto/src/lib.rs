//! Trident gRPC definitions.
//!
//! This module contains the gRPC definitions for Trident's gRPC API, generated
//! from the source proto files using Tonic.

/// The generated gRPC code for the Trident API v1.
pub mod v1 {
    tonic::include_proto!("trident.v1");
}

/// The generated gRPC code for the Trident API v1preview.
#[cfg(feature = "grpc-preview")]
pub mod v1preview {
    tonic::include_proto!("trident.v1preview");
}

/// The default socket for the Trident gRPC server in absolute path format.
pub const TRIDENT_DEFAULT_SOCKET_PATH: &str = "/run/trident/trident.sock";

/// The default socket for the Trident gRPC server, in URI format.
pub const TRIDENT_DEFAULT_SOCKET_URI: &str = "unix:///run/trident/trident.sock";

pub mod logging;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_socket_path_uri() {
        // The URI MUST be equal to the path prefixed with "unix://".
        assert_eq!(
            TRIDENT_DEFAULT_SOCKET_URI,
            format!("unix://{}", TRIDENT_DEFAULT_SOCKET_PATH)
        );
    }
}
