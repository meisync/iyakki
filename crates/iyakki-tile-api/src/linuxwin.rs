//! The plain data and the port of the LINUX WINDOW tile (D-S1-E4-01, pattern b): the sandbox's programs and usage as plain mirrors of the types
//! of `iyakki_shared::linuxwin`, the remote screen and the display process as two small traits (the shell's bridge is over `iyakki_rfb::Link`
//! and the sandbox's `Window`), the keysyms the key mapping uses (mirror of `iyakki_rfb::keysyms`), and what the tile asks of the rest of the
//! system. The sandbox's safety rules (the command, the folders, the namespaces, the network switch) stay in `iyakki_shared::linuxwin`; the tile
//! never builds a command, it asks the rig to start a program.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// At most this many windows run at once in a tile (mirror of `iyakki_shared::linuxwin::MAX_SLOTS`).
pub const MAX_SLOTS: u32 = 4;
/// The smallest and the largest side of the remote screen the tile asks for (mirror of `MIN_SIZE`, `MAX_SIZE`).
pub const MIN_SIZE: u32 = 64;
pub const MAX_SIZE: u32 = 8192;
/// The programs offered for install (package, what it is; mirror of `TEST_PROGRAMS`).
pub const TEST_PROGRAMS: [(&str, &str); 3] = [("xterm", "xterm: a small terminal window (MIT)"), ("x11-apps", "x11-apps: xclock, xeyes and friends (MIT)"), ("mousepad", "mousepad: a small text editor (GPL-2.0)")];
/// What the tile says about the sandbox (mirror of `NOTE`).
pub const NOTE: &str = "Linux windows run a graphical program of a Debian sandbox in an IYAKKI window. Debian's packages (TigerVNC's X server, matchbox, fonts, Mesa and the programs you add) are downloaded by apt inside the sandbox at your click and never shipped; each keeps its own licence (/usr/share/doc/<package>/copyright). There is no graphics card and no sound in the window, so it is slower than a program on a normal computer.";

/// A graphical program of the sandbox (mirror of `iyakki_shared::linuxwin::Program`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    /// The file name without `.desktop`.
    pub id: String,
    pub name: String,
    /// The command as words (no shell).
    pub exec: Vec<String>,
}

/// What the sandbox's processes use now (mirror of `iyakki_shared::linuxwin::Usage`): the process count, resident memory (kB) and cpu time (clock ticks, 100 a second).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub procs: u32,
    pub rss_kb: u64,
    pub ticks: u64,
}

/// Cpu use in percent of one core between two samples taken `secs` apart (mirror of `iyakki_shared::linuxwin::cpu_percent`).
pub fn cpu_percent(before: Usage, after: Usage, secs: f32) -> f32 {
    if secs <= 0.0 || after.ticks < before.ticks {
        return 0.0;
    }
    (((after.ticks - before.ticks) as f32) / 100.0 / secs * 100.0).clamp(0.0, 100_000.0)
}

/// What is installed in the Linux windows sandbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxInfo {
    /// The display server and the session script are there.
    pub ready: bool,
    /// The graphical programs found in it (empty unless ready).
    pub programs: Vec<Program>,
    /// Its network switch (false when it cannot be read).
    pub network: bool,
}

/// What a remote screen reports since the last look: the whole picture, or a part of it, as RGBA bytes (alpha 255).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenUpdate {
    /// The picture is new or has a new size.
    Full { w: u32, h: u32, rgba: Vec<u8> },
    /// A part of the picture changed; `rgba` holds `w * h` pixels.
    Rect { x: u32, y: u32, w: u32, h: u32, rgba: Vec<u8> },
    /// Text the program's clipboard got.
    Clip(String),
}

/// The remote screen of a window (the shell's is an RFB link on the sandbox's socket).
pub trait Screen: Send {
    /// The handshake is done.
    fn ready(&self) -> bool;
    /// Why the connection ended, when it did.
    fn ended(&self) -> Option<String>;
    /// What changed since the last call.
    fn take_updates(&self) -> Vec<ScreenUpdate>;
    /// A pointer move or press: position in the remote screen and the button mask (1 left, 4 right, 8 wheel up, 16 wheel down).
    fn pointer(&self, x: i32, y: i32, buttons: u8);
    /// A key event `(down, keysym)`.
    fn key(&self, down: bool, keysym: u32);
    /// Put typed text on the program's clipboard.
    fn cut_text(&self, text: &str);
    /// Ask for a new screen size; false when the server cannot resize.
    fn request_resize(&self, w: u32, h: u32) -> bool;
    fn close(&mut self);
}

