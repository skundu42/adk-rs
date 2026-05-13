//! SQL-backed [`SessionService`](adk_rs_core::SessionService) over `sqlx`.
//!
//! Features:
//! * `sqlite` (default) — file or in-memory SQLite.
//! * `postgres` — PostgreSQL via `postgres://` URLs.
//!
//! At most one backend feature should be enabled at a time. Schema lives
//! under `migrations/`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[cfg(feature = "postgres")]
#[cfg_attr(feature = "sqlite", allow(dead_code, unreachable_pub))]
mod postgres_backend;
#[cfg(feature = "sqlite")]
mod sqlite_backend;

#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
pub use postgres_backend::SqlSessionService;
#[cfg(feature = "sqlite")]
pub use sqlite_backend::SqlSessionService;
