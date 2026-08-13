//! Behavioural tests for the file backed configuration.

use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::{
    AppConfig, ConfigError, DEFAULT_CONFIG_PATH, LogFormat, LogOutput, McpTransport, OpenPrConfig, REDACTED,
    StdoutRole, StorageBackend,
};

const WORKSPACE: &str = "0f8a1b2c-3d4e-4f60-8182-93a4b5c6d7e8";
const OTHER_WORKSPACE: &str = "11111111-2222-4333-8444-555555555555";

fn origin() -> PathBuf {
    PathBuf::from("config/openpr.toml")
}

fn parse(source: &str) -> Result<OpenPrConfig, ConfigError> {
    OpenPrConfig::parse(source, &origin())
}

fn issues(source: &str) -> Vec<String> {
    match parse(source) {
        Ok(_) => panic!("configuration should not have validated"),
        Err(ConfigError::Invalid { issues, .. }) => issues,
        Err(other) => panic!("expected a validation failure, got {other}"),
    }
}

/// A file that exercises every section, used as the base for the negative cases.
fn full_config() -> String {
    format!(
        r#"
[server]
app_name = "api"
bind_addr = "0.0.0.0:8081"

[database]
url = "postgres://openpr:s3cret@localhost:5432/openpr"
max_connections = 40
min_connections = 4
connect_timeout_seconds = 7
idle_timeout_seconds = 45
acquire_timeout_seconds = 6

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"
access_ttl_seconds = 3600
refresh_ttl_seconds = 604800
default_author_id = "a1b2c3d4-e5f6-4890-abcd-ef1234567890"

[logging]
filter = "api=debug,tower_http=info"
format = "text"
output = "stdout"

[storage]
backend = "s3"
dir = "./uploads"

[storage.s3]
endpoint = "https://s3.example.com"
bucket = "openpr-uploads"
region = "eu-central-1"
access_key_id = "AKIAEXAMPLE"
secret_access_key = "s3-secret-access-key"
session_token = "s3-session-token"

[migrations]
replay = true
continue_on_error = true

[outbound]
allowed_hosts = ["worker.internal", "hooks.example.com:8443"]
allow_private = true

[mcp]
api_url = "http://api:8080"
bot_token = "opr_live_botexampletoken"
workspace_id = "{WORKSPACE}"
auth_token = "mcp-inbound-token-value"
transport = "http"
bind_addr = "0.0.0.0:8090"
invocation_id = "inv-42"

[connectors.secrets."{WORKSPACE}"]
SHIPPING = "shipping-credential"
PAYMENTS = "payments-credential"

[connectors.secrets."{OTHER_WORKSPACE}"]
PAYMENTS = "other-tenant-credential"
"#
    )
}

// ---- happy path ----

