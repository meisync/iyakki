//! Chat tools for the desktop theme, including the theme files a person wrote (F-SHELL-62): the list is free (files that cannot be read
//! are listed with the reason), changing the choice asks a card first. The choice is saved with the person's display settings and takes
//! effect when they sign in again (the open screens keep the colours they were drawn with).

pub(super) use iyakki_shared::settingstools::{no_args, Entry};
pub(super) use super::chat_settings::Env;
use serde_json::json;

use super::AppCx;

pub const TOOLS: [&str; 14] = ["display_type", "display_type_set", "display_themes", "display_theme_set", "display_wallpaper", "display_wallpaper_set", "display_fonts", "display_font_set", "display_board", "display_board_set", "display_look", "display_look_set", "display_dock", "display_dock_set"];

pub const PROMPT_RULE: &str = "\nDESKTOP THEMES: display_themes lists the built-in themes (dark, light, high-contrast), the theme files the person or an administrator put in the themes folder (a file that cannot be read is listed with the reason) and which one is chosen; display_theme_set chooses one and asks the person first. The choice applies the next time they sign in. DESKTOP WALLPAPER: display_wallpaper lists the wallpapers (theme, daylight, dusk, meadow, slate) and which one is chosen; display_wallpaper_set chooses one and asks the person first; it applies the next time they sign in (Settings, Display changes it at once with w). TYPE: display_type shows the line height (tight, normal, loose), the letter spacing (normal, wide, wider) and tabular figures (on or off) and display_type_set changes any of them and asks the person first; it applies at once in apps made with the UI SDK. DESKTOP FONTS: display_fonts lists the bundled fonts (DejaVu and Liberation, each with its licence) for the text and for code and which are chosen; display_font_set chooses the text font and/or the code font and asks the person first; it applies the next time they sign in (Settings, Appearance changes it at once with f and p). The text size stays as the person chose it. WIDGET BOARD (the home screen's machine page): display_board lists the widgets on it in order with their width and the ones that can be added (attention, approvals, vitals, runs, tokens, network, apps); display_board_set changes it with op add, remove, move (to position), size (half or full) or reset and asks the person first; it shows within a moment. DESKTOP LOOK: display_look lists the Looks (Default, Compact, Classic and the person's files), the one chosen and its parts (icon shape, style and size, launcher layout, dock and its pinned apps, top bar items); display_look_set chooses a Look by its key and/or changes parts of it by their Look-file keys (icon_style, icon_shape, icon_size, launcher, columns, density, dock, dock_items, topbar_items, ...) and asks the person first. The bell, the needs-you sign, Settings, the account and power always stay on the top bar. Accessibility settings overrule a Look. DOCK: display_dock lists the apps pinned to the dock in order and the apps that can be pinned (JSON); display_dock_set pins, unpins or moves one app and asks the person first.";

/// The most apps the dock holds (the shell shows at most this many).
const DOCK_MAX: usize = 10;

/// The person's pinned apps in order (their own list, else the Look's, else the shell's default set) and every app id that can be pinned.
fn dock_state(data: &std::path::Path, user: &str) -> (Vec<String>, Vec<String>) {
    use iyakki_uikit::look_registry as reg;
    let available: Vec<String> = iyakki_sec::policy::App::ALL.iter().map(|a| a.id().to_string()).collect();
    let pinned = reg::load_dock(data, user)
        .or_else(|| Some(reg::current(data, user).look.dock.items).filter(|i| !i.is_empty()))
        .unwrap_or_else(|| iyakki_uikit::look::DEFAULT_DOCK_IDS.iter().map(|s| s.to_string()).collect());
    (pinned.into_iter().filter(|p| available.contains(p)).take(DOCK_MAX).collect(), available)
}

