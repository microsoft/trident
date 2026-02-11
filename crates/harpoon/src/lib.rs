//! Harpoon gRPC definitions.
//!
//! This module contains the gRPC definitions for Harpoon, generated from the
//! `harpoon.proto` file using Tonic.

#[cfg(not(feature = "grpc-preview"))]
tonic::include_proto!("harpoon.v1");

#[cfg(feature = "grpc-preview")]
tonic::include_proto!("harpoon.v1preview");
