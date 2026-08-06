//! Library surface for the `trident-acl-agent` crate.
//!
//! Currently this exposes the [`nebraska`] client module, a self-contained,
//! reusable implementation of the Nebraska/Omaha update protocol. It is usable
//! both by this crate's agent binary and by a future Trident ACL Agent that
//! orchestrates updates differently.

pub mod nebraska;
