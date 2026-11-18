//! Looks: a theme that changes more than colours (F-LOOK-01.., docs/development/LOOK-DESIGN.md).
//!
//! A [`Look`] holds every part the design lists: the colours (the existing [`Theme`]), the wallpaper, the icons, the launcher, the
//! dock, the top bar, the menu, the home widgets and the motion. This module is **data and format only**: nothing here draws, and
//! nothing in the shell reads it yet. [`Look::default`] is what the shell does today, so a Look nobody chose changes nothing.
//!
//! The file format v2 is plain text, still `key = value`, `#` for comments, one `[section]` per part, lists as `[a, b, c]`. A v1
//! `.theme` file (no sections: `name`, `base`, `ring`, colours) is read unchanged and is a Look with only colours. See
//! [`parse`] for the rules and `docs/development/LOOK-DESIGN.md` for every key.
//!
//! Safety rules (design, "Rules that keep a Look safe"): names come only from closed lists of built-in parts; numbers are in range;
//! the colours pass the contrast rules of [`crate::usertheme::check`]; the essential controls (sign out and lock, power, the bell,
//! Settings, the "needs you" indicator) are put back when a file leaves them out, and [`Accepted::fixes`] says so. A Look with any
//! other problem is refused as a whole with every problem listed by line; the caller keeps the Look the person had.

use std::fmt;

use crate::theme::{Theme, ThemeId};
use crate::usertheme;
use crate::wallpaper;

/// Biggest Look file (a v1 theme file is 4096; a Look has more parts), most lines, longest line, most items in a list.
pub const MAX_BYTES: u64 = 8192;
pub const MAX_LINES: usize = 300;
pub const MAX_LINE: usize = 400;
pub const MAX_LIST: usize = 24;

macro_rules! closed {
    ($(#[$m:meta])* $name:ident { $($var:ident = $key:literal),+ $(,)? } default $def:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum $name { $($var),+ }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$var),+];
            /// The word written in a Look file.
            pub fn key(self) -> &'static str { match self { $($name::$var => $key),+ } }
            pub fn from_key(k: &str) -> Option<$name> { Self::ALL.iter().copied().find(|v| v.key() == k) }
            /// The accepted words, for an error message.
            pub fn keys() -> String { Self::ALL.iter().map(|v| v.key()).collect::<Vec<_>>().join(", ") }
        }
        impl Default for $name { fn default() -> Self { $name::$def } }
    };
}

closed!(
    /// The shape of an app icon's body.
    IconShape { Squircle = "squircle", Circle = "circle", Rounded = "rounded", Square = "square" } default Squircle
);
closed!(
    /// How an icon body is drawn. Glass and 3d are data only until the drawing exists.
    IconStyle { Gradient = "gradient", Flat = "flat", Outline = "outline", Glass = "glass", ThreeD = "3d" } default Gradient
);
closed!(
    /// The launcher's arrangement.
    LauncherLayout { Paged = "paged", Grid = "grid", List = "list" } default Paged
);
closed!(Density { Compact = "compact", Comfortable = "comfortable", Roomy = "roomy" } default Comfortable);
closed!(
    /// Where the launcher's groups (Ask & Create, Build, ...) are shown.
    Groups { Tabs = "tabs", Sidebar = "sidebar", None = "none" } default Tabs
);
closed!(DockPos { Bottom = "bottom", Left = "left", Right = "right", Off = "off" } default Bottom);
closed!(BarPos { Top = "top", Bottom = "bottom" } default Top);
closed!(MenuPos { Left = "left", Right = "right", Top = "top", Bottom = "bottom" } default Left);
closed!(Motion { Full = "full", Reduced = "reduced" } default Full);
closed!(
    /// Corner radius scale of the SDK widgets (a skin parameter, see `tokens`).
    Shape { Sharp = "sharp", Soft = "soft", Round = "round", Pill = "pill" } default Soft
);
closed!(
    /// Border width scale of the SDK widgets.
    Edge { Thin = "thin", Regular = "regular", Heavy = "heavy" } default Regular
);
closed!(
    /// Spacing scale: how much room the SDK widgets and apps leave between things.
    Spacing { Tight = "tight", Normal = "normal", Airy = "airy" } default Normal
);
closed!(
    /// How much shadow lifts raised surfaces.
    Elevation { Flat = "flat", Low = "low", High = "high" } default Flat
);
closed!(
    /// The parts a top bar can show; drawn by the shell, never code from a file.
    BarItem {
        Clock = "clock", Battery = "battery", Network = "network", Volume = "volume", Bell = "bell", NeedsYou = "needs_you",
        Apps = "apps", Settings = "settings", Account = "account", Power = "power"
    } default Clock
);
closed!(
    /// The entries a menu can offer; a Look reorders or leaves out, never adds a command.
    MenuEntry {
        Home = "home", Apps = "apps", Files = "files", Chat = "chat", Settings = "settings", Lock = "lock", SignOut = "sign_out",
        Power = "power"
    } default Home
);
closed!(WidgetKind {
    Attention = "attention", Approvals = "approvals", Vitals = "vitals", Runs = "runs", Tokens = "tokens", Network = "network"
} default Attention);
closed!(WidgetSize { Small = "S", Medium = "M", Large = "L" } default Medium);

