// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Used by agents that only speak stdio MCP; see agent.rs.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--mcp-stdio") {
        if let (Some(url), Some(token)) = (args.get(2), args.get(3)) {
            review_tool_lib::run_mcp_stdio_bridge(url, token);
        }
        return;
    }
    review_tool_lib::run()
}