/// The program's process in the sandbox. Dropping it stops the sandbox.
pub trait Window: Send {
    fn alive(&mut self) -> bool;
    fn pid(&self) -> Option<u32>;
    /// Stop the program and everything of the sandbox that runs.
    fn stop(&mut self);
}

/// A started window: its process and its screen.
pub type Live = (Box<dyn Window>, Box<dyn Screen>);

/// What the tile needs of the sandbox, the spawner and the process table (the shell's rig; a fake in tests). Used from threads of the tile's own.
pub trait WinRig: Send + Sync {
    /// What is installed (files only); an error says why the sandbox manager could not be made.
    fn sandbox(&self) -> Result<SandboxInfo, String>;
    /// The network switch of the sandbox, as a start must check it before it runs anything.
    fn network(&self) -> Result<bool, String>;
    /// Start `program` in slot `slot` with a screen of `w` x `h`, wait for its display and connect to it. Blocks (the tile runs it on a thread);
    /// setting `cancel` stops the wait.
    fn start(&self, program: &Program, w: u32, h: u32, slot: u32, cancel: &AtomicBool) -> Result<Live, String>;
    /// What the processes below `pid` use (read from the process table).
    fn usage(&self, pid: u32) -> Usage;
    /// Install one of the test programs with apt in the sandbox. Blocks.
    fn install_program(&self, package: &str) -> Result<(), String>;
}

/// What the LINUX WINDOW tile asks of the rest of the system.
pub trait LinuxWinPort {
    /// The sandbox, the spawner and the process table of the signed-in person.
    fn rig(&self) -> Arc<dyn WinRig>;
    /// The person may install programs for the sandbox (administrators).
    fn can_install(&self) -> bool;
    /// The person has the full command-line permission (Linux windows run programs of the person's choosing).
    fn full_cli(&self) -> bool;
    /// The person's network policy is "any host".
    fn all_hosts(&self) -> bool;
    /// The "Install Linux windows" job of this person is running now.
    fn base_install_running(&self) -> bool;
    /// Start that install as a detached job (administrators); the audit line is written there.
    fn start_base_install(&self) -> Result<(), String>;
    /// The last key press was a Command chord of the Mac profile (Command+C arrives as Ctrl+C).
    fn last_press_was_command(&self) -> bool;
    /// A steady clock in milliseconds (a number the test sets).
    fn now_ms(&self) -> u64;
}

/// The X11 keysyms the key mapping uses (mirror of `iyakki_rfb::keysyms`; a test in the shell keeps them equal).
pub mod keysyms {
    pub const BACKSPACE: u32 = 0xff08;
    pub const TAB: u32 = 0xff09;
    pub const RETURN: u32 = 0xff0d;
    pub const ESCAPE: u32 = 0xff1b;
    pub const HOME: u32 = 0xff50;
    pub const LEFT: u32 = 0xff51;
    pub const UP: u32 = 0xff52;
    pub const RIGHT: u32 = 0xff53;
    pub const DOWN: u32 = 0xff54;
    pub const PAGE_UP: u32 = 0xff55;
    pub const PAGE_DOWN: u32 = 0xff56;
    pub const END: u32 = 0xff57;
    pub const INSERT: u32 = 0xff63;
    pub const DELETE: u32 = 0xffff;
    pub const SHIFT_L: u32 = 0xffe1;
    pub const CONTROL_L: u32 = 0xffe3;
    pub const ALT_L: u32 = 0xffe9;
    pub const ISO_LEFT_TAB: u32 = 0xfe20;
    pub const F1: u32 = 0xffbe;

    /// F1 to F12 (anything else gives F1 or F12).
    pub fn function_key(n: u8) -> u32 {
        F1 + u32::from(n.clamp(1, 12)) - 1
    }

    /// The keysym of a character: Latin-1 is its own code, other Unicode is `0x0100_0000 + code`.
    pub fn from_char(c: char) -> u32 {
        let u = c as u32;
        if (0x20..=0x7e).contains(&u) || (0xa0..=0xff).contains(&u) {
            u
        } else {
            0x0100_0000 + u
        }
    }
}
