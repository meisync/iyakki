# UTCP (Universal Tool Calling Protocol) beside MCP

Status: **plan**, requested 2026-10-31, not built. Nothing here changes how MCP works today (F-AI-06, `iyakki-agent::mcp`).

## Why

MCP puts a server between the agent and the tool: every tool needs a process that speaks MCP. UTCP describes the tool's own endpoint and how
to call it, and the agent then calls it directly. For IYAKKI that means less to run on a small machine, and any plain web API or command-line
program can become a tool by a description alone. The owner wants UTCP supported as MCP ages (2026-10-31).

## What UTCP is (read from utcp.io and the published spec on 2026-10-31; re-check before building)

* A **manual** is a JSON document: `utcp_version`, `manual_version`, `info` (`title`, `version`, optional `description`), `tools`, and an optional
  `allowed_communication_protocols` (default: only `http`; a manual must list `cli` to add command-line tools).
* A **tool**: `name`, `description`, `inputs` and `outputs` (JSON Schema), optional `tags`, and a `tool_call_template`.
* A **call template** has `call_template_type`: `http`, `cli`, `text`, `sse`, `websocket`, `mcp`.
  * `http`: `url` (with `{name}` path placeholders), `http_method`, `content_type`, `headers`, `body_field`, `header_fields`, `auth`
    (`api_key` with `location` header, query or cookie; `basic`; `oauth2`). Arguments go to the path, then the body field, then header fields,
    and what is left becomes the query. Discovery: a URL whose JSON has `utcp_version` and `tools` is a manual; otherwise it is read as OpenAPI.
  * `cli`: `commands` (ordered steps, `UTCP_ARG_<name>_UTCP_END` placeholders, `append_to_final_output`), `working_dir`, `env_vars`.
  * `text`: a manual or OpenAPI file on disk (`file_path`, no auth).
* Licence of the specification: Apache-2.0 for the text and guides (the generated API reference is MPL-2.0 and is not used). We write our own
  reader from the specification; no UTCP library is added ([[feedback-build-ourselves]]).

## What IYAKKI supports, in order

1. **Step 1 (first release): manuals and `http` tools.** Add a manual by URL or by pasting it; list its tools like MCP tools; call them. The
   safety rules of the UTCP documentation apply and are tested: `https` only (plain `http` only for loopback), each redirect hop checked and no
   credentials sent to another origin, a remote manual may not point at loopback or the local network, path values URL-encoded, 10 s discovery
   timeout, output size cap, credentials only from the vault (never in the manual).
2. **Step 2: `text` and `cli`.** `cli` runs a program on this machine, so it is the risky one: off unless the manual lists `cli`, the person approves
   the manual once, argument values are passed as separate arguments and never through a shell, the schema is checked first, the call runs in the
   same sandbox as other tool commands, and every call is an approval card unless the person has allowed it.
3. **Step 3: IYAKKI as a UTCP provider.** The tools we already offer over the MCP bridge for external agents (`board_*`, `deploy_*`, ...) are also
   published as a manual (one local `http` endpoint behind the same token and gate), so Claude Code, Copilot and others can use whichever protocol
   they speak.
4. **Later: `sse`, `websocket`, and `mcp` templates** (a UTCP manual can point at an MCP server; ours already speaks that).

## Where it fits

* New module `iyakki-agent::utcp` next to `mcp.rs`: manual parser, tool list, `http` caller, store (`UtcpStore` like `McpStore`, on the data volume).
* The assistant sees UTCP tools exactly like MCP tools (same tool list, same approval card, same audit row with the manual's name). `iyakki-sec`
  policy gets `utcp_manuals` next to `mcp_servers` (`allows_utcp`), so a child account sees none by default.
* The MCP app becomes "Tools": MCP servers and UTCP manuals in one list, each row saying which protocol it uses.
* Network use goes through the existing firewall and proxy settings; no new open port in step 1.

## Tests (when built)

* a manual with every field parses; missing required fields and an unknown template type are refused with a plain sentence;
* `http` argument mapping (path, body field, header fields, query) against a local test server; path values are encoded;
* `https` rule, redirect re-check, no credential on another origin, remote manual naming loopback is refused, 10 s timeout, size cap;
* `cli` (step 2): not run unless allowed, arguments never reach a shell, a failing command returns its error text;
* policy: a restricted account sees no manual; the audit row names the manual and the tool;
* the assistant lists a UTCP tool and an MCP tool side by side and calls both.

## Open points

* The exact current `utcp_version` (sources showed 1.0.1 and 1.1.0): read the specification repository again before step 1 and accept both.
* Tool names: the documentation's examples use plain names; we prefix with the manual's name inside IYAKKI to avoid clashes.
* Whether the person adds manuals only by hand or also from the store catalog (later).