/// The apps the dock holds when the person pinned none and the Look names none, in order (the shell keeps the same list and a test says so).
pub const DEFAULT_DOCK_IDS: [&str; 6] = ["chat", "browser", "ide", "edit", "cli", "system"];

/// Controls that stay reachable in every Look (design rule 1). The top bar holds all five; the menu holds the last four of its own.
pub const ESSENTIAL_BAR: [BarItem; 5] = [BarItem::NeedsYou, BarItem::Bell, BarItem::Settings, BarItem::Account, BarItem::Power];
pub const ESSENTIAL_MENU: [MenuEntry; 4] = [MenuEntry::Settings, MenuEntry::Lock, MenuEntry::SignOut, MenuEntry::Power];

/// `S`, `M`, `L` or an exact pixel size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconSize {
    S,
    M,
    L,
    Px(u16),
}

impl IconSize {
    pub const PX: std::ops::RangeInclusive<u16> = 24..=128;
    pub fn text(self) -> String {
        match self {
            IconSize::S => "S".into(),
            IconSize::M => "M".into(),
            IconSize::L => "L".into(),
            IconSize::Px(p) => p.to_string(),
        }
    }
}

/// How many tile columns: the screen decides (today), or a fixed 3 to 8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Columns {
    Auto,
    Fixed(u8),
}

impl Columns {
    pub const RANGE: std::ops::RangeInclusive<u8> = 3..=8;
}

