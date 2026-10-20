# Embeddable components and a swappable backend (design and audit)

Status: design, with a first cut of slices B0 and B1 built (see "Built so far" below); the rest is not built. Written 2026-10-14 against the working tree (code was changing while this was
written; line counts are approximate). Two requests drive it:

1. "Make every tile an embeddable component with parameter passing (the browser inside the AI IDE)."
2. "Keep an abstraction between frontend and core logic so the backend can later be remote and scalable."

The two are related but independent: part B (components) is about how UI pieces nest; part C (backend) is about what the
UI is allowed to call. They share one rule: **a component never reaches for process-global things, it is handed a context.**

---

## D. Decision list (read this first)

### Recommend now

| # | Decision | Why |
|---|---|---|
| 1 | Add a `Component` trait next to `App` (not replacing it). `App` becomes a thin "full tile" host around a `Component`. | Zero risk to the 20 tiles; each converts when someone needs it embedded. |
| 2 | Embed by the mechanism the IDE already uses (draw into a `R`, remap hit ids into a private id range), but generalise it: `STRIDE` pane codes (1..4) become a dynamic `ComponentSlot` allocator. | The IDE proves it works for chat+editor; the limit is 4 hard-coded panes. |
| 3 | First slice: Browser as an IDE bottom-panel tab, and a Data viewer inside chat (needs `iyakki-data`). | Smallest slices that prove "param passing" and "host reacts to child events" in both directions. |
| 4 | Create `iyakki-api` (traits + serde types only) and start with the Data service, then Models/Chat runs. | `iyakki-data` is being written now, so it is free to be born behind a trait; Chat runs are the first thing worth running remotely. |
| 5 | Add the dependency-rule test now with an allow-list that lists today's violations, and only ever shrink it. | Cheap, and the only thing that stops the coupling from growing while the refactor is under way. |
| 6 | Keep `iyakki-remote` (pixel streaming) as is. Remote backend is for logic, pixel streaming is for display; they combine, they do not compete. | See C.9. |

### Postpone

- A real network transport (HTTP/WebSocket/gRPC). Build the traits and `LocalBackend` first; prove the boundary with an in-process
  "loopback" backend that serialises every request to JSON and back (catches non-serialisable types for free).
- Floating panels / free window manager (hosting mode 3). Panes and tabs cover every use case asked for.
- Converting the big tiles (SETTINGS, MEDIA, INBOX) to components. They are not embedded anywhere; convert on demand.
- Multi-tenant scheduling, quotas, autoscaling. Design notes only (C.8).
- Moving `iyakki-js`, `iyakki-web` rendering and `iyakki-webview` behind the API: they are local-always (C.5).

### Risks

1. **Hit-id model.** Click ids are plain `usize` constants per module with hand-picked bases (`ide_k8s::TAB_BASE = 9_300_000`, `board::TAB_BASE = 9_400_000`, `chat_view::COPY_BASE = 50_000`, `browser::CONTROL_BASE = 1000`). Nesting a third level (browser inside IDE inside ...) or two browsers can collide. Mitigation: slot allocator (B.4) plus a test that renders every component twice in one frame and asserts no duplicate ids.
2. **`browser.rs` is 6,142 lines with ~70 fields in one struct** and the full engine (iyakki-webview helper process) is bound to one rectangle and one scale (`view_rect`, `ui_scale`). Embedding at a different size means re-sizing the engine; do it by reusing the existing "start at the size last drawn" path, and measure.
3. **Services are not yet per-call-site mockable.** 521 `services` accesses in the shell go straight to `Arc<Services>` fields. Any slice that touches those has to move them behind a trait *and* keep behaviour; the test suites (about 30% of shell lines are tests) are the safety net, so do not delete tests while converting.
4. **Sync-on-the-UI-thread calls.** Some service calls are plain mutex + file IO inside `handle`/`render` (settings, calendar, inbox listing). Fine locally, fatal remotely. The `Client` handle in C.4 forces the poll model; until a call is converted it stays on a `local_only` allow-list.
5. **Permissions drift.** An embedded component must not widen the host's scope (an IDE workspace is jailed to its folder; a browser embedded in it must not get the user's whole browser profile). Defined in B.8; needs a test per component.
6. **Remote data residency / secrets.** Vault and credentials must stay on the machine that owns them until we decide otherwise (open question 3).

### Open questions for the product owner

1. Is "remote backend" mainly (a) a team server many people sign in to, (b) a thin desktop/mobile client talking to the user's own box, or (c) bursting heavy work (models, indexing) to the cloud? The API is the same, auth and tenancy are not.
2. Should a browser pane inside the IDE share the user's normal browser profile (cookies, logins, bookmarks) or get a clean per-workspace profile? Recommendation: clean per-workspace by default, an explicit "use my profile" switch.
3. May the vault ever leave the user's device? Recommendation: no; remote runs receive short-lived, scoped secrets by reference.
4. Which embedding combinations are product features rather than possibilities (so we test them)? Proposed list: browser-in-IDE, data-in-chat, editor-in-chat (exists), media-in-chat, terminal-in-IDE (exists as bottom tab).
5. Does the assistant get to rearrange the user's screen (open panes) without asking, or always with consent? Recommendation: always a one-tap consent for the first embed of a kind per session (B.10).
6. Offline policy: is a partial UI (local files/editor working, chat greyed out) acceptable when the backend is unreachable? Recommendation: yes.

### Rough effort (one engineer, calendar-ish days including tests; risk H/M/L)

| Slice | Size | Risk |
|---|---|---|
| B0 `Component` trait, `ComponentRegistry`, slot allocator, host helper, no component converted | 3-4 d | L |
| B1 Browser pane in IDE bottom panel + `Browser{url,mode,readonly}` params | 5-7 d | M-H |
| B2 Data viewer in chat (after `iyakki-data` lands) | 3-4 d | L-M |
| B3 Editor/Chat/Board/K8s wrapped as components (they already embed) | 4-5 d | L |
| B4 Media item/Terminal/Radio mini player | 5-6 d | M |
| B5 `component_open` assistant tool with consent | 3 d | M |
| B6 Remaining tiles wrapped one by one | ~1 d each, 14 tiles | L |
| C0 `iyakki-api` crate + dependency-rule test (allow-list = today) | 2-3 d | L |
| C1 Data service (`DataService`, local impl, loopback test) | 2-3 d | L |
| C2 Models/Providers + Chat/Assistant runs behind traits (poll/subscribe `RunHandle`) | 8-12 d | M-H |
| C3 Files, Workspaces, Vault, Inbox, Media, K8s services | 15-25 d total | M |
| C4 `iyakki-backend-remote` (HTTP+WS, token auth, reconnect) for Data + Runs | 8-10 d | M-H |
| C5 Multi-user server mode, quotas, observability | 10+ d | H |

---

## A. Audit of what exists today

### A.1 The App contract and registration