#[test]
fn a_complete_file_parses_into_every_section() {
    let config = parse(&full_config()).expect("the complete example should validate");

    assert_eq!(config.server.app_name.as_deref(), Some("api"));
    assert_eq!(config.server.bind_addr.as_deref(), Some("0.0.0.0:8081"));

    let database = config.database_runtime().expect("the database section is complete");
    assert_eq!(database.url.expose(), "postgres://openpr:s3cret@localhost:5432/openpr");
    assert_eq!(database.max_connections, 40);
    assert_eq!(database.min_connections, 4);
    assert_eq!(database.connect_timeout_seconds, 7);
    assert_eq!(database.idle_timeout_seconds, 45);
    assert_eq!(database.acquire_timeout_seconds, 6);

    let auth = config.auth_runtime().expect("the auth section is complete");
    assert_eq!(auth.jwt_secret.expose(), "0123456789abcdef0123456789abcdef");
    assert_eq!(auth.access_ttl_seconds, 3600);
    assert_eq!(auth.refresh_ttl_seconds, 604_800);
    assert_eq!(
        auth.default_author_id.map(|id| id.to_string()),
        Some("a1b2c3d4-e5f6-4890-abcd-ef1234567890".to_string())
    );

    assert_eq!(config.logging.filter.as_deref(), Some("api=debug,tower_http=info"));
    assert_eq!(config.logging.format, LogFormat::Text);
    assert_eq!(config.logging.output, LogOutput::Stdout);

    assert_eq!(config.storage.backend, StorageBackend::S3);
    let s3 = config.storage.s3.as_ref().expect("s3 section should be present");
    assert_eq!(s3.endpoint, "https://s3.example.com");
    assert_eq!(s3.bucket, "openpr-uploads");
    assert_eq!(s3.region, "eu-central-1");
    assert_eq!(s3.secret_access_key.expose(), "s3-secret-access-key");
    assert_eq!(
        s3.session_token.as_ref().map(super::Secret::expose),
        Some("s3-session-token")
    );

    assert!(config.migrations.replay);
    assert!(config.migrations.continue_on_error);

    assert_eq!(
        config.outbound.allowed_hosts,
        vec!["worker.internal", "hooks.example.com:8443"]
    );
    assert!(config.outbound.allow_private);
    assert_eq!(
        config.outbound.allowlist_csv(),
        "worker.internal,hooks.example.com:8443"
    );

    assert_eq!(config.mcp.transport, McpTransport::Http);
    assert!(config.mcp.transport.is_networked());
    let mcp = config.mcp_runtime().expect("mcp section is complete");
    assert_eq!(mcp.api_url, "http://api:8080");
    assert_eq!(mcp.bot_token.expose(), "opr_live_botexampletoken");
    assert_eq!(mcp.workspace_id.to_string(), WORKSPACE);
    assert_eq!(
        mcp.auth_token.as_ref().map(super::Secret::expose),
        Some("mcp-inbound-token-value")
    );
    assert_eq!(mcp.bind_addr, "0.0.0.0:8090");
    assert_eq!(mcp.invocation_id.as_deref(), Some("inv-42"));

    assert_eq!(config.connectors.secrets.workspace_count(), 2);
    assert_eq!(config.connectors.secrets.entry_count(), 3);
}

#[test]
fn a_minimal_file_falls_back_to_documented_defaults() {
    let config = parse(
        r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"
"#,
    )
    .expect("database and auth alone should be enough");

    assert!(config.server.app_name.is_none());
    assert!(config.server.bind_addr.is_none());
    let database = config.database_runtime().expect("the url alone is enough");
    assert_eq!(database.max_connections, 20);
    assert_eq!(database.min_connections, 2);
    assert_eq!(database.connect_timeout_seconds, 5);
    assert_eq!(database.idle_timeout_seconds, 30);
    assert_eq!(database.acquire_timeout_seconds, 5);
    let auth = config.auth_runtime().expect("the secret alone is enough");
    assert_eq!(auth.access_ttl_seconds, 1_296_000);
    assert_eq!(auth.refresh_ttl_seconds, 1_728_000);
    assert!(auth.default_author_id.is_none());
    assert_eq!(config.logging.format, LogFormat::Json);
    assert_eq!(config.logging.output, LogOutput::Stderr);
    assert_eq!(
        config.logging.filter_or_default("worker"),
        "worker=info,tower_http=info"
    );
    assert_eq!(config.storage.backend, StorageBackend::Local);
    assert_eq!(config.storage.dir, Path::new("./uploads"));
    assert!(!config.migrations.replay);
    assert!(!config.migrations.continue_on_error);
    assert!(config.outbound.allowed_hosts.is_empty());
    assert!(!config.outbound.allow_private);
    assert_eq!(config.mcp.transport, McpTransport::Stdio);
    assert_eq!(config.connectors.secrets.workspace_count(), 0);
}

// ---- one error report for every problem ----

