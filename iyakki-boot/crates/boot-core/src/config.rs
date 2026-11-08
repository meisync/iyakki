// SPDX-License-Identifier: MIT OR Apache-2.0
//! Parser for the line-based boot config file (`boot.cfg` by default).
//!
//! ```text
//! # comment
//! timeout 5
//! default current
//! tries 3
//! scan yes
//! lock-cmdline yes
//! splash /splash.bmp
//! splash-bg #101820
//! splash-ms 1500
//!
//! entry current
//!   title Main system
//!   kernel /{tree}/vmlinuz
//!   initrd /{tree}/initrd.img
//!   cmdline quiet
//!   kind linux
//!   default
//!   order 10
//!   hold r
//!
//! override efi:0:1:/efi/vendor/boot.efi
//!   name Other system
//!   hidden
//!   order 50
//! ```
//!
//! Keys and values are separated by white space or `=`. Flags (`default`, `hidden`) take no value or yes/no.
//! `{tree}` in a kernel or initrd path becomes `current` or `previous` (see `assess`).

use crate::choose::HoldKey;
use crate::cmdline;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// Largest file read.
pub const MAX_FILE: usize = 32 * 1024;
/// Longest line.
pub const MAX_LINE: usize = 512;
/// Most `entry` blocks.
pub const MAX_ENTRIES: usize = 48;
/// Most `override` blocks.
pub const MAX_OVERRIDES: usize = 64;
/// Most initrds per entry.
pub const MAX_INITRDS: usize = 4;
/// Longest entry name or override id.
pub const MAX_ID: usize = 128;
/// Longest title.
pub const MAX_TITLE: usize = 64;
/// Longest path.
pub const MAX_PATH: usize = 255;
/// Longest `splash-ms`.
pub const MAX_SPLASH_MS: u32 = 10_000;
/// Longest timeout in seconds.
pub const MAX_TIMEOUT: u32 = 600;
/// Most ids on a `previous` line (all lines of one entry together).
pub const MAX_PREVIOUS: usize = 4;
/// Default `order` of a configured entry.
pub const DEFAULT_ORDER: i32 = 100;
/// Tries before a rollback when `tries` is not set.
pub const DEFAULT_TRIES: u8 = 3;

/// What an entry starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A Linux kernel (EFI stub) with initrds.
    Linux,
    /// Another EFI program; `kernel` is its path.
    Chain,
    /// Restart into the firmware's own setup screen (no kernel; the entry is dropped when the firmware cannot do it).
    Firmware,
}

/// One `entry` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Name (also its id).
    pub id: String,
    /// Menu text; the id when not given.
    pub title: String,
    /// Linux or chain.
    pub kind: Kind,
    /// Kernel or loader path.
    pub kernel: String,
    /// Initrd paths.
    pub initrd: Vec<String>,
    /// Kernel command line.
    pub cmdline: String,
    /// Marked default.
    pub default: bool,
    /// Not listed in the menu.
    pub hidden: bool,
    /// Sort key.
    pub order: i32,
    /// Hold-a-key selection.
    pub hold: Option<HoldKey>,
    /// Listed on the second level ("More options") instead of the first.
    pub more: bool,
    /// Path on the boot volume that must exist, else the entry is left out (`requires`).
    pub requires: Option<String>,
    /// Ids of entries that start in place of this one when the boot state says "previous version" (`previous`, the first one that is
    /// listed and startable wins). Empty: this entry has no previous version of its own.
    pub previous: Vec<String>,
}

impl Entry {
    /// Kernel and initrd paths with `{tree}` replaced.
    pub fn paths_for(&self, tree: &str) -> (String, Vec<String>) {
        (self.kernel.replace("{tree}", tree), self.initrd.iter().map(|p| p.replace("{tree}", tree)).collect())
    }
}

/// One `override` block: the person's changes to a discovered item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    /// Discovery id.
    pub id: String,
    /// New title.
    pub name: Option<String>,
    /// Hide or show.
    pub hidden: Option<bool>,
    /// New sort key.
    pub order: Option<i32>,
    /// Make it the default.
    pub default: bool,
}

