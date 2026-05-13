//! Pluggable services: session, memory, artifact, and credential.
//!
//! The in-memory backends in [`mem`] are always available. The filesystem
//! ([`fs`], feature `fs`) and SQL ([`sql`], features `sqlite` / `postgres`)
//! backends are gated behind cargo features.

pub mod mem;

#[cfg(feature = "fs")]
pub mod fs;

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub mod sql;
