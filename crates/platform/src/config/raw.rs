//! The serde layer, and the validation that turns it into a usable [`OpenPrConfig`].
//!
//! Everything here is optional at the deserialization level even when it is mandatory in
//! practice. serde stops at the first missing field, which would make an operator restart the
//! service once per missing value; validation instead walks the whole file and reports every
//! unusable value in one error.
//!
//! Validation checks the *shape* of every value that is present and refuses the file if any of
//! them is unusable. It does not check that `database.url` or `auth.jwt_secret` are present at
//! all: one file serves three binaries and the MCP server needs neither, so presence is checked
//! by the binary that needs the value, through the runtime accessors on [`OpenPrConfig`].
//!
//! None of these types derive `Debug`: they hold credential material as plain `String`, and only
//! the validated types wrap it in [`Secret`].

use std::path::{Path, PathBuf};

use serde::Deserialize;
use uuid::Uuid;

use super::secret::Secret;
use super::{
    AuditConfig, AuthConfig, ConfigError, DEFAULT_OPERATION_LOG_RETENTION_DAYS, DEFAULT_S3_REGION, DEFAULT_STORAGE_DIR,
    DatabaseConfig, LogFormat, LogOutput, LoggingConfig, MIN_JWT_SECRET_LEN, McpConfig, McpTransport, MigrationsConfig,
    OpenPrConfig, OutboundConfig, S3Config, ServerConfig, StorageBackend, StorageConfig,
};

const DEFAULT_MAX_CONNECTIONS: u32 = 20;
const DEFAULT_MIN_CONNECTIONS: u32 = 2;
const DEFAULT_CONNECT_TIMEOUT_SECONDS: u64 = 5;
const DEFAULT_IDLE_TIMEOUT_SECONDS: u64 = 30;
const DEFAULT_ACQUIRE_TIMEOUT_SECONDS: u64 = 5;
const DEFAULT_ACCESS_TTL_SECONDS: i64 = 1_296_000;
const DEFAULT_REFRESH_TTL_SECONDS: i64 = 1_728_000;

/// Substrings that mark a value as a template that was never filled in.
///
/// Carried over from the environment variable era: `${...}` is an unexpanded shell or compose
/// interpolation, and the other two are the placeholders the shipped examples use.
const PLACEHOLDER_MARKERS: &[&str] = &["${", "replace_with", "change-me-in-production"];

