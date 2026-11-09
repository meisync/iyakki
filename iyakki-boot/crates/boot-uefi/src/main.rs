// SPDX-License-Identifier: MIT OR Apache-2.0
//! UEFI front end of the boot loader. Reads the configuration from the volume it was loaded from, decides what to start with
//! `boot-core`, shows the menu when needed, and starts the Linux EFI stub (or another loader) with `LoadImage` / `StartImage`.
//! Names, labels and entries come from the configuration file; nothing about a particular distribution is built in.
#![no_std]
#![no_main]

extern crate alloc;

mod font;
mod fs;
mod gfx;
mod hwlog;
mod launch;
mod scan;
mod shim;
mod splash;
mod sys;
mod ui;
mod vars;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use boot_core::assess::{BootState, Outcome, Reason, STATE_LEN};
use boot_core::choose::{self, Decision, KeyState, Params, Why};
use boot_core::cmdline::{self, Rules};
use boot_core::config::{Config, Kind, MAX_FILE};
use boot_core::discover;
use boot_core::item::{Action, Item};
use core::fmt::Write;
use uefi_sys::*;

/// Where the configuration lives on the boot volume. Set `BOOT_CONFIG_PATH` when building to use another name.
const CONFIG_PATH: &str = match option_env!("BOOT_CONFIG_PATH") {
    Some(p) => p,
    None => "/boot.cfg",
};
/// How long the keyboard is read at start to see which key is held, in milliseconds.
const KEY_WINDOW_MS: usize = 200;
/// Header of the menu.
const MENU_HEADER: &str = "Choose what to start";
/// Largest initrd accepted, in bytes.
const MAX_INITRD: usize = 1 << 31;
/// The variable that forces the text menu even when a graphics output exists (set it from the OS when debugging a screen).
const TEXT_ONLY_VAR: &str = "BootText";
/// Where the log of this run is written on the boot volume: beside the configuration file, so a stick plugged into another
/// computer shows what the loader found, chose and started. Written when the entries are known, right before a start, and after
/// a start came back. A volume that cannot be written (a CD, a firmware without FAT writing) is simply left without it.
const LOG_PATH: &str = match option_env!("BOOT_LOG_PATH") {
    Some(p) => p,
    None => "/EFI/iyakki-boot/last-boot.log",
};

/// The markers the system update leaves with the copy of the previous version on the boot volume (`EFI/iyakki-boot/prev/`). Either one
/// holds `stamp N`, which changes with every update (boot_core::assess::stamp_of): a changed stamp means a new set was installed.
const PREV_MARKERS: [&str; 2] = ["/EFI/iyakki-boot/prev/set-plain", "/EFI/iyakki-boot/prev/set-oldroot"];

/// Where the log goes when the directory of `LOG_PATH` does not exist on this volume (the root always does).
const LOG_FALLBACK_PATH: &str = "/last-boot.log";

/// Write the log so far to `LOG_PATH` (or `LOG_FALLBACK_PATH`). Says so first, so the file's last line names the moment it was
/// written, and once more when no file could be written at all.
fn save_log(vol: &fs::Volume, when: &str) {
    say!("log: {when}; this log is in {LOG_PATH} on the boot volume\n");
    if vol.write(LOG_PATH, &sys::log_bytes()) {
        return;
    }
    say!("log: {LOG_PATH} could not be written; trying {LOG_FALLBACK_PATH}\n");
    if !vol.write(LOG_FALLBACK_PATH, &sys::log_bytes()) {
        say!("log: no log file could be written (read-only medium or firmware)\n");
    }
}

#[global_allocator]
static ALLOCATOR: sys::Pool = sys::Pool;

/// Use the text menu even when the graphics output exists.
pub fn force_text() -> bool {
    vars::get(TEXT_ONLY_VAR).is_some()
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    splash::release();
    say!("\nboot loader panic: {info}\nThe machine restarts after 30 seconds, or press a key.\n");
    ui::wait_key(30_000);
    sys::reset()
}

#[no_mangle]
pub extern "efiapi" fn efi_main(image: Handle, st: *mut SystemTable) -> Status {
    sys::init(image, st);
    run()
}

/// Stop with a visible message (nothing can be started).
fn halt(msg: &str) -> ! {
    splash::release();
    say!("boot: {msg}\nPress a key to restart.\n");
    loop {
        if ui::wait_key(60_000) {
            sys::reset();
        }
    }
}

