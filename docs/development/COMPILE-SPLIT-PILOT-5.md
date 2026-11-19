# Compile-split pilot 5: the PDF tile crate (2026-11-11)

Phase 2 of `COMPILE-SPEED-PLAN.md`, first measured step after the pilots of 2026-11-07 to 2026-11-09. Same recipe as the calendar and usage tiles
(`COMPILE-SPLIT-PILOT.md`, "Second step"): tile code and tests move into `crates/iyakki-tile-<name>`, the shell keeps a thin adapter, the
`FEATURES.md` citations are re-pointed by `compile-split-rename.py`, the dependency ratchet holds the boundary. No behaviour change, no new
third-party dependency (`tiny-skia` was already a dependency of the shell and the toolkit).

## Why `pdfview`

Chosen from the ranking in the plan (lines x commits since 2026-09-01) among the tiles that no one else was editing at the time. The big ones
(`settings`, `browser`, `chat`, `story`, `store`) were excluded on purpose: other work was in `settings*.rs`, `browser*.rs`, `store.rs`, `cameras*.rs`,
`input.rs`, `shell_views.rs` and the mixer/audio files. `pdfview.rs` is 948 lines plus 211 lines of tests, 3 commits in the window, and it touches
the agent in one place (`scope::check_read`), the audit log in one place, and the Downloads folder in one place. It is also used by the browser
(`PdfView`, the document handling of a PDF page), so it shows how a component shared with a tile that stays in the shell is handled.

## What moved

| New crate | From `iyakki-shell/src/apps` | Lines | Tests | Depends on |
|---|---|---|---|---|
| `iyakki-tile-pdf` | `pdfview.rs` (viewer component `PdfView` and the tile `PdfApp`), `pdfview_tests.rs` | 962 + 354 (tests were 211 lines; the harness and fakes are new) | 8 | `iyakki-uikit`, `iyakki-tile-api`, `iyakki-pdf` (reader library), `tiny-skia` |

The facade `iyakki-tile-api` got one module, `pdf`: the trait `PdfFiles` (`downloads_dir()`, `check_read(path)`, `opened(file_name)`) and `NoPdfFiles`.
`TileCx` was **not** changed: its seven struct literals are spread over the tile crates and `cameras.rs` (someone else's file), so the tile gets its
source by constructor (`PdfApp::new(Arc<dyn PdfFiles>)`), not through the context. `handle` and `render` take the `TileCx` and ignore it.

In the shell, `apps/pdfview.rs` is now the adapter (191 lines with one test):

* `ShellPdfFiles` implements `PdfFiles` with the person's policy (`iyakki_agent::scope::check_read`), `BrowserApp::downloads_dir` and the audit log.
* `PdfApp` wraps `iyakki_tile_pdf::PdfApp` and implements the shell's `App` (including `open_item` and `wants_redraw`).
* `PdfView` is a newtype around the tile crate's `PdfView` with `Deref`/`DerefMut`. Only `open_file`, `hint` and `handle` are forwarded by hand,
  because the browser (`browser.rs`, `browser/pdfhost.rs`, not edited) matches on the shell's `Flow` returned by `handle`. **No browser file changed.**
* `is_tab`, `ACCENT`, `TAB_BASE` are re-exported.

Edits to the moved code besides imports: the pixmap conversion `bitmap_pixmap` (12 lines, from `browser_images.rs`) is copied into the crate instead of
reaching into the browser; `PdfApp::new` and `open_path` take the `PdfFiles` instead of `AppCx`; `crate::ui::clipboard`, `crate::safeclamp` and
`crate::gfx` are the toolkit's.

## Tests

The eight tests of `pdfview_tests.rs` moved to `iyakki-tile-pdf::tests`. They used the shell's `TileHarness` and `cx_with_admin_store` (an `AppCx` with
the agent's `Services`); the crate has a 60-line harness of its own (click by label, text on screen, no-overlap check at the four standard sizes) and a
fake `PdfFiles` (a Downloads folder, a "denied" folder, the audit lines it was told about). Two assertions were added to the "bad files" and "empty tile"
tests: a file the person's rules refuse shows their sentence and is not audited as opened, and opening from the list is audited by file name.
One test stays in the shell (`the_shell_adapter_feeds_the_real_rules_downloads_and_audit_to_the_tile_crate`): the real Downloads folder lists, a
click opens, the browser's `PdfView` answers in the shell's `Flow`.

Results: `cargo test -p iyakki-tile-pdf` 8 passed; the shell's `pdf` filter 4 passed (adapter, the browser's PDF page test, the launcher test, a camera sheet test);
`feature_matrix` 4 and `dependency_ratchet` 7 passed.

## Gates

* `crates/iyakki-core/tests/data/tile_crate_deps.allow` has its **first line**: `iyakki-tile-pdf iyakki-pdf ...`. The reader library `iyakki-pdf` is the tile's
  engine (a parser and renderer for untrusted files, no I/O, no agent; it needs `iyakki-web` for the picture decoders). The rule was "the list may only
  shrink", so this is a deviation that is written down: the alternative is a `PdfEngine` trait in `iyakki-tile-api` with the real library behind the shell,
  which would put the page bitmaps and the glyph source across the facade for no gain in compile time. The line is removed if that ever changes. Decide whether
  you accept the first exception; the test fails again if the line goes stale.
* Citations: `COMPILE-SPLIT-PILOT-5-RENAMES-pdf.tsv` (8 `FEATURES.md` citations, F-PDF-05), plus the adapter test added to the row.

## Measurements

See `COMPILE-SPEED-PLAN.md`, "Phase 2 first measurable step", for the table: the test binary of the tile is 24 MB against 462 MB for the shell's, it links no
agent (`cargo tree -p iyakki-tile-pdf -e all` names no `iyakki-agent`), and a one-line edit in it costs 0.2 s (check) and 0.3 to 0.4 s (compile and link its
tests), against 3 to 30 s for the same edit in a tile that is still in the shell.

## Not done

* The browser still reaches `PdfView` through the shell newtype; when the browser is split it can use `iyakki_tile_pdf::PdfView` directly and the newtype goes.
* `TileCx` has no `pdf` field. When the next edit to `cameras.rs` is free, the seven literals could be given a `..TileCx::none(user)` helper; not worth a
  conflict today.
* Nothing was run on a device; no image was built.
