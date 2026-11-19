# Making an app for IYAKKI: the procedure

Status: written 2026-11-14 for the alpha.3 baseline. Every new app follows it from the first line; the existing built-in apps are moved onto it in alpha.4 (section 10).
Written for a developer who has not worked in this repository before. Read section 1 and 2 first (ten minutes), then follow the steps in order.

## 1. The one rule

**Everything goes through the IYAKKI core.** An app never talks to Linux or to a device. It asks the core, the core asks for the permission and writes the decision to
the access log, and only then does anything real happen (owner decision, 2026-11-14; design in [CORE-ACCESS-MODEL.md](CORE-ACCESS-MODEL.md)).

In practice, an app (any kind) does **not**:

* read `/proc`, `/sys` or `/dev`, or open a device node;
* start a program (`Command`, `spawn`, a shell command);
* open a network socket or make a web request on its own;
* read or write files outside the folders the core gives it;
* depend on `iyakki-agent`, `iyakki-shell` internals, `Services`, or a hardware crate (`iyakki-devctl`, `iyakki-usb`, `iyakki-storage` ...).

An app does:

* use the **handles the core gives it**: the bridge `window.iyakki` (web-bundle tile), `TileCx` sources (extracted Rust tile), `cx.os` (the machine, through
  `iyakki-core-api`) and the service call `call(domain, method, json)` (WASM tile);
* **declare** what it needs (capabilities, each with a reason a person can read);
* **fail closed**: no capability, no answer; a refusal is a normal result and is shown to the person (the Info panel lists refused calls).

A new need of the machine is never a direct call. It is a method on a core API trait, a capability name in the catalogue, a fake in `FakeOs` and a line in the access log
(section 5). The ratchets that enforce the rule may only shrink (section 8).

## 2. Choose the kind of app

