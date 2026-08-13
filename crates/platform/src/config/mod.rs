//! File backed configuration for every `OpenPR` binary.
//!
//! `OpenPR` reads no environment variables. One TOML file, divided into sections, configures the
//! API, the worker and the MCP server; each binary consumes the sections it needs and ignores the
//! rest. The file is located by an explicit path only — a `--config <path>` flag, or
//! [`DEFAULT_CONFIG_PATH`] relative to the process working directory — so a deployment is never
//! configured by something that happened to be exported into the process environment.
//!
//! # Loading
//!
//! ```no_run
//! use platform::config::{AppConfig, OpenPrConfig};
//!
//! # fn main() -> Result<(), platform::error::AppError> {
//! // `explicit` is whatever `--config` carried, or `None`.
//! let explicit: Option<std::path::PathBuf> = None;
//! let config = OpenPrConfig::load(explicit.as_deref())?;
//! let cfg = AppConfig::from_config(&config, "api", "0.0.0.0:8081")?;
//! # let _ = cfg;
//! # Ok(())
//! # }
//! ```
//!
//! # Failure behaviour
//!
//! A missing file is an error, never a silent fallback to defaults: a service that starts with an
//! invented configuration is a service that starts with the wrong database and the wrong signing
//! key. Validation collects *every* unusable value before returning, so one round of edits fixes
//! the whole file.
//!
//! # Eager shape, lazy presence
//!
//! Loading validates the *shape* of every value the file does contain, and refuses the file if
//! any of them is unusable. Whether a *mandatory* value is present is decided by the binary that
//! needs it, through [`OpenPrConfig::database_runtime`], [`OpenPrConfig::auth_runtime`] and
//! [`OpenPrConfig::mcp_runtime`]: one file serves three binaries, and the MCP server reaches the
//! API over HTTP without ever opening a database connection or signing a token. Requiring
//! `database.url` and `auth.jwt_secret` at load time forced an MCP-only deployment to invent two
//! credentials it never uses, which is worse than useless — an invented database URL is a real
//! connection string pointing somewhere unintended.
//!
//! A binary that does need a section still fails to start without it: the accessors report every
//! missing value of the section at once, and [`AppConfig::from_config`] — the API's and the
//! worker's single entry point — reports the missing database *and* auth values together.

mod connectors;
mod error;
mod raw;
mod secret;

use std::fs;
use std::path::{Path, PathBuf};

use uuid::Uuid;

pub use connectors::{
    ConnectorSecretError, ConnectorSecrets, MAX_CONNECTOR_SECRET_NAME_LEN, validate_connector_secret_name,
};
pub use error::{ConfigError, EXAMPLE_CONFIG_PATH};
pub use secret::{REDACTED, Secret};

/// Where a binary looks for its configuration when no `--config` path is given.
///
/// Relative to the process working directory, so a container that mounts the file at
/// `/app/config/openpr.toml` and runs with `/app` as its working directory needs no flag.
pub const DEFAULT_CONFIG_PATH: &str = "config/openpr.toml";

/// Shortest accepted `auth.jwt_secret`.
///
/// Every access and refresh token in the deployment is signed with it; a short value is brute
/// forceable offline from a single captured token.
pub const MIN_JWT_SECRET_LEN: usize = 16;

/// Shortest accepted `mcp.auth_token`.
///
/// A two character token is worse than none: it invites the operator to believe the port is
/// protected while it is trivially guessable.
pub const MIN_MCP_AUTH_TOKEN_LEN: usize = 16;

/// Region assumed when `[storage.s3]` does not name one.
pub const DEFAULT_S3_REGION: &str = "us-east-1";

/// Directory assumed by the local storage backend.
pub const DEFAULT_STORAGE_DIR: &str = "./uploads";

/// API base URL assumed by the MCP server when `[mcp]` does not name one.
pub const DEFAULT_MCP_API_URL: &str = "http://localhost:8081";

/// Bind address assumed by the MCP server's HTTP and SSE transports.
pub const DEFAULT_MCP_BIND_ADDR: &str = "127.0.0.1:8090";

