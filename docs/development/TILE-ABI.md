# Tile ABI, version 1

The interface between a **tile** (a WASM guest: an app people develop on IYAKKI) and its **host** (the device shell, or a browser page that
talks to the OS on a device or in the cloud). Context: `COMPILE-SPEED-PLAN.md` ("Two audiences, one idea", Phase 4 option 3) and
`UI-OS-SPLIT-PLAN.md`. This is the first slice: the contract, the codec (`crates/iyakki-tile-abi`), a host runner and reference renderer
(`crates/iyakki-tile-host`) and an example tile (`crates/iyakki-tile-example`). It is **not wired into `iyakki-shell`** yet.

The idea in one line: **events in, display list out, services through capability-scoped JSON calls.** The tile owns its state and never draws
pixels or touches the system; the host draws the list and answers the calls. Because the guest is deterministic, the same `.wasm` renders the same
picture on the framebuffer shell and in a browser.

## 1. Files, exports and imports

A tile is one `.wasm` (or `.wat` in development) plus a `tile` section in `iyakki-app.json`:

```json
{ "tile": { "abi": 1, "name": "counter", "caps": ["storage.get", "storage.put", "chat.*"] } }
```

`caps` entries are `"domain"` (every method), `"domain.*"` (same) or `"domain.method"`. Anything not listed is refused at call time with `DENIED`.
`iyakki pkg validate` states them in plain words, like other permissions.

### Guest exports (all required; names exact)

| Export | Signature | Meaning |
|---|---|---|
| `memory` | linear memory | the only memory; the host reads and writes buffers in it |
| `iyakki_tile_abi` | `() -> i32` | the ABI version the tile was built for (`1`) |
| `iyakki_alloc` | `(size: i32) -> i32` | returns a pointer to `size` writable bytes, or 0; the host fills it with an event batch. The tile may recycle its input heap at the end of `iyakki_tile_init` and `iyakki_tile_event` |
| `iyakki_tile_init` | `(abi: i32, ptr: i32, len: i32) -> i32` | called once; the buffer is an event batch holding at least `Resize` and `Theme`. Returns 0, or a non-zero tile error code |
| `iyakki_tile_event` | `(ptr: i32, len: i32) -> i32` | an event batch. Returns `1` (redraw wanted), `0` (nothing changed) or negative (tile error) |
| `iyakki_tile_frame` | `() -> i32` | returns a pointer to `[u32 LE length][display list bytes]` in guest memory; the host copies it out before the next call |

### Host imports (module `iyakki_tile`; every pointer pair is `(ptr, len)` into guest memory)

| Import | Signature | Meaning |
|---|---|---|
| `measure_text` | `(face, size, ptr, len) -> i32` | width in pixels of the UTF-8 text, or a negative errno |
| `call` | `(dom_p, dom_l, meth_p, meth_l, json_p, json_l, out_p, out_cap) -> i32` | service call, section 5. Returns the reply length written at `out_p`, or a negative errno |
| `random_u32` | `() -> i32` | the only randomness; seeded by the host |
| `iyakki.log` | `(ptr, len)` | existing `iyakki-wasm` log import, capped at 1 MiB |

Negative errnos (from `iyakki-wasm`): -1 bad memory, -2 denied, -4 service error, -5 buffer too small, -8 too many host calls in one step.
A tile that imports anything else fails to load.

## 2. Versioning

`ABI_VERSION` is a single integer (now 1). It must match **exactly**; there is no negotiation in version 1.

* The host checks `iyakki_tile_abi()` and the manifest's `tile.abi`, and every encoded buffer carries `magic + u16 version`.
* On a mismatch the host **refuses to load the tile** and shows one plain sentence to the person, for example: "This app was built for tile
  interface version 2, but this system speaks version 1. Update the app or the system." (`TileError::VersionMismatch`). It never tries to run it.
* A change that old tiles can still read (a new optional import) does not bump the number; any change to an encoding, a tag, an export signature
  or a meaning does. Tags are never reused. When version 2 is needed, the host may carry both decoders for one release.

## 3. Encodings (all little-endian, no padding)

* **varint**: LEB128, at most 5 bytes (a 5th byte above 0x0F is an error). **zigzag** for signed values: `(v << 1) ^ (v >> 31)`.
* **colour**: 4 bytes `r g b a`, straight (not premultiplied) alpha.
* **string**: varint byte length (at most 1024), then UTF-8.
* **rect**: `zigzag x, zigzag y, varint w, varint h` in logical pixels. Coordinates are integers; there is no floating point on the wire, so the
  decoder gives the same numbers everywhere.

Header of both buffers: 4 magic bytes (`IYDL` display list, `IYEV` event batch), `u16` ABI version, varint item count.

### Events (host -> tile), tag byte then payload

