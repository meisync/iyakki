//! Types moved out of `iyakki_agent::mcpadmintools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nMCP SERVERS: mcp_servers lists the MCP servers the person may use and where each comes from (free). mcp_server_add (name, command, args) adds a server that is started as a program on this computer and mcp_server_remove (name) removes one that was added here; both are for administrators and ask every time. A server of the administrator's mcp.json cannot be removed from here. The tools of a connected server are called as mcp__SERVER__TOOL.";