/// The whole configuration file, parsed and validated.
///
/// `Debug` is derived and is safe: every field carrying credential material is a [`Secret`],
/// which renders as [`REDACTED`], and [`ConnectorSecrets`] renders as counts only.
#[derive(Clone, Debug)]
pub struct OpenPrConfig {
    /// File this configuration was read from, used in diagnostics.
    pub origin: PathBuf,
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub auth: AuthConfig,
    pub logging: LoggingConfig,
    pub storage: StorageConfig,
    pub migrations: MigrationsConfig,
    pub outbound: OutboundConfig,
    pub mcp: McpConfig,
    pub connectors: ConnectorsConfig,
}

impl OpenPrConfig {
    /// Reads and validates the configuration.
    ///
    /// `explicit` is the path a `--config` flag carried. When it is `None`,
    /// [`DEFAULT_CONFIG_PATH`] is used, relative to the process working directory.
    pub fn load(explicit: Option<&Path>) -> Result<Self, ConfigError> {
        let path = explicit.map_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH), Path::to_path_buf);
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(ConfigError::NotFound { path: absolute(&path) });
            }
            Err(err) => {
                return Err(ConfigError::Unreadable {
                    path: absolute(&path),
                    reason: err.to_string(),
                });
            }
        };
        Self::parse(&source, &absolute(&path))
    }

    /// Validates configuration already held in memory. `origin` only labels diagnostics.
    pub fn parse(source: &str, origin: &Path) -> Result<Self, ConfigError> {
        let raw: raw::RawConfig = toml::from_str(source).map_err(|err| ConfigError::from_toml(origin, source, &err))?;
        raw.validate(origin)
    }

    /// The database connection settings, or every reason they are not usable.
    ///
    /// `[database]` is optional for the MCP server, which reaches the API over HTTP and opens no
    /// connection of its own, so presence is checked here rather than at load time; a value that
    /// *is* present is already known to be well formed.
    pub fn database_runtime(&self) -> Result<DatabaseRuntime, ConfigError> {
        self.database.runtime(&self.origin)
    }

    /// The token signing settings, or every reason they are not usable.
    ///
    /// `[auth]` is optional for the MCP server, which presents a bot token issued elsewhere and
    /// signs nothing itself; see [`Self::database_runtime`] for why presence is checked here.
    pub fn auth_runtime(&self) -> Result<AuthRuntime, ConfigError> {
        self.auth.runtime(&self.origin)
    }

    /// The MCP server's runtime settings, or every reason they are not usable.
    ///
    /// `[mcp]` is optional for the API and the worker, so presence is checked here rather than at
    /// load time; a value that *is* present is already known to be well formed.
    pub fn mcp_runtime(&self) -> Result<McpRuntime, ConfigError> {
        self.mcp.runtime(&self.origin)
    }
}

/// Builds the validation error a runtime accessor returns when values are missing.
fn missing_values(origin: &Path, issues: Vec<String>) -> ConfigError {
    ConfigError::Invalid {
        path: origin.to_path_buf(),
        issues,
    }
}

/// Resolves a path against the working directory so diagnostics name a real location.
fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
}

/// `[server]` — identity and listen address of an HTTP binary.
///
/// Both values are optional because one file serves three binaries that listen on different
/// ports; each binary supplies its own fallback through [`AppConfig::from_config`].
#[derive(Clone, Debug, Default)]
pub struct ServerConfig {
    pub app_name: Option<String>,
    pub bind_addr: Option<String>,
}

/// What a binary that opens a database connection is told when `database.url` is absent.
const DATABASE_URL_REQUIRED: &str =
    "database.url is required to open a database connection; add it to the [database] section";

/// What a binary that issues or verifies tokens is told when `auth.jwt_secret` is absent.
const JWT_SECRET_REQUIRED: &str = "auth.jwt_secret is required to sign and verify tokens; add it to the [auth] section (generate one with \
     `openssl rand -hex 32`)";

/// `[database]` — connection string and pool shape.
///
/// `url` is an `Option` because the MCP server shares this file and never connects to the
/// database; [`DatabaseRuntime`] is the shape a binary that does connect asks for.
#[derive(Clone, Debug, Default)]
pub struct DatabaseConfig {
    /// Connection URL. A [`Secret`] because it carries the password.
    pub url: Option<Secret>,
    pub max_connections: u32,
    pub min_connections: u32,
    pub connect_timeout_seconds: u64,
    pub idle_timeout_seconds: u64,
    pub acquire_timeout_seconds: u64,
}