#[test]
fn every_value_the_api_is_missing_is_reported_in_one_pass() {
    let config = parse("[server]\napp_name = \"api\"\n").expect("presence is not a load time question");
    let Err(ConfigError::Invalid { issues: reported, .. }) = AppConfig::from_config(&config, "api", "0.0.0.0:8081")
    else {
        panic!("the api must refuse to start without a database and a signing key");
    };
    assert!(
        reported.iter().any(|issue| issue.contains("database.url is required")),
        "{reported:?}"
    );
    assert!(
        reported
            .iter()
            .any(|issue| issue.contains("auth.jwt_secret is required")),
        "{reported:?}"
    );
    assert_eq!(reported.len(), 2, "exactly the two missing values, got {reported:?}");
}

#[test]
fn unrelated_problems_across_sections_are_all_reported_together() {
    let reported = issues(&format!(
        r#"
[server]
bind_addr = "0.0.0.0:not-a-port"

[database]
min_connections = 90
max_connections = 10
connect_timeout_seconds = 0

[auth]
jwt_secret = "short"
access_ttl_seconds = 0
default_author_id = "00000000-0000-0000-0000-000000000000"

[logging]
format = "yaml"

[storage]
backend = "gcs"

[outbound]
allowed_hosts = ["https://hooks.example.com/path", "*.example.com"]

[mcp]
api_url = "ftp://api"
bot_token = "not-prefixed"
workspace_id = "not-a-uuid"
auth_token = "short"
transport = "grpc"

[connectors.secrets."{WORKSPACE}"]
lowercase = "value"
"#
    ));

    for expected in [
        "server.bind_addr",
        "database.min_connections",
        "database.connect_timeout_seconds",
        "auth.jwt_secret must be at least",
        "auth.access_ttl_seconds",
        "auth.default_author_id must not be the nil UUID",
        "logging.format",
        "storage.backend",
        "outbound.allowed_hosts entry https://hooks.example.com/path",
        "outbound.allowed_hosts entry *.example.com",
        "mcp.api_url",
        "mcp.bot_token",
        "mcp.workspace_id",
        "mcp.auth_token must be at least",
        "mcp.transport",
        "connectors.secrets",
    ] {
        assert!(
            reported.iter().any(|issue| issue.contains(expected)),
            "missing {expected:?} in {reported:?}"
        );
    }
    assert!(reported.len() >= 16, "expected every problem at once, got {reported:?}");
}

#[test]
fn the_rendered_error_lists_all_issues_and_points_at_the_example() {
    let config = parse("[server]\napp_name = \"api\"\n").expect("presence is not a load time question");
    let rendered = AppConfig::from_config(&config, "api", "0.0.0.0:8081")
        .expect_err("should fail")
        .to_string();
    assert!(rendered.contains("2 unusable values"), "{rendered}");
    assert!(rendered.contains("database.url is required"), "{rendered}");
    assert!(rendered.contains("auth.jwt_secret is required"), "{rendered}");
    assert!(rendered.contains("config/openpr.example.toml"), "{rendered}");
    assert!(rendered.contains(&origin().display().to_string()), "{rendered}");
}

// ---- one file, three binaries: presence is the caller's question ----

#[test]
fn an_mcp_only_file_needs_neither_a_database_nor_a_signing_key() {
    let config = parse(&format!(
        r#"
[mcp]
bot_token = "opr_live_token"
workspace_id = "{WORKSPACE}"
"#
    ))
    .expect("an MCP-only deployment must not have to invent a database URL and a signing key");

    let mcp = config.mcp_runtime().expect("the mcp section is complete");
    assert_eq!(mcp.workspace_id.to_string(), WORKSPACE);
    assert_eq!(mcp.api_url, super::DEFAULT_MCP_API_URL);
    assert!(config.database.url.is_none());
    assert!(config.auth.jwt_secret.is_none());
}