#[derive(Clone, Debug, PartialEq)]
pub struct Icons {
    pub shape: IconShape,
    pub size: IconSize,
    pub style: IconStyle,
    pub labels: bool,
    /// The glyph set: only the built-in one for now.
    pub set: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Launcher {
    pub layout: LauncherLayout,
    pub columns: Columns,
    pub density: Density,
    pub page_dots: bool,
    pub groups: Groups,
    pub island: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dock {
    pub position: DockPos,
    /// The person's pinned apps, in order (app ids; the shell shows only those the person may use; empty means the shell chooses).
    pub items: Vec<String>,
    /// A dot under apps that are open.
    pub running: bool,
    pub size: u16,
    pub magnify: bool,
    pub autohide: bool,
}

impl Dock {
    pub const SIZE: std::ops::RangeInclusive<u16> = 24..=128;
}

#[derive(Clone, Debug, PartialEq)]
pub struct TopBar {
    pub on: bool,
    pub position: BarPos,
    pub items: Vec<BarItem>,
    pub size: u16,
    pub footer_hints: bool,
}

impl TopBar {
    pub const SIZE: std::ops::RangeInclusive<u16> = 20..=64;
}

#[derive(Clone, Debug, PartialEq)]
pub struct MenuPart {
    /// The entries shown, in order; an entry not listed is hidden (except the essential ones, which are put back).
    pub entries: Vec<MenuEntry>,
    pub position: MenuPos,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    pub kind: WidgetKind,
    pub size: WidgetSize,
}

/// The parts of a skin beyond colour: shape, borders, spacing, shadow, glass and blur, the glyph size of the SDK widgets.
/// The numbers they mean are in [`crate::tokens::Tokens`]; an app reads those, never these words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Surface {
    pub shape: Shape,
    pub edge: Edge,
    pub spacing: Spacing,
    pub elevation: Elevation,
    /// Frosted surfaces (a translucent fill over a blur) where the renderer can; off draws plain fills.
    pub glass: bool,
    pub blur: u8,
    /// The size of a glyph or small icon inside an SDK widget, in logical units.
    pub glyph: u16,
}

impl Surface {
    pub const BLUR: std::ops::RangeInclusive<u8> = 0..=24;
    pub const GLYPH: std::ops::RangeInclusive<u16> = 12..=48;
}

impl Default for Surface {
    /// What the SDK widgets drew before skins existed.
    fn default() -> Surface {
        Surface { shape: Shape::Soft, edge: Edge::Regular, spacing: Spacing::Normal, elevation: Elevation::Flat, glass: false, blur: 12, glyph: 20 }
    }
}

/// Every part of one Look.
#[derive(Clone, Debug)]
pub struct Look {
    pub name: String,
    pub theme: Theme,
    pub wallpaper: wallpaper::Id,
    pub icons: Icons,
    pub launcher: Launcher,
    pub dock: Dock,
    pub topbar: TopBar,
    pub menu: MenuPart,
    pub widgets: Vec<Widget>,
    pub motion: Motion,
    pub surface: Surface,
}

impl PartialEq for Look {
    fn eq(&self, o: &Look) -> bool {
        // `Theme` has no `PartialEq` (it holds floats); its `Debug` text covers every field exactly.
        self.name == o.name
            && format!("{:?}", self.theme) == format!("{:?}", o.theme)
            && self.wallpaper == o.wallpaper
            && self.icons == o.icons
            && self.launcher == o.launcher
            && self.dock == o.dock
            && self.topbar == o.topbar
            && self.menu == o.menu
            && self.widgets == o.widgets
            && self.motion == o.motion
            && self.surface == o.surface
    }
}

impl Default for Look {
    /// What the shell does today: the Dark theme, the theme's own wallpaper, squircle gradient icons with labels, the paged island
    /// with the screen choosing the columns, the top bar on top with every essential control, the dock at the bottom (the owner's
    /// decision of 2026-11-15; nothing draws it yet), no widgets, full motion.
    fn default() -> Look {
        Look {
            name: "Default".into(),
            theme: *ThemeId::Dark.theme(),
            wallpaper: wallpaper::Id::default(),
            icons: Icons { shape: IconShape::Squircle, size: IconSize::M, style: IconStyle::Gradient, labels: true, set: "builtin".into() },
            launcher: Launcher {
                layout: LauncherLayout::Paged,
                columns: Columns::Auto,
                density: Density::Comfortable,
                page_dots: true,
                groups: Groups::Tabs,
                island: true,
            },
            dock: Dock { position: DockPos::Bottom, items: vec![], running: true, size: 56, magnify: false, autohide: false },
            topbar: TopBar {
                on: true,
                position: BarPos::Top,
                items: vec![
                    BarItem::Clock,
                    BarItem::Battery,
                    BarItem::Network,
                    BarItem::Volume,
                    BarItem::NeedsYou,
                    BarItem::Bell,
                    BarItem::Settings,
                    BarItem::Account,
                    BarItem::Power,
                ],
                size: 32,
                footer_hints: true,
            },
            menu: MenuPart {
                entries: vec![
                    MenuEntry::Home,
                    MenuEntry::Apps,
                    MenuEntry::Files,
                    MenuEntry::Chat,
                    MenuEntry::Settings,
                    MenuEntry::Lock,
                    MenuEntry::SignOut,
                    MenuEntry::Power,
                ],
                position: MenuPos::Left,
            },
            widgets: vec![],
            motion: Motion::Full,
            surface: Surface::default(),
        }
    }
}

impl Look {
    /// Denser launcher (seven columns, compact rows), small icons, a slimmer top bar, a slimmer dock.
    pub fn compact() -> Look {
        let mut l = Look::default();
        l.name = "Compact".into();
        l.icons.size = IconSize::S;
        l.launcher.density = Density::Compact;
        l.launcher.columns = Columns::Fixed(7);
        l.dock.size = 40;
        l.topbar.size = 24;
        l
    }

    /// Today's grid-like layout: a plain grid with no island and no dock, rounded icons.
    pub fn classic() -> Look {
        let mut l = Look::default();
        l.name = "Classic".into();
        l.icons.shape = IconShape::Rounded;
        l.launcher.layout = LauncherLayout::Grid;
        l.launcher.island = false;
        l.dock.position = DockPos::Off;
        l
    }

