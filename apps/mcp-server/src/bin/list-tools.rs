// 独立的工具列表程序 - 不需要数据库连接
use mcp_server::get_all_tool_definitions;

fn main() {
    let tools = get_all_tool_definitions();

    println!("Available MCP Tools ({} total):\n", tools.len());

    for tool in tools {
        println!("📦 {}", tool.name);
        println!("   {}", tool.description);
        println!(
            "   Schema: {}",
            serde_json::to_string_pretty(&tool.input_schema).unwrap_or_default()
        );
        println!();
    }
}