#[test]
fn a_binary_that_opens_the_pool_still_refuses_a_file_without_a_database() {
    let config = parse(&format!(
        r#"
[mcp]
bot_token = "opr_live_token"
workspace_id = "{WORKSPACE}"
"#
    ))
    .expect("the file itself is valid");

    let Err(ConfigError::Invalid { issues, .. }) = config.database_runtime() else {
        panic!("database_runtime must refuse an absent [database] section");
    };
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert!(
        issues.iter().any(|issue| issue.contains("database.url is required")),
        "{issues:?}"
    );

    let Err(ConfigError::Invalid { issues, .. }) = config.auth_runtime() else {
        panic!("auth_runtime must refuse an absent [auth] section");
    };
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert!(
        issues.iter().any(|issue| issue.contains("auth.jwt_secret is required")),
        "{issues:?}"
    );
}

#[test]
fn a_present_but_unusable_database_url_is_still_refused_at_load_time() {
    // Laziness applies to presence only: a value that *is* written down is checked as before, so
    // a typo never survives until the first connection attempt.
    let reported = issues("[database]\nurl = \"postgres://openpr:${PGPASSWORD}@db/openpr\"\n");
    assert!(
        reported
            .iter()
            .any(|issue| issue.contains("database.url must be a concrete database URL")),
        "{reported:?}"
    );

    let reported = issues("[auth]\njwt_secret = \"short\"\n");
    assert!(
        reported
            .iter()
            .any(|issue| issue.contains("auth.jwt_secret must be at least")),
        "{reported:?}"
    );
}

#[test]
fn a_database_section_without_a_url_still_validates_its_pool_shape() {
    let reported = issues("[database]\nmin_connections = 90\nmax_connections = 10\n");
    assert!(
        reported.iter().any(|issue| issue.contains("database.min_connections")),
        "{reported:?}"
    );
    assert!(
        !reported.iter().any(|issue| issue.contains("database.url")),
        "an absent url is the caller's question, not the file's: {reported:?}"
    );
}

// ---- logging ----

#[test]
fn logging_writes_to_stderr_unless_the_file_asks_for_stdout() {
    let config = parse("[logging]\nformat = \"text\"\n").expect("logging alone is a valid file");
    assert_eq!(config.logging.output, LogOutput::Stderr);
    assert_eq!(config.logging.effective_output(StdoutRole::Free), LogOutput::Stderr);

    let config = parse("[logging]\noutput = \"stdout\"\n").expect("stdout is a valid choice");
    assert_eq!(config.logging.output, LogOutput::Stdout);
    assert_eq!(config.logging.effective_output(StdoutRole::Free), LogOutput::Stdout);
}

#[test]
fn a_stdio_transport_keeps_its_stdout_even_when_the_file_asks_for_stdout_logs() {
    let config = parse("[logging]\noutput = \"stdout\"\n").expect("stdout is a valid choice");
    assert_eq!(
        config.logging.effective_output(StdoutRole::Protocol),
        LogOutput::Stderr,
        "a log line on a JSON-RPC channel corrupts the frame, so the file cannot ask for it"
    );
}

#[test]
fn an_unknown_logging_output_is_refused_rather_than_guessed() {
    let reported = issues("[logging]\noutput = \"syslog\"\n");
    assert!(
        reported
            .iter()
            .any(|issue| issue.contains("logging.output must be stderr or stdout")),
        "{reported:?}"
    );
}

// ---- placeholders ----

#[test]
fn placeholder_values_are_refused() {
    for placeholder in [
        "",
        "   ",
        "replace_with_postgres_password",
        "${POSTGRES_PASSWORD:?set POSTGRES_PASSWORD}",
        "change-me-in-production",
    ] {
        let reported = issues(&format!(
            "[database]\nurl = \"{placeholder}\"\n\n[auth]\njwt_secret = \"0123456789abcdef0123456789abcdef\"\n"
        ));
        assert!(
            reported.iter().any(|issue| issue.contains("database.url")),
            "placeholder {placeholder:?} should have been refused, got {reported:?}"
        );
    }

    for placeholder in [
        "",
        "change-me-in-production",
        "replace_with_long_random_secret",
        "${JWT_SECRET:?set JWT_SECRET for OpenPR services}",
    ] {
        let reported = issues(&format!(
            "[database]\nurl = \"postgres://localhost/openpr\"\n\n[auth]\njwt_secret = \"{placeholder}\"\n"
        ));
        assert!(
            reported.iter().any(|issue| issue.contains("auth.jwt_secret")),
            "placeholder {placeholder:?} should have been refused, got {reported:?}"
        );
    }
}