    /// The three built-in Looks, in the order Settings offers them.
    pub fn builtin() -> Vec<Look> {
        vec![Look::default(), Look::compact(), Look::classic()]
    }

    /// The key a Look is chosen by (the file name for a file).
    pub fn key(&self) -> String {
        usertheme::key_of(&self.name)
    }

    /// The Look as file text (format v2, every key written, so nothing depends on a default of a later release).
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        let t = &self.theme;
        s.push_str("# IYAKKI Look, file format 2\n[look]\n");
        s.push_str(&format!("name = {}\nformat = 2\n\n[colours]\nbase = {}\nring = {}\n", self.name, t.id.key(), t.ring));
        for (k, c) in colours_of(t) {
            s.push_str(&format!("{k} = {}\n", hex(c)));
        }
        s.push_str(&format!("\n[wallpaper]\nwallpaper = {}\n", self.wallpaper.key()));
        let i = &self.icons;
        s.push_str(&format!(
            "\n[icons]\nicon_shape = {}\nicon_size = {}\nicon_style = {}\nicon_labels = {}\nicon_set = {}\n",
            i.shape.key(),
            i.size.text(),
            i.style.key(),
            onoff(i.labels),
            i.set
        ));
        let l = &self.launcher;
        let cols = match l.columns {
            Columns::Auto => "auto".to_string(),
            Columns::Fixed(n) => n.to_string(),
        };
        s.push_str(&format!(
            "\n[launcher]\nlauncher = {}\ncolumns = {}\ndensity = {}\npage_dots = {}\ngroups = {}\nisland = {}\n",
            l.layout.key(),
            cols,
            l.density.key(),
            onoff(l.page_dots),
            l.groups.key(),
            onoff(l.island)
        ));
        let d = &self.dock;
        s.push_str(&format!(
            "\n[dock]\ndock = {}\ndock_items = {}\ndock_running = {}\ndock_size = {}\ndock_magnify = {}\ndock_autohide = {}\n",
            d.position.key(),
            list(d.items.iter().map(String::as_str)),
            onoff(d.running),
            d.size,
            onoff(d.magnify),
            onoff(d.autohide)
        ));
        let b = &self.topbar;
        s.push_str(&format!(
            "\n[topbar]\ntopbar = {}\ntopbar_position = {}\ntopbar_items = {}\ntopbar_size = {}\nfooter_hints = {}\n",
            onoff(b.on),
            b.position.key(),
            list(b.items.iter().map(|x| x.key())),
            b.size,
            onoff(b.footer_hints)
        ));
        s.push_str(&format!("\n[menu]\nmenu = {}\nmenu_position = {}\n", list(self.menu.entries.iter().map(|x| x.key())), self.menu.position.key()));
        let w: Vec<String> = self.widgets.iter().map(|w| format!("{}:{}", w.kind.key(), w.size.key())).collect();
        s.push_str(&format!("\n[widgets]\nwidgets = {}\n", list(w.iter().map(String::as_str))));
        s.push_str(&format!("\n[motion]\nmotion = {}\n", self.motion.key()));
        let f = &self.surface;
        s.push_str(&format!("\n[shape]\nshape = {}\nedge = {}\n", f.shape.key(), f.edge.key()));
        s.push_str(&format!("\n[spacing]\nspacing = {}\n", f.spacing.key()));
        s.push_str(&format!("\n[effects]\nelevation = {}\nglass = {}\nblur = {}\nglyph_size = {}\n", f.elevation.key(), onoff(f.glass), f.blur, f.glyph));
        s
    }

    /// Read and check a Look from text, ignoring what was put back (use [`parse`] to see that).
    pub fn from_text(text: &str) -> Result<Look, Vec<LookProblem>> {
        parse(text).map(|a| a.look)
    }
}

fn onoff(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

fn list<'a>(items: impl Iterator<Item = &'a str>) -> String {
    format!("[{}]", items.collect::<Vec<_>>().join(", "))
}

