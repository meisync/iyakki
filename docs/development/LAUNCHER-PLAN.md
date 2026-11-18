# The new home screen: build plan (alpha.4)

Owner request 2026-11-14: build the locked home-screen design (`docs/design/launcher/`, open `index.html` in a browser). The design pages are HTML for decisions only; IYAKKI draws its own pixels with `tiny-skia` (`iyakki-uikit`), so everything below is Rust in `crates/iyakki-shell/src/launcher.rs` and `crates/iyakki-uikit`.

Branch: `alpha.4/launcher` (from the alpha.3 cut). Alpha.3 is not touched.

## Slices, in order (each ends green: feature_matrix, boundary, dependency_ratchet, zero warnings, the overflow tests, all three themes still work)

1. **Paged launcher and icons (no blur, no wallpaper yet).** Six pages side by side: *The machine*, then the five groups. Keys `←` `→` `1`-`6`, page dots and edge chevrons (mouse and touch). Icons: 108 px squircles at 1080p (use `scale_for`), per-app hue on a gradient, top gloss, rim light, white glyph; the state never changes the icon colour: it lives on the corner badge and the chip under the label. One frosted-looking island panel (40 px radius, rim, gloss band, ambient shadow) drawn as a translucent panel for now. Motion: island pop 0.975 -> 1 and an icon stagger of 42 ms, off when `IYAKKI_REDUCED_MOTION` is set. Tests: layout at 720p, 1080p, 1440p, 4K and at the largest text size; every app reachable by key; no overlap (the overflow harness).
2. **Wallpaper.** A setting (per account), a shipped default (an original image we own: `wall.svg` is a stand-in), a picker in Settings -> Display, a chat tool (`display_wallpaper`) with an approval, and the core-rule path (reads through the core).
3. **Blur.** `tiny-skia` has no blur primitive: blur the wallpaper once into a cached pixmap (box or stack blur, three passes) and sample it behind the island at the island's offset; the cache is rebuilt on wallpaper or size change only. Budget: no frame cost beyond one blit.
4. **Widget board (page 1).** A customisable board: Attention first (what needs a person, named with the real problem), approval queue, machine vitals, runs, tokens, network. Edit mode: remove badge, drag grip, size selector, add slot. Saved per account. First version with data from sources that exist today (inbox, approvals, `Sysinfo`, runs, usage ledger).
5. **`App::status()` on the 37 apps.** A default method returning none; implement in stages, most useful first, so the subtitle under each icon and the Attention list are real. Until an app implements it, show the app's static subtitle, never a made-up figure.

## Rules for this work
- Colours come from the theme (`iyakki-uikit::theme`), never literals; Dark, Light and High contrast must all stay readable (contrast tests exist: keep them green).
- Every new setting has a chat tool and a Settings control (parity rule); a wallpaper file is read through the core.
- The first slice must not slow an idle Home: it still redraws at most every 500 ms when nothing moves (KI-092).
- Screens for review: `./run.sh screens` renders every screen to PNG; `THEME=light ./run.sh screens` for Light. Compare with the design page and say what differs.
- Honest status in FEATURES.md: add rows for each slice (planned -> partial), Impl = code written, Test = tested.

## Status 2026-11-15 (branch alpha.5/q-launcher-shell)
- Slice 3, blur: `iyakki-uikit::blur` (F-LAUNCH-07). The finished background is shrunk to a quarter, blurred with three box passes and kept; the island paints it through its rounded shape. Rebuilt only on a size, theme or wallpaper change.
- Slice 4, widget board: `iyakki-uikit::board` and `iyakki-shell/src/shell_board.rs` (F-LAUNCH-08). Seven widgets, edit mode (Arrange button or `e`), kept per account, chat tools `display_board` and `display_board_set`.
- Slice 5, status: `iyakki_core_api::AppStatuses` (a facet of `Core`) and `App::status` (F-LAUNCH-09). Seven apps are named through the core and CHAT answers itself while open; the remaining apps say nothing yet and keep their static line.
- Also done on this branch: font choice (F-SHELL-42), hand-picked Light colours (F-SHELL-63), the screen's resolution on one screen (F-DISP-14).