pub fn entries(cx: &AppCx, env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let (data, user) = (cx.data_dir().to_path_buf(), env.user.clone());
        v.push(Entry::read("Display", "display_themes", "List the desktop themes: the built-in ones, the person's own theme files and the shared ones (each with its name or, when the file cannot be read, why), and which one is chosen.", no_args(), move |_| {
            let chosen = iyakki_uikit::theme::load_choice(&data, &user).unwrap_or_else(|| "dark".into());
            let mut out = vec![format!("Chosen: {chosen}"), "Built in: dark, light, high-contrast".to_string()];
            let files = iyakki_uikit::usertheme::available(&data, &user);
            if files.is_empty() {
                out.push("Theme files: none (a file `name.theme` in the themes folder adds one).".into());
            }
            for (k, r) in files {
                out.push(match r {
                    Ok(u) => format!("Theme file {k}: {}", u.name),
                    Err(e) => format!("Theme file {k}: cannot be used: {e}"),
                });
            }
            Ok(out.join("\n"))
        }));
    }
    {
        let (data, user, e1) = (cx.data_dir().to_path_buf(), env.user.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"theme": {"type": "string", "description": "dark, light, high-contrast, or the key of a theme file from display_themes (for example midnight-blue)."}}, "required": ["theme"]});
        v.push(Entry::change("Display", "display_theme_set", "Choose the desktop theme: a built-in one or a theme file. A file that cannot be read is refused. It applies the next time the person signs in. The person approves a card first.", schema, |a| format!("Desktop theme: {}", a["theme"].as_str().unwrap_or("?")), move |a| {
            let t = a.get("theme").and_then(|x| x.as_str()).unwrap_or("").trim().to_lowercase();
            match t.as_str() {
                "dark" | "light" | "high-contrast" => {
                    iyakki_uikit::theme::save(&data, &user, iyakki_uikit::theme::ThemeId::from_key(&t));
                    e1.audit("settings.display.theme", &t);
                    Ok(format!("The theme {t} is chosen; it applies the next time you sign in."))
                }
                key => {
                    let (_, r) = iyakki_uikit::usertheme::available(&data, &user).into_iter().find(|(k, _)| k == key).ok_or_else(|| format!("there is no theme `{key}` (display_themes lists them)"))?;
                    let u = r.map_err(|e| format!("the theme file {key} cannot be used: {e}"))?;
                    iyakki_uikit::theme::save_choice(&data, &user, &format!("{}{key}", iyakki_uikit::usertheme::CHOICE_PREFIX));
                    e1.audit("settings.display.theme", &format!("file {key}"));
                    Ok(format!("The theme {} is chosen; it applies the next time you sign in.", u.name))
                }
            }
        }));
    }
    {
        let (data, user) = (cx.data_dir().to_path_buf(), env.user.clone());
        v.push(Entry::read("Display", "display_wallpaper", "List the desktop wallpapers (theme, daylight, dusk, meadow, slate; each says whether it is a light picture) and which one is chosen.", no_args(), move |_| {
            use iyakki_uikit::wallpaper as wp;
            let chosen = wp::load(&data, &user);
            let mut out = vec![format!("Chosen: {}", chosen.key())];
            for w in wp::ALL {
                out.push(format!("Wallpaper {}: {} ({})", w.key(), w.name(), if w.key() == "theme" { "the theme's own background" } else if w.is_light() { "light picture" } else { "dark picture" }));
            }
            Ok(out.join("\n"))
        }));
    }
    {
        let (data, user, e1) = (cx.data_dir().to_path_buf(), env.user.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"wallpaper": {"type": "string", "description": "theme, daylight, dusk, meadow or slate (display_wallpaper lists them)."}}, "required": ["wallpaper"]});
        v.push(Entry::change("Display", "display_wallpaper_set", "Choose the desktop wallpaper. It applies the next time the person signs in. The person approves a card first.", schema, |a| format!("Desktop wallpaper: {}", a["wallpaper"].as_str().unwrap_or("?")), move |a| {
            use iyakki_uikit::wallpaper as wp;
            let k = a.get("wallpaper").and_then(|x| x.as_str()).unwrap_or("").trim().to_lowercase();
            let w = wp::ALL.into_iter().find(|w| w.key() == k).ok_or_else(|| format!("there is no wallpaper `{k}` (display_wallpaper lists them)"))?;
            wp::save(&data, &user, w);
            e1.audit("settings.display.wallpaper", w.key());
            Ok(format!("The wallpaper {} is chosen; it applies the next time you sign in.", w.name()))
        }));
    }
    {
        let (data, user) = (cx.data_dir().to_path_buf(), env.user.clone());
        v.push(Entry::read("Display", "display_type", "Show the person's type choices for apps made with the UI SDK: line height (tight, normal, loose), letter spacing (normal, wide, wider) and tabular figures (on or off).", no_args(), move |_| {
            use iyakki_uikit::typeset;
            let p = typeset::load(&data, &user);
            let mut out = vec![format!("Chosen: {}", typeset::describe(p))];
            out.push(format!("Line height: {}", typeset::LINE_ALL.iter().map(|l| l.key()).collect::<Vec<_>>().join(", ")));
            out.push(format!("Letter spacing: {}", typeset::TRACKING_ALL.iter().map(|l| l.key()).collect::<Vec<_>>().join(", ")));
            Ok(out.join("\n"))
        }));
    }
    {
        let (data, user, e1) = (cx.data_dir().to_path_buf(), env.user.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"line_height": {"type": "string", "description": "tight, normal or loose."}, "letter_spacing": {"type": "string", "description": "normal, wide or wider."}, "tabular": {"type": "boolean", "description": "digits of one width."}}});
        v.push(Entry::change("Display", "display_type_set", "Change the line height, the letter spacing and/or tabular figures of the text in apps made with the UI SDK. The person approves a card first.", schema, |a| format!("Type: line height {}, letter spacing {}, tabular {}", a["line_height"].as_str().unwrap_or("unchanged"), a["letter_spacing"].as_str().unwrap_or("unchanged"), a["tabular"].as_bool().map_or("unchanged".to_string(), |b| b.to_string())), move |a| {
            use iyakki_uikit::typeset;
            let mut p = typeset::load(&data, &user);
            let (l, t, b) = (a.get("line_height").and_then(|x| x.as_str()), a.get("letter_spacing").and_then(|x| x.as_str()), a.get("tabular").and_then(|x| x.as_bool()));
            if l.is_none() && t.is_none() && b.is_none() {
                return Err("name a choice: `line_height`, `letter_spacing` and/or `tabular` (display_type lists them)".into());
            }
            if let Some(l) = l {
                p.line = typeset::LineHeight::from_key(l).ok_or_else(|| format!("there is no line height `{l}` (display_type lists them)"))?;
            }
            if let Some(t) = t {
                p.tracking = typeset::Tracking::from_key(t).ok_or_else(|| format!("there is no letter spacing `{t}` (display_type lists them)"))?;
            }
            if let Some(b) = b {
                p.tabular = b;
            }
            typeset::save(&data, &user, p);
            typeset::set_current(p);
            e1.audit("settings.display.type", &typeset::describe(p));
            Ok(format!("The type is set: {}.", typeset::describe(p)))
        }));
    }
    {
        let (data, user) = (cx.data_dir().to_path_buf(), env.user.clone());
        v.push(Entry::read("Display", "display_fonts", "List the bundled fonts (each with its licence) for the interface text (dejavu, liberation) and for code (dejavu, liberation) and which ones are chosen.", no_args(), move |_| {
            use iyakki_uikit::fonts;
            let (u, c) = fonts::load_choice(&data, &user);
            let mut out = vec![format!("Chosen: text {}, code {}", u.key(), c.key())];
            for f in fonts::UI_ALL {
                out.push(format!("Text font {}: {} ({})", f.key(), f.name(), f.licence()));
            }
            for f in fonts::CODE_ALL {
                out.push(format!("Code font {}: {} ({})", f.key(), f.name(), f.licence()));
            }
            Ok(out.join("\n"))
        }));
    }
    {
        let (data, user, e1) = (cx.data_dir().to_path_buf(), env.user.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"text": {"type": "string", "description": "dejavu or liberation: the font of the interface text (display_fonts lists them)."}, "code": {"type": "string", "description": "dejavu or liberation: the font of code and fixed-width text."}}});
        v.push(Entry::change("Display", "display_font_set", "Choose the font of the interface text and/or of code from the bundled ones. The text size stays as chosen. It applies the next time the person signs in. The person approves a card first.", schema, |a| format!("Fonts: text {}, code {}", a["text"].as_str().unwrap_or("unchanged"), a["code"].as_str().unwrap_or("unchanged")), move |a| {
            use iyakki_uikit::fonts;
            let (mut u, mut c) = fonts::load_choice(&data, &user);
            let (t, k) = (a.get("text").and_then(|x| x.as_str()), a.get("code").and_then(|x| x.as_str()));
            if t.is_none() && k.is_none() {
                return Err("name a font: `text` and/or `code` (display_fonts lists them)".into());
            }
            if let Some(t) = t {
                u = fonts::UiFont::from_key(t).ok_or_else(|| format!("there is no text font `{t}` (display_fonts lists them)"))?;
            }
            if let Some(k) = k {
                c = fonts::CodeFont::from_key(k).ok_or_else(|| format!("there is no code font `{k}` (display_fonts lists them)"))?;
            }
            fonts::save_choice(&data, &user, u, c);
            e1.audit("settings.display.font", &format!("{} {}", u.key(), c.key()));
            Ok(format!("The fonts are {} for text and {} for code; they apply the next time you sign in.", u.name(), c.name()))
        }));
    }
    {
        let (data, user) = (cx.data_dir().to_path_buf(), env.user.clone());
        v.push(Entry::read("Display", "display_board", "List the widgets on the home screen's machine page in order, each with its width (half or full), and the widgets that can still be added.", no_args(), move |_| {
            let b = iyakki_uikit::board::load(&data, &user);
            let mut out = vec![format!("Board: {} widget{}", b.slots.len(), if b.slots.len() == 1 { "" } else { "s" })];
            for (i, s) in b.slots.iter().enumerate() {
                out.push(format!("{}. {} [{}] ({} width)", i + 1, s.kind.title(), s.kind.key(), s.size.key()));
            }
            let miss: Vec<&str> = b.missing().iter().map(|k| k.key()).collect();
            out.push(format!("Can be added: {}", if miss.is_empty() { "nothing, every widget is on the board".to_string() } else { miss.join(", ") }));
            Ok(out.join("\n"))
        }));
    }
    {
        let (data, user, e1) = (cx.data_dir().to_path_buf(), env.user.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"op": {"type": "string", "description": "add, remove, move, size or reset"}, "widget": {"type": "string", "description": "attention, approvals, vitals, runs, tokens, network or apps (display_board lists them)"}, "position": {"type": "integer", "description": "for move: the new place, counted from 1"}, "size": {"type": "string", "description": "for size: half or full"}}, "required": ["op"]});
        v.push(Entry::change("Display", "display_board_set", "Change the widget board on the home screen's machine page: add or remove a widget, move it, make it half or full width, or put the usual board back. It is kept for the person and shows within a moment. The person approves a card first.", schema, |a| format!("Widget board: {} {}", a["op"].as_str().unwrap_or("?"), a["widget"].as_str().unwrap_or("")).trim().to_string(), move |a| {
            use iyakki_uikit::board::{self, Kind, Size};
            let op = a.get("op").and_then(|x| x.as_str()).unwrap_or("").trim().to_lowercase();
            let mut b = board::load(&data, &user);
            let kind = || -> Result<Kind, String> {
                let w = a.get("widget").and_then(|x| x.as_str()).unwrap_or("");
                Kind::from_key(w).ok_or_else(|| format!("there is no widget `{w}` (display_board lists them)"))
            };
            let at = |b: &board::Board, k: Kind| b.slots.iter().position(|s| s.kind == k).ok_or_else(|| format!("{} is not on the board", k.key()));
            let said = match op.as_str() {
                "add" => {
                    let k = kind()?;
                    if !b.add(k) {
                        return Err(format!("{} is already on the board", k.key()));
                    }
                    format!("Added {} at the end.", k.title())
                }
                "remove" => {
                    let k = kind()?;
                    let i = at(&b, k)?;
                    b.remove(i);
                    format!("Removed {}.", k.title())
                }
                "move" => {
                    let k = kind()?;
                    let i = at(&b, k)?;
                    let p = a.get("position").and_then(|x| x.as_u64()).filter(|p| *p >= 1).ok_or("move needs a position, counted from 1")? as usize;
                    let to = b.move_to(i, p - 1).unwrap_or(i);
                    format!("Moved {} to place {}.", k.title(), to + 1)
                }
                "size" => {
                    let k = kind()?;
                    let i = at(&b, k)?;
                    let z = a.get("size").and_then(|x| x.as_str()).and_then(Size::from_key).ok_or("size must be half or full")?;
                    b.set_size(i, z);
                    format!("{} is {} width now.", k.title(), z.key())
                }
                "reset" => {
                    b = board::Board::default();
                    "The usual board is back.".to_string()
                }
                other => return Err(format!("unknown op `{other}`: add, remove, move, size or reset")),
            };
            board::save(&data, &user, &b);
            iyakki_uikit::look_registry::notify_changed();
            e1.audit("settings.display.board", &format!("{op} {}", b.to_text()));
            Ok(said)
        }));
    }
    {
        let (data, user) = (cx.data_dir().to_path_buf(), env.user.clone());
        v.push(Entry::read("Display", "display_look", "List the desktop Looks (the built-in Default, Compact and Classic and the person's own files, each with its name or why it cannot be used), which one is chosen, and its parts: icon shape, style and size, the launcher layout and columns, the dock and its pinned apps, and the top bar items.", no_args(), move |_| {
            use iyakki_uikit::look_registry as reg;
            let chosen = reg::load_choice(&data, &user).unwrap_or_else(|| "default".into());
            let cur = reg::current(&data, &user).look;
            let mut out = vec![format!("Chosen: {chosen}")];
            for e in reg::available(&data, &user) {
                out.push(match e.result {
                    Ok(a) => format!("Look {}: {}{}", e.key, a.look.name, if e.builtin { " (built in)" } else { "" }),
                    Err(p) => format!("Look {}: cannot be used: {}", e.key, iyakki_uikit::look::problems_text(&p)),
                });
            }
            let pinned = reg::load_dock(&data, &user).unwrap_or_else(|| cur.dock.items.clone());
            out.push(format!("Icons: shape {}, style {}, size {}, names {}", cur.icons.shape.key(), cur.icons.style.key(), cur.icons.size.text(), if cur.icons.labels { "shown" } else { "hidden" }));
            out.push(format!("Launcher: {}, columns {}, spacing {}", cur.launcher.layout.key(), match cur.launcher.columns { iyakki_uikit::look::Columns::Auto => "auto".to_string(), iyakki_uikit::look::Columns::Fixed(n) => n.to_string() }, cur.launcher.density.key()));
            out.push(format!("Dock: {}, pinned apps: {}", cur.dock.position.key(), if pinned.is_empty() { "the usual ones".to_string() } else { pinned.join(", ") }));
            out.push(format!("Top bar items: {}", cur.topbar.items.iter().map(|i| i.key()).collect::<Vec<_>>().join(", ")));
            out.push(format!("Wallpaper: {}", cur.wallpaper.key()));
            Ok(out.join("\n"))
        }));
    }
    {
        let (data, user, e1) = (cx.data_dir().to_path_buf(), env.user.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {
            "look": {"type": "string", "description": "the key of a Look from display_look (default, compact, classic or a file of the person's)."},
            "set": {"type": "object", "description": "parts to change, by their Look-file keys: icon_shape (squircle, circle, rounded, square), icon_style (gradient, flat, outline, glass, 3d), icon_size (S, M, L or 24 to 128), icon_labels (on, off), launcher (paged, grid, list), columns (auto or 3 to 8), density (compact, comfortable, roomy), dock (bottom, left, right, off), dock_items (a list of app ids such as [chat, browser]), dock_running (on, off), topbar_items (a list such as [clock, battery, network, volume, needs_you, bell, settings, account, power]), wallpaper (theme, daylight, dusk, meadow, slate), motion (full, reduced).", "additionalProperties": {"type": "string"}}
        }});
        v.push(Entry::change("Display", "display_look_set", "Choose a desktop Look and/or change parts of it (icons, launcher layout, dock and pinned apps, top bar items, wallpaper). The result is kept as the person's own Look and checked first: the bell, the needs-you sign, Settings, the account and power are always put back on the top bar. It applies within a moment. The person approves a card first.", schema, |a| {
            let what = a.get("look").and_then(|x| x.as_str()).map(|l| format!("Look {l}")).unwrap_or_default();
            let keys: Vec<String> = a.get("set").and_then(|x| x.as_object()).map(|o| o.keys().cloned().collect()).unwrap_or_default();
            format!("Desktop look: {}{}{}", what, if what.is_empty() || keys.is_empty() { "" } else { " and " }, if keys.is_empty() { String::new() } else { format!("change {}", keys.join(", ")) })
        }, move |a| {
            use iyakki_uikit::look_registry as reg;
            let key = a.get("look").and_then(|x| x.as_str()).map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty());
            let set = a.get("set").and_then(|x| x.as_object()).cloned().unwrap_or_default();
            if key.is_none() && set.is_empty() {
                return Err("name a `look` to choose and/or a `set` of parts to change (display_look lists them)".into());
            }
            if let Some(k) = &key {
                reg::choose(&data, &user, k).map_err(|p| iyakki_uikit::look::problems_text(&p))?;
            }
            if !set.is_empty() {
                // patch the Look's text, key by key (the text has every key), then let the parser check the whole file
                let mut text = reg::current(&data, &user).look.to_text();
                for (k, v) in &set {
                    let k = k.trim();
                    if !k.chars().all(|c| c.is_ascii_lowercase() || c == '_') || k == "name" || k == "format" {
                        return Err(format!("`{k}` is not a part of a Look that can be changed here"));
                    }
                    let v = v.as_str().map(String::from).or_else(|| v.as_array().map(|a| format!("[{}]", a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")))).unwrap_or_default();
                    if v.contains('\n') || v.contains('\r') {
                        return Err(format!("the value of `{k}` is not a single line"));
                    }
                    let mut found = false;
                    text = text.lines().map(|l| {
                        if l.trim_start().split('=').next().is_some_and(|x| x.trim() == k) && l.contains('=') {
                            found = true;
                            format!("{k} = {v}")
                        } else {
                            l.to_string()
                        }
                    }).collect::<Vec<_>>().join("\n");
                    if !found {
                        return Err(format!("there is no Look part `{k}` (display_look and the docs list them)"));
                    }
                }
                let parsed = iyakki_uikit::look::parse(&text).map_err(|p| iyakki_uikit::look::problems_text(&p))?;
                let put_back = parsed.fixes.clone();
                let mut l = parsed.look;
                l.name = "My look".into();
                if let Some(items) = set.get("dock_items") {
                    let _ = items; // the Look's own list; a pinned list the person made in the dock still wins
                    reg::clear_dock(&data, &user);
                }
                let a = reg::save_custom(&data, &user, &l).map_err(|p| iyakki_uikit::look::problems_text(&p))?;
                iyakki_uikit::wallpaper::save(&data, &user, a.look.wallpaper);
                let fixes: Vec<String> = put_back.into_iter().chain(a.fixes.clone()).collect();
                if !fixes.is_empty() {
                    e1.audit("settings.display.look", &format!("set {} (put back: {})", set.keys().cloned().collect::<Vec<_>>().join(","), fixes.join("; ")));
                    reg::notify_changed();
                    return Ok(format!("Your Look is updated. {}", fixes.join(" ")));
                }
            }
            e1.audit("settings.display.look", &format!("{}{}", key.as_deref().map(|k| format!("choose {k} ")).unwrap_or_default(), set.keys().cloned().collect::<Vec<_>>().join(",")));
            reg::notify_changed();
            Ok("The Look is chosen and kept; it applies within a moment.".to_string())
        }));
    }
    {
        let (data, user) = (cx.data_dir().to_path_buf(), env.user.clone());
        v.push(Entry::read("Display", "display_dock", "List the apps pinned to the dock (in order) and the apps that can be pinned, as JSON: {\"pinned\": [...], \"available\": [...]}. The shell shows only the apps the person may use.", no_args(), move |_| {
            let (pinned, available) = dock_state(&data, &user);
            Ok(json!({"pinned": pinned, "available": available}).to_string())
        }));
    }
    {
        let (data, user, e1) = (cx.data_dir().to_path_buf(), env.user.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {
            "op": {"type": "string", "description": "pin (add to the end), unpin (take off) or move (to `position`, 1 is first)"},
            "app": {"type": "string", "description": "an app id from display_dock, such as chat or browser"},
            "position": {"type": "integer", "description": "for move: the place in the dock, 1 is first"}
        }, "required": ["op", "app"]});
        v.push(Entry::change("Display", "display_dock_set", "Pin an app to the dock, unpin it, or move it to another place. It is kept for the person and shows within a moment. The person approves a card first.", schema, |a| format!("Dock: {} {}", a["op"].as_str().unwrap_or("?"), a["app"].as_str().unwrap_or("?")), move |a| {
            let op = a.get("op").and_then(|x| x.as_str()).unwrap_or("").trim().to_lowercase();
            let app = a.get("app").and_then(|x| x.as_str()).unwrap_or("").trim().to_lowercase();
            let (mut pinned, available) = dock_state(&data, &user);
            if !available.contains(&app) {
                return Err(format!("there is no app `{app}` to pin (display_dock lists them)"));
            }
            let at = pinned.iter().position(|p| *p == app);
            let said = match op.as_str() {
                "pin" => {
                    if at.is_some() {
                        return Err(format!("`{app}` is already in the dock"));
                    }
                    if pinned.len() >= DOCK_MAX {
                        return Err(format!("the dock holds at most {DOCK_MAX} apps; unpin one first"));
                    }
                    pinned.push(app.clone());
                    format!("Pinned {app}.")
                }
                "unpin" => {
                    let Some(i) = at else { return Err(format!("`{app}` is not in the dock")) };
                    pinned.remove(i);
                    format!("Unpinned {app}.")
                }
                "move" => {
                    let Some(i) = at else { return Err(format!("`{app}` is not in the dock")) };
                    let to = a.get("position").and_then(|x| x.as_u64()).filter(|p| *p >= 1).ok_or("say the `position` to move to (1 is first)")? as usize;
                    let x = pinned.remove(i);
                    let to = (to - 1).min(pinned.len());
                    pinned.insert(to, x);
                    format!("Moved {app} to place {}.", to + 1)
                }
                _ => return Err("`op` is pin, unpin or move".into()),
            };
            iyakki_uikit::look_registry::save_dock(&data, &user, &pinned);
            e1.audit("settings.display.dock", &format!("{op} {app}"));
            iyakki_uikit::look_registry::notify_changed();
            Ok(said)
        }));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_shared::agent::{Risk, ToolDef};
    use iyakki_shared::agent::{Tools};
    use iyakki_shared::settingstools::SettingsTools;
    use serde_json::Value;
    use std::sync::{Arc, Mutex};

    struct NoTools;
    impl Tools for NoTools {
        fn defs(&self) -> Vec<ToolDef> {
            vec![]
        }
        fn call(&self, name: &str, _a: &Value) -> Result<String, String> {
            Err(format!("no tool {name}"))
        }
        fn describe(&self, name: &str, _a: &Value) -> String {
            name.to_string()
        }
    }

    #[test]
    fn the_list_is_free_the_choice_asks_a_file_that_cannot_be_read_is_refused_and_the_choice_is_saved() {
        let cx = crate::apps::tests::cx_with_admin_store("chatthemes");
        cx.services.on_login("root", "rootpass12", true);
        let audits = Arc::new(Mutex::new(Vec::new()));
        let a2 = audits.clone();
        let settings = Arc::new(Mutex::new(iyakki_shared::settings::SettingsStore::open(&cx.data_dir().join("settings.json")).unwrap()));
        let env = Env { dir: cx.data_dir().to_path_buf(), user: "root".into(), admin: true, settings, audit: Arc::new(move |a, d| a2.lock().unwrap().push(format!("{a} {d}"))) };
        let dir = cx.data_dir().to_path_buf();
        std::fs::create_dir_all(dir.join("themes")).unwrap();
        std::fs::write(dir.join("themes/midnight-blue.theme"), "name = Midnight blue\nbase = dark\nbg_top = #0b1020\n").unwrap();
        std::fs::write(dir.join("themes/dull.theme"), "name = Dull\ntext = #111111\n").unwrap();
        let t = SettingsTools::new(Arc::new(NoTools), true, entries(&cx, &env));
        let d = t.defs();
        assert_eq!(d.iter().find(|x| x.spec.name == "display_themes").unwrap().risk, Risk::Safe);
        assert_eq!(d.iter().find(|x| x.spec.name == "display_theme_set").unwrap().risk, Risk::Ask);
        let l = t.call("display_themes", &json!({})).unwrap();
        assert!(l.contains("Chosen: dark") && l.contains("Theme file midnight-blue: Midnight blue") && l.contains("Theme file dull: cannot be used") && l.contains("`text` on"), "{l}");
        assert!(t.call("display_theme_set", &json!({"theme": "dull"})).unwrap_err().contains("cannot be used"));
        assert!(t.call("display_theme_set", &json!({"theme": "nope"})).unwrap_err().contains("no theme"));
        assert_eq!(iyakki_uikit::theme::load_choice(&dir, "root"), None, "nothing was saved by the refusals");
        assert!(t.call("display_theme_set", &json!({"theme": "midnight-blue"})).unwrap().contains("next time you sign in"));
        assert_eq!(iyakki_uikit::theme::load_choice(&dir, "root").as_deref(), Some("user:midnight-blue"));
        assert!(t.call("display_themes", &json!({})).unwrap().contains("Chosen: user:midnight-blue"));
        assert!(t.call("display_theme_set", &json!({"theme": "light"})).is_ok());
        assert_eq!(iyakki_uikit::theme::load(&dir, "root"), iyakki_uikit::theme::ThemeId::Light);
        assert!(audits.lock().unwrap().iter().any(|a| a == "settings.display.theme file midnight-blue"));
    }

    #[test]
    fn the_wallpaper_list_is_free_the_choice_asks_refuses_an_unknown_one_is_saved_and_logged() {
        let cx = crate::apps::tests::cx_with_admin_store("chatwall");
        cx.services.on_login("root", "rootpass12", true);
        let audits = Arc::new(Mutex::new(Vec::new()));
        let a2 = audits.clone();
        let settings = Arc::new(Mutex::new(iyakki_shared::settings::SettingsStore::open(&cx.data_dir().join("settings.json")).unwrap()));
        let env = Env { dir: cx.data_dir().to_path_buf(), user: "root".into(), admin: true, settings, audit: Arc::new(move |a, d| a2.lock().unwrap().push(format!("{a} {d}"))) };
        let dir = cx.data_dir().to_path_buf();
        let t = SettingsTools::new(Arc::new(NoTools), true, entries(&cx, &env));
        let d = t.defs();
        assert_eq!(d.iter().find(|x| x.spec.name == "display_wallpaper").unwrap().risk, Risk::Safe);
        assert_eq!(d.iter().find(|x| x.spec.name == "display_wallpaper_set").unwrap().risk, Risk::Ask);
        let l = t.call("display_wallpaper", &json!({})).unwrap();
        assert!(l.contains("Chosen: theme") && l.contains("Wallpaper daylight: Daylight (light picture)") && l.contains("Wallpaper dusk: Dusk (dark picture)"), "{l}");
        assert!(t.call("display_wallpaper_set", &json!({"wallpaper": "nope"})).unwrap_err().contains("no wallpaper"));
        assert_eq!(iyakki_uikit::wallpaper::load(&dir, "root"), iyakki_uikit::wallpaper::Id::Theme, "the refusal saved nothing");
        assert!(t.call("display_wallpaper_set", &json!({"wallpaper": "Dusk"})).unwrap().contains("next time you sign in"));
        assert_eq!(iyakki_uikit::wallpaper::load(&dir, "root"), iyakki_uikit::wallpaper::Id::Dusk);
        assert!(t.call("display_wallpaper", &json!({})).unwrap().contains("Chosen: dusk"));
        assert!(audits.lock().unwrap().iter().any(|a| a == "settings.display.wallpaper dusk"));
        assert!(TOOLS.contains(&"display_wallpaper") && TOOLS.contains(&"display_wallpaper_set"));
    }

    #[test]
    fn the_font_tools_list_free_change_with_a_card_refuse_unknown_names_and_log() {
        let cx = crate::apps::tests::cx_with_admin_store("chatfont");
        cx.services.on_login("root", "rootpass12", true);
        let audits = Arc::new(Mutex::new(Vec::new()));
        let a2 = audits.clone();
        let settings = Arc::new(Mutex::new(iyakki_shared::settings::SettingsStore::open(&cx.data_dir().join("settings.json")).unwrap()));
        let env = Env { dir: cx.data_dir().to_path_buf(), user: "root".into(), admin: true, settings, audit: Arc::new(move |a, d| a2.lock().unwrap().push(format!("{a} {d}"))) };
        let dir = cx.data_dir().to_path_buf();
        let t = SettingsTools::new(Arc::new(NoTools), true, entries(&cx, &env));
        let d = t.defs();
        assert_eq!(d.iter().find(|x| x.spec.name == "display_fonts").unwrap().risk, Risk::Safe);
        assert_eq!(d.iter().find(|x| x.spec.name == "display_font_set").unwrap().risk, Risk::Ask);
        let l = t.call("display_fonts", &json!({})).unwrap();
        assert!(l.contains("Chosen: text dejavu, code dejavu") && l.contains("Text font liberation: Liberation Sans (SIL Open Font License 1.1)") && l.contains("Code font dejavu: DejaVu Sans Mono"), "{l}");
        assert!(t.call("display_font_set", &json!({})).unwrap_err().contains("name a font"));
        assert!(t.call("display_font_set", &json!({"text": "comic"})).unwrap_err().contains("no text font"));
        assert!(t.call("display_font_set", &json!({"code": "comic"})).unwrap_err().contains("no code font"));
        assert_eq!(iyakki_uikit::fonts::load_choice(&dir, "root"), (iyakki_uikit::fonts::UiFont::DejaVu, iyakki_uikit::fonts::CodeFont::DejaVu), "refusals saved nothing");
        assert!(t.call("display_font_set", &json!({"text": "Liberation"})).unwrap().contains("next time you sign in"));
        assert_eq!(iyakki_uikit::fonts::load_choice(&dir, "root"), (iyakki_uikit::fonts::UiFont::Liberation, iyakki_uikit::fonts::CodeFont::DejaVu), "only the named one changed");
        assert!(t.call("display_font_set", &json!({"code": "liberation"})).is_ok());
        assert!(t.call("display_fonts", &json!({})).unwrap().contains("Chosen: text liberation, code liberation"));
        assert!(audits.lock().unwrap().iter().any(|a| a == "settings.display.font liberation liberation"));
        assert!(TOOLS.contains(&"display_fonts") && TOOLS.contains(&"display_font_set"));
    }

    #[test]
    fn the_type_tools_list_free_change_with_a_card_refuse_unknown_words_and_log() {
        use iyakki_uikit::typeset::{self, LineHeight, Prefs, Tracking};
        let cx = crate::apps::tests::cx_with_admin_store("chattype");
        cx.services.on_login("root", "rootpass12", true);
        let audits = Arc::new(Mutex::new(Vec::new()));
        let a2 = audits.clone();
        let settings = Arc::new(Mutex::new(iyakki_shared::settings::SettingsStore::open(&cx.data_dir().join("settings.json")).unwrap()));
        let env = Env { dir: cx.data_dir().to_path_buf(), user: "root".into(), admin: true, settings, audit: Arc::new(move |a, d| a2.lock().unwrap().push(format!("{a} {d}"))) };
        let dir = cx.data_dir().to_path_buf();
        typeset::set_current(Prefs::default());
        let t = SettingsTools::new(Arc::new(NoTools), true, entries(&cx, &env));
        let d = t.defs();
        assert_eq!(d.iter().find(|x| x.spec.name == "display_type").unwrap().risk, Risk::Safe);
        assert_eq!(d.iter().find(|x| x.spec.name == "display_type_set").unwrap().risk, Risk::Ask);
        let l = t.call("display_type", &json!({})).unwrap();
        assert!(l.contains("Chosen: line height normal, letter spacing normal, tabular figures off") && l.contains("Line height: tight, normal, loose"), "{l}");
        assert!(t.call("display_type_set", &json!({})).unwrap_err().contains("name a choice"));
        assert!(t.call("display_type_set", &json!({"line_height": "huge"})).unwrap_err().contains("no line height"));
        assert!(t.call("display_type_set", &json!({"letter_spacing": "huge"})).unwrap_err().contains("no letter spacing"));
        assert_eq!(typeset::load(&dir, "root"), Prefs::default(), "refusals saved nothing");
        assert!(t.call("display_type_set", &json!({"line_height": "Loose", "tabular": true})).unwrap().contains("line height loose"));
        assert_eq!(typeset::load(&dir, "root"), Prefs { line: LineHeight::Loose, tracking: Tracking::Normal, tabular: true });
        assert_eq!(typeset::current().line, LineHeight::Loose, "applied at once");
        assert!(audits.lock().unwrap().iter().any(|a| a.starts_with("settings.display.type line height loose")));
        assert!(TOOLS.contains(&"display_type") && TOOLS.contains(&"display_type_set"));
        typeset::set_current(Prefs::default());
    }

    #[test]
    fn the_board_tools_list_free_change_with_a_card_refuse_what_makes_no_sense_and_log() {
        let cx = crate::apps::tests::cx_with_admin_store("chatboard");
        cx.services.on_login("root", "rootpass12", true);
        let audits = Arc::new(Mutex::new(Vec::new()));
        let a2 = audits.clone();
        let settings = Arc::new(Mutex::new(iyakki_shared::settings::SettingsStore::open(&cx.data_dir().join("settings.json")).unwrap()));
        let env = Env { dir: cx.data_dir().to_path_buf(), user: "root".into(), admin: true, settings, audit: Arc::new(move |a, d| a2.lock().unwrap().push(format!("{a} {d}"))) };
        let dir = cx.data_dir().to_path_buf();
        let t = SettingsTools::new(Arc::new(NoTools), true, entries(&cx, &env));
        let d = t.defs();
        assert_eq!(d.iter().find(|x| x.spec.name == "display_board").unwrap().risk, Risk::Safe);
        assert_eq!(d.iter().find(|x| x.spec.name == "display_board_set").unwrap().risk, Risk::Ask);
        let l = t.call("display_board", &json!({})).unwrap();
        assert!(l.contains("Board: 5 widgets") && l.contains("1. NEEDS A PERSON [attention] (full width)") && l.contains("Can be added: approvals, tokens"), "{l}");
        for (args, why) in [(json!({"op": "dance"}), "unknown op"), (json!({"op": "add", "widget": "weather"}), "no widget"), (json!({"op": "add", "widget": "apps"}), "already on the board"), (json!({"op": "remove", "widget": "tokens"}), "not on the board"), (json!({"op": "move", "widget": "apps"}), "position"), (json!({"op": "size", "widget": "apps", "size": "huge"}), "half or full")] {
            let e = t.call("display_board_set", &args).unwrap_err();
            assert!(e.contains(why), "{args}: {e}");
        }
        assert_eq!(iyakki_uikit::board::load(&dir, "root"), iyakki_uikit::board::Board::default(), "the refusals saved nothing");
        assert!(t.call("display_board_set", &json!({"op": "add", "widget": "Tokens"})).unwrap().contains("Added TOKENS"));
        assert!(t.call("display_board_set", &json!({"op": "move", "widget": "tokens", "position": 1})).unwrap().contains("place 1"));
        assert!(t.call("display_board_set", &json!({"op": "size", "widget": "apps", "size": "full"})).is_ok());
        assert!(t.call("display_board_set", &json!({"op": "remove", "widget": "network"})).is_ok());
        let b = iyakki_uikit::board::load(&dir, "root");
        assert_eq!(b.to_text(), "tokens:half,attention:full,apps:full,vitals:half,runs:half");
        assert!(iyakki_uikit::look_registry::take_changed(), "the shell is told to read the board again");
        assert!(t.call("display_board", &json!({})).unwrap().contains("1. TOKENS, 24 HOURS [tokens] (half width)"));
        assert!(t.call("display_board_set", &json!({"op": "reset"})).is_ok());
        assert_eq!(iyakki_uikit::board::load(&dir, "root"), iyakki_uikit::board::Board::default());
        assert!(audits.lock().unwrap().iter().any(|a| a.starts_with("settings.display.board remove ") && a.contains("network") == false));
        assert!(TOOLS.contains(&"display_board") && TOOLS.contains(&"display_board_set"));
    }

    #[test]
    fn the_look_tools_list_free_change_with_a_card_patch_parts_check_the_file_and_put_the_essentials_back() {
        let cx = crate::apps::tests::cx_with_admin_store("chatlook");
        cx.services.on_login("root", "rootpass12", true);
        let audits = Arc::new(Mutex::new(Vec::new()));
        let a2 = audits.clone();
        let settings = Arc::new(Mutex::new(iyakki_shared::settings::SettingsStore::open(&cx.data_dir().join("settings.json")).unwrap()));
        let env = Env { dir: cx.data_dir().to_path_buf(), user: "root".into(), admin: true, settings, audit: Arc::new(move |a, d| a2.lock().unwrap().push(format!("{a} {d}"))) };
        let dir = cx.data_dir().to_path_buf();
        let t = SettingsTools::new(Arc::new(NoTools), true, entries(&cx, &env));
        let d = t.defs();
        assert_eq!(d.iter().find(|x| x.spec.name == "display_look").unwrap().risk, Risk::Safe);
        assert_eq!(d.iter().find(|x| x.spec.name == "display_look_set").unwrap().risk, Risk::Ask);
        let l = t.call("display_look", &json!({})).unwrap();
        assert!(l.contains("Chosen: default") && l.contains("Look compact: Compact (built in)") && l.contains("Icons: shape squircle, style gradient") && l.contains("Dock: bottom"), "{l}");
        // nothing named: refused, nothing saved
        assert!(t.call("display_look_set", &json!({})).is_err());
        // unknown part, bad word, a value out of range: each refused with the reason, nothing saved
        assert!(t.call("display_look_set", &json!({"set": {"nope": "x"}})).unwrap_err().contains("no Look part"));
        assert!(t.call("display_look_set", &json!({"set": {"icon_style": "shiny"}})).is_err());
        assert!(t.call("display_look_set", &json!({"set": {"columns": "99"}})).is_err());
        assert_eq!(iyakki_uikit::look_registry::load_choice(&dir, "root"), None);
        // a good change keeps the person's own Look, logs it and takes effect
        assert!(t.call("display_look_set", &json!({"set": {"icon_style": "3d", "icon_shape": "circle", "dock": "left", "dock_items": ["chat", "browser"]}})).is_ok());
        let now = iyakki_uikit::look_registry::current(&dir, "root").look;
        assert_eq!((now.icons.style, now.icons.shape, now.dock.position), (iyakki_uikit::look::IconStyle::ThreeD, iyakki_uikit::look::IconShape::Circle, iyakki_uikit::look::DockPos::Left));
        assert_eq!(now.dock.items, vec!["chat".to_string(), "browser".to_string()]);
        assert!(iyakki_uikit::look_registry::take_changed(), "the shell is told");
        assert!(audits.lock().unwrap().iter().any(|a| a.starts_with("settings.display.look")));
        // a top bar without the essentials: they are put back and the result says so
        let r = t.call("display_look_set", &json!({"set": {"topbar_items": ["clock"]}})).unwrap();
        assert!(r.contains("updated"), "{r}");
        let bar = iyakki_uikit::look_registry::current(&dir, "root").look.topbar.items;
        for e in iyakki_uikit::look::ESSENTIAL_BAR {
            assert!(bar.contains(&e), "{e:?} is always there");
        }
        // choosing a built-in Look by its key
        assert!(t.call("display_look_set", &json!({"look": "compact"})).is_ok());
        assert_eq!(iyakki_uikit::look_registry::load_choice(&dir, "root").as_deref(), Some("compact"));
        assert!(t.call("display_look_set", &json!({"look": "no-such"})).is_err());
        assert!(TOOLS.contains(&"display_look") && TOOLS.contains(&"display_look_set"));
    }

    #[test]
    fn the_dock_tools_list_free_pin_unpin_and_move_with_a_card_and_keep_the_list_per_person() {
        let cx = crate::apps::tests::cx_with_admin_store("chatdock");
        cx.services.on_login("root", "rootpass12", true);
        let audits = Arc::new(Mutex::new(Vec::new()));
        let a2 = audits.clone();
        let settings = Arc::new(Mutex::new(iyakki_shared::settings::SettingsStore::open(&cx.data_dir().join("settings.json")).unwrap()));
        let env = Env { dir: cx.data_dir().to_path_buf(), user: "root".into(), admin: true, settings, audit: Arc::new(move |a, d| a2.lock().unwrap().push(format!("{a} {d}"))) };
        let dir = cx.data_dir().to_path_buf();
        let t = SettingsTools::new(Arc::new(NoTools), true, entries(&cx, &env));
        let d = t.defs();
        assert_eq!(d.iter().find(|x| x.spec.name == "display_dock").unwrap().risk, Risk::Safe);
        assert_eq!(d.iter().find(|x| x.spec.name == "display_dock_set").unwrap().risk, Risk::Ask);
        let l: Value = serde_json::from_str(&t.call("display_dock", &json!({})).unwrap()).unwrap();
        assert_eq!(l["pinned"], json!(["chat", "browser", "ide", "edit", "cli", "system"]), "the shell's default set when nothing was pinned");
        assert!(l["available"].as_array().unwrap().iter().any(|x| x == "radio"));
        // refusals save nothing
        for (args, why) in [(json!({"op": "dance", "app": "chat"}), "pin, unpin or move"), (json!({"op": "pin", "app": "nope"}), "no app"), (json!({"op": "pin", "app": "chat"}), "already in the dock"), (json!({"op": "unpin", "app": "radio"}), "not in the dock"), (json!({"op": "move", "app": "chat"}), "position")] {
            let e = t.call("display_dock_set", &args).unwrap_err();
            assert!(e.contains(why), "{args}: {e}");
        }
        assert_eq!(iyakki_uikit::look_registry::load_dock(&dir, "root"), None);
        assert!(t.call("display_dock_set", &json!({"op": "pin", "app": "Radio"})).unwrap().contains("Pinned radio"));
        assert!(t.call("display_dock_set", &json!({"op": "unpin", "app": "cli"})).is_ok());
        assert!(t.call("display_dock_set", &json!({"op": "move", "app": "radio", "position": 1})).unwrap().contains("place 1"));
        assert_eq!(iyakki_uikit::look_registry::load_dock(&dir, "root").unwrap(), ["radio", "chat", "browser", "ide", "edit", "system"]);
        assert!(iyakki_uikit::look_registry::take_changed(), "the shell is told");
        assert!(audits.lock().unwrap().iter().any(|a| a == "settings.display.dock pin radio"));
        // the dock holds ten
        for id in ["calendar", "media", "store", "usage"] {
            let _ = t.call("display_dock_set", &json!({"op": "pin", "app": id}));
        }
        assert!(t.call("display_dock_set", &json!({"op": "pin", "app": "scan"})).unwrap_err().contains("at most 10"));
        assert!(TOOLS.contains(&"display_dock") && TOOLS.contains(&"display_dock_set"));
    }
}
