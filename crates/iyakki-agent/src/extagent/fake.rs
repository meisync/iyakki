//! Fake vendor CLIs for the tests: small shell scripts that print what the real program prints (Claude-style stream-json,
//! Copilot-style JSONL, plain text) and record how they were started. No test ever calls a real vendor service.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub fn tmp(tag: &str) -> PathBuf {
    let mut r = [0u8; 4];
    let _ = iyakki_sec::random_fill(&mut r);
    let d = std::env::temp_dir().join(format!("iyakki-extagent-{tag}-{}-{}", std::process::id(), r.iter().map(|b| format!("{b:02x}")).collect::<String>()));
    std::fs::create_dir_all(&d).unwrap();
    d.canonicalize().unwrap()
}

pub fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    let tmpf = dir.join(format!(".{name}.new"));
    std::fs::write(&tmpf, body.replace("__DIR__", &dir.display().to_string())).unwrap();
    std::fs::set_permissions(&tmpf, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&tmpf, &p).unwrap();
    p
}

/// A `claude` that records its arguments, environment and folder in `dir`, and answers by a word in the request.
pub fn claude(dir: &Path) -> PathBuf {
    script(
        dir,
        "claude",
        r#"#!/bin/sh
D="__DIR__"
case "$1" in
  --version) echo "2.1.999 (Claude Code)"; exit 0;;
  auth) if [ -f "$D/signedout" ]; then echo '{"loggedIn":false,"authMethod":"none"}'; exit 1; else echo '{"loggedIn":true,"authMethod":"claude.ai"}'; exit 0; fi;;
esac
: > "$D/args.txt"
for a in "$@"; do printf '%s\n' "$a" >> "$D/args.txt"; done
pwd > "$D/cwd.txt"
env > "$D/env.txt"
for last; do :; done
case "$last" in
  *HANG*)
    echo '{"type":"system","subtype":"init","session_id":"sess-h","model":"m"}'
    sleep 60 &
    echo $! > "$D/child.pid"
    echo $$ > "$D/leader.pid"
    wait;;
  *AUTHFAIL*) echo "Invalid API key - Please run /login" >&2; exit 1;;
  *RATE*) echo "API Error: 429 rate_limit_error: you are rate limited" >&2; exit 1;;
  *NETDOWN*) echo "request failed: getaddrinfo ENOTFOUND api.anthropic.com" >&2; exit 1;;
  *EMPTY*) exit 0;;
  *RESULTERR*)
    echo '{"type":"system","subtype":"init","session_id":"sess-e","model":"m"}'
    echo '{"type":"result","subtype":"success","is_error":true,"result":"Invalid API key - Please run /login","session_id":"sess-e","total_cost_usd":0,"usage":{"input_tokens":0,"output_tokens":0}}'
    exit 1;;
  *BADBYTES*) printf '\377\376 not utf8\n'; echo 'plainly not json';;
esac
cat <<'JSONL'
{"type":"system","subtype":"init","session_id":"sess-1","model":"claude-sonnet-x","tools":[],"mcp_servers":[{"name":"iyakki","status":"connected"}]}
{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Looking"}}}
{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" now."}}}
{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"plan it"}]}}
{"type":"assistant","message":{"content":[{"type":"text","text":"Looking now."}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"a.txt","is_error":false}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_2","name":"mcp__iyakki__board_list","input":{}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_2","content":[{"type":"text","text":"B-1 open"}],"is_error":false}]}}
{"type":"assistant","message":{"content":[{"type":"text","text":"Done: found a.txt"}]}}
{"type":"result","subtype":"success","is_error":false,"result":"Done: found a.txt","session_id":"sess-1","total_cost_usd":0.0123,"usage":{"input_tokens":100,"cache_creation_input_tokens":20,"cache_read_input_tokens":30,"output_tokens":40}}
JSONL
"#,
    )
}

/// A `copilot` that prints JSONL and (on request) writes a file the way an agent that ignores its flags would.
pub fn copilot(dir: &Path) -> PathBuf {
    script(
        dir,
        "copilot",
        r#"#!/bin/sh
D="__DIR__"
case "$1" in --version) echo "GitHub Copilot CLI 1.0.0"; exit 0;; esac
: > "$D/args.txt"
for a in "$@"; do printf '%s\n' "$a" >> "$D/args.txt"; done
for last; do :; done
case "$last" in *SNEAKY*) echo changed > sneaky.txt;; esac
cat <<'JSONL'
{"type":"session.mcp_server_status_changed","data":{"serverName":"iyakki","status":"connected"},"ephemeral":true}
{"type":"assistant.turn_start","data":{"turnId":"0"}}
{"type":"assistant.message","data":{"messageId":"m1","content":"","toolRequests":[{"toolCallId":"t1","name":"iyakki-board_list","arguments":{},"type":"function","mcpServerName":"iyakki","mcpToolName":"board_list"}]}}
{"type":"assistant.tool_call_delta","data":{"toolCallId":"t1"},"ephemeral":true}
{"type":"tool.execution_start","data":{"toolCallId":"t1","toolName":"iyakki-board_list","arguments":{},"mcpServerName":"iyakki","mcpToolName":"board_list"}}
{"type":"tool.execution_complete","data":{"toolCallId":"t1","success":true,"result":{"content":"no rows","contents":[{"type":"text","text":"no rows"}]}}}
{"type":"tool.execution_start","data":{"toolCallId":"t2","toolName":"bash","arguments":{"command":"touch x"}}}
{"type":"tool.execution_complete","data":{"toolCallId":"t2","success":false,"error":{"message":"Permission to run this tool was denied due to the following rules: `shell`","code":"denied"}}}
{"type":"assistant.message_delta","data":{"messageId":"m2","deltaContent":"Use a "},"ephemeral":true}
{"type":"assistant.message_delta","data":{"messageId":"m2","deltaContent":"HashMap."},"ephemeral":true}
{"type":"assistant.message","data":{"messageId":"m2","content":"Use a HashMap.","toolRequests":[]}}
{"type":"assistant.idle","data":{},"ephemeral":true}
{"type":"result","sessionId":"cop-1","exitCode":0,"usage":{"premiumRequests":1}}
JSONL
"#,
    )
}

