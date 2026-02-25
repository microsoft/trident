//! Trident gRPC definitions.
//!
//! This module contains the gRPC definitions for Trident's gRPC API, generated
//! from the source proto files using Tonic.

use const_format::formatcp;

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

/// The URI scheme for Unix domain sockets.
const UNIX_SOCKET_SCHEME: &str = "unix://";

/// The default socket for the Trident gRPC server, in URI format.
pub const TRIDENT_DEFAULT_SOCKET_URI: &str =
    formatcp!("{UNIX_SOCKET_SCHEME}{TRIDENT_DEFAULT_SOCKET_PATH}");

pub mod logging;
