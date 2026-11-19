# The IYAKKI UI SDK (alpha.4 design)

Owner decision 2026-11-15: provide a UI SDK that every app can use, so an app works on the device desktop, in a browser, in a mobile-sized view and in a desktop client. **Scope for alpha.4: new apps, plus the shared desktop parts: themes (Looks), docks, wallpapers, multiple screens and multiple views.** The existing apps are not rewritten in alpha.4; they move over later, one at a time.

Related: [SDK-API.md](SDK-API.md) (what an app may *ask the OS for*: the capability half), [LOOK-DESIGN.md](LOOK-DESIGN.md) (themes, icons, dock, bars), [CLIENT-SPLIT-DESIGN.md](CLIENT-SPLIT-DESIGN.md) (browser and client), [APP-DEVELOPMENT-PROCEDURE.md](APP-DEVELOPMENT-PROCEDURE.md) (ports, the core rule), [FAST-LOOP.md](FAST-LOOP.md).

## 1. What exists

| Piece | Where | State |
|---|---|---|
| Immediate-mode drawing kit: `Gfx`, widgets, theme, fonts | `iyakki-uikit` | built; draws pixels with tiny-skia |
| Tile facade (`App`, `TileCx`, source traits) for native tile crates | `iyakki-tile-api` | built; used by Calendar, Cameras, PDF, Radio, Remote files, Usage |
| WASM tile ABI with a **display list** (`Rect, RoundRect, Outline, Line, Text, Image, PushClip, PopClip, Hit`) | `iyakki-tile-abi`, `iyakki-tile-kit` | built; Chat, Edit, Runs, Settings tiles run in the web UI |
| Emulator that renders a tile to PNG and compares goldens | `iyakki-sdk-emulator` | built |
| Capability API for apps (JS bridge, local HTTP, MCP) | [SDK-API.md](SDK-API.md) | partly built |
| Desktop views (named views with their own panes) and multi-display settings | `shell_views`, `panes`, Settings -> Displays | first slices |

The gap: native apps draw straight into `Gfx` (pixels), WASM tiles produce a small display list, and the two share nothing. So a browser or client can only show streamed pictures of native apps.

## 2. The idea: an app describes a frame; each target renders it

```
app (native crate or WASM tile)
   | view(state, cx: UiCx) -> draws through the `Canvas` trait
   v
Canvas trait  ---- RasterCanvas  (Gfx, tiny-skia: device, desktop window)
              ---- ListCanvas    (records a DisplayList v2: browser, client, tests, WASM)
   events in: pointer, touch, wheel, key, text input (IME), focus, resize
   accessibility tree out (names, roles, bounds) from the same calls
```

1. **`Canvas` trait** in `iyakki-uikit` with the calls apps already use (rect, rounded rect, outline, line, gradient, shadow, text with face/size/measure, image by handle, path for squircles, clip, hit region with label). `Gfx` implements it unchanged; `ListCanvas` records it. Widgets (list, form, tabs, dialog, scroll bar, picker...) are written against `Canvas`, so they work on every target.
2. **DisplayList v2:** the current tile display list plus `Gradient`, `Shadow`, `Path`, `Blur` (cached backdrop), text runs with clustering (so a client can lay text out the same way), layers, and a version number. Wire format stays small, versioned and additive (compat rules apply: [CORE-COMPATIBILITY-DESIGN.md](CORE-COMPATIBILITY-DESIGN.md)).
3. **Renderers:** device and desktop window (raster, as now); **browser and desktop client** (a client renderer: the existing web UI tile renderer extended to v2, in JS or the client WASM engine; the client draws the list locally, so text is sharp and scrolling is local); **test** (PNG via the emulator for golden screenshots and the device UI tests).
4. **Input is one event model** (`Event`): pointer with id (multi-touch), wheel, keys with modifiers, text input and composition, focus, drag and drop, size and scale change. A mobile view is the same events with touch.
5. **Accessibility** comes from the same calls: every `hit` region has a name and a role; the SDK builds the tree for the screen reader (exists for native: reuse) and a client can expose it as ARIA.

## 3. Responsive layout (mobile view, multi-screen)