/// A plain-text agent: records how it was started and what came on standard input (`--stdin`).
pub fn texty(dir: &Path) -> PathBuf {
    script(
        dir,
        "texty",
        r#"#!/bin/sh
D="__DIR__"
: > "$D/args.txt"
for a in "$@"; do printf '%s\n' "$a" >> "$D/args.txt"; done
if [ "$1" = "--stdin" ]; then cat > "$D/stdin.txt"; fi
echo "first line"
echo "second line"
"#,
    )
}

/// A `claude` that speaks MCP to the bridge like the real one: per run (counted in `count`) it does what `plan.json` says for
/// that run (`calls`: IYAKKI tools through the socket, `approve`: its own tool prompts), prints the stream-json for them and a
/// `result` with the cost in `cost`. Needs `python3` (the tests skip without it).
pub fn claude_mcp(dir: &Path) -> PathBuf {
    script(
        dir,
        "claude",
        r#"#!/usr/bin/env python3
import sys, os, json, socket
D = "__DIR__"
a = sys.argv[1:]
if a[:1] == ["--version"]:
    print("2.1.9 (Claude Code)"); sys.exit(0)
if a[:1] == ["auth"]:
    print(json.dumps({"loggedIn": True, "authMethod": "claude.ai"})); sys.exit(0)
n = (int(open(D + "/count").read()) if os.path.exists(D + "/count") else 0) + 1
open(D + "/count", "w").write(str(n))
open(D + "/args%d.txt" % n, "w").write("\n".join(a))
plans = json.load(open(D + "/plan.json"))
plan = plans[min(n, len(plans)) - 1]
cfg = json.load(open(a[a.index("--mcp-config") + 1]))["mcpServers"]["iyakki"]
s = socket.socket(socket.AF_UNIX); s.connect(cfg["args"][1])
f = s.makefile("rw"); f.write(cfg["env"]["IYAKKI_MCP_TOKEN"] + "\n"); f.flush()
rid = [0]
def rpc(method, params):
    rid[0] += 1
    f.write(json.dumps({"jsonrpc": "2.0", "id": rid[0], "method": method, "params": params}) + "\n"); f.flush()
    return json.loads(f.readline())
def out(o):
    print(json.dumps(o)); sys.stdout.flush()
rpc("initialize", {"protocolVersion": "2025-03-26"})
sid = "sess-ap"
out({"type": "system", "subtype": "init", "session_id": sid, "model": "fake-opus"})
for i, c in enumerate(plan.get("calls", [])):
    tid = "toolu_%d_%d" % (n, i)
    if "approve" in c:
        tool = c["approve"]["tool_name"]
        out({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": tid, "name": tool, "input": c["approve"]["input"]}]}})
        r = rpc("tools/call", {"name": "approve", "arguments": dict(c["approve"], tool_use_id=tid)})
        verdict = json.loads(r["result"]["content"][0]["text"])
        ok = verdict["behavior"] == "allow"
        if ok and tool in ("Write", "Edit"):
            open(c["approve"]["input"]["file_path"], "w").write(c["approve"]["input"].get("content", ""))
        out({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": tid, "content": "ran" if ok else verdict.get("message", "denied"), "is_error": not ok}]}})
        continue
    out({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": tid, "name": "mcp__iyakki__" + c["name"], "input": c.get("args", {})}]}})
    r = rpc("tools/call", {"name": c["name"], "arguments": c.get("args", {}), "_meta": {"claudecode/toolUseId": tid}})
    res = r["result"]
    out({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": tid, "content": res["content"][0]["text"], "is_error": bool(res.get("isError"))}]}})
text = plan.get("text", "ok")
out({"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}})
out({"type": "result", "subtype": "success", "is_error": False, "result": text, "session_id": sid, "total_cost_usd": plan.get("cost", 0.0), "usage": {"input_tokens": 100, "cache_read_input_tokens": 5000, "output_tokens": 20}})
"#,
    )
}

/// A custom agent that can list its models: `--version`, and `--models` printing one per line with an optional note.
pub fn modelist(dir: &Path) -> PathBuf {
    script(
        dir,
        "modelist",
        r#"#!/bin/sh
case "$1" in
  --version) echo "modelist 1.0"; exit 0;;
  --models) printf 'fast-1  quick and cheap\nbig-2\tthe most capable\nmax-3  requires the Max plan\nbad id with spaces\n$(evil)\n# a comment\nfast-1\n'; exit 0;;
esac
echo ok
"#,
    )
}
