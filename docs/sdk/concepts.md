# Concepts

Ten ideas explain the whole SDK. Each says what it is, the one rule that comes with it, and where it is in the code.

## 1. The core contract

IYAKKI has a **core** (the program that is PID 1 on a device) and **apps**. The core owns the machine: files, network, sound, camera, the browser engine, the person's settings, the permissions. An app owns its own screen and nothing else. The rule (owner, 2026-11-14): **everything goes through the core.** The app layer never touches Linux or a device. It calls the core's API; the core checks the permission, asks the person when something changes, writes the decision to the log, and only then acts. The contract is in `crates/iyakki-core-api` (the traits) and `docs/development/CORE-CONTRACT.md`.

For you that means: your app crate depends on `iyakki-ui-sdk` and nothing else from IYAKKI; no `std::fs`, `std::net`, `std::process`, no `/proc`, `/sys`, `/dev`, no `iyakki_agent::`. The generated tests scan for these and fail.

## 2. Ports

A **port** is a trait in the SDK that stands for one thing the core can do for the app: `StoragePort` (keep text under keys), `FilesPort`, `TerminalPort`, `MediaPort`, `BrowserPort`, `AssistantPort`, `DockPort`, `WallpaperPort`. The host hands the app a port; the app never builds one. On a device the port is backed by the core. In a test or the browser preview it is an in-memory fake (`MemStorage`, `MemFiles`, `MemTerminal`, `MemMedia`, `MemBrowser`, `SharedAssistant`) that follows the same rules.

Rule: take the port as a constructor argument (`Box<dyn FilesPort>`), so a test can give a fake.

## 3. Facets

A **facet** is the core's side of a port: a trait in `iyakki-core-api` (for example `Files`, `Terminal`, `Media`, `BrowserSessions`, `Assistant`) with default methods that say "this core has no such thing". A core implements the facets it has. `AgentCore` is the real core on a device; `iyakki-core-fake` has fakes of every facet for tests. A new embeddable component is a port in the SDK, a facet in the core API, a real implementation, a fake and a shell adapter; see `.claude/skills/iyakki-sdk-component`.

## 4. Approvals

When a call changes something (writes a file, runs a command, opens an address, plays a stream address), the core asks the person on their screen with a **card** that says what will happen. The person allows or denies. Some asks are "hard": every time, never covered by a session grant. The app does not draw the card for core actions: the embeddable views draw it (ChatView: Allow and Deny, also `y` and `n`; TerminalView: a question line; BrowserView: a prompt bar). For the app's own tools you declare `ask = true` in `ToolSpec::new(..., true)`.

## 5. Audit

Every decision (allowed, denied, by whom: `auto`, `user` or `system`) is written to the person's audit log by the core. The app does not write it and cannot skip it. A test of a facet checks that the audit line exists.

## 6. `UiApp`

```text
trait UiApp { fn id; fn title; fn event(&mut self, &UiEvent, &mut UiCx) -> Flow; fn view(&mut self, &mut dyn Canvas, &UiCx);
              fn dock_status (optional); fn wants_keyboard (optional) }
```

`event` changes state and answers `Flow::Stay` or `Flow::Back`. `view` draws the whole frame from the state; keep it cheap and keep state in the struct.

## 7. `UiCx` and `Host`

`UiCx` is what an app is handed on every call: the Look and colours (`cx.colors()`), the skin, the tokens, the size class and metrics (`cx.metrics()`), the screen, the pane, the view, the dock and wallpaper ports. Nothing in it reaches the machine.

`Host<A>` is the driver. It owns the app, turns raw pointers into gestures and taps into actions, resizes and moves the pane, and publishes the dock status. The shell, the browser preview and your tests use the same `Host`, so the app behaves the same everywhere.

## 8. Canvas and the display list

`Canvas` is the one drawing trait: `rect`, `rrect`, `outline`, `line`, `gradient`, `shadow`, `text`, `measure`, `clipped`, `hit`. Two canvases implement it: `RasterCanvas` (pixels, on the device) and `ListCanvas` (records a **display list**, plain versioned text that a browser, a client or a test reads). `host.record()` gives you the list; `list.texts()`, `list.a11y()` and `list.ops` let a test read what was drawn without a screen.

## 9. Action numbers

A clickable region (`Canvas::hit` or any widget) carries an `action: u32`. When the person taps it, clicks it or presses Enter on it, the app gets `UiEvent::Action(n)`. You choose the numbers. An embedded view takes a **range**: you give it a base, it uses `base .. base + ACTIONS` (`filesview::ACTIONS`, `chatview::ACTIONS`, ...; a browser view takes `browser::ACTION_SPAN`). Keep the ranges apart. `view.owns(a)` says whether a number is the view's.

## 10. Tick, key, draw

`UiEvent::Tick` comes about 30 times a second: poll your ports there (`files.tick()`, `chat.tick()`). `UiEvent::Key` and `UiEvent::Text` are for the focused field; offer them to an embedded view first and use the key yourself only when the view did not (`if !view.key(k) && k == Esc { return Flow::Back }`). `view` draws; it must be cheap and must not change state.

## What the SDK promises and does not

`UI_SDK_VERSION` is 1: additive changes only; a breaking change is a new number and a fixture folder. The display list is version 1 unless it holds a font run or a blur (version 2); a reader refuses a newer list by name.