- `crates/iyakki-shell/src/apps/mod.rs`: `trait App` (title, icon, accent, hint, help, `handle(&Event, &AppCx) -> Flow`, `render(&mut Gfx, R, scale, &AppCx, tick)`, plus optional `take_request`, `wants_redraw`, `pointer_hint`, `context_items`/`context_pick`). `Flow` is only `Stay | Back`.
- Registration is **hand-wired in three places**: the `iyakki_sec::policy::App` enum (20 ids, also the permission unit: `Policy::allows_app`), the `TILES` array in `shell.rs` (19 tiles; Inbox has none), and the `match` in `Shell::open` (`shell.rs` ~558) that calls each `XApp::new(&cx)`. Group membership lives in `launcher.rs` (`Group::of`). Adding or embedding an app means touching all of them. There is no id-to-factory registry and no parameters: `new(&cx)` is the only constructor, and the only "open this item" channel is the pair of strings returned by `take_request()` (app id, item).
- `AppCx` (the "context" every app gets) holds: `session` (user, role, `Policy`), `host: Arc<dyn HostApi>`, `cfg: AiConfig`, `chat: Arc<dyn ChatBackend>`, `audit`, `store: Arc<Mutex<UserStore>>`, `mcp_servers`, `services: Arc<Services>` and `web: Arc<dyn iyakki_web::Fetch>`. It is the right shape for a component context; `services` is the problem (A.4).
- Input model: one `Event` enum (`ui.rs`): keys, mouse, `Tick` (30 Hz), `Paste`, and **hit-region events** (`Tab(n)`, `Row(n)`, `Focus(id)`, `Area(id)`, `Sub(code, key)`). Apps register clickable rectangles while drawing (`Gfx::hit`); the shell turns a click into the event stored on the topmost hit. So routing is by **integer id, assigned by convention**, not by component identity.
- Concurrency: UI is single-threaded; long work is `std::thread::spawn` + `mpsc` channel polled on `Tick` (chat, edit, rag, media, scan, ide_deploy, browser images, web_agent ...). **The UI is already a poll model**, which makes C much cheaper than it would be.
- Frame production is shared by all front ends: `Shell::render(&mut Gfx)` -> pixels -> window (`iyakki-gui/desktop.rs`, egui texture), framebuffer (`backend.rs`), or browser stream (`iyakki-remote`).

### A.2 Tile inventory (what each owns today)

Sizes are lines of the main file (plus helpers where noted). "Services" are `Services` fields touched in the app's files. "Direct" means other agent modules or crates called straight from UI code.

| Tile (`PApp`) | File(s), lines | State it owns | Drawing / input | Services and direct calls | Embeds / is embedded |
|---|---|---|---|---|---|
| CHAT | `chat.rs` 3485, `chat_view.rs` 1774, `chatpic.rs` 713, + 3 test files | message list, input `TextField`, run session, approvals, picture cache, voice recorder/player, session, model picker, `ProjectScope` (when in IDE) | own transcript drawing with text selection (`textsel`), pictures/SVG | `sessions`, `runs`, `skills`, `settings`, `kb`, `voice_settings`, `browser`, `screen`, `secret_requests`, `session_files`; direct: `agent`, `chat`, `providers`, `voice`, `webengine`, `scope`, `clock`, `assistrun`, `assistant_guard` (63 `iyakki_agent` refs) | Embedded by IDE (`ws.chat`); embeds the editor for proposals (`Event::Sub`) |
| RAG | `rag.rs` 615 | query field, hits list | list + detail | `kb`, `search`, `settings` | none |
| MCP | `mcp.rs` 613 | server list, add form | forms | `mcp` | none |
| CLI | `cli.rs` 844 (+ `clis` in agent) | terminal scrollback (`ScrollLog`), history, catalog of installable CLIs | scrolling log, text field | `clis`, `cluster`, `ollama`, `packages`, `skills`, `settings`; direct: `cli`, `exec`, `textcmd` | none (terminal logic duplicated as IDE bottom tab, `ide_bottom.rs`) |
| USERS | `users.rs` 588 | forms, user list | admin only | `reset_user_vault`; direct `UserStore` | none |
| SETTINGS | `settings.rs` 3748 + `settings_nav.rs` 247 | pages (models, vault, voice, browser, dev cluster, ...), forms, test-connection threads | many forms | 14 `services` fields (`settings`, `secrets`, `kb`, `skills`, `voice_settings`, `system_*`, `user_*vault*`, `lock_system`...); direct: `providers`, `vault`, `regsetup`, `cluster`, `devcluster`, `ollama`, `voice`, `settings`; 52 `std::fs`/`Command` uses | none |
| RUNS | `runs.rs` 403 | list + detail of recorded agent runs | list/detail | `runs`; direct `runs`, `iyakki_ledger` | none |
| EDIT | `edit.rs` 2715 (+ `iyakki-edit` crate: buffers, syntax, diff) | tabs, buffers, cursor/selection, find, proposals review | text area drawing, `AreaAt` clicks | `cluster`, `settings`; direct `iyakki_edit`, `wsexec`, 28 `std::fs` uses | Embedded by IDE (`ws.editor`); stand-alone tile |
| BROWSER | `browser.rs` 6142 + 12 `browser_*.rs`, `web_agent.rs` 624 (total about 12k) | tabs, history, page layout, scroll, forms, find, bookmarks/history/downloads (`Data`), JS sites (`iyakki_js::Sites`), full-engine handle (`Full`), assistant panel, devtools | own layout engine (reader) or pixels from the full engine; address bar; context menu | `browser` (assistant consent/jobs), `page_snapshot`, `kb`, `settings`, `on_login`; direct `iyakki_web` (88 refs shell-wide), `iyakki_js` (29), `iyakki_webview` (5), `webengine`, `browserconsent`, `webtools` | Embeds the assistant side panel (`browser_assist`), the full engine; **not embedded anywhere** |
| RADIO | `radio.rs` 869 | station list/search state | list | `radio` (player lives in `Services` so it outlives the app), `settings`, `voice_settings` | none |
| MEDIA | `media.rs` 3174 | library view, queue, player UI, download UI | grid/list, player | `media_index`, `media_queue`, `inbox`, `set_streamer`; direct `media`, `mediaindex`, `mediaqueue`, `mediaserve` | none |
| SKILLS | `skills.rs` 561 | skill list/editor | forms | `skills`, `packages`, `on_login` | none |
| NET SCAN | `scan.rs` 951 | scan state, results | list | `devwatch`, `settings`, `system_*`; direct `iyakki_netscan` | none |
| MESH | `mesh.rs` 852 | Consul view | list | `quick_upstream`, `secrets`; direct `consul` | none |
| USAGE | `usage.rs` 394 | totals | table | `usage`, `settings` | none |
| SYSTEM | `system.rs` 880 | load, agents, hardware | gauges | `runs`; direct `sysinfo`, `iyakki_hwinfo`; 28 fs/process uses | none |
| CALENDAR | `calendar.rs` 790 | month/day view, forms | grid | `calendar` | none |
| SRE PILOT | `srepilot.rs` 1658 | demo data only | dashboards | none (DEMO ribbon) | none |
| AI IDE | `ide.rs` 2294, `ide_bottom.rs` 816, `ide_k8s.rs` 1094, `ide_deploy.rs` 457, `ide_tree.rs` 506, `ide_ui.rs` 625, `ide_layout.rs` 330, `ide_chrome.rs`, `board.rs` 1990, `ide_tests.rs` 1868 | workspaces registry (`Registry`), per-workspace `Ws {editor: EditApp, chat: ChatApp, board, k8s: Option<K8sView>, bottom tabs, prefs, layout}`, density, modal | 3-pane layout, dividers, tabs; routes events to children | `dir`; direct `workspace`, `k8stools`, `devcluster`, `featureboard`, `boardtools`, `wsexec`, `cluster`, `k8s` | **Embeds** chat, editor (as full `App`s), board, K8s view, deploy view, bottom terminal/log tabs |
| INBOX | `inbox.rs` 1314 | notifications, tasks, approvals, secret prompts | list/detail | 11 `services` fields (`inbox`, `approvals`, `decide_approval`, `answer_secret_request`, `browser`, `calendar`, `runs`...) | Opens other apps through `take_request` |
| (support) | `devices.rs` 652, `widgets.rs` 612, `controls.rs`, `neon.rs` | USB/devices view (used by SETTINGS/SYSTEM), shared widgets | | | |

### A.3 How embedding works today (IDE)

