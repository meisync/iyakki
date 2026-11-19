# __TITLE__

An MCP server (Model Context Protocol, stdio) for IYAKKI, in Python with the standard library only.

Files
* `server.py`: the server. `TOOLS`, `RESOURCES` and `PROMPTS` at the top are what to edit.
* `tests/test_server.py`: a fake client that talks to the real server process (`python3 tests/test_server.py`).
* `iyakki-app.json`: what the store shows. `runtime` is the program that runs the server (`python3`).

Rules the template follows: every argument is checked against the schema, a tool has a time limit and a result size
limit, no shell is ever started, only protocol messages go to stdout, secrets come from the environment.

Try it by hand: `printf '{"jsonrpc":"2.0","id":1,"method":"tools/list"}\n' | python3 server.py`

Ship it
1. Edit, then `python3 tests/test_server.py`.
2. `iyakki pkg validate .` (permissions, licence, secrets scan).
3. `iyakki pkg keygen me` once, then `iyakki pkg pack . --key me.key -o __NAME__.iyp`. The package carries the server
   source: installing adds `python3 -c <source>` to the MCP tile, and the review shows the program it runs.
4. `iyakki pkg publish --server URL --token-env STORE_TOKEN __NAME__.iyp`

Local trial without a store: add a server in the MCP tile with command `python3` and argument `server.py`.
If the server needs the network or a vault entry, list it in `permissions` (for example `net:api.example.com`).