fn poll_keys(ms: usize) -> KeyState {
    let mut keys = KeyState::default();
    let mut left = ms;
    loop {
        while let Some(k) = ui::read_key() {
            if k.scan_code == SCAN_ESC {
                keys.esc = true;
            }
            match char::from_u32(k.unicode_char as u32) {
                Some(' ') => keys.space = true,
                Some(c) if c.is_ascii_alphanumeric() => keys.press_char(c.to_ascii_lowercase()),
                // Enter, Tab, Backspace
                Some(c) if (c as u32) != 0 => keys.esc = true,
                // arrows, Home/End, Page keys, function keys: no character, only a scan code. Any of them asks for the menu.
                _ if k.scan_code != 0 => keys.esc = true,
                _ => {}
            }
        }
        if left == 0 {
            return keys;
        }
        sys::stall_us(10_000);
        left = left.saturating_sub(10);
    }
}

/// Path of this image on its volume, from the loaded-image device path.
fn own_path(li: *mut LoadedImage) -> String {
    let mut out = String::new();
    let mut p = unsafe { (*li).file_path } as *const DevicePathProtocol;
    if p.is_null() {
        return out;
    }
    unsafe {
        while !(*p).is_end_entire() {
            let n = (*p).node_length() as usize;
            if n < 4 {
                break;
            }
            if (*p).ty == DP_TYPE_MEDIA && (*p).sub_type == DP_SUBTYPE_MEDIA_FILE_PATH {
                let w = core::slice::from_raw_parts((p as *const u8).add(4) as *const u16, (n - 4) / 2);
                out.push_str(&sys::from_ucs2(w));
            }
            p = (p as *const u8).add(n) as *const DevicePathProtocol;
        }
    }
    out.replace('\\', "/")
}

struct Ctx {
    cfg: Config,
    items: Vec<Item>,
    found: Vec<scan::Found>,
    vol: fs::Volume,
    secure: bool,
}

/// The menu: returns the chosen item and whether the countdown chose it.
fn menu(c: &Ctx, selected: usize, timeout: Option<u32>, waiting: bool, notice: Option<&str>) -> (usize, bool) {
    let p = ui::choose(&c.items, selected, timeout, MENU_HEADER, notice, waiting);
    let item = &c.items[p.item];
    say!("menu: chosen {}{}\n", item.id, if p.timed_out { " (countdown)" } else { "" });
    (p.item, p.timed_out && !waiting)
}

fn read_initrds(c: &Ctx, paths: &[String]) -> Result<Option<Vec<u8>>, String> {
    if paths.is_empty() {
        return Ok(None);
    }
    let mut all: Vec<u8> = Vec::new();
    for p in paths {
        let d = c.vol.read(p, MAX_INITRD).ok_or_else(|| format!("initrd {p} cannot be read"))?;
        all.extend_from_slice(&d);
        while all.len() % 4 != 0 {
            all.push(0); // concatenated cpio archives start on 4-byte boundaries
        }
    }
    Ok(Some(all))
}

/// Start one item. `Ok` means the started image came back; `Err` that it could not be prepared or loaded.
fn start_item(c: &Ctx, item: &Item, tree: &str) -> Result<Status, String> {
    match &item.action {
        Action::Linux(i) => {
            let e = &c.cfg.entries[*i];
            let (kernel, initrds) = e.paths_for(tree);
            match e.kind {
                Kind::Linux => {
                    let rules = Rules { secure_boot: c.secure, ..Rules::default() };
                    let mut line = cmdline::apply(&e.cmdline, &[], &rules).map_err(|_| String::from("the command line is not valid"))?;
                    // Macs have no serial port, and on the 2016-2017 MacBook Pros ttyS0 is the Bluetooth chip's link: no serial console there
                    if hwlog::vendor().starts_with("Apple") {
                        let (l, n) = cmdline::without_serial_console(&line);
                        if n > 0 {
                            say!("cmdline: {n} serial console parameter(s) dropped on Apple firmware (ttyS0 is the Bluetooth link on these Macs)\n");
                            line = l;
                        }
                    }
                    say!("start: {} kernel={} tree={} initrds={} cmdline={}\n", item.id, kernel, tree, initrds.len(), line);
                    let initrd = read_initrds(c, &initrds)?;
                    save_log(&c.vol, "about to start"); // the file now ends with the start line above: a kernel that never draws is still named
                    launch::start(c.vol.handle, &kernel, Some(&line), initrd)
                }
                Kind::Chain => {
                    say!("start: {} chain={}\n", item.id, kernel);
                    save_log(&c.vol, "about to start"); // the file now ends with the start line above: a kernel that never draws is still named
                    launch::start(c.vol.handle, &kernel, None, None)
                }
                Kind::Firmware => Err(String::from("a firmware entry is not started like a kernel")),
            }
        }
        Action::Firmware => {
            say!("start: {} firmware setup\n", item.id);
            save_log(&c.vol, "about to start");
            if !vars::request_firmware_setup() {
                return Err(String::from("the firmware setup request could not be stored"));
            }
            sys::reset()
        }
        // A file on the loader's own volume named by the config (`kind chain`: "Previous version", "Recovery (GRUB menu)"): started from that
        // volume whether or not the scanner is on (the scanner only lists what it finds; these rows are written by the image build).
        Action::Chain(discover::Target::BootVolume { path }) if !c.found.iter().any(|f| f.disc.target == discover::Target::BootVolume { path: path.clone() }) => {
            say!("start: {} chain={}\n", item.id, path);
            save_log(&c.vol, "about to start"); // the file now ends with the start line above: a kernel that never draws is still named
            launch::start(c.vol.handle, path, None, None)
        }
        Action::Chain(t) => {
            let f = c.found.iter().find(|f| f.disc.target == *t).ok_or_else(|| String::from("this kind of system cannot be started by the UEFI loader"))?;
            say!("start: {} chain={}\n", item.id, f.path);
            save_log(&c.vol, "about to start"); // the file now ends with the start line above: a kernel that never draws is still named
            launch::start(f.volume, &f.path, None, None)
        }
    }
}

