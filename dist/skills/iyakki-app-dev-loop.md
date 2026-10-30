---
name: iyakki-app-dev-loop
description: The edit, push, watch, verify loop for building an IYAKKI app (tile app, skill, MCP server) on your own computer with the SDK and developer mode, up to a signed package for a private store
triggers: iyakki sdk, sdk init, dev push, live reload, tile app loop, build iyakki app, app on the device, iyakki pkg
---
Use for an IYAKKI app built from your own computer against a real device (inside the IDE use iyakki-tile-author; what to build: iyakki-app-author; the device commands: iyakki-dev-mode). ACT yourself.

1. Start: `iyakki sdk doctor` says what is missing and the next step. `iyakki sdk init NAME --kind tile|skill|mcp|service` makes the project with tests, CLAUDE.md, AGENTS.md, iyakki.d.ts (typings of window.iyakki) and a .gitignore that keeps keys out. It refuses a folder that has files. `iyakki sdk docs [topic]` lists the offline guides.
2. Pair once (iyakki-dev-mode step 1). `iyakki dev whoami` must show developer mode on.
3. The loop, one small change at a time:
   a. Edit. A tile app is plain HTML, CSS and JS: no server, no Dockerfile, real button/input elements, files under 500 KB.
   b. `iyakki dev push . --watch --open` packs and installs it as an unsigned dev app and reloads an open copy at every save. A failing build is reported and the watch goes on.
   c. `iyakki dev logs NAME --follow` shows install messages and the page console (console.log, errors).
   d. Verify on the real screen: `iyakki dev screenshot -o /tmp/s.png`, `iyakki dev input click "LABEL"`, `input text`, then `state` for the texts and buttons. Seed data with `iyakki dev app config NAME set KEY VALUE` and read it with `app data NAME`; `app clear-data NAME` starts clean.
4. Tests: tests/*.js with T.test, T.click, T.type, T.text, T.eq, T.ok run headless; keep them green before every push. Test the real flow, not only that the page loads.
5. Declare only used capabilities in app.json: storage, clipboard, notifications, files, agent, and network as a list of exact hosts (default none). There is no vault capability and no eval.
6. Check: `iyakki pkg validate .` runs the install review checks, a secrets scan and the licence list; fix every PROBLEM.
7. Ship: raise the version in iyakki-app.json (a lower version is refused, an equal one reinstalls), `iyakki pkg keygen NAME` once (keep NAME.key private, never commit it), `iyakki pkg pack . --key NAME.key -o NAME.iyp`, then install with `iyakki dev install NAME.iyp` or publish to the private store.
8. Never: real secrets in the project, remote scripts or fonts, a vault read, a restart of the device, a guess about what the screen shows (take a screenshot).

Board: track steps with board_add (done only after a passed check). Bugs and failures: own board row with a B- id (never F-); bug_report captures logs; fix, verify, attach the passing run.

Example: "a timer tile" -> sdk init, push --watch --open, click Start, `state` shows the counting text, a T.test for it, validate, pack, install.