impl DatabaseConfig {
    /// Why this section cannot open a connection, or `None` when it can.
    const fn missing(&self) -> Option<&'static str> {
        if self.url.is_none() {
            Some(DATABASE_URL_REQUIRED)
        } else {
            None
        }
    }

    fn runtime(&self, origin: &Path) -> Result<DatabaseRuntime, ConfigError> {
        let Some(url) = self.url.as_ref() else {
            return Err(missing_values(origin, vec![DATABASE_URL_REQUIRED.to_owned()]));
        };
        Ok(DatabaseRuntime {
            url: url.clone(),
            max_connections: self.max_connections,
            min_connections: self.min_connections,
            connect_timeout_seconds: self.connect_timeout_seconds,
            idle_timeout_seconds: self.idle_timeout_seconds,
            acquire_timeout_seconds: self.acquire_timeout_seconds,
        })
    }
}

/// `[database]` with the mandatory fields resolved, as a binary that opens the pool needs them.
#[derive(Clone, Debug)]
pub struct DatabaseRuntime {
    pub url: Secret,
    pub max_connections: u32,
    pub min_connections: u32,
    pub connect_timeout_seconds: u64,
    pub idle_timeout_seconds: u64,
    pub acquire_timeout_seconds: u64,
}

/// `[auth]` — token signing and issuance.
///
/// `jwt_secret` is an `Option` for the same reason as [`DatabaseConfig::url`]: the MCP server
/// presents a bot token issued by the API and signs nothing itself.
#[derive(Clone, Debug, Default)]
pub struct AuthConfig {
    pub jwt_secret: Option<Secret>,
    pub access_ttl_seconds: i64,
    pub refresh_ttl_seconds: i64,
    pub default_author_id: Option<Uuid>,
}

impl AuthConfig {
    /// Why this section cannot sign a token, or `None` when it can.
    const fn missing(&self) -> Option<&'static str> {
        if self.jwt_secret.is_none() {
            Some(JWT_SECRET_REQUIRED)
        } else {
            None
        }
    }

    fn runtime(&self, origin: &Path) -> Result<AuthRuntime, ConfigError> {
        let Some(jwt_secret) = self.jwt_secret.as_ref() else {
            return Err(missing_values(origin, vec![JWT_SECRET_REQUIRED.to_owned()]));
        };
        Ok(AuthRuntime {
            jwt_secret: jwt_secret.clone(),
            access_ttl_seconds: self.access_ttl_seconds,
            refresh_ttl_seconds: self.refresh_ttl_seconds,
            default_author_id: self.default_author_id,
        })
    }
}

/// `[auth]` with the mandatory fields resolved, as a binary that issues tokens needs them.
#[derive(Clone, Debug)]
pub struct AuthRuntime {
    pub jwt_secret: Secret,
    pub access_ttl_seconds: i64,
    pub refresh_ttl_seconds: i64,
    pub default_author_id: Option<Uuid>,
}

/// Rendering of the tracing subscriber's output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogFormat {
    /// One JSON object per event, for log shippers.
    #[default]
    Json,
    /// Human readable lines, for a terminal.
    Text,
}

/// Stream the tracing subscriber writes to.
///
/// Defaults to stderr. Stderr is never a data channel, so a log line written there can never
/// corrupt whatever a binary writes to stdout, while every container runtime and init system
/// this project deploys under captures the two streams identically — stderr costs nothing and
/// removes a whole class of failure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogOutput {
    #[default]
    Stderr,
    Stdout,
}

impl LogOutput {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stderr => "stderr",
            Self::Stdout => "stdout",
        }
    }
}

/// What the calling binary's stdout carries, which decides whether it may also carry logs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StdoutRole {
    /// Nothing else is written to stdout, so `logging.output` decides freely.
    #[default]
    Free,
    /// Stdout is a protocol channel — the MCP server's stdio JSON-RPC framing. A single log line
    /// on it corrupts a frame and breaks the session, so logs go to stderr whatever the file says.
    Protocol,
}