- `IdeApp` holds, per workspace, a **concrete** `EditApp` and `ChatApp` (not trait objects) and calls their methods by name: `editor.open_path`, `editor.goto_line`, `editor.propose`, `editor.selection_info`, `chat.prefill`, `chat.take_proposal`, `chat.set_project`, `chat.set_file_context`, `chat.set_agent_mode`, ... (about 60 call sites in `ide.rs`). The "parameters" are setter methods; the "events" are `take_*` methods polled each tick.
- Event routing: IDE registers children's click regions through `render_child(g, pane, f)`, which **remaps every hit id by `pane.code() * STRIDE` (STRIDE = 1,000,000, pane codes 1..4)**, and `decode(ev)` reverses it to deliver the original event to `editor.handle` or `chat.handle`. Keyboard goes to the focused pane (`w.focus`), keys for a nested child use `Event::Sub(11..14, key)`.
- Chat embeds the editor for reviewing proposals via `Event::Sub` as well (the "editor inside the chat" comment in `ui.rs`).
- Sizing: IDE keeps `scale`, `base_scale`, `chat_scale` separately and tells children. Density (`ide_chrome::Density`, persisted via `uisize::save_ide_density`) is IDE-local.
- Scoping: the chat in a workspace gets a `ProjectScope {name, root, key, features, cluster: Option<ClusterLink>}`. `ClusterLink` carries an `Arc<dyn wsexec::Runner>`. Read-only workspaces (`ws.ro`) block keys to the editor in `handle`.
- Limits: only 4 pane codes; no generic way to embed a *third* thing; children are concrete types so the IDE cannot host a browser without editing `ide.rs`; there is no typed event channel from child to host (the host polls specific getters).

### A.4 Where UI code calls core/agent code directly (the real coupling)

Measured on 67 non-test source files of `iyakki-shell/src` (`*_tests.rs` excluded):

| Coupling | Count |
|---|---|
| `iyakki_agent::...` path references | **449** in **34** files, touching **54** distinct agent modules |
| Direct `services.<field>` accesses (the `Arc<Services>` god object, 40+ fields) | **about 520** (`cx.services` / `env.services`); biggest users: `chat.rs` 75, `settings.rs` 70, `shell.rs` 49, `inbox.rs` 44, `scan.rs` 39, `cli.rs` 38, `radio.rs` 35, `browser.rs` 28, `calendar.rs` 22, `web_agent.rs` 20 |
| Top agent modules referenced | `chat` 76, `clock` 51, `webengine` 35, `iyakki_proxy` 22, `inbox` 22, `agent` 18, `voice` 16, `skills` 16, `providers` 15, `scope` 11, `runs` 11, `wsexec` 10, `settings` 10, `regsetup` 8, `cluster` 8 |
| Other crates called from UI | `iyakki_web` 88, `iyakki_devctl` 67, `iyakki_sec` 57 (policy: fine, it is data), `iyakki_storage` 39, `iyakki_js` 29, `iyakki_edit` 9, `iyakki_webview` 5, `iyakki_netscan` 2 |
| `std::fs` / `Command::new` in UI files | settings 52, chat 38, board 29, system 28, edit 28, browser 28, accounts 26, shell 20, media 16, ide_tree 13, rag 12 |
| Background `thread::spawn`/`mpsc` in UI files | about 17 files (edit 9, ide_deploy 5, chat 5, ...): the UI already polls |
| Trait seams already present | `ChatBackend` (model), `HostApi` (system), `iyakki_web::Fetch` (web), `Kube` (k8s), `Runner` (workspace exec), `Tools` (agent tools), `Upstream`/`Streamer` (network). Good, small, and they are the seeds for C. |

Reading of the numbers:
- The real coupling is **not** "UI imports agent types" (that is only 54 modules, many just data types like `Message`, `ToolCall`); it is that **business logic is executed inside UI handlers**: for example `chat.rs` builds the tool set, runs the agent loop on a thread it spawns, loads sessions, reads vault entries and writes files itself, and `settings.rs` runs connection tests and writes provider config through `services.settings` mutexes.
- `Services` is a service locator holding both state and behaviour; every app can reach every domain. That is the thing to replace by per-domain traits (C.2).
- Places where UI embeds *policy*: `Policy::allows_app` in `Shell::open`, `cx.session.policy.agent` in chat, scope checks through `iyakki_agent::scope` in edit/browser. Authorization must move with the call when the backend is remote (C.7).

---

## B. Embedding design

### B.1 The `Component` trait

New module `crates/iyakki-shell/src/component/` (re-exported from `apps`), deliberately close to `App` so conversion is mechanical.

```rust
/// Typed, serialisable construction/update parameters. Each component defines its own struct; the registry
/// stores them as JSON so they can come from the assistant, from saved layouts and from other components.
pub type Params = serde_json::Value;

pub trait Component {
    // identity and chrome
    fn kind(&self) -> &'static str;            // "browser", "editor", "chat", "dataview", "terminal", "media"
    fn title(&self) -> String;                 // may change (page title, file name)
    fn icon(&self) -> Icon;
    fn accent(&self) -> [u8; 4];
    fn min_size(&self) -> (f32, f32);          // logical px at scale 1.0; host refuses or scrolls below it

    // parameters
    fn params(&self) -> Params;                // current state that should persist (url, path+line, filter ...)
    fn set_params(&mut self, p: &Params, cx: &CompCx) -> Result<(), String>; // navigate / reconfigure live

    // frame loop
    fn render(&mut self, g: &mut Gfx, rect: R, view: &View, cx: &CompCx);   // View = scale, tick, focused, density
    fn handle(&mut self, ev: &Event, cx: &CompCx) -> Outcome;
    fn tick(&mut self, _cx: &CompCx) -> Outcome { Outcome::None }   // poll channels, no blocking
    fn wants_redraw(&mut self) -> bool { false }
    fn pointer_hint(&self, _x: i32, _y: i32) -> Option<Pointer> { None }

    // focus, clipboard, commands
    fn focus(&mut self, on: bool) {}
    fn wants_keys(&self) -> bool { true }                // false while the host should keep shortcuts
    fn commands(&self) -> Vec<Command>;                  // for the host's palette / menu / help (replaces hint/help)
    fn run_command(&mut self, id: &str, args: &Params, cx: &CompCx) -> Outcome;
    fn context_items(&mut self, x: i32, y: i32) -> Vec<crate::ctxmenu::Item> { vec![] }
    fn context_pick(&mut self, id: &str, cx: &CompCx) -> Outcome { Outcome::None }

    // lifecycle
    fn suspend(&mut self) {}                             // hidden/stashed: stop timers, release engine surface
    fn resume(&mut self, _cx: &CompCx) {}
    fn close(&mut self, _cx: &CompCx) {}                 // flush state, stop threads; must be idempotent
}

/// What a component tells its host. The host drains these every tick (typed replacement for take_request/take_proposal ...).
pub enum Outcome { None, Handled, Emit(Vec<HostEvent>) }   // Handled = swallow the key/click

pub enum HostEvent {
    Closed,                                   // component asks to be closed (Back, Esc at root)
    TitleChanged(String),
    Navigated { url: String, title: String }, // browser
    OpenFile { path: PathBuf, line: Option<u32> },       // browser download link, chat "open this", data viewer row
    OpenComponent { kind: String, params: Params, placement: Placement },  // "open this link in a new pane / the full browser"
    Proposal { path: PathBuf, text: String }, // chat -> editor review
    Selection { text: String, source: String },  // "send selection to chat"
    DirtyChanged(bool),                       // unsaved editor: host blocks close
    Notify { text: String, level: Level },
    NeedsFocus,                               // click inside while host thinks another pane has focus
    Custom { name: String, payload: Params }, // escape hatch, namespaced
}
```

