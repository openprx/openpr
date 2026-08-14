//! Shared helpers for the end-to-end tests that drive the shipped binary.
//!
//! The binary reads no environment variables, so a test that starts it writes a real
//! configuration file and passes it with `--config`. Everything the child process is told
//! therefore travels the path a deployment uses; a test that configured the process some
//! other way would stop proving the shipped path works.

use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// Sections `OpenPrConfig::load` requires from every file, whichever binary reads it.
///
/// The MCP server opens no database connection and signs no token, so neither value is
/// used here; they are present because one file serves three binaries and the loader
/// validates the whole file before any binary picks out its own section.
const SHARED_SECTIONS: &str = "\
[database]
url = \"postgres://openpr:e2e-unused-password@127.0.0.1:5432/openpr\"

[auth]
jwt_secret = \"e2e-jwt-secret-unused-by-the-mcp-server\"

[logging]
filter = \"error\"
format = \"text\"

";

/// The `[mcp]` section a test wants the child process to read.
///
/// Every field is spelled out at each call site rather than defaulted, so a test that
/// depends on, say, the absence of an inbound token says so where it is read.
pub struct McpSettings<'a> {
    pub api_url: &'a str,
    /// The identity `stdio` and the CLI subcommands act as. `None` writes no key at all,
    /// which is what an `http`/`sse` deployment looks like: it holds no identity of its own
    /// and serves every request as the caller that presented one.
    pub bot_token: Option<&'a str>,
    pub workspace_id: &'a str,
    /// `None` leaves the file's own default, `stdio`.
    pub transport: Option<&'a str>,
    /// `None` leaves the file's own default, `127.0.0.1:8090`.
    pub bind_addr: Option<&'a str>,
}

/// A configuration file in a private temporary directory, deleted when the test drops it.
///
/// It has to outlive the child process that reads it, so tests keep it next to the child.
pub struct ConfigFile {
    dir: PathBuf,
    path: PathBuf,
}

impl ConfigFile {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ConfigFile {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Writes a complete configuration file carrying `settings`.
pub fn write_config(settings: &McpSettings<'_>) -> Result<ConfigFile, Box<dyn Error>> {
    write_config_with_extra_mcp_keys(settings, "")
}

/// As [`write_config`], plus raw lines appended to the `[mcp]` table.
///
/// The escape hatch exists for keys [`McpSettings`] deliberately does not model — a retired
/// one, for instance, which a test needs to write precisely because the loader should refuse
/// it. Modelling those as fields would suggest they are still part of the schema.
pub fn write_config_with_extra_mcp_keys(
    settings: &McpSettings<'_>,
    extra_mcp_keys: &str,
) -> Result<ConfigFile, Box<dyn Error>> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let unique = format!(
        "openpr-mcp-e2e-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let dir = std::env::temp_dir().join(unique);
    fs::create_dir_all(&dir)?;

    let mut source = String::from(SHARED_SECTIONS);
    writeln!(source, "[mcp]")?;
    writeln!(source, "api_url = \"{}\"", settings.api_url)?;
    if let Some(bot_token) = settings.bot_token {
        writeln!(source, "bot_token = \"{bot_token}\"")?;
    }
    writeln!(source, "workspace_id = \"{}\"", settings.workspace_id)?;
    if let Some(transport) = settings.transport {
        writeln!(source, "transport = \"{transport}\"")?;
    }
    if let Some(bind_addr) = settings.bind_addr {
        writeln!(source, "bind_addr = \"{bind_addr}\"")?;
    }
    source.push_str(extra_mcp_keys);

    let path = dir.join("openpr.toml");
    fs::write(&path, source)?;
    Ok(ConfigFile { dir, path })
}
