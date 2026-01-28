// Include generated gRPC code
tonic::include_proto!("harpoon.v1");

pub mod cli;
pub mod client;

pub use cli::{AllowedOperation, Cli, Commands, GetKind};
pub use client::TridentClient;
