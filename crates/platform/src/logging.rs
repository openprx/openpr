use tracing_subscriber::{EnvFilter, fmt};

use crate::config::{LogFormat, LoggingConfig};
use crate::error::{AppError, AppResult};

/// Installs the global tracing subscriber described by the `[logging]` section.
///
/// The filter comes from the configuration file, never from `RUST_LOG`: a deployment's log level
/// is part of its configuration, and reading it from the environment made the effective level
/// depend on whatever the surrounding shell happened to export.
pub fn init(logging: &LoggingConfig, service_name: &str) -> AppResult<()> {
    let directives = logging.filter_or_default(service_name);
    let filter = EnvFilter::try_new(&directives).map_err(|err| {
        AppError::Config(format!(
            "logging.filter {directives} is not a valid tracing filter: {err}"
        ))
    })?;

    match logging.format {
        LogFormat::Json => fmt()
            .with_env_filter(filter)
            .json()
            .with_current_span(true)
            .with_span_list(true)
            .init(),
        LogFormat::Text => fmt().with_env_filter(filter).init(),
    }

    Ok(())
}
