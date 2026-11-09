# SDK emulator: run a tile on your own computer

For a WASM tile (`iyakki-app.json` with a `tile` section, see `docs/development/TILE-ABI.md`). It runs the tile in the same sandbox the device uses,
against fake bridge services, with no device and no OS build. Code: `crates/iyakki-sdk-emulator` (library and command) and
`crates/iyakki-gui/src/sdk_emulate.rs` (the window).

## Quick start

```sh
cargo build --release -p iyakki-sdk-emulator                      # once; a small build, it does not build the OS
E=target/release/iyakki-sdk-emulator
$E check my-tile/                       # loads the tile; prints the text the install card would show; refuses a bad manifest
$E shot my-tile/ --out first.png --size 1024x600 --dark           # the first picture as a PNG
mkdir -p my-tile/tests/emulator
echo '{"steps":[{"shot":"start"},{"tap":[30,80]},{"expect_text":"1"},{"shot":"after-tap"}]}' > my-tile/tests/emulator/basic.json
$E test my-tile/ --update               # first run writes the golden pictures to tests/emulator/golden/
$E test my-tile/                        # later runs compare; exit code 1 on any difference (use it in CI)
cargo build --release --manifest-path crates/iyakki-gui/Cargo.toml --bin iyakki-sdk-emulate-window   # the window, once
$E emulate my-tile/ --size 800x480      # the window; save the tile file and it reloads
```

`iyakki sdk check|test|shot|emulate ...` runs the same program when it is next to `iyakki` or on PATH.

## The tile folder

* `iyakki-app.json`: `{"version": "0.1.0", "tile": {"abi": 1, "name": "counter", "caps": ["storage.get", "storage.put"]}}`
* `tile.wasm` or `tile.wat` (or the file named in `tile.file`).
* `iyakki-emulator.json` (optional, emulator only, never packed): settings for the fake services.

```json
{ "settings": {"language": "en"},
  "secrets":  {"api": "a-made-up-value"},
  "net":      {"https://example.com/data": {"status": 200, "body": "{\"x\": 1}"}},
  "deny":     ["camera", "net.fetch"] }
```

`deny` is what the emulated person answers "no" to when asked. A `.iyp` package is not run: it holds a signed container and no defined WASM payload;
emulate the folder you pack from. Web-bundle tiles (HTML and `window.iyakki`) are not covered, see the limits.

## Fake bridge services

A tile calls `call(domain, method, json)`. The emulator answers these; anything else is an error ("not emulated").

| Call | Answer |
|---|---|
| `clock.now` | a fixed start (2026-01-01T00:00:00Z) that moves only with `Tick` events, so runs repeat exactly |
| `settings.get {key}` | from `settings` in `iyakki-emulator.json` |
| `storage.get/put/delete` | in memory, 256 KB cap, gone when the emulator stops |
| `files.read/write/list {path}` | a scratch folder (`.emulator-scratch/` in the tile folder); plain relative paths only, no `..`, no links out |
| `vault.get {name}` | a value from `secrets`, else a made-up one; asks the (emulated) person every time |
| `net.fetch {url}` | only the canned answers in `net`; the emulator never touches the real network; asks first |
| `notify.send {title, body}` | recorded |
| `audio.*`, `camera.*` | `{"emulated": true, ...}`: nothing is played, recorded or photographed |
| `chat.ask` | a fixed reply |

## What the window shows

The tile at 800x480, 1024x600 or 1920x1080 (restarts the tile), touch (tap only) or mouse (pointer moves first), light or dark. The side panel lists:
the install card text, the grants the manifest holds, every prompt the tile triggered with the emulated answer, and every bridge call with its
arguments and reply. Anything under a key named secret, password, token, authorization, cookie or api_key, and every vault value, shows as
`[redacted]`. Refused calls are red.

## Packaging check

The manifest is the only list of permissions. If the tile calls something it did not declare, the call is refused exactly as on the device, and the
emulator also stops the tile with `REFUSED: the tile called vault.get, but its manifest does not declare it`; in `test` that fails the script.
A malformed capability, a missing `tile` section, or a different ABI version is refused at load with the device's sentence. The text printed by
`check` follows the install card's layout (`tile NAME VERSION (new)`, `permissions: ...`, `warning: ...`).

## Scripts and golden pictures

```json
{ "size": [800, 480], "mode": "touch", "dark": false, "tolerance": {"channel": 2, "percent": 0.1},
  "steps": [ {"tap": [30, 80]}, {"expect_text": "1"}, {"shot": "one"} ] }
```

Steps: `tap [x,y]`, `move [x,y]`, `text "abc"`, `key CODE` (with `mods`), `tick MS`, `hit ID`, `resize [w,h]`, `theme "dark"|"light"`, `shot NAME`,
`expect_text`, `expect_no_text`, `expect_call "domain.method"`, `expect_prompt "domain.method"`, `expect_refused "domain.method"`.
Golden pictures are `tests/emulator/golden/<script>--<shot>.png`, written by `--update`. `tolerance.channel` is how far one colour channel may differ,
`tolerance.percent` how many percent of the pixels may differ by more. Each shot prints its SHA-256 (first 12 digits). A failed shot keeps the
actual picture in `golden/_actual/`. The reference picture format is 8-bit RGBA PNG written by the emulator; a PNG saved by another program is
refused with a message to run `--update`.

## Limits (what it cannot show)

* No real hardware: no sound (audio calls are stubs), no camera, no microphone, no GPU, no Bluetooth, no sensors.
* No real network: only canned answers. Real servers, certificates, timeouts and proxies are not tried.
* Timing differs: the clock moves only by ticks; the window sends a tick about every 33 ms; compute budgets (fuel, 100 ms wall limit) are the
  device's numbers but this computer is faster or slower.
* Text is drawn with the reference 5x7 bitmap font (upper case look), not the device's fonts, so text widths in the picture differ from the device;
  `measure_text` inside the tile uses the ABI's built-in metrics, the same everywhere.
* Prompts are answered by the settings file, not by a person; the real approval card, autonomy settings and the assistant are not part of it.
* The wording of capabilities is a copy kept in the emulator; it matches the install card's words for storage, files and notifications, and
  may drift for the others until the card itself is shared.
* Only WASM tiles. Web-bundle tiles run in a browser (`tile_test`, `iyakki dev push --watch`); Rust shell tiles use `TileHarness`. A passing run
  here is not a device test.