| Tag | Event | Payload |
|---|---|---|
| 1 | PointerDown | zz x, zz y, u8 button |
| 2 | PointerUp | zz x, zz y, u8 button |
| 3 | PointerMove | zz x, zz y |
| 4 | Key | var code, u8 mods (1 shift, 2 ctrl, 4 alt, 8 meta), u8 down |
| 5 | TextInput | string (already composed by the host) |
| 6 | Tick | var dt_ms: the only clock |
| 7 | Resize | var w, var h, var scale_pct (200 = 2x) |
| 8 | Theme | u8 dark, colour accent, colour background, colour foreground |
| 9 | Hit | var id: a pointer release landed on the topmost hit region with this id |

Key codes: ASCII for printable keys, 8 backspace, 9 tab, 13 enter, 27 escape, 0x100.. for arrows (0x100 left, 0x101 right, 0x102 up, 0x103 down),
home/end/page keys and function keys in a table fixed in version 1 (to be finalised with the shell's key map before wiring).
The host sends `Hit` after the `PointerUp` it belongs to, using the hit regions of the **last frame the tile returned**
(`TileRunner::click`).

### Display list (tile -> host), tag byte then payload

| Tag | Op | Payload |
|---|---|---|
| 1 | Rect | rect, colour |
| 2 | RoundRect | rect, var radius, colour |
| 3 | Outline | rect, var thickness, colour (drawn inside the rect) |
| 4 | Line | zz x1, y1, x2, y2, var thickness, colour |
| 5 | Text | zz x, zz y, u8 face, var size, colour, string (x, y = top-left of the text box) |
| 6 | Image | var handle, rect (the host knows images by handle; unknown handle draws nothing in production, a grey box in the reference renderer) |
| 7 | PushClip | rect (intersected with the current clip) |
| 8 | PopClip | |
| 9 | Hit | var id, rect (invisible; topmost in draw order wins) |

Faces: 0 UI sans, 1 monospace, 2 bold. Painting order is list order; clip and hit regions follow the same stack discipline.

**Checks the host makes on every list, before anything is drawn:** valid magic and version; at most 8192 ops, 256 KiB, 1024-byte texts,
text size 1..512; every coordinate and size within +-32768; clip pushes and pops balanced with depth at most 16; no trailing bytes; and the
**surface check**: every rectangle (and the measured text box) lies within one surface size beyond each edge of the surface (room for scrolled
content, not for nonsense). A list that fails any check is **refused whole** (no partial drawing) and the person sees "The app returned a bad
picture" or "tried to draw far outside its window".

## 4. Memory layout and calling convention

* The guest has one linear memory, exported as `memory`, at most 16 MiB by default (`Budgets::max_memory`); growth beyond that fails inside
  the guest (`memory.grow` returns -1).
* Host to guest: the host calls `iyakki_alloc(n)`, writes the encoded batch at the returned pointer (a pointer outside memory, or 0, is an
  error), then calls `iyakki_tile_event(ptr, n)`. The tile reads the buffer during that call only.
* Guest to host: `iyakki_tile_frame()` returns a pointer; the host reads 4 bytes (length, at most 256 KiB) and then the bytes. A pointer or length
  that leaves guest memory is refused (`BadPointer`), never trusted.
* All arguments and results are `i32` (the `iyakki-wasm` call path); a pointer is a `u32` offset.
* Frames are pulled: the host asks for a frame when `iyakki_tile_event` returned 1, on a tick it chooses, or when it needs to repaint. A tile
  must return the same list for the same state (pure function of its state).

## 5. Services: `call(domain, method, json) -> json`

A tile reaches IYAKKI services (storage, chat, settings, models, network through the proxy, ...) only through `call`. The host:
1. checks the manifest capability (`domain`, `domain.*` or `domain.method`); otherwise returns -2 without calling anything;
2. limits the request to 64 KiB and counts the call against the per-step limit (256);
3. forwards to the `ServiceBackend` (on the device, the same `iyakki-core-api` the native tiles use; in the cloud, the connector), which
   sees the tile's name for its audit trail and its approvals ("asks first" actions still ask the person);
4. writes the reply JSON at `out_p` if it fits in `out_cap` (else -5; the tile may retry with a bigger buffer).

Requests and replies are UTF-8 JSON; errors are the negative return codes (the reason goes to the host log, not into the tile). Calls are
synchronous and short; long work uses a service method that returns a job id and is polled from `Tick`. Because every reply is recorded by the
host (like `iyakki-ledger` records tool outputs), a session can be replayed exactly.

## 6. Budgets

Per call of `iyakki_tile_init`, `iyakki_tile_event` or `iyakki_tile_frame` (`Budgets`, defaults in brackets):

| Budget | Default | On exceeding |
|---|---|---|
| Fuel (instructions): init / event / frame | 20 M / 5 M / 10 M | the call is stopped, "The app was stopped: it used too much computing time for one step" |
| Wall clock (host watchdog, epoch based) | 100 ms | same, "too much real time" |
| Linear memory | 16 MiB | `memory.grow` fails inside the guest |
| Host calls (`call`, `measure_text`) | 256 per step | the call returns -8 |
| Display list | 8192 ops, 256 KiB | refused |
| Event batch | 256 events, 64 KiB | host splits the batch |

Fuel is refilled for every call. The hard stop is the largest fuel value; each call kind is then also compared with its own budget. A stopped
tile is not restarted automatically: the shell shows the message and a **Reload** button (state is lost, which is the same as a hot reload).

## 7. Determinism

A tile's output is a function of its wasm, the bytes of every event, and every host-call reply. Therefore:

* no wall clock: time is `Tick.dt_ms` only; the module may not import WASI or any clock;
* no host randomness except `random_u32` (seeded per session by the host and recorded);
* no threads, no floating-point non-determinism that matters: wasm float arithmetic is deterministic except NaN payloads, and the wire format
  carries integers only;
* the host never reads host memory into the guest except through the calls above.

This lets a test (and a bug report) replay a recorded event stream and compare the display lists byte for byte.

## 8. Text measurement

Layout is the tile's job, so it needs exact widths: it calls `measure_text(face, size, text)` and positions runs itself. The host's answer
must be the same on the device and in the browser, so both use the **same metrics tables** (advances per face and size; kerning is not used in
version 1) shipped with the shell and the browser page. `line_height(size)` is defined by the ABI as `size + size / 4` (integer division).
The built-in reference font in `iyakki-tile-abi` (advance `6 * size / 7`, at least 1) is the table the tests use; production replaces the
tables, not the contract. A tile must never assume a width. Exporters put each run's measured `width` and `height` next to the text, and the SVG
export sets `textLength`, so a browser lays the text out like the device even with another font installed.

## 9. Reference renderer, exporters and the browser

* `iyakki_tile_host::render::render(list, w, h, bg)` draws a list into an RGBA buffer using only ABI types and a built-in 5x7 bitmap font
  (upper case; lower case draws as upper case). It exists so tests can assert pixels. The production device renderer is `iyakki-ui`
  drawing the same list.
* `iyakki_tile_host::export::to_json` writes `{"abi":1,"width":W,"height":H,"ops":[...]}` with colours as `#rrggbbaa`; `to_svg` writes a
  standalone SVG (clip paths, `data-hit-id` rectangles, `iyakki-image:HANDLE` image references).
* **Browser side.** Built as "the tile stays on the server, the browser draws": see section 12. (The other design, the browser loading the
  `.wasm` with its own WebAssembly engine, is not built; the display-list path below replaced it for the first slice because it keeps the
  tile, its state and its permissions in one place.)

## 10. Push and hot reload (target, not built)

`dev push --watch` sends the new `.wasm` (kilobytes) and the host swaps the module: build a new runner, call `init` with the current size and
theme, replay the recorded event stream if the tile asks for state restore (`iyakki_tile_state` export, reserved, not in version 1), and
switch frames. No image, no restart. The cold compile of the `wasmtime` engine is paid once per shell start; a new module compiles in
milliseconds for kilobyte-size tiles.

## 11. What is verified in this slice

See the test list in the delivering summary and `FEATURES.md` rows: codec roundtrip and hostile input, runner behaviour (version
refusal, fuel and wall budgets, memory cap, capability scoping, determinism, out-of-bounds refusal), the example tile through the real
sandbox, and the renderer and exporters. **Not verified:** a Rust-built wasm32 tile (the `wasm32-unknown-unknown` target is not installed
here; the example is hand-written WAT plus a native Rust twin that must draw the identical list), browser-side rendering, wiring into
`iyakki-shell`, real fonts, images, state restore on reload.

## 12. In a browser (built; Q18, F-WASM-10, F-WEB-04)

Idea: the OS (and with it the tile) stays on the machine that serves the web UI (a desktop, or the cloud pod); the browser only draws and
forwards input. Because the tile is deterministic and the browser draws the same display list with the same integer algorithm and the same
text metrics, the tile looks the same as on the framebuffer shell.

```
browser (ui/tiles.js + ui/tilerender.js)             web service (webservice/tiles.rs)
  pointer / key / text / wheel / resize   --JSON-->   POST /api/v1/tiles/<id>/events
  canvas <-- tilerender.js <-- display list JSON <--  iyakki-tile-host sandbox (fuel, time, memory) -> display list
                                                      service calls -> the person's tools (profile list, safe only, audited)
```

**Routes** (bearer token and origin check exactly like every other route; listed in the OpenAPI document):

* `GET /api/v1/tiles` -> `{"tiles":[{"id","title","abi","width","height"}]}`: the tiles this person may open.
* `POST /api/v1/tiles/<id>/events`, body at most 64 KiB, JSON `{"width","height","scale_pct","dark","reset","events":[...]}`. `width` and `height`
  (1..4096) are used when the tile has to start (the first call, `reset: true`, or after the server dropped an idle one); `events` is at most 128
  events, each an object with a `type`: `pointer_down|pointer_up {x,y,button}`, `pointer_move {x,y}`, `key {code,mods,down}`, `text {text}`,
  `tick {dt_ms}`, `resize {w,h,scale_pct}`, `theme {dark,accent,background,foreground}`. Every number is a whole number inside the ABI's range
  (a float, a string, a huge value is refused with 400 before anything runs). `Hit` is never accepted from the page: on a `pointer_up` the host
  adds the `Hit` for the topmost hit region of the last picture, as `TileRunner::click` does on the device.
* Reply `{"id","redraw","frame":{"abi","width","height","ops":[...]},"stats":{"fuel","bytes"}}`: `frame` is `export::to_json`, always the
  current picture. A tile that is refused (wrong interface version, not a tile) answers 501, one that was stopped or crashed (fuel, time,
  bad picture, trap) answers 502, each with the plain sentence of `TileError`; the crashed tile's state is dropped (the page offers Reload).
  Other refusals: 400 bad JSON or event, 401, 403 (origin), 404 (no such tile), 413 (body or event count), 429 (`busy` when too many tiles
  are open on the machine, `rate_limited`).

**Per-person state.** One running tile per (person, tenant volume, tile), at most 32 on a machine, dropped after 30 idle minutes. A second
person gets their own copy; nothing in a request names a person, the token does. The route has its own limit of 600 requests a minute per person
instead of the general per-person limit (a page that follows the pointer would never fit in 60). The service's per-address limit (600 a minute
from one address) still applies, so many people behind one address share it.

