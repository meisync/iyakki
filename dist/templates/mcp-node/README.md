# __TITLE__

An MCP server (Model Context Protocol, stdio) for IYAKKI, in Node with built-in modules only (no npm packages to review).

Files
* `server.js`: the server. `TOOLS`, `RESOURCES` and `PROMPTS` at the top are what to edit.
* `tests/test_server.js`: a fake client that talks to the real server process (`node tests/test_server.js`).
* `iyakki-app.json`: what the store shows. `runtime` is the program that runs the server (`node`).

Rules the template follows: every argument is checked against the schema, a tool has a time limit and a result size
limit, no shell or child process is started, only protocol messages go to stdout, secrets come from the environment.

Ship it
1. Edit, then `node tests/test_server.js`.
2. `iyakki pkg validate .`
3. `iyakki pkg keygen me` once, then `iyakki pkg pack . --key me.key -o __NAME__.iyp`. The package carries the server
   source: installing adds `node -e <source>` to the MCP tile, and the review shows the program it runs.
4. `iyakki pkg publish --server URL --token-env STORE_TOKEN __NAME__.iyp`

Local trial without a store: add a server in the MCP tile with command `node` and argument `server.js`.
If it needs the network or a vault entry, list it in `permissions` (for example `net:api.example.com`).
