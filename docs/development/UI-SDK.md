# Writing an app with the UI SDK

For people who write a new IYAKKI app. Design and reasons: [UI-SDK-DESIGN.md](UI-SDK-DESIGN.md). What an app may ask the OS for (permissions, ports): [SDK-API.md](SDK-API.md). The working example is the crate `crates/iyakki-app-sample` (the SAMPLE tile).

**Status (alpha.4): a proof slice.** What is built and tested is listed in "What works today"; what is not built is in "What is not wired". New apps only: the existing apps are not on the SDK and are not rewritten in alpha.4.

## The idea in one minute

You write an app once. It implements `UiApp` (an id, a title, `event`, `view`). `view` draws through the `Canvas` trait. The same source then runs:

| Where | What draws it |
|---|---|
| The device screen and the desktop window | `RasterCanvas`: the `Canvas` calls become pixels (the toolkit's `Gfx`) |
| A browser tab or a client | `ListCanvas` records a **display list** (plain text, versioned); the browser draws it locally |
| A test | the same list, or the pixels; a test also reads the controls by name |
| A phone-sized view | the same app: it asks the SDK for its size class and lays out one column |

You never call `Gfx`, a file, a socket or a device. The app is given ports for what it may ask the machine, and the core decides.

## The crates

- `iyakki-ui-sdk` (layer `ui`): the SDK. It depends on `iyakki-uikit` only (for the Look and the pixel canvas) and has no file, socket or device access of its own (a test scans for it).
- Your app: a crate `iyakki-app-<name>` (layer `apps`) that depends on **`iyakki-ui-sdk` and nothing else from IYAKKI**. The SDK re-exports `look`, `theme` and `wallpaper`, so you never name `iyakki-uikit`. A test in the sample app checks its manifest and scans its source for colour literals.
- The shell side is one small file in `crates/iyakki-apps/src/apps/` (see `sample.rs`, about 100 lines): it maps the shell's events to SDK events and gives the app a `RasterCanvas`.

## A first app

```rust
use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::canvas::{Canvas, TextStyle};
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;

pub struct Hello { count: u32 }

impl UiApp for Hello {
    fn id(&self) -> &'static str { "hello" }
    fn title(&self) -> String { "HELLO".into() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Action(1) => self.count += 1,          // a tap or click on the "More" button
            UiEvent::Key(UiKey::Esc) => return Flow::Back,
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();                              // logical units, never a constant
        let th = cx.colors();                               // the Look's colours
        let m = cx.metrics();                               // size class, touch target, text size
        c.gradient(Rect::new(0.0, 0.0, w, h), th.bg_top, th.bg_bottom);
        c.text(m.gutter(), m.gutter(), m.font(20.0), th.text, &format!("Pressed {} times", self.count), TextStyle::Bold);
        iyakki_ui_sdk::widgets::button(c, th, &m, Rect::new(m.gutter(), 80.0, 160.0, 40.0), "More", 1, true);
    }
}
```

## The rules for `view`

1. **Draw only through `Canvas`**, and lay out from `canvas.size()` and `cx.metrics()`. Coordinates are logical units; the pixel canvas applies the screen's scale.
2. **Colours come from `cx.colors()`** (the Look's theme). No colour literals: the sample's test fails on `rgb(` and on three-number arrays.
3. **Every clickable thing is a hit region with a name and a role** (`Canvas::hit`, or a widget that does it). The name is what the screen reader says and what a browser client exposes as ARIA. A hit's `action` number comes back as `UiEvent::Action(n)` when the thing is clicked, tapped or confirmed with Enter.
4. **Ask the size class, do not guess pixels.** `Compact` (under 600 units wide: a phone or a narrow pane) is one column; `Medium` (under 900) and `Expanded` may add a side panel. The pane can be resized or moved to another screen or view at any time; the host then sends `UiEvent::Resized` and the next `view` asks again.
5. **Targets are at least `m.min_target()`**: 44 on touch, 24 with a pointer. The widgets enforce it. Font sizes go through `m.font(size)` so the person's text size applies (the SDK caps it at 3x; test your layout at 3x).
6. **Shorten, do not overflow.** `canvas::fit(c, size, text, style, max_w)` cuts text with an ellipsis; the widgets do it for you.
7. `view` is called again and again: keep it cheap and keep state in the struct, not in `view`.

## Input

One event type, `UiEvent`: `Pointer` (with an id, so several fingers), `Gesture` (tap, long press, drag, swipe), `Wheel`, `Key`, `Text` (typed, pasted or composed), `Action` (a hit region was used), `Resized`, `Tick`. Raw pointers become gestures in `gesture::Recognizer` (a tap is a press and release within 300 ms and 10 units; a long press is 500 ms; a swipe is a drag of 60 units in 300 ms). A tap that lands on a hit region of the last frame is delivered as `Action`, so most apps only handle `Action` and `Key`. Two fingers make a pinch: `Gesture::Pinch { scale, x, y }` (the distance between them over the distance when the second finger came down, and the place half way) and `Gesture::PinchEnd`; neither finger of a pinch makes a tap, a drag or a swipe.

## Looks, the dock, the wallpaper, screens and views

All of this is in `UiCx`:

| You want | Call |
|---|---|
| The colours and the Look | `cx.colors()`, `cx.look()` (icon shape and style, bars, dock choices, motion) |
| The size class and metrics | `cx.size_class()`, `cx.metrics()` |
| The screen this pane is on | `cx.screen()` (size in pixels, scale, orientation, place in the arrangement) |
| The pane and the view | `cx.pane()` (screen, view name, size), `cx.view()` (name, index, count) |
| Pin, unpin, open an app | `cx.dock.pin("app")`, `unpin`, `open` (a `DockPort`; the answer can be a refusal, show it) |
| A badge and one status line on your dock icon | return `Some(DockStatus::new(badge, "3 open"))` from `UiApp::dock_status`; the host publishes it after every event (the line is cut at 40 characters and loses control characters) |
| The wallpaper of this screen | `cx.wallpaper_here()`; set it with `cx.wallpaper.set(screen, id)`; the `widgets::wallpaper_picker` draws the choice |
| Readable text on the wallpaper | `cx.text_on_wallpaper()` (the Look's text colour when it reads at 4.5:1, else black or white) |
| The dock and wallpaper of any screen | `cx.arrangement.dock_for(look, screen)`, `wallpaper_for(look, screen)`: a screen's own choice, else the Look's; **the dock is off on a second screen unless chosen** |

When a screen is unplugged, `Host::unplug_screen` moves a pane that was on it back to the primary screen and sends `Resized`; nothing is lost.

## Themes, skins and fonts (owner order 2026-11-16, rows F-SKIN-01 to F-SKIN-07 (the matrix allows two-digit numbers only, so the `F-SDK-120` the owner order named became the area SKIN))

What an SDK app gets: the Look's colours (`cx.colors()`), the size class, the text size (`m.font(size)`, capped at 3x) and, since slice 2, **design tokens beyond colour** from the Look's skin parts. Slices still to come are listed at the end of this section.

### Audit (2026-11-16): what was written into the code by hand

Counted as decimal literals of 3.0 or more outside comments (a rough count of styling numbers), before the tokens:

| File | Literals | What they were |
|---|---|---|
| `widgets.rs` (button, row, field, wallpaper picker) | 23 | corner radii 6 and 8; border width 1 and the focus ring `2 * ring`; paddings 8, 10, 12, 16, 20; text sizes 14, 15, 34; swatch width 120; caret 2 wide; no shadow, no glass, no motion |
| `chatview.rs` (ChatView) | 25 | panel radius 10, card radius 8; paddings 4, 6, 8, 12, 16, 22, 24; text sizes 12, 14; line heights 1.3 and 1.35; button width 76; a 2-wide amber outline |
| `iyakki-app-sample` | 28 | side panel radius 10, gaps 4, 6, 8, 10, 12, 14, 16; text sizes 12 to 22; row height 30; widths 70 and 150 |
| `iyakki-app-markdown` | 22 | row radius 8; indents 8, 12, 16, 20; heading sizes 15, 17, 20, 24; code size 13; line height 1.35 |
| `raster.rs`, `list.rs` | 3 | the text measure estimate (0.55, 0.58, 0.6 of the size per letter); scale 1.0 defaults. Not styling |
| Files, Browser, Terminal, Editor views | not on this branch | follow the same rules once they land: the scan test below applies to them |

Findings. (1) Nothing in the SDK read anything but colour from the Look; the Look already had `motion`, icon size and shape, but only the launcher used them. (2) Radii, borders, gaps and text sizes were literals, so no skin could change them. (3) Shadows existed as a `Canvas` call and in the display list (`Shadow`) but no widget drew one; glass and blur were data in the launcher's icon style only; `Canvas` has no blur operation (display list v2, F-SDK-35). (4) Motion: no SDK widget animates; `Look::motion` is read only by the shell; renderers never animate. (5) Found by the contrast check while building skins: the label on a primary button was the theme's text colour, which is 2.9:1 on the Light accent and below 4.5:1 on the Dark accent; `skins::label_on` now picks the theme's text when it reads and black or white otherwise (the Dark primary button label turns black). (6) The focus ring was `2 * theme.ring` in two places. (7) Text had three styles (`Regular`, `Bold`, `Mono`), no family choice, no line height, no script fallback in the SDK; the family is the shell's per-person choice (`iyakki_uikit::fonts`), which the pixel canvas follows and the browser and client renderers do not.

### Tokens (slice 2, F-SKIN-01 and F-SKIN-02)

`UiCx::metrics()` carries `m.tokens` (`iyakki_ui_sdk::tokens::Tokens`), made from the Look: `look.surface` (shape, edge, spacing, elevation, glass, blur, glyph size), `look.motion` and the theme's `ring`. An app and a widget ask for a step of a scale, never a number:

| Token | Call | Skin words (Look file) | Plain skin |
|---|---|---|---|
| Corner radius | `m.radius(Radius::Sm / Md / Lg)`, `tokens.radius_for(r, height)` (never more than half the height) | `[shape] shape = sharp / soft / round / pill` | 6, 8, 10 |
| Border width | `m.line(Line::Hair / Normal / Thick)` | `[shape] edge = thin / regular / heavy` | 1, 1, 2 |
| Gap and padding | `m.gap(Space::Xxs .. Xl)`, `m.gutter()` | `[spacing] spacing = tight / normal / airy` (x0.75, x1, x1.25) | 2, 4, 6, 8, 12, 16 |
| Shadow of raised things | `tokens.shadow_spread()`, `widgets::surface(.., raised)` | `[effects] elevation = flat / low / high` | flat |
| Glass | `tokens.glass`, `tokens.blur`, `widgets::glass_fill` | `[effects] glass = on / off`, `blur = 0..24` | off |
| Glyph size | `tokens.glyph()` | `[effects] glyph_size = 12..48` | 20 |
| Motion | `tokens.duration_ms(Speed)`, `tokens.animates()` | `[motion] motion = full / reduced` (reduced gives 0 ms) | full |
| Focus ring | `tokens.focus_width()` | the theme's `ring` | 2 |
| Text size | `m.size(TypeRole::Caption .. Display)`, `m.leading(size)`, `m.em(n)` (a width in text sizes) | the person's text size | 12 to 24 |

Widgets have `widgets::surface` (fill with the skin's radius, shadow and glass) and `widgets::panel`. A Look file without the new sections reads as the plain skin, so every Look file written before skins keeps working and looks as it did. Glass in the SDK is a translucent fill with a hairline edge; the blur radius is recorded in the tokens and drawn when the display list gets its Blur operation (F-SDK-35), until then renderers draw the translucent fill.

**The rule, and its test.** `iyakki_ui_sdk::lint::style_literals(source)` finds decimal numbers of 3.0 or more in drawing code (numbers under 3, `const` lines, `em(` arguments, comments, strings and test modules are let through). It is run over `widgets.rs`, `chatview.rs`, the sample and the Markdown viewer; run it over your own app's `lib.rs` in a test:

```rust
#[test]
fn no_styling_numbers() {
    let bad = iyakki_ui_sdk::lint::style_literals(include_str!("lib.rs"));
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
```

### Skins (slice 3, F-SKIN-03)

A skin is a Look file: the sections of the Look format plus `[shape]`, `[spacing]` and `[effects]` (above). It restyles shapes and tokens, not only colours; it is validated and listed by the same registry as every Look, and chosen per person.

| What | Where |
|---|---|
| Shipped skins: Paper (pale, round, thin edges, airy), Glass night (dark, glass, blur 16, low shadow), Blueprint (dark, square corners, heavy edges, tight), Calm (pale, pill corners, high shadow, reduced motion) | `crates/iyakki-uikit/skins/*.look`, listed by `look_registry::available` after the three built-in Looks (a file cannot take their names) |
| Validator | `iyakki_uikit::skins::validate(text)`: the Look rules (ranges, closed lists, colour contrast) and the pairs the widgets draw: the label on a primary button (`text` on `accent`, 4.5:1) and the accent against the card and the page (3:1). Every reason is returned |
| Choice per person, live | Settings, Appearance, and the chat tool for Looks write the person's choice (`look_registry::choose`) and call `notify_changed`; the shell reads it at its next heartbeat. An SDK host takes `Host::set_look(look)`; the app keeps its state and the next `view` sees the new skin. Nothing restarts |
| What an app may ask | `cx.skin()`: key, name, base theme, dark, glass, reduced motion, shape, edge and spacing words. `cx.tokens()` and `m.tokens`: the numbers |
| An app's own accent | `cx.accent(wanted)` returns the colour to draw with (WCAG AA: the label 4.5:1, the control against card and page 3:1) and says whether it was `adjusted`; `cx.colors_with_accent(wanted)` is the theme with it fitted. A failing accent is moved step by step towards the skin's own accent, so the app's hue stays as long as it can; if no step passes the skin's accent is used |

The writing of a skin file is the Look format (`docs/development/LOOK-DESIGN.md`); to make your own, copy a shipped skin into your themes folder with another name.

### Fonts (slice 4, F-SKIN-04)

An app asks for a **role**, never a file. `FontRole` is `Body`, `Ui`, `Mono` or `Display`; the person's choices come from the core and reach the app as `cx.fonts()` (`TypeStyle`: family, line height, letter spacing, tabular figures) and `m.text_style(role, TypeRole)` (canvas style, size, leading, options). `m.leading(size)` already follows the person's line height, so layouts written with it grow and shrink with the choice.

| Choice | Words | Where it is kept | Who applies it |
|---|---|---|---|
| Family | DejaVu Sans (default), Liberation Sans; DejaVu Sans Mono or Liberation Mono for code | `ui-<person>.json` (`fonts`, F-SHELL-42) | the pixel canvas draws it; the display list carries it (`textx`); the browser draws its CSS family |
| Line height | tight 1.2, normal 1.35, loose 1.6 | `typeset` | `m.leading` |
| Letter spacing | normal, wide 0.04, wider 0.08 (of the text size) | `typeset` | the canvas (`Canvas::set_type`); not applied to connected scripts |
| Tabular figures | on, off | `typeset` | the browser draws digits at one width; DejaVu and Liberation digits already have one width |
| Text size | 0.5x to 3x | the UI size | `m.font`, `m.size` |

**Scripts.** DejaVu and Liberation carry Latin, Greek and Cyrillic; Noto Sans carries Arabic, Bengali, Devanagari, Gujarati, Gurmukhi, Hebrew, Kannada, Malayalam, Tamil, Telugu and Thai (legal/assets.txt). `TypeStyle::uncovered(text)` lists the scripts of a text that no bundled font covers. **No CJK font and no dyslexia-friendly font is bundled**, so neither is offered; adding either needs a font with a licence entry first.

### Display list version 3: paths, layers, cluster runs (F-SDK-35)

Version 3 is additive again: a list is written as version 3 only when it holds one of these, else as 2 or 1 as before; a list newer than 3 is refused by name. Fixture: `tests/compat/alpha7/display-list-v3.txt`.

* `path FILL STROKE WIDTH SEGMENTS` (`Canvas::path`): `FILL` and `STROKE` are colours or `-` (at least one is given); the segments are joined with `;`: `M x:y`, `L x:y`, `Q cx:cy:x:y`, `C c1x:c1y:c2x:c2y:x:y`, `Z`; a path starts with a move, has at most 2048 segments and finite numbers. Filled with non-zero winding (a subpath wound the other way is a hole), stroked with round ends. `iyakki_ui_sdk::path` flattens, fills and strokes, so every renderer draws the same shape.
* `layer ALPHA` ... `endlayer` (`Canvas::layer`): everything inside is drawn at that opacity (0 to 1, clamped). Layers nest with clips, 64 deep in all. The pixel canvas, the browser (`globalAlpha`), the client engine and the tile adapter fade each shape on its own; overlapping shapes of one group each fade (the same as a browser's `globalAlpha`).
* `textc X Y SIZE COLOUR STYLE FAMILY TRACKING TABULAR ADVANCES TEXT` (`Canvas::text_clusters`; `ListCanvas::text` writes it by itself for a run that has a mark, a joined emoji, a flag or Hangul jamo): one advance for every cluster (`path::clusters`: a letter with its marks), so a renderer without the font puts each cluster where the device's shaper did. The browser draws each cluster at its pen; the engine places the glyphs of each cluster on the cluster's pen; the pixel canvas shapes the run itself and ignores the advances; the tile ABI draws a plain run.
* Images (`image`, version 1 since alpha.5) and `blur` (version 2) are as before.
* A text measure without a font file for a client: `list::AdvanceTable` keeps the advance of each letter per unit of size; `UiSurface::measure_table` in the engine builds one from the glyphs the page has sent, and `AdvanceTable::into_measure` goes to `ListCanvas::with_measure`.

| Renderer | Path | Layer | Cluster run |
|---|---|---|---|
| Shell (pixel canvas) | filled and stroked (tiny-skia) | colours faded | shaped by the toolkit |
| Browser (`ui/displaylist.js`) | Canvas2D path | `globalAlpha` | each cluster at its pen |
| Client WASM engine | in-module scanline fill and stroke | faded | atlas glyphs at cluster pens |
| WASM tile adapter (tile ABI) | row bands and lines | colours faded | plain run |

The engine module's size limit went from 608 to 640 KB for this (the version 3 reader and the path rasteriser, about 28 KB); see ALPHA-7-DECISIONS.md.

### Display list version 2 and the renderers (slice 5, F-SKIN-05)

Version 2 is additive. A list is written as version 1 unless it holds a `textx` run (`textx x y size colour style family tracking tabular text`) or a `blur` (`blur x y w h radius`), so older readers keep reading lists that do not need them; a list newer than 2 is refused by name. Fixture: `tests/compat/alpha5/display-list-v2.txt`; the alpha.4 fixture still reads and writes the same.

| Renderer | Family | Letter spacing | Tabular figures | Blur |
|---|---|---|---|---|
| Shell (pixel canvas) | the person's family (the one the shell draws with) | yes, letter by letter | the bundled digits are tabular | yes, a box blur twice over the region |
| Browser (`ui/displaylist.js`) | CSS family of the run | yes | digit by digit at the widest digit's width | canvas filter over the region |
| Client WASM engine | the page's font, atlas glyphs as sent | yes (pen distance) | no | no (the surface is drawn without frost) |
| WASM tile adapter (tile ABI 1) | no | no | no | no |

### Chat tools and Settings (slice 6, F-SKIN-06)

`display_type` and `display_type_set` (asks first) for line height, letter spacing and tabular figures; Settings, Appearance rows `l`, `m`, `u`. The Look and skin (`display_look`, `display_look_set`, `[` and `]`) and the font family (`display_fonts`, `display_font_set`, `f`, `p`) had both already.

### Tests (slice 7, F-SKIN-07)

`type_tests` runs the widgets and a ChatView at Dark, Light, High contrast and the pale Paper skin, at the three size classes, text 1x, 2x and 3x, both families and letter spacing off and wide, with the device's real text measure: nothing runs off its space, every target is at least the smallest size, text and label contrast are at least 4.5:1 and the accent 3:1 against the card. Skin switching and the accent correction are in `token_tests`.

### Not done

- The SAMPLE and Markdown tiles in the shell still draw with the default Look and the current theme: a skin chosen in Settings does not reach them (it reaches an SDK `Host` through `set_look`).
- A skin file cannot carry type settings; the person's choices apply to every skin.
- The family of a single run is not switched on the pixel canvas; it draws the person's one family.
- The browser harness (`dist/browsertest/uisdksuite.py`) was not run on the JavaScript changes.

## Testing your app

The `Host` is the same driver the shell uses. In a test:

```rust
let mut h = Host::new(MyApp::new(), Look::default(), Arrangement::single(360, 740),
                      Box::new(MemDock::knowing(&["hello"])), Box::new(MemWallpaper::default()));
let list = h.record();                       // the frame as a display list
assert!(list.texts().contains(&"HELLO"));
let (x, y) = centre_of(&list, "More");       // find a control by its name (list.a11y(), list.ops)
h.mouse(x, y, Phase::Down, 0); h.mouse(x, y, Phase::Up, 60);   // a tap
```

What the sample's tests do and you should copy: render at 360x740, 800x480 and 1280x720 and at text size 3x and check every control has a name and sits inside the pane and (on touch) is at least 44 high; check the compact and expanded layouts differ; drive it with keys, taps and `Text`; check the dock status; scan the source for colour literals and machine access.

`DisplayList::to_text()` and `DisplayList::parse()` are the wire format (line based; strings are percent-escaped; a list newer than `DISPLAY_LIST_VERSION` is refused by name; an unknown operation is an error naming the line). `list.replay(&mut canvas)` draws a list on any canvas: a test proves a recorded and replayed frame has the same pixels as the direct frame.

## Registering the app as a tile

1. Add the app to `iyakki-sec::policy::App` (the id and, in `Role::template`, the roles that may use it) and to the lists that name every app (`cloudmode::app_class`, `memgate::class_of`).
2. Write the shell adapter next to `sample.rs` (events in, `RasterCanvas` out) and add the `Tile` row and the `build` arm in `shell.rs`.
3. Add the crate to the workspace, to `dist/scripts/crate-layers.txt` (layer `apps`), and rows to `FEATURES.md`.
4. Chat tool and approvals: if your app changes the machine, it does it through a port and the core asks. The SAMPLE app changes nothing outside itself.

`iyakki new-app NAME [--dir DIR] [--sdk PATH]` makes steps 3 and the crate for you (the crate, `app.toml`, a storage port stub and a golden test; it prints what is left: workspace member, layer line, matrix row). Registering the tile (steps 1 and 2) is still by hand.

## As a WASM tile

`iyakki new-app NAME --tile` also makes `src/tile.rs` (one line: `iyakki_sdk_tile::export_sdk_tile!(crate::NameApp::with_storage);`), a `[lib] crate-type = ["rlib", "cdylib"]` and `iyakki-sdk-tile` as a second dependency, and adds `tile_abi = 1` and `tile_caps = ["storage", "dock"]` to `app.toml`. Build it with `cargo build --release --lib --target wasm32-unknown-unknown` (needs `rustup target add wasm32-unknown-unknown`). The tile host loads the `.wasm` with only the capabilities the manifest names; the storage and dock ports go through the tile's `call`, so the host checks the capability and the core asks the person where it must and logs.

What the adapter does (`crates/iyakki-sdk-tile`): the SDK display list is mapped onto tile ABI 1 operation by operation (the table is in `convert.rs`: a gradient becomes bands of rectangles, a shadow three fading rounded rectangles, an outline loses its corner radius, a hit region's id is its action plus one); the tile's events drive the SDK `Host` (the clock is the sum of the `Tick` events); a tap is resolved by the SDK host and the ABI's `Hit` for the same tap is dropped, so the app is told once. Text is laid out with the ABI's own measuring table, so what the app lays out is what the host draws.

Not in the tile build: names for a screen reader (the ABI's `Access` operation is ABI 2, the tile kit speaks ABI 1, so a tile has hit regions but not names), a pinch event (the tile ABI has one pointer), the wallpaper port, the shell's pane manager, and a `dock` namespace in the core (the dock port asks `dock.pinned`, `dock.pin`, `dock.unpin`, `dock.open`, `dock.status`; the core's published namespace table has none yet, so the host refuses and the app says "The dock did not allow that").

## Embedding the assistant (ChatView)

Owner decision 2026-11-16: an SDK app can place the IYAKKI assistant in its own layout, bound to a conversation and to what the person is looking at, and the assistant acts in the app with the app's own tools. Rows F-SDK-80 to F-SDK-85.

**What you write.** Keep a `ChatView` in your app state, give it the core's `AssistantPort` (the host hands it to you, like the storage port), a `SessionConfig` and an action number range (`chatview::ACTIONS` numbers from a base you choose):

```rust
let chat = ChatView::new(port, SessionConfig {
    conversation: "notes".into(), title: "My notes".into(),            // the same name gives the history back
    tools: vec![ToolSpec::new("add_note", "Add one note to the list.", SCHEMA_JSON, /* ask */ true),
                ToolSpec::new("list_notes", "List the notes.", "{\"type\":\"object\"}", false)],
    ..SessionConfig::default() }, 20_000);
// event(Tick):    for inv in chat.tick() { let r = self.run_tool(&inv); chat.answer(&inv, r); }
// event(Action):  if chat.owns(a) { chat.action(a); }       event(Key / Text): offer to chat.key(k) / chat.text(t) while chat.focused
// event(...):     chat.set_context(vec![ContextItem::new(ContextKind::Document, "Notes", &text)]);   // what the person looks at now
// view:           chat.draw(canvas, cx.colors(), &cx.metrics(), rect);                                // theme and size class are the host's
```

`iyakki-app-sample` is the working example (`with_assistant`, `run_tool`, `sync_context`). `toolargs::Args::parse(&inv.args)` reads the JSON arguments without a dependency.

**What the view does.** It shows the transcript (your messages, the answer as it is written, one line per tool call with its state), the field with Send and Stop, the model and role buttons (the choices come from the core), the approval card (Allow and Deny; `y` and `n` while a card shows and the field has no focus) and the sentence of a refusal. It opens the session on the first tick, send or context change; a refusal ("The agent is not enabled for your account.") is shown and tried again at the next message. It owns nothing else: your app keeps its keys unless the field has focus.

**Where things run.**

| Piece | Where | Rule |
|---|---|---|
| The model, the history, the tool loop | the core (`iyakki_core_api::assistant::Assistant`; on the device `iyakki-agent::embedassist::AgentAssistant` over the run loop) | your app never sees a key, a provider or a file |
| Your tools | your app (`run_tool`) | the model can call only the tools you declared, as `app_<app>_<tool>`; arguments are checked against your schema before the call reaches you; check them again |
| Approvals | the core | a tool declared `ask` shows a card; nothing reaches your app before the person allows it; no "allow for the session" for app tools |
| Context | your app, through `set_context` | the model reads it as untrusted data (a block marked as such), at most 8 items, 16 KB each, 48 KB together |
| Audit | the core | `assistant.open`, `assistant.send`, `assistant.tool ... approved by user` / `denied` / `ran ok`, `assistant.cancel`: names only, never the message, arguments or context |
| Limits | the core | a request over a limit is refused with a sentence (tools 16, schema 4 KB, message 8 KB); the account's agent switch and budget apply |

**Event order** (`ChatEvent`): `ToolRequest`, then `Approval` (when it asks), `Decision` (`by` is `auto`, `user` or `system`), `ToolCall` (your turn: run it and `answer`), `ToolResult`, more partial answers, then exactly one of `Final`, `Failed`, `Stopped`. A call the person denied, a tool you did not declare and an app that does not answer within 100 seconds are told to the model as failures.

**Testing.** `SharedAssistant::new(vec![Turn::call("text", "add_note", "{\"text\":\"milk\"}"), Turn::say("Done.")])` is a scripted port with the same approval rules as the core (`seen` records what the model was shown, `audit` the lines). Drive the app with `Host` as usual (`Action`, `Key`, `Tick`) and read `chat.transcript()`. Against the core's own fake: `FakeCore::assistant_fake().script(vec![FakeTurn::call(..), FakeTurn::say(..)])`. The real adapter is tested over the loopback model server (`embedassist_wire_tests.rs`).

**More components.** The IDE panel, Files and Terminal follow the same pattern (a core facet with a port, a view that draws through `Canvas`, events polled, every machine action through the core with the person's approval). They are planned, not built; the assistant is the first.

**Not built yet** (rows F-SDK-84, F-SDK-85): the WASM tile build of an app with an assistant (the tile `call` has no `assistant` domain, so the tile port is missing), and the facet over the contract socket for a UI program outside the process (it sees `NoAssistant`). One model choice only (the model chosen in Settings). The Chat app itself does not use the facet: it runs its own full tool set and keeps its direct agent uses in the ratchet.
## Embedding the file browser (FilesView)

`iyakki_ui_sdk::filesview::FilesView` is the person's own home folder as a component an app places in its layout (F-SDK-100). It is the
same shape as the other embeddable components (the assistant's `ChatView`, the browser): a **port** the host hands the app, a **view** the app
keeps in its state, and a **core facet** behind the port.

* **The facet** (`iyakki_core_api::files::Files`, defaults answer "this core has no file browser"): `list(user, dir)` and
  `read_text(user, path, max)`, version 1 reads only. Same policy as `markdown.openFile` (`HomeFiles`): the root is the person's home folder and
  nothing outside it is listed or read; a link is never followed or listed; hidden names are skipped; paths are relative (`""` is the home) and the
  app never learns where the home is; a folder lists at most 500 entries (`truncated` says so), a text read at most 1 MB and valid UTF-8 only; a file
  over the limit is refused with a sentence, never cut. Every implementation uses `files::check_rel`, so the refusals read the same everywhere.
  **Version 2 (F-SDK-101) changes files, and every change is a question.** `Files::change(user, FileChange)` never changes anything: it checks the
  request (`check_change`: plain paths, a plain name, a save of at most 1 MB) and answers with the card's words (`ChangeAsk`, names only: the
  text of a save is never in them); `Files::decide(user, call, approve)` is the person's answer, and only a yes runs it (`apply`, which checks
  again, because the app is not trusted). The changes are `MakeDir`, `Rename`, `Trash` (moved to `<data>/trash/<person>`, not erased) and `Save`
  (written beside the file and moved over it, so a failed write leaves the old file whole). A person has at most 4 questions waiting; a call can be
  answered once and only by the person it was asked of. The audit has one line per step (`files.change.asked`, `.approved`, `.denied`, `.done`,
  `.failed`) with the card's words.
* **The implementations:** `iyakki-core-local::homebrowse` (the folder walk), `AgentCore` (the real core: the root is the account's home), and
  `iyakki_core_fake::FakeFileTree` (made-up homes for tests; `FakeCore::file_tree_fake()`).
* **The port** (`iyakki_ui_sdk::files::FilesPort`): `list` and `read_text` with `FilesError::{Refused(sentence), Unavailable}`. `MemFiles` is the
  in-memory port for tests and the browser preview, with the same rules. The shell-side port over the core is `apps::sample::CoreFilesPort`.
* **The view**, in four lines:
```text
event(Tick):   files.tick()                           (opens the home folder once)
event(Action): if files.owns(a) { files.action(a) }   event(Key): if files.key(k) { return }   event(Wheel): files.wheel(n)
view:          files.draw(c, th, &metrics, rect)      what the person chose: files.take_opened() -> Option<(path, text)>
```
  The buttons New folder, Rename and Delete and `FilesView::save(path, text)` ask for a change; the card (Allow, Deny; keys `y`, `n`, Esc) is the core's question and nothing changes before it is answered; after a yes the folder is read again and the view says what happened. A tap or Enter on a folder goes into it; on a text file it shows the text beside the list (instead of it on a pane under 560 units wide) and
  hands the app the path and the text once. Up, Backspace and Left go to the folder above and select the one the person came from. The view
  uses `ACTIONS` (72) action numbers from the base the app gives it, draws only inside its rectangle, gives every control a name, and
  works at every size class and text size.
* **In SAMPLE:** the "Files" button (or `f`) shows the pane in place of the notes; opening a text file puts its first line in the status line.
  The tile gets the core's port at `bind`.
* **Chat and approvals:** nothing new to switch: listing and reading a person's files by chat is the existing `list_dir` and `read_file`
  (CHAT-REACHABILITY, Files), with the same home-folder scope; the view adds no write that would need a card.

## Embedding the terminal (TerminalView)

`iyakki_ui_sdk::terminalview::TerminalView` is the IYAKKI command line (the CLI app's) as a component (F-SDK-102). Same shape as `FilesView`:

* **The facet** (`iyakki_core_api::terminal::Terminal`, defaults say "this core has no terminal"): `open(user, app)`, `run(user, session, line)`,
  `decide(user, session, call, approve)`, `close`. The core owns the session and the policy: every line is checked against the person's own
  policy (their files, their role's commands, their hosts), which comes from the accounts (`PolicySource`) and never from the app. Lines that
  only read and print (`terminal::READ_ONLY`, no redirection) run at once; **anything else does not run: the reply carries a `TermApproval` and the
  session waits** for `decide`; a new line meanwhile is refused with a sentence, and only the person's own session can be answered.
  The machine is never switched off or restarted from here (`NO_POWER`). At most 4 sessions a person, a line of at most 4096 characters, output
  cut at 64 KB (`cut` says so). Each open, run, question, answer and close is one audit line with the command word only, never the arguments.
* **The implementations:** `iyakki-agent::terminalfacet` (the real one: `cli::Cli` per session, with a host that can do nothing to the machine),
  the `AgentCore` adapter (`AgentCore::with_policies(..)`; the shell gives it the accounts), `iyakki_core_fake::FakeTerminal`.
* **The port** (`iyakki_ui_sdk::terminal::TerminalPort`): `open`, `run`, `decide`, `close`; `MemTerminal` for tests and the preview. The
  shell-side port over the core is `apps::sample::CoreTerminalPort`.
* **The view:** the app calls `tick()`, forwards actions, keys, text and the wheel, calls `draw(..)`, and `close()` when it leaves. The view
  shows the typed lines, the output (monospace, wrapped by characters), the field with the prompt, Run, the history on Up and Down, and the
  "Allow / Deny" card (keys `y`, `n`, Esc). It uses `ACTIONS` (6) action numbers and works at every size class.
* **In SAMPLE:** the "Terminal" button (or `t`) shows the pane in place of the notes; it excludes the Files pane.
* **Not built (F-SDK-103):** full-screen programs (`vi`, `top`, `ssh`): the facet is a command line, not a pseudo-terminal; `iyakki-vt` has the
  parser, the screen and the pty, and the cell grid is the next version of the port.
* **Chat and approvals:** the assistant's own `terminal` tool stays read-only; there is no new chat tool, the card of the embedded terminal is
  the approval. 

## Embedding the editor (EditorView)

`iyakki_ui_sdk::editorview::EditorView` is the IYAKKI text editor as a component (F-SDK-104). The engine is `iyakki-edit` (plain Rust, no
dependencies, no I/O: the buffer with undo and redo, the highlighter, the line diff and the reader of the assistant's edit blocks), which the
SDK links with the pixel side. The view owns the gutter, the highlighted lines, the cursor and selection, a toolbar (Undo, Redo, Save) and the
review of what the assistant proposes; it draws through `Canvas` with the colours of the host.

* **No core facet of its own.** The editor never touches a file or a model. It uses the facets the app already has: the text comes from the files
  port (`FilesView` / `Files`, version 1 reads) and goes back through a write the app makes itself (the Save button only hands the app the path and
  the text once, `take_save()`; writing through the core with the person's approval is F-SDK-101); the assistant is the assistant component
  (`ChatView` / `Assistant`, from the same round). That keeps every approval where it already is.
* **What the app does:**
```text
open a file:   ed.load("notes/todo.md", &text)                   (refused with a sentence when binary or over 2 MiB)
event(Action): if ed.owns(a) { ed.action(a) }   event(Key): ed.key(k)   event(Text): ed.text_in(t)   event(Wheel): ed.wheel(n)
view:          ed.draw(c, th, &metrics, rect)
saving:        if let Some((path, text)) = ed.take_save() { ...write it through the app's own port...; ed.mark_saved() }
the assistant: ed.context() -> the document (at most 40 KB) and the selection, to hand the assistant component as context;
               ed.offer_reply(&reply) -> a reply with SEARCH/REPLACE blocks (or a whole file) that match becomes a proposal to review
```
* **The assistant hook is a review, not an edit.** A proposal never changes the text by itself: the view lists each changed place as a hunk
  ("Accept line 3: -1 +1", a tap skips it), and Apply puts the accepted ones in as one undoable edit (Undo takes all of it back). A reply whose
  blocks do not match, or match twice, is refused with its reason; Esc or Discard drops a proposal.
* **Keys:** the editor takes keys only while it has the focus (a tap on a line gives it; Esc gives it back); arrows, Enter (keeps the indent), Backspace,
  Delete, Tab, characters. While a proposal waits Enter or `y` applies and Esc or `n` discards. It uses `ACTIONS` (160) action numbers.
* **In SAMPLE:** the "Edit" button (or `e`) shows the notes as text (a `[x] ` in front of a done one); Save turns the lines back into notes.
* **Not built:** selecting with the keyboard or the pointer (the buffer has it; the view has no shift keys yet), find and replace, soft wrap,
  and the on-screen keyboard's hand-off beyond `wants_keyboard`.

## Embedding camera views (CameraView)

Owner request 2026-11-16: an SDK app can place live camera pictures in its own layout (a doorbell panel, a home dashboard, a kiosk) with a `CameraView`. Rows F-SDK-160 to F-SDK-169. A camera is a privacy-sensitive device, so the app never opens one: it asks the core for a *view*, and the core decides who sees what, asks the person, shows that a view is open and writes the audit line. **Status (2026-11-16): built and tested on fakes and over the real hub with the loopback RTSP camera: the facet, the fake, the device implementation (F-SDK-161, F-SDK-162), the picture operation (F-SDK-164), the `CameraView` widget with `CameraPort` and `MemCameras` (F-SDK-160), the core-backed port and the SAMPLE cameras tab (F-SDK-163), the chat tools for the person's switch (F-SDK-165). Not built: the WASM tile and the contract socket (F-SDK-166), pan, tilt and zoom on a real camera (F-SDK-167), a Settings row, the machine's red sign counting views. Nothing here was drawn on a real screen or run with a real camera.** Facet `iyakki_core_api::cameras::Cameras` (`Core::cameras()`, default `NoCameras`), fake `FakeCameras` in `iyakki-core-fake` (scripted frames), device implementation `iyakki-agent::embedcameras::AgentCameras` over the camera hub (F-CAM-02/16) and the motion watcher (F-CAM-04).

**What the person gets.** The list of the cameras they may watch; a live view of one camera, or a grid of up to 4 (one column on a compact pane, 2x2 on a wider one; Enter or a click enlarges one cell, Esc goes back to the grid); a snapshot button; pan, tilt and zoom buttons when the source has them; the owner's motion zones and recent motion boxes drawn over the picture, read-only; a REC mark on a camera that is being recorded; a fullscreen toggle; an error sentence in the cell of a camera that is lost, busy, needs its password or the video add-on or shows black; no sound.

**What you write** (the same pattern as `ChatView`):

```rust
let mut cams = CameraView::new(port, ViewConfig { cameras: vec![], label: "Doorbell panel".into(), ..ViewConfig::default() }, 40_000);
// event(Tick(t)): cams.tick(t);                      // polls the core, fetches frames at the view's own pace, never queues them
// event(Action):  if cams.owns(a) { cams.action(a); }    event(Key): offer to cams.key(k) while it is focused
// event(Resized / pane hidden): cams.set_visible(false)   // a hidden view pulls nothing
// view:           cams.draw(canvas, cx.colors(), &cx.metrics(), rect);
```

`CameraPort` is the app's handle on the core (one port is one person and one app): `list`, `open`, `decide`, `state`, `set_visible`, `frame`, `overlay`, `snapshot`, `ptz`, `events`, `close`. `MemCameras` is the in-memory port with scripted frames for tests and the browser preview; the core-backed port is `iyakki-apps::apps::cameras_port::CoreCameraPort` (asks `cx.core.cameras()`). `UiCx::now_ms` is the host's clock at the last heartbeat (0 before the first), for pacing only: pass it to `CameraView::tick`. The SAMPLE app's `with_cameras(port)` adds a Cameras tab (`with_cameras_for_the_wire` for JPEG pictures over a display list); the shell tile gives its host the tile's own clock on every heartbeat.

**The person's switch.** `Cameras::enabled`, `set_enabled` and `open_views` are the person's one switch for camera views in apps (on until they switch it off; kept in `<data>/cameras/embed-<person>.json`): off closes every open view of the person at once, nothing more is pulled, and `open` says "Camera views in apps are switched off for you". The chat tools `embedded_cameras_status` (free) and `embedded_cameras_set` (asks a card) are the way to change it (docs/development/CHAT-REACHABILITY.md); there is no Settings row yet. Colours, sizes and fonts come from `cx.colors()` and `cx.metrics()` only; targets are at least `m.min_target()`; layout is tested at 360x740, 800x480 and 1280x720 and at text size 3x.

**The rules, and where each is enforced** (every one has a test against the real hub with the loopback fake camera):

| Rule | Where | How |
|---|---|---|
| A person sees only the cameras they may watch | the core | `list` and `open` use the hub's rule (the owner, the people on the camera's list, this computer's own cameras for an account that may use them). A manager who may not watch sees nothing. A camera that is not yours is refused with the same sentence as one that does not exist |
| Consent | the core | `open` pulls nothing. The core's `CamEvent::Approval` card says "app wants to show 2 cameras: Front door, Yard" and what is not kept; the view draws Allow and Deny (`y` and `n` while it shows). A denied card shows nothing and starts nothing. No standing grant for an embedded view |
| An indicator while a view is open | the view and the core | the view draws a sign (a dot and "Live: Front door, Yard") while `ViewState::indicator` is true (allowed, open and visible); the core reports the same names to the machine's own sign (`embedcameras::viewing_names`) |
| No frames written anywhere by the view | the core | frames are handed to the app and kept nowhere (no file, no cache); the view keeps only the newest picture of each cell in memory |
| Snapshot only to the person's own folder, with a card | the core | one card every time; the file goes through the hub's snapshot (the person's Pictures, the account's write rule, the owner's privacy masks if they asked for them in stored pictures); the event names the file and the folder, not the path |
| Pan, tilt and zoom behind an approval | the core | the first command of a view for a camera asks, later ones run; a source that has none refuses with a sentence. **None of the hub's sources has a control channel today** (RTSP over TCP, Motion-JPEG, V4L2): `CameraInfo::ptz` is false on the device, the buttons are not shown, and the facet, the view and the fake are ready for the ONVIF client (F-CAM-22, planned) |
| Audit, by names only | the core | `cameras.open app: Front door, Yard`, `cameras.consent app allowed`, `cameras.snapshot app: Front door`, `cameras.close app` through the hub's audit sink (address and password scrubbed there). Never a frame, an address or a path |
| Limits | the core | 1 to 4 cameras per view, 4 views per person, 2 per app; a request over a limit is a sentence |
| Per-camera allow lists, per-person visibility | the core | the hub's: the list of people on a camera, and the owner's. The overlay (zones, masks, motion marks) is the owner's alone |
| Pause when hidden | the view and the core | `set_visible(false)` stops the view's polling and the core hands out nothing; the hub lets the stream go 8 s after the last look (the computer's own camera's light goes out then) |
| Low latency | the view and the core | the core hands out only the newest picture newer than the sequence number the view has; the view asks at most `fps` times a second (5 for a grid cell, 10 for a single large view, the hub's own caps) and keeps one picture per camera, so a slow viewer drops frames and never falls behind |
| No sound unless asked | the core | `ViewSpec::audio` true is refused with "camera views carry no sound in this system" (interleaved audio is F-CAM-20, planned) |

**Frames and the display list (built 2026-11-16, F-SDK-164).** A frame is `RGBA` (a canvas that draws pixels: the device and the desktop window) or a complete `JPEG` (a display list that travels to a browser or client). The view asks for the format its canvas takes (`ViewConfig::format`). The picture reaches the screen through one new canvas call, `Canvas::image(rect, &Image)`, and one new display list operation, `image`, additive in `DISPLAY_LIST_VERSION` 1: `image X Y W H FORMAT PIXELS_W PIXELS_H SEQ BASE64`. On 2026-11-16 neither `release/alpha.5` nor `alpha.5/q-embed-media` had an image operation (F-SDK-35 lists images as not built), so the camera view adds the smallest one; `AudioView` and `MediaView` must use it for video frames rather than invent a second. The operation is capped (the picture's bytes at 1 MB, the whole list at the list's existing 8 MB), refused by name by an older reader, drawn by `RasterCanvas` (RGBA; a JPEG draws nothing on pixels, so a view on a pixel canvas asks the core for RGBA), by the browser's Canvas2D renderer (`displaylist.js`: RGBA through ImageData, JPEG through `createImageBitmap`, decoded once per picture, a redraw when it has decoded) and by the client WASM engine (RGBA stretched into the framebuffer; the module has no JPEG decoder, so a JPEG is a dark block there until the page hands decoded pixels in; the module's size limit went from 600 to 608 KB for the operation). Tile ABI 1 has no picture operation: the tile build draws a dark block.
## Embedding audio and video: the options

Owner request 2026-11-16 (rows F-SDK-140 and up). An SDK app that wants to play a sound or a film has three ways. They are not exclusive: the sound-only path comes first because everything else needs it.

**What exists today.** Sound: in-house decoders for MP3 (`nanomp3`, a port of minimp3, permissive), Ogg Vorbis (`iyakki-audiodec`, own code, run as a helper program), WAV (trivial) and Opus (own CELT/SILK/hybrid code in `iyakki-audiodec`, not yet joined to the helper; until then the add-on decodes it). FLAC: tags are read, the sound is not decoded. AAC: only through the user-downloaded ffmpeg add-on. Everything audible goes through the core's mixer (`SourceKind::Media`), which applies the person's volume, ducks under speech and alarms, and feeds the sound doctor. Video: **no in-house video decoder exists.** The MEDIA app's player (F-MEDIA-08) uses the user-downloaded `ffmpeg` as a decoder (two processes, raw RGBA frames and PCM), the web engine decodes through the browser add-on (GStreamer, FFmpeg libraries built on the device from pinned sources on request, `AV-ADDON-K3S-BUILD.md`; none of it is shipped). The display list has no image operation yet.

### Option A: native `AudioView` and `MediaView` through a core `media` facet (recommended, built first)

The core decodes; the app never sees a byte of the file or stream. The facet (`iyakki_core_api::media::Media`, `Core::media()`, default `NoMedia`) opens a source (a file of the person's home folder under the Files home-folder policy, an http(s) address through the core's network guard, or a station of the person's radio list), controls it (play, pause, seek, speed, volume, mute, track) and hands out events (state, position, level, errors in sentences). One session per person and app, at most four sessions per person. A remote source shows an approval card first; audit lines carry the kind of source and the app, never a name or an address. The sound is written into the mixer as a `Media` source, so volume, ducking and the sound doctor apply without the app doing anything, and the position the app shows is what the mixer reports as heard (`played_ms`), which is also the clock a picture is paced against. Video frames (a later slice) come back to the widget as finished images: the core converts YUV to RGB, shows the frame whose time has come on the audio clock and drops late ones. The widget draws them through the display list's image operation (`Canvas::image`, added by the camera lane; this lane reuses it and adds no second one).

Costs: a new facet and an adapter on the device (done for sound: F-SDK-140, F-SDK-141); a renderer change in the browser and WASM renderers for the image operation (F-SDK-146); decoding stays in the core process or a helper process, so a bad file hurts the helper, not the app. Licences: nothing new for sound (all in-house or permissive and already in `legal/`). Video needs a decoder: there is none in house. The options are an in-house VP8/VP9 or AV1 decoder (a large piece of work, no licence cost, patents are royalty-free for these codecs), or the add-on route (frames from a decoder process in an isolated pod or process, LGPL libraries built on the device on request; H.264 and AAC carry a patent note the person is shown). The facet's video path is written against a frame source trait so either plugs in; until one does, a video file is refused with a sentence and the row says so.

### Option B: video inside `BrowserView` pages (the full engine)

An app that only needs to show a page with a player (a lecture, a stream) embeds `BrowserView` and the web engine plays the page's `<video>` and `<audio>` itself: nothing to build in the SDK beyond the BrowserView work. Costs: the full engine and the browser add-on must be present (an image without the engine cannot do this), the whole page runs rather than one player, and the app gets no transport controls of its own: it cannot show its own seek bar, captions list or level meter, cannot read the position, and the sound reaches the mixer through the browser's own route (`browsermedia`), not through the app's session, so the per-app volume and the per-app audit line do not exist. Licences: the engine's own and the add-on's LGPL libraries, built on the device, never shipped. Good for "show this page"; not a replacement for A.

### Option C: a `<video>`-like element for the web and WASM renderers

In a browser tab the display list is drawn on a Canvas2D by `ui/displaylist.js`, and the client engine draws it into a framebuffer. Two ways to show video there: (1) a new display list operation `media <id> x y w h` that the Canvas2D renderer backs with an `HTMLMediaElement` (the browser decodes, using its own codecs and the person's own browser licences; the core hands over an address on a short-lived, signed, per-person route instead of the file), and (2) decoded frames sent as images for the client engine, which has no `HTMLMediaElement` (frame size and rate then decide the bandwidth: it is the pixel-streaming cost, acceptable at small sizes). Costs: two renderers to keep equal, a signed media route in the core, and the sound then plays in the browser and bypasses the device mixer (the person hears it on the client, which is right for a remote client but means the sound doctor and ducking of the machine do not apply). Licences: none for us in (1); in (2) the same decoder question as A.

### Recommendation

1. Build A for sound now (done in slices 2 and 3): `AudioView` is useful at once, needs no new decoder, and fixes the facet's shape (sessions, approvals, audit, mixer clock).
2. Make the video path of A the single native way. Pick the decoder by cost: use the add-on route first when its package is installed (it exists as a design; frames from an isolated process), and write an in-house VP8/VP9 decoder as its own row (no licence or patent cost) so an image without the add-on can still play WebM.
3. Use the display list image operation (needed by A; the camera lane adds it)  in option C's second form for web clients. Option C's first form (a real `HTMLMediaElement`) only after A works, for browser tabs where the client's own decoder is better than ours.
4. Option B stays what it is: the way to show a page that happens to contain a player.

Rows: F-SDK-140 (facet, fake, home policy), F-SDK-141 (device adapter, sound: WAV, MP3, Vorbis; Opus through the add-on), F-SDK-142 (planned: in-house FLAC decoder), F-SDK-143 (planned: video path), F-SDK-144 (planned: internet addresses through the network guard), F-SDK-145 (planned: widgets), F-SDK-146 (planned: sample pane, renderers, chat tools, Settings), see `FEATURES.md`.

## Embedding the sound player (AudioView)

Rows F-SDK-140 to F-SDK-149. The first built part of option A above: a sound player an SDK app places in its layout, the same way as `ChatView`, `FilesView` and `BrowserView`.

**What you write.** Keep an `AudioView` in your app state, give it the core's `MediaPort` (the host hands it to you) and an action number range (`audioview::ACTIONS` numbers from a base you choose):

```rust
let audio = AudioView::new(port, 54_000);
// event(Tick):    audio.tick()                      lists the person's sound files once, then takes what the core says (also while another pane shows)
// event(Action):  if audio.owns(a) { audio.action(a) }      event(Key): if audio.key(k) { return }      event(Wheel): audio.wheel(n)
// view:           audio.draw(canvas, cx.colors(), &cx.metrics(), rect)
// your own start: audio.open(Source::Home(name), "Title")    // a file of the home folder; Source::Stream(url) and Source::Station(name) ask the person first
```

`iyakki-app-sample` is the working example (the Sound pane, `with_media`; key `u`). **What the view does:** the transport (play or pause, 10 seconds back and forward, a seek bar of ten tappable parts, mute, volume down and up, speed 1x, 1.25x, 1.5x, 2x, 0.75x), the time and length (or LIVE), a level meter, the track list when there is a choice (a second audio track, captions), the approval card for a remote source (Allow, Deny), a refusal as a sentence, and the person's list of sound files. Keys: Space, Left and Right (5 seconds), Up and Down, Enter, `+` and `-`, `m`, `r`, `n` and `p`, Esc (stop). It goes on to the next file when one ends. Everything is drawn with the host's colours and metrics: it was checked at 360x740 touch, 800x480 and 1280x720, at normal and 3x text.

**Where things run.**

| Piece | Where | Rule |
|---|---|---|
| Cameras, streams, decoding, access rules, snapshot | the core (`iyakki-agent::embedcameras` over `cameras::Hub`) | your app never sees an address, a password, a socket or a device |
| Motion zones and events | the core's detector (F-CAM-04) | read-only to the view |
| The picture, the cards, the layout | your app (`CameraView`) | draws only through `Canvas`, never writes a file |

**Testing.** `FakeCore::cameras_fake()` (`FakeCameras`): `add(FakeCam::new("front", "Front door", "amy").allowing(&["bob"]).with_ptz())`, `push_solid(id, w, h, rgb)` for a frame, `push_motion`, `set_zones`, `no_screen(true)` (every card is answered "no" by the system), and `pulled(id)`, `audit()`, `snapshots()`, `indicator_names()` to prove what the core did. The device's own tests run the facet over the real hub and the in-house loopback RTSP camera with real JPEG frames (`iyakki-agent::embedcameras::tests`).

**Not built yet.** The WASM tile build (the tile `call` has no `cameras` domain), the facet over the contract socket for a UI program outside the process, pan, tilt and zoom on a real camera (F-CAM-22), camera sound (F-CAM-20), sharing one view between two apps, and the machine's red sign counting embedded views (the names are reported by `viewing_names`; the shell's sign does not read them yet).
| Reading and decoding the file or stream | the core (`iyakki-agent::embedmedia`, helper `iyakki-audiodec` for Ogg Vorbis, Opus and FLAC, `nanomp3` for MP3, own WAV) | your app never gets a byte or a path; names are relative to the home folder |
| The sound | the core's mixer, a Media source named after the app | the person's volume, ducking under speech and alarms and the sound doctor apply; the player's own volume and speed are per session (the pitch follows the speed) |
| Position and meter | the core: what the mixer says was heard | the widget never keeps its own clock |
| Approvals | the core | an internet address or a station: a card before anything is fetched; a refusal is a sentence |
| Limits | the core | one session per person and app, at most four sessions per person, files up to 64 MB; formats WAV, MP3, FLAC, Ogg Vorbis, Opus |
| Switch | the person | `embedded_media_status` and `embedded_media_set` (chat); off stops every player |
| Audit | the core | `media.open`, `media.decide`, `media.switch`, `media.close`: the app and the kind of source only |

**Testing.** `MemMedia` is the in-memory port: `put_file`, `put_stream`, `advance(ms)` moves its clock, `calls()` lists what the view asked. Against the core's own fake: `FakeCore::media_fake()`, with the same rules. The real player is tested on committed fixtures through the real mixer (`embedmedia_tests`).

### MediaView: video (F-SDK-143, F-SDK-149)

`MediaView` is an `AudioView` with a picture, used the same way (`MediaView::new(port, base)`, the same four lines, `mediaview::ACTIONS`); SAMPLE's Media pane is one. The picture is drawn through the display list's image operation (`Canvas::image`, F-SDK-164, letterboxed in a surface above the transport); tap it to play or pause; `f` or the Full screen button shows only the picture and a Play or Pause and Exit bar, and Esc leaves full screen before it stops the player. A sound-only source shows the audio layout. `MediaPort::frame(after)` asks the core for the picture that is due; `MemMedia::put_video` makes one for tests.

**What the core does with video today.** Motion-JPEG in an AVI file of the person's home folder (camera recordings, most webcam clips), nothing else:

| Step | Where | Rule |
|---|---|---|
| Container | `iyakki-agent::avimjpeg`: the RIFF chunks of the AVI, the video stream, the PCM sound | a file over 64 MB, more than 200 000 pictures, or sound that is not 8 or 16 bit PCM is refused in a sentence |
| Pictures | the browser's own JPEG decoder (`iyakki_web::jpeg`), RGBA, scaled down in the core to at most 250 000 pixels (under the 1 MB image limit) | the app gets finished pixels, never the file |
| Clock | the file's sound, played through the mixer; the position is what the mixer says was heard | a video without sound is driven by silence of its own length |
| Pacing | `Pacer`: the picture whose time has come, never the same one twice, the late ones counted and dropped (never queued) | after a seek the next picture shows at once; a paused video keeps its picture once the mixer's queued sound (about 300 ms) has played out |

**Missing, and said in a sentence to the person.** H.264, MPEG-4 (so most files from phones and the web), VP8, VP9 and AV1: there is no in-house decoder for them, and the AV add-on route (frames from a decoder process in an isolated pod) is a design only. Video from the internet (only AVI files of the home folder play). Caption text (the track list exists, the facet carries no text). AVI1 camera frames that omit the Huffman tables (the fix-up is in iyakki-apps). Picture quality beyond nearest-neighbour scaling.

**Internet addresses (F-SDK-144).** `Source::Stream(url)` and `Source::Station(name)` ask the person first, then the core opens the address with the account's host policy (`allows_host`, the list the radio uses), the streaming HTTP transport and a 200-answer check; a host outside the policy is refused with "your account may not reach host". MP3, Ogg Vorbis, Opus, FLAC and WAV streams play; HLS and AAC streams do not (the radio has them).

**Not built yet** (row F-SDK-147): the browser renderers were not run on a media picture (the camera lane tests the image line in real Chrome), there is no Settings control for the switch (the chat tools are the way), and the facet is served in process: a UI program outside the process sees `NoMedia`.

## What works today

Tested (on fakes and in memory; the tests are named in `FEATURES.md`, rows F-SDK-17 to F-SDK-19): the `Canvas` trait with the pixel and list canvases; the display list, its text format, limits and refusal rules; replay equals direct drawing; the hit regions and accessibility tree; gestures; size classes and metrics; the widgets at the three reference sizes and 3x text; the dock, wallpaper, screen and view model including unplug; the host driver; the SAMPLE app in the SDK harness and as a shell tile (drawn into a pixel canvas in a test).

## What is not wired (be honest about these)

- **The Markdown viewer is a tile (F-SDK-14, 2026-11-16).** `crates/iyakki-app-markdown` is registered as `App::Markdown` (policy and role templates, memory and cloud classes, the launcher row, the shell adapter `apps/markdown.rs`). `markdown.view` is wired: the bridge's viewer backend (`PendingViewer`) keeps the document for the person in the core's app data (up to 5 documents, about 56 KB together), the app list says how many documents wait, and the tile shows the oldest one when the person opens it. `markdown.openFile` (the person picks a file) still answers `Unavailable` with a fix line (F-SDK-14's split row).

- **The launcher, cloudmode and contract tests of the alpha.4 cut were re-run in alpha.5** (2026-11-16) and pass.

- **Browser renderer: built; the client WASM engine draws the list too (F-SDK-22), text excepted.** The module `iyakki-client-engine.wasm` reads the list with the SDK's own reader, draws the shapes into a framebuffer, answers with the accessibility tree and the text runs (it has no fonts, so the page draws the text with its own), and makes the input lines (`ui_load`, `ui_hit`, `ui_line` in `js/engine.js`); `python3 dist/browsertest/run_uisdk.py --preview-bin PATH` runs it in real Chrome after the JavaScript renderer's checks. `ui/displaylist.js` draws the list on a Canvas2D, builds the ARIA tree and sends input lines back (`iyakki_ui_sdk::wire`); `iyakki-ui-preview` serves an app to a tab on 127.0.0.1 (`cargo run -p iyakki-ui-preview`, then open the printed address). It was run in a real Chrome by `python3 dist/browsertest/run_uisdk.py --preview-bin PATH` (18 checks). The client WASM engine renderer is F-SDK-22. No page of the web UI uses the renderer yet; nobody has watched it on a phone.
- **Not seen on a real screen.** The SAMPLE tile was drawn in tests only (Manual: needed).
- **The dock, wallpaper and storage ports of the SAMPLE tile are the core's (2026-11-16).** The tile asks `cx.core.app_data()` and `cx.core.desktop()` (`iyakki-core-local::uistate`): its notes come back at the next start, the pin button changes the person's pinned dock apps (only apps the person may open) and the picker changes the person's wallpaper, each audited. One dock and one wallpaper per person: per-screen choices are not saved and the shell's real pane manager does not use `Pane` and `ViewInfo` (the split row of F-SDK-18).
- The text measure of `ListCanvas` is a fixed estimate unless the recorder supplies the real one (`ListCanvas::with_measure`); a client that lays text out itself will differ slightly. Text runs with clustering, `Blur`, `Path`, layers and the keyboard-composition events of the design are not in display list version 1.
- Drag and drop is not built. The on-screen keyboard hook is `UiApp::wants_keyboard` / `Host::wants_keyboard` (true while a field has focus); the shell does not yet show its keyboard from it. A WASM tile build of an SDK app exists (F-SDK-23, `iyakki-sdk-tile`): see "As a WASM tile" below.
- Multi-screen and multi-view are a model plus the host's `move_to` and `unplug_screen`; the shell's real pane manager does not use them yet.

## Embedding the browser (`BrowserView`, F-SDK-60 to F-SDK-65)

An SDK app can place the IYAKKI Browser in its layout like a web view widget. The app never touches a socket or a device: the view asks the core for a *session* (one page and its history for a person and an app), and the core judges every address, loads the page, and answers with a display list that the view replays inside its rectangle. So the shell, a client and a WASM tile draw the same page.

```rust
let mut view = BrowserView::new(port, 20_000)          // port: the core's (the host hands it); 20_000: the first of 10_000 action numbers the view uses
    .with_engine(Engine::InHouse)                       // InHouse, Full or Auto (the state says which one was used)
    .with_size_policy(SizePolicy::Fill);                // Fill, Fixed(w, h), Height(h) or Aspect(ratio)
view.load_url("example.org");                           // or view.load_html("about:help", "<h1>Help</h1>")
// in event():  if view.event(&ev) { return Flow::Stay; }   then   for e in view.take_events() { ... }
// in view():   let used = view.draw(canvas, cx, area);
```

| Call | What it does |
|---|---|
| `load_url`, `load_html`, `back`, `forward`, `reload`, `stop` | navigation; an address is judged by the core (known-bad list, the person's host policy, http and https only) and a refusal arrives as `BrowserEvent::Failed(Some(why))` |
| `find(query, forward)`, `find_state()` | find in page: marks the matches, scrolls to the current one, says "n of m" |
| `url()`, `title()`, `is_loading()`, `can_back()`, `can_forward()`, `error()`, `engine_used()` | state, also sent as events: `Opened`, `Title`, `Url`, `Loading`, `Failed`, `Find`, `Prompt`, `Closed` |
| `event(&UiEvent)` | link taps (`Action` in the view's range), the wheel, Up, Down and Space while the view is focused (`set_focused`), typed text, `Tick` (state and frame refresh) |
| `prompts()`, `answer(allow)` | a download or permission question the page raised: a bar under the page with Allow and Deny; the answer goes to the core, never to the page |
| `draw(canvas, cx, area)` | draws inside `area` by the size policy and returns the rectangle used; when the core gives no session the view says why and offers Try again |

What the core does (`iyakki_core_api::BrowserSessions`, reached as `cx.core.browser()`): `open`, `close`, `navigate`, `input`, `find`, `state`, `frame` (display list text and a generation), `prompts`, `answer_prompt`, plus the person's switch (`enabled`, `set_enabled`) and `open_views`. A session belongs to a person and an app: another person or app gets the same "closed" answer as for a view that never existed. Up to 8 sessions per app. The real implementation is `iyakki-core-browser` over the in-house reader engine (pages load on a worker thread; a file that is not a page becomes a download question; page content the app brings is never fetched and its links are still judged); the device adapter is `iyakki_agent::embedbrowser` (the transport behind the known-bad list and the ad and tracker filter, audit lines `browser.embed.*`). The scripted fake is `FakeCore::browser_fake()`; the SDK's own `MemBrowser` serves tests and the preview.

**Themes, fonts and text size.** The page chrome uses only the Look's colours (`cx.colors()`), handed to the core as a palette at open; when the Look or the text size changes the view opens a new session and goes back to the address. The core also gets the page defaults: the colour scheme (dark, light, high contrast), the default text size and family, and the person's reduced-motion choice. The in-house reader has no animation; it follows high contrast with thicker underlines, rules and find marks in the text colour, wraps at any width and scales to 3x text. A full engine would take the same defaults as `prefers-color-scheme`, a forced-colours hint, default font size and family. When the SDK design tokens and font roles land, the view switches to them (it uses `m.font(size)` and `cx.colors()` today).

**What the person can switch.** Settings -> Browser (chat tools `embedded_browser_status` free, `embedded_browser_set` asks every time; see CHAT-REACHABILITY.md): embedded views on or off for the person; off closes the open views at once.

**In a WASM tile and in the client (F-SDK-64, F-SDK-108).** A tile's `browser.*` calls are served by the web service over the core's sessions for the tile's own id (`webservice/tilebrowser.rs`, `ports::CoreBrowser` on the tile side), and the frame a session gives is the display list text the client engine draws as it is (`iyakki-client-engine/tests/browser_frame.rs`: the text and the link come through, a tap on the link is the action `activate` takes).

**Not built yet.** The full engine cannot be embedded (the core answers "cannot be embedded yet"); forms and typing into page fields; saving a download (the host's `save_download` says "not switched on" until the Downloads service is wired); the account's host policy is a hook on the device core (`set_policy`) that the shell does not set yet, so the known-bad list is the guard that runs on the device today; nothing was seen on a real screen.

## Versions

`UI_SDK_VERSION` (the traits and types) and `DISPLAY_LIST_VERSION` (the wire format) are both 1 and are listed in `compat-surfaces.txt`. Changes are additive; a breaking change is a new number and a fixture folder.
