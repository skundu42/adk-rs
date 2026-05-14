//! SQL-backed [`SessionService`](crate::core::SessionService) over `sqlx`.
//!
//! Features:
//! * `sqlite` — file or in-memory SQLite.
//! * `postgres` — PostgreSQL via `postgres://` URLs.
//!
//! If both backend features are enabled, both concrete aliases are exported
//! and the compatibility `SqlSessionService` alias points at SQLite. Schema
//! lives under `migrations/`.

#[cfg(feature = "postgres")]
mod postgres_backend;
#[cfg(feature = "sqlite")]
mod sqlite_backend;

#[cfg(feature = "postgres")]
pub use postgres_backend::SqlSessionService as PostgresSessionService;
#[cfg(feature = "sqlite")]
pub use sqlite_backend::SqlSessionService as SqliteSessionService;

#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
pub use postgres_backend::SqlSessionService;
#[cfg(feature = "sqlite")]
pub use sqlite_backend::SqlSessionService;
