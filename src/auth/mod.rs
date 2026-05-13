//! Authentication subsystem.
//!
//! Pure data types (`credential`, `scheme`, `config`), the
//! `CredentialService` trait, the in-memory/session-state stores, and the
//! custom-provider plug-in registry are **always available** — they have no
//! heavy dependencies.
//!
//! The OAuth2 wiring (`manager`, `handler`, `exchanger`, `refresher`,
//! `preprocessor`) lives behind `feature = "auth"` because it pulls
//! `oauth2`, `jsonwebtoken`, `rsa`, and `reqwest`.

pub mod config;
pub mod credential;
pub mod provider;
pub mod scheme;
pub mod service;

#[cfg(feature = "auth")]
pub mod exchanger;
#[cfg(feature = "auth")]
pub mod handler;
#[cfg(feature = "auth")]
pub mod manager;
#[cfg(feature = "auth")]
pub mod preprocessor;
#[cfg(feature = "auth")]
pub mod refresher;

pub use config::AuthConfig;
pub use credential::{
    AuthCredential, AuthCredentialType, HttpAuth, OAuth2Auth, ServiceAccountAuth,
};
pub use provider::{AuthProviderRegistry, BaseAuthProvider};
pub use scheme::{ApiKeyLocation, AuthScheme, OAuthFlow, OAuthFlows, OAuthGrantType};
pub use service::{CredentialService, InMemoryCredentialService, SessionStateCredentialService};

#[cfg(feature = "auth")]
pub use exchanger::{
    CredentialExchanger, ExchangerRegistry, OAuth2Exchanger, ServiceAccountExchanger,
};
#[cfg(feature = "auth")]
pub use handler::AuthHandler;
#[cfg(feature = "auth")]
pub use manager::{CredentialManager, ResolveOutcome};
#[cfg(feature = "auth")]
pub use preprocessor::{AuthPreprocessor, PreprocessOutcome};
#[cfg(feature = "auth")]
pub use refresher::{CredentialRefresher, OAuth2Refresher, RefresherRegistry};

/// Synthetic function name used when the runner needs interactive consent.
pub const REQUEST_CREDENTIAL_FUNCTION_NAME: &str = "adk_request_credential";

/// Prefix for `function_call_id`s that authorise an entire toolset (rather
/// than one specific tool) before tool discovery.
pub const TOOLSET_AUTH_CREDENTIAL_ID_PREFIX: &str = "_adk_toolset_auth_";