/// Whether a value is blank or still carries an example placeholder.
pub(super) fn is_placeholder(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.is_empty() || PLACEHOLDER_MARKERS.iter().any(|marker| trimmed.contains(marker))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct RawConfig {
    #[serde(default)]
    server: RawServer,
    #[serde(default)]
    database: RawDatabase,
    #[serde(default)]
    auth: RawAuth,
    #[serde(default)]
    logging: RawLogging,
    #[serde(default)]
    storage: RawStorage,
    #[serde(default)]
    audit: RawAudit,
    #[serde(default)]
    migrations: RawMigrations,
    #[serde(default)]
    outbound: RawOutbound,
    #[serde(default)]
    mcp: RawMcp,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawServer {
    app_name: Option<String>,
    bind_addr: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawDatabase {
    url: Option<String>,
    max_connections: Option<u32>,
    min_connections: Option<u32>,
    connect_timeout_seconds: Option<u64>,
    idle_timeout_seconds: Option<u64>,
    acquire_timeout_seconds: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawAuth {
    jwt_secret: Option<String>,
    access_ttl_seconds: Option<i64>,
    refresh_ttl_seconds: Option<i64>,
    default_author_id: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawLogging {
    filter: Option<String>,
    format: Option<String>,
    output: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawStorage {
    backend: Option<String>,
    dir: Option<String>,
    #[serde(default)]
    s3: Option<RawS3>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawAudit {
    operation_log_retention_days: Option<u32>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawS3 {
    endpoint: Option<String>,
    bucket: Option<String>,
    region: Option<String>,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    session_token: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawMigrations {
    replay: Option<bool>,
    continue_on_error: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawOutbound {
    allowed_hosts: Option<Vec<String>>,
    allow_private: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawMcp {
    api_url: Option<String>,
    bot_token: Option<String>,
    workspace_id: Option<String>,
    /// Retired. Kept only so that a file still carrying it gets an explanation instead of
    /// `deny_unknown_fields`' bare "unknown field" — the key used to be load bearing, and an
    /// operator who wrote it deserves to be told what replaced it.
    auth_token: Option<String>,
    transport: Option<String>,
    bind_addr: Option<String>,
}

/// Collects validation problems so the operator sees all of them at once.
struct Issues(Vec<String>);

impl Issues {
    const fn new() -> Self {
        Self(Vec::new())
    }

    fn push(&mut self, issue: impl Into<String>) {
        self.0.push(issue.into());
    }

    /// Records `Err` and yields `None`, so validation can keep walking the file.
    fn record<T>(&mut self, result: Result<T, String>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(issue) => {
                self.0.push(issue);
                None
            }
        }
    }

    fn into_result(self, path: &Path) -> Result<(), ConfigError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(ConfigError::Invalid {
                path: path.to_path_buf(),
                issues: self.0,
            })
        }
    }
}

impl RawConfig {
    pub(super) fn validate(self, origin: &Path) -> Result<OpenPrConfig, ConfigError> {
        let mut issues = Issues::new();

        let server = self.server.validate(&mut issues);
        let database = self.database.validate(&mut issues);
        let auth = self.auth.validate(&mut issues);
        let logging = self.logging.validate(&mut issues);
        let storage = self.storage.validate(&mut issues);
        let audit = self.audit.validate(&mut issues);
        let migrations = self.migrations.validate();
        let outbound = self.outbound.validate(&mut issues);
        let mcp = self.mcp.validate(&mut issues);

        issues.into_result(origin)?;

        // `storage` is `None` only in cases that already recorded an issue, so this point is only
        // reached when it is `Some`.
        let Some(storage) = storage else {
            return Err(ConfigError::Invalid {
                path: origin.to_path_buf(),
                issues: vec!["configuration could not be assembled from the validated sections".to_string()],
            });
        };

        Ok(OpenPrConfig {
            origin: origin.to_path_buf(),
            server,
            database,
            auth,
            logging,
            storage,
            audit,
            migrations,
            outbound,
            mcp,
        })
    }
}

impl RawServer {
    fn validate(self, issues: &mut Issues) -> ServerConfig {
        let app_name = self.app_name.and_then(|name| {
            issues.record(if is_placeholder(&name) {
                Err("server.app_name must be a concrete name".to_string())
            } else {
                Ok(name.trim().to_string())
            })
        });
        let bind_addr = self.bind_addr.and_then(|addr| {
            issues.record(validate_bind_addr("server.bind_addr", &addr).map(|()| addr.trim().to_string()))
        });
        ServerConfig { app_name, bind_addr }
    }
}

impl RawDatabase {
    /// Checks the shape of whatever is present.
    ///
    /// A missing `url` is not an issue here: only a binary that opens a connection can say
    /// whether it is missing, and it says so through `OpenPrConfig::database_runtime`.
    fn validate(self, issues: &mut Issues) -> DatabaseConfig {
        let url = match self.url {
            None => None,
            Some(url) if is_placeholder(&url) => {
                issues.push(
                    "database.url must be a concrete database URL, not a placeholder or an unexpanded ${...} \
                     template",
                );
                None
            }
            Some(url) => Some(Secret::new(url.trim())),
        };

        let max_connections = self.max_connections.unwrap_or(DEFAULT_MAX_CONNECTIONS);
        let min_connections = self.min_connections.unwrap_or(DEFAULT_MIN_CONNECTIONS);
        if max_connections == 0 {
            issues.push("database.max_connections must be at least 1");
        }
        if min_connections > max_connections {
            issues.push(format!(
                "database.min_connections ({min_connections}) must not exceed database.max_connections \
                 ({max_connections})"
            ));
        }
        let connect_timeout_seconds = positive_seconds(
            issues,
            "database.connect_timeout_seconds",
            self.connect_timeout_seconds,
            DEFAULT_CONNECT_TIMEOUT_SECONDS,
        );
        let idle_timeout_seconds = positive_seconds(
            issues,
            "database.idle_timeout_seconds",
            self.idle_timeout_seconds,
            DEFAULT_IDLE_TIMEOUT_SECONDS,
        );
        let acquire_timeout_seconds = positive_seconds(
            issues,
            "database.acquire_timeout_seconds",
            self.acquire_timeout_seconds,
            DEFAULT_ACQUIRE_TIMEOUT_SECONDS,
        );

        DatabaseConfig {
            url,
            max_connections,
            min_connections,
            connect_timeout_seconds,
            idle_timeout_seconds,
            acquire_timeout_seconds,
        }
    }
}

fn positive_seconds(issues: &mut Issues, field: &str, value: Option<u64>, default: u64) -> u64 {
    match value {
        None => default,
        Some(0) => {
            issues.push(format!("{field} must be at least 1 second"));
            default
        }
        Some(seconds) => seconds,
    }
}

impl RawAuth {
    /// Checks the shape of whatever is present.
    ///
    /// A missing `jwt_secret` is not an issue here, for the reason given on
    /// [`RawDatabase::validate`]; `OpenPrConfig::auth_runtime` reports it instead.
    fn validate(self, issues: &mut Issues) -> AuthConfig {
        let jwt_secret = match self.jwt_secret {
            None => None,
            Some(secret) if is_placeholder(&secret) => {
                issues.push(
                    "auth.jwt_secret must be a concrete deployment secret, not a placeholder or an unexpanded \
                     ${...} template; generate one with `openssl rand -hex 32`",
                );
                None
            }
            Some(secret) if secret.trim().len() < MIN_JWT_SECRET_LEN => {
                issues.push(format!(
                    "auth.jwt_secret must be at least {MIN_JWT_SECRET_LEN} characters; every token in the \
                     deployment is signed with it"
                ));
                None
            }
            Some(secret) => Some(Secret::new(secret.trim())),
        };

        let access_ttl_seconds = positive_ttl(
            issues,
            "auth.access_ttl_seconds",
            self.access_ttl_seconds,
            DEFAULT_ACCESS_TTL_SECONDS,
        );
        let refresh_ttl_seconds = positive_ttl(
            issues,
            "auth.refresh_ttl_seconds",
            self.refresh_ttl_seconds,
            DEFAULT_REFRESH_TTL_SECONDS,
        );

        let default_author_id = self
            .default_author_id
            .and_then(|raw| issues.record(parse_workspace_scale_uuid("auth.default_author_id", &raw)));

        AuthConfig {
            jwt_secret,
            access_ttl_seconds,
            refresh_ttl_seconds,
            default_author_id,
        }
    }
}

fn positive_ttl(issues: &mut Issues, field: &str, value: Option<i64>, default: i64) -> i64 {
    match value {
        None => default,
        Some(seconds) if seconds <= 0 => {
            issues.push(format!("{field} must be greater than 0"));
            default
        }
        Some(seconds) => seconds,
    }
}

impl RawLogging {
    fn validate(self, issues: &mut Issues) -> LoggingConfig {
        let filter = self.filter.and_then(|filter| {
            issues.record(
                tracing_subscriber::EnvFilter::try_new(filter.trim())
                    .map(|_| filter.trim().to_string())
                    .map_err(|err| format!("logging.filter is not a valid tracing filter: {err}")),
            )
        });
        let format = self.format.map_or_else(
            || Some(LogFormat::default()),
            |raw| {
                issues.record(match raw.trim().to_ascii_lowercase().as_str() {
                    "json" => Ok(LogFormat::Json),
                    "text" | "pretty" | "plain" => Ok(LogFormat::Text),
                    other => Err(format!("logging.format must be json or text, got {other}")),
                })
            },
        );
        let output = self.output.map_or_else(
            || Some(LogOutput::default()),
            |raw| {
                issues.record(match raw.trim().to_ascii_lowercase().as_str() {
                    "stderr" => Ok(LogOutput::Stderr),
                    "stdout" => Ok(LogOutput::Stdout),
                    other => Err(format!("logging.output must be stderr or stdout, got {other}")),
                })
            },
        );
        LoggingConfig {
            filter,
            format: format.unwrap_or_default(),
            output: output.unwrap_or_default(),
        }
    }
}

impl RawStorage {
    fn validate(self, issues: &mut Issues) -> Option<StorageConfig> {
        let backend = self.backend.map_or_else(
            || Some(StorageBackend::default()),
            |raw| {
                issues.record(match raw.trim().to_ascii_lowercase().as_str() {
                    "local" | "filesystem" | "fs" => Ok(StorageBackend::Local),
                    "s3" | "s3-compatible" | "s3_compatible" => Ok(StorageBackend::S3),
                    other => Err(format!("storage.backend must be local or s3, got {other}")),
                })
            },
        );

        let dir = match self.dir {
            None => PathBuf::from(DEFAULT_STORAGE_DIR),
            Some(dir) if dir.trim().is_empty() => {
                issues.push("storage.dir must not be empty");
                PathBuf::from(DEFAULT_STORAGE_DIR)
            }
            Some(dir) => PathBuf::from(dir.trim()),
        };

        let backend = backend?;
        let s3 = match (backend, self.s3) {
            (StorageBackend::S3, None) => {
                issues.push("storage.backend is s3, so the [storage.s3] section is required");
                None
            }
            (StorageBackend::S3, Some(raw)) => raw.validate(issues),
            // A leftover [storage.s3] section under the local backend is harmless and left unread,
            // so an operator can switch backends by editing one line.
            (StorageBackend::Local, _) => None,
        };
        if backend == StorageBackend::S3 && s3.is_none() {
            return None;
        }

        Some(StorageConfig { backend, dir, s3 })
    }
}

impl RawAudit {
    fn validate(self, issues: &mut Issues) -> AuditConfig {
        let operation_log_retention_days =
            self.operation_log_retention_days
                .map_or(DEFAULT_OPERATION_LOG_RETENTION_DAYS, |days| {
                    if (1..=3_650).contains(&days) {
                        days
                    } else {
                        issues.push("audit.operation_log_retention_days must be between 1 and 3650");
                        DEFAULT_OPERATION_LOG_RETENTION_DAYS
                    }
                });
        AuditConfig {
            operation_log_retention_days,
        }
    }
}

impl RawS3 {
    fn validate(self, issues: &mut Issues) -> Option<S3Config> {
        let endpoint = match self.endpoint {
            None => {
                issues.push("storage.s3.endpoint is required when storage.backend is s3");
                None
            }
            Some(endpoint) => issues.record(
                validate_http_url("storage.s3.endpoint", &endpoint)
                    .map(|()| endpoint.trim().trim_end_matches('/').to_string()),
            ),
        };
        let bucket = match self.bucket {
            None => {
                issues.push("storage.s3.bucket is required when storage.backend is s3");
                None
            }
            Some(bucket) => issues.record(validate_s3_bucket(&bucket).map(|()| bucket.trim().to_string())),
        };
        let region = self.region.map_or_else(
            || Some(DEFAULT_S3_REGION.to_string()),
            |region| {
                issues.record(if is_placeholder(&region) {
                    Err("storage.s3.region must be a concrete region".to_string())
                } else {
                    Ok(region.trim().to_string())
                })
            },
        );
        let access_key_id = required_secret(issues, "storage.s3.access_key_id", self.access_key_id);
        let secret_access_key = required_secret(issues, "storage.s3.secret_access_key", self.secret_access_key);
        let session_token = self.session_token.and_then(|token| {
            issues.record(if is_placeholder(&token) {
                Err("storage.s3.session_token must be a concrete token when it is set".to_string())
            } else {
                Ok(Secret::new(token.trim()))
            })
        });

        Some(S3Config {
            endpoint: endpoint?,
            bucket: bucket?,
            region: region?,
            access_key_id: access_key_id?,
            secret_access_key: secret_access_key?,
            session_token,
        })
    }
}

fn required_secret(issues: &mut Issues, field: &str, value: Option<String>) -> Option<Secret> {
    match value {
        None => {
            issues.push(format!("{field} is required"));
            None
        }
        Some(raw) if is_placeholder(&raw) => {
            issues.push(format!("{field} must be a concrete value, not a placeholder"));
            None
        }
        Some(raw) => Some(Secret::new(raw.trim())),
    }
}

impl RawMigrations {
    fn validate(self) -> MigrationsConfig {
        MigrationsConfig {
            replay: self.replay.unwrap_or(false),
            continue_on_error: self.continue_on_error.unwrap_or(false),
        }
    }
}

impl RawOutbound {
    fn validate(self, issues: &mut Issues) -> OutboundConfig {
        let allowed_hosts = self
            .allowed_hosts
            .unwrap_or_default()
            .into_iter()
            .filter_map(|entry| issues.record(validate_allowlist_entry(&entry)))
            .collect();
        OutboundConfig {
            allowed_hosts,
            allow_private: self.allow_private.unwrap_or(false),
        }
    }
}

/// Checks one `outbound.allowed_hosts` entry.
///
/// The matcher compares `host` or `host:port` literally, so a URL, a path or a wildcard would
/// silently never match and quietly leave the target blocked.
fn validate_allowlist_entry(entry: &str) -> Result<String, String> {
    let trimmed = entry.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return Err("outbound.allowed_hosts must not contain empty entries".to_string());
    }
    if trimmed.contains("://") || trimmed.contains('/') {
        return Err(format!(
            "outbound.allowed_hosts entry {entry} must be a host or host:port, not a URL"
        ));
    }
    if trimmed.contains('*') {
        return Err(format!(
            "outbound.allowed_hosts entry {entry} uses a wildcard, which is matched literally and would never \
             allow anything"
        ));
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err(format!(
            "outbound.allowed_hosts entry {entry} must not contain whitespace"
        ));
    }
    if let Some((host, port)) = split_host_port(&trimmed) {
        validate_port(&format!("outbound.allowed_hosts entry {entry}"), port)?;
        if host.is_empty() {
            return Err(format!("outbound.allowed_hosts entry {entry} names no host"));
        }
    }
    Ok(trimmed)
}

impl RawMcp {
    fn validate(self, issues: &mut Issues) -> McpConfig {
        let api_url = self
            .api_url
            .and_then(|url| issues.record(validate_http_url("mcp.api_url", &url).map(|()| url.trim().to_string())));
        let bot_token = self.bot_token.and_then(|token| {
            issues.record(if is_placeholder(&token) {
                Err("mcp.bot_token must be a concrete bot token".to_string())
            } else if token.trim().starts_with("opr_") {
                Ok(Secret::new(token.trim()))
            } else {
                Err("mcp.bot_token must use the opr_ token prefix".to_string())
            })
        });
        let workspace_id = self
            .workspace_id
            .and_then(|raw| issues.record(parse_workspace_scale_uuid("mcp.workspace_id", &raw)));
        if self.auth_token.is_some() {
            // The value is never echoed, not even its length.
            issues.push(
                "mcp.auth_token has been removed: an inbound http/sse caller now presents its own workspace bot token \
                 in `Authorization: Bearer opr_...`, which the MCP server forwards to the API unchanged and the API \
                 authenticates. Delete the key; there is no shared inbound secret any more, and a request without a \
                 caller bot token is refused",
            );
        }
        let transport = self.transport.map_or_else(
            || Some(McpTransport::default()),
            |raw| {
                issues.record(match raw.trim().to_ascii_lowercase().as_str() {
                    "stdio" => Ok(McpTransport::Stdio),
                    "http" => Ok(McpTransport::Http),
                    "sse" => Ok(McpTransport::Sse),
                    other => Err(format!("mcp.transport must be stdio, http or sse, got {other}")),
                })
            },
        );
        let bind_addr = self.bind_addr.and_then(|addr| {
            issues.record(validate_bind_addr("mcp.bind_addr", &addr).map(|()| addr.trim().to_string()))
        });
        McpConfig {
            api_url,
            bot_token,
            workspace_id,
            transport: transport.unwrap_or_default(),
            bind_addr,
        }
    }
}

/// Parses a UUID that identifies a tenant scoped object, rejecting the nil placeholder.
///
/// The nil UUID is what an unfilled template or a defaulted struct produces. Accepting it would
/// create a workspace-shaped identity that no real row ever has, and any credential filed under
/// it would be reachable by anything else that also failed to be configured.
fn parse_workspace_scale_uuid(field: &str, raw: &str) -> Result<Uuid, String> {
    let trimmed = raw.trim();
    if is_placeholder(trimmed) {
        return Err(format!("{field} must be a concrete UUID, not a placeholder"));
    }
    let parsed = Uuid::parse_str(trimmed).map_err(|err| format!("{field} is not a valid UUID: {err}"))?;
    if parsed.is_nil() {
        return Err(format!("{field} must not be the nil UUID placeholder"));
    }
    Ok(parsed)
}

/// Checks that a value is an absolute `http`/`https` URL with a host.
///
/// Deliberately hand rolled: `platform` has no URL parser and one is not worth adding for a check
/// this shallow.
fn validate_http_url(field: &str, value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if is_placeholder(trimmed) {
        return Err(format!("{field} must be a concrete URL, not a placeholder"));
    }
    let rest = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .ok_or_else(|| format!("{field} must start with http:// or https://"))?;
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|authority| !authority.is_empty())
        .ok_or_else(|| format!("{field} names no host"))?;
    if authority.chars().any(char::is_whitespace) {
        return Err(format!("{field} must not contain whitespace"));
    }
    if let Some((host, port)) = split_host_port(authority) {
        validate_port(field, port)?;
        if host.is_empty() {
            return Err(format!("{field} names no host"));
        }
    }
    Ok(())
}

/// Checks that a value is a `host:port` a listener can be bound to.
fn validate_bind_addr(field: &str, value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if is_placeholder(trimmed) {
        return Err(format!("{field} must be a concrete host:port, not a placeholder"));
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err(format!("{field} must not contain whitespace"));
    }
    let (host, port) =
        split_host_port(trimmed).ok_or_else(|| format!("{field} must be host:port, for example 0.0.0.0:8081"))?;
    if host.is_empty() {
        return Err(format!("{field} names no host"));
    }
    validate_port(field, port)
}

/// Splits an authority into host and port, tolerating bracketed IPv6 literals.
///
/// Returns `None` when there is no port at all.
fn split_host_port(authority: &str) -> Option<(&str, &str)> {
    if let Some(end) = authority.rfind(']') {
        let after = authority.get(end + 1..)?;
        let port = after.strip_prefix(':')?;
        return Some((authority.get(..=end)?, port));
    }
    let (host, port) = authority.rsplit_once(':')?;
    Some((host, port))
}

fn validate_port(field: &str, port: &str) -> Result<(), String> {
    match port.parse::<u16>() {
        Ok(0) | Err(_) => Err(format!("{field} has an invalid port {port}, expected 1-65535")),
        Ok(_) => Ok(()),
    }
}

/// Checks a bucket name against the naming rules every S3 compatible service shares.
fn validate_s3_bucket(bucket: &str) -> Result<(), String> {
    let trimmed = bucket.trim();
    if is_placeholder(trimmed) {
        return Err("storage.s3.bucket must be a concrete bucket name, not a placeholder".to_string());
    }
    if !(3..=63).contains(&trimmed.len()) {
        return Err(format!(
            "storage.s3.bucket {trimmed} must be between 3 and 63 characters"
        ));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
    {
        return Err(format!(
            "storage.s3.bucket {trimmed} may only contain lowercase letters, digits, dots and dashes"
        ));
    }
    let starts_ok = trimmed.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit());
    let ends_ok = trimmed.ends_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit());
    if !starts_ok || !ends_ok {
        return Err(format!(
            "storage.s3.bucket {trimmed} must start and end with a lowercase letter or digit"
        ));
    }
    Ok(())
}