fn hex(c: [u8; 4]) -> String {
    if c[3] == 255 { format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]) } else { format!("#{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3]) }
}

/// The 20 colour keys in file order.
pub const COLOUR_KEYS: [&str; 20] = [
    "bg_top", "bg_bottom", "card", "card_hover", "card_sel", "border", "accent", "text", "muted", "dim", "err", "ok", "field", "panel", "well",
    "amber", "cyan", "magenta", "button", "button_hot",
];

fn colours_of(t: &Theme) -> Vec<(&'static str, [u8; 4])> {
    let mut probe = *t;
    COLOUR_KEYS.iter().filter_map(|k| usertheme::get(&mut probe, k).map(|s| (*k, *s))).collect()
}

fn colour(v: &str) -> Option<[u8; 4]> {
    let h = v.trim().strip_prefix('#')?;
    if !(h.len() == 6 || h.len() == 8) || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(h, 16).ok()?;
    Some(if h.len() == 6 { [(n >> 16) as u8, (n >> 8) as u8, n as u8, 255] } else { [(n >> 24) as u8, (n >> 16) as u8, (n >> 8) as u8, n as u8] })
}

// ------------------------------------------------------------------------------------------------------------ problems

/// One reason a Look file is refused, in plain words. `line` is 1-based, 0 when it concerns the whole file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LookProblem {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for LookProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 { write!(f, "{}", self.message) } else { write!(f, "line {}: {}", self.line, self.message) }
    }
}

/// A Look that passed, and what was put back or changed so that it is safe ("the top bar was missing the bell: added it").
#[derive(Clone, Debug, PartialEq)]
pub struct Accepted {
    pub look: Look,
    pub fixes: Vec<String>,
}

/// Every problem as one plain sentence per line (for a refusal message or a log line).
pub fn problems_text(p: &[LookProblem]) -> String {
    p.iter().map(|x| x.to_string()).collect::<Vec<_>>().join("; ")
}

// ------------------------------------------------------------------------------------------------------------ the parser

const SECTIONS: [&str; 13] =
    ["look", "colours", "wallpaper", "icons", "launcher", "dock", "topbar", "menu", "widgets", "motion", "shape", "spacing", "effects"];

fn section_of(key: &str) -> Option<&'static str> {
    Some(match key {
        "name" | "format" => "look",
        "base" | "ring" => "colours",
        "wallpaper" => "wallpaper",
        "icon_shape" | "icon_size" | "icon_style" | "icon_labels" | "icon_set" => "icons",
        "launcher" | "columns" | "density" | "page_dots" | "groups" | "island" => "launcher",
        "dock" | "dock_items" | "dock_running" | "dock_size" | "dock_magnify" | "dock_autohide" => "dock",
        "topbar" | "topbar_position" | "topbar_items" | "topbar_size" | "footer_hints" => "topbar",
        "menu" | "menu_position" => "menu",
        "widgets" => "widgets",
        "motion" => "motion",
        "shape" | "edge" => "shape",
        "spacing" => "spacing",
        "elevation" | "glass" | "blur" | "glyph_size" => "effects",
        k if COLOUR_KEYS.contains(&k) => "colours",
        _ => return None,
    })
}

fn flag(v: &str) -> Result<bool, String> {
    match v {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err("write `on` or `off`".into()),
    }
}

fn word<T: Copy>(v: &str, from: fn(&str) -> Option<T>, all: String) -> Result<T, String> {
    from(v).ok_or_else(|| format!("`{v}` is not one of: {all}"))
}

/// A `[a, b, c]` list of items.
fn items(v: &str) -> Result<Vec<String>, String> {
    let inner = v.trim().strip_prefix('[').and_then(|r| r.strip_suffix(']')).ok_or("write a list like [a, b, c]")?;
    let mut parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    if parts.last() == Some(&"") {
        parts.pop();
    }
    if parts.iter().any(|s| s.is_empty()) {
        return Err("the list has an empty item".into());
    }
    if parts.len() > MAX_LIST {
        return Err(format!("a list holds at most {MAX_LIST} items"));
    }
    let out: Vec<String> = parts.into_iter().map(String::from).collect();
    Ok(out)
}

fn names<T: Copy + PartialEq>(v: &str, from: fn(&str) -> Option<T>, all: String) -> Result<Vec<T>, String> {
    let mut out: Vec<T> = Vec::new();
    for s in items(v)? {
        let x = from(&s).ok_or_else(|| format!("`{s}` is not one of: {all}"))?;
        if out.contains(&x) {
            return Err(format!("`{s}` is listed twice"));
        }
        out.push(x);
    }
    Ok(out)
}

fn app_id_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.'))
}