/// `[logging]` — replaces `RUST_LOG`.
#[derive(Clone, Debug, Default)]
pub struct LoggingConfig {
    /// `tracing_subscriber` filter directives. `None` means "this binary's own default".
    pub filter: Option<String>,
    pub format: LogFormat,
    /// Requested stream. Honoured only where stdout is not already spoken for; see
    /// [`Self::effective_output`].
    pub output: LogOutput,
}

impl LoggingConfig {
    /// The filter to install, falling back to a per-binary default.
    pub fn filter_or_default(&self, service_name: &str) -> String {
        self.filter
            .clone()
            .unwrap_or_else(|| format!("{service_name}=info,tower_http=info"))
    }

    /// The stream to actually write to, given what the caller's stdout carries.
    ///
    /// The configured value is a preference, not a promise: a process whose stdout carries a
    /// protocol has no stdout to log to, and an operator who writes `output = "stdout"` into a
    /// file shared by three binaries must not be able to break the MCP server's framing with it.
    pub const fn effective_output(&self, stdout: StdoutRole) -> LogOutput {
        match stdout {
            StdoutRole::Free => self.output,
            StdoutRole::Protocol => LogOutput::Stderr,
        }
    }
}

/// Which object storage implementation `[storage]` selects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StorageBackend {
    /// Files under [`StorageConfig::dir`].
    #[default]
    Local,
    /// An S3 compatible service described by [`StorageConfig::s3`].
    S3,
}

/// `[storage]` — where uploaded objects live.
#[derive(Clone, Debug)]
pub struct StorageConfig {
    pub backend: StorageBackend,
    /// Root directory of the local backend. Ignored when `backend` is `s3`.
    pub dir: PathBuf,
    /// Present and validated whenever `backend` is `s3`.
    pub s3: Option<S3Config>,
}

/// `[storage.s3]` — credentials and addressing for the S3 compatible backend.
#[derive(Clone, Debug)]
pub struct S3Config {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key_id: Secret,
    pub secret_access_key: Secret,
    pub session_token: Option<Secret>,
}

/// `[migrations]` — the two escape hatches of the migration runner.
#[derive(Clone, Copy, Debug, Default)]
pub struct MigrationsConfig {
    /// Re-execute every migration once, reporting failures without aborting.
    pub replay: bool,
    /// Start even though a migration failed or the schema check found a gap.
    pub continue_on_error: bool,
}

/// `[outbound]` — what the delivery pipeline is allowed to call.
#[derive(Clone, Debug, Default)]
pub struct OutboundConfig {
    /// Hosts (`host` or `host:port`) exempt from the private address checks.
    pub allowed_hosts: Vec<String>,
    /// Disables the private address checks entirely. Only for closed networks.
    pub allow_private: bool,
}

impl OutboundConfig {
    /// The allowlist in the comma separated form the host matcher consumes.
    pub fn allowlist_csv(&self) -> String {
        self.allowed_hosts.join(",")
    }
}

/// Transport the MCP server serves on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum McpTransport {
    /// A pipe pair owned by the parent process.
    #[default]
    Stdio,
    Http,
    Sse,
}

impl McpTransport {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
            Self::Sse => "sse",
        }
    }

    /// Whether the transport opens a socket that unrelated callers can reach.
    pub const fn is_networked(self) -> bool {
        matches!(self, Self::Http | Self::Sse)
    }
}

/// `[mcp]` — MCP server settings.
///
/// Every field is optional so that the API and the worker can share the file without declaring a
/// section they do not use. A field that *is* present has already been checked for shape;
/// [`OpenPrConfig::mcp_runtime`] checks that the mandatory ones are there.
#[derive(Clone, Debug, Default)]
pub struct McpConfig {
    pub api_url: Option<String>,
    pub bot_token: Option<Secret>,
    pub workspace_id: Option<Uuid>,
    /// Bearer token inbound HTTP/SSE callers must present.
    pub auth_token: Option<Secret>,
    pub transport: McpTransport,
    pub bind_addr: Option<String>,
    /// Correlation id stamped onto API calls made on behalf of one agent invocation.
    pub invocation_id: Option<String>,
}