Notes:
- `CompCx` replaces `AppCx`: same fields **plus** `scope: Scope` (B.8), `host_id: ComponentId`, and (after part C) a `backend: Arc<dyn Backend>` instead of `services`. In slice B0 `CompCx` simply wraps `AppCx`, so nothing breaks.
- `Outcome` is returned instead of mutated-through getters; the host loops `for e in child.drain_events()`; the old `take_request()`/`take_proposal()` pairs become `OpenComponent`/`Proposal`.
- `View { scale, density, tick, focused, clip }` is passed in, not stored; a child never decides its own scale (B.7).
- `App` stays. Provide `struct TileHost<C: Component>(C)` that implements `App` so the launcher can open any component full-screen unchanged (`Flow::Back` <- `HostEvent::Closed`).

### B.2 ComponentRegistry

```rust
pub struct ComponentSpec {
    pub kind: &'static str,
    pub title: &'static str,
    pub app: Option<PApp>,                       // the permission unit and the tile, if any
    pub schema: ParamSchema,                     // field names, types, required, defaults, enum values, doc
    pub modes: &'static [HostMode],              // Tile | Pane | Floating
    pub make: fn(&Params, &CompCx) -> Result<Box<dyn Component>, String>,
}
pub struct ComponentRegistry { /* BTreeMap<&'static str, ComponentSpec> */ }
impl ComponentRegistry {
    pub fn standard() -> Self;                                  // registers the built-ins (one line each)
    pub fn open(&self, kind: &str, p: &Params, cx: &CompCx) -> Result<Box<dyn Component>, String>;
    pub fn describe(&self) -> Vec<ComponentInfo>;               // feeds the assistant tool (B.10) and docs
}
```

- `ParamSchema` is a small hand-written struct (we prefer our own code over dependencies, see the project memory); it validates JSON (`required`, type check, enum, path-must-be-inside-scope) and produces the JSON-schema the assistant's tool spec needs.
- The registry replaces the `match` in `Shell::open` and the `TILES` table: tiles become `registry.tiles()`, with `Group` and the label/sub/accent living in `ComponentSpec`. The permission check (`Policy::allows_app`) is done **inside `open`**, so an embed gets the same check as a tile.

### B.3 Hosting modes

| Mode | What it is | Host provides | Notes |
|---|---|---|---|
| **Tile** (full) | today's behaviour via `TileHost` | whole screen below the header, Back | the launcher opens `registry.open(kind, default_params)` |
| **Pane** | rectangle inside another component: IDE bottom tab, editor tab, chat inline block | `rect`, focus handling, close button, title for the tab | the first-class mode; both first slices use it |
| **Floating panel** | movable/resizable overlay (picture-in-picture: radio mini player, a browser docked right) | a `FloatingLayer` in the shell that draws after the screen and takes hits first | postponed; the trait needs no change for it |

A host stores `Vec<Slot>`, `struct Slot { id: ComponentId, comp: Box<dyn Component>, rect: R, placement: Placement, focused: bool }`, i.e. the generalisation of the IDE's four hard-coded panes.

### B.4 Event routing and hit ids

- Replace pane-code `STRIDE` remapping by a **slot allocator**: every `Slot` gets an id range `base = slot_index * 1_000_000 (within the host's own range)`. Nesting composes: a host's range is `parent_base + local`. Keep the exact `render_child`/`decode` logic from `ide.rs:1895-1931` and move it to `component::route` (so the IDE's behaviour is unchanged and its tests keep passing).
- Contract: components register hit ids as small integers as now; **they never need to know their base**. The host remaps after render and un-remaps before delivery.
- Keyboard: delivered to the focused slot only, wrapped in `Event::Sub(slot_code, key)` as today. Mouse: the shell hit-test selects the slot by the remapped id; non-hit mouse moves go to the slot under the pointer (`pointer_hint`).
- Child -> host: `Outcome::Emit(HostEvent)`; host decides (open file in editor, navigate its own tab, ignore). Host -> child: `set_params`, `run_command`, `focus`.
- Child -> child (browser link opens file in editor) always goes *via the host* (`OpenFile`, `OpenComponent`), so scope checks and layout stay centralised.
- Guard: debug-assert that no two live hit regions in a frame share an id (cheap test hook already possible: `Gfx::hits`).

### B.5 Focus, clipboard and selection

- One focused slot per host, one focused host per shell. Click anywhere in a slot -> `NeedsFocus` -> host focuses it and blurs the previous (`focus(false)` lets editors hide carets and the browser stop the engine from capturing keys).
- Global shortcuts (F1 help, F9 inbox, Ctrl+Tab between panes, Esc) are checked by the host *before* delegation; a component declares `wants_keys()` false only when it truly wants nothing (e.g. a static image). Text fields inside components keep Esc.
- Clipboard: stays the shell's single clipboard (`ui::clipboard`, `take_host_copy`, history picker). Components call the same functions; they must not talk to the OS clipboard themselves. Context-menu entries `copy/cut/paste/selall` already map to Ctrl+C/X/V/A sent to the focused app and keep working because the shell sends them to the focused slot.
- Text selection (`textsel::Pane`) is per component; starting a selection in one slot clears selections in sibling slots (host calls `clear_selection` through `run_command("selection.clear")`). Cross-component "Send selection to chat": component emits `Selection{text, source}`; the host routes it (IDE -> `chat.prefill`).

### B.6 Persistence of params

- `Component::params()` returns exactly what `set_params` accepts. Hosts save `Vec<(kind, params, placement)>` in their own state (IDE: `ide-state-<user>-<ws>.json` already exists via `state_path`; add `panes: [...]`). The shell can save the open tile's params, so after restart it re-opens with the same url/file.
- Defaults come from the schema; unknown fields are ignored on load (forward compatible), failures to restore fall back to defaults, and a restore never auto-runs anything with side effects (no assistant run, no download).
- Secrets never in params (URLs may contain tokens: strip query secrets before persisting, same rule as browser history today).

### B.7 Density and scale inheritance

- `View.scale` = window scale × host's density factor, computed by the host and passed down; the child multiplies its constants by it exactly like `render(.., s, ..)` today. The IDE's special `chat_scale` becomes `View::with_text_scale(f)` supplied by the host per slot, so a host can choose a larger type for chat than for panes.
- Density levels (`Density` 0/1/2) are a host concept, passed as `view.density`; a component may ignore it. No component stores density.
- `min_size` is in scale-1 px; host multiplies by the view scale. Below min size the host shows a "too small" placeholder with a button to maximise (`OpenComponent{placement: Tile}`).

### B.8 Permissions: scope and jail inherited from the host

- `CompCx.scope: Scope` is derived from the session policy and then **narrowed** by the host, never widened: `Scope { read: Vec<PathBuf>, write: Vec<PathBuf>, hosts: HostPolicy, tools: ToolSet, readonly: bool, project: Option<ProjectScope> }`. Today's inputs are `Policy::read_scopes/write_scopes` and `iyakki_agent::scope::resolve` (symlink-safe), the IDE's `ws.ro` and `ProjectScope.root`.
- `registry.open()` takes `parent: &Scope` and computes `child = parent.intersect(requested)`. A `readonly` parent forces `readonly` child (Editor ignores edits; Browser disables downloads to disk and form POSTs to non-allowed hosts).
- Browser in an IDE workspace: network host policy = the user's policy ∩ workspace allow-list; no access to the user's saved logins unless the user flips the switch (open question 2); downloads land only under the workspace root.
- Every `open` writes an audit entry `component.open kind=... parent=...` (`AuditLog::record`), same as `app.open` now.
- Test per component: given a `Scope` that forbids X, params that ask for X are refused (`set_params` returns Err, nothing opens).

### B.9 Lifecycle