/// Set one key of `look` from its text. Syntax only; ranges and cross-part rules are [`check`].
fn set(look: &mut Look, key: &str, v: &str) -> Result<(), String> {
    match key {
        "format" => {
            if v != "2" {
                return Err("format is 2 (a file with no sections is read as format 1)".into());
            }
        }
        "wallpaper" => {
            let id = wallpaper::Id::from_key(v);
            if id.key() != v {
                return Err(format!("`{v}` is not one of: {}", wallpaper::ALL.iter().map(|w| w.key()).collect::<Vec<_>>().join(", ")));
            }
            look.wallpaper = id;
        }
        "icon_shape" => look.icons.shape = word(v, IconShape::from_key, IconShape::keys())?,
        "icon_size" => {
            look.icons.size = match v {
                "S" => IconSize::S,
                "M" => IconSize::M,
                "L" => IconSize::L,
                _ => IconSize::Px(v.parse::<u16>().map_err(|_| "write S, M, L or a size in pixels (24 to 128)".to_string())?),
            }
        }
        "icon_style" => look.icons.style = word(v, IconStyle::from_key, IconStyle::keys())?,
        "icon_labels" => look.icons.labels = flag(v)?,
        "icon_set" => {
            if v != "builtin" {
                return Err("only the built-in glyph set `builtin` exists for now".into());
            }
            look.icons.set = v.into();
        }
        "launcher" => look.launcher.layout = word(v, LauncherLayout::from_key, LauncherLayout::keys())?,
        "columns" => {
            look.launcher.columns = if v == "auto" {
                Columns::Auto
            } else {
                Columns::Fixed(v.parse::<u8>().map_err(|_| "write `auto` or a number from 3 to 8".to_string())?)
            }
        }
        "density" => look.launcher.density = word(v, Density::from_key, Density::keys())?,
        "page_dots" => look.launcher.page_dots = flag(v)?,
        "groups" => look.launcher.groups = word(v, Groups::from_key, Groups::keys())?,
        "island" => look.launcher.island = flag(v)?,
        "dock" => look.dock.position = word(v, DockPos::from_key, DockPos::keys())?,
        "dock_items" => {
            let mut out: Vec<String> = Vec::new();
            for s in items(v)? {
                if !app_id_ok(&s) {
                    return Err(format!("`{s}` is not an app id (lower case letters, digits, dashes, up to 32)"));
                }
                if out.contains(&s) {
                    return Err(format!("`{s}` is listed twice"));
                }
                out.push(s);
            }
            look.dock.items = out;
        }
        "dock_running" => look.dock.running = flag(v)?,
        "dock_size" => look.dock.size = v.parse::<u16>().map_err(|_| "write a size in pixels (24 to 128)".to_string())?,
        "dock_magnify" => look.dock.magnify = flag(v)?,
        "dock_autohide" => look.dock.autohide = flag(v)?,
        "topbar" => look.topbar.on = flag(v)?,
        "topbar_position" => look.topbar.position = word(v, BarPos::from_key, BarPos::keys())?,
        "topbar_items" => look.topbar.items = names(v, BarItem::from_key, BarItem::keys())?,
        "topbar_size" => look.topbar.size = v.parse::<u16>().map_err(|_| "write a size in pixels (20 to 64)".to_string())?,
        "footer_hints" => look.topbar.footer_hints = flag(v)?,
        "menu" => look.menu.entries = names(v, MenuEntry::from_key, MenuEntry::keys())?,
        "menu_position" => look.menu.position = word(v, MenuPos::from_key, MenuPos::keys())?,
        "widgets" => {
            let mut out: Vec<Widget> = Vec::new();
            for s in items(v)? {
                let (k, sz) = s.split_once(':').unwrap_or((s.as_str(), "M"));
                let kind = word(k.trim(), WidgetKind::from_key, WidgetKind::keys())?;
                let size = word(sz.trim(), WidgetSize::from_key, WidgetSize::keys())?;
                if out.iter().any(|w| w.kind == kind) {
                    return Err(format!("`{k}` is listed twice"));
                }
                out.push(Widget { kind, size });
            }
            look.widgets = out;
        }
        "motion" => look.motion = word(v, Motion::from_key, Motion::keys())?,
        "shape" => look.surface.shape = word(v, Shape::from_key, Shape::keys())?,
        "edge" => look.surface.edge = word(v, Edge::from_key, Edge::keys())?,
        "spacing" => look.surface.spacing = word(v, Spacing::from_key, Spacing::keys())?,
        "elevation" => look.surface.elevation = word(v, Elevation::from_key, Elevation::keys())?,
        "glass" => look.surface.glass = flag(v)?,
        "blur" => look.surface.blur = v.parse::<u8>().map_err(|_| "write a number of pixels from 0 to 24".to_string())?,
        "glyph_size" => look.surface.glyph = v.parse::<u16>().map_err(|_| "write a size in logical units (12 to 48)".to_string())?,
        _ => return Err("internal: unknown key".into()),
    }
    Ok(())
}

