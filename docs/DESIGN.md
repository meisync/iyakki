# IYAKKI visual language

Every app draws itself with `Gfx` (software 2D, `tiny-skia`). RADIO was the first app given a "futuristic" look; this file
describes that look, how the other apps differed before, and the shared module that now carries it so apps do not copy
colours and radii around: **`crates/iyakki-shell/src/fx.rs`** (plus three small helpers in `gfx.rs`).

`cargo run -q --release -p iyakki-shell --example screens [outdir] [WxH]` draws every app (and the pages of the apps that
have tabs) to PNGs in `/tmp/iyakki-screens` and prints the draw time of each at 1280x720. Look at the PNGs after changing
anything here.

## 1. What RADIO looks like

| Part | Value |
|---|---|
| Background | vertical gradient `BG_TOP (20,25,44)` to `BG_BOTTOM (9,11,20)` |
| Panels | `PANEL (8,11,26)`, near-black glass; rows and cards one step lighter `CARD (30,37,62)`, hover `CARD_HOVER (40,49,80)`, selected `CARD_SEL (48,58,96)` |
| Neon | `CYAN (70,225,255)` for chosen things, `MAGENTA (255,80,190)` for live/moving things, mixed along bars (`mix(CYAN, MAGENTA, t)`) |
| Text | `TEXT (231,235,247)`, `MUTED`, `DIM`; status colours `OK (110,224,160)`, `ERR (255,120,120)` |
| Glass card | 22 px radius, fill `PANEL`, a 3 px outer halo of the accent at low alpha that brightens with activity, 1.5 px border in `mix(CYAN, MAGENTA, .35)` at alpha 90 to 210 |
| Selected row | `CARD_SEL` fill, 1.2 px cyan outline (alpha 160); list rows are 12 px radius, side navigation rows 10 px, with a cyan wash (alpha 46) + 1.3 px outline for the chosen one |
| Chips / tabs | full pill (height 32), cyan wash and 1.5 px outline when chosen, muted text otherwise |
| Buttons | pill (height 2.1 x text size), transport buttons are circles with a ring; the main one glows in the accent |
| Section labels | 12 px bold, capitals ("LANGUAGES"), `DIM` |
| Typography | title 28 bold, subtitle 15, body 14-17, caption 11-12, mono for numbers and addresses |
| Spacing | 24 px side margin, 12-18 px between panels, 6 px between rows |
| Animation | spinning tick ring and needle on the dial, level bars that rise at once and fall slowly with a held peak, scrolling song title, a small bar animation on the station that is on. Only drawn while music plays |

## 2. Where the other apps differed (before this work)

| App | Deviation from RADIO |
|---|---|
| Header (all apps) | flat `rgb(14,18,34)` strip, 2 px solid accent line, plain icon, no subtitle |
| Background | plain two-colour gradient, no depth |
| Buttons (all apps) | flat grey-blue pills, outline only on hover, no glow, same look for danger and secondary except text colour |
| SETTINGS | flat cards (`CARD`), plain slider track, green/grey switch, rows with a flat accent stripe, no inline actions on the Models rows (users could not find Delete) |
| SYSTEM | flat cards and bars, page tabs as buttons with an underline, process list header in dim text, confirm dialog in a different blue |
| USERS | permission chips in brown/green blocks, flat detail text, no card |
| RAG, RUNS, MCP, SKILLS, MESH | flat rows with a 4 px stripe, text-only empty states and statuses, panels in unrelated dark blues |
| CALENDAR | button tabs, plain clock face, amber/grey switch |
| SCAN / NET SCAN | filter pills in a different blue, flat detail panel, brown chip for "NEW" |
| USAGE | flat range buttons, chart well in an unrelated blue, text-only empty chart |
| INBOX | button tabs, text-only empty pages, dialog in a different blue |
| CLI | button tabs, flat terminal box, flat progress bar |
| EDIT | flat tab strip, toolbar chips, status bar; text area correct but unframed |
| BROWSER | flat tool bar, tab strip and status line; panels in unrelated blues; page content unchanged by design |
| Forms, pickers, confirm dialogs | flat `rgb(24,30,52)` card, 2 px accent outline, red box for confirms |
| Sign-in | flat card with a drop shadow |
| Launcher | flat tiles, blue chip |