#[test]
fn the_shipped_example_is_refused_because_it_is_all_placeholders() {
    let example = include_str!("../../../../config/openpr.example.toml");
    let reported = issues(example);
    assert!(
        reported.iter().any(|issue| issue.contains("database.url")),
        "{reported:?}"
    );
    assert!(
        reported.iter().any(|issue| issue.contains("auth.jwt_secret")),
        "{reported:?}"
    );
}

#[test]
fn the_shipped_example_is_structurally_valid_toml_for_this_schema() {
    // Placeholders must fail *validation*, not parsing: an operator who fills them in must get a
    // working file without having to also fix the shape.
    let example = include_str!("../../../../config/openpr.example.toml");
    let filled = example
        .replace("replace_with_postgres_password", "s3cret")
        .replace("replace_with_a_64_character_hex_secret", &"a".repeat(64))
        .replace("replace_with_s3_access_key_id", "AKIAEXAMPLE")
        .replace("replace_with_s3_secret_access_key", "s3-secret")
        .replace("replace_with_opr_bot_token", "opr_live_token")
        .replace("replace_with_mcp_inbound_token", "mcp-inbound-token-value")
        .replace("replace_with_connector_credential", "connector-credential");
    let config = parse(&filled).expect("the example should validate once the placeholders are replaced");
    let database = config
        .database_runtime()
        .expect("the filled in example names a database");
    assert_eq!(database.url.expose(), "postgres://openpr:s3cret@localhost:5432/openpr");
    AppConfig::from_config(&config, "api", "0.0.0.0:8081").expect("the filled in example is enough to start the api");
}

// ---- uuids ----

#[test]
fn malformed_and_nil_uuids_are_refused_everywhere_they_appear() {
    for field in ["auth.default_author_id", "mcp.workspace_id"] {
        for (value, marker) in [
            ("not-a-uuid", "not a valid UUID"),
            ("00000000-0000-0000-0000-000000000000", "nil UUID"),
            ("replace_with_workspace_uuid", "placeholder"),
        ] {
            let section = if field == "auth.default_author_id" {
                format!("[auth]\njwt_secret = \"0123456789abcdef0123456789abcdef\"\ndefault_author_id = \"{value}\"\n")
            } else {
                format!(
                    "[auth]\njwt_secret = \"0123456789abcdef0123456789abcdef\"\n\n[mcp]\nworkspace_id = \"{value}\"\n"
                )
            };
            let reported = issues(&format!(
                "[database]\nurl = \"postgres://localhost/openpr\"\n\n{section}"
            ));
            assert!(
                reported
                    .iter()
                    .any(|issue| issue.contains(field) && issue.contains(marker)),
                "{field} = {value:?} should be refused with {marker:?}, got {reported:?}"
            );
        }
    }
}

#[test]
fn a_connector_workspace_key_must_be_a_real_uuid() {
    let reported = issues(
        r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"

[connectors.secrets.not-a-uuid]
SHIPPING = "value"

[connectors.secrets."00000000-0000-0000-0000-000000000000"]
PAYMENTS = "value"
"#,
    );
    assert!(
        reported
            .iter()
            .any(|issue| issue.contains("not-a-uuid") && issue.contains("not a valid UUID")),
        "{reported:?}"
    );
    assert!(reported.iter().any(|issue| issue.contains("nil UUID")), "{reported:?}");
}

#[test]
fn two_spellings_of_one_workspace_are_refused_rather_than_silently_merged() {
    let upper = WORKSPACE.to_ascii_uppercase();
    let reported = issues(&format!(
        r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"

[connectors.secrets."{WORKSPACE}"]
SHIPPING = "first"

[connectors.secrets."{upper}"]
SHIPPING = "second"
"#
    ));
    assert!(
        reported.iter().any(|issue| issue.contains("names the same workspace")),
        "{reported:?}"
    );
}

