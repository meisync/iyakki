---
name: iyakki-app-development
description: Create, test and review an IYAKKI app written against the UI SDK (crate iyakki-ui-sdk): scaffold with `iyakki new-app`, build the screens from widgets and tokens, embed Files, Chat, Browser, Terminal, Editor, Audio or Camera views, keep data through ports, add a chat tool for every switch, and check the app with `app-check` before handing it over. Use whenever someone asks for a new IYAKKI app, a pane or a screen in an SDK app, a review of an SDK app, or a fix of an app-check or SDK lint failure.
---

# IYAKKI app development (UI SDK)

You write a small Rust app against ONE crate, `iyakki-ui-sdk`. The same source draws on the device, in a window, in a browser tab and as a WASM tile. The app reaches the machine only through ports the core hands it. This skill is the method; the guide is `docs/sdk/README.md`, the examples that compile are `docs/sdk/recipes/*.rs`, the exact API is `docs/sdk/reference/*.md` (generated, always true). Read a recipe before you write the same thing from memory.

## Golden rules (follow them line by line; each is checked by `app-check`)

1. The app crate depends on `iyakki-ui-sdk` and nothing else from IYAKKI (`iyakki-sdk-tile` too, for a tile). No other IYAKKI crate, ever.
2. Never write `iyakki_agent::`, `iyakki_shell`, `iyakki_uikit`, `std::fs`, `std::net`, `std::process`, or a path under `/proc`, `/sys`, `/dev`. Need a file, a command, an address, a sound, a camera? Use a port the host hands in (`FilesPort`, `TerminalPort`, `BrowserPort`, `MediaPort`, `CameraPort`, `StoragePort`, `AssistantPort`).
3. Draw only through `Canvas`. Take colours only from `cx.colors()`: no `rgb(`, no `[r, g, b]` arrays.
4. No styling numbers: a decimal number of 3.0 or more in drawing code is wrong. Use `m.size(TypeRole::Body)`, `m.gap(Space::Md)`, `m.radius(Radius::Md)`, `m.line(Line::Normal)`, `m.min_target()`, `m.gutter()` (`let m = cx.metrics();`).
5. Every control has a name (the visible label) and a role. Use `widgets::button/row/field`, or `Canvas::hit` with a non-empty `Hit.name`.
6. Every target is at least `m.min_target()` (44 on touch). Nothing leaves the pane at 360x740, 800x480 and 1280x720, at text 1x and 3x. Shorten long text with `canvas::fit`.
7. Lay out from `c.size()` and `cx.size_class()` (Compact is one column). Never from a pixel constant.
8. `view` only draws. State lives in the struct. `event` changes state. Poll ports in `UiEvent::Tick`.
9. A port can refuse. Show its sentence to the person; never swallow it.
10. A tool that changes data or the machine is declared with `ask = true`. A read is `ask = false`.
11. EVERY switch, setting or action a person can use on screen also has a chat tool (a read tool and an asking change tool) that calls the same function as the screen. No control without a tool.
12. Never invent the wording of an approval card, a consent, or a privacy rule; they come from the core. Never type or log a secret.
13. No personal names, emails, home paths, IPs or secrets in code, tests or docs.
14. Run `./run.sh app-check PATH` before you say you are done, and report its output.

## Workflow

1. **Scaffold.** Outside the repository: `iyakki new-app NAME --dir DIR --sdk REPO/crates/iyakki-ui-sdk`. Inside: `iyakki new-app NAME` (then add the workspace member, the line `iyakki-app-NAME apps` in `dist/scripts/crate-layers.txt`, a row in `FEATURES.md`). Add `--tile` for a WASM tile. (`iyakki` is `cargo run -p iyakki-core --bin iyakki --`.) NAME: lower case letters, digits, single dashes; not `sample settings store chat home shell core system`.
2. **Read** `src/lib.rs` (a counter: `UiApp`, `event`, `view`), `tests/golden.rs`, `README.md`.
2b. The generated `tests/golden.rs` is written for the STARTER counter (tap More, Enter, Backspace, `with_storage`, `count()`): when you change the app, rewrite that test for your behaviour, keep a `with_storage(Box<dyn StoragePort>)` constructor or edit the line that calls it, and add your own test file `tests/NAME.rs` (see `references/testing-checklist.md`; a worked example is `docs/sdk/dry-run.md`).
3. **Plan the screens** as panes (see `references/templates.md`). Decide the state struct, the action numbers (`const A_X: u32 = n;`), the ports (`app.toml` `ports = [...]`).
4. **Build one pane at a time** from a recipe (`references/recipes.md` says which file to copy). After each pane: `./run.sh app-check PATH --no-tests` (milliseconds), then with tests.
5. **Embed components** with their recipe: give each view its own action-number base (ranges must not overlap: Files 100, Chat 200, Terminal 300, Editor 400, Audio 500, Camera 600, Browser 10000 and up).
6. **Persist** through `StoragePort` (key `count`, `notes`, ...). Keep values small (64 KB each, 256 KB total).
7. **Chat tools** for each switch/setting: see `references/chat-tools.md`.
8. **Test** like `references/testing-checklist.md`: golden files, three sizes, text 3x, taps and keys through `Host`, port refusals, tool asks before it acts.
9. **Check**: `./run.sh app-check PATH`. Fix the numbered list from the top. Run it again until it prints `no problems`.
10. **Hand over**: say what you ran and its output; list what you did not run (a real device, a browser, a real model). Do not claim more.