- **Size classes** instead of pixels: `Compact` (phone, small pane), `Medium`, `Expanded`; plus density and a minimum touch target (44 px) and the user's text size. The app's `view` asks `cx.size_class()` and `cx.metrics()`.
- **Layout helpers** in the SDK (stack, split, grid, wrap, scroll area, safe areas) so overflow cannot happen by construction; the existing overflow tests become the contract (every SDK widget is tested at 360x740, 800x480, 1280x720 and the largest text size).
- **Pointer and touch:** gestures (tap, long press, drag, swipe, pinch) are produced by the SDK from raw events; the on-screen keyboard (exists) opens when a text field takes focus.

## 4. Apps and the core

- An app is a crate with `App` + `view` using `UiCx`; it reaches the machine **only through ports** (the core rule): `UiCx` carries the ports it declared. Compiled **native** (a built-in, in the shell) or as a **WASM tile** (sandboxed, runs on the device, in a browser tab or in the client). The same source builds both; the WASM build reaches ports through host functions (the tile ABI).
- `iyakki new-app` makes the crate, the manifest, the port stubs and a golden test; `iyakki dev push` hot-reloads (exists for tiles); the headless harness and the device UI test runner test it.

## 5. Desktop parts the SDK also covers (alpha.4)

- **Looks / themes:** colours, icon shape/size/style, density: apps get them through `UiCx::look()`; no app hard-codes a colour (a lint test scans new apps).
- **Dock:** pinned apps, running dots, position, size; the SDK exposes `dock.pin/unpin/open` and an app can offer a badge and a short status line for its dock icon (the `status()` slice of the launcher plan).
- **Wallpaper:** per account and optionally **per screen**; the SDK offers a wallpaper picker widget and the contrast helper (text over wallpaper).
- **Multiple screens:** a `Screen` (output) has size, scale, orientation, position in the arrangement; each screen has its own dock/top-bar choice and wallpaper; windows/panes are placed per screen; hot-plug moves panes back safely (exists in Settings -> Displays as settings; the SDK adds the model the apps see: `cx.screen()`).
- **Multiple views:** a `View` is a named layout of panes (exists as desktop views); the SDK gives apps `cx.pane()` (its size class changes when the pane is resized or moved to another view/screen) and the layout manager (tiles, columns, floating) as a shared component with keyboard and touch gestures.

## 6. Versioning, tests, rules
- `UI_SDK_VERSION` and `DISPLAY_LIST_VERSION` rows in `compat-surfaces.txt`; golden fixtures for the wire format; additive changes only.
- Tests: a **reference app** rendered three ways (raster, list-to-PNG, list-through-the-client-renderer) must match within tolerance; widgets at all size classes; accessibility tree snapshot; no-hard-coded-colour lint; the core-rule ratchet (an SDK app's `UiCx` has no direct file/socket/device access).
- Honest status words in FEATURES.md rows: built / tested with fakes / seen on a real screen / seen in a browser / seen on a phone.

## 7. Slices (alpha.4)
1. **Proof slice (2 weeks):** `Canvas` trait + `ListCanvas` + DisplayList v2 (additive) + client renderer for v2 + one reference app (a small new app, e.g. "Notes pad" or a port of Calendar) drawn on the device, as PNG and in a browser; golden tests; compat rows.
2. **Widgets and responsive layout:** the shared widget set over `Canvas`, size classes, touch gestures; the model picker, tabs and lists as the first users.
3. **Looks, dock and wallpaper through the SDK:** `UiCx::look()`, dock API, wallpaper per screen.
4. **Screens and views:** the `Screen` and `View` models for apps; per-screen dock/wallpaper; pane placement.
5. **`iyakki new-app`, docs and samples** (UI-SDK.md for app authors).

## 8. Not in alpha.4
Rewriting the existing apps onto the SDK (one by one later); a phone app; GPU-accelerated rendering; third-party widgets from the store; remote apps over the network beyond the client we have.

## 9. Decided (owner, 2026-11-15)
1. Existing apps migrate onto the SDK slowly, in the next releases and later, one app at a time; none is forced in alpha.4.
2. The proposals below are accepted: client renderer in JavaScript (Canvas2D) first; WASM tiles use the same display list version (a newer-than-known op is refused or skipped by the written rule); one dock per screen, off by default on secondary screens.

## 10. Open questions
None open.
