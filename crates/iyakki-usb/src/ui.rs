//! What the user sees, as plain strings: the drive menu, the erase warning, the progress bar.
//! Kept free of terminal calls so it can be tested.

use std::time::Duration;

use crate::drives::{Drive, Verdict};
use crate::writer::Phase;

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const RED: &str = "\x1b[31m";
pub const GREEN: &str = "\x1b[32m";
pub const YELLOW: &str = "\x1b[33m";
pub const REVERSE: &str = "\x1b[7m";
pub const CLEAR: &str = "\x1b[2J\x1b[H";

pub fn human_size(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let (mut v, mut i) = (b as f64, 0);
    while v >= 1000.0 && i < U.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", U[i]) }
}

pub fn human_duration(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Enter,
    Quit,
    Other,
}

/// Terminal bytes to keys: arrows are `ESC [ A/B`, Enter is CR/LF, `q`, Esc and Ctrl+C quit; `j`/`k` also move.
pub fn parse_keys(b: &[u8]) -> Vec<Key> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            0x1b if b.get(i + 1) == Some(&b'[') || b.get(i + 1) == Some(&b'O') => {
                out.push(match b.get(i + 2) {
                    Some(b'A') => Key::Up,
                    Some(b'B') => Key::Down,
                    _ => Key::Other,
                });
                i += 3;
                continue;
            }
            0x1b | 0x03 | b'q' => out.push(Key::Quit),
            b'\r' | b'\n' => out.push(Key::Enter),
            b'k' => out.push(Key::Up),
            b'j' => out.push(Key::Down),
            _ => out.push(Key::Other),
        }
        i += 1;
    }
    out
}

#[derive(Clone, Debug)]
pub struct Item {
    pub drive: Drive,
    pub verdict: Verdict,
}

#[derive(Debug, PartialEq, Eq)]
pub enum MenuOutcome {
    Continue,
    Chosen(usize),
    /// Enter on a drive that may not be written; carries the reason to show.
    Blocked(String),
    Quit,
}

pub struct Menu {
    pub items: Vec<Item>,
    pub sel: usize,
}

impl Menu {
    pub fn new(items: Vec<Item>) -> Menu {
        let sel = items.iter().position(|i| i.verdict == Verdict::Ok).unwrap_or(0);
        Menu { items, sel }
    }

    pub fn handle(&mut self, k: Key) -> MenuOutcome {
        match k {
            Key::Up => self.sel = self.sel.saturating_sub(1),
            Key::Down => self.sel = (self.sel + 1).min(self.items.len().saturating_sub(1)),
            Key::Quit => return MenuOutcome::Quit,
            Key::Enter => {
                return match self.items.get(self.sel).map(|i| &i.verdict) {
                    None => MenuOutcome::Continue,
                    Some(Verdict::Refused(r)) => MenuOutcome::Blocked(r.clone()),
                    Some(_) => MenuOutcome::Chosen(self.sel),
                };
            }
            Key::Other => {}
        }
        MenuOutcome::Continue
    }
}

fn describe(i: &Item) -> String {
    let d = &i.drive;
    let kind = if d.usb { "USB" } else if d.removable { "removable" } else { "internal" };
    format!("{:<10} {:>9}  {:<26} {}", d.path, human_size(d.size_bytes), d.model.chars().take(26).collect::<String>(), kind)
}

pub fn render_menu(m: &Menu, image_name: &str, image_size: u64, note: Option<&str>) -> String {
    let mut s = String::from(CLEAR);
    s.push_str(&format!("{BOLD}IYAKKI USB writer{RESET}\n"));
    s.push_str(&format!("Image: {image_name} ({})\n\n", human_size(image_size)));
    if m.items.is_empty() {
        s.push_str("No USB drives found.\nPlug in the stick, wait a few seconds, and run this again. (--all also lists internal disks.)\n");
    } else {
        s.push_str("Choose the drive to write to (↑/↓ to move, Enter to select, q to quit):\n\n");
    }
    for (n, i) in m.items.iter().enumerate() {
        let (color, tail) = match &i.verdict {
            Verdict::Ok => (GREEN, String::new()),
            Verdict::Internal => (YELLOW, "  [internal disk: be very sure]".to_string()),
            Verdict::Refused(r) => (DIM, format!("  [not allowed: {r}]")),
        };
        let line = format!("{}{}{}", describe(i), tail, "");
        if n == m.sel {
            s.push_str(&format!("{REVERSE}{color} > {line} {RESET}\n"));
        } else {
            s.push_str(&format!("{color}   {line}{RESET}\n"));
        }
    }
    if let Some(n) = note {
        s.push_str(&format!("\n{YELLOW}{n}{RESET}\n"));
    }
    s
}

