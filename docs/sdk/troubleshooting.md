# Troubleshooting and FAQ

## A test failed

| Message | Meaning | Fix |
|---|---|---|
| `the first frame matches its golden files` : `the phone frame changed` | your drawing changed | if you meant it, delete `tests/golden` and run `cargo test`, read the diff, commit |
| `no_styling_numbers_in_the_drawing_code`: `line N: ...` | a number of 3.0 or more in drawing code | use `m.size(TypeRole::..)`, `m.gap(Space::..)`, `m.radius(Radius::..)`, `m.line(Line::..)` ([themes-skins-fonts.md](themes-skins-fonts.md)) |
| `a control with no name` | a `Canvas::hit` with an empty name | give it the visible label; icon-only buttons need a name too |
| `leaves the pane` | something is wider than the pane at some size or at 3x text | lay out from `canvas.size()` and `m.gutter()`; shorten text with `canvas::fit` |
| `is too small to touch` | a target under 44 high on touch | use the widgets (they enforce it) or `m.min_target()` |
| `contains rgb(` / `std::fs` / `iyakki_agent` | forbidden in app source | colours from `cx.colors()`; files, network and processes through a port |
| `a dependency besides the SDK` | `Cargo.toml` names another IYAKKI crate | remove it; the SDK re-exports `look`, `theme`, `wallpaper` |
| `the SDK docs are stale` | a doc comment or a recipe changed | `python3 dist/sdk/gen-widget-docs.py` and commit `docs/sdk` |

## Build and tool problems

- **`iyakki new-app` says `needs --sdk`**: you are outside the repository. Pass `--sdk <repo>/crates/iyakki-ui-sdk`. The generated `Cargo.toml` then holds that path (an absolute path on your machine; it is your project, not the repository).
- **The build runs on another machine and the SDK path is wrong there**: `--sdk` writes a relative path when the app is inside the SDK's checkout or beside it, an absolute path otherwise. Edit the `iyakki-ui-sdk = { path = ... }` line to a path that exists on the build host.
- **`app-check --cargo` fails on files that are not yours**: it runs cargo from the checkout root with `--manifest-path` of the app, so only the app's tests run; a failing `compile` row names the file. A standalone app is built in its own folder. See first-app.md, "If cargo runs on another machine".
- **Golden files exist only on the build host**: `./run.sh app-check APP --update-golden --cargo "WRAPPER DIR"` brings them back.
- **Layout of a project folder with its own repository next to a checkout**: keep the checkout (the folder with `crates/iyakki-ui-sdk`) and your project folder as siblings, for example `work/iyakki/` and `work/billing/` (a `.git` inside `billing/` is fine; the app crates are `billing/iyakki-app-x`). `iyakki new-app x --dir work/billing --sdk work/iyakki/crates/iyakki-ui-sdk` then writes `../../iyakki/crates/iyakki-ui-sdk` (relative; up to three folders below the checkout's parent). `app-check` takes the checkout from the nearest parent that holds `crates/iyakki-ui-sdk` (a project's own `.git` is not the checkout); when the project is not under the checkout say it: `app-check APP --root work/iyakki --cargo "WRAPPER work/iyakki"` (cargo then runs from the checkout root with `--manifest-path ../billing/iyakki-app-x/Cargo.toml`, so the wrapper must make both folders available there).
- **`cannot delete non-empty directory: .../crates/iyakki-gui` on stderr**: this is `rsync --delete` in the remote cargo wrapper, not `iyakki new-app`. A folder that is gone from your checkout still holds files on the build host that the wrapper never copies (its `target` folders are excluded), so rsync reports it and leaves it. `iyakki new-app` only creates a new folder and writes files into it: it has no code that deletes or renames anything and it refuses a folder that exists.
- **`app-check --update-golden` says tests/golden.rs has no dump mode**: you rewrote the golden test. The message prints the four lines to put at the top of the loop over frames (`name`, `got`); then the command works again.
- **Chat tool arguments**: only strings, whole numbers and booleans can be read (`Args::str`, `uint`, `bool`). A list is one string, `SKU:QTY,SKU:QTY` (recipe `tool_list_args.rs`).
- **The approval card is not there**: the card is drawn whenever a tool call waits, with no input focus needed. To send the first message the field must be focused (tap "Ask the assistant"); when a card arrives the field gives up the focus so `y` and `n` answer at once. Tap Allow or Deny, or press `y` or `n`.
- **`exists already; nothing was written`**: the command never overwrites; pick another name or folder.
- **The preview shows nothing**: open the exact address printed on the console; a request with another Host header is refused (DNS rebinding guard). A POST needs the `X-Iyakki-Preview: 1` header; the page sends it.
- **First build is slow**: a clean build of the workspace takes minutes; later builds are incremental. A standalone app project builds only the SDK and the toolkit.
- **`wasm32-unknown-unknown` is not installed**: `rustup target add wasm32-unknown-unknown`.
- **A tile loads on my computer but not on the device**: the device needs Developer mode, WASM tiles on; and the manifest `tile_caps` must list every domain the tile calls.

## Common mistakes

1. Drawing in `event` or changing state in `view`.
2. Two embedded views with overlapping action ranges: give each its own base (a range is `ACTIONS` numbers; a browser view `ACTION_SPAN`).
3. Handling Esc before offering it to the embedded view (the view closes its own text first).
4. Forgetting `wants_keyboard` on a touch screen: no on-screen keyboard appears.
5. Hiding a port refusal: show the sentence.
6. A switch with no chat tool.
7. Testing only at one size: the layout breaks at 360 wide or at 3x text.

## FAQ

**Can I use a crate from crates.io?** Prefer your own code; a dependency must be permissively licensed and, for an SDK app, the manifest guard allows `iyakki-ui-sdk` (and `iyakki-sdk-tile`) only among IYAKKI crates. Other crates need the owner's word.

**Can my app open a file?** Not itself. Use `FilesPort` (the person's home folder) or `StoragePort` (your own data).

**Can my app run a program?** Through `TerminalView` or a core tool, under the person's policy and with approval, never `std::process`.

**Why a name on every control?** The screen reader says it, a browser client exposes it as ARIA, and the tests find controls by it.

**Where are examples?** `crates/iyakki-app-sample` (notes, an assistant pane, files, terminal, editor, browser and sound panes), `docs/sdk/recipes/`, and `crates/iyakki-app-markdown`.

## Verified

Run on 2026-11-16 on a Linux build host through the project's remote cargo wrapper: `iyakki new-app hello --dir <folder> --sdk crates/iyakki-ui-sdk`, `cargo test` in the generated crate (5 tests pass), the generated tile app builds for wasm32 and runs in the real tile host (`cargo test -p iyakki-agent --lib newapp -- --include-ignored`), and `cargo test -p iyakki-ui-sdk --test docs_recipes --test docs_fresh`. Not run: `cargo run --example preview` in a real browser, `iyakki dev push` to a real device, `iyakki pkg ...`.
