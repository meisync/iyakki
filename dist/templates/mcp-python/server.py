#!/usr/bin/env python3
"""__TITLE__: an MCP server over stdio (newline-delimited JSON-RPC 2.0). Standard library only.

Tools, one resource and one prompt are declared below. Add yours to TOOLS, RESOURCES and PROMPTS.
Rules this file follows: validate every argument against the schema, never start a shell, bound the
time a tool may take and the size of what it returns, never print anything but protocol messages on
stdout (use stderr for logs), never put secrets in the code (read them from the environment).
"""
import json
import sys
import threading

PROTOCOL = "2024-11-05"
NAME = "__NAME__"
VERSION = "0.1.0"
MAX_LINE = 1_000_000      # longest request line accepted
MAX_RESULT = 100_000      # longest text a tool may return
TOOL_SECONDS = 10         # time budget of one tool call


def tool_echo(args):
    return args["text"].upper() if args.get("shout") else args["text"]


def tool_add(args):
    return str(args["a"] + args["b"])


TOOLS = {
    "echo": {
        "description": "Return the text, optionally in capitals.",
        "schema": {"type": "object", "properties": {"text": {"type": "string", "maxLength": 2000}, "shout": {"type": "boolean"}}, "required": ["text"]},
        "run": tool_echo,
    },
    "add": {
        "description": "Add two numbers.",
        "schema": {"type": "object", "properties": {"a": {"type": "number"}, "b": {"type": "number"}}, "required": ["a", "b"]},
        "run": tool_add,
    },
}

RESOURCES = {"notes://readme": {"name": "About this server", "mime": "text/plain", "text": "__TITLE__ is an example MCP server."}}

PROMPTS = {"summarise": {"description": "Ask for a short summary of a text.", "arguments": [{"name": "text", "required": True}],
                         "text": "Summarise this in three bullet points:\n{text}"}}

_TYPES = {"string": str, "boolean": bool, "integer": int, "number": (int, float), "object": dict, "array": list}


class BadArgs(Exception):
    pass


def check_args(schema, args):
    """A small subset of JSON schema: object, required, primitive types, maxLength. Raises BadArgs."""
    if not isinstance(args, dict):
        raise BadArgs("arguments must be an object")
    props = schema.get("properties", {})
    for r in schema.get("required", []):
        if r not in args:
            raise BadArgs("missing argument %r" % r)
    for k, v in args.items():
        if k not in props:
            raise BadArgs("unknown argument %r" % k)
        t = props[k].get("type")
        if t and (not isinstance(v, _TYPES[t]) or (t in ("integer", "number") and isinstance(v, bool))):
            raise BadArgs("%r must be %s" % (k, t))
        if isinstance(v, str) and len(v) > props[k].get("maxLength", 10_000):
            raise BadArgs("%r is too long" % k)


def run_bounded(fn, args):
    """Run fn(args) with a time limit. A tool that overruns is abandoned (it is a daemon thread)."""
    box = {}

    def target():
        try:
            box["ok"] = fn(args)
        except Exception as e:  # reported to the client, never crashes the server
            box["err"] = str(e)

    t = threading.Thread(target=target, daemon=True)
    t.start()
    t.join(TOOL_SECONDS)
    if t.is_alive():
        raise TimeoutError("the tool took longer than %d seconds" % TOOL_SECONDS)
    if "err" in box:
        raise RuntimeError(box["err"])
    return box["ok"]


def result(id_, value):
    return {"jsonrpc": "2.0", "id": id_, "result": value}


def error(id_, code, message):
    return {"jsonrpc": "2.0", "id": id_, "error": {"code": code, "message": message}}


def handle(msg):
    """One JSON-RPC message in, one response (or None for a notification) out."""
    if not isinstance(msg, dict) or msg.get("jsonrpc") != "2.0":
        return error(None, -32600, "invalid request")
    id_, method, params = msg.get("id"), msg.get("method"), msg.get("params") or {}
    if id_ is None:
        return None  # notifications (such as notifications/initialized) get no answer
    if method == "initialize":
        return result(id_, {"protocolVersion": PROTOCOL, "serverInfo": {"name": NAME, "version": VERSION},
                            "capabilities": {"tools": {}, "resources": {}, "prompts": {}}})
    if method == "ping":
        return result(id_, {})
    if method == "tools/list":
        return result(id_, {"tools": [{"name": n, "description": t["description"], "inputSchema": t["schema"]} for n, t in TOOLS.items()]})
    if method == "tools/call":
        tool = TOOLS.get(params.get("name"))
        if tool is None:
            return error(id_, -32602, "unknown tool")
        try:
            check_args(tool["schema"], params.get("arguments", {}))
            text = str(run_bounded(tool["run"], params.get("arguments", {})))[:MAX_RESULT]
            return result(id_, {"content": [{"type": "text", "text": text}], "isError": False})
        except BadArgs as e:
            return error(id_, -32602, str(e))
        except Exception as e:
            return result(id_, {"content": [{"type": "text", "text": str(e)}], "isError": True})
    if method == "resources/list":
        return result(id_, {"resources": [{"uri": u, "name": r["name"], "mimeType": r["mime"]} for u, r in RESOURCES.items()]})
    if method == "resources/read":
        r = RESOURCES.get(params.get("uri"))
        if r is None:
            return error(id_, -32602, "unknown resource")
        return result(id_, {"contents": [{"uri": params["uri"], "mimeType": r["mime"], "text": r["text"]}]})
    if method == "prompts/list":
        return result(id_, {"prompts": [{"name": n, "description": p["description"], "arguments": p["arguments"]} for n, p in PROMPTS.items()]})
    if method == "prompts/get":
        p = PROMPTS.get(params.get("name"))
        if p is None:
            return error(id_, -32602, "unknown prompt")
        text = p["text"].replace("{text}", str((params.get("arguments") or {}).get("text", "")))
        return result(id_, {"messages": [{"role": "user", "content": {"type": "text", "text": text}}]})
    return error(id_, -32601, "method not found")


def serve(stdin=sys.stdin, stdout=sys.stdout):
    for line in stdin:
        if len(line) > MAX_LINE:
            out = error(None, -32600, "request too large")
        else:
            try:
                out = handle(json.loads(line))
            except json.JSONDecodeError:
                out = error(None, -32700, "parse error")
        if out is not None:
            stdout.write(json.dumps(out) + "\n")
            stdout.flush()


if __name__ == "__main__":
    serve()
