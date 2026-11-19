# Look: themes that change more than colours (alpha.4 design)

Owner request 2026-11-15: "theme (not just dark or white, icon, layout, the top bar, location, menu customized)".

Today a theme is a colour file (`iyakki-uikit::usertheme`, `<name>.theme`: palette keys plus `ring`, with contrast checks) and, since Hari's wallpaper work, a wallpaper. This design makes a theme a **Look**: one small, validated file (or store package of kind `theme`, F-APPSTORE-16) that describes the whole desktop.

## What a Look holds

| Part | Keys (first version) | Notes |
|---|---|---|
| Colours | the existing palette keys, `base`, `ring` | unchanged; the 4.5:1 / 3:1 contrast rules stay |
| Wallpaper | `wallpaper = daylight` (a built-in name, later a picture file) | per account today (Hari) |
| Icons | `icon_shape = squircle / circle / rounded / square`, `icon_size = S/M/L or px`, `icon_style = gradient / flat / outline / glass / 3d`, `icon_labels = on / off`, optional `icon_set = <name>` (a glyph set: the built-in one first) | slice 1 of the launcher draws squircles; make the shape a parameter, not a constant |
| Launcher layout | `launcher = paged / grid / list`, `columns = 3..8`, `density = compact / comfortable / roomy`, `page_dots`, `groups = tabs / sidebar / none`, `island = on / off` | the paged island is the default Look |
| Dock | `dock = bottom / left / right / off` (default bottom), `dock_items = [pinned apps, in order]` (the person's chosen apps for easy launch, like other desktops), `dock_running = on / off` (a dot under apps that are open), `dock_size`, `dock_magnify = on / off`, `dock_autohide = on / off`; a click opens or raises the app; drag to reorder; a pinned list per account | the dock is a shell part; its apps come from the apps the person is allowed to use (role gating) |
| Bars | `topbar = on / off`, `topbar_position = top / bottom`, `topbar_items = [clock, battery, network, bell, apps, ...]` in order, `topbar_size`, `footer_hints = on / off`, optional `dock = left / right / bottom / off` with its items | items are a closed list drawn by the shell (no code in a theme) |
| Menu | `menu = [entries in order]`, `menu_position`, which entries are hidden | only entries the shell offers; a Look can reorder or hide, never add a command |
| Home widgets | `widgets = [attention, approvals, vitals, runs, tokens, network]` and their sizes and order | slice 4 of the launcher |
| Motion | `motion = full / reduced` (the reduced-motion setting always wins) | |

**Icon styles (owner 2026-11-15: 3D and glassy icons).** All drawn by in-house code, no picture files in the image: *glass* = a translucent body over a blurred copy of what is behind it, a bright rim, an inner highlight and a soft shadow (needs the cached blur of launcher slice 3; until then the glass body uses a gradient fake); *3d* = layered gradients with a bevel, a specular highlight, a contact shadow and slight depth under the glyph. The same two styles apply to the dock icons. Contrast of the glyph over the body is checked per theme.

## Rules that keep a Look safe (checked when it is loaded; a Look that fails is refused with the line and reason, and the person keeps the one they had)
1. **Essential controls cannot be hidden or moved out of reach:** sign out / lock, power, the notification bell, Settings, and the "something needs you" indicator stay reachable in every Look (the validator adds them back and says so).
2. **Contrast** stays checked for text over the wallpaper and the island in every theme-by-wallpaper pair (the launcher agent already has a test that does this over black, white and grey).
3. **No code and no files outside the theme folder:** a Look names built-in parts only. A custom icon glyph set or wallpaper picture is a data file in the same folder, size-capped, parsed by in-house code, never executed.
4. **Accessibility wins:** the larger-text size, screen-reader mode and reduced motion overrule a Look; the High contrast look cannot be overridden by a custom one for the sign-in screen.
5. Per account, applied at sign-in and live in Settings; the sign-in screen is always the default Dark look.

## How it is built (order)
1. **Make the shell data-driven:** the top bar, the menu and the launcher read a `Look` struct (defaults = today's behaviour exactly), so no Look = no visible change. This must happen **in the launcher slices already in progress**: the icon shape, layout and bar items become fields, not constants. (Branch `alpha.4/launcher`; tell its owner.)
2. **The Look file format v2** extends `usertheme.rs` (a `[section]` per part, still `key = value`, still plain text) and keeps reading old `.theme` files unchanged.
3. **Settings -> Appearance:** pick a Look; edit its parts with live preview (a preview pane that draws the launcher and bars at the chosen settings); Save as my Look; export/import. Replaces the Theme card plus the wallpaper hint (the wallpaper gets buttons here).
4. **Chat tools** for every setting (`display_look`, `display_look_set`, with the approval card for changes of bar items), per the parity rule; access log line.
5. **Store:** Looks as packages of kind `theme` (signed like the rest), listed in the STORE; three built-in Looks to start (Default, Compact, Classic = today's layout).

## The file format v2 (written 2026-11-15, `iyakki-uikit::look`)
Plain text. `#` starts a comment, `key = value`, lists are `[a, b, c]`, one `[section]` per part. A file with no sections (the old `.theme` format) is read unchanged as a Look with only colours. Limits: 8192 bytes, 300 lines, 400 characters a line, 24 items in a list. Every key belongs to one section; a key in the wrong section, an unknown key, a key written twice, a word outside its closed list and a number out of range each refuse the whole file with `line N: reason`, all problems listed together. Files are `<name>.look` in `<data>/themes/<person>/` or `<data>/themes/` (the person's own shadows a shared one; `.theme` files are listed too; the names `default`, `compact`, `classic` belong to the built-in Looks).

| Section | Key | Values (default = today) |
|---|---|---|
| `[look]` | `name` (1 to 32 letters, digits, space, `-_.`), `format` | `format = 2` |
| `[colours]` | `base` `dark/light/high-contrast`, `ring` 1.0 to 3.0, and the 20 colours `bg_top bg_bottom card card_hover card_sel border accent text muted dim err ok field panel well amber cyan magenta button button_hot` as `#rrggbb` (or `#rrggbbaa`) | colours not written come from `base` |
| `[wallpaper]` | `wallpaper` `theme/daylight/dusk/meadow/slate` | `theme` |
| `[icons]` | `icon_shape` `squircle/circle/rounded/square`; `icon_size` `S/M/L` or 24 to 128 px; `icon_style` `gradient/flat/outline/glass/3d`; `icon_labels` `on/off`; `icon_set` `builtin` | squircle, M, gradient, on |
| `[launcher]` | `launcher` `paged/grid/list`; `columns` `auto` or 3 to 8; `density` `compact/comfortable/roomy`; `page_dots`; `groups` `tabs/sidebar/none`; `island` | paged, auto, comfortable, on, tabs, on |
| `[dock]` | `dock` `bottom/left/right/off`; `dock_items` app ids in order (empty: the shell chooses); `dock_running`; `dock_size` 24 to 128; `dock_magnify`; `dock_autohide` | bottom, empty, on, 56, off, off |
| `[topbar]` | `topbar` `on/off`; `topbar_position` `top/bottom`; `topbar_items` from `clock battery network volume bell needs_you apps settings account power`; `topbar_size` 20 to 64; `footer_hints` | on, top, the default list, 32, on |
| `[menu]` | `menu` from `home apps files chat settings lock sign_out power` (an entry not listed is hidden); `menu_position` `left/right/top/bottom` | all eight, left |
| `[widgets]` | `widgets` from `attention approvals vitals runs tokens network`, each with an optional size `:S/:M/:L` | empty |
| `[motion]` | `motion` `full/reduced` | full |

Checked on top of the ranges: the colour contrast rules of `usertheme::check`, and the top bar and the dock may not both sit at the bottom. **Put back, not refused:** the top bar always holds `needs_you bell settings account power`, and the menu `settings lock sign_out power`; what a file leaves out is added at the end and the result's `fixes` says so. The default Look puts the dock at the bottom (owner 2026-11-15) and the top bar at the top; the lists of bar items and menu entries are a first guess of today's shell and are to be matched when the shell reads a Look.

Example (the full set is in `docs/examples/looks/`):

```text
[look]
name = Midnight glass
[colours]
base = dark
bg_top = #0b1020
[icons]
icon_shape = circle
icon_style = glass
[dock]
dock = left
dock_items = [chat, browser, files, settings]
[topbar]
topbar_items = [clock, network, needs_you, bell, settings, account, power]
[widgets]
widgets = [attention:L, approvals:M]
```

In code: `look::parse(text) -> Result<Accepted { look, fixes }, Vec<LookProblem>>`, `Look::to_text()` (every key, lossless round trip), `look::validate(&Look)`, `Look::builtin()`, and `look_registry` (`available`, `load`, `choose`, `current`; the choice is the key `look` in `ui-<user>.json`, written only when a person chooses). Stages 1 and 2 are done; the shell does not read a Look yet.

## Decisions (owner)
- 2026-11-15: **Looks do not change the layout inside the apps for now** ("this can be later"): apps keep their layout; colours and the shared chrome change. Revisit after the split.
- 2026-11-15: a **dock** with the person's selected apps for easy launch, at the bottom by default, and **3D and glassy icon styles**.

## Open questions for the owner
- "Location": I read it as where the bars, dock and menu sit (top / bottom / left / right) and where widgets sit. If you also mean the on-screen location of individual icons (a free grid), say so: that is a drag-and-drop editor, bigger than reordering.
