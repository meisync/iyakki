---
name: iyakki-mcp-author
description: Write an MCP server (Python or Node, stdio) for IYAKKI with validated tools, resources and prompts, test it with a fake client, register it in the MCP tile and package it as an mcp package
triggers: mcp, mcp server, model context protocol, tool server, stdio server, json-rpc
---
Use this when someone wants a new MCP server or to give the assistant new tools through one.

1. Scaffold with app_scaffold {kind: mcp-python or mcp-node, name, dir}. Python uses only the standard library; Node uses only built-ins. Prefer these: no dependencies means nothing extra to review. Add a dependency only when the person asks and say what it adds.
2. Protocol (stdio, one JSON-RPC 2.0 message per line): initialize returns protocolVersion, serverInfo, capabilities; notifications get no reply; tools/list returns name, description, inputSchema; tools/call returns {content:[{type:text,text}], isError}; resources/list and resources/read; prompts/list and prompts/get; ping; unknown method gives error -32601. Only protocol messages go to stdout; logs go to stderr.
3. Tools: one clear job each, a verb name (search_notes), a description that says when to use it, a JSON schema with required fields and types. Validate every argument against the schema BEFORE running (reject unknown keys, wrong types, over-long strings). Return short text; cap the size (the template caps at 100000 characters).
4. Safety rules: never start a shell and never build a command line from input (no os.system, shell=True, child_process, eval); file access only inside one named folder (resolve the path, refuse anything outside); network only to hosts you list in permissions; give every tool a time limit (10 s in the template); read secrets from the environment, never from code; do not echo secrets in results or logs.
5. Resources are read-only data with a uri and mime type; prompts are reusable instruction templates with arguments.
6. Tests: tests/test_server.py or tests/test_server.js is a fake client that talks to the real process over pipes: handshake, list, good call, bad arguments (expect an error), unknown tool, slow tool cut off, garbage input does not kill the server. Add a test for every tool and every refusal rule. Run them until green.
7. Try it in IYAKKI: MCP tile, add a server: command python3 (or node), argument the path of server.py (or server.js). Check the tools appear in chat.
8. Package: set permissions in iyakki-app.json (net:host, read:/folder, vault:name), store_validate the folder, then `iyakki pkg pack . --key me.key -o NAME.iyp`. The package carries the source and installs as `python3 -c <source>` (or node -e); the install review shows the program and permissions. Then store_publish (the person approves).
9. Raise version for each release. Keep the README current: tools, arguments, permissions, examples.
10. Track multi-step work on the feature board (board_list/board_add/board_update, tests=, files=; done only after tests pass). ACT: write files and run builds and tests yourself; never ask the person to. Bugs/failures: own board row with a B- id (never F-); bug_report (failing pods) captures logs; fix, verify, attach the passing run.