// ---- tokens ----

#[test]
fn an_mcp_auth_token_shorter_than_the_minimum_is_refused_without_echoing_it() {
    let reported = issues(&format!(
        r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"

[mcp]
workspace_id = "{WORKSPACE}"
bot_token = "opr_live_token"
auth_token = "fifteen-chars-x"
"#
    ));
    assert!(
        reported
            .iter()
            .any(|issue| issue.contains("mcp.auth_token must be at least 16 characters")),
        "{reported:?}"
    );
    assert!(
        !reported.iter().any(|issue| issue.contains("fifteen-chars-x")),
        "the token must never be echoed: {reported:?}"
    );
}

#[test]
fn an_mcp_auth_token_at_the_minimum_is_accepted() {
    let config = parse(&format!(
        r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"

[mcp]
workspace_id = "{WORKSPACE}"
bot_token = "opr_live_token"
auth_token = "sixteen-chars-ab"
"#
    ))
    .expect("a 16 character token should be accepted");
    assert_eq!(
        config.mcp.auth_token.as_ref().map(super::Secret::expose),
        Some("sixteen-chars-ab")
    );
}

#[test]
fn an_incomplete_mcp_section_reports_everything_it_still_needs() {
    let config = parse(
        r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"
"#,
    )
    .expect("the api and the worker do not need [mcp]");

    let Err(ConfigError::Invalid { issues, .. }) = config.mcp_runtime() else {
        panic!("mcp_runtime should refuse an absent [mcp] section");
    };
    assert!(issues.iter().any(|issue| issue.contains("mcp.bot_token")), "{issues:?}");
    assert!(
        issues.iter().any(|issue| issue.contains("mcp.workspace_id")),
        "{issues:?}"
    );
}

#[test]
fn an_mcp_section_without_optional_values_gets_the_documented_defaults() {
    let config = parse(&format!(
        r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"

[mcp]
workspace_id = "{WORKSPACE}"
bot_token = "opr_live_token"
"#
    ))
    .expect("bot token and workspace are enough");
    let mcp = config.mcp_runtime().expect("runtime should resolve");
    assert_eq!(mcp.api_url, super::DEFAULT_MCP_API_URL);
    assert_eq!(mcp.bind_addr, super::DEFAULT_MCP_BIND_ADDR);
    assert_eq!(mcp.transport, McpTransport::Stdio);
    assert!(mcp.auth_token.is_none());
}

// ---- connector credential isolation ----

#[test]
fn a_connector_reads_only_the_credentials_of_its_own_workspace() {
    let config = parse(&full_config()).expect("the complete example should validate");
    let own: Uuid = WORKSPACE.parse().expect("test constant is a uuid");
    let other: Uuid = OTHER_WORKSPACE.parse().expect("test constant is a uuid");

    assert_eq!(
        config
            .connectors
            .secrets
            .get(own, "PAYMENTS")
            .expect("own credential")
            .expose(),
        "payments-credential"
    );
    assert_eq!(
        config
            .connectors
            .secrets
            .get(other, "PAYMENTS")
            .expect("the other tenant's own credential")
            .expose(),
        "other-tenant-credential"
    );
    // The same name in two workspaces resolves to two different values, and neither workspace can
    // reach the other's.
    let error = config
        .connectors
        .secrets
        .get(other, "SHIPPING")
        .expect_err("SHIPPING belongs to the first workspace only");
    assert!(error.to_string().contains(&other.to_string()), "{error}");
    assert!(!error.to_string().contains("shipping-credential"), "{error}");

    // A workspace nobody declared reaches nothing at all.
    assert!(config.connectors.secrets.get(Uuid::new_v4(), "PAYMENTS").is_err());
}

