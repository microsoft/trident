//! Trident gRPC definitions.
//!
//! This module contains the gRPC definitions for Trident's gRPC API, generated
//! from the source proto files using Tonic.

pub mod v1 {
    tonic::include_proto!("trident.v1");
}

#[cfg(feature = "grpc-preview")]
pub mod v1preview {
    tonic::include_proto!("trident.v1preview");
}
