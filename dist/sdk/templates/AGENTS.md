# Agent instructions: __TITLE__ (IYAKKI __KIND__ project)

(The same instructions as CLAUDE.md, for agents that read AGENTS.md.)

You are building `__NAME__` for IYAKKI, an OS for agents. Work in small steps, run the checks, report what you saw.
Full guide (if this machine has the IYAKKI sources): `docs/development/CLAUDE-CODE-APP-DEVELOPMENT.md`. Run `iyakki sdk docs` to list the guides.

## Rules for every kind

* Plain files, standard library or built-ins only. No dependency without asking the person. Permissive licences only.
* Never put keys, tokens or passwords in files. `*.key` is in `.gitignore`.
* Nothing runs on the device unless the person allowed this computer to (`iyakki dev whoami` lists the scopes it holds).
* **Permissions: ask the person.** If a command says Denied, Blocked, "waiting for approval" or a scope is missing, STOP and tell the person
  in plain words what you need and why (for example: "On the device open Settings, Developer mode, and add the scope `input` for this computer").
  Do not retry, do not work around it, never press Allow on the device for them unless they said to.
* Stop after 8 attempts without progress, or when the same failure happens twice, and report.
* Before you start, write down the check that proves you are done (a test name that passes, a text on screen, a log line).

## The loop (a paired device)

1. `iyakki sdk doctor` once: it says if a device is paired and what to do next.
2. `iyakki dev push . --watch --open` : packs and installs the folder as an unsigned dev app and reloads on every change.
3. `iyakki dev logs __NAME__ --follow` : the app's console and install messages. Look here first.
4. Run the tests (below), then `iyakki dev screenshot -o shot.png` and look at it only if text cannot answer.
5. Change one thing, repeat. Report: what you ran, test counts, what you did not test.

<!-- kind:tile -->
## This is a tile app

* Files: `index.html`, `app.js`, `style.css`, `app.json` (id, name, version, capabilities), `iyakki-app.json` (store metadata), `tests/app.test.js`.
* The page engine draws text, headings, lists, tables, boxes, simple flex rows and real form controls. No canvas. Plain elements take no clicks: use `<button>`, `<input>`, `<select>`, `<a>`.
* The bridge `window.iyakki` is typed in `iyakki.d.ts`. Everything is off unless declared in `app.json` `capabilities`; undeclared calls throw. Keep data with `iyakki.storage`, not `localStorage`.
* Test headless on the real engine: **tile_test** (in the IDE or through the assistant; files `tests/*.js` use `T.test`, `T.click`, `T.type`, `T.text`, `T.exists`, `T.eq`, `T.ok`). Lint with `tile_validate`.
* Ship: `iyakki pkg validate .`, then `iyakki pkg keygen me` once (keep `me.key` private), `iyakki pkg pack . --key me.key -o __NAME__.iyp`, `iyakki pkg verify __NAME__.iyp`, then `iyakki pkg publish --server URL --token-env STORE_TOKEN __NAME__.iyp`. Raise `version` in `iyakki-app.json` for every release.
<!-- /kind -->
<!-- kind:skill -->
## This is a skill

* `skill.md` is text the assistant reads (no code, no access). Keep it short and concrete: when to use it, the steps, what to check.
* Test: `python3 tests/test_skill.py`. Check: `iyakki pkg validate .`. Install to the device to try it: `iyakki dev push .`; see it with `iyakki dev list` and `iyakki dev logs __NAME__`.
* Ship: `iyakki pkg keygen me` once, `iyakki pkg pack . --key me.key -o __NAME__.iyp`, `iyakki pkg verify __NAME__.iyp`, `iyakki pkg publish ...`. Raise `version` for every release.
<!-- /kind -->
<!-- kind:mcp -->
## This is an MCP server

* `server.py` (or `server.js`) speaks MCP over stdio, standard library or built-ins only. `iyakki-app.json` names `runtime`, `entry` and `permissions` (`read:/abs/path`, `net:host`, `run:program`; wide ones like `net:*` are refused). Ask for the least.
* Test with the fake client: `python3 tests/test_server.py` (or `node tests/test_server.js`). Add a test for every tool.
* Check: `iyakki pkg validate .`. Try on the device: `iyakki dev push .`, then `iyakki dev logs __NAME__`.
* Ship: `iyakki pkg keygen me` once, `iyakki pkg pack . --key me.key -o __NAME__.iyp`, `iyakki pkg verify __NAME__.iyp`, `iyakki pkg publish ...`.
<!-- /kind -->
<!-- kind:service -->
## This is a service for k3s

* Listen on `0.0.0.0:$PORT`, answer `GET /healthz`, log to stdout, stop on SIGTERM. Dockerfile: pinned small base (never `:latest`), a `test` stage, non-root runtime. Manifests in `deploy/` (no Namespace), tests in `tests/`.
* Unit test: `python3 tests/test_server.py`. Then build the `test` stage in a container before anything ships.
* Never start, install or stop a cluster, never delete a namespace. Say which cluster you use. Stop after three failed deploy attempts and report.
* A service can have a tile front end (a second project from `iyakki sdk init NAME-ui --kind tile`) that declares `network` for the service host. Deploying through the install wizard with its permission list is designed, not built yet.
* `iyakki pkg validate` and `iyakki dev push` apply to the tile front end, not to the service.
<!-- /kind -->
