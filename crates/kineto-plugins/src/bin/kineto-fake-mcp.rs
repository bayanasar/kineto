//! The fake MCP server (see [`kineto_plugins::fake_mcp`]).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    kineto_plugins::fake_mcp::serve(&args);
}