fn run() -> Status {
    let secure = sys::secure_boot();
    say!("boot: loader started, secure boot {}\n", if secure { "on" } else { "off" });
    let Some(li) = sys::protocol::<LoadedImage>(sys::image(), &LOADED_IMAGE_PROTOCOL_GUID) else { halt("no loaded-image protocol") };
    let dev = unsafe { (*li).device_handle };
    let own = own_path(li);
    let Some(vol) = fs::Volume::open(dev) else { halt("the boot volume cannot be opened") };

    // configuration: a missing or broken file never stops the start, the built-in entries are used
    let bytes = vol.read(CONFIG_PATH, MAX_FILE + 1);
    let loaded = Config::load(bytes.as_deref());
    match (&loaded.error, loaded.fallback) {
        (Some(e), _) => say!("config: {CONFIG_PATH} is broken ({e}); built-in entries used\n"),
        (None, true) => say!("config: {CONFIG_PATH} not found; built-in entries used\n"),
        (None, false) => say!("config: {CONFIG_PATH} read\n"),
    }
    let mut cfg = loaded.config;
    // entries that cannot work here are left out: a `requires` file that is not on the volume, firmware setup the firmware cannot do
    let dropped = cfg.drop_unavailable(|p| vol.exists(p), vars::firmware_setup_supported());
    if dropped > 0 {
        say!("config: {dropped} entries left out (their file or firmware feature is missing)\n");
    }
    // the logo first: the person sees the loader is alive before anything else is tried
    splash::show(&cfg, &vol);
    // the first write of the log, then what this machine is and what its firmware exposes, the log written after every section: a
    // start that stops in one of those walks, the scan or the menu still leaves the machine's description (as far as it got) on the
    // volume, and its last line names where it stopped
    save_log(&vol, "config read; next: the hardware listing");
    hwlog::summary(&|where_| save_log(&vol, where_));
    save_log(&vol, "devices listed");

    // other systems
    let found = if cfg.scan { scan::scan(dev, &own) } else { Vec::new() };
    let items = discover::all_items(&cfg, found.iter().map(|f| f.disc.clone()).collect(), secure);
    for (n, it) in items.iter().enumerate() {
        say!("item: {n} {} \"{}\"{}\n", it.id, it.title, if it.enabled() { "" } else { " (disabled)" });
    }
    let c = Ctx { cfg, items, found, vol, secure };
    save_log(&c.vol, "entries known");

    // boot assessment
    let stored = vars::get(vars::TRIES);
    let mut state = BootState::load(stored.as_deref().filter(|b| b.len() == STATE_LEN), c.cfg.tries);
    // a system update installed a new set since the state was last judged: trust the new set again (a rollback to the previous version
    // from an earlier set does not outlive the update that replaced that set)
    let stamp = PREV_MARKERS.iter().find_map(|p| c.vol.read(p, 512)).and_then(|b| String::from_utf8(b).ok()).map_or(0, |t| boot_core::assess::stamp_of(&t));
    if state.sync_stamp(stamp, c.cfg.tries) {
        say!("state: a new set was installed (stamp {stamp}); the tries start over on the current version\n");
    }
    // the running system confirmed the last start (it writes the confirm variable once the sign-in screen is drawn): refill the tries
    let confirmed = vars::get(vars::CONFIRM).is_some();
    if confirmed {
        state.mark_good(c.cfg.tries);
        vars::delete(vars::CONFIRM);
        say!("state: the last start was confirmed good\n");
    }
    let mut trial = state;
    let outcome = trial.on_start(c.cfg.tries);
    let wait = match outcome {
        Outcome::StopAndWait(r) => Some(r),
        Outcome::Start(_) => None,
    };
    let notice: Option<&str> = match (wait, state.reason) {
        (Some(r), _) => Some(r.text()),
        (None, Reason::None) => None,
        (None, r) => Some(r.text()),
    };
    if let Some(n) = notice {
        say!("notice: {n}\n");
    }
    if confirmed {
        state.take_reason(); // told once (above); stored cleared by the next write
        trial.reason = state.reason;
    }

    // one-shot and held keys
    let once = vars::get(vars::ONESHOT).and_then(|b| String::from_utf8(b).ok());
    let oneshot = choose::resolve_oneshot(once.as_deref(), &c.items);
    if oneshot.clear {
        vars::delete(vars::ONESHOT); // always cleared before anything starts
    }
    // listen for the whole time the logo is up (at least KEY_WINDOW_MS): keys pressed a moment after power-on, or by a slow keyboard
    // (a Mac's), still open the menu. The time counts toward the splash time, so a start is not slower.
    let window = KEY_WINDOW_MS.max(c.cfg.splash_ms as usize);
    // keys left in the firmware's buffer by its own menus (the Enter of a Mac's startup picker) are not a request for ours
    let stale = ui::drain_keys();
    if stale > 0 {
        say!("input: {stale} key(s) were already in the firmware's buffer when the loader started: ignored\n");
    }
    let keys = poll_keys(window);
    splash::spent(window as u32);
    let params = Params {
        items: &c.items,
        default_name: c.cfg.default.as_deref(),
        timeout_secs: c.cfg.timeout_secs,
        oneshot: &oneshot,
        keys,
        force_menu: false,
        wait: wait.is_some(),
    };
    let decision = choose::decide(&params);
    let (mut idx, mut auto) = match decision {
        Decision::Boot { index, why } => {
            say!("decide: start {} ({})\n", c.items[index].id, match why { Why::HoldKey => "hold key", Why::OneShot => "one-shot", Why::Default => "default" });
            (index, why == Why::Default)
        }
        Decision::Menu { selected, timeout_secs } => {
            say!("decide: menu{}\n", if wait.is_some() { " (waiting)" } else { "" });
            menu(&c, selected, timeout_secs, wait.is_some(), notice)
        }
        Decision::Nothing => halt("nothing can be started"),
    };

    loop {
        let mut item = &c.items[idx];
        let is_linux = matches!(&item.action, Action::Linux(i) if c.cfg.entries[*i].kind == Kind::Linux);
        let tree = if auto && is_linux && wait.is_none() {
            state = trial; // the start the loader chose itself spends a try (and may have switched to the previous tree)
            // the previous version is due and this volume keeps one: its row starts instead of the default (the menu stays as it is)
            if let Some(p) = choose::previous_item(&c.cfg.entries, &c.items, idx, trial.tree) {
                say!("state: the current version was not confirmed good; {} starts instead\n", c.items[p].id);
                item = &c.items[p];
            }
            trial.tree
        } else {
            state.manual_start();
            state.tree
        };
        say!("state: tree={} tries_left={} starts={}\n", tree.name(), state.tries_left, state.starts);
        if !vars::set(vars::TRIES, &state.encode()) {
            say!("state: cannot store the boot counter\n");
        }
        splash::wait_minimum(); // no-op after a menu; the picture stays on screen, the screen is not cleared before the kernel
        let result = start_item(&c, item, tree.name());
        let mut msg = String::new();
        match result {
            Ok(s) => write!(msg, "{} returned: {}", item.title, launch::status_text(s)).ok(),
            Err(e) => write!(msg, "{} was not started: {e}", item.title).ok(),
        };
        say!("boot: {msg}\n");
        save_log(&c.vol, "the start came back");
        // it came back: never loop, wait for the person
        (idx, _) = menu(&c, idx, None, true, Some(&msg));
        auto = false;
    }
}