**Authority.** A browser session adds none. The tile manifest (`caps`) says what the tile may ask for; the only service domain in this slice is
`tools`: `call("tools", <tool name>, <args as JSON>)` with `tools.<name>` (or `tools.*`) in the manifest. What passes the manifest is then run
as the person's own tool, if the service profile lists it, if it is a tool that does not ask first, and if the tool does not refuse it. A tool
that would ask is **not run** from a tile and the tile is told "no" (the browser has no approval prompt for tile calls yet); that is a limit,
not a bypass. Every call, refusal and denial is an audit line `tile.call`, `tile.refused` or `tile.denied` with the tile and tool name
(never the arguments or the reply).

**The renderer** (`ui/tilerender.js`, no library) is a line-by-line port of `iyakki-tile-host/src/render.rs` into integer JavaScript: rectangles,
rounded rectangles, outlines, lines, clips, alpha, text in the same 5x7 font and the ABI's advance (`6 * size / 7`), images as grey boxes.
It draws into an RGBA buffer and puts that on the canvas, enlarged by the whole-number device pixel ratio with nearest-neighbour so a dense
screen stays crisp. The browser's own fonts are not used (that is what keeps the metrics identical); real font tables replace the 5x7 table on
the device and in the page together, later. Faces are drawn alike for now (the reference renderer does not tell them apart).