/// The two ways to put IYAKKI on a stick, as a menu.
pub fn render_mode_menu(sel: usize) -> String {
    let rows = [
        ("Install IYAKKI on the stick (recommended)", "A real system with the partitions a PC expects: a boot partition and a data partition that keeps your accounts, settings and files. Boots on legacy BIOS and UEFI PCs."),
        ("Copy the live ISO", "Runs from memory; nothing is saved at power-off unless you add a data disk later. Same as the image used for CDs."),
    ];
    let mut s = String::from(CLEAR);
    s.push_str(&format!("{BOLD}IYAKKI USB writer{RESET}\n\nHow do you want to put IYAKKI on the stick? (↑/↓, Enter, q to quit)\n\n"));
    for (i, (t, d)) in rows.iter().enumerate() {
        if i == sel {
            s.push_str(&format!("{REVERSE}{GREEN} > {t} {RESET}\n"));
        } else {
            s.push_str(&format!("{GREEN}   {t}{RESET}\n"));
        }
        s.push_str(&format!("   {DIM}{d}{RESET}\n\n"));
    }
    s
}

/// Up/Down/Enter/q for the mode menu: `Some(Some(i))` chosen, `Some(None)` quit, `None` keep going.
pub fn mode_menu_key(sel: &mut usize, k: Key) -> Option<Option<usize>> {
    match k {
        Key::Up => *sel = sel.saturating_sub(1),
        Key::Down => *sel = (*sel + 1).min(1),
        Key::Enter => return Some(Some(*sel)),
        Key::Quit => return Some(None),
        Key::Other => {}
    }
    None
}

/// The partitions an install creates, for the warning screen.
pub fn install_layout_lines(device_bytes: u64, boot_bytes: u64) -> Vec<String> {
    let data = device_bytes.saturating_sub(boot_bytes + 2 * 1024 * 1024);
    vec![
        format!("1  IYAKKI-BOOT   {:>9}  FAT32, bootable: boot loader, kernel and settings as ordinary files", human_size(boot_bytes)),
        format!("2  IYAKKI data   {:>9}  formatted on first boot; keeps accounts, settings, documents and chats", human_size(data)),
        "Boots on legacy BIOS PCs (boot code in the first sectors) and on UEFI PCs.".to_string(),
    ]
}

pub fn render_warning(d: &Drive, v: &Verdict, image_name: &str, image_size: u64, layout: Option<&[String]>) -> String {
    let mut s = String::from(CLEAR);
    s.push_str(&format!("{BOLD}{RED}  WARNING: THIS WILL ERASE THE WHOLE DRIVE  {RESET}\n\n"));
    s.push_str(&format!("  Drive:  {BOLD}{}{RESET}  {}  ({})\n", d.path, d.model, human_size(d.size_bytes)));
    s.push_str(&format!("  Image:  {image_name} ({})\n\n", human_size(image_size)));
    if let Some(lines) = layout {
        s.push_str("  The drive will be partitioned like this:\n");
        for l in lines {
            s.push_str(&format!("    {l}\n"));
        }
        s.push('\n');
    }
    if !d.mounts.is_empty() {
        s.push_str("  Currently in use (will be unmounted):\n");
        for (dev, mp) in &d.mounts {
            s.push_str(&format!("    {dev}  →  {mp}\n"));
        }
        s.push('\n');
    }
    s.push_str(&format!("  {RED}Everything on this drive will be lost and cannot be recovered.{RESET}\n"));
    if *v == Verdict::Internal {
        s.push_str(&format!("  {BOLD}{YELLOW}This is an INTERNAL disk, not a USB stick. Check that it is the one you mean.{RESET}\n"));
    }
    s.push_str(&format!("\n  To continue, type the drive name ({BOLD}{}{RESET}) and press Enter.\n  Press Enter alone to cancel: ", d.name));
    s
}

/// The typed confirmation: the short name or the full path, nothing else.
pub fn confirmed(input: &str, d: &Drive) -> bool {
    let t = input.trim();
    !t.is_empty() && (t == d.name || t == d.path)
}

