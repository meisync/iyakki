# Testing a tile

Short answer: **a new tile is a web-bundle tile**. Make a Rust tile only when it must draw with `Gfx` (the framebuffer shell's own screens,
the browser, the editor, the player). Background: `docs/development/UI-OS-SPLIT-PLAN.md`.

## Web-bundle tile (the default)

A folder with a manifest, HTML/JS/CSS and `tests/*.js`. It reaches the system only through `window.iyakki` (`store/tilesdk.rs`); every backend there is a
trait with a fake, so nothing in a test touches the machine.

1. Scaffold: `app_scaffold` / `tile_scaffold` from the chat assistant or the IDE (see `docs/APP-AUTHORING.md`).
2. Test headless on the real engine: `tile_test {path}` (IDE: Tests); `tests/*.js` run without a screen.
3. Iterate on a device: `iyakki dev push --watch <folder>` (developer mode, `docs/DEVELOPER-MODE.md`); no rebuild, no image.

No Rust compile, no shell test binary; seconds, not minutes.

## Rust tile (only where `Gfx` is needed)

Tests live next to the tile (`apps/<tile>.rs` `mod tests`, or `apps/<tile>_tests.rs`). Use `apps::tile_harness::TileHarness` (test builds only):

```rust
use crate::apps::tests::cx_with_admin_store;
use crate::apps::tile_harness::TileHarness;

let cx = cx_with_admin_store("my-tile");          // an admin session, a store, fake host; no framebuffer
let mut t = TileHarness::new(MyApp::new(&cx), cx.clone());
t.type_text("hello");                              // keys into the focused field
t.key(Key::Enter);                                 // any key
t.click_label("Save");                             // sends the event the button registered; panics listing the labels that exist
assert!(t.shows("Saved"));                         // text drawn in the last picture (Gfx::text_log)
t.assert_no_overlap_at_standard_sizes();           // 1280x720, 1024x600, 800x480, 480x600: no two controls partly overlap
t.draw_at_standard_sizes();                        // every size draws without panicking
let app = &mut t.app;                              // the tile itself, for state asserts
```

Rules of thumb:

- Click by label, not by `Event::Tab(310)`; labels are what the person sees.
- Reach the system through `AppCx` (`cx.host`, `cx.services`, and from step 5 of the split plan `cx.os`, a `FakeOs`). A tile that reads `/proc`, `/sys`,
  `/dev`, runs a program or names a hardware crate is counted by the ratchet (`crates/iyakki-core/tests/dependency_ratchet.rs`); it may not gain such a
  line, and a new file may have none. When you remove one, lower its number in `crates/iyakki-core/tests/data/shell_kernel_access.allow`.
- Run only your tile's tests, never the whole shell lib suite (`ide_tests::browser_tests` hangs):
  `cargo test -p iyakki-shell --lib -- apps::<tile>::tests`.
- `TileHarness` draws with the real software `Gfx` at a header of 72 px and scale `width/1280` (0.6 to 3); a tile that wants another area can call its
  `render` itself. Overlap means partly overlapping clickable regions; a control inside a card is fine.