`create` (registry.open) -> `render/handle/tick` while visible -> `suspend` when its tab/pane is hidden or the app is stashed (the shell already stashes an app behind the Inbox: `Shell::stash`) -> `resume` -> `close` on tab close, host close, or logout. Rules:
- `tick` always runs for suspended components that have background work whose result matters (a chat waiting for an answer, a download); drawing does not. This preserves the current "chat is not cut off when the inbox is opened" behaviour.
- `close` must stop threads or detach them with a cancellation flag (the browser already has `Arc<AtomicBool>` for downloads). The host owns a panic boundary: `catch_unwind` per component call (the shell already does this around `app.handle`, `shell.rs` ~1234) so a crashing child shows an error placeholder instead of taking the host down.
- Logout closes everything (existing `Services::on_logout` hook, `web_agent::close`).

### B.10 How the assistant opens or embeds components (`component_open`)

New agent tool (in `iyakki-agent`, registered like `browsertools`/`boardtools`; risk class `Medium`):

```json
{ "name": "component_open",
  "description": "Show a component to the user: a web page, a file, a data table, a media item.",
  "schema": { "kind": "browser|editor|dataview|media|terminal|chat",
              "params": { "...": "validated by the component's ParamSchema" },
              "placement": "auto|pane|tile|floating" } }
```

- The tool does not draw. It enqueues `UiRequest::OpenComponent{kind, params, placement, run_id}` on a queue in the shell (same pattern as `services.browser` jobs and `screen` capture requests, polled on `Tick`).
- **Consent**: first open of each `kind` per session -> an inline approval card in the chat ("Show https://... in a browser pane?" with the params). `Approvals::Auto` pins can skip it for *read-only kinds* (browser/dataview/media), never for `terminal` or `editor` with write access. Consent text is built from validated params, never from model prose.
- The host decides placement: IDE puts `browser` in the bottom panel or an editor tab; chat shows `dataview` inline; plain tiles open full screen. If no host can take it, it opens as a tile with Back to the chat.
- The result returned to the model is a short string ("opened browser pane #3 with https://..."); no pixels, no page text unless the user shares it (existing browser-agent consent rules in `docs/BROWSER-AGENT.md` stay in force).
- Closing a component the assistant opened emits `Closed` back to the run (so the assistant knows the user dismissed it).
- Audit: `component.open` with run id; usage counted like other tools (`toollog`).

### B.11 Parameter examples (JSON accepted by `registry.open` / `set_params`)

```json
{ "kind": "browser",  "params": { "url": "https://docs.rs/serde", "mode": "reader", "readonly": true, "allow_hosts": ["docs.rs"] } }
{ "kind": "editor",   "params": { "path": "src/main.rs", "line": 42, "readonly": false } }
{ "kind": "chat",     "params": { "workspace": "iyakki", "system_prompt": "You review diffs.", "tools": ["read_file","grep"], "agent": true } }
{ "kind": "dataview", "params": { "path": "data/sales.csv", "filter": "region = 'EU' and qty > 10", "columns": ["date","qty"], "limit": 500 } }
{ "kind": "terminal", "params": { "cwd": "~/work/iyakki", "command": null } }
{ "kind": "media",    "params": { "item": "lib:7f3a", "autoplay": false } }
```

Rules: relative paths resolve against the host's root; `workspace`, `cwd`, `path` are checked by `Scope`; `mode: full` for the browser requires the webview helper to be available, else falls back to `reader` with a note in `title`.

### B.12 Migration plan (small slices)

Each slice keeps `cargo test` green and does not change behaviour of existing tiles. File names below are the proposed ones.

