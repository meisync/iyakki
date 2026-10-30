# IYAKKI developer SDK API (specification)

**Status:** DESIGN for the owner to approve. Nothing in sections 4 to 12 is built unless its status says `exists`. Written 2026-10-24 against the working tree.
**For:** people and agents (Claude Code, Copilot) that write tile apps, k3s services and MCP/assistant tools and need OS features.
**You will get:** what the OS offers today (inventory, section 2), the API of every area in three forms (JS bridge, local HTTP, MCP), the embed design
(section 5), usage tracking and budgets (section 6), the remote debug loop (section 7), test fakes, phases, security notes, versioning and open decisions.
Related: `docs/development/CLAUDE-CODE-APP-DEVELOPMENT.md` (kinds of app, section 6 = proposed `service` package kind), `docs/APP-AUTHORING.md` (today's bridge),
`docs/DEVELOPER-MODE.md`, `docs/EMBEDDING-AND-BACKEND.md`.

Status words: **exists** = reachable by the person or the assistant today, but not by apps; **partly**; **gap** = nothing yet. Effort: S under 3 days, M about a week, L more.

## 1. Principles

1. One namespace per area under `window.iyakki`, promise-based, frozen, arguments checked in the page and again on the host (as `crates/iyakki-js/src/modules/bridge.rs` does now).
2. Everything is **off unless declared** in `app.json` `capabilities`, shown to the person in plain words at install and in the Info panel (as now).
3. A call that spends money, uses the microphone or camera, reads the screen, or changes a cluster asks the person **every time** (the `agent.ask` pattern: at most 4 waiting, `MAX_PENDING`).
4. Keys, tokens, hosts of providers and the vault are **never visible** to an app. The app names a purpose (`llm.chat`), the host picks provider and model from the person's settings.
5. The same surface in three forms with the same names, limits and errors: JS bridge (tiles), local HTTP (k3s services, PC tools), MCP tools (assistants).
6. Every namespace has a **test fake** so an app is testable without a model, a microphone or a cluster.
7. Build in-house: no new dependency is needed by anything here (the HTTP form reuses `httpmini.rs`; the MCP form reuses `extagent/bridge.rs` and `mcp.rs` patterns).

## 2. Inventory: what exists today

| Area | Internal module (verified) | Permission model today | Gaps for apps |
|---|---|---|---|
| App bridge | `crates/iyakki-js/src/modules/bridge.rs` (`Bridge`, `BridgeModule`, prelude), `crates/iyakki-agent/src/store/tile.rs` (`Caps`, `TileHost::call`, `Outbox`, `TileStorage`, `TileFetch`, `run_tests`), `crates/iyakki-shell/src/apps/tile.rs` (`dialog_for`) | `Caps`: storage, network[], clipboard, notifications, files, agent; 200 calls/s (`RATE_PER_SEC`), 4 pending (`MAX_PENDING`) | only 8 namespaces; no versioning call; no events; no usage numbers |
| Sound out | `audioout.rs` (`Selector`, `scan`, `route_args`, `scale`, `play_wav_on`), `softvol.rs` (`Plan`, `Softvol`), `syssounds.rs` (4 UI cues), `radio.rs` (`RadioPlayer`) | none for apps | T2 speaker is plughw + softvol, **one program at a time, no mixer** (dmix burns a core; memory note project-own-mixer-t2-audio). No app play call |
| Video/media | `player.rs` (`Player`, `probe`, ffmpeg decoder), `media.rs` (`Library`, `MediaPlayer`, `shared()`), `mediaqueue.rs`, `apps/media.rs`, `apps/mediaplay.rs`; `docs/MEDIA-PLAYER.md` | none for apps; needs ffmpeg installed from vendor (never shipped) | frames go to the shell's own view, not into a page rectangle |
| Markdown | `crates/iyakki-shell/src/mdtext.rs` (`layout`, `layout_with`, `draw`, `link_rects`), used by `apps/chat_view.rs`. EDIT opens `.md` as source (`Lang::Markdown`) | n/a | **no stand-alone .md viewer exists**; the layout engine is chat-only. Needs a viewer component (scroll, links, images, search) |
| Browser | `crates/iyakki-webview` (helper process, `docs/WEBVIEW-PROTOCOL.md`), `apps/browser.rs`, `apps/browser_full.rs` (page fullscreen: `fullscreen`, `leave_fullscreen`), `apps/browser_consent.rs`, `browsertools.rs`, `devtools.rs` (chat tools `browser_failed_requests`, `browser_timings`, `browser_console`; off unless the person turns them on in Settings -> Browser), `idebrowser.rs` (own-host list, `IdeBrowserTools`) | per-site look consent, per-domain click/type approval, policy `allows_host` | tile apps cannot open or embed it; no allow-list per embed |
| Embedding | `apps/component.rs` (`Component`, `EmbedParams`, `HostEvent`, `route::*`), `apps/ide_browser.rs` | the host decides; `docs/EMBEDDING-AND-BACKEND.md` B.8 | only the IDE embeds; no registry, no `TileHost`, ids use fixed `STRIDE` codes |
| TTS/STT | `voice.rs` (`SpeechEngine`, `synthesize_with`, `transcribe`, `Recorder`, `SpeakOpts`), `voicedesk.rs` (`Reader`, `consent_text`, `destination`, `read_voices`, `stt_engine`, `list_mics`, `t2_capture_allowed`), `hostvoice.rs` (browser voices), `lemonade.rs` (`tts_engine`, `stt_engine`), `docs/VOICE.md` | person is told where text or sound goes (`consent_text`); mic only after a click/key, max 300 s (`MAX_RECORD_SECS`), 20000 chars (`MAX_TEXT_CHARS`) | no app call. **T2 microphone capture is off by default** (apple-bce patch `dist/buildroot/package/apple-bce/0003-aaudio-real-periods-capture-off-by-default.patch`, parameter `capture`; capturing while playing hung one machine, `docs/MAC-HARDWARE.md`) |
| k3s | `k8s.rs` (`Kube`, `Kubectl`, `Snapshot`, `Feed`), `k8stools.rs` (18 tools, `TOOLS`), `k3sdev.rs`, `clusters.rs`, `platform/` (`ServiceDef`, wizard), `deploylint.rs` | read tools are `Risk::Safe`; apply/delete/restart/stop/teardown are `Risk::Ask`; teardown always asks and needs the typed name; only the workspace namespace; lint refuses privileged/hostPath/etc. | no app/service access; no per-app namespace; no service package kind |
| Dashboards | `dash/` (`DataSource`, `QueryRequest`, `Frame`, `engine::run_panel`, `readonly`, `tools.rs`: `dash_sources`, `dash_describe`, `dash_query`, `dash_explain`, `dash_panel_*`, `dash_save`) | read-only by rule; saving asks | no app call; source credentials stay in the vault |
| Workflows | `flows/` (`host.rs` `AgentHost`, `tools.rs` `flow_*`), engine in `crates/iyakki-flow` (`Engine::start_run`, `runs_of`), `docs/FLOWS.md` | `flow_list/get/runs/logs` safe; create/edit/run/cancel ask; publish, rollback, `flow_delete` and `flow_drop_version` always ask; `ask_form` (FLOWS chat only, safe) shows a form for missing input | no app call; no "run flow X with inputs" for apps |
| Notifications | `inbox.rs` (`Inbox::add`, `Kind`, `Priority`, `AGENT_PER_HOUR` = 20), tile `notify.show` (5 per run) | capability `notifications` | no tasks/reminders for apps; no click-through link; no read of own notes |
| LLM/models | `chat.rs` (`ChatBackend`, `ModelTurn`, `ToolSpec`), `providers.rs` (`Kind`: Ollama, OpenAI, OpenRouter, Anthropic, Gemini, Compat, Lemonade; `ProviderClient`), `lemonade.rs` (`Model`, `ModelType`), `ollama.rs`, `modelhost.rs`, `models.rs`, `agent.rs` (`start`, `RunLimits`, `RunEvent`, `Risk`), `vault.rs` (`Secrets`, key_ref), `cloudmode.rs`, shell `apps/model_picker.rs` | assistant: per-call approvals, `hard_ask`, budgets from policy `AgentBudget` | apps only have `agent.ask(question)` (one string, one answer, asks every time). No plain `llm.chat`, no model list, no embeddings call |
| Assistant | `agent.rs`, `assistrun.rs`, `extagent/` (Claude Code, Copilot), `extagent/bridge.rs` (policy bridge over a local MCP socket, `iyakki --mcp-tools <socket>`, run token) | tools offered through `Gate`; same decisions as the built-in loop | the bridge serves one chat run, not an installed app |
| Local HTTP | `httpmini.rs` (one request per connection, 16 KiB head, 64 KiB body), `devmode/api.rs` (`/v1/dev/*`, paired-key auth) | developer-mode keys and scopes | **no per-app token API** for services |
| Developer mode | `devmode/` (`scopes.rs` 11 scopes, `app.rs`, `input.rs`, `files.rs`, `config.rs`, `cli.rs`, `cli_adb.rs`, `api.rs`, `debug.rs`), `docs/DEVELOPER-MODE.md` | scopes per paired computer; activity log + audit | see section 7 |

Things that surprised me while reading: the page engine's `getBoundingClientRect` returns zeros (`crates/iyakki-js/src/modules/dom.js`), so an app cannot learn where its own element is;
`tile.rs` lays out one centred column (900 px max) and gives hit regions to controls only; there is no markdown file viewer anywhere.

## 3. Conventions shared by all namespaces

* **Call shape:** `iyakki.<ns>.<fn>(args)` returns a Promise. Arguments are plain JSON; every string has a stated maximum.
* **Handles and events:** long-lived things return a handle `{id}` plus `on(event, fn)` / `off`. Events are delivered on the page's next turn; at most 50 queued per handle, oldest dropped with a `Dropped` event.
* **Feature test:** `iyakki.version` (e.g. `"1.0"`), `iyakki.has('audio.play')`. Missing functions are absent, never stubs that fail late.
* **Approval kinds:** `none`, `first-use` (once per app install, revocable in Info), `each-call` (card every time, 4 waiting at most), `each-call+typed` (the person types a name, as `k8s_teardown`).
* **Errors:** `Error` objects with `.code` (a string from the table below) and `.message` (plain words). Codes are stable across versions.

| Code | Meaning |
|---|---|
| `NoCapability` | app.json does not declare it (text: "this app did not ask for X", as today) |
| `Denied` | the person said no, or did not answer in 120 s |
| `BadArgument` | type, size or shape wrong (names the field) |
| `QuotaExceeded` | storage, rate, size, count or per-app budget reached; `.limit` names it |
| `BudgetStop` | daily token/cost cap hit (section 6); `.resets_at` |
| `Unavailable` | hardware, engine, model or cluster missing (cloud mode, no mic, no ffmpeg); `.fix` has the one-line way out |
| `Busy` | exclusive resource in use (speaker, microphone); `.holder` is a label |
| `Timeout` | host time limit reached |
| `Blocked` | allow-list or policy refused (network host, path, namespace) |
| `Failed` | the engine or server failed; `.detail` is redacted |
| `Unsupported` | needs a newer SDK or image than present |

## 4. Namespaces

Each block: signature, capability (app.json key), approval, limits, events, errors, internal module, status, effort. JS is shown; section 9 maps the same call to HTTP and MCP.

### 4.1 `notify`  (status: partly; effort S)

```ts
notify.show(title: string, body?: string, opts?: {kind?: 'info'|'warning'|'reminder'|'download', link?: {label: string}}): Promise<{id: string}>
notify.task(title: string, opts?: {notes?: string, due?: number /*unix s*/, priority?: 'low'|'normal'|'high'}): Promise<{id: string}>
notify.on('click', fn)   // the person opened the note: {id}
```
Capability `notifications`. Approval none for `show`, `first-use` for `task`. Limits: title 80, body 300 (today); 5 per run today, proposed 20 per hour per app, sharing the inbox cap (`MAX_NOTIFICATIONS` 300).
Errors: `QuotaExceeded`. Maps to `Inbox::add`, `Inbox::add_task`. Today `notify.show` returns nothing and takes no kind: extend, keep old call working.
`notify.show` returns today `undefined`; the new return value is additive.

### 4.2 `markdown`  (status: gap; effort M)

```ts
markdown.render(text: string, opts?: {width?: number}): Promise<{lines: number, plain: string, links: {text: string, url: string}[]}>   // analysis only, no drawing
markdown.openFile(path?: string): Promise<{id}>        // opens the viewer as its own tile; with no path the person picks (like host.openFile)
markdown.view(text: string, opts?: {title?: string}): Promise<{id}>
```
Embedded use is `embed.mount(..., 'markdown')` (section 5). Capability `markdown`. Approval: none for `view(text)`; `openFile` goes through the file dialog (person chooses, app never sees a path). Limits: 1 MB of text per view.
Links open only after the shell's link warning (`crate::links::warnings`), never run. Images: only files inside the app bundle or `data:` PNG/JPEG up to 256 KB; no remote images unless the app declared that host in `network`.
Maps to `mdtext.rs` (`layout_with`, `draw`, `link_rects`) wrapped in a new viewer `Component`. Needs: scrolling, find, link click, code-block copy (chat has some). No dependency.

### 4.3 `speech`  (text to speech: partly; speech to text: gap)

```ts
speech.voices(): Promise<{id: string, label: string, lang: string, where: 'this computer'|'network'|'internet'}[]>
speech.say(text: string, opts?: {voice?: string, speed?: number /*0.7..1.5*/, lang?: string}): Promise<{id}>   // resolves when started
speech.stop(id?): Promise<void>
speech.on('end'|'error'|'word', fn)
speech.listen(opts?: {lang?: string, maxSeconds?: number /*<=60*/, device?: string}): Promise<{id}>
speech.onResult(fn)    // {id, text}
```
Capability `speech` (`speech.say` only, "reads text aloud") and `microphone` (separate, "listens through the microphone"). Approval: `say` first-use plus the destination line from `voicedesk::consent_text` when text leaves the machine (network/internet engines);
`listen` **each-call**, with a visible recording bar, the mic name, and a Stop button; only after the person's click. Limits: `say` 20000 chars (`MAX_TEXT_CHARS`), 30 calls/hour/app; `listen` 60 s per call (hard `MAX_RECORD_SECS` is 300), 20 per hour; audio is never returned to the app, only text, and is dropped after transcription (as dictation does today).
Errors: `Unavailable` ("no microphone: built-in microphones of some laptops are not supported", from `voicedesk::no_mic_text_t2`), `Busy` (speaker in use while Settings -> Sound "Mix sounds from several apps" is off; with it on `say` is mixed as speech and gets no `Busy` for another program, `audiosink::mixing()`), `Blocked`.
Maps to `voice::synthesize_with`, `voicedesk::Reader`, `voicedesk::transcribe`, `Recorder`. Status: `say` partly (engine and Reader exist, app wrapper and consent card do not); `listen` gap and **blocked on T2 capture**: the apple-bce patch refuses capture streams unless `capture=1`, which hung a MacBookPro16,2 when played at the same time. Ship `listen` first for USB mics and non-T2 hardware, answer `Unavailable` on T2.
Effort: `say` M (depends on the mixer for overlap; before that `Busy`), `listen` L.

### 4.4 `audio`  (status: gap; effort M, L with a mixer)

```ts
audio.play(src: {file: string}|{url: string}|{wav: ArrayBuffer-as-base64}, opts?: {volume?: number, loop?: boolean, channel?: 'cue'|'music'|'speech'}): Promise<{id}>
audio.pause(id) / resume(id) / stop(id) / setVolume(id, v)
audio.on('end'|'error'|'level', fn)     // level: 8 band values for visualisers, from radio::band_levels
audio.outputs(): Promise<{name, active}[]>   // read only
```
Capability `audio`. Approval none (it only plays; Settings -> Sound volume and mute always apply). Limits: 1 stream per app; app-bundle files and `data:` WAV up to 2 MB; `url` only to declared `network` hosts; WAV/MP3 built in, other formats need ffmpeg (`Unavailable` else).
Today the T2 speaker is **one program at a time**: radio, browser audio, UI cues and TTS compete. Until the in-house mixer exists, `audio.play` takes the speaker only when free (`Busy` otherwise, `.holder` = "radio"/"browser"). Mixer design (later, in-house, above plughw, not dmix): fixed 48 kHz S16 stereo, one mixing thread, per-stream gain, soft clip, 1 reader per stream; target under 3 percent of a core for 3 streams. Maps to `audioout::route_args`, `softvol`, `syssounds::render`.

### 4.5 `media`  (status: partly; effort L)

```ts
media.library(kind?: 'audio'|'video'): Promise<{id, title, artist?, secs, kind}[]>   // metadata only, limited to the person's media folders (media::may_read)
media.play(id|{file}, opts?: {start?: number}): Promise<{id}>     // opens the MEDIA player as a tile
media.queue.add(id) / media.queue.list()
media.on('state', fn)    // playing|paused|ended, position
```
Embedded video: `embed.mount(el, 'media', {file, controls})` (phase 2). Capability `media`; approval first-use for library listing, each-call for `play` of a file outside the person's media folders. Maps to `media.rs` (`Library`, `MediaPlayer`, `shared()`), `player.rs`. Needs ffmpeg from the vendor (installer only). Frames are RGBA drawn by the shell: embedding in a tile page rectangle uses the same mechanism as the browser (section 5).

### 4.6 `browser`  (status: partly; effort M)

```ts
browser.open(url: string, opts?: {fullscreen?: boolean}): Promise<{id}>   // opens the BROWSER tile
browser.on('navigated', fn)   // not for opened tabs; for embeds only
```
Capability `browser` (open) and `embed` (embed). `open`: each-call unless the host is in the app's declared `network` list, then first-use. Only http/https (no `file:`, `javascript:`, `data:`). Embedded: section 5. Page fullscreen (just added, `browser_full.rs`) works inside an embed only if the app set `allowFullscreen: true`; Esc always leaves.
Maps to `apps/browser.rs`, `browserconsent.rs`.

### 4.7 `llm` and `models`  (status: partly; effort M)

```ts
llm.chat(messages: {role: 'system'|'user'|'assistant', content: string}[], opts?: {model?: string, maxTokens?: number, temperature?: number, json?: object /*schema*/, stream?: boolean}): Promise<{text: string, usage: {input: number, output: number, estimated: boolean}, model: string}>
llm.stream(messages, opts): {on('text'|'done'|'error', fn), cancel()}
llm.embed(texts: string[], opts?: {model?: string}): Promise<{vectors: number[][], dim: number}>
llm.usage(opts?: {since?: 'today'|'7d'|'30d'}): Promise<AppUsage>      // this app's own numbers only
models.list(opts?: {kind?: 'chat'|'embedding'|'image'|'stt'|'tts'}): Promise<{id, label, kind, where: 'this computer'|'network'|'internet', free: boolean}[]>
models.pick(opts?: {kind}): Promise<{id}|null>    // opens the person's model picker (apps/model_picker.rs); the choice is remembered for this app
```
Capabilities `llm` (chat/stream/embed), `models` (list/pick). Approval: **first-use card** that names the app, the chosen provider and whether text leaves the machine; **each-call** when the model is not free (cloud, key in vault) unless the person set an app budget (section 6) in the card ("allow up to 50 cents a day"). `agent.ask` stays: it is the assistant with tools; `llm.chat` is a bare model with no tools, no files, no memory.
Limits (defaults, administrator can lower): prompt 100k chars, `maxTokens` 4096, 20 calls/minute, 1 stream at a time, 60 s time limit; images not in v1. The app never sees provider name, URL or key; `models.list` returns ids and a `where` label only. Cloud mode: only remote models are listed.
Errors: `BudgetStop`, `Unavailable` ("no chat model configured. Open Settings -> Models", from `chat::Unconfigured`), `QuotaExceeded`, `Failed`.
Maps to `ChatBackend::complete_tools_stream`, `providers::ProviderClient`, `usage::Metered` (extended, section 6), `lemonade::parse_models`/`ModelType`, `vault::Secrets`. Embeddings: `rag.rs`/`TextEmbedder` in `embed.rs` exist for the OS's own use.

### 4.8 `dash`  (status: partly; effort M)

```ts
dash.sources(): Promise<{name, kind, language}[]>
dash.query(source: string, query: string, opts?: {range?: {from: string, to: string}, limit?: number /*<=1000*/}): Promise<{columns: {name, type}[], rows: any[][], truncated: boolean}>
dash.panel.add(dashboard: string, panel: {title, source, query, visual}): Promise<{id}>      // each-call
embed.mount(el, 'dash', {dashboard: string})   // phase 3
```
Capability `dash` with a source list ("may read from: prometheus-main"). Read-only by rule (`dash/readonly.rs`); queries are the person's own sources, so approval is first-use per source; adding panels is each-call. 1000 rows, 10 s, 30 queries/minute. Maps to `dash/engine.rs::run_panel`, `dash/registry.rs`, `dash/tools.rs`. Errors carry `DashError` text with its `fix` line.

### 4.9 `flows`  (status: partly; effort M)

```ts
flows.list(): Promise<{id, name, version, inputs: {name,type,required}[]}[]>
flows.run(flow: string, inputs?: object, opts?: {key?: string /*idempotency*/}): Promise<{run: string}>     // each-call (card shows flow, version, inputs)
flows.status(run: string): Promise<{state, steps: {node, state}[], error?: string}>
flows.on('run', fn)    // state changes of runs this app started
flows.cancel(run): Promise<void>
```
Capability `flows` listing the flow names it may run. App can run only **published** versions; never create, edit or publish (those stay the assistant's, always-asking, tools). 10 runs/hour/app. Maps to `flows/tools.rs` standalone path (`standalone_call`), `iyakki_flow::Engine::start_run`, `runs_of`.

### 4.10 `k3s`  (status: partly for tile apps via nothing; for services gap; effort L, depends on service package kind)

```ts
k3s.status(): Promise<{cluster: string, namespace: string, ready: boolean, writable: boolean}>
k3s.get(kind: string, opts?: {name?, selector?}): Promise<object[]>      // read only; Secrets are names only
k3s.logs(pod: string, opts?: {tail?: number /*<=500*/}): Promise<string[]>
k3s.apply(yaml: string): Promise<{plan: string}>     // each-call: shows the lint result and diff; only the app's own namespace
k3s.restart(name: string): Promise<void>             // each-call
```
Capability `k3s` with level `read` or `manage`. Only the namespace owned by the app (`<service>-<instance>`, proposal from the service kind) or the workspace namespace in the IDE. Never: namespace delete, cluster-scoped objects, other namespaces, Secret values, exec. Everything that changes asks every time and runs through `deploylint.rs` and the `k8stools.rs` checks (no privileged, hostPath, hostNetwork, `:latest`).
Maps to `k8s.rs::Kube`, `k8stools.rs`, `platform/engine.rs`. A tile cannot get this before the `service` kind defines "its namespace". Lower priority than 4.1 to 4.9.

### 4.11 `embed`  See section 5 (status: gap; effort L).

## 5. Embedding the markdown viewer and the browser

**Why it is hard here.** A tile page is drawn by IYAKKI's own page engine (`iyakki-web` layout, `tile.rs::render`): no canvas, no nested web views, and
`getBoundingClientRect` is a constant zero. The browser engine (`iyakki-webview`) is a separate helper process painted into one rectangle by the shell
(`browser_full.rs`); the IDE already shows it in a panel through `Component` (`component.rs`). So the page cannot host a component; it can only **ask the shell to paint a
component into a rectangle of the page area and route input to it**. The page owns the content and layout around it; the shell owns the pixels inside it.

### 5.1 JS API

```ts
embed.mount(target: string /*element id*/ | {x,y,w,h} /*CSS px in the page*/, kind: 'markdown'|'browser'|'media'|'dash', options: object): Promise<EmbedHandle>
EmbedHandle = {
  id: string,
  update(options: object): Promise<void>,         // same object shape as mount; only changed keys apply
  call(method: string, args?: object): Promise<any>,   // kind-specific: markdown.scrollTo, browser.back/forward/reload/stop
  focus(): void, blur(): void,
  unmount(): Promise<void>,
  on(event: string, fn): void, off(event, fn): void,
}
```
Options and events per kind:

| kind | options | events | notes |
|---|---|---|---|
| `markdown` | `{text}` or `{file}` (an app bundle path), `{theme?, toc?: boolean}` | `link` `{url}` (the app decides; default: shell link warning then BROWSER tile), `scroll` `{y, max}`, `ready` | links never auto-open; images as 4.2 |
| `browser` | `{url, mode: 'full'|'reader', readonly?: boolean, allow?: string[], allowFullscreen?: boolean}` | `navigated` `{url,title}`, `loading` `{on}`, `blocked` `{url, reason}`, `fullscreen` `{on}`, `closed` | maps to `EmbedParams` and `HostEvent` in `component.rs` |
| `media` | `{file, controls, loop}` | `state`, `ended`, `error` | phase 2 |
| `dash` | `{dashboard}` | `range` | phase 3 |

### 5.2 How the rectangle is found

Two ways, both host-side so the page cannot lie about geometry:

1. **By element** (preferred): the page puts a block element with an id and an explicit size (`<div id="doc" style="height:320px"></div>`) and calls `mount('doc', ...)`.
   The host looks the element up in its own layout result (`tile.rs` holds `Layout` with `prims`; this needs a lookup from element id to box: **gap, M**). The rectangle follows layout and scrolling of the page automatically, so the embed scrolls with the page and is clipped by the page area (the shell clips to `view`, the same clip it applies to the page's own primitives).
2. **By rect** (fallback and for tests): `{x,y,w,h}` in page CSS px relative to the page top; the page must call `update` itself when it moves. Used by the headless harness, which has no layout.

Constraints: min 120x80, max the page column (900 px) by 4 screens high; at most **2 embeds per tile** and 1 browser (the web engine runs one helper per tab; memory is the limit, see `memgate.rs`); an embed is hidden (and its helper paused) while the tile is not on screen or the element scrolls fully out of view.

### 5.3 Focus, keys, scrolling

* Clicking inside an embed gives it focus; Tab leaves it at the end (`HostEvent`-style hand-back, like Esc in the IDE's browser panel). While focused, the shell does not give keys to page controls, and `Component::wants_text` decides whether the host keeps its own letter shortcuts out.
* Mouse wheel over an embed scrolls the embed first; at its end the wheel scrolls the page (browser and markdown both report "at edge").
* Esc in an embedded browser: first leaves page fullscreen, then hands focus back to the page, never closes the tile.
* Hit ids: each mounted embed gets a **dynamic slot** code from an allocator replacing the fixed `STRIDE` codes 1 to 5 (`route::BROWSER` = 5); a test renders two embeds in one frame and asserts `route::duplicate_ids` is empty (already specified in `docs/EMBEDDING-AND-BACKEND.md`, risk 1).
* The page never sees pixels, key events inside an embed, or the page's cookies; it only gets the typed events above.

### 5.4 Permissions and the browser's network allow-list

* Capability `embed` lists kinds: `"embed": ["markdown","browser"]`. Browser embeds add the hosts: `"embed_hosts": ["docs.example.com"]` (exact names, no wildcards, like `network`).
* **The embedded browser may navigate only to `embed_hosts` that the person's host policy also allows** (`Policy::allows_host`, same intersection as `reachable_hosts` for `network`). A navigation elsewhere (redirect, link, `window.open`) is not followed: the embed shows a "left the allowed sites" strip and fires `blocked`; a button opens the address in the full BROWSER tile, where the usual consent and content blocking apply.
  Subresources (images, scripts) follow the page's own origin rules of the engine; the content blocker stays on.
* The embedded page has its **own cookie jar** per app (not the person's browser profile; same rule as the IDE embed, EMBEDDING risk 5), no saved logins, no downloads, no camera/mic/location, no popups, no file chooser.
* The assistant's `browser_*` tools do **not** act on an app's embed unless the person opens it in the BROWSER tile (an embed is untrusted ground for prompt injection, section 11).
* First mount of each kind per app: card "App X wants to show a web page inside itself (docs.example.com)". `markdown` needs no card.

### 5.5 Rust components

`apps/component.rs::Component` is the contract: `kind`, `title`, `params`/`set_params`, `render(g, rect, view, cx)`, `handle`, `tick`, `drain_events`, `focus`, `wants_text`, `close`.
Plan (matches `docs/EMBEDDING-AND-BACKEND.md` B0): add `ComponentRegistry` (`kind` -> constructor with JSON params and the policy for its default consent), a `TileHost<C>` wrapper (opens any component full screen with Back), and a dynamic slot allocator.
A Rust app (built-in tile) embeds with `route::render_slot` as the IDE does; a tile page's `embed.mount` is just another host of the same registry, so one `MarkdownView` component serves chat, IDE, the MD tile and tile pages.
Status of pieces: `Component`, `route`, browser component **exists**; registry, `TileHost`, markdown component, element-box lookup, per-app cookie jar and allow-list **gap**. Effort: markdown embed M (after viewer), browser embed L.

## 6. Usage, budgets and limits

### 6.1 What exists today (verified)

* `crates/iyakki-agent/src/usage.rs`: `UsageLog` (file `usage.jsonl` in the data folder, opened in `services.rs`), `Entry {h (hour), user, provider, model, input, output, calls}`, compacted into hourly totals, kept 400 days (`KEEP_DAYS`), charts by day/week/month/year (`series_of`, `Range`); shown in the shell's `apps/usage.rs`.
* `Metered` wraps a `ChatBackend`, counts `providers::reported_usage()` when the provider reports it, else a length estimate (~4 chars per token). It records only totals, "never what was said".
* **Attribution:** by user, provider and model only. It reads the provider and model from the person's current chat setting (`self.services.settings...chat`), **so it does not know which app, tile, MCP server or external agent made a call**; an error, latency and cost are not recorded; there is no per-day cap.
* Limits that exist: `RunLimits` in `agent.rs` (`max_tokens`, `max_cost_cents`, `price_cents_per_mtok`, `max_seconds`, `max_files`) from policy `AgentBudget` (presets tiny/small/standard/large/unlimited in `iyakki-sec/src/policy.rs`), checked before each model call of an assistant run; a run's `RunEvent::Usage {tokens, cost_cents, files, seconds}`. Per **run**, not per day, not per app. Cost needs a price set in Settings, otherwise unmetered.
* STT, TTS and embeddings are **not** metered. `extagent` (Claude Code, Copilot) runs use their own vendor accounts: IYAKKI sees events but not an exact token log for them (gap: record what the CLI's event stream reports, if it does; unverified).

### 6.2 Proposed record

One line per call (compacted per hour like today), new fields **additive** so old lines still read:

```json
{"h":490000,"user":"admin","src":"app:calc","cat":"llm","provider":"lemonade","model":"qwen3-8b",
 "input":812,"output":240,"est":false,"calls":1,"errors":0,"ms":3100,"cents":0.0,"priced":false}
```
* `src` (who): `assistant`, `app:<id>` (tile), `mcp:<server>`, `svc:<instance>` (k3s service by its token), `ext:claude-code|copilot|custom:<id>`, `system` (OS features, e.g. dictation). Attribution for tile apps is free: the bridge knows the app id (`AppInfo.id`). For services it is the HTTP token's app. For MCP servers the host tags calls made through the policy bridge by the run token. For external agents, by the agent id the bridge already audits.
* `cat` (what): `llm` | `assistant` (the OS's own chat/agents; **separate** from apps) | `embed` | `stt` (seconds of audio in `input`) | `tts` (characters in `input`) | `image`.
* `cents` only when a price is known (`price_cents_per_mtok`; per-model prices from the person); `priced:false` otherwise and UIs say "not priced", never "free" for a cloud model.
* `ms` latency (summed per hour with a count so mean and max can be shown), `errors` count by hour. Prompts and replies are never stored.

### 6.3 Budgets and limits

| Limit | Default | Set by | On reaching |
|---|---|---|---|
| per-call `maxTokens` | 4096 | app option, capped by person/admin | `BadArgument` |
| per-app daily tokens | 200k (cloud), unlimited (this computer) | person in the first-use card / Settings; admin ceiling | warning at 80 percent (toast + inbox note once/day), `BudgetStop` at 100 percent until local midnight |
| per-app daily cost | 50 cents (cloud only) | same | same |
| per-app rate | 20 llm calls/min, 1 stream | admin | `QuotaExceeded` |
| person-wide daily | none by default | admin policy | `BudgetStop` for every app, assistant exempt only if the admin says so |
| `stt`/`tts` | 20 listens/hour, 30 says/hour | admin | `QuotaExceeded` |

A hard stop is checked **before** the provider call (using the estimated prompt size, like `RunLimits` does), so a stopped app costs nothing. Every stop, every warning, and every change of an app's budget is an **audit entry** (`app.budget.warn`, `app.budget.stop`, `app.budget.set`, with app id, never the content). Enforcement is on the host, in one `Metered`-style wrapper all three API forms share; an app cannot lower its own limits or reset counters.

### 6.4 Where the numbers show

* Settings -> Usage (exists, `apps/usage.rs`): add a "By app" list and the categories (apps / assistant / speech / embeddings) with today's tokens, calls, errors, cost.
* App Info panel (`tile.rs::info_card`): "Today: 12 calls, 9.4k tokens, ~0.08 USD (priced) of your 50 cents limit", with a button "Change limit".
* `iyakki dev usage [--app X] [--since 7d] [--json]` (scope `debug`, own account's apps for non-admins) and `GET /v1/dev/usage`.
* `iyakki.llm.usage()` / `GET /v1/usage` / MCP `usage_get`: **only the calling app's own numbers**: `{today: {calls, input, output, cents, priced}, limits: {tokensPerDay, centsPerDay}, remaining: {...}, byDay: [...]}`.
* Per-person view of all apps is the person's (and admin's), never another account's.

Status: log **exists**; per-app attribution, categories, caps, warnings, audit, UI, CLI **gap**. Effort M (wrapper + record change S, caps and warnings S, UI S, CLI S). Depends on the `llm` namespace.

## 7. The remote debug loop (agents drive a device)

Goal: "run the app on the device, test, fix, screenshot, fix", done by an agent from a PC or from the AI IDE on the device.

### 7.1 Inventory

| Need | Today | Status |
|---|---|---|
| pair, scopes per computer | `devmode/` (`scopes.rs`: `install`, `debug`, `files`, `files:system`, `config`, `input`, `input:password`, `shell`, `power`, `update`, `forward`), `iyakki dev pair/whoami/devices` | exists |
| install/push/watch, run, uninstall | `iyakki dev install`, `push --watch --open`, `run`, `uninstall`, `list` (`devmode/app.rs`, `cli.rs`) | exists |
| app console and logs | `iyakki dev logs <id> --follow`, `logs --system` (`debug`) | exists |
| screenshot, short recording | `iyakki dev screenshot [-o] [--record S --fps N]` (PNG sequence, at most 120 frames; display 0 only) | exists |
| input | `iyakki dev input tap/key/text/swipe` (`devmode/input.rs`: validated, queue 256, password focus refused without `input:password`) | exists |
| app state/storage, config, restart | `iyakki dev app data/config/clear-data/reset`, `dev config ...`, `dev reboot` (approval each time) | exists |
| device info, status, ps, dmesg | `dev info/status/ps/dmesg` | exists |
| run the **headless tile tests** | IDE tool `tile_test {path}` (`store/authtools.rs`, `tile::run_tests`) on the same machine as the files; **no `iyakki dev test`** (PC runs need the IYAKKI program locally) | partly |
| on-device acceptance checks / UI assertions | none: `dev input` + screenshot only; no "read the screen as text" or "wait until text X" | gap |
| MCP front for the CLI | none (`extagent/bridge.rs` serves a chat run, not developer mode) | gap |
| screen picture inside the AI IDE | assistant tool `screen_capture` (`screencap.rs`, asks every time, "allow 5 minutes", audit `screen.capture`); IDE has `tile_test`, `tile_install_dev`, `tile_scaffold`, `tile_validate`, `ide_browser_check` (screenshot into `.iyakki/evidence`) | partly |
| emulator target | QEMU scripts `dist/buildroot/qemu-*.sh`, `docs/LOCAL-TEST.md`; no auto-pairing | gap |

### 7.2 (a) MCP server `iyakki dev mcp`  (gap; effort M)

A stdio MCP server run on the PC: `claude mcp add iyakki -- iyakki dev mcp --device NAME`. It is a thin client of the existing paired-key HTTP API (`/v1/dev/*`, `devmode/client.rs`) so **scopes, approvals and audit are the device's, not the server's**: the MCP process holds no powers the key lacks.

| Tool | CLI equivalent | Scope | Notes |
|---|---|---|---|
| `dev_whoami`, `dev_status` | `whoami`, `status` | install / debug | first call; reports scopes so the agent knows what it can ask |
| `app_push {folder, open?}` | `push` | install | returns build errors as text |
| `app_run {id}`, `app_remove {id}` | `run`, `uninstall` | install | |
| `app_logs {id, tail, since}` | `logs` | install | bounded (default 200 lines), redacted as today |
| `sys_logs {source, grep, n}` | `logs --system`, `dmesg` | debug | |
| `tile_test {folder}` | (new) run `tests/*.js` on the real engine | install | gap on device: send the bundle to a new `POST /v1/dev/app/test`; the device runs `tile::run_tests` and returns the report |
| `screen_shot {scale?, region?, format?}` | `screenshot` | install | returns an MCP image (see 7.5) |
| `screen_record {seconds, fps}` | `screenshot --record` | install | returns N small frames, max 12 returned inline |
| `screen_text {}` | (new) the shell's own text of what is on screen (labels, field values, no password fields) | install | cheap alternative to vision; gap, M |
| `wait_for {text, timeout}` | (new) poll `screen_text` | install | gap, S once `screen_text` exists |
| `input_tap/key/text/swipe` | `input ...` | input (+`input:password`) | |
| `app_data {id, key?}`, `app_data_set`, `app_clear` | `app data ...` | config | |
| `config_get/set` | `config ...` | config | whitelisted keys only |
| `device_reboot` | `reboot` | power | the device asks on its screen |
| `usage_get {app?, since?}` | `usage` (new) | debug | section 6 |

Not offered through MCP at all in v1: `shell`, `files:system`, `update`, `forward`, `power off` (the person uses the CLI if they mean it). Every call is an audit line `devmode.<action>` with the paired computer's name plus `via=mcp`.

### 7.3 (b) The loop contract

An agent-readable contract shipped as a skill (`dist/skills/iyakki-device-loop.md`, ASCII, under 3950 characters) and returned by `dev_whoami`:

1. **Plan:** state the acceptance check first, as something the tools can observe: a test name that passes (`tile_test`), a text that appears (`wait_for`), a console line, or a screenshot matching a described state. Write it down before changing anything.
2. **Change** the smallest thing. **Push** (`app_push`; reload is automatic with `--watch`). **Run** (`app_run`).
3. **Observe in this order of cost:** `app_logs` (console errors) -> `tile_test` -> `screen_text` -> `screen_shot`. Use vision only when text cannot answer.
4. **Decide:** check met -> stop and report evidence (test counts, final screenshot path, log excerpt). Not met -> change exactly one thing and repeat.
5. **Stop conditions:** acceptance met; **8 iterations** without progress (same failure twice counts as no progress); any `Denied`, `Blocked`, locked-screen or "nobody signed in" answer (report it, do not retry); any request that needs a scope the key lacks (ask the person); token budget for the loop reached (agent states it up front).
6. **Needs the person on the device screen:** first pairing, scope grants, `shell`, reboot, `input:password`, install of a signed store package, any cost-bearing call the app makes while under test (the app's own approval cards appear on the device; the agent sees "waiting for approval" and stops rather than tapping Allow on the person's behalf unless the person granted `input` and asked for it; the skill says to ask first).
7. Report honestly: what was run, counts, what was not tested.

### 7.4 (c) The same loop inside the on-device AI IDE (no PC)

The IDE assistant (and Claude Code through `extagent/bridge.rs`) already has `tile_scaffold`, `tile_validate`, `tile_test`, `tile_install_dev` (asks, administrators), `ide_browser_open/check`, and `screen_capture` (asks every time). Missing for a closed loop:
* `tile_run {id}` + `tile_logs {id}` as IDE tools (the device-side half of `app_run`/`app_logs`; the page console log already exists for `dev logs`): S.
* `screen_text` and `wait_for` as tools (shared implementation with 7.2): M.
* `tile_input {...}` for the IDE: **no**; inside the IDE the agent must not tap the shell it runs in. It uses the headless harness with `T.click`/`T.type` (real engine, no hardware) and `screen_capture` for what the person sees.
* The loop skill text is the same; the tool names map 1 to 1 (a table in the skill).
Effort: M total.

### 7.5 (d) Guard rails

* **Scopes per computer** as today; MCP never widens them; `iyakki dev mcp --read-only` exposes only observe tools.
* **Passwords:** `input` into a password field refused without `input:password` (`input::set_secret_focus`); screenshots while a password field has focus are **blanked in the region** (proposal; today the picture shows the screen as drawn, gap, S) and `screen_text` omits it.
* **Rate limits:** screenshots 1 per second and 120 per hour per computer; input events as today (queue 256); `app_push` 1 per 3 s; tile tests 6 per minute.
* **Audit:** one line per call in the device's `activity.log` and system audit (`devmode.screenshot`, `devmode.input` with length not text, new `devmode.test`, `devmode.screentext`); a loop id (`--loop X`) the client sends lets the person read one loop as a block.
* **Locked screen / nobody signed in:** `run`, `screenshot` and `input` answer `Unavailable: no one is signed in` (today `run` is queued until sign-in; the MCP reports the queue state and does not wait more than 30 s). A screensaver or lock is never dismissed by the tools. Reads of logs and `tile_test` keep working.
* **Screenshot token cost:** options `scale` (default 0.5, i.e. about 960x540 on a 1080p screen), `region {x,y,w,h}`, `format` (`png` default for UI text, `jpeg` q60 for photos), `max_edge`. Rule of thumb: a 1024x576 image is about 800 tokens for a vision model, 1920x1080 about 2.8k; the skill tells agents to start at 0.5 scale and crop to a region before asking for more. `screen_record` returns at most 12 frames at 0.35 scale unless asked.
* **Secrets:** logs and activity are redacted as today; app storage dumps (`app_data`) only for apps developer mode installed.

### 7.6 (e) QEMU as a loop target  (gap; effort M)

A script `dist/buildroot/qemu-dev.sh` (proposal) boots the existing image in QEMU with user-mode networking, forwards the developer-mode port to the host loopback, switches developer mode on and **pairs automatically through a first-boot file** on a virtual disk (`dev-autopair.json`: public key of the PC, scopes `install,debug,input,config`, expiry 1 hour, only honoured when `iyakki.devmode_autopair=1` is on the kernel line, which the production images never set). `iyakki dev devices` then lists `qemu` and the MCP server targets it with `--device qemu`. Screenshots come from the same API (the guest draws on its own framebuffer), so no QEMU monitor access is needed. No audio, no microphone, no T2 hardware: say so in `dev_whoami` (`hardware: "qemu"`) so agents skip `speech.listen` and audio assertions. This builds on `qemu-test.sh` and `LOCAL-TEST.md`; it needs no image rebuild beyond the autopair read in the shell (S) and the script (S).

## 8. Test fakes (every namespace)

Two harnesses: the **emulator** (window mode with a fake backend switch, `iyakki dev emulate`, proposal) and the headless `tile_test` harness (`tile::run_tests`, `T.*`). Both install fakes through a `iyakki-test` object that exists only there:

```js
T.fake.llm.reply('Hello')           // next llm.chat returns this; also T.fake.llm.fail('Unavailable')
T.fake.llm.usage(120, 30)           // tokens reported
T.fake.models.set([{id:'m1',label:'Fast',kind:'chat',where:'this computer'}])
T.fake.speech.voices([...]); T.fake.speech.heard('turn on the light')  // result for the next listen
T.fake.audio.busy('radio')          // next play fails with Busy
T.fake.media.library([...]); T.fake.dash.frame(cols, rows); T.fake.flows.run('f1', {state:'succeeded'})
T.fake.k3s.objects('pods', [...]); T.fake.notify.sent()    // list of what the app posted
T.fake.embed.events('browser', [{event:'navigated', url:'https://x'}])   // drives on('navigated')
T.fake.approve('each-call'|'deny')  // the person's answer to the next card
T.fake.clock.advance(60000); T.fake.budget.set({tokensPerDay: 100})
```
Rules: fakes never touch network, microphone, speaker or cluster; the real host code path for argument checks, capabilities, quotas and error codes still runs (as today's `TileHost` does), so an app cannot pass tests with fakes and fail on `BadArgument`. Existing pieces to extend: `store/tile.rs::run_tests`, `TileHost::with_clock`, `Outbox`/`Request` (the requests list is how tests see cards), shell `cues::fake` (a fake audio sink for cues), `lemonade_fake.rs`, `extagent/fake.rs`. For Rust code, each namespace is a trait with a `Fake` in the same module, like `softvol::Runner` and `k8s::Kube`. Status: harness exists; `T.fake.*` gap; effort S per namespace, M for the framework.

## 9. The three forms

### 9.1 JS bridge (tiles)
Extension of `bridge.rs`: new ops `llm.chat`, `speech.say`, `embed.mount`, ... in the prelude (page-side checks) and in `TileHost::call` (host-side checks), long ones through the existing `{pending:id}` + `__iyakkiResolve` mechanism. Events need a second path: a host-to-page `__iyakkiEmit(handleId, event, payload)` defined like `__iyakkiResolve` (non-writable). `app.json` capabilities grow; `Caps` and `permissions()` (`tile:<name>`) get the new names so the store vocabulary and `store/vet.rs` plain-words text cover them.

### 9.2 Local HTTP API with per-app tokens (k3s services, PC tools)  (gap; effort M)

* **Server:** in the agent, on loopback `127.0.0.1:<port>` plus the cluster-facing address below; HTTP/1.1, JSON, one request per connection (`httpmini.rs` limits: 16 KiB head, 64 KiB body; raise body to 1 MiB for this API), SSE for streams. Not reachable from the LAN: the firewall default (`docs/FIREWALL.md`) already blocks incoming.
* **Auth:** `Authorization: Bearer iyk_<random 32 bytes base32>`. A token belongs to one app instance (`svc:<instance>`), is minted by the install wizard / `iyakki dev`, stored in the vault and injected into the pod as a Secret named by `secrets[].mount` in the service manifest (`IYAKKI_TOKEN`) with `IYAKKI_API` holding the base address. Tokens are stored hashed on the device, scoped to the capabilities in the service's manifest, revocable (Settings, Dev cluster), expire after 90 days, and are never accepted from query strings.
* **CORS-free:** no `Access-Control-*` headers are ever sent and `Origin` headers are refused (`403`), so no web page can call it; it is for servers and tools only.
* **Reach from a pod:** the API listens on the node IP the cluster uses (not `0.0.0.0`), and a headless Service + Endpoints named `iyakki-api` in the service's namespace points to it, so the pod calls `http://iyakki-api:7421`. A NetworkPolicy example (not applied by default) limits egress to that address. Remote or cloud clusters get no route (the API is local-device only; `Unavailable`), unless the person creates a tunnel; stated honestly in the wizard. (Exact port and mechanism: open decision 6.)
* **Limits:** per-token rate 20 req/s, body 1 MiB, response 4 MiB, same budgets as section 6 under `src = svc:<instance>`.

| Method and route | Capability | Same as | Notes |
|---|---|---|---|
| `GET /v1/info` | none | `app.info` | id, version, capabilities, limits |
| `POST /v1/notify` | notifications | `notify.show/task` | |
| `POST /v1/llm/chat` (SSE with `stream:true`) | llm | `llm.chat/stream` | |
| `POST /v1/llm/embed` | llm | `llm.embed` | |
| `GET /v1/models` | models | `models.list` | |
| `GET /v1/usage` | llm | `llm.usage` | own numbers only |
| `POST /v1/speech/say` -> `{id}`, `DELETE /v1/speech/{id}` | speech | `speech.say/stop` | |
| `POST /v1/speech/listen` | microphone | `speech.listen` | each-call card on the device screen, `202` + poll `GET /v1/speech/{id}` |
| `POST /v1/audio/play`, `DELETE /v1/audio/{id}` | audio | `audio.play/stop` | |
| `POST /v1/markdown/render` | markdown | `markdown.render` | |
| `GET /v1/dash/sources`, `POST /v1/dash/query` | dash | `dash.*` | |
| `GET /v1/flows`, `POST /v1/flows/{id}/runs`, `GET /v1/runs/{id}` | flows | `flows.*` | |
| `GET /v1/k3s/{kind}`, `GET /v1/k3s/logs/{pod}` , `POST /v1/k3s/apply` | k3s | `k3s.*` | apply returns `202` until the person approves |
| `GET /v1/approvals/{id}` | none | pending card state | for `202` calls |

Errors: HTTP status 400 `BadArgument`, 401 bad token, 403 `NoCapability`/`Denied`/`Blocked`, 408 `Timeout`, 409 `Busy`, 429 `QuotaExceeded`/`BudgetStop`, 502 `Failed`, 503 `Unavailable`; body `{"error":{"code":"BudgetStop","message":"...","resets_at":1790000000}}`.
`embed` has no HTTP form (it needs a page).

### 9.3 MCP tools (assistants)  (partly; effort S per area)
Tool names `iyakki_<ns>_<fn>` (e.g. `iyakki_llm_chat`, `iyakki_notify_show`), schemas from the same table as the `.d.ts` (one source file generates all). The assistant already has many (`dash_*`, `flow_*`, `k8s_*`, `tile_*`); the new part is exposing the same set to **installed MCP servers and external agents** under an app identity (`src = mcp:<server>`) with the capability list from the package, through `extagent/bridge.rs`'s `Gate` (approvals identical to the built-in loop). Tools that need a page (`embed`) are not offered. Device-side developer tools are separate (7.2).

### 9.4 Typed `.d.ts`
Generated from one JSON description of the API (`dist/sdk/api.json`, proposal): `iyakki.d.ts` with `declare const iyakki: Iyakki;` and interfaces per namespace, error code union, `AppUsage`, `EmbedHandle`. Shipped inside the scaffold (`tile_scaffold` writes `types/iyakki.d.ts`) and used by `tile_validate` to warn on unknown calls. Effort S.

### 9.5 Dependency-free Python client (outline)
`iyakki_sdk.py`, standard library only (`urllib.request`, `json`): `Client(base=os.environ["IYAKKI_API"], token=os.environ["IYAKKI_TOKEN"])`; one `_call(method, path, body)` with timeout and redacted errors; methods mirror 9.2 (`llm_chat`, `notify`, `usage`, `speech_say`, ...); `stream()` yields SSE lines; raises `IyakkiError(code, message)` with the code table of section 3; honours `Retry-After` on 429 once. Ships in the `service` scaffold and as a store skill. A Node twin is the same size. Effort S each.

## 10. Phased order

| Phase | Items | Why first | Depends on |
|---|---|---|---|
| 1 | `notify` (extend), `markdown` viewer + `markdown.*`, `speech.say`, `llm` + `models` + usage attribution/budgets (6), `embed` for markdown, `.d.ts`, test fakes (framework + these), `iyakki dev usage` | small, safe, mostly wrapping code that exists; usage first so money is never unmetered | markdown viewer component; `Metered` change |
| 2 | `audio` (with the in-house mixer), `media`, `embed` browser (allow-list, cookie jar), `dash`, `flows`, HTTP API with tokens, `iyakki dev mcp` + device loop (7), QEMU autopair | each needs one new mechanism (mixer, per-app jar, token server) | mixer; element-box lookup; token store |
| 3 | `k3s`, `speech.listen`, `embed` media/dash, Python/Node clients polish | need the `service` package kind (namespace ownership) and safe mic capture | service kind; T2 capture fix or USB-only |

Effort summary: notify S, markdown M, speech.say M, listen L, audio M (+mixer L), media L, browser M, embed L, llm M, usage M, dash M, flows M, k3s L, HTTP M, dev MCP M, fakes M, d.ts S.

## 11. Security review notes

| Risk | What could go wrong | Mitigation in this design |
|---|---|---|
| Prompt injection through embedded pages | an embedded page (or markdown file from the web) holds text that tries to steer an assistant that reads the app, or the app passes page text to `llm.chat`/`agent.ask` | the assistant's browser tools do not act on app embeds; `agent.ask` and `llm.chat` carry only what the app sends, and tool-less `llm.chat` cannot act; page text stays labelled untrusted in any assistant context (`ToolDef.untrusted`); embeds cannot trigger approvals for other things |
| Embed escape | embedded browser navigates to a phishing look-alike inside a trusted app frame | allow-list of exact hosts, a visible address strip the app cannot hide, `blocked` strip + open in BROWSER tile, own cookie jar, no password manager, no downloads |
| Token theft | a service token leaks from a pod or log and is used from elsewhere | tokens only valid from the node/cluster address, hashed at rest, injected as Secret, 90-day expiry, per-token scope and rate, revocation, no query-string tokens, redacted in logs, audit with source address |
| Cost abuse | a loop or hostile app burns a cloud key | key never visible, per-app daily caps checked before the call, first-use card names the provider, hard stop code, person-wide cap, rate limits, audit of stops, `cents` shown honestly (not priced is said) |
| Microphone abuse | an app records without consent | separate `microphone` capability, each-call card, visible recording bar, 60 s cap, audio never returned, T2 capture off, no background listening, audit entry per listen |
| Speaker abuse | an app plays loud or endless sound | Settings volume/mute always apply, one stream per app, stops when the tile closes, `Busy` instead of stealing the speaker |
| Cluster abuse | an app applies a privileged workload or deletes data | own namespace only, `deploylint`, no cluster-scoped objects, each-call plan with diff, typed confirmation for destructive actions, no Secret values, no exec |
| Screenshot/loop abuse | an agent reads private screens or taps Allow on the person's behalf | `input` scope separate, password fields blanked, rate limits, approval cards need the person unless granted, locked screen refuses, audit per call |
| Fake data | an app fills the inbox | per-app notify rate, inbox cap, per-app label on every note, mute per app |
| API surface growth | many calls, each a bug | one description file generates JS checks, `.d.ts`, HTTP validation and MCP schemas; every call has a refusal test (the repo rule: a test for every tool and every refusal) |

## 12. Versioning and compatibility

* `iyakki.version` is `MAJOR.MINOR`. Within a MAJOR, only **additions**: new functions, new optional arguments, new event names, new error `.detail` text. Codes, argument meaning and defaults never change.
* `app.json` may state `"sdk": "1.2"` (minimum); the device answers `Unsupported` at install if older, and the store lists it. Without it, `1.0` is assumed (today's bridge = 1.0).
* A function is deprecated for at least two alpha/beta releases: it still works, `tile_validate` warns, Info lists the use. A MAJOR bump keeps the previous MAJOR available for one release via `"sdk": "1"` shims.
* HTTP routes live under `/v1/`; a new MAJOR adds `/v2/` side by side. MCP tool names carry no version, schemas only add optional properties.
* The usage record only adds fields; readers ignore unknown ones (as `UsageLog::load` does by skipping unparsable lines, so a downgrade loses only the new fields).
* Contract tests: a golden `api.json` per released version; a test fails if a released call changes shape.

## 13. Open decisions for the owner

1. **Mixer before audio?** Ship `audio.play`/`speech.say` with `Busy` on the one-program speaker now, or wait for the in-house mixer (L)? Recommended: ship with `Busy`, build the mixer in phase 2.
2. **`llm.chat` approval:** first-use card plus per-app budget (recommended), or an each-call card for every cloud call? And default daily caps: 200k tokens / 50 cents for cloud models, unlimited local?
3. **Do apps get the person's default model or must they pick?** Recommended: default model, with `models.pick` to let the person choose per app.
4. **Embedded browser cookie jar:** per app, wiped on app removal (recommended), or session-only?
5. **Markdown links:** open in BROWSER tile after warning (recommended) or let the app handle every link?
6. **HTTP API address:** one fixed port on the node IP plus an `iyakki-api` Service (proposed), or a sidecar? How do remote/cloud clusters reach it, or do we say "local device only"?
7. **Service tokens:** 90-day expiry and hashed storage, or longer-lived with rotation by the wizard?
8. **Microphone on T2:** keep `speech.listen` as `Unavailable` there until capture is proven safe, or enable `capture=1` behind a Settings switch with a clear risk text?
9. **External agents in usage:** can we log Claude Code / Copilot usage at all (depends on their event streams, unverified), or only count calls and time?
10. **Device loop:** may `iyakki dev mcp` tap approval cards (needs `input`), or must the person always press Allow on the device? Recommended: never by default. Also: blank password fields in screenshots always, or only when the lock/secret focus flag is set?
11. **QEMU autopair:** accept a kernel-line switch (`iyakki.devmode_autopair=1`) in the production image's code path, or ship a separate dev-only image variant?
12. **`screen_text`:** acceptable to expose the shell's text of the screen (labels, values) to a paired computer with `install`, or require `debug`?
13. **Service package kind** (`CLAUDE-CODE-APP-DEVELOPMENT.md`, section 6): k3s and the HTTP API depend on it; approve it first?
14. **Element-box lookup in the page engine:** extend `iyakki-web` layout to report an element's box to the host (M), or accept rect-only mounting in v1?
15. **Where to hold the API description** (`dist/sdk/api.json`) and who reviews changes (a CI test that diffs against the golden file)?

### Owner's answers (2026-10-24)

Answered (these are decisions, not proposals; the numbers refer to the list above):

* **1. Mixer.** Ship `audio` and `speech` with `Busy` while no mixer is enabled. **Build an in-house mixer; when it is enabled, the APIs use it.** The mixer lowers (ducks) other
  programs' volume and plays notification tones over them. (The T2 speaker currently runs `plug > softvol > plughw` because ALSA's dmix spins a core there; the mixer sits above plughw.)
* **2. LLM limits and approval.** Limits follow the **model/provider settings**. External agents such as Claude Code emit their own session limits; show and honour those instead of
  inventing numbers for them. (Default numbers for the built-in providers are still to be written into the settings page.)
* **4. Embedded browser cookie jar.** Shared only **if the person permits it** (a per-app permission, off by default).
* **6. How pods reach the local API.** Through a **skill (`skill.md`) with instructions**; the chat assistant can set it up for the person. No sidecar is built for this.
* **10. The dev agent tapping approval cards.** Only **if the person permits it**, for everything or **partially by a filter** (which kinds of card, which app). Off by default.
* **13. Service package kind: approved.** The install wizard shows the person **every permission the service asks for** (what it will use) and nothing is granted without that.

Still open: 3, 5, 7, 8, 9, 11, 12, 14, 15.
