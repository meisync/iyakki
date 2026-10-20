# __TITLE__

A sandboxed IYAKKI tool: a WebAssembly module with a manifest. It runs with a fuel, memory and time budget and can
touch only the folders and hosts listed in `capabilities`; every call still asks the person.

Files
* `tool.json`: name, version, licence, description, argument schema, capabilities, budget.
* `module.wat`: the module (text format; `iyakki pkg build` accepts `.wat` or a compiled `.wasm`).
  The host writes the JSON arguments at 65536, calls `run(length)`, and reads the JSON answer from 131072.
* `iyakki-app.json`: store information; `permissions` must equal `capabilities`.
* `tests/check_tool.py`: static checks (`python3 tests/check_tool.py`).

Rust or C: compile to `wasm32-unknown-unknown` with the same two exports (`memory`, `run`), set `"module": "target/.../x.wasm"`.

Ship it
1. `python3 tests/check_tool.py`, then try it: `iyakki run module.wat --entry run`.
2. `iyakki pkg validate .`
3. `iyakki pkg keygen me` once, then `iyakki pkg pack . --key me.key -o __NAME__.iyp`
4. `iyakki pkg publish --server URL --token-env STORE_TOKEN __NAME__.iyp`