/// The whole file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Seconds before the default starts; 0 starts at once.
    pub timeout_secs: u32,
    /// Default entry id (an entry or a discovery id).
    pub default: Option<String>,
    /// Failed starts before a rollback.
    pub tries: u8,
    /// Scan for other systems.
    pub scan: bool,
    /// Refuse command line edits under Secure Boot.
    pub lock_cmdline: bool,
    /// Path of the splash picture (an uncompressed 24 or 32-bit BMP, see `splash`); `None` shows no splash.
    pub splash: Option<String>,
    /// Background colour behind the splash and behind the menu while a splash is configured (`splash-bg #RRGGBB`).
    pub splash_bg: Option<[u8; 3]>,
    /// Least time the splash stays up before a normal start, in milliseconds (`splash-ms`, 0 to 10000).
    pub splash_ms: u32,
    /// Entries in file order.
    pub entries: Vec<Entry>,
    /// Overrides for discovered items.
    pub overrides: Vec<Override>,
}

/// What went wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// File over `MAX_FILE`.
    TooLarge,
    /// Not UTF-8.
    NotUtf8,
    /// Line over `MAX_LINE`.
    LineTooLong,
    /// Control character in a line.
    ControlChar,
    /// Key not known in this place.
    UnknownKey(String),
    /// Key needs a value.
    MissingValue(String),
    /// Value rejected; the text says which rule.
    BadValue(&'static str),
    /// Name or id already used.
    Duplicate,
    /// Too many entries or overrides.
    TooMany,
    /// Entry has no kernel.
    NoKernel,
    /// More than one entry marked default.
    TwoDefaults,
    /// No entry at all.
    NoEntries,
}

/// Error with the line number (0 when it is about the whole file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// 1-based line.
    pub line: usize,
    /// Reason.
    pub kind: ErrorKind,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line > 0 {
            write!(f, "line {}: ", self.line)?;
        }
        match &self.kind {
            ErrorKind::TooLarge => write!(f, "file larger than {MAX_FILE} bytes"),
            ErrorKind::NotUtf8 => write!(f, "file is not UTF-8"),
            ErrorKind::LineTooLong => write!(f, "line longer than {MAX_LINE} bytes"),
            ErrorKind::ControlChar => write!(f, "control character"),
            ErrorKind::UnknownKey(k) => write!(f, "unknown key {k:?}"),
            ErrorKind::MissingValue(k) => write!(f, "key {k:?} needs a value"),
            ErrorKind::BadValue(why) => write!(f, "bad value: {why}"),
            ErrorKind::Duplicate => write!(f, "name used twice"),
            ErrorKind::TooMany => write!(f, "too many entries"),
            ErrorKind::NoKernel => write!(f, "entry has no kernel"),
            ErrorKind::TwoDefaults => write!(f, "more than one default entry"),
            ErrorKind::NoEntries => write!(f, "no entry"),
        }
    }
}

/// Result of `Config::load`: always a usable config.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// The parsed config, or the built-in one.
    pub config: Config,
    /// Why the file was not used (`None` when it was, or when it was missing).
    pub error: Option<ParseError>,
    /// The built-in list is in use.
    pub fallback: bool,
}

fn err<T>(line: usize, kind: ErrorKind) -> Result<T, ParseError> {
    Err(ParseError { line, kind })
}

fn valid_id(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_graphic() && b != b'\\' && b != b'"')
}

fn valid_path(s: &str) -> bool {
    s.len() <= MAX_PATH
        && s.starts_with('/')
        && s.bytes().all(|b| b.is_ascii_graphic() && b != b'\\' && b != b'"')
        && !s.split('/').any(|c| c == "..")
}

fn flag(v: &str) -> Result<bool, ErrorKind> {
    match v {
        "" | "yes" | "true" | "1" | "on" => Ok(true),
        "no" | "false" | "0" | "off" => Ok(false),
        _ => Err(ErrorKind::BadValue("expected yes or no")),
    }
}

fn number(v: &str, lo: i64, hi: i64) -> Result<i64, ErrorKind> {
    match v.parse::<i64>() {
        Ok(n) if (lo..=hi).contains(&n) => Ok(n),
        _ => Err(ErrorKind::BadValue("number out of range")),
    }
}

