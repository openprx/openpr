#![allow(clippy::print_stdout)]
// Standalone tool listing binary — does not require a database connection.
use mcp_server::get_all_tool_definitions;

fn main() {
    let tools = get_all_tool_definitions();
    let count = tools.len();
    println!("Available MCP Tools ({count} total):\n");

    for tool in tools {
        println!("  {}", tool.name);
        println!("   {}", tool.description);
        let schema = serde_json::to_string_pretty(&tool.input_schema).unwrap_or_default();
        println!("   Schema: {schema}");
        println!();
    }
}