/// Read a Look file's text. Every problem found is returned together (so a person fixes the file once), the Look only when there is
/// none; what was put back is in [`Accepted::fixes`]. Never panics, whatever the text.
///
/// - Before any `[section]` a v1 theme file's keys (`name`, `base`, `ring`, colours) are accepted: that file is a Look with only colours.
/// - Otherwise a key belongs to one section (`columns` to `[launcher]`); the wrong section, an unknown key or section, a key written
///   twice, a name outside the closed lists and a number out of range are each refused with the line.
pub fn parse(text: &str) -> Result<Accepted, Vec<LookProblem>> {
    let mut bad: Vec<LookProblem> = Vec::new();
    macro_rules! problem {
        ($l:expr, $m:expr) => {
            bad.push(LookProblem { line: $l, message: $m })
        };
    }
    if text.len() as u64 > MAX_BYTES {
        return Err(vec![LookProblem { line: 0, message: format!("the file is bigger than {MAX_BYTES} bytes") }]);
    }
    let mut look = Look::default();
    let mut name_seen = false;
    let mut base = ThemeId::Dark;
    let mut ring: Option<f32> = None;
    let mut cols: Vec<(String, [u8; 4])> = Vec::new();
    let mut seen: Vec<(String, usize)> = Vec::new();
    let mut section = "";
    let mut sections_seen: Vec<&str> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let at = i + 1;
        if at > MAX_LINES {
            problem!(at, format!("the file has more than {MAX_LINES} lines"));
            break;
        }
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if raw.len() > MAX_LINE {
            problem!(at, format!("the line is longer than {MAX_LINE} characters"));
            continue;
        }
        if let Some(inner) = line.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            let s = inner.trim();
            match SECTIONS.iter().find(|x| **x == s) {
                Some(x) if sections_seen.contains(x) => problem!(at, format!("the section [{s}] is written twice")),
                Some(x) => {
                    section = x;
                    sections_seen.push(x);
                }
                None => problem!(at, format!("`[{s}]` is not a section of a look (the sections are: {})", SECTIONS.join(", "))),
            }
            if !SECTIONS.contains(&s) {
                section = "?";
            }
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            problem!(at, "expected `key = value`, a `[section]` or a `#` comment".into());
            continue;
        };
        let k = k.trim();
        let v = if k == "name" { v.trim() } else { v.trim().split(" #").next().unwrap_or("").trim() };
        if section == "?" {
            continue;
        }
        let Some(home) = section_of(k) else {
            problem!(at, format!("`{k}` is not a key of a look"));
            continue;
        };
        if !section.is_empty() && home != section {
            problem!(at, format!("`{k}` belongs in [{home}], not [{section}]"));
            continue;
        }
        if seen.iter().any(|(s, _)| s == k) {
            problem!(at, format!("`{k}` is written twice"));
            continue;
        }
        seen.push((k.to_string(), at));
        match k {
            "name" => {
                if usertheme::name_ok(v) {
                    look.name = v.to_string();
                    name_seen = true;
                } else {
                    problem!(at, "the name is 1 to 32 letters, digits, spaces, dashes, underscores or dots".into());
                }
            }
            "base" => match crate::theme::ALL.iter().find(|t| t.key() == v) {
                Some(t) => base = *t,
                None => problem!(at, "base is dark, light or high-contrast".into()),
            },
            "ring" => match v.parse::<f32>() {
                Ok(r) if (1.0..=3.0).contains(&r) => ring = Some(r),
                _ => problem!(at, "ring is a number from 1.0 to 3.0".into()),
            },
            _ if COLOUR_KEYS.contains(&k) => match colour(v) {
                Some(c) => cols.push((k.to_string(), c)),
                None => problem!(at, format!("`{k}` is written #rrggbb")),
            },
            _ => {
                if let Err(m) = set(&mut look, k, v) {
                    problem!(at, format!("`{k}`: {m}"));
                }
            }
        }
    }
    if !name_seen && bad.is_empty() {
        bad.push(LookProblem { line: 0, message: "the file needs a `name = ...` line".into() });
    }
    let mut t = *base.theme();
    for (k, c) in &cols {
        if let Some(slot) = usertheme::get(&mut t, k) {
            *slot = *c;
        }
    }
    if let Some(r) = ring {
        t.ring = r;
    }
    look.theme = t;
    let line_of = |key: &str| seen.iter().find(|(s, _)| s == key).map_or(0, |(_, l)| *l);
    for (key, m) in check(&look) {
        bad.push(LookProblem { line: line_of(key), message: m });
    }
    if !bad.is_empty() {
        return Err(bad);
    }
    Ok(make_safe(look))
}