## 3. The design system (`fx.rs`)

Everything takes the UI scale `s` (so Settings -> Display text size keeps working) and only draws. Click regions are only
registered by the helpers that say so (`tab_strip`, `modal`), exactly as before, with the same labels.

**Palette.** `CYAN`, `MAGENTA`, `PANEL`, `WELL (18,24,46)`, `AMBER`; `mix`, `with_alpha`, `over` (a translucent colour
over an opaque one), and the contrast maths `luminance`, `contrast`, `readable(fg, bg, min)` (nudges a colour towards white
until it reads at least `min`:1). The grey text colours were lifted to `MUTED (165,175,202)` and `DIM (132,142,174)` so that
they read at 4.5:1 or more on every card (a test checks this). Each app keeps its own `accent()`; apps pass it to the helpers.

**Background.** `fx::background(g)` draws the gradient, two faint neon corner glows, a fine grid, scanlines and a scatter of
particles once per screen size into a cache and copies it afterwards (a memory copy). The shell calls it instead of the old
gradient. Nothing in it moves.

**Header.** `fx::app_header` (called by the shell for every app): dark gradient band, the icon in a glowing disc, the title
(20 bold), the tile's tagline as a subtitle, and `fx::accent_line`: a 2 px line in the app's accent fading to the right with a
short bright highlight that slides along it as frames are drawn. The shell does not ask for extra frames for it, so an idle
screen still costs nothing; set `IYAKKI_REDUCED_MOTION=1` (or `fx::set_reduced_motion(true)`) to freeze it.

**Cards and rows.** `fx::card` (glass card, shadow, top highlight, accent border, optional glow `0..1`), `fx::panel` (darkest
well for charts, terminals, detail panes), `fx::well`, `fx::glass` (cheap resting tile for many-per-screen cards),
`fx::list_row` / `fx::row_at` (rest, hover, selected with neon outline and accent edge), `fx::tile` (launcher tiles).

**Buttons and tags.** `Gfx::button`, `icon_button` and `button_off` now draw through `fx::pill`; the variant follows the
accent you pass: `ERR` is a danger button (red glass), `MUTED`/`DIM`/`TEXT` are secondary (quiet outline), anything else is
primary (tinted fill, neon outline, glow on hover). `fx::chip` (neon tag, optional filled), `fx::toggle_chip`, `fx::tab_strip`
(page tabs: lit chosen tab with an underline; registers one hit per tab labelled with its text), `fx::chrome_button` (header
buttons).

**Sections and data.** `fx::section_title` (spaced capitals and a fading accent rule), `fx::kv_row`, `fx::table_header`,
`fx::progress_bar` (glowing head), `fx::ring_gauge`, `fx::switch`, `fx::scrollbar`, `fx::empty_state` (icon in a soft ring,
title, hint), `fx::status_line` (dot + text), `fx::toast`.

**Dialogs.** `fx::modal(g, area, s, w, h, accent, danger)` clears the click regions behind it, dims, registers the `backdrop`
region and draws the glass panel with a glowing border; `modal_title` and `modal_panel` (picture only). `Form`, `Picker`,
`render_confirm` and the F1 help, power menu and sign-in card use them. Text fields use `fx::field_frame` (cyan glow when
focused).

**Sizes** (before scaling, `fx::size`): title 28, header title 20, heading 17, body 14, caption 12, tiny 11; radii card 16, row
12, field 8, pill = height/2, tile 26, dialog 18; margin 24, gap 12.

**Rules of thumb.** Text on a card is `TEXT` or `MUTED` or a `readable()` accent (4.5:1). Highlight with the app's accent,
signal "live" with `CYAN` to `MAGENTA`. Animate only small things and only while something is happening; never request frames
for decoration. The static background is cached; do not paint full-screen gradients per frame. Keep hit labels and keys when
restyling. The editor's text area and the browser's page content stay as they are (legibility first).

## 4. For the CHAT and MEDIA owners

CHAT and MEDIA were not restyled here beyond the shared pieces they already use (header, background, buttons, fields, forms
and, for MEDIA/RADIO, `apps/neon.rs`, which now re-exports the palette from `fx`). To adopt the rest:

- composer: `fx::card(g, r, s, accent, glow)` with the field inside via `TextField::render` (already glowing when focused),
  send as `g.button(..., accent, ...)`, a footer line with `fx::chip` for the model and usage;