fn title(v: &str) -> Result<String, ErrorKind> {
    if v.is_empty() || v.len() > MAX_TITLE {
        return Err(ErrorKind::BadValue("title empty or too long"));
    }
    Ok(v.to_string())
}

enum Block {
    Top,
    Entry,
    Override,
}

impl Config {
    /// Settings with no entries.
    fn empty() -> Config {
        Config {
            timeout_secs: 5,
            default: None,
            tries: DEFAULT_TRIES,
            scan: false,
            lock_cmdline: true,
            splash: None,
            splash_bg: None,
            splash_ms: 0,
            entries: Vec::new(),
            overrides: Vec::new(),
        }
    }

    /// The built-in list used when the file is missing or broken: `current` (then `seed`), no prompt.
    pub fn fallback() -> Config {
        let mk = |id: &str, dir: &str, order: i32, default: bool| Entry {
            id: id.to_string(),
            title: id.to_string(),
            kind: Kind::Linux,
            kernel: alloc::format!("/{dir}/vmlinuz"),
            initrd: alloc::vec![alloc::format!("/{dir}/initrd.img")],
            cmdline: String::new(),
            default,
            hidden: false,
            order,
            hold: None,
            more: false,
            requires: None,
            previous: Vec::new(),
        };
        let mut c = Config::empty();
        c.timeout_secs = 0;
        c.entries.push(mk("current", "{tree}", DEFAULT_ORDER, true));
        c.entries.push(mk("seed", "seed", DEFAULT_ORDER + 1, false));
        c
    }

