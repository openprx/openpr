// Standalone tool listing binary — does not require a database connection.
// It is a CLI, so writing the failure reason to stderr is the expected behaviour.
#![allow(clippy::print_stderr)]
use mcp_server::get_all_tool_definitions;
use std::io::{self, Write};

/// Writes the tool catalogue. A closed pipe (`| head`) is a normal end of output,
/// not a failure, so it stops quietly instead of panicking inside `println!`.
fn write_tools(out: &mut impl Write) -> io::Result<()> {
    let tools = get_all_tool_definitions();
    let count = tools.len();
    writeln!(out, "Available MCP Tools ({count} total):\n")?;

    for tool in tools {
        writeln!(out, "  {}", tool.name)?;
        writeln!(out, "   {}", tool.description)?;
        let schema = serde_json::to_string_pretty(&tool.input_schema).unwrap_or_default();
        writeln!(out, "   Schema: {schema}")?;
        writeln!(out)?;
    }
    out.flush()
}

fn main() {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    if let Err(error) = write_tools(&mut out)
        && error.kind() != io::ErrorKind::BrokenPipe
    {
        eprintln!("Failed to write tool list: {error}");
    }
}