**The page** (`ui/tiles.js`, entry "Tiles"): Pointer Events for mouse, touch and pen (a move is sent only while pressed, only the newest);
the keyboard to a hidden text box so input methods work (text is sent when a composition ends, nothing while it runs); the wheel as the
Up/Down arrow keys (version 1 has no wheel event); the surface follows the page width and the device pixel ratio (`scale_pct`); one request
in flight, events meanwhile are merged and sent together; a lost connection retries after 1, 2, 4, 8, 15 s with the same events; a refused or
crashed tile shows the service's sentence and a Reload button (`reset: true`).

**Chat.** `tiles_list` (safe, no approval) names the tiles and where to open them.

**Verified:** the endpoint through the pipeline and over TLS on a loopback port (events to display list for the example tile, equal to the Rust
twin's list), budgets (a spinning tile is stopped), refusals (wrong version, junk module), 40 hostile-JSON cases, sign-in and origin, per-person
isolation, service calls scoped by manifest then by the person's tools, caps and rate limits; the JS renderer against the Rust reference renderer
(pixel equality, tolerance 0, seven pictures incl. every printable ASCII character) and the page controller, both under node.
**Not verified:** a real browser (a headless Firefox screenshot comparison is written but was skipped here: the only Firefox is an Ubuntu snap wrapper; see
the delivering summary for whether it did), latency (not measured), touch on a real touch screen, input methods in a real browser, tiles
with images (the grey box only), approvals for tile calls, an installed Rust-built tile (only the WAT example exists).