#[test]
fn reserved_and_malformed_credential_names_are_refused_at_load_time() {
    let reported = issues(&format!(
        r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"

[connectors.secrets."{WORKSPACE}"]
JWT_SECRET = "stolen"
AWS_SECRET_ACCESS_KEY = "stolen"
lowercase = "bad shape"
EMPTY = ""
"#
    ));
    assert!(
        reported
            .iter()
            .any(|issue| issue.contains("JWT_SECRET") && issue.contains("reserved")),
        "{reported:?}"
    );
    assert!(
        reported
            .iter()
            .any(|issue| issue.contains("AWS_SECRET_ACCESS_KEY") && issue.contains("reserved")),
        "{reported:?}"
    );
    assert!(
        reported
            .iter()
            .any(|issue| issue.contains("lowercase") && issue.contains("invalid")),
        "{reported:?}"
    );
    assert!(
        reported
            .iter()
            .any(|issue| issue.contains("EMPTY") && issue.contains("placeholder")),
        "{reported:?}"
    );
}

// ---- storage ----

#[test]
fn the_s3_backend_requires_its_own_section() {
    let reported = issues(
        r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"

[storage]
backend = "s3"
"#,
    );
    assert!(
        reported
            .iter()
            .any(|issue| issue.contains("[storage.s3] section is required")),
        "{reported:?}"
    );
}

#[test]
fn the_s3_backend_reports_every_missing_credential_at_once() {
    let reported = issues(
        r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"

[storage]
backend = "s3"

[storage.s3]
bucket = "UPPERCASE"
"#,
    );
    for expected in [
        "storage.s3.endpoint is required",
        "storage.s3.bucket UPPERCASE may only contain lowercase",
        "storage.s3.access_key_id is required",
        "storage.s3.secret_access_key is required",
    ] {
        assert!(
            reported.iter().any(|issue| issue.contains(expected)),
            "missing {expected:?} in {reported:?}"
        );
    }
}

#[test]
fn a_leftover_s3_section_under_the_local_backend_is_ignored() {
    let config = parse(
        r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"

[storage]
backend = "local"
dir = "/var/lib/openpr/uploads"

[storage.s3]
bucket = "still-here"
"#,
    )
    .expect("an unread [storage.s3] section must not block the local backend");
    assert_eq!(config.storage.backend, StorageBackend::Local);
    assert_eq!(config.storage.dir, Path::new("/var/lib/openpr/uploads"));
    assert!(config.storage.s3.is_none());
}

// ---- structural failures ----

#[test]
fn an_unknown_key_is_refused_rather_than_silently_ignored() {
    let error = parse(
        r#"
[database]
url = "postgres://localhost/openpr"
maxconnections = 40

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"
"#,
    )
    .expect_err("a typo must not be accepted");
    let rendered = error.to_string();
    assert!(matches!(error, ConfigError::Malformed { .. }), "{rendered}");
    assert!(rendered.contains("maxconnections"), "{rendered}");
}

#[test]
fn a_parse_failure_never_reproduces_the_offending_line() {
    let error = parse("[auth]\njwt_secret = \"unterminated-super-secret\n").expect_err("should fail");
    let rendered = error.to_string();
    assert!(!rendered.contains("unterminated-super-secret"), "{rendered}");
    assert!(rendered.contains("may hold a secret"), "{rendered}");
}

#[test]
fn a_missing_file_says_where_it_looked_and_how_to_create_it() {
    let error = OpenPrConfig::load(Some(Path::new("/nonexistent/openpr-does-not-exist.toml")))
        .expect_err("a missing file must not fall back to defaults");
    assert!(matches!(error, ConfigError::NotFound { .. }));
    let rendered = error.to_string();
    assert!(
        rendered.contains("/nonexistent/openpr-does-not-exist.toml"),
        "{rendered}"
    );
    assert!(rendered.contains(DEFAULT_CONFIG_PATH), "{rendered}");
    assert!(rendered.contains("openssl rand -hex 32"), "{rendered}");
}