impl McpConfig {
    fn runtime(&self, origin: &Path) -> Result<McpRuntime, ConfigError> {
        let mut issues = Vec::new();
        if self.bot_token.is_none() {
            issues.push("mcp.bot_token is required to run the MCP server".to_string());
        }
        if self.workspace_id.is_none() {
            issues.push("mcp.workspace_id is required to run the MCP server".to_string());
        }
        let (Some(bot_token), Some(workspace_id)) = (self.bot_token.as_ref(), self.workspace_id) else {
            return Err(ConfigError::Invalid {
                path: origin.to_path_buf(),
                issues,
            });
        };
        Ok(McpRuntime {
            api_url: self.api_url.clone().unwrap_or_else(|| DEFAULT_MCP_API_URL.to_string()),
            bot_token: bot_token.clone(),
            workspace_id,
            auth_token: self.auth_token.clone(),
            transport: self.transport,
            bind_addr: self
                .bind_addr
                .clone()
                .unwrap_or_else(|| DEFAULT_MCP_BIND_ADDR.to_string()),
            invocation_id: self.invocation_id.clone(),
        })
    }
}

/// `[mcp]` with the mandatory fields resolved, as the MCP server needs them.
#[derive(Clone, Debug)]
pub struct McpRuntime {
    pub api_url: String,
    pub bot_token: Secret,
    pub workspace_id: Uuid,
    pub auth_token: Option<Secret>,
    pub transport: McpTransport,
    pub bind_addr: String,
    pub invocation_id: Option<String>,
}

/// `[connectors]` — credentials the delivery pipeline may present to third parties.
#[derive(Clone, Debug, Default)]
pub struct ConnectorsConfig {
    pub secrets: ConnectorSecrets,
}

/// The subset of the configuration carried in `AppState` and reached from request handlers.
///
/// `Debug` is written by hand rather than derived so that adding a plain `String` credential to
/// this struct in the future cannot start printing it.
#[derive(Clone)]
pub struct AppConfig {
    pub app_name: String,
    pub bind_addr: String,
    pub database_url: Secret,
    pub jwt_secret: Secret,
    pub jwt_access_ttl_seconds: i64,
    pub jwt_refresh_ttl_seconds: i64,
    pub default_author_id: Option<Uuid>,
}

impl AppConfig {
    /// Projects a validated file onto the fields request handlers use.
    ///
    /// `default_name` and `default_bind` are the calling binary's own fallbacks, used when
    /// `[server]` does not name them — one file serves three binaries, so the file cannot carry a
    /// single correct answer for either.
    ///
    /// Fails when the file carries no database URL or no signing key. This is the API's and the
    /// worker's declaration that they need both, so it reports both at once rather than sending
    /// the operator round the loop twice.
    pub fn from_config(config: &OpenPrConfig, default_name: &str, default_bind: &str) -> Result<Self, ConfigError> {
        let (Some(database_url), Some(jwt_secret)) = (config.database.url.as_ref(), config.auth.jwt_secret.as_ref())
        else {
            let issues = [config.database.missing(), config.auth.missing()]
                .into_iter()
                .flatten()
                .map(str::to_owned)
                .collect();
            return Err(missing_values(&config.origin, issues));
        };
        Ok(Self {
            app_name: config
                .server
                .app_name
                .clone()
                .unwrap_or_else(|| default_name.to_string()),
            bind_addr: config
                .server
                .bind_addr
                .clone()
                .unwrap_or_else(|| default_bind.to_string()),
            database_url: database_url.clone(),
            jwt_secret: jwt_secret.clone(),
            jwt_access_ttl_seconds: config.auth.access_ttl_seconds,
            jwt_refresh_ttl_seconds: config.auth.refresh_ttl_seconds,
            default_author_id: config.auth.default_author_id,
        })
    }
}

impl std::fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppConfig")
            .field("app_name", &self.app_name)
            .field("bind_addr", &self.bind_addr)
            .field("database_url", &REDACTED)
            .field("jwt_secret", &REDACTED)
            .field("jwt_access_ttl_seconds", &self.jwt_access_ttl_seconds)
            .field("jwt_refresh_ttl_seconds", &self.jwt_refresh_ttl_seconds)
            .field("default_author_id", &self.default_author_id)
            .finish()
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic,
    clippy::nursery
)]
mod tests;
