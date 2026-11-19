---
name: iyakki-sdk-component
description: Add a new embeddable component to the IYAKKI UI SDK (the pattern used for ChatView, BrowserView, FilesView, TerminalView, EditorView, AudioView and CameraView): a core facet, a fake, a real implementation, an SDK port with an in-memory fake, a view, the SAMPLE pane, the shell adapter, a chat tool with approvals, tests, docs and feature rows. Use when someone asks to make something (a calendar, a map, a contacts list, a player...) embeddable in an SDK app.
---

# Adding an embeddable SDK component

A component is the same seven pieces every time. The earlier ones are the worked examples; read the closest one first (files is the smallest, camera the fullest):

| Piece | Where (FilesView example; CameraView in brackets) | Notes |
|---|---|---|
| 1. Core facet | `crates/iyakki-core-api/src/files.rs` (`cameras.rs`), exported from `lib.rs`; a method on the core trait in `uicore.rs` (`Core::files()`) with a default that refuses ("this core has no file browser") | traits only, no I/O; every refusal is a sentence; a `check_*` function shared by all implementations |
| 2. Fake of the facet | `crates/iyakki-core-fake/src/files.rs` (`cameras.rs`), field and accessor in `fakecore.rs` (`FakeCore::file_tree_fake()`), `pub mod` in `lib.rs` | same rules as the real one, plus an audit list and test handles |
| 3. Real implementation | local: `crates/iyakki-core-local/src/homebrowse.rs`; device: an adapter on `AgentCore` (`corehandle.rs`: `Core::files()` returns it) or `iyakki-agent/src/embedcameras.rs` | every action: permission check, approval where it changes something, audit line, no secret in the line |
| 4. SDK port + memory fake | `crates/iyakki-ui-sdk/src/files.rs` (`cameras.rs`): trait `FilesPort`, `FilesError`, `MemFiles` | the SDK has no file/socket/device access; the port is what an app is handed; `Mem*` follows the core's rules so a test is faithful |
| 5. The view | `crates/iyakki-ui-sdk/src/filesview.rs` (`cameraview.rs`) | see "The view" below |
| 6. SAMPLE pane + shell adapter | `crates/iyakki-app-sample/src/lib.rs` (a pane, a tab, a key), `crates/iyakki-apps/src/apps/sample.rs` (`CoreFilesPort`) or its own `*_port.rs` / `sdk_*.rs` | the adapter turns the port into `cx.core.files()` calls bound to the person and the app |
| 7. Chat tool + switch | `crates/iyakki-apps/src/apps/chat_embed*.rs` (`chat_embedcameras.rs`), registered in `chat_settings.rs`; a row in `docs/development/CHAT-REACHABILITY.md` | a free status tool and an asking set tool (on/off for the person); off closes open views at once |

## The view (rules every component follows)

- `pub struct XView`; `new(port: Box<dyn XPort>, action_base: u32)`; `pub const ACTIONS: u32` (the count of numbers it uses); `owns(a)`; `tick()` (or `tick(now_ms)`) polls the port; `action(a) -> bool`, `key(k) -> bool`, `wheel(n)`, `draw(c, th, &metrics, rect)`.
- Draw only through `Canvas`; colours from the theme, sizes from the metrics tokens: no styling numbers. Add the file to the style-literal scan (`crates/iyakki-ui-sdk/src/token_tests.rs`, the list of scanned files).
- Every control has a name and is at least `m.min_target()`; works at three size classes and text 3x; nothing outside `area`.
- A port refusal is drawn as a sentence. A consent or approval question is drawn by the view and answered through the port (never by the view itself).
- Put `pub mod x; pub mod xview;` (xview behind `#[cfg(feature = "uikit")]`) and `#[cfg(all(test, feature = "uikit"))] mod xview_tests;` in `crates/iyakki-ui-sdk/src/lib.rs`.

## Tests to write (all of them, in this order)

1. Facet fake and real implementation: scope and refusal rules, audit lines, limits (`iyakki-core-fake`, `iyakki-core-local`, agent adapter tests).
2. SDK port memory fake: same rules.
3. View tests `xview_tests.rs`: draws at three size classes and 3x text, names, bounds, targets, keys, refusal sentence, consent flow, `ACTIONS` ranges do not overlap another view's.
4. SAMPLE tests (`crates/iyakki-app-sample/src/*_tests.rs`): the pane works through `Host`.
5. Shell adapter test against `FakeCore` (the real-adapter test).
6. The chat tools: free read, asking set, off closes views.

## Docs and rows (the change is not done without them)

- Section "Embedding X" in `docs/development/UI-SDK.md` (the port, the facet, four lines of what the app does, where things run, testing, not built yet).
- `docs/sdk/embedding.md`: a recipe file `docs/sdk/recipes/embed_x.rs` (a 30-line app, compiled by `crates/iyakki-ui-sdk/tests/docs_recipes.rs`: add the `mod`, a `rules(...)` line and a driving test), and the `<!-- snippet: embed_x.rs -->` block. Add the module names to `PAGES` in `dist/sdk/gen-widget-docs.py` and run it.
- Core API surface: `IYAKKI_CORE_SURFACE_WRITE=1 cargo test -p iyakki-core-api --test core_surface` and read the diff.
- Feature rows (see the skill `iyakki-feature-rows`): one row each for facet, real implementation, view, SAMPLE pane, switch and chat tools; planned rows for what is not built (video, writes, the WASM tile route). Update the ceiling files.

## Checks before you hand over

`cargo test -p iyakki-ui-sdk` (incl. `docs_recipes`, `docs_fresh`), the crates you touched, `cargo test -p iyakki-core --test boundary --test dependency_ratchet --test ui_agent_uses --test feature_matrix --test kernel_single_reader`, the chat reachability gate in `iyakki-apps`, and `./run.sh app-check crates/iyakki-app-sample --no-tests`.

## Stop and ask

A new facet is the owner's decision when it reaches hardware, network, secrets or other people's data; so are the wording of its approval cards, the hard-ask list, retention and privacy rules, and any change to an existing facet's methods (additive only; a breaking change is a new version and a fixture folder). Write the plan (what the person sees, what the core refuses, what is logged) in the row notes and ask.