/// Check a Look made in code (a built-in, or one a settings page edited) with the same rules as a file, and put back what is missing.
pub fn validate(look: &Look) -> Result<Accepted, Vec<LookProblem>> {
    let bad: Vec<LookProblem> = check(look).into_iter().map(|(_, m)| LookProblem { line: 0, message: m }).collect();
    if bad.is_empty() { Ok(make_safe(look.clone())) } else { Err(bad) }
}

/// The refusals: `(the key to point at, the reason)`. Does not change the Look.
fn check(l: &Look) -> Vec<(&'static str, String)> {
    let mut v: Vec<(&'static str, String)> = Vec::new();
    if !usertheme::name_ok(&l.name) {
        v.push(("name", "the name is 1 to 32 letters, digits, spaces, dashes, underscores or dots".into()));
    }
    if let Err(e) = usertheme::check(&l.theme) {
        v.push(("base", format!("colours: {e}")));
    }
    if let IconSize::Px(p) = l.icons.size {
        if !IconSize::PX.contains(&p) {
            v.push(("icon_size", format!("{p} is out of range: write S, M, L or a size from 24 to 128 pixels")));
        }
    }
    if let Columns::Fixed(n) = l.launcher.columns {
        if !Columns::RANGE.contains(&n) {
            v.push(("columns", format!("{n} is out of range: write `auto` or a number from 3 to 8")));
        }
    }
    if !Dock::SIZE.contains(&l.dock.size) {
        v.push(("dock_size", format!("{} is out of range: write a size from 24 to 128 pixels", l.dock.size)));
    }
    if !TopBar::SIZE.contains(&l.topbar.size) {
        v.push(("topbar_size", format!("{} is out of range: write a size from 20 to 64 pixels", l.topbar.size)));
    }
    if !Surface::BLUR.contains(&l.surface.blur) {
        v.push(("blur", format!("{} is out of range: write a number of pixels from 0 to 24", l.surface.blur)));
    }
    if !Surface::GLYPH.contains(&l.surface.glyph) {
        v.push(("glyph_size", format!("{} is out of range: write a size from 12 to 48", l.surface.glyph)));
    }
    if l.dock.items.len() > MAX_LIST || l.dock.items.iter().any(|s| !app_id_ok(s)) {
        v.push(("dock_items", format!("the dock holds at most {MAX_LIST} app ids (lower case letters, digits, dashes)")));
    }
    if l.dock.position == DockPos::Bottom && l.topbar.on && l.topbar.position == BarPos::Bottom {
        v.push(("topbar_position", "the top bar and the dock cannot both be at the bottom".into()));
    }
    v
}

/// Put back what is essential (design rule 1) and say so.
fn make_safe(mut look: Look) -> Accepted {
    let mut fixes = Vec::new();
    for e in ESSENTIAL_BAR {
        if !look.topbar.items.contains(&e) {
            look.topbar.items.push(e);
            fixes.push(format!("the top bar was missing `{}`: added it at the end (it must stay reachable)", e.key()));
        }
    }
    for e in ESSENTIAL_MENU {
        if !look.menu.entries.contains(&e) {
            look.menu.entries.push(e);
            fixes.push(format!("the menu was missing `{}`: added it at the end (it must stay reachable)", e.key()));
        }
    }
    Accepted { look, fixes }
}

#[cfg(test)]
#[path = "look_tests.rs"]
mod tests;