**B0. Foundation (3-4 d, risk L). Partly built:** `apps/component.rs` has `Component` (smaller than B.1), `EmbedParams`, `HostEvent` (Navigated, Loading, Closed, OpenInTab) and `route` (slot id remap/decode, `render_slot`, duplicate-id detection, the browser at code 5 next to the IDE's 1-4). Not built: the registry, `TileHost`, `CompCx`, moving `ide.rs`'s own `remap`/`decode` onto `route`. `component/mod.rs` (trait, `Outcome`, `HostEvent`, `Params`, `View`, `CompCx`), `component/registry.rs`, `component/route.rs` (move `render_child`, `decode`, `STRIDE` logic from `ide.rs`, generalised to slot ids), `TileHost`. No existing code converted; `IdeApp` switches to `route.rs` only.
Tests: registry round-trip (schema defaults, validation errors); slot id remap/decode round trip for nested slots (host in host); duplicate-id detection over a frame with two instances of the same fake component; `TileHost` Back/Closed mapping; policy denial through `open`.

**B1. Browser as IDE pane (5-7 d, risk M-H). Built in a reduced form:** `MainTab::Browser` (Editor | Board | Browser | K8s) with `ide_browser.rs` (`IdeBrowser: Component`, reader body via `BrowserApp::new_embedded`/`render_body`, Full mode = the shared assistant's-tab engine, never a second one), params `{url, mode, readonly, start_consent_tier}`, `preview.url` persisted in project.json, Open-app buttons, K8s `o`, assistant tools `ide_browser_open/check`. Not built: the bottom-panel BTab (dropped: one surface only), `.html/.md` menu and chat links, a separate profile per workspace, `readonly` blocking downloads/POSTs (it ignores the person's page input only), `start_consent_tier` enforcement (stored; the account policy and per-site approvals decide), a persisted pane list in `ide-state-*.json`.
- Wrap `BrowserApp` as `BrowserComponent` (thin adapter: `render` -> `BrowserApp::render(g, rect, s, cx, tick)`, `handle` -> `BrowserApp::handle`; `Navigated` emitted by watching `history[pos]` change; `Closed` on `Flow::Back`). `params`/`set_params` = `{url, mode, readonly}`: `go(url)` plus `cfg` toggles; `mode: full` maps to the existing `Full` engine toggle.
- IDE: new `BTab::Browser` in `ide_bottom.rs` (bottom panel) *and* editor-tab placement (`MainTab::Browser`, alongside `Editor/Board/K8s`); menu "Open in browser pane" on `.html`/`.md`, and Chat links `http(s)://` open with `OpenComponent`. Slot persisted in `ide-state-*.json`.
- Needed work in the browser: a headless-friendly `view_rect` (it already records `view_rect` and `ui_scale` for the engine; verify it re-sizes the engine on pane resize), disable the app-level header/hint when embedded, separate profile (`Data` path) per workspace, and a `readonly` flag that disables forms/downloads.
- Risks: focus fights between engine and IDE key shortcuts (engine grabs keys), engine surface lifetime on `suspend`, hit-id collisions with `CONTROL_BASE=1000` (covered by slot remap).
- Tests: render the IDE with a browser slot at three sizes (design-test style in `design_tests.rs`: no overflow, min_size honoured); events: click in browser slot never reaches editor and vice versa; `set_params{url}` navigates and emits `Navigated`; `readonly` blocks a download and a POST; scope: host allow-list refuses another host; close/suspend stops image loader threads (no leaked channels); IDE persists/restores the pane params.

**B2. Data viewer in chat (3-4 d, risk L-M; depends on `iyakki-data`).**
- New `DataViewComponent` (table with column sort/filter, paging) over `DataService` (C.3), params `{path, filter, columns, limit}`. In chat, when the assistant calls `data_query`/`component_open{kind:dataview}`, the transcript renders the component inline via a new transcript item `Item::Embed(slot)` in `chat_view.rs` (fixed max height, "Expand" -> `OpenComponent{placement:Tile}`).
- Needs chat transcript to host live (non-text) items: scroll maths in `chat_view.rs` uses text line heights, so embeds have a fixed measured height in the layout pass (do it as a picture-like block: chat already does this for images in `chatpic.rs`).
- Tests: data fixture render at two widths; filter param error shows message not panic; huge result is paged (never loads all rows); click in table does not toggle chat message selection; copy of a cell uses the shared clipboard; scope: path outside read scope refused.

**B3. Wrap what already embeds (4-5 d, L).** `EditorComponent` (from `EditApp`), `ChatComponent` (from `ChatApp`, params `workspace`, `system_prompt`, `tools`), `BoardComponent`, `K8sViewComponent`; replace the IDE's concrete fields by `Slot`s and its named method calls by `HostEvent` handling (`Proposal`, `OpenFile`, `Selection`). The existing 1,868-line `ide_tests.rs` is the regression suite; do this behind unchanged public IDE behaviour and land in two commits (editor/chat first, then board/k8s).

**B4. Terminal, Media item, Radio mini (5-6 d, M).** `TerminalComponent{cwd}` from the CLI's `ScrollLog` + exec path and the IDE's bottom terminal (removes the duplicated terminal logic); `MediaComponent{item}` (player strip from `media.rs`), `RadioMini`. Tests: cwd jailed to scope; player keeps state across suspend/resume; radio keeps playing when the pane closes (player lives in `Services`).

**B5. `component_open` tool (3 d, M).** Tool in `iyakki-agent` (`componenttools.rs`), queue in `Services` (or its replacement), consent card in `chat.rs`/`inbox.rs`, audit. Tests: schema validation errors reach the model as tool results; consent denied = nothing opened; auto-approval only for read-only kinds; scope violation refused; prompt-injection test (page text cannot trigger `component_open` for a different host).

**B6. Convert the remaining tiles, one per change (~1 d each).** Order by "likely to be embedded" and by smallness: RUNS, USAGE, CALENDAR, RAG, SKILLS, MCP, RADIO, INBOX (list), SYSTEM, NET SCAN, MESH, USERS, SETTINGS (last: 3.7k lines, never embedded), SRE PILOT (demo). Each conversion is `impl Component` plus one registry line plus removing its row from `TILES` and its arm from `Shell::open`.

Stop criteria: after B3 the IDE needs no knowledge of concrete child types; after B6 `Shell::open` and `TILES` are generated from the registry and `PApp` stays only as the permission id.

---

## C. Backend abstraction (service layer)

### C.1 Goals and non-goals

- Goal: the UI crate (`iyakki-shell`) depends only on `iyakki-api` (traits + serde types). An implementation is chosen at start-up: `LocalBackend` (today's behaviour, in process) or `RemoteBackend` (same traits over a transport).
- Non-goals now: choosing a wire format for good; moving rendering; making the single-user bare-metal/window mode any slower or more complex.
- The existing seams are kept and become the building blocks: `ChatBackend`, `Kube`, `Runner`, `Fetch`, `HostApi`, `Tools`.

### C.2 Crate layout

```
iyakki-api              traits + request/response/event types (serde), error type, ids, Scope/Identity, no IO, no threads
iyakki-backend-local    implements the traits over today's iyakki-agent / iyakki-storage / iyakki-data / iyakki-web ...
iyakki-backend-remote   implements the traits by calling a server (transport, auth, reconnect); depends only on iyakki-api
iyakki-server           (later) hosts a LocalBackend behind HTTP/WS; per-user auth, tenancy, limits; depends on backend-local
iyakki-agent            unchanged at first; becomes an implementation detail of backend-local
iyakki-shell            depends on iyakki-api (+ iyakki-ui, gfx deps); NOT on iyakki-agent (end state)
iyakki-gui / -core      wire: build a Backend (local or remote) and pass it to Shell
```

Transitional rule: `iyakki-shell` keeps its `iyakki-agent` dependency while the allow-list (C.10) is non-empty. `iyakki-api` re-exports shared data types (`Message`, `ToolCall`, `ToolSpec`) by moving them out of `iyakki-agent::chat`, with `pub use` left behind so nothing else breaks.

### C.3 Traits per domain (sketch)

All methods take `&Identity` implicitly via the `Backend` handle (the handle is created per signed-in user) and return `Result<T, ApiError>`. All request/response/event types derive `Serialize, Deserialize, Clone, Debug`. Long operations return a handle (C.4) instead of blocking.

```rust
pub trait Backend: Send + Sync {            // one per session; cheap to clone (Arc)
    fn files(&self) -> &dyn Files;           fn runs(&self) -> &dyn Runs;
    fn models(&self) -> &dyn Models;         fn vault(&self) -> &dyn Vault;
    fn media(&self) -> &dyn Media;           fn deploy(&self) -> &dyn Deploy;
    fn workspaces(&self) -> &dyn Workspaces; fn data(&self) -> &dyn DataService;
    fn inbox(&self) -> &dyn Inbox;           fn web(&self) -> &dyn WebService;
    fn audit(&self) -> &dyn Audit;           fn events(&self) -> EventStream; // global push (inbox, alarms, jobs)
    fn health(&self) -> Health;              // Online | Degraded(reason) | Offline, for the status chip
}
```

| Trait | Replaces today's | Key methods (all serde in/out) |
|---|---|---|
| `Files` | `std::fs` in edit/ide_tree/chat + `scope::resolve` | `list(path)`, `read(path, range)`, `write(path, bytes, expect_version)`, `stat`, `watch(path) -> Subscription`; scope enforced server-side. Local always for window mode paths (C.5) |
| `Runs` (Chat/Assistant) | `ChatApp` agent loop + `assistrun` + `RunStore` | `start(RunRequest{session, workspace, prompt, mode, tools, model}) -> RunHandle`; `RunHandle::poll() -> Vec<RunEvent>`; `cancel`; `decide(call_id, Verdict)`; `list/load(run_id)`; `sessions_list/load/delete` |
| `Models` | `providers`, `ollama`, `models`, `usage` | `list_providers`, `list_models`, `test_connection`, `pull_model -> Progress`, `usage_summary`; `complete` for one-shot (summaries) |
| `Vault` | `vault`, `credguard`, `secretreq`, `services.secrets/user_*vault*` | `names()`, `put(name, secret)` (never returns the secret to remote UI except by explicit `reveal` with step-up), `request_secret(prompt) -> id`, `answer_secret_request`. Stays local by default (C.5) |
| `Media` | `media`, `mediaindex`, `mediaqueue`, `mediadl`, `radio*` | `library(query)`, `index_status`, `queue_add/list/cancel`, `stream_url(item) -> ExpiringUrl`, `radio_search`, `radio_play/stop` (player is local audio output: command goes to the machine with speakers) |
| `Deploy` (K8s) | `k8s`, `k8stools`, `devcluster`, `cluster`, `deploylint` | `profiles`, `get(kind, ns)`, `logs(pod) -> Stream`, `plan_apply -> Diff`, `apply(approval)`, `rollout_status`; wraps `Kube`/`Runner` |
| `Workspaces` | `workspace` registry, `featureboard` | `registry_list/open/create`, `meta`, `board_get/put`, `project_json`, `locks` |
| `DataService` | new `iyakki-data` | `open(DataSource) -> DatasetId`, `schema`, `query(DatasetId, Query{filter, sort, columns, offset, limit}) -> Page`, `aggregate`, `export`; being written now with serde request/response types, **adopt its types verbatim as `iyakki-api::data`** |
| `Inbox` | `inbox`, `services.approvals`, `calendar` alarms | `list`, `mark`, `act`, `pending_approvals`, `subscribe` |
| `WebService` | `webtools`, `webresearch`, `iyakki_web::Fetch` for assistant tools | `search`, `fetch_text`, `weather`, `quote`; the *browser's own page loading* is not here (local, C.5) |
| `Audit` | `AuditLog` | `record(action, detail)`, `query(filter)`; server-side append-only |

Plus `Identity { user, role, policy_digest, token }` and `ApiError { Denied, NotFound, Invalid, Busy, Unavailable, Timeout, Internal(id) }` so the UI can show consistent messages and retry only where sensible.

### C.4 Sync/async handling for a single-threaded UI

- Rule: **no blocking call from `render` or `handle`.** The UI draws on one thread at 30 Hz and the 17 files that already use `spawn + mpsc` follow this.
- Three call shapes, so a remote implementation is possible without async runtimes leaking into the UI:
  1. **Quick local reads** (`health()`, cached `names()`): immediate `T`. Allowed only on an allow-listed set; remote impl serves from a client cache refreshed in the background.
  2. **Request/response**: `Pending<T>` (`fn poll(&mut self) -> Poll<Result<T, ApiError>>`, `cancel()`), created by the call, polled from `tick` (exactly the existing `Receiver` pattern, wrapped). `LocalBackend` can complete it before returning or on a worker thread.
  3. **Streams**: `Subscription<E>` with `poll() -> Vec<E>` and a monotonically increasing `seq` so a reconnect resumes (`since_seq`). Used for `RunEvent` (text delta, thinking delta, tool call, approval needed, tool result, usage, finished), `Progress`, `logs`, `Inbox`.
- `RunEvent` is the one load-bearing type: define it from what `chat.rs` consumes today (`partial`, `think_rx`, `RunSession`, approvals, usage tuple, timing) so `ChatApp` becomes a view over a `RunHandle` and loses its own thread spawns.
- `iyakki-api` contains no executor; `backend-remote` owns its own thread(s) (blocking sockets are enough: we already ship our own HTTP/WS code in `iyakki-remote/http.rs` and `ws.rs`, so no new dependency) and exposes the same `Pending`/`Subscription` objects.
- Back-pressure and size: poll returns at most N events per tick; large payloads (file contents, images) are references plus a separate chunked `read`.

### C.5 What stays local, what can go remote

| Always local | Reason |
|---|---|
| Rendering (`Gfx`, fonts, layout, `iyakki-js`, the reader's HTML layout), input, hit-testing, selection | needs the display; latency |
| Clipboard, window, audio output, screen capture | the user's device |
| Local files in window mode (the user's project folder), editor buffers and unsaved state | the files are on this machine; `Files` impl is local unless the workspace itself is remote |
| Helper processes: the web engine helper (`iyakki-webview`, `dist/webview/host-helper.py`), USB/hardware, network scan (`iyakki_netscan`: scans *this* LAN), podman/kubectl against the *local* machine | tied to the device |
| Vault secrets (by default) | trust boundary; see open question 3 |

| Can go remote | Notes |
|---|---|
| Agent runs and tool execution (`Runs`) | biggest win: long-running, CPU/network-heavy, survives the UI closing. Tools that touch local files need a "tool bridge" (the remote run asks the local client to execute a file tool, with the same approval) or a synced/remote workspace. Decide per tool: `location: local|server` in `ToolDef` |
| Models and providers (`Models`) | keys on the server, one rate-limit domain |
| K8s/Deploy | already has a "cloud" mode (docs/K3S-DEV.md) |
| Media indexing/transcoding, downloads | CPU/disk heavy; stream back via expiring URLs |
| Data processing (`DataService`) | large files stay near the compute |
| Storage (`Files` for remote workspaces, sessions, run logs, knowledge base) | needs versioned writes (`expect_version`) |
| RAG/KB embeddings | same |

### C.6 Session, identity, auth

- Local: the existing login (`UserStore`, argon2) yields a `Session` (user, role, `Policy`). `LocalBackend::for_session(session)` captures it; nothing else changes.
- Remote: `RemoteBackend::connect(url, credentials)` -> server verifies against its own user store, issues a **short-lived access token + refresh token per user** (same idea as `iyakki-remote/session.rs`: random 32-byte tokens, idle limit, CSRF for browsers, sign-in rate limit per address). Every request carries the token; the server derives `Identity` and `Policy` from the token, never from request fields. Tokens for devices (desktop app) are separate from browser cookies and individually revocable.
- Authorization lives **server-side**: `allows_app`, path scopes, host policy, tool risk/approval are re-checked by the server. The client's copy of the policy is a UX hint only.
- Step-up for sensitive calls (`Vault.reveal`, `Deploy.apply`, `Files.write` outside the workspace): a second approval in the UI bound to the call id (the existing approval flow, now carried by `RunEvent::ApprovalNeeded` / `Runs::decide`).
- Transport security: TLS (`iyakki-remote/tls.rs` exists), no secrets in URLs, per-request id for idempotent retries.

### C.7 Failure behaviour

- `Health` surface: `Online`, `Degraded("models unreachable")`, `Offline`. A status chip in the header and per-feature disabled states.
- Offline/reconnect: `Pending` times out with `ApiError::Unavailable` (UI shows a retry button, not a panic). `Subscription` reconnects with `since_seq`; the server replays from its log; the UI shows "reconnecting..." on the run, never loses the already received transcript. Runs continue server-side while the UI is gone (and show in RUNS/INBOX on return).
- Partial failure: domains fail independently (models down, files fine). Editing local files and the reader browser work with no backend at all.
- Writes: idempotency key per mutating request; `expect_version` on file/board writes to detect lost updates; the UI keeps unsent drafts (input fields, unsaved buffers) locally.
- Timeouts per call class (read 5 s, mutate 15 s, run start 30 s; streams have heartbeats every 15 s) with the existing `assistant_guard`/`assistrun` timeouts staying inside the server.
- Version skew: handshake `{api_version, features[]}`; client greys out features the server lacks.

### C.8 Multi-tenant scaling notes

- `LocalBackend` is the single-tenant server. `iyakki-server` is a process hosting many `LocalBackend` instances keyed by user: per-user directories (`services.dir()/users/<user>` like `ide-state-<user>-*` today), per-user vault, per-user run queue.
- Stateless front doors: sessions and run logs are stored (files first, DB later) so any server instance can resume a `RunHandle`; sticky routing only for active streams.
- Isolation: agent tool execution already happens under scope checks, jailed paths and (for code) the sandbox cluster; for hosted multi-tenancy each user's tools run in a per-user sandbox (`cluster.rs` k3s slot / `wsexec::Runner`), never in the server process.
- Limits: per-user concurrent runs, token budgets (the `usage` log is the base), request rate, upload sizes; return `ApiError::Busy` with retry-after, which the UI already can show.
- Capacity levers in order of cost: more model provider keys, a worker pool for `Runs`, separate media worker, shared object storage for files.

### C.9 Relation to `iyakki-remote` and cloud mode

- `iyakki-remote` today = **UI remoting**: server holds a `Shell` per signed-in browser session (`session.rs`, `ShellGate`, `ShellEnv::share()`), draws it and streams pixels over WebSocket; keys/mouse come back. "Every tile works at once because the browser only ever sees pixels."
- The new `RemoteBackend` = **logic remoting**: the UI runs where the user is (desktop, later a phone/web client) and only data and events travel.
- They compose: cloud mode (`dist/k3s-web.yaml`, docs/K3S-DEV.md "Cloud" row) = pixel streaming + `LocalBackend` inside the pod. A future thin client = local UI + `RemoteBackend` to that pod. A big multi-user deployment = pixel-streaming front doors (cheap, works on any browser) calling an `iyakki-server` backend pool through `RemoteBackend`, instead of each streamed `Shell` owning full local services. Same shell code in all three; only what is injected into `ShellEnv` differs.
- Pixel streaming's cost is server CPU per user (frame encode) and latency; the logic split reduces the server to API work for clients that can render themselves. Neither obsoletes the other; keep both.
- K8s/Deploy remote clusters (tunnel/cloud profiles in K3S-DEV.md) already look like a remote service from the app's point of view; `Deploy` trait wraps that without changing it.

### C.10 Dependency-rule test

A test in `crates/iyakki-core/tests/` (next to `feature_matrix.rs`) or `crates/iyakki-shell/tests/deps.rs`:

1. Walk `crates/iyakki-shell/src/**/*.rs` (skip `*_tests.rs`, `tests/`, `#[cfg(test)]` blocks are fine to include: tests may use mocks).
2. Collect every `use iyakki_agent::...` / `iyakki_agent::path` (and later `iyakki_web`, `iyakki_storage`, `iyakki_devctl` except an explicit allowed set) occurrence per file, normalised to `file -> set of module names`.
3. Compare with a checked-in allow-list `crates/iyakki-shell/deps-allow.txt` (`apps/chat.rs: chat, agent, providers, ...`). **Fail if** a file uses a module not in its entry, or a new file uses any. Also **fail if an allow-list entry is stale** (listed but no longer used), which forces shrinking: deleting lines is the only way to make progress and the test enforces that nobody forgets.
4. Initial allow-list is generated from today's 449 references (34 files, 54 modules); each migration slice deletes its lines in the same commit.
5. Second rule: `services.<field>` accesses (about 520) counted per file with the same mechanism, and a rule that **no new file** may touch `services`.
6. End state: allow-list empty, `iyakki-agent` removed from `iyakki-shell/Cargo.toml`, and a final `cargo metadata`-based check that fails if it comes back.

Cheap to implement with `std::fs` and string scanning (no new dependency), matching how `feature_matrix.rs` validates `FEATURES.md`.

### C.11 Staged plan

**C0. Skeleton and guard (2-3 d, L).** Create `iyakki-api` (empty traits, `ApiError`, `Identity`, `Pending`, `Subscription`), `iyakki-backend-local` with `LocalBackend::new(Arc<Services>, Session)`, add the dependency-rule test with the generated allow-list. `AppCx` gains `backend: Arc<dyn Backend>` next to `services` (nothing uses it yet). Test: loopback harness `LoopbackBackend` that wraps any `Backend` and round-trips every request/response/event through `serde_json` (used by all later slices' tests).

**C1. Data service first (2-3 d, L).** Land `iyakki-data`'s serde `DataService` trait and request/response types under `iyakki-api::data`; `LocalBackend` implements it by calling `iyakki-data`. The Data viewer component (B2) is its first and only consumer, so the very first UI code is born behind the boundary and never gets a `services` shortcut. Tests: query round-trip through loopback; paging; scope denial; cancellation of a long aggregate; error mapping.

**C2. Models/Providers then Chat runs (8-12 d, M-H).**
- `Models`: wrap `providers`, `ollama`, `models`, `usage` for SETTINGS and the model picker (small, mostly request/response). Removes ~25 of SETTINGS' agent references.
- `Runs`: the big one. Define `RunRequest`/`RunEvent` from `chat.rs`; implement `LocalBackend::runs().start()` by moving the thread/loop code out of `ChatApp` (`assistrun`, `assistant_guard`, `agent::run`) behind `RunHandle::poll()`. `ChatApp` loses `thread::spawn`, `Receiver`s and direct `agent`/`providers` references; it keeps rendering and input. Approvals travel as events. Tool location (`local|server`) added to `ToolDef` with everything `local` initially.
- Migrate the 1116-line `assistrun_tests.rs` and the chat tests to run against the loopback backend; keep a local-only fast path test.
- Risk: streaming UX regressions (partial text, thinking, timing numbers) and tool bridge semantics; mitigate by recording a golden event sequence from the current implementation and replaying it in tests.

**C3. Remaining domains, in order of benefit/size (15-25 d, M).** `Inbox` (+approvals, calendar alarms), `Workspaces` + board, `Files` (local impl is `std::fs` + `scope`; most of the 28/38/52 raw `fs` uses in edit/chat/settings move here), `Vault`, `Deploy`, `Media`, `WebService`. Each slice: define trait from the existing call sites (grep the allow-list entries), implement local, switch call sites, delete allow-list lines, loopback test.

**C4. Remote implementation (8-10 d, M-H).** `iyakki-server` (HTTP/1.1 + WebSocket reusing `iyakki-remote/http.rs`/`ws.rs`; JSON bodies; `POST /api/<domain>/<method>`, `GET /api/events?since=<seq>` upgraded to WebSocket) and `iyakki-backend-remote` (blocking client on worker threads, `Pending`/`Subscription` adapters, reconnect with `since_seq`, token refresh). Start with `Data` + `Runs` + `Models` only; everything else stays local through a **composite backend** (`Backend` whose `data()/runs()/models()` point to remote and the rest to local), which is also how window mode with "cloud models" would work. A gRPC-style schema can be generated later from the same serde types if a second language needs the API; do not decide now.

**C5. Multi-user hosting (10+ d, H, postponed).** Tenancy, quotas, observability below, hardening, storage backends.

### C.12 Observability

- Every call carries `request_id`, `session_id`, `user`, `run_id` (when relevant); `ApiError::Internal(id)` shows the id to the user and logs details server-side only.
- Structured log line per call (domain, method, duration, outcome, bytes) and counters/histograms per domain (kept in-process first, a `/metrics` text endpoint later). The existing `toollog` (tool calls, kept for days) and `usage` (tokens per model) stay and move behind `Runs` and `Models`.
- Audit trail: `Audit` records both client-initiated and server-side actions with the same format as `AuditLog` now; tamper-evident chain for runs already exists (`runs::ledger_for`, `iyakki_ledger`), keep it server-side.
- A "Backend" page in SYSTEM: health, latency per domain, reconnect count, API version.

---

## Appendix: file and symbol index used above

- Trait/registration: `crates/iyakki-shell/src/apps/mod.rs` (`App`, `AppCx`, `Flow`), `crates/iyakki-shell/src/shell.rs` (`ShellEnv` l.31, `Screen`, `TILES`, `Shell::open` ~l.540-580, `stash`), `crates/iyakki-shell/src/launcher.rs` (`Group`), `crates/iyakki-sec/src/policy.rs` (`App` enum, `allows_app`).
- Events and hit regions: `crates/iyakki-shell/src/ui.rs` (`Event`), `crates/iyakki-shell/src/gfx.rs` (`Gfx::hit`, `clipped`).
- Embedding today: `crates/iyakki-shell/src/apps/ide.rs` (`Ws`, `IdeApp`, `STRIDE`, `render_child`, `decode`, `Pane`), `apps/chat.rs` (`ProjectScope`, `ClusterLink`), `apps/ide_bottom.rs`, `apps/ide_k8s.rs`, `apps/board.rs`.
- Services: `crates/iyakki-agent/src/services.rs` (`Services`), `chat.rs` (`ChatBackend`), `agent.rs` (`Tools`, `ToolDef`), `runs.rs` (`RunStore`), `k8s.rs` (`Kube`), `wsexec.rs` (`Runner`), `scope.rs`.
- Remote: `crates/iyakki-remote/src/{server,session,screen,http,ws,tls}.rs` (`Gate`, `Screen`, `ShellGate`), `crates/iyakki-gui/src/desktop.rs` (event mapping to `Shell::handle`), `docs/K3S-DEV.md` (modes table).
- Counts in A.4 were taken with `grep` over `crates/iyakki-shell/src` excluding `*tests*.rs`; regenerate them with the dependency-rule test's scanner once it exists, and treat the test's allow-list as the authoritative number.

## Built so far (2026-10-14): the browser in the AI IDE

* `crates/iyakki-shell/src/apps/component.rs`, `ide_browser.rs`, `ide_browser_tests.rs`; additive hooks in `browser.rs` (`embedded`, `render_body`, a page that needs the engine is handed to the host), `web_agent.rs` (`render_with`, `page_view`), `ide.rs`, `ide_ui.rs`, `ide_k8s.rs`.
* `crates/iyakki-agent/src/idebrowser.rs`: the two tools as a wrapper (`IdeBrowserTools`) around the workspace tools, own-host list, hand-off queue to the IDE.
* Plain HTTP is allowed for the workspace's own cluster and localhost because NodePorts and dev servers have no certificate; everything else keeps the usual rules.
