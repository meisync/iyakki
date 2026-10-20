# Making apps for IYAKKI: skills, MCP servers, tools and extensions

Everything that extends IYAKKI is a **package** (docs/STORE.md, sections 1 and 7). This page is the workflow for making one and
shipping it to a private store (docs/STORE.md, section 13) or a local catalog. The assistant knows it too: three built-in skills teach it
(`iyakki-app-author`, `iyakki-mcp-author`, `iyakki-skill-author`; they live in `dist/skills/` and are compiled in like the `lang-*` skills),
and the IDE starts it for you.

## The loop

1. **Scaffold.** IDE: top bar `⋯`, *New app, MCP server or skill…* (or Ctrl+Shift+N): choose the kind, give a lowercase name. The project is created
   in the workspace and the chat opens with the next step; the authoring skills are pinned for any workspace that holds an `iyakki-app.json`.
   Assistant: `app_scaffold {kind, name, dir}` (asks; writes only inside the workspace; the folder must be new or empty).
2. **Edit** with the agent. Keep it small; least permissions.
3. **Test.** Each scaffold has its own test: `python3 tests/test_skill.py`, `python3 tests/test_server.py`, `node tests/test_server.js`,
   `python3 tests/check_tool.py`. The MCP ones drive the real server with a fake client over pipes.
4. **Validate.** `iyakki pkg validate .` or the tool `store_validate {path}`: manifest, size, permissions in plain words, licence against the permissive list
   (MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib, CC0-1.0, 0BSD, Unlicense, BSL-1.0), a scan for secrets (private keys, access tokens, hard-coded passwords:
   the line is named, the secret is never printed), the name against look-alikes.
5. **Pack and sign.** `iyakki pkg keygen me` once (keep `me.key` private; never commit it), then `iyakki pkg pack . --key me.key -o app.iyp`,
   then `iyakki pkg verify app.iyp`.
6. **Try it here.** STORE, Settings, *Install from a file*, or add a local folder catalog with `iyakki pkg catalog-add`.
7. **Publish.** `iyakki pkg publish --server URL --token-env STORE_TOKEN app.iyp`, or `store_publish` (always asks the person; the token comes from the vault entry
   `store-token-<catalog id>`).
8. **Next version.** Raise `version` in `iyakki-app.json` (versions only go up), add to `CHANGELOG.md`, repeat.

## The project file

`iyakki-app.json` in the project root:

```json
{"kind":"mcp","name":"my-tool","version":"0.1.0","license":"Apache-2.0","title":"My tool","summary":"One sentence (140 characters at most).",
 "permissions":["net:api.example.com"],"min_version":"","categories":["tools"],"tags":[],"entry":"server.py","runtime":"python3"}
```

`kind` is `tile`, `skill`, `tool`, `mcp`, `agent`, `template`, `theme` or `extension` (for `extension` add `target`: dash, tales, flows, ide, chat, any, and `ext_kind`:
connector, panel, preset, node, ...). `entry` is the file that holds the payload (the skill Markdown, `tool.json`, the server source, or the JSON of an agent preset, template,
theme or extension); a `tool` also names its `module` (`.wat` or `.wasm`); an `mcp` names its `runtime` (`python3` or `node`). `README.md` becomes the store description and
`CHANGELOG.md` the change log. `permissions` use the store vocabulary: `read:/abs/path`, `write:/abs/path`, `net:host`, `vault:name`, `run:program`; wide ones
(`net:*`, `read:/`) are refused.

## What each kind becomes

| Kind | Scaffold | In the package | Installing |
|---|---|---|---|
| skill | `skill` | the Markdown (body at most 3950 characters, ASCII, at most 12 triggers) | SKILLS store; the review shows the whole text |
| mcp | `mcp-python`, `mcp-node` | `{"name","command":"python3"\|"node","args":["-c"\|"-e", <source>]}` | the MCP tile's servers; the review shows the program it runs |
| tool | `wasm-tool` | manifest and module (sandbox: fuel, memory, time; capabilities declared) | tool store; every call still asks |
| tile | `tile` (`tile_scaffold`) | a bundle: `app.json` and the web files | a tile in the launcher (below) |
| agent, template, theme, extension | write the JSON yourself | the JSON | kept in the store's content folder; the tile that extends reads it |

An MCP package carries the server's source, so it works on any device that has the runtime; keep dependencies to the standard library (the templates do).

## Rules the templates and the skills teach

Never embed secrets (read them from the vault or the environment); least permissions; validate every input against the schema before acting; no shell, no `eval`;
a time limit and a size limit on every tool call; only protocol messages on an MCP server's stdout; a test for every tool and every refusal.

## Where the code is

`store/author.rs` (scaffold, pack, publish client), `store/vet.rs` (checks), `store/authtools.rs` (the tools), `dist/templates/` (the four scaffolds), `dist/skills/iyakki-*.md`,
`projectstate.rs` (pinned skills), `apps/ide.rs` (the New app entry).


## Tile apps: an app for the IYAKKI launcher

