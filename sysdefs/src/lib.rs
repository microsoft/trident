//! sysdefs (System Definitions) is a dependency-less crate meant exclusively to
//! contain definitions for simple, basic, or axiomatic system/OS
//! concepts, abstractions, and constants.
//!
//! As the name implies, the crate mainly provides definitions, and should
//! contain minimal or no behavior at all.
//!

pub mod arch;
pub mod filesystems;
pub mod osuuid;
pub mod partition_types;
pub mod tpm2;