| Kind | Use it when | Where it lives | Reaches the system through | Test it with |
|---|---|---|---|---|
| **Web-bundle tile** (default) | almost everything a person or a team writes: forms, lists, small tools, calculators, dashboards | a folder with `index.html`, JS, CSS, `app.json`, `tests/*.js`; no Rust build | the bridge `window.iyakki` and the capabilities in `app.json` | `tile_test` (headless, real page engine); `iyakki dev push --watch` on a device |
| **Extracted Rust tile** | a built-in app that must draw with `Gfx` (the shell's own screens: browser, editor, player, desktop views) | its own crate `crates/iyakki-tile-<name>` | `TileCx` (narrow `*Source` traits in `iyakki-tile-api`), and `cx.os` for machine facts | `TileHarness` + a fake source |
| **WASM tile** (experimental, browser) | a tile that must also run in the web UI | a Rust crate built to wasm32 over `iyakki-tile-kit`, binary `tile.wasm` | the tile ABI and `call(domain, method, json)`; the host checks the domain list in its manifest | native tests, the real host sandbox, the HTTP routes |
| Skill, MCP server, CLI tool, extension | no screen | see [APP-AUTHORING.md](../APP-AUTHORING.md) | its own protocol, through the assistant's tool calls | per its kind |

Rule of thumb: start with a web-bundle tile. Make a Rust tile only where `Gfx` is needed ([TILE-TESTING.md](TILE-TESTING.md)). Make a WASM tile only when the same screen must be
drawn in the browser by the same code.

## 3. Write the app down first (one page, in the matrix)

Before code, add a row to `FEATURES.md` (status `planned`, Impl 0, Test 0, a free id in the right area, e.g. `F-MYAPP-01`) and put these in the row or the app's README:

1. **Name and id** (lowercase letters, digits, `-`), the group it sits in on the home screen, the icon.
2. **Who may use it**: the role policy `App` entry (guests too?).
3. **Capabilities**, one line each with the reason the person will read (`camera.view` "show the live view"). Ask for the least. Capabilities with a hard ask (record, listen, `net.listen`,
   screen capture, input injection) show a card at every use, whatever was approved at install ([CORE-ACCESS-MODEL.md](CORE-ACCESS-MODEL.md) section 4).
4. **Data it keeps** and where (the app's own storage only; sizes).
5. **What it must never do** (the negative tests of section 6 come from this list).
6. **Its chat tools and its Settings controls** (section 7): every feature is reachable by the assistant (with approval where it changes something) **and** by the screen.
7. **What can only be tried by a person** (hardware, a real service) so the row says `Manual needed` honestly.

Use the next free `KI-` number in your range when you must record a known issue ([KNOWN-ISSUES.md](../../KNOWN-ISSUES.md)).

## 4. Build it

### 4A. Web-bundle tile (the default)

1. Scaffold: `tile_scaffold {name}` from the assistant, or the IDE's New form ("IYAKKI tile app"). It writes `index.html`, `app.js`, `style.css`, `app.json`, `iyakki-app.json`,
   `README.md` and `tests/app.test.js`.
2. Fill `app.json` with **only** the capabilities of section 3 (`storage`, `network` hosts, `clipboard`, `notifications`, `files`, `agent`, `llm`, `speech`, ...). Everything is off unless declared.
3. Reach the machine only through `window.iyakki` ([APP-AUTHORING.md](../APP-AUTHORING.md) lists every call and the capability it needs; [SDK-API.md](SDK-API.md) the details).
   No `eval`, no `new Function`, no scripts from other hosts, no paths.
4. Test: `tile_test {path}` runs `tests/*.js` headless on the real page engine with fake services (`T.fake.*`). Lint: `tile_validate {path}`.
5. Try it live: `tile_install_dev {path}` (administrators; asks) or `iyakki dev push --watch <folder>`. A dev app is marked **UNSIGNED DEV APP** everywhere.
6. Ship: `tile_pack {path}` packs and signs it; publish to a store from the IDE.

### 4B. Extracted Rust tile (built-in apps)

Model to copy: `crates/iyakki-tile-calendar`, `iyakki-tile-usage`, `iyakki-tile-cameras` ([COMPILE-SPLIT-PILOT.md](COMPILE-SPLIT-PILOT.md)).

1. New crate `crates/iyakki-tile-<name>`; add it to the workspace. It depends on `iyakki-uikit` and `iyakki-tile-api` **only** (an exception needs a line in
   `crates/iyakki-core/tests/data/tile_crate_deps.allow` with the reason; the list may only shrink).
2. In `iyakki-tile-api` add a trait for what the tile needs from the system, named for the job and not for the device (`CalendarSource`, `CameraSource`): plain types in, plain types out,
   no `Arc<Services>`, no paths. Add one field to `TileCx` for it. Add a field only when a tile needs it.
3. The tile implements `iyakki_tile_api::App` and draws with `iyakki_uikit`. It holds no state about the machine; it asks its source.
4. The real source lives **outside** the tile, in the shell or the agent: it implements the trait over the real thing and is where capability checks and access-log lines happen
   (through the core API, section 5). The shell wraps the tile in an adapter and registers it in the launcher.
5. Test next to the tile with a **fake source** and `TileHarness` ([TILE-TESTING.md](TILE-TESTING.md)): click by label, type, assert the drawn text, no overlap at the standard sizes.
   Run only your tile's tests: `cargo test -p iyakki-tile-<name>` and `cargo test -p iyakki-apps --lib -- apps::<tile>::tests`.

### 4C. WASM tile

Build over `crates/iyakki-tile-kit` (palette, clipping, buttons, JSON call helper, the six exports). The tile declares the **service domains** it may call; the host refuses any other
domain and any method not in the list, audits each call without content, and asks the person where the tool asks first ([TILE-ABI.md](TILE-ABI.md) sections 5 and 12). Examples: `iyakki-tile-runs`
(read only), `iyakki-tile-edit` (no service call), `iyakki-tile-settings` (one call, two switches that wait for the person's click). Commit the built `tile.wasm`; a test rebuilds it and checks both builds draw alike.

## 5. Needing something new from the machine (the only way)

1. **Name the capability** in the catalogue ([CORE-ACCESS-MODEL.md](CORE-ACCESS-MODEL.md) section 4): a name, what it gives, parameters, the built-in default, whether it is a hard ask. A catalogue change is an additive change;
   follow [CORE-COMPATIBILITY-DESIGN.md](CORE-COMPATIBILITY-DESIGN.md) (a removal needs a note and a deprecation entry).
2. **Add the method** to a trait in `crates/iyakki-core-api` (`Os`, `Sysinfo`, `Memory`, `Hardware`, `Audio`, `Disks`, `Net` or a new one). Reads of harmless facts may be free by the catalogue; **changes** (set a radio, brightness, swap,
   volume, a link; start, stop, install) need the capability and, where the catalogue says so, a per-use approval card.
3. **Implement it twice**: `iyakki-core-local` (the only code that touches `/proc`, `/sys`, `/dev`) and `iyakki-core-fake` (`FakeOs`, for tests). Nothing else may implement it over the real machine.
4. **Check and log**: the call passes the guard for the app's identity (`GuardedOs`, owner rule of 2026-11-14; until it covers a page, state that in the row), and the decision is written to the access log.
5. **Surface it**: the core API surface file is re-recorded with a note (`IYAKKI_CORE_SURFACE_WRITE=1 IYAKKI_CORE_SURFACE_NOTE="why" cargo test -p iyakki-core-api --test core_surface`); a C export needs its line in `docs/development/c-abi-symbols.txt`.

## 6. Isolation: the proof every app carries

An app is not finished until its tests show that the walls hold. Write these tests (they come from the "must never do" list of section 3):

| Claim | What the test does |
|---|---|
| It holds only the capabilities it declared | call every other capability: refused, the refusal is audited, nothing reaches the fake machine |
| A denied capability stays denied | the person answers No (or does not answer in 120 s): the call fails with `Denied`, nothing changed |
| A change asks first | a change to the machine shows an approval card; nothing happens before the click; one click is one change |
| The role policy still applies | an account the role forbids gets a refusal whatever the app holds |
| Its data is its own | storage written by the app is not readable by another app or person; the origin is its own (`https://tile-<id>.iyakki.invalid/`) |
| No direct route to the machine | the dependency ratchet and the boundary scan pass **with no new allow line**; the app's crate has no `iyakki-devctl`, `iyakki-usb`, `iyakki-storage`, `libc` ... |
| It cannot reach another app's tools or domains | a WASM tile calling a domain not in its manifest, a web tile calling a bridge call it did not declare: refused |
| It keeps secrets out | secrets never appear in a card, an audit line, a log line or a chat transcript (test with a marker string) |
| A guest or another person sees nothing of it | the route or tile answers 403/404 and nothing starts |

Look at it in a browser too: open the web UI in a phone-sized window (browser developer tools, mobile view, 390x844 and 360x740), pick the app, and check that it draws, scrolls and
fits, and that the refused calls show in its Info panel. The refused call is the proof, so make one on purpose (call something you did not declare) and read the refusal.
The checklist with the exact steps is in [APP-ISOLATION-CHECK.md](APP-ISOLATION-CHECK.md).

## 7. Reach it everywhere: chat and Settings

Owner rule: every feature is reachable by chat **and** by the screen.

* **Chat tool** for each action: read tools are free; tools that change something **ask first** with a card that says what will change; secrets are never arguments (a vault entry name is).
  Wire it into the assistant's tool list and its prompt rule, and add the tool to the table in [CHAT-REACHABILITY.md](CHAT-REACHABILITY.md) (the reachability test fails otherwise).
* **Settings control** for each choice the person can make (key and touch), laid out at the five standard sizes (the monkey/overflow tests catch overlap), with the same words as the chat tool.
* The two agree: an administrator-only control is administrator-only in chat too.

## 8. Checks before you ask for a review

Run these on your machine (Linux or macOS; the Linux gate on the PC is the final word):

```
cargo test -p <your crates>                                   # your tests, never the whole shell suite
cargo test -p iyakki-core --test feature_matrix --test boundary --test dependency_ratchet
cargo test -p iyakki-shell --test poison_scan --test safeclamp_scan
bash dist/release/check-linux-cfg.sh                          # both targets, zero warnings
bash dist/release/compat-check.sh                             # surfaces, core API snapshot, fixtures
python3 dist/features/migrate-impl-test.py                    # recomputes the matrix totals
```

Hazards that failed real gates (temp folders inside the repo, socket paths over 100 bytes, wall-clock races, font-dependent layout, shared files between tests): [LANE-RULES.md](LANE-RULES.md).
New code adds no line to `kernel_readers.allow`, `shell_kernel_access.allow`, `tile_crate_deps.allow` or `BOUNDARY.md` unless the reason is written next to it; those lists may only shrink.

## 9. Done means

Copy this into the pull request and tick it honestly. "Done" for a row needs Impl 100 and Test 100 and no open manual test; most rows will say `partial` and that is fine if the Notes say what is missing.

- [ ] Row in `FEATURES.md` (Impl, Test, Manual set by what the tests really prove; Notes name the missing parts)
- [ ] Kind chosen (section 2); the app depends only on what the kind allows
- [ ] Capabilities declared, each with a reason; the least that works
- [ ] No direct `/proc`, `/sys`, `/dev`, spawn, socket or file access; no new allow line anywhere
- [ ] Every machine need is a core API method with `iyakki-core-local` and `FakeOs` implementations, a capability and an access-log line
- [ ] Isolation tests of section 6 written and passing
- [ ] Chat tools and Settings controls for every action and choice; reachability table updated; changes ask first
- [ ] Tested at the five standard sizes and in a phone-sized browser window where the app is shown there
- [ ] The checks of section 8 pass; zero warnings
- [ ] Known problems written down with a `KI-` number; nothing claimed that was not seen
- [ ] A person who has not written the app can start it from the README

## 10. Moving an existing built-in app onto this standard (alpha.4)

The files under `crates/iyakki-apps/src/apps/` are the built-in apps; they were written before the split and still receive the shell's whole `AppCx` (`host`, `services`, `web`, `chat`, `store` ...). The first one moved is RADIO (section 12 is that worked example, with what the first draft of this section got wrong). To move one:

1. **Inventory** (an hour): list every use of the machine and of the shell context. Two kinds, and only the first has a ratchet:
   * kernel and hardware: `grep -n '"/proc\|"/sys\|"/dev\|Command::new' apps/<tile>.rs` (the ratchet file `shell_kernel_access.allow` counts exactly these, and `BOUNDARY.md` names the files that spawn);
   * the shell context and the services: `grep -n 'cx\.\|Services\|iyakki_agent::' apps/<tile>.rs`. **No ratchet counts these**, so a tile can have an empty line in `shell_kernel_access.allow` and still be wired to everything (RADIO had no line there and 53 uses of the context).
   Also look for the hidden spawns: a call into an agent type that starts a program or opens a socket on its own (MCP's `McpClient::connect` ran the server as a child process from a thread of the tile, and no ratchet saw it): `grep -n '::connect(\|thread::spawn\|spawn(' apps/<tile>.rs`. Those belong behind the port too.
   Write one line per use in the row's Notes. Run `python3 dist/scripts/app-inventory.py` for the numbers of every app at once (APP-MIGRATION-INVENTORY.md).
2. **Pick the replacement per use.** Three answers, not two:
   * a fact about the machine, or a change to it (a device, a link, a volume of the card, a file outside the person's folders) → a core API method (section 5), with the capability and the log line;
   * something the shell already runs in process for the person (a player, a store of the person's own list, a search thread, the audit log) → **a port trait that the tile owns**, named for the job (`RadioPort`), with plain types in and out;
   * a data source shared by several tiles (calendar, cameras, usage) → a `*Source` trait in `iyakki-tile-api`.
   A tile that talks to an in-process player does not need a core API method; asking for one would send you to the wrong place.
3. **Write the port, then the real implementation, then change the tile** (one change):
   * the port is one trait in `apps/<tile>_port.rs` (later `iyakki-tile-api`); every method the tile calls on `cx.services`, `cx.audit`, `cx.session` or a store becomes a method of it, including the person's host policy and the audit line, so the tile cannot forget either;
   * the real port is a small struct that borrows the `AppCx` (`CxPort(&cx)`): the only code that names `Services`. Name the port variable `port`, not `p`: MEDIA's tile has locals called `p` (a playlist, a picker) that the mechanical rename ran into six times;
   * when the tile keeps objects of its own for the person (MEDIA keeps the playlists and folders, the library index and the download queue), the port hands them over once in one struct (`MediaParts`) when the tile opens, and asks the port only for what depends on the account afterwards (who, what may be read and written, the context of a download);
   * logic that the tile and the chat both need goes into the agent once and both call it (`mcp::visible_servers`, `mcp::origin_of`): the chat tool and the tile cannot then disagree about which servers an account sees;
   * the tile keeps its `App` trait methods, which build the real port and call `*_with(&dyn Port)` versions holding the old bodies. Tests call the `_with` versions with a fake port. Put the fake-port tests in a child module of the tile (`#[cfg(test)] #[path = "<tile>_fake_tests.rs"] mod fake_tests;`) so they can call the tile's private methods (`rate_selected`, `start_queue`) directly; the old tests stay as they are and run through `CxPort`, which proves the port changed nothing.
   * the number of `AppCx` mentions a migrated tile keeps is one for the import plus one for each `App` trait method that takes the context: RADIO 4, MCP 3, MEDIA 7. A mechanical rename script does most of the work (replace `cx.services...`, `cx.audit.record(&cx.session.user, ...)`, `cx.session.policy.allows_read(&cx.session.user, ...)` and the `cx: &AppCx` parameters), then the compiler lists the rest; plan an hour for a tile of 500 lines and half a day for 2,000.
4. **Extraction into its own crate** (4B) is a separate change and has three more preconditions that the port does not give you: the data types the tile shows (stations, states, problems) live in `iyakki-agent` and must move to `iyakki-tile-api` or a leaf crate first; the shared shell widgets it uses (forms, the sound dialog, the neon painting) must be available to a crate that depends on `iyakki-uikit` only; and the tile's old tests that use `Services` stay in the shell as adapter tests. Do not start this until the port is in and green.
5. **Lock it in.** Add the tile to `crates/iyakki-core/tests/data/migrated_tiles.list` (path and the number of `AppCx` mentions it keeps, which is the entry points of the `App` trait). The `dependency_ratchet` test then fails if the tile names `Services`, `.services` or another handle of the shell context again, or if the number can be lowered. Remove lines from `shell_kernel_access.allow`, `kernel_readers.allow` and `BOUNDARY.md` only where the tile had them.
6. **Isolation tests** (section 6) and **parity** (section 7). For a built-in tile the table of section 6 reads: capabilities (rows 1 to 3) apply where the tile asks the core for one, otherwise the role policy and the host policy are the walls; data (row 5) is the person's own folder; the no-direct-route row is the `migrated_tiles.list` test. Parity: list the keys of the tile's `help()` and put the chat tool next to each key that changes something; a key with no tool is a gap, and a change that writes no audit line is a bug.
7. Update the row (`F-ARCH-0x`, the tile's own row) and the plan numbers in [UI-OS-SPLIT-PLAN.md](UI-OS-SPLIT-PLAN.md) / [CORE-LOCAL-EXTRACTION-PLAN.md](CORE-LOCAL-EXTRACTION-PLAN.md).

Order: tiles that already read through `cx.os` first (Memory, Network, Display, Sound, System); then tiles with few direct uses; the browser, editor, IDE and shell chrome last. One tile per change, one change per
bundle, gates green each time. A migrated tile is not "done" until it is on `migrated_tiles.list`, its fake-port tests and isolation tests pass, and (for extraction) its crate depends on `iyakki-uikit` and `iyakki-tile-api` only.

## 11. Where to look

| Topic | File |
|---|---|
| The rule, capabilities, broker, manifest, access log, app bundles, install wizards | [CORE-ACCESS-MODEL.md](CORE-ACCESS-MODEL.md) |
| Compatibility: versions, additive changes, deprecation, rollback | [CORE-COMPATIBILITY-DESIGN.md](CORE-COMPATIBILITY-DESIGN.md) |
| Web-bundle apps: files, `app.json`, every bridge call, the loop | [APP-AUTHORING.md](../APP-AUTHORING.md), [SDK-API.md](SDK-API.md), [SDK-BRIDGE-V2.md](SDK-BRIDGE-V2.md), [SDK-EMULATOR.md](../SDK-EMULATOR.md) |
| Rust tiles: harness and the facade | [TILE-TESTING.md](TILE-TESTING.md), [COMPILE-SPLIT-PILOT.md](COMPILE-SPLIT-PILOT.md) |
| WASM tiles and the browser | [TILE-ABI.md](TILE-ABI.md), [TILE-FONTS.md](TILE-FONTS.md) |
| The split plan and what is left | [UI-OS-SPLIT-PLAN.md](UI-OS-SPLIT-PLAN.md), [CORE-LOCAL-EXTRACTION-PLAN.md](CORE-LOCAL-EXTRACTION-PLAN.md) |
| Chat reachability table | [CHAT-REACHABILITY.md](CHAT-REACHABILITY.md) |
| Rules and hazards for contributors | [LANE-RULES.md](LANE-RULES.md), [CONTRIBUTING.md](../../CONTRIBUTING.md) |
| Examples to copy | `crates/iyakki-tile-calendar`, `iyakki-tile-usage`, `iyakki-tile-cameras`, `iyakki-tile-runs` (WASM), `dist/samples/tiles/` (web-bundle) |

## 12. Worked example: RADIO (2026-11-14)

RADIO (`apps/radio.rs`, 1,650 lines with tests, one of the apps with the most live parts: a player, a station list, a directory search thread, a speaker dialog, a certificate dialog) was moved to step 3 of section 10 in one change.

| | Before | After |
|---|---|---|
| Direct kernel access (the ratchet) | none, no line in `shell_kernel_access.allow` | none |
| Names of the shell context in non-test code | 53 uses of `cx.services` (29), `cx.session` (11), `cx.audit` (6), `store(cx)` (7) | 0; `AppCx` named 4 times (the import and the three `App` methods) |
| What a test needs | a signed-in `AppCx`, a temp folder, a `Services`, a fake streamer | `FakePort` (a struct of mutexes) |
| Tests | 13 through the real services | the same 13, unchanged, now run through `CxPort`, plus 7 with the fake port (keys and volume, host refusal, add and its error, removal asks first, search, certificate trust, drawing in every state at four sizes) |
| Chat parity | tools for play, stop, pause, resume, volume, status, add, save, search, trust | and **radio_remove** and **radio_forget_certificate** (the keys `d` and `f` had no tool); both ask first, are never covered by a standing grant, and are in the system prompt |
| Audit | add, play, search, trust, revoke | and **remove** (the key wrote no line before) |

Files: `apps/radio_port.rs` (the trait, `CxPort`), `apps/radio.rs` (the tile, `*_with` versions), `crates/iyakki-tile-radio/src/fake_tests.rs` (the fake port and its tests), `crates/iyakki-core/tests/data/migrated_tiles.list` and the `migrated_tiles_never_name_the_services_or_the_shell_context_and_the_list_is_real` test.

What the first version of section 10 got wrong, found while doing it:

1. It said "the ratchet files name the uses" and "remove the old lines": the ratchets count kernel and hardware access only, so a tile with no such use had nothing to remove and nothing that kept it migrated. The `migrated_tiles.list` ratchet is new for this reason.
2. It offered two replacements (core API or a `*Source` in `iyakki-tile-api`). A tile that drives an in-process player needs a third: a port the tile owns.
3. It put "move into its own crate" as step 4 of the same job. RADIO shows the crate move needs the data types (`Station`, `RadioState`, `CertProblem`, the language lists) moved out of `iyakki-agent` and the shell widgets (`Form`, the sound dialog, `neon`) made available first. That is its own change, not left to the end of this one.
4. It did not say to compare the keys with the chat tools. Two keys of RADIO had no tool and one changed the list without an audit line.
5. The isolation table of section 6 is written for bundled and WASM apps (capabilities declared by the app); for a built-in tile the walls are the role policy, the host policy and the person's own folder. Section 10 now says which rows apply.
6. It counted on a quick fix of the tile's tests; they did not need changes beyond the constructor (`RadioApp::new(&CxPort(&cx))`), which is why the port is the cheap first step and the crate move the expensive second one.

What is left for RADIO (not done): the extraction of step 4, the sound dialog (`soundask`) and the `Registry` of players still come from the shell and the agent (`soundhold`), and the add-on installer request still goes out through `take_request`.

### Second and third tile: MCP and MEDIA (2026-11-14)

| | MCP (`mcp.rs`, 442 lines) | MEDIA (`media.rs`, 2,169 lines) |
|---|---|---|
| Port | `McpPort` (servers, origin, may-manage, add, remove, **connect**, account, audit) and `McpSession` | `MediaPort` (user, policy, may read and write, `parts`, the download context, the planned folder, the web profile, audit) and `MediaParts` |
| What the tile did itself before | started the server program (`McpClient::connect`, a child process) from a thread of its own | nothing of that kind; it took its store, index and queue from `cx.services` and checked the policy in a dozen places |
| `AppCx` mentions after | 3 | 7 |
| Tests | the 3 old ones through `CxPort`, plus 4 with a fake port and a fake session (no server program, no folder) | the 25 old ones through `CxPort`, plus 5 with a fake port (a track plays with one audit line naming the file; a file the port refuses is never started; rating; download folders; drawing) |
| Chat parity | the keys `n` and `d` (add and remove a server) had no tool: `mcp_servers`, `mcp_server_add`, `mcp_server_remove` (the last two ask every time, administrators only, never covered by a standing grant) | done in the round before (next, previous, mode, playlists, the download queue) |
| Ratchet | `migrated_tiles.list` 3 | `migrated_tiles.list` 7 |

What these two changed in the procedure (the corrections are in section 10): the hidden spawn in a client type; the port name `port`; the "parts" struct for a tile that keeps objects of its own; one function in the agent for what the tile and the chat both decide; fake tests as a child module; how many `AppCx` mentions to expect. What is still true for both: the data types (`ServerSpec`, `Tool`, `MediaStore`, `MediaIndex`, `DownloadQueue`, `Library`) come from `iyakki-agent`, so neither tile can move into a crate that depends only on `iyakki-uikit` and `iyakki-tile-api` yet; the inventory lists the `iyakki_agent::` paths of every app (the column "agent paths") so that the size of that second step is known per app.

The crate step (step 4 of section 10) for RADIO is measured and ordered in [RADIO-EXTRACTION-PLAN.md](RADIO-EXTRACTION-PLAN.md): five changes, the first two (the data types and the speaker question) are most of the work.

### Fourth and fifth tile: SKILLS and DATA (2026-11-14)

| | SKILLS (`skills.rs`, 486 lines) | DATA (`data.rs`) |
|---|---|---|
| Port | `SkillsPort` (list, add, review, install, remove, trust key, trusted keys, the package a skill came from, `read_text_file`, audit) | `DataPort` (the environment of the view, the person) |
| What was hidden in the tile | it read the file the person named itself (scope check, size check, `std::fs`), now `read_text_file` of the port | nothing: the view already worked from a `DataEnv`; only the place that built it from the shell context was left |
| `AppCx` mentions after | 4 | 5 |
| Tests | the 2 old ones through `CxPort`, plus 5 with a fake port | the 17 old ones through `CxPort` or an env of their own, plus 3 with a fake port |
| Chat parity | the page was on the exemption list as `todo`: `skills_list`, `skill_add`, `skill_remove` now exist (the last two ask every time, the card shows the text, never covered by a standing grant); the exemption line is gone | had tools already |

Corrections these two added: a tile that is already built on an environment object needs only the port that builds it (DATA took an hour); the `Form` widget walks through its fields on Enter and submits on the last one, so a fake-port test of a form submits with `Ctrl+S` instead of counting Enter presses; and a tile that is on the reachability exemption list is the best next candidate, because the migration finds the missing tool in the same pass.

### AUTH and CAMERA (Q-C-9): two small tiles, port and fake tests in one sitting

| Tile | Port | What it asks | Tests |
|---|---|---|---|
| AUTH (`apps/auth/mod.rs`, port in `apps/auth/port.rs`) | `AuthPort`: `vault()`, `audit()` | the vault the codes are kept in, one audit line | 3 fake-port tests in `fake_tests.rs`; the 26 old tests untouched (they use `AuthApp::with_vault`) |
| CAMERA (`apps/camera.rs`, port in `apps/camera_port.rs`) | `CameraPort`: `user()`, `allows_write()`, `camera_source()`, `audit()` | the account name, the write policy, the camera in use (browser or machine), one audit line | 3 fake-port tests in `camera/fake_tests.rs`; the old rig tests untouched |

Corrections learned:
- A tile that already has a seam (AUTH's `with_vault`) needs only the missing pieces behind the port: the vault constructor and the audit.
- When a script moves a body into an inherent `*_with` method, check where it landed (the first run put CAMERA's body inside the next free function; the compiler caught it). Insert before the closing brace of the `impl` that holds the other private methods, not after the last `}` before `impl Drop`.
- Other callers of `Tile::new(&cx)` exist outside `shell.rs` (a voice test built `CameraApp::new(&cx)`); grep the whole crate for `Tile::new(` before the first build.
- A module the shell names (`CxPort`) must be `pub` or re-exported (`pub use self::port::CxPort;`), else the build says `AppCx: Port is not satisfied`, which reads like a missing impl.

### USERS (Q-C-9): the smaller seam first

USERS keeps the real account store behind the port (the store is the rule keeper and re-checks the actor on every change, so a fake port wraps a real `UserStore` in a scratch folder). `UsersPort` asks for: `me()`, `can_manage_users()`, `users()`, `create_user`, `set_password`, `set_role`, `set_overrides`, `delete_user`, `audit()` and the audit chain (`audit_verify`, `audit_recent`, `audit_start_new_log`). The two vault wipes (after a password reset and after a delete) moved into the real port, so the tile no longer names `Services`; the fake port records them and a test asserts they happen only on success.
- A Result from the store is passed through unchanged; the tile never sees the lock.
- The 11 old tests did not change (they go through the `App` trait, which wraps `CxPort`); 3 fake-port tests were added in `users_fake_tests.rs`.
- Still open for USERS: no `users_list` chat tool (the Account row of the chat reachability exemption stays).