## Commands (copy exactly)

| What | Command |
|---|---|
| make an app | `iyakki new-app NAME --dir DIR --sdk REPO/crates/iyakki-ui-sdk` |
| test | `cd APP && cargo test` (first run writes `tests/golden`: read and commit) |
| see it in a browser tab | `cd APP && cargo run --example preview` (prints an address on 127.0.0.1) |
| check everything an author can hit | `./run.sh app-check APP` (add `--no-tests` for the instant part, `--contract` inside the repo) |
| regenerate the SDK reference | `python3 dist/sdk/gen-widget-docs.py` |
| WASM tile | `cargo build --release --lib --target wasm32-unknown-unknown` |

On a machine that builds through a remote wrapper, use `app-check PATH --cargo "WRAPPER DIR"` (it runs only the app's tests, from the checkout root with `--manifest-path`; no shim). Golden files written on the build host come back with `--update-golden`. If the SDK path in Cargo.toml is absolute and the build host lacks it, edit that one line.

## Where to find examples

- `docs/sdk/recipes/*.rs`: one compiling file per pattern (tabs, list, form, storage, themed drawing, approval card, and an embedding of each component). Start here.
- `crates/iyakki-app-sample`: a full app (notes, assistant pane, files, terminal, editor, browser, sound, camera).
- `crates/iyakki-ui-sdk/tests/docs_recipes.rs`: how to test each recipe.
- `docs/sdk/reference/*.md`: every public item with its doc comment; search it before you guess a name.

## Common mistakes (what the lints catch)

| Mistake | `app-check` rule | Fix |
|---|---|---|
| `let gap = 8.0;`, `c.text(.., 14.0, ..)` | `styling-number` | `m.gap(Space::Md)`, `m.size(TypeRole::Body)` |
| `Color`/`rgb(..)` or `[12, 34, 56, 255]` | `colour-literal` | `cx.colors()` (`th.text`, `th.accent`, ...) |
| `std::fs::read_to_string(p)` | `machine-access` | `FilesPort::read_text` |
| `Hit { name: "".into(), .. }` | `unnamed-control` | the visible label |
| a button 30 high | `target-size` (test) | `m.min_target()` or a widget |
| `/ 3.0` for three tabs | `styling-number` | `const TABS: f32 = 3.0;` then `/ TABS` |
| a barcode table written from memory | wrong bars | `iyakki_ui_sdk::barcode` (`encode_code128`, `encode_ean13`, `encode_upca`) and `BarcodeView` |
| a list as a tool argument (array) | argument unreadable | one string `SKU:QTY,SKU:QTY` (`tool_list_args.rs`) |
| a rect `Rect::new(g, y, 400.0, ..)` | `bounds` (test) | `w - 2.0 * g` |
| `iyakki-agent` in Cargo.toml | `manifest-dependency` | delete it; use a port |
| `ToolSpec::new("delete_x", .., false)` | `tool-should-ask` | `true` |
| two views with the same action base | tests act on the wrong control | separate bases |

## Events (online and offline)

An app that needs to react to the machine (network, power, a folder, a paired computer, another app) registers for named events through the `EventsPort` and handles them with an `EventRouter` (`docs/sdk/events.md`, recipe `docs/sdk/recipes/events_react.rs`, catalogue in `docs/sdk/events.md`).

1. Add `"events"` to the `ports` of `app.toml`; list the scopes you need (`events.network`, ...) in the `capabilities` of the manifest with a reason the person reads.
2. `EventRouter::new().on("name", handler).on_missed(handler)`, then `register_all(port)` (show the refusal sentences), then `begin(port, actions)` before the first frame, then `pump` on every tick, and `port.stop()` when the app goes away.
3. Handlers act through `EventActions` (`notify`, `start_flow`, `call_tool`); never ring or start something for an event with `e.late == true`; on `missed`, read the state again.
4. Test with `MemEvents` (`allow`, `happen`, `set_time`): ordering, a late event, a missed count, a refusal. `app-check` fails an app that registers and handles neither `missed` nor `late` (rules `events-missed`, `events-late`).
5. A payload has no secrets and your app publishes only `custom.<your app>.<name>` (scope `events.publish`).

## Review checklist and when to stop

For a review use `references/review-checklist.md` (numbered, one yes/no per line; `docs/sdk/conformance-checklist.md` is the long form).

**Stop and ask (escalate; do not improvise)** when: you need a new core facet or a port the SDK does not have; a contract test fails in a crate that is not your app (`boundary`, `dependency_ratchet`, `ui_agent_uses`, `feature_matrix`, `kernel_single_reader`); the wording of an approval card, a consent, a privacy or retention rule is needed; a layout cannot hold at 3x text without removing a feature; you want a dependency; you are about to edit the SDK itself, the shell adapters or the core. In the report say what you need and why, and what you did instead.

## References

- `references/templates.md` pane skeletons; `references/recipes.md` which recipe for which job; `references/chat-tools.md` the tool-for-every-switch pattern; `references/testing-checklist.md`; `references/review-checklist.md`; `references/failures.md` every lint and contract failure with its fix.
- For the OS itself (not an app): the skill `iyakki-os-development`. To add a new embeddable component: `iyakki-sdk-component`. For matrix rows: `iyakki-feature-rows`.