#[test]
fn a_directory_in_place_of_the_file_is_reported_as_unreadable() {
    let error = OpenPrConfig::load(Some(Path::new("/"))).expect_err("a directory is not a configuration file");
    assert!(
        matches!(error, ConfigError::Unreadable { .. } | ConfigError::Malformed { .. }),
        "{error}"
    );
}

// ---- projection onto AppConfig ----

#[test]
fn app_config_takes_the_file_over_the_binary_defaults() {
    let config = parse(&full_config()).expect("the complete example should validate");
    let cfg = AppConfig::from_config(&config, "worker", "127.0.0.1:9999").expect("the file names both values");
    assert_eq!(cfg.app_name, "api");
    assert_eq!(cfg.bind_addr, "0.0.0.0:8081");
    assert_eq!(cfg.jwt_secret.expose(), "0123456789abcdef0123456789abcdef");
    assert_eq!(cfg.jwt_access_ttl_seconds, 3600);
    assert_eq!(cfg.jwt_refresh_ttl_seconds, 604_800);
    assert!(cfg.default_author_id.is_some());
}

#[test]
fn app_config_falls_back_to_the_binary_defaults_when_the_file_is_silent() {
    let config = parse(
        r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"
"#,
    )
    .expect("minimal file");
    let cfg = AppConfig::from_config(&config, "worker", "0.0.0.0:8081").expect("the file names both values");
    assert_eq!(cfg.app_name, "worker");
    assert_eq!(cfg.bind_addr, "0.0.0.0:8081");
}

// ---- redaction ----

#[test]
fn debug_output_of_app_config_never_contains_a_secret() {
    let config = parse(&full_config()).expect("the complete example should validate");
    let cfg = AppConfig::from_config(&config, "api", "0.0.0.0:8081").expect("the file names both values");
    let rendered = format!("{cfg:?}");
    assert!(!rendered.contains("0123456789abcdef0123456789abcdef"), "{rendered}");
    assert!(!rendered.contains("s3cret"), "{rendered}");
    assert_eq!(rendered.matches(REDACTED).count(), 2, "{rendered}");
    assert!(rendered.contains("app_name"), "{rendered}");
}

#[test]
fn debug_output_of_the_whole_config_never_contains_a_secret() {
    let config = parse(&full_config()).expect("the complete example should validate");
    for rendered in [format!("{config:?}"), format!("{config:#?}")] {
        for plaintext in [
            "0123456789abcdef0123456789abcdef",
            "s3cret",
            "postgres://openpr",
            "s3-secret-access-key",
            "s3-session-token",
            "opr_live_botexampletoken",
            "mcp-inbound-token-value",
            "shipping-credential",
            "payments-credential",
            "other-tenant-credential",
        ] {
            assert!(!rendered.contains(plaintext), "{plaintext} leaked into {rendered}");
        }
        assert!(rendered.contains(REDACTED), "{rendered}");
    }
}

#[test]
fn debug_output_of_the_database_and_auth_runtimes_never_contains_a_secret() {
    let config = parse(&full_config()).expect("the complete example should validate");
    let database = config.database_runtime().expect("the database section is complete");
    let auth = config.auth_runtime().expect("the auth section is complete");
    for rendered in [format!("{database:?}"), format!("{auth:?}")] {
        assert!(!rendered.contains("s3cret"), "{rendered}");
        assert!(!rendered.contains("postgres://openpr"), "{rendered}");
        assert!(!rendered.contains("0123456789abcdef0123456789abcdef"), "{rendered}");
        assert!(rendered.contains(REDACTED), "{rendered}");
    }
}

#[test]
fn debug_output_of_the_mcp_runtime_never_contains_a_secret() {
    let config = parse(&full_config()).expect("the complete example should validate");
    let mcp = config.mcp_runtime().expect("mcp section is complete");
    let rendered = format!("{mcp:?}");
    assert!(!rendered.contains("opr_live_botexampletoken"), "{rendered}");
    assert!(!rendered.contains("mcp-inbound-token-value"), "{rendered}");
    assert!(rendered.contains("http://api:8080"), "{rendered}");
}