pub fn bar(width: usize, frac: f64) -> String {
    let filled = ((frac.clamp(0.0, 1.0) * width as f64).round() as usize).min(width);
    format!("[{}{}]", "█".repeat(filled), "░".repeat(width - filled))
}

/// One progress line (without the newline; the caller prefixes `\r`).
pub fn render_progress(phase: Phase, done: u64, total: u64, elapsed: Duration, width: usize) -> String {
    let frac = if total == 0 { 1.0 } else { done as f64 / total as f64 };
    let rate = if elapsed.as_secs_f64() > 0.05 { done as f64 / elapsed.as_secs_f64() } else { 0.0 };
    let eta = if rate > 0.0 && total > done { Duration::from_secs_f64((total - done) as f64 / rate) } else { Duration::ZERO };
    let label = format!("{:<15}", phase.label());
    let stats = format!(" {:>3.0}%  {:>9}/s  ETA {}", frac * 100.0, human_size(rate as u64), human_duration(eta));
    let room = width.saturating_sub(label.chars().count() + stats.chars().count() + 3).clamp(10, 60);
    format!("{label} {} {stats}", bar(room, frac))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(name: &str, size: u64) -> Drive {
        Drive { path: format!("/dev/{name}"), name: name.into(), model: "SanDisk Cruzer Blade".into(), size_bytes: size, removable: true, usb: true, read_only: false, mounts: vec![], swap: false }
    }
    fn item(name: &str, v: Verdict) -> Item {
        Item { drive: drive(name, 16_000_000_000), verdict: v }
    }

    #[test]
    fn keys_are_parsed_from_terminal_bytes() {
        assert_eq!(parse_keys(b"\x1b[A\x1b[B\r"), vec![Key::Up, Key::Down, Key::Enter]);
        assert_eq!(parse_keys(b"jk\nq"), vec![Key::Down, Key::Up, Key::Enter, Key::Quit]);
        assert_eq!(parse_keys(b"\x1b"), vec![Key::Quit], "a lone Esc quits");
        assert_eq!(parse_keys(b"\x03"), vec![Key::Quit], "Ctrl+C");
        assert_eq!(parse_keys(b"\x1b[C\x1bOA"), vec![Key::Other, Key::Up], "right arrow ignored, application-mode up arrow works");
        assert_eq!(parse_keys(b"x"), vec![Key::Other]);
        assert!(parse_keys(b"").is_empty());
    }

    #[test]
    fn the_menu_starts_on_the_first_usable_drive_and_blocks_refused_ones() {
        let mut m = Menu::new(vec![item("sda", Verdict::Refused("holds the running system".into())), item("sdb", Verdict::Ok), item("sdc", Verdict::Internal)]);
        assert_eq!(m.sel, 1, "the first allowed drive is preselected");
        assert_eq!(m.handle(Key::Down), MenuOutcome::Continue);
        assert_eq!(m.handle(Key::Down), MenuOutcome::Continue);
        assert_eq!(m.sel, 2, "stops at the last item");
        assert_eq!(m.handle(Key::Enter), MenuOutcome::Chosen(2));
        m.handle(Key::Up);
        m.handle(Key::Up);
        m.handle(Key::Up);
        assert_eq!(m.sel, 0);
        assert_eq!(m.handle(Key::Enter), MenuOutcome::Blocked("holds the running system".into()));
        assert_eq!(m.handle(Key::Quit), MenuOutcome::Quit);
        assert_eq!(m.handle(Key::Other), MenuOutcome::Continue);
        let mut empty = Menu::new(vec![]);
        assert_eq!((empty.sel, empty.handle(Key::Enter), empty.handle(Key::Down)), (0, MenuOutcome::Continue, MenuOutcome::Continue));
    }

    #[test]
    fn the_menu_shows_every_drive_with_its_status() {
        let m = Menu::new(vec![item("sda", Verdict::Refused("holds the running system".into())), item("sdb", Verdict::Ok), item("sdc", Verdict::Internal)]);
        let s = render_menu(&m, "iyakki.iso", 61_000_000, Some("pick one"));
        for want in ["IYAKKI USB writer", "iyakki.iso (61.0 MB)", "/dev/sdb", "16.0 GB", "SanDisk Cruzer Blade", "not allowed: holds the running system", "internal disk: be very sure", "pick one", "↑/↓"] {
            assert!(s.contains(want), "missing `{want}` in:\n{s}");
        }
        assert!(s.contains("> /dev/sdb"), "the selection marker is on the selected row");
        assert!(!s.contains("> /dev/sda"));
        let none = render_menu(&Menu::new(vec![]), "x.iso", 1, None);
        assert!(none.contains("No USB drives found") && none.contains("--all"));
    }

    #[test]
    fn the_warning_is_loud_and_names_exactly_what_will_be_erased() {
        let mut d = drive("sdb", 16_000_000_000);
        d.mounts = vec![("/dev/sdb1".into(), "/media/amy/KEY".into())];
        let s = render_warning(&d, &Verdict::Ok, "iyakki.iso", 61_000_000, None);
        for want in ["WARNING: THIS WILL ERASE THE WHOLE DRIVE", "/dev/sdb", "SanDisk Cruzer Blade", "16.0 GB", "/dev/sdb1  →  /media/amy/KEY", "cannot be recovered", "type the drive name", "sdb", "cancel"] {
            assert!(s.contains(want), "missing `{want}`:\n{s}");
        }
        assert!(!s.contains("INTERNAL"));
        assert!(render_warning(&d, &Verdict::Internal, "iyakki.iso", 1, None).contains("INTERNAL disk"));
        let lay = install_layout_lines(64_000_000_000, 268_435_456);
        let w = render_warning(&d, &Verdict::Ok, "iyakki-install.img", 270_000_000, Some(&lay));
        for want in ["partitioned like this", "IYAKKI-BOOT", "FAT32, bootable", "IYAKKI data", "63.7 GB", "legacy BIOS"] {
            assert!(w.contains(want), "missing `{want}`:\n{w}");
        }
        assert!(!s.contains("partitioned like this"));
        // the mode menu
        let (mut sel, m) = (0usize, render_mode_menu(0));
        assert!(m.contains("Install IYAKKI on the stick") && m.contains("Copy the live ISO") && m.contains("> Install"));
        assert_eq!(mode_menu_key(&mut sel, Key::Down), None);
        assert!(render_mode_menu(sel).contains("> Copy the live ISO"));
        assert_eq!((mode_menu_key(&mut sel, Key::Down), sel), (None, 1), "stops at the last row");
        assert_eq!(mode_menu_key(&mut sel, Key::Enter), Some(Some(1)));
        assert_eq!(mode_menu_key(&mut sel, Key::Quit), Some(None));
        assert_eq!((mode_menu_key(&mut sel, Key::Up), sel), (None, 0));
        assert!(s.contains(RED), "red text");
    }

    #[test]
    fn confirmation_needs_the_exact_name_or_path() {
        let d = drive("sdb", 1);
        assert!(confirmed("sdb", &d) && confirmed("  sdb\n", &d) && confirmed("/dev/sdb", &d));
        for bad in ["", "   ", "sdc", "SDB", "yes", "sd", "sdb1", "y"] {
            assert!(!confirmed(bad, &d), "{bad:?}");
        }
    }

    #[test]
    fn progress_lines_show_percent_speed_and_time_left() {
        let line = render_progress(Phase::Writing, 30_000_000, 60_000_000, Duration::from_secs(3), 100);
        for want in ["Writing", "50%", "10.0 MB/s", "ETA 00:03", "█", "░"] {
            assert!(line.contains(want), "missing `{want}` in {line}");
        }
        let done = render_progress(Phase::Verifying, 60, 60, Duration::from_secs(1), 80);
        assert!(done.contains("100%") && done.contains("ETA 00:00") && !done.contains('░'));
        assert!(render_progress(Phase::Syncing, 0, 0, Duration::ZERO, 80).contains("100%"), "an empty total does not divide by zero");
        assert_eq!(bar(10, 0.5), "[█████░░░░░]");
        assert_eq!(bar(4, 2.0), "[████]");
        assert_eq!(bar(4, -1.0), "[░░░░]");
        assert!(render_progress(Phase::Writing, 1, 2, Duration::from_secs(1), 20).chars().count() < 90, "narrow terminals still work");
        assert_eq!((human_size(999), human_size(1_500_000), human_size(61_000_000_000)), ("999 B".to_string(), "1.5 MB".to_string(), "61.0 GB".to_string()));
        assert_eq!(human_duration(Duration::from_secs(125)), "02:05");
    }
}
