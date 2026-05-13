//! In-memory [`CredentialService`](crate::core::CredentialService) — re-exports
//! the canonical implementation from [`crate::auth::service`] so existing
//! `crate::services::mem::InMemoryCredentialService` imports keep working.

pub use crate::auth::service::InMemoryCredentialService;
