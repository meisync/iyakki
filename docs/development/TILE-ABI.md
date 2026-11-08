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
* **Browser side (later job).** A thin page loads the same `.wasm` with the browser's WebAssembly engine, implements the imports in about 150
  lines of JavaScript (`measure_text` from the shared tables, `call` as a request to the connector, `random_u32` from the host-provided seed),
  decodes the display list (or receives the JSON form from a runner on the device) and draws it on a canvas; pointer and key events go back as
  event batches, hit ids come back as `Hit`. Whether the tile runs in the browser or on the device is then a deployment choice; with the OS in
  the cloud the connector carries `call`. Not built in this slice.

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