A tile app is plain HTML, CSS and JavaScript that gets **its own tile** in the launcher and runs on IYAKKI's own page engine (the one the BROWSER tile
uses), full screen, with a standard bar: *custom app* mark with publisher and trust level, **Reload**, **Info** (what it may do, in plain words, and the
calls it made that were refused) and **Remove**; **Back** is the header's button. It is *not* a web service: no server, no Dockerfile, no k3s.
If you ask the assistant for "a calculator app for iyakki", this is what it builds.

**Files.** `index.html` (required), any `.js`, `.css`, `.json`, `.svg`, `.txt`, `.md`, `.csv`, `.png`, `.jpg`, `.gif`, `.webp`, `.woff2`, `.ico`; at most 64 files,
512 KB each, 1.5 MB in all; no hidden files or odd names. `app.json` is optional (the folder name and defaults are used without it):

```json
{"id": "calc", "name": "Calculator", "version": "1.0.0", "icon": "🧮", "group": "My apps",
 "capabilities": {"storage": true, "network": ["api.example.com"], "clipboard": false, "notifications": false, "files": false, "agent": false}}
```

`id` is the package name and the app's private origin (`https://tile-<id>.iyakki.invalid/`); `icon` is one emoji or a small `.png`/`.svg` in the app (the
launcher tile shows the first letter of the name in the app's colour, the icon appears in the info panel); `group` is a home screen section (Home, Web & Media,
Work, System; "Assistant" means Home) or anything else, which puts the tile in **Work**. Everything is off unless declared. `vault` and `js-eval` are refused by name.

**What the page can draw.** The reader engine's layout: text, headings, lists, tables, boxes, colours and sizes from CSS, simple flex rows, real form controls.
There is no canvas, and plain elements take no clicks: use `<button>`, `<input>`, `<select>` (a click moves to the next choice), `<textarea>`, checkboxes and `<a>`.
Use a table for a keypad. `localStorage` exists but lives only while the app is open: keep data with `iyakki.storage`.

**The bridge `window.iyakki`** (frozen; every argument is checked in the page and again on the host; at most 200 calls a second):

| Call | Needs | What it does |
|---|---|---|
| `iyakki.app.info()` | nothing | `{id, name, version, publisher, trust, dev, capabilities}` |
| `iyakki.storage.get(k)`, `set(k, v)`, `remove(k)`, `list()` | `storage` | the app's own data for this person: keys of letters, digits and `. _ : -` (64), values up to 64 KB, 256 KB and 256 keys in all; `QuotaExceeded` past that |
| `iyakki.ui.toast(text)`, `iyakki.ui.title(text)` | nothing | a short message (200) / a new title in the header (60) |
| `iyakki.host.openFile()` | `files` | a promise; the person picks a text file (up to 256 KB) from their Uploads folder in a dialog; `{name, text}` |
| `iyakki.host.saveFile(name, text)` | `files` | a promise; the person is asked and chooses where it is saved; the app never sees a path |
| `iyakki.clipboard.read()`, `write(text)` | `clipboard` | promises; each asks the person |
| `iyakki.notify.show(title, body)` | `notifications` | a note in the person's inbox (5 per run) |
| `iyakki.agent.ask(question)` | `agent` | a promise; the person sees the question and must allow it **every time**; the assistant gets only the question |

Calls without the capability throw with the words "this app did not ask for …"; the refusals are listed in Info. At most four requests wait for the person at once.

**Security.** The app's origin is its own, so it shares nothing with a site, the shell or another app (each person's saved data is separate too). Its page gets the
standard web modules and the bridge, nothing else: no vault, no other tile, no `file:` address. Its network is **only the hosts it declared that the person's host policy
also allows**; none by default. Code is never made from text: `eval`, `new Function` and the other function constructors throw (decision: this is strict and has no
opt-out in this version). Scripts and styles from other hosts are a lint problem unless declared. A dev app is marked **UNSIGNED DEV APP** everywhere.

