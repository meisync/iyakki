# IYAKKI SDK (phase 1)

**What it is.** Everything on your own computer for building tile apps, skills, MCP servers and services for IYAKKI, alone or with a coding agent
(Claude Code and others). It is the `iyakki` program (`iyakki sdk`, `iyakki dev`, `iyakki pkg`) plus the files in this folder. There is nothing else to install and it works offline
(only `dev` commands need a device).

| File | What for |
|---|---|
| `iyakki.d.ts` | TypeScript declarations of `window.iyakki` (the bridge a tile app sees), for editors and agents. One block per namespace. |
| `templates/CLAUDE.md`, `templates/AGENTS.md` | the agent instructions `iyakki sdk init` puts in a new project (loop, rules, how to ask the person for permissions) |
| `templates/.mcp.json` | for the future `iyakki dev mcp`. **When available**: not built yet. |
| `templates/service/` | the starting files of a k3s service project |
| `EMULATOR.md` | the target spec of `iyakki sdk emulator` (a QEMU device on your computer). **NOT BUILT.** |

## Install

Put the `iyakki` program on your PATH (from the release; Linux, macOS). Check: `iyakki --version`, then `iyakki sdk doctor`, which says in plain words what is missing.

## Quickstart

```sh
iyakki sdk init meeting-notes --kind tile      # or skill, mcp, service; --dir DIR to choose the folder
cd meeting-notes

iyakki dev pair 192.168.1.20                   # once: on the device, Settings -> Developer mode -> Pair a computer
iyakki sdk doctor                              # what is ready, and the next step

iyakki dev push . --watch --open               # pack, install as an unsigned dev app, reload on every change
iyakki dev logs meeting-notes --follow         # the app's console and install messages
iyakki dev screenshot -o shot.png              # a picture of the device's screen

# tests: tile apps run headless on the real engine (tile_test in the IDE or through the assistant, files in tests/);
# skills and MCP servers: python3 tests/test_skill.py | test_server.py | node tests/test_server.js
iyakki pkg validate .                          # the store's install review, offline
iyakki pkg keygen me                           # once; keep me.key private (it is in .gitignore)
iyakki pkg pack . --key me.key -o meeting-notes.iyp
iyakki pkg verify meeting-notes.iyp
iyakki pkg publish --server URL --token-env STORE_TOKEN meeting-notes.iyp
```

`iyakki sdk docs` lists the guides (offline); `iyakki sdk docs tile` or `iyakki sdk docs loop` shows one.

## Using it with Claude Code or another agent

`iyakki sdk init` writes `CLAUDE.md` and `AGENTS.md`: the loop (push, logs, tests, screenshot), the rules, and what to do on a refusal (stop and ask the person
for the exact permission, never work around it or press Allow on the device). Start your agent in the project folder. The device decides what a computer may do (scopes, set on the device); the SDK never widens them.

## Versions

The bridge version is 1.0 (the bridge as it ships). Within a major version only additions; see `docs/development/SDK-API.md`, section 12. New namespaces (llm, speech, markdown and others) are designed there and are not part of this `iyakki.d.ts` until they are built.

## Where to read more

`docs/DEVELOPER-MODE.md` (pairing, scopes, every `iyakki dev` command), `docs/APP-AUTHORING.md` (tile bridge, packages, store), `docs/development/CLAUDE-CODE-APP-DEVELOPMENT.md`,
`docs/development/CLAUDE-CODE-OS-DEVELOPMENT.md`, `docs/development/SDK-API.md` (the design of the whole SDK), `docs/LOCAL-TEST.md`.