    /// Leave out the entries that cannot work on this machine: a `requires` path that is not on the boot volume (`exists` says),
    /// and a `firmware` entry when the firmware cannot restart into its setup (`firmware_setup`). Returns how many were left out.
    pub fn drop_unavailable(&mut self, exists: impl Fn(&str) -> bool, firmware_setup: bool) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| e.requires.as_deref().is_none_or(&exists) && (e.kind != Kind::Firmware || firmware_setup));
        before - self.entries.len()
    }

    /// Use the file when it parses, else the built-in list. `bytes` is `None` when the file is missing.
    pub fn load(bytes: Option<&[u8]>) -> Loaded {
        let Some(bytes) = bytes else { return Loaded { config: Config::fallback(), error: None, fallback: true } };
        let parsed = if bytes.len() > MAX_FILE {
            err(0, ErrorKind::TooLarge)
        } else {
            match core::str::from_utf8(bytes) {
                Ok(t) => Config::parse(t),
                Err(_) => err(0, ErrorKind::NotUtf8),
            }
        };
        match parsed {
            Ok(config) => Loaded { config, error: None, fallback: false },
            Err(e) => Loaded { config: Config::fallback(), error: Some(e), fallback: true },
        }
    }

    /// Parse the text strictly: the first problem is an error.
    pub fn parse(text: &str) -> Result<Config, ParseError> {
        if text.len() > MAX_FILE {
            return err(0, ErrorKind::TooLarge);
        }
        let mut cfg = Config::empty();
        let mut block = Block::Top;
        for (n, raw) in text.lines().enumerate() {
            let ln = n + 1;
            if raw.len() > MAX_LINE {
                return err(ln, ErrorKind::LineTooLong);
            }
            if raw.chars().any(|c| c.is_control() && c != '\t') {
                return err(ln, ErrorKind::ControlChar);
            }
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let split = line.find([' ', '\t', '=']).unwrap_or(line.len());
            let key = &line[..split];
            let mut val = line[split..].trim_start_matches([' ', '\t']);
            if let Some(v) = val.strip_prefix('=') {
                val = v.trim_start_matches([' ', '\t']);
            }
            let val = val.trim_end();
            let r = cfg.line(&mut block, key, val);
            if let Err(kind) = r {
                return err(ln, kind);
            }
        }
        cfg.finish()
    }

    fn line(&mut self, block: &mut Block, key: &str, val: &str) -> Result<(), ErrorKind> {
        let need = |v: &str| if v.is_empty() { Err(ErrorKind::MissingValue(key.to_string())) } else { Ok(()) };
        match key {
            "entry" => {
                need(val)?;
                if !valid_id(val, 32) {
                    return Err(ErrorKind::BadValue("entry name: 1-32 printable characters, no spaces"));
                }
                if self.entries.len() >= MAX_ENTRIES {
                    return Err(ErrorKind::TooMany);
                }
                if self.entries.iter().any(|e| e.id == val) {
                    return Err(ErrorKind::Duplicate);
                }
                self.entries.push(Entry {
                    id: val.to_string(),
                    title: val.to_string(),
                    kind: Kind::Linux,
                    kernel: String::new(),
                    initrd: Vec::new(),
                    cmdline: String::new(),
                    default: false,
                    hidden: false,
                    order: DEFAULT_ORDER,
                    hold: None,
                    more: false,
                    requires: None,
                    previous: Vec::new(),
                });
                *block = Block::Entry;
                return Ok(());
            }
            "override" => {
                need(val)?;
                if !valid_id(val, MAX_ID) {
                    return Err(ErrorKind::BadValue("override id: printable characters, no spaces"));
                }
                if self.overrides.len() >= MAX_OVERRIDES {
                    return Err(ErrorKind::TooMany);
                }
                if self.overrides.iter().any(|o| o.id == val) {
                    return Err(ErrorKind::Duplicate);
                }
                self.overrides.push(Override { id: val.to_string(), name: None, hidden: None, order: None, default: false });
                *block = Block::Override;
                return Ok(());
            }
            _ => {}
        }
        match block {
            Block::Top => match key {
                "timeout" => self.timeout_secs = number(val, 0, MAX_TIMEOUT as i64)? as u32,
                "default" => {
                    need(val)?;
                    if !valid_id(val, MAX_ID) {
                        return Err(ErrorKind::BadValue("default: printable characters, no spaces"));
                    }
                    self.default = Some(val.to_string());
                }
                "tries" => self.tries = number(val, 1, 9)? as u8,
                "scan" => self.scan = flag(val)?,
                "lock-cmdline" => self.lock_cmdline = flag(val)?,
                "splash" => {
                    if !valid_path(val) {
                        return Err(ErrorKind::BadValue("path: absolute, no spaces, no .."));
                    }
                    self.splash = Some(val.to_string());
                }
                "splash-bg" => self.splash_bg = Some(crate::splash::parse_colour(val).ok_or(ErrorKind::BadValue("splash-bg: #RRGGBB"))?),
                "splash-ms" => self.splash_ms = number(val, 0, MAX_SPLASH_MS as i64)? as u32,
                _ => return Err(ErrorKind::UnknownKey(key.to_string())),
            },
            Block::Entry => {
                let e = self.entries.last_mut().expect("entry block has an entry");
                match key {
                    "title" => e.title = title(val)?,
                    "kind" => {
                        e.kind = match val {
                            "linux" => Kind::Linux,
                            "chain" => Kind::Chain,
                            "firmware" => Kind::Firmware,
                            _ => return Err(ErrorKind::BadValue("kind: linux, chain or firmware")),
                        }
                    }
                    "kernel" => {
                        if !valid_path(val) {
                            return Err(ErrorKind::BadValue("path: absolute, no spaces, no .."));
                        }
                        e.kernel = val.to_string();
                    }
                    "initrd" => {
                        if !valid_path(val) {
                            return Err(ErrorKind::BadValue("path: absolute, no spaces, no .."));
                        }
                        if e.initrd.len() >= MAX_INITRDS {
                            return Err(ErrorKind::TooMany);
                        }
                        e.initrd.push(val.to_string());
                    }
                    "cmdline" => {
                        cmdline::validate(val).map_err(|_| ErrorKind::BadValue("cmdline: forbidden character or too long"))?;
                        e.cmdline = val.to_string();
                    }
                    "default" => e.default = flag(val)?,
                    "hidden" => e.hidden = flag(val)?,
                    "more" => e.more = flag(val)?,
                    "requires" => {
                        if !valid_path(val) {
                            return Err(ErrorKind::BadValue("path: absolute, no spaces, no .."));
                        }
                        e.requires = Some(val.to_string());
                    }
                    "previous" => {
                        if val.trim().is_empty() {
                            return Err(ErrorKind::BadValue("previous: entry ids"));
                        }
                        for id in val.split_whitespace() {
                            if !valid_id(id, MAX_ID) {
                                return Err(ErrorKind::BadValue("previous: entry ids"));
                            }
                            if e.previous.len() >= MAX_PREVIOUS {
                                return Err(ErrorKind::TooMany);
                            }
                            e.previous.push(id.to_string());
                        }
                    }
                    "order" => e.order = number(val, -100_000, 100_000)? as i32,
                    "hold" => e.hold = Some(HoldKey::parse(val).ok_or(ErrorKind::BadValue("hold: a letter, digit, shift, esc, space or touch"))?),
                    _ => return Err(ErrorKind::UnknownKey(key.to_string())),
                }
            }
            Block::Override => {
                let o = self.overrides.last_mut().expect("override block has an override");
                match key {
                    "name" => o.name = Some(title(val)?),
                    "hidden" => o.hidden = Some(flag(val)?),
                    "order" => o.order = Some(number(val, -100_000, 100_000)? as i32),
                    "default" => o.default = flag(val)?,
                    _ => return Err(ErrorKind::UnknownKey(key.to_string())),
                }
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<Config, ParseError> {
        if self.entries.is_empty() {
            return err(0, ErrorKind::NoEntries);
        }
        if self.entries.iter().any(|e| e.kernel.is_empty() && e.kind != Kind::Firmware) {
            return err(0, ErrorKind::NoKernel);
        }
        if self.entries.iter().filter(|e| e.default).count() > 1 {
            return err(0, ErrorKind::TwoDefaults);
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "# test\ntimeout 7\ndefault b\ntries 4\nscan yes\nlock-cmdline no\n\
entry a\n  title Alpha\n  kernel /{tree}/k\n  initrd /{tree}/i1\n  initrd /{tree}/i2\n  cmdline quiet x=1\n  hold r\n\
entry b\n  kernel=/b/k\n  kind=chain\n  hidden\n  order -5\n  default\n\
override efi:0:1:/efi/x/y.efi\n  name Renamed\n  hidden no\n  order 3\n";

    #[test]
    fn splash_keys() {
        let base = "entry a\n  kernel /k\n";
        let c = Config::parse(base).unwrap();
        assert_eq!((c.splash, c.splash_bg, c.splash_ms), (None, None, 0));
        let c = Config::parse(&format!("splash /boot/logo.bmp\nsplash-bg = #0A1B2c\nsplash-ms 1500\n{base}")).unwrap();
        assert_eq!((c.splash.as_deref(), c.splash_bg, c.splash_ms), (Some("/boot/logo.bmp"), Some([0x0a, 0x1b, 0x2c]), 1500));
        assert_eq!(Config::parse(&format!("splash-ms 10000\n{base}")).unwrap().splash_ms, 10_000);
        for bad in ["splash logo.bmp", "splash /a/../b", "splash /a b", "splash", "splash \"/x\"", "splash-bg 102030", "splash-bg #12345",
                    "splash-bg red", "splash-bg", "splash-ms -1", "splash-ms 10001", "splash-ms abc", "splash-ms"] {
            assert!(Config::parse(&format!("{bad}\n{base}")).is_err(), "{bad}");
        }
        // inside an entry block the keys do not exist
        assert!(Config::parse("entry a\n  kernel /k\n  splash /x.bmp\n").is_err());
        // a broken file falls back to no splash at all
        let l = Config::load(Some(b"splash-ms 99999\nentry a\n  kernel /k\n"));
        assert!(l.fallback && l.config.splash.is_none());
        let long = format!("splash /{}\n{base}", "a".repeat(MAX_PATH));
        assert!(Config::parse(&long).is_err());
    }

    #[test]
    fn previous_names_the_entries_that_replace_this_one() {
        let c = Config::parse("entry a\n kernel /k\n previous p1 p2\n previous p3\nentry p1\n kernel /p\n").unwrap();
        assert_eq!(c.entries[0].previous, ["p1", "p2", "p3"]);
        assert!(c.entries[1].previous.is_empty());
        for bad in ["previous", "previous a b c d e", "previous \"x\""] {
            assert!(Config::parse(&format!("entry a\n kernel /k\n {bad}\n")).is_err(), "{bad}");
        }
        assert!(Config::parse("previous p\nentry a\n kernel /k\n").is_err());
    }

    #[test]
    fn more_requires_and_firmware() {
        let t = "entry a\n kernel /k\n default\nentry b\n kernel /k\n more yes\n requires /EFI/x/y.efi\nentry fw\n kind firmware\n title UEFI setup\n more\n";
        let c = Config::parse(t).unwrap();
        assert_eq!(c.entries.iter().map(|e| e.more).collect::<Vec<_>>(), [false, true, true]);
        assert_eq!(c.entries[1].requires.as_deref(), Some("/EFI/x/y.efi"));
        assert_eq!(c.entries[2].kind, Kind::Firmware);
        // a firmware entry needs no kernel, any other entry still does
        assert_eq!(e("entry a\n kind chain").kind, ErrorKind::NoKernel);
        for bad in ["more maybe", "requires rel", "requires /a/../b", "requires /a b", "requires \"/x\"", "requires"] {
            assert!(Config::parse(&format!("entry a\n kernel /k\n {bad}\n")).is_err(), "{bad}");
        }
        // top-level keys do not exist in the other direction
        assert!(Config::parse("more yes\nentry a\n kernel /k\n").is_err());
        // unavailable entries are dropped, the others keep their order
        let mut c = Config::parse(t).unwrap();
        assert_eq!(c.drop_unavailable(|_| false, false), 2);
        assert_eq!(c.entries.len(), 1);
        let mut c = Config::parse(t).unwrap();
        assert_eq!(c.drop_unavailable(|p| p == "/EFI/x/y.efi", true), 0);
        let mut c = Config::parse(t).unwrap();
        assert_eq!(c.drop_unavailable(|_| true, false), 1);
        assert_eq!(c.entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    }

    /// A real file a distribution generated (set `BOOT_CFG_SAMPLE=/path/to/boot.cfg`; skipped without it): it must parse strictly, fit
    /// the limits, and have a default, a first level and a second level.
    #[test]
    fn a_generated_config_parses() {
        let Ok(path) = std::env::var("BOOT_CFG_SAMPLE") else { return };
        let text = std::fs::read_to_string(&path).unwrap();
        let c = Config::parse(&text).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(text.len() <= MAX_FILE && c.entries.len() <= MAX_ENTRIES);
        assert_eq!(c.entries.iter().filter(|e| e.default).count(), 1);
        assert!(c.entries.iter().any(|e| !e.more) && c.entries.iter().any(|e| e.more));
        assert!(c.entries.iter().filter(|e| e.kind == Kind::Linux).all(|e| !e.kernel.is_empty() && !e.initrd.is_empty()));
    }

    #[test]
    fn parses_good_file() {
        let c = Config::parse(GOOD).unwrap();
        assert_eq!((c.timeout_secs, c.tries, c.scan, c.lock_cmdline), (7, 4, true, false));
        assert_eq!(c.default.as_deref(), Some("b"));
        assert_eq!(c.entries.len(), 2);
        let a = &c.entries[0];
        assert_eq!((a.title.as_str(), a.cmdline.as_str(), a.initrd.len()), ("Alpha", "quiet x=1", 2));
        assert_eq!(a.hold, Some(HoldKey::Char('r')));
        assert_eq!(a.order, DEFAULT_ORDER);
        assert_eq!(a.paths_for("previous").0, "/previous/k");
        let b = &c.entries[1];
        assert_eq!((b.kind, b.hidden, b.default, b.order, b.title.as_str()), (Kind::Chain, true, true, -5, "b"));
        assert_eq!(c.overrides[0].name.as_deref(), Some("Renamed"));
        assert_eq!(c.overrides[0].hidden, Some(false));
        assert_eq!(c.overrides[0].order, Some(3));
    }

    fn e(text: &str) -> ParseError {
        Config::parse(text).unwrap_err()
    }

    #[test]
    fn clear_errors_with_lines() {
        assert_eq!(e("bogus 1"), ParseError { line: 1, kind: ErrorKind::UnknownKey("bogus".into()) });
        assert_eq!(e("entry a\nkernel /k\nwhat 1").line, 3);
        assert_eq!(e("timeout").kind, ErrorKind::BadValue("number out of range"));
        assert_eq!(e("timeout 601").line, 1);
        assert_eq!(e("entry").kind, ErrorKind::MissingValue("entry".into()));
        assert_eq!(e("entry a\nkernel ../x").kind, ErrorKind::BadValue("path: absolute, no spaces, no .."));
        assert_eq!(e("entry a\nkernel rel").line, 2);
        assert_eq!(e("entry a\nkernel /k\nkind x").line, 3);
        assert_eq!(e("entry a\nkernel /k\ncmdline a\"b").line, 3);
        assert_eq!(e("entry a\nkernel /k\nhold ab").line, 3);
        assert_eq!(e("entry a\nkernel /k\nentry a").kind, ErrorKind::Duplicate);
        assert_eq!(e("entry a").kind, ErrorKind::NoKernel);
        assert_eq!(e("timeout 1").kind, ErrorKind::NoEntries);
        assert_eq!(e("entry a\nkernel /k\ndefault\nentry b\nkernel /k\ndefault").kind, ErrorKind::TwoDefaults);
        assert_eq!(e("entry a\nkernel /k\nhidden maybe").line, 3);
        // entry keys do not work at top level, and top keys do not work in a block
        assert_eq!(e("kernel /k").line, 1);
        assert_eq!(e("entry a\nkernel /k\ntimeout 3").line, 3);
        let msg = alloc::format!("{}", e("entry a\nkernel rel"));
        assert!(msg.starts_with("line 2:"), "{msg}");
    }

    #[test]
    fn limits() {
        let big = "#".repeat(MAX_FILE + 1);
        assert_eq!(e(&big).kind, ErrorKind::TooLarge);
        let long = alloc::format!("entry a\nkernel /{}", "x".repeat(MAX_LINE));
        assert_eq!(e(&long).kind, ErrorKind::LineTooLong);
        let mut many = String::new();
        for i in 0..=MAX_ENTRIES {
            many.push_str(&alloc::format!("entry e{i}\nkernel /k\n"));
        }
        assert_eq!(e(&many).kind, ErrorKind::TooMany);
        let mut ov = String::from("entry a\nkernel /k\n");
        for i in 0..=MAX_OVERRIDES {
            ov.push_str(&alloc::format!("override o{i}\n"));
        }
        assert_eq!(e(&ov).kind, ErrorKind::TooMany);
        let ini = "entry a\nkernel /k\ninitrd /1\ninitrd /2\ninitrd /3\ninitrd /4\ninitrd /5";
        assert_eq!(e(ini).kind, ErrorKind::TooMany);
        assert_eq!(e("entry a\nkernel /k\ntitle \u{7}x").kind, ErrorKind::ControlChar);
    }

    #[test]
    fn fallback_when_missing_or_broken() {
        let l = Config::load(None);
        assert!(l.fallback && l.error.is_none());
        assert_eq!(l.config.entries[0].id, "current");
        assert_eq!(l.config.entries[1].id, "seed");
        assert_eq!(l.config.timeout_secs, 0);
        let l = Config::load(Some(b"entry\n"));
        assert!(l.fallback && l.error.is_some());
        let l = Config::load(Some(&[0xff, 0xfe]));
        assert_eq!(l.error.unwrap().kind, ErrorKind::NotUtf8);
        let l = Config::load(Some(GOOD.as_bytes()));
        assert!(!l.fallback && l.error.is_none());
        let l = Config::load(Some(&vec![b'#'; MAX_FILE + 1]));
        assert_eq!(l.error.unwrap().kind, ErrorKind::TooLarge);
    }

    #[test]
    fn fallback_is_valid_and_matches_parser_rules() {
        let c = Config::fallback();
        assert!(c.entries.iter().all(|e| valid_path(&e.kernel) && e.initrd.iter().all(|p| valid_path(p))));
        assert_eq!(c.entries.iter().filter(|e| e.default).count(), 1);
    }

    #[test]
    fn crlf_and_tabs() {
        let c = Config::parse("entry a\r\n\tkernel\t/k\r\n").unwrap();
        assert_eq!(c.entries[0].kernel, "/k");
    }
}