**The loop.** `tile_scaffold {name}` (or the IDE's New form, "IYAKKI tile app", the default) writes `index.html`, `app.js`, `style.css`, `app.json`, `iyakki-app.json`,
`README.md` and `tests/app.test.js`. Test with `tile_test {path}` (IDE: Tests, or *Validate and test app…*): `tests/*.js` run **headless on the real engine** in the
loaded page with `T.test(name, fn)`, `T.click(sel)`, `T.type(sel, text)`, `T.text(sel)`, `T.exists(sel)`, `T.eq(a, b)`, `T.ok(cond)` and `iyakki` as the app uses it; they share one
page. `tile_validate {path}` lints. **Run** (F5) opens it live as an unsigned dev app. **Add tile** / `tile_install_dev {path}` (administrators; always asks) puts it on your launcher as a
dev app that reloads when its files change. `tile_pack {path}` packs and signs it. The samples (calculator, notes, converter in `dist/samples/tiles/`) are in the built-in
catalog and are a good start.

## Targets: what a workspace is for

A request such as "create an app" is ambiguous, so each IDE workspace has a **target** in `.iyakki/project.json` (`"target"`): `iyakki-tile`, `k3s-service`, `mcp-server`,
`skill`, `wasm-tool` or `library/other`. The New form offers *IYAKKI tile app* first and by default; the header chip shows the target and opens **Change target**
(under ⋯ on a narrow window). The assistant resolves a request in this order when no target is set: it mentions *iyakki*, *tile*, *launcher*, *add tile*, *for iyakki* or
*in the OS* → tile app (no question); an explicit *mcp*, *skill* or *wasm* → that; *k3s*, *kubernetes*, *docker*, *deploy*, *service*, *api* → service; both iyakki and k3s words, or
neither → it asks **one** question (ask_user: IYAKKI tile app, Web service on k3s, MCP server, Skill). A tile workspace has the tile skill and tools, board rows "build, test
(headless), install to the launcher, verify", **no** deploy tools (a call is refused with "this workspace targets an IYAKKI tile app; change the target to deploy to k3s") and the board
refuses cluster, image, registry and deploy rows. **Change target** on a k3s workspace shows the plan (files to drop such as `server.js` and `Dockerfile`, cluster rows that become
deprecated, the old dev namespace), then asks the assistant to convert the project; only `k8s_teardown` (which always asks) stays available, to remove that one namespace.

## Publish to a private store from the IDE

For a tile app, skill, MCP server or WASM tool, the header's **Publish** button (also *Publish…* in ⋯ and the assistant's `app_preflight`, `app_bump_version`, `tile_pack` and
`store_publish`) walks the whole route:

1. **Checks.** Validation problems (with *Ask the assistant to fix*), a title, a summary, an icon for a tile app, and the headless tests must pass.
2. **Store and version.** The dropdown lists the stores added on this device, private ones first, with address, key fingerprint and ★ for the default target; or
   *Deploy a new private store to k3s…* (the platform wizard, then press Publish again) or *Export package file* (a signed `.iyp` next to the project, nothing sent). The version
   offers *Keep*, *Patch*, *Minor*, *Major*; the store's `catalog.json` is asked for what it already has, and a version that is not higher is refused with the way out. Raising
   the version writes `iyakki-app.json`, `app.json` and a `CHANGELOG.md` line (required).
3. **Signing key.** Pick one of your publisher keys or create one; the private half is a 0600 file in your data folder (`publisher-keys/<you>/`), never in a workspace or git. The
   fingerprint is shown. A key the store has not seen is **new**: the package is listed as **Community** until the store's administrator adds the key to `trusted-keys.txt`
   (*Copy key for the store admin* copies the line).
4. **Token.** The vault entry `store-token-<catalog id>`; if it is missing a masked dialog stores it. It is never shown to the assistant or written to a log.
5. **Review card.** Server (a plain `http` address is warned about: only loopback and private addresses are accepted), package, version, kind, permissions, the trust level it will get,
   size and the files. *Approve and publish* is the only thing that sends anything; the assistant's `store_publish` asks the same way and no mode skips it.
6. **Result.** Listed as … with the catalog number, the withdrawal note (an administrator can withdraw it; copies already installed stay), and *Install it here to test*: the normal install
   review from that store on this device; a tile app is then opened. `.iyakki/publish.json` keeps store, version, time and hash (no secrets) and the header shows
   *Published: 1.0.2 → Office store*, with *update available* when the workspace is ahead.

Not covered: signing with a hardware key, a store that needs a client certificate, publishing from a read-only workspace.


## One step for an agent: `tile_ship`

The AI IDE's agent (the built-in assistant, Claude Code or Copilot through the bridge) publishes a tile app with one tool, `tile_ship {path}`
(`store_publish` also takes a project folder now): it runs the preflight and headless tests, raises the patch version when the store already
has this one, packs, signs and publishes, after ONE approval card (name, version, permissions in plain words, the store, the trust it gets).
Nobody is asked for anything else:

* **Signing key.** If the person has none, IYAKKI makes `my-publisher-key` (the default) in `publisher-keys/<user>/` of the data folder, a 0600
  file outside every workspace (this is where the Publish dialog's keys live too; the vault is not used because a locked vault would block
  signing). The agent never sees it, only the signature. Its public half is trusted on the device for packages that key signs, so the
  person's own app installs as Trusted; no other key, store or catalog is touched.
* **Publish token.** A private store that IYAKKI deployed on this device keeps its admin token in the vault (`svc.<instance>.admin-token`);
  once the store is added in STORE it is used as the publish token. A token the person typed (`store-token-<catalog id>`) wins.
* **No store yet.** The tool answers with the one sentence for the person: "Deploy the private store first (STORE → Private store → Deploy;
  one click now creates the registry)", or the next step when it is deployed but not added.

A tile is a web bundle (app.json, HTML, CSS, JavaScript) for IYAKKI's page engine: not Rust, Python, Docker or k3s (Rust is for IYAKKI itself and
for wasm tools).