- conversation list rows: `fx::row_at`; the sidebar header `fx::section_title`;
- empty conversation: `fx::empty_state`; toasts: `fx::toast`; status: `fx::status_line`;
- MEDIA lists: `fx::list_row` + `fx::progress_bar` for the seek bar, `fx::tab_strip` for the browse tabs (labels stay as they
  are), `fx::chip` for type tags, `fx::scrollbar` for long lists.

## 5. Draw times at 1280x720 (release build, milliseconds per frame, best of 5)

"Before" for SIGN-IN and the SETTINGS pages was measured with the old code. For the other apps the first pass of the tool
could not open them, so their "before" is the same screen drawn with the app bodies still unchanged (only the new backdrop
and header in place); the old flat backdrop and header cost about the same as the new cached ones. SYSTEM -> Processes was
slow before (it cloned every process and fitted kilobyte-long command lines); both were fixed here.

| Screen | Before (ms) | After (ms) |
|---|---|---|
| 00-login | 3.92 | 4.62 |
| 01-launcher | n/a | 4.39 |
| settings | 4.44 | 1.48 |
| settings-01-rag | 4.77 | 1.83 |
| settings-03-skills | 4.52 | 1.43 |
| settings-09-voice | 4.55 | 1.37 |
| settings-02-vault | 4.18 | 1.08 |
| settings-10-secret_providers | 4.13 | 1.00 |
| settings-12-account | 4.50 | 1.28 |
| settings-04-network | 4.27 | 1.15 |
| settings-05-bluetooth | 4.12 | 1.02 |
| settings-06-display | 4.49 | 1.45 |
| settings-07-sound | 4.31 | 1.27 |
| settings-08-storage | 5.39 | 1.81 |
| settings-11-browser | 4.78 | 1.64 |
| system | 1.55* | 1.34 |
| system-01-processes | 134.88* | 1.62 |
| system-02-agents | 0.80* | 0.94 |
| system-03-hardware | 1.19* | 1.06 |
| users | 0.93* | 1.62 |
| rag | 0.84* | 1.30 |
| runs | 0.62* | 0.81 |
| mcp | 0.84* | 0.76 |
| skills | 1.30* | 1.43 |
| calendar | 1.54* | 2.96 |
| calendar-01-clock___alarms | 1.54* | 3.23 |
| scan | 0.85* | 1.08 |
| scan-01-model_servers | 0.85* | 1.08 |
| scan-02-cameras | 0.85* | 1.09 |
| scan-03-media | 0.85* | 1.10 |
| scan-04-devices | 0.85* | 1.10 |
| scan-05-risks | 0.85* | 1.09 |
| mesh | 0.77* | 0.85 |
| mesh-01-nodes | 0.77* | 0.89 |
| mesh-02-key_value | 0.77* | 0.91 |
| mesh-03-setup | 0.77* | 0.85 |
| usage | 0.96* | 1.56 |
| usage-01-week | 0.96* | 1.54 |
| usage-02-month | 0.96* | 1.54 |
| usage-03-3_months | 0.96* | 1.54 |
| usage-04-year | 0.96* | 1.57 |
| usage-10-only_me | 0.96* | 1.56 |
| inbox | n/a | 1.08 |
| inbox-01-tasks | n/a | 1.13 |
| inbox-02-approvals | n/a | 0.97 |
| cli | 0.76* | 0.91 |
| cli-01-clis | 0.76* | 1.52 |
| edit | 0.91* | 1.14 |
| radio | 3.05* | 2.84 |
| browser | 0.81* | 1.49 |
| browser-03-start_page | 0.81* | 1.50 |
| browser-04-go | 0.81* | 1.50 |
| chat | 1.03* | 1.25 |
| media | 1.96* | 1.82 |

`*` = measured with the app body unchanged and only the new backdrop/header (see above); pages of an app use their app's figure. Slowest screen now: 4.6 ms (limit 15). RADIO tabs and MEDIA pages are within 0.1 ms of their first screens. Measured on a busy shared machine (load average 6 to 20), so treat sub-millisecond differences as noise; a few small apps (USERS, CALENDAR, BROWSER) are now about 2 to 2.7 times their sub-millisecond old time, all under 5 ms.

