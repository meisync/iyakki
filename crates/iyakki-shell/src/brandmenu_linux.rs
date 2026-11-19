//! The Linux apps page of the IYAKKI menu (child module of `brandmenu`; `docs/LINUX-APPS.md`): Debian and Ubuntu sandboxes (make one, open a
//! terminal in it, install packages, network switch, mount folders, delete with the name typed) and the image apps (ready-made container
//! images run in the dev cluster). The page starts nothing itself: the buttons return [`LinuxAction`]s and `shell_linux.rs` does them
//! (jobs, the CLI tile, the cluster). The facts come from the agent crate when the page opens and after every action.

use super::*;
use iyakki_shared::linuxapps_dist::RELEASES;

pub const L_TAB: usize = BASE + 200;
pub const L_ROW: usize = BASE + 210;
pub const L_IMG: usize = BASE + 260;
pub const L_REL: usize = BASE + 310;
pub const L_TERMINAL: usize = BASE + 320;
pub const L_INSTALL: usize = BASE + 321;
pub const L_REMOVE: usize = BASE + 322;
pub const L_UPGRADE: usize = BASE + 323;
pub const L_NET: usize = BASE + 324;
pub const L_MOUNT_RO: usize = BASE + 325;
pub const L_MOUNT_RW: usize = BASE + 326;
pub const L_UNMOUNT: usize = BASE + 327;
pub const L_DELETE: usize = BASE + 328;
pub const L_DELETE_YES: usize = BASE + 329;
pub const L_DELETE_NO: usize = BASE + 330;
pub const L_CREATE: usize = BASE + 331;
pub const L_NEWNET: usize = BASE + 332;
pub const L_NEWRW: usize = BASE + 333;
pub const L_RUN: usize = BASE + 334;
pub const L_STOP: usize = BASE + 335;
pub const L_REFRESH: usize = BASE + 336;
pub const L_IMGMAKE: usize = BASE + 337;
pub const L_IMGYES: usize = BASE + 338;
pub const L_IMGNO: usize = BASE + 339;
pub const L_PKGS: usize = BASE + 340;
pub const L_FOLDER: usize = BASE + 341;
pub const L_NAME: usize = BASE + 342;
pub const L_CONFIRM: usize = BASE + 343;
pub const L_ARCHIVE: usize = BASE + 344;
pub const L_PORT: usize = BASE + 345;
pub const L_LICENCE: usize = BASE + 346;
pub const L_CMD: usize = BASE + 347;
const MAX_ROWS: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LView {
    Sandboxes,
    New,
    Images,
    /// Make an image app from the archived root of a sandbox (the chat tool `linuxapp_image`, with its card).
    Archive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    None,
    Pkgs,
    Folder,
    Name,
    Confirm,
    Archive,
    Port,
    Licence,
    Cmd,
}

/// What a button of the page asks for; `shell_linux.rs` does it.
use iyakki_shared::linuxapps_net::Mode as NetMode;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinuxAction {
    /// Read the facts again.
    Refresh,
    /// Download a base and make a sandbox (a job): name, release id, network on, and a folder to mount (path, read-write).
    Create { name: String, release: String, network: bool, folder: Option<(String, bool)> },
    Install { name: String, packages: Vec<String> },
    Remove { name: String, packages: Vec<String> },
    Upgrade { name: String },
    SetNetwork { name: String, on: bool },
    /// The network switch of the page: off, outbound only (own namespace, the computer unreachable) or shared with the computer.
    SetNetMode { name: String, mode: NetMode },
    Mount { name: String, folder: String, rw: bool },
    Unmount { name: String, folder: String },
    /// Open the CLI tile inside the sandbox.
    Terminal { name: String },
    /// The sandbox's name was typed again.
    Delete { name: String, typed: String },
    RunImage { id: String },
    StopImage { id: String },
    /// Make an image app from a sandbox's archived root. `confirmed` false: show the card (what goes in, what runs); true: the person said yes.
    ImageFromRoot { archive: String, name: String, port: u16, licence: String, command: Vec<String>, confirmed: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoxRow {
    pub name: String,
    pub title: String,
    pub network: bool,
    /// Off, outbound only or shared with the computer (`network` is true for the last two).
    pub mode: NetMode,
    /// `(folder on the machine, mount point inside, read-write)`
    pub mounts: Vec<(String, String, bool)>,
    pub mb: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageRow {
    pub id: String,
    pub name: String,
    pub description: String,
    pub licence: String,
    pub example: bool,
    pub pinned: bool,
    pub image: String,
    pub status: String,
}

/// What the page shows; the shell reads it from the agent crate.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinuxFacts {
    pub boxes: Vec<BoxRow>,
    pub images: Vec<ImageRow>,
    /// Image app files that were not valid.
    pub problems: Vec<String>,
    /// Kernel options missing for the sandboxes (empty when they can run).
    pub kernel_missing: Vec<String>,
    /// Nice-to-have options missing (limits, filter).
    pub optional_missing: Vec<String>,
    /// How the sandboxes run ("namespaces" / "pod in the dev cluster" / why not).
    pub runtime: String,
    pub jobs_running: usize,
    /// The account may use it (full command-line permission).
    pub allowed: bool,
    /// The account may reach every host (a sandbox with its network on can).
    pub any_host: bool,
    pub cluster: bool,
}

pub struct LinuxPage {
    pub view: LView,
    pub facts: Option<LinuxFacts>,
    pub pkgs: TextField,
    pub folder: TextField,
    pub name: TextField,
    pub confirm: TextField,
    pub archive: TextField,
    pub port: TextField,
    pub licence: TextField,
    pub command: TextField,
    /// The card of an image made from an archive that waits for Yes or No: its text and the action to run on Yes.
    pub card: Option<(String, LinuxAction)>,
    focus: Focus,
    rel: usize,
    new_net: bool,
    new_rw: bool,
    confirm_for: Option<String>,
    /// The sandbox and image lists' scrolling (one helper, `scrolllist.rs`), and the row that was chosen at the last drawing: a change
    /// of the choice by a key scrolls it into view, the wheel and a drag do not move the choice.
    pub(super) list: ScrollList,
    seen_sel: Option<usize>,
}

impl LinuxPage {
    pub fn new() -> LinuxPage {
        LinuxPage {
            view: LView::Sandboxes,
            facts: None,
            pkgs: TextField::new("packages, e.g. curl git python3", false),
            folder: TextField::new("/home/you/project", false),
            name: TextField::new("name, e.g. dev", false),
            confirm: TextField::new("type the name to delete", false),
            archive: TextField::new("archive in your home, e.g. Downloads/root.tar.gz", false),
            port: TextField::new("port the app listens on, e.g. 8080", false),
            licence: TextField::new("licence, e.g. MIT", false),
            command: TextField::new("command that starts it, e.g. /usr/bin/myapp --serve", false),
            card: None,
            focus: Focus::None,
            rel: 0,
            new_net: true,
            new_rw: false,
            confirm_for: None,
            list: ScrollList::default(),
            seen_sel: None,
        }
    }

    pub(super) fn reset(&mut self) {
        self.view = LView::Sandboxes;
        self.focus = Focus::None;
        self.confirm_for = None;
        self.list.to_top();
        self.seen_sel = None;
    }

    /// Place the list for this drawing; a choice that changed since the last drawing (a key) is brought into view.
    fn layout_list(&mut self, view: R, n: usize, rh: f32, s: f32, sel: usize) {
        self.list.layout(view, n as f32 * rh, rh, s);
        if self.seen_sel != Some(sel) {
            self.list.show(sel as f32 * rh, (sel + 1) as f32 * rh);
            self.seen_sel = Some(sel);
        }
    }

    fn list_len(&self) -> usize {
        match (self.view, &self.facts) {
            (LView::Sandboxes, Some(f)) => f.boxes.len().min(MAX_ROWS),
            (LView::Images, Some(f)) => f.images.len().min(MAX_ROWS),
            _ => 0,
        }
    }

    fn field(&mut self) -> Option<&mut TextField> {
        match self.focus {
            Focus::None => None,
            Focus::Pkgs => Some(&mut self.pkgs),
            Focus::Folder => Some(&mut self.folder),
            Focus::Name => Some(&mut self.name),
            Focus::Confirm => Some(&mut self.confirm),
            Focus::Archive => Some(&mut self.archive),
            Focus::Port => Some(&mut self.port),
            Focus::Licence => Some(&mut self.licence),
            Focus::Cmd => Some(&mut self.command),
        }
    }
}

impl Default for LinuxPage {
    fn default() -> LinuxPage {
        LinuxPage::new()
    }
}

fn words(t: &str) -> Vec<String> {
    t.split(|c: char| c.is_whitespace() || c == ',').filter(|w| !w.is_empty()).map(str::to_string).collect()
}

impl BrandMenu {
    pub(super) fn linux_reset(&mut self) {
        self.linux.reset();
    }

    /// The page's facts (the shell fills them when the page opens and after every action).
    pub fn set_linux(&mut self, f: LinuxFacts) {
        let n = match self.linux.view {
            LView::Sandboxes => f.boxes.len(),
            LView::Images => f.images.len(),
            LView::New | LView::Archive => 0,
        };
        if n > 0 && self.sel >= n {
            self.sel = n - 1;
        }
        self.linux.facts = Some(f);
    }

    pub(super) fn linux_rows(&self) -> usize {
        self.linux.list_len()
    }

    fn lbox(&self) -> Option<&BoxRow> {
        self.linux.facts.as_ref().and_then(|f| f.boxes.get(self.sel))
    }

    fn limage(&self) -> Option<&ImageRow> {
        self.linux.facts.as_ref().and_then(|f| f.images.get(self.sel))
    }

    fn set_view(&mut self, v: LView) {
        self.linux.view = v;
        self.linux.focus = Focus::None;
        self.linux.confirm_for = None;
        self.linux.list.to_top();
        self.linux.seen_sel = None;
        self.sel = 0;
        self.note = None;
    }

    fn lnote(&mut self, t: impl Into<String>) -> Option<Outcome> {
        self.note = Some((t.into(), false));
        Some(Outcome::Nothing)
    }

    fn linux_need_allowed(&mut self) -> Option<Outcome> {
        match self.linux.facts.as_ref() {
            Some(f) if !f.allowed => self.lnote("Linux apps need the full command-line permission; ask an administrator."),
            _ => None,
        }
    }

    fn act_terminal(&mut self) -> Option<Outcome> {
        if let Some(o) = self.linux_need_allowed() {
            return Some(o);
        }
        let name = self.lbox()?.name.clone();
        Some(Outcome::Linux(LinuxAction::Terminal { name }))
    }

    fn act_install(&mut self, remove: bool) -> Option<Outcome> {
        if let Some(o) = self.linux_need_allowed() {
            return Some(o);
        }
        let name = self.lbox()?.name.clone();
        let packages = words(&self.linux.pkgs.value);
        if packages.is_empty() {
            self.linux.focus = Focus::Pkgs;
            return self.lnote("Type the packages first (names of Debian/Ubuntu packages, separated by spaces), then press Install.");
        }
        if let Some(bad) = packages.iter().find(|p| !iyakki_apps::agent().linuxapps_valid_package(p)) {
            return self.lnote(format!("`{bad}` is not a package name (lower-case letters, digits, + . - only)."));
        }
        Some(Outcome::Linux(if remove { LinuxAction::Remove { name, packages } } else { LinuxAction::Install { name, packages } }))
    }

    fn act_network(&mut self) -> Option<Outcome> {
        if let Some(o) = self.linux_need_allowed() {
            return Some(o);
        }
        let b = self.lbox()?.clone();
        if !b.network && !self.linux.facts.as_ref().is_some_and(|f| f.any_host) {
            return self.lnote("A sandbox with its network on can reach any server; only an account allowed every host may turn it on.");
        }
        let mode = match b.mode {
            NetMode::Off => NetMode::OutboundOnly,
            NetMode::OutboundOnly => NetMode::HostShared,
            NetMode::HostShared => NetMode::Off,
        };
        Some(Outcome::Linux(LinuxAction::SetNetMode { name: b.name, mode }))
    }

    fn act_mount(&mut self, rw: bool) -> Option<Outcome> {
        if let Some(o) = self.linux_need_allowed() {
            return Some(o);
        }
        let name = self.lbox()?.name.clone();
        let folder = self.linux.folder.value.trim().to_string();
        if folder.is_empty() {
            self.linux.focus = Focus::Folder;
            return self.lnote("Type the full path of the folder to mount, then press Mount. Read-only is the safe choice; read-write lets the sandbox change your files.");
        }
        Some(Outcome::Linux(LinuxAction::Mount { name, folder, rw }))
    }

    fn act_unmount(&mut self) -> Option<Outcome> {
        let name = self.lbox()?.name.clone();
        let folder = self.linux.folder.value.trim().to_string();
        if folder.is_empty() {
            self.linux.focus = Focus::Folder;
            return self.lnote("Type the folder (or its mount point, such as /mnt/work) to unmount.");
        }
        Some(Outcome::Linux(LinuxAction::Unmount { name, folder }))
    }

    fn act_delete_ask(&mut self) -> Option<Outcome> {
        let name = self.lbox()?.name.clone();
        self.linux.confirm.set("");
        self.linux.confirm_for = Some(name);
        self.linux.focus = Focus::Confirm;
        self.lnote("Deleting removes the sandbox and everything installed or saved inside it. Type its name to confirm.")
    }

    fn act_delete_do(&mut self) -> Option<Outcome> {
        let name = self.linux.confirm_for.clone()?;
        let typed = self.linux.confirm.value.trim().to_string();
        if typed != name {
            return self.lnote(format!("Type {name} exactly to delete it; nothing was deleted."));
        }
        self.linux.confirm_for = None;
        self.linux.focus = Focus::None;
        Some(Outcome::Linux(LinuxAction::Delete { name, typed }))
    }

    fn act_create(&mut self) -> Option<Outcome> {
        if let Some(o) = self.linux_need_allowed() {
            return Some(o);
        }
        let name = self.linux.name.value.trim().to_string();
        if !iyakki_apps::agent().linuxapps_valid_name(&name) {
            self.linux.focus = Focus::Name;
            return self.lnote("Give the sandbox a name: lower-case letters, digits and -, starting with a letter (for example dev).");
        }
        if self.linux.new_net && !self.linux.facts.as_ref().is_some_and(|f| f.any_host) {
            return self.lnote("A sandbox with its network on can reach any server; only an account allowed every host may make one. Switch Network off, or ask an administrator.");
        }
        let folder = self.linux.folder.value.trim();
        let folder = (!folder.is_empty()).then(|| (folder.to_string(), self.linux.new_rw));
        Some(Outcome::Linux(LinuxAction::Create { name, release: RELEASES[self.linux.rel].id.to_string(), network: self.linux.new_net, folder }))
    }

    /// The archive form: check the fields here, then ask the shell for the card (the same one the chat tool shows).
    fn act_image_card(&mut self) -> Option<Outcome> {
        if let Some(o) = self.linux_need_allowed() {
            return Some(o);
        }
        let archive = self.linux.archive.value.trim().to_string();
        let name = self.linux.name.value.trim().to_string();
        if archive.is_empty() {
            self.linux.focus = Focus::Archive;
            return self.lnote("Give the archive: the .tar.gz you made inside the sandbox, in your home folder.");
        }
        if name.is_empty() {
            self.linux.focus = Focus::Name;
            return self.lnote("Give the app a name.");
        }
        let Some(port) = self.linux.port.value.trim().parse::<u16>().ok().filter(|p| *p >= 1024) else {
            self.linux.focus = Focus::Port;
            return self.lnote("Give the port the app listens on: a number from 1024 to 65535.");
        };
        let licence = self.linux.licence.value.trim().to_string();
        if licence.is_empty() {
            self.linux.focus = Focus::Licence;
            return self.lnote("An image app needs its licence (for example MIT).");
        }
        let command = words(&self.linux.command.value);
        if command.first().is_none_or(|c| !c.starts_with('/')) {
            self.linux.focus = Focus::Cmd;
            return self.lnote("Give the command that starts the app, beginning with a full path such as /usr/bin/myapp.");
        }
        self.linux.card = None;
        Some(Outcome::Linux(LinuxAction::ImageFromRoot { archive, name, port, licence, command, confirmed: false }))
    }

    /// The shell made the card: show it with Yes and No.
    pub fn set_image_card(&mut self, text: String, yes: LinuxAction) {
        self.linux.card = Some((text, yes));
    }

    fn act_image_yes(&mut self) -> Option<Outcome> {
        let (_, a) = self.linux.card.take()?;
        Some(Outcome::Linux(a))
    }

    fn act_run_image(&mut self, stop: bool) -> Option<Outcome> {
        if let Some(o) = self.linux_need_allowed() {
            return Some(o);
        }
        let a = self.limage()?.clone();
        if !stop && !a.pinned {
            return self.lnote(format!("{} cannot run yet: its image digest is a placeholder. Run dist/release/pin-image-apps.sh (or put the real digest in the file).", a.name));
        }
        Some(Outcome::Linux(if stop { LinuxAction::StopImage { id: a.id } } else { LinuxAction::RunImage { id: a.id } }))
    }

    /// The Enter key on the page without a field in focus: the main action of the view.
    fn linux_enter(&mut self) -> Option<Outcome> {
        match self.linux.view {
            LView::Sandboxes => self.act_terminal(),
            LView::New => self.act_create(),
            LView::Images => self.act_run_image(false),
            LView::Archive => if self.linux.card.is_some() { self.act_image_yes() } else { self.act_image_card() },
        }
    }

    /// Keys of the page. `None`: the menu's usual keys apply (Up/Down, Esc, ...).
    /// The wheel or the trackpad over the list: a notch scrolls three rows (the choice stays where it is); not while a field is being
    /// typed in.
    pub(super) fn linux_wheel(&mut self, d: i32) {
        if self.linux.focus != Focus::None {
            return;
        }
        self.linux.list.wheel(d);
    }

    pub(super) fn linux_key(&mut self, k: &Key) -> Option<Outcome> {
        if self.linux.focus != Focus::None {
            let focus = self.linux.focus;
            return match k {
                Key::Esc => {
                    self.linux.focus = Focus::None;
                    Some(Outcome::Nothing)
                }
                Key::Enter => match focus {
                    Focus::Pkgs => self.act_install(false),
                    Focus::Folder if self.linux.view == LView::New => self.act_create(),
                    Focus::Folder => self.act_mount(false),
                    Focus::Name => self.act_create(),
                    Focus::Confirm => self.act_delete_do(),
                    Focus::Archive | Focus::Port | Focus::Licence | Focus::Cmd => self.act_image_card(),
                    Focus::None => None,
                },
                Key::Tab => {
                    self.linux.focus = match (self.linux.view, focus) {
                        (LView::Archive, Focus::Archive) => Focus::Name,
                        (LView::Archive, Focus::Name) => Focus::Port,
                        (LView::Archive, Focus::Port) => Focus::Licence,
                        (LView::Archive, Focus::Licence) => Focus::Cmd,
                        (LView::Archive, _) => Focus::Archive,
                        (LView::New, Focus::Name) => Focus::Folder,
                        (LView::New, _) => Focus::Name,
                        (_, Focus::Pkgs) => Focus::Folder,
                        _ => Focus::Pkgs,
                    };
                    Some(Outcome::Nothing)
                }
                other => {
                    if let Some(f) = self.linux.field() {
                        f.handle(other);
                    }
                    Some(Outcome::Nothing)
                }
            };
        }
        match k {
            Key::Enter => self.linux_enter(),
            Key::Char(c) => match c.to_ascii_lowercase() {
                '1' => {
                    self.set_view(LView::Sandboxes);
                    Some(Outcome::Nothing)
                }
                '2' => {
                    self.set_view(LView::New);
                    Some(Outcome::Nothing)
                }
                '3' => {
                    self.set_view(LView::Images);
                    Some(Outcome::Nothing)
                }
                '4' => {
                    self.set_view(LView::Archive);
                    Some(Outcome::Nothing)
                }
                'm' if self.linux.view == LView::Archive && self.linux.card.is_none() => self.act_image_card(),
                'y' if self.linux.view == LView::Archive && self.linux.card.is_some() => self.act_image_yes(),
                'n' if self.linux.view == LView::Archive && self.linux.card.is_some() => {
                    self.linux.card = None;
                    Some(Outcome::Nothing)
                }
                'r' => Some(Outcome::Linux(LinuxAction::Refresh)),
                't' if self.linux.view == LView::Sandboxes => self.act_terminal(),
                'i' if self.linux.view == LView::Sandboxes => {
                    self.linux.focus = Focus::Pkgs;
                    Some(Outcome::Nothing)
                }
                'n' if self.linux.view == LView::Sandboxes => self.act_network(),
                'd' if self.linux.view == LView::Sandboxes => self.act_delete_ask(),
                'u' if self.linux.view == LView::Sandboxes => {
                    if let Some(o) = self.linux_need_allowed() {
                        return Some(o);
                    }
                    let name = self.lbox()?.name.clone();
                    Some(Outcome::Linux(LinuxAction::Upgrade { name }))
                }
                'c' if self.linux.view == LView::New => self.act_create(),
                'a' | 'x' if self.linux.view == LView::Images => self.act_run_image(c.eq_ignore_ascii_case(&'x')),
                _ => Some(Outcome::Nothing),
            },
            _ => None,
        }
    }

    /// Clicks and typing by mouse on the page's own regions. `None`: not ours.
    pub(super) fn linux_hit(&mut self, ev: &Event) -> Option<Outcome> {
        match ev {
            Event::Tab(n) if (L_TAB..L_TAB + 4).contains(n) => {
                self.set_view([LView::Sandboxes, LView::New, LView::Images, LView::Archive][n - L_TAB]);
                Some(Outcome::Nothing)
            }
            Event::Tab(n) if (L_ROW..L_ROW + MAX_ROWS).contains(n) || (L_IMG..L_IMG + MAX_ROWS).contains(n) => {
                // a drag that follows the press scrolls the list with the finger
                self.linux.list.body_press();
                self.sel = if *n >= L_IMG { n - L_IMG } else { n - L_ROW };
                self.linux.confirm_for = None;
                self.note = None;
                Some(Outcome::Nothing)
            }
            Event::Tab(n) if (L_REL..L_REL + RELEASES.len()).contains(n) => {
                self.linux.rel = n - L_REL;
                Some(Outcome::Nothing)
            }
            Event::Tab(L_TERMINAL) => self.act_terminal(),
            Event::Tab(L_INSTALL) => self.act_install(false),
            Event::Tab(L_REMOVE) => self.act_install(true),
            Event::Tab(L_UPGRADE) => self.linux_key(&Key::Char('u')),
            Event::Tab(L_NET) => self.act_network(),
            Event::Tab(L_MOUNT_RO) => self.act_mount(false),
            Event::Tab(L_MOUNT_RW) => self.act_mount(true),
            Event::Tab(L_UNMOUNT) => self.act_unmount(),
            Event::Tab(L_DELETE) => self.act_delete_ask(),
            Event::Tab(L_DELETE_YES) => self.act_delete_do(),
            Event::Tab(L_DELETE_NO) => {
                self.linux.confirm_for = None;
                self.linux.focus = Focus::None;
                self.note = None;
                Some(Outcome::Nothing)
            }
            Event::Tab(L_CREATE) => self.act_create(),
            Event::Tab(L_NEWNET) => {
                self.linux.new_net = !self.linux.new_net;
                Some(Outcome::Nothing)
            }
            Event::Tab(L_NEWRW) => {
                self.linux.new_rw = !self.linux.new_rw;
                Some(Outcome::Nothing)
            }
            Event::Tab(L_RUN) => self.act_run_image(false),
            Event::Tab(L_STOP) => self.act_run_image(true),
            Event::Tab(L_REFRESH) => Some(Outcome::Linux(LinuxAction::Refresh)),
            Event::Tab(L_IMGMAKE) => self.act_image_card(),
            Event::Tab(L_IMGYES) => self.act_image_yes(),
            Event::Tab(L_IMGNO) => {
                self.linux.card = None;
                Some(Outcome::Nothing)
            }
            Event::FocusAt(id, x) if [L_PKGS, L_FOLDER, L_NAME, L_CONFIRM, L_ARCHIVE, L_PORT, L_LICENCE, L_CMD].contains(id) => {
                self.linux.focus = match *id {
                    L_PKGS => Focus::Pkgs,
                    L_FOLDER => Focus::Folder,
                    L_NAME => Focus::Name,
                    L_ARCHIVE => Focus::Archive,
                    L_PORT => Focus::Port,
                    L_LICENCE => Focus::Licence,
                    L_CMD => Focus::Cmd,
                    _ => Focus::Confirm,
                };
                if let Some(f) = self.linux.field() {
                    f.click(*x);
                }
                Some(Outcome::Nothing)
            }
            Event::Paste(t) => {
                if let Some(f) = self.linux.field() {
                    f.paste(t);
                }
                Some(Outcome::Nothing)
            }
            _ => None,
        }
    }

    // ------------------------------------------------------------------------------------------------ drawing

    pub(super) fn linux_foot(&self) -> &'static str {
        match self.linux.view {
            LView::Sandboxes => "1 2 3 views · Up/Down choose · t terminal · i install · n network · d delete · r refresh · Esc back",
            LView::New => "1 2 3 views · c create · Tab next field · Esc back",
            LView::Images => "1 2 3 4 views · Up/Down choose · a run · x stop · r refresh · Esc back",
            LView::Archive => "1 2 3 4 views · m make (shows the card) · y yes · n no · Tab next field · Esc back",
        }
    }

    pub(super) fn draw_linux(&mut self, g: &mut Gfx, body: R, s: f32, tick: u64) {
        let blink = (tick / 15) % 2 == 0;
        let size = 14.0 * s;
        let bh = size * 2.1 + 6.0 * s;
        let gap = 10.0 * s;
        let allowed = self.linux.facts.as_ref().is_none_or(|f| f.allowed);
        // ---- the top: how the sandboxes run (or what the kernel lacks), then the three views
        let mut y = body.y;
        let (status, col) = match &self.linux.facts {
            None => ("looking...".to_string(), DIM),
            Some(f) if !f.kernel_missing.is_empty() => (format!("Sandboxes cannot run on this kernel. Missing: {}. A kernel rebuild with these options is the owner's to schedule.", f.kernel_missing.join(", ")), ERR),
            Some(f) if !f.optional_missing.is_empty() => (format!("{}. Not enforced here: {}", f.runtime, f.optional_missing.join("; ")), AMBER),
            Some(f) => (f.runtime.clone(), OK),
        };
        for l in g.wrap(11.5 * s, &status, Face::Sans, body.w).iter().take(2) {
            g.text(body.x, y, 11.5 * s, col, l, Face::Sans);
            y += 15.0 * s;
        }
        y += 4.0 * s;
        let mut x = body.x;
        for (i, (label, v)) in [("Sandboxes", LView::Sandboxes), ("New", LView::New), ("Image apps", LView::Images), ("From archive", LView::Archive)].iter().enumerate() {
            x += g.button(x, y, size, label, if self.linux.view == *v { ACCENT } else { MUTED }, Event::Tab(L_TAB + i)) + gap;
        }
        g.button(x, y, size, "Refresh", DIM, Event::Tab(L_REFRESH));
        y += bh + 2.0 * s;
        if !allowed {
            for l in g.wrap(13.0 * s, "Linux apps run programs of your choosing, so they need the full command-line permission. Ask an administrator for it.", Face::Sans, body.w).iter().take(3) {
                g.text(body.x, y, 13.0 * s, AMBER, l, Face::Sans);
                y += 18.0 * s;
            }
            return;
        }
        let area = R::new(body.x, y, body.w, (body.y + body.h - y).max(0.0));
        match self.linux.view {
            LView::Sandboxes => self.draw_boxes(g, area, s, blink),
            LView::New => self.draw_new(g, area, s, blink),
            LView::Images => self.draw_images(g, area, s),
            LView::Archive => self.draw_archive(g, area, s, blink),
        }
    }

    /// Buttons in lines that wrap at the width; returns each one's (x, line) and the number of lines.
    fn flow(g: &mut Gfx, w: f32, size: f32, labels: &[String]) -> (Vec<(f32, f32)>, f32) {
        let (mut x, mut line) = (0.0f32, 0.0f32);
        let pos: Vec<(f32, f32)> = labels
            .iter()
            .map(|l| {
                let bw = g.text_width(size, l, Face::Bold) + size * 1.7;
                if x > 0.0 && x + bw > w {
                    x = 0.0;
                    line += 1.0;
                }
                let at = (x, line);
                x += bw + size * 0.7;
                at
            })
            .collect();
        (pos, if labels.is_empty() { 0.0 } else { line + 1.0 })
    }

    fn draw_boxes(&mut self, g: &mut Gfx, area: R, s: f32, blink: bool) {
        let size = 14.0 * s;
        let bh = size * 2.1 + 6.0 * s;
        let field_h = 34.0 * s;
        let Some(f) = self.linux.facts.clone() else { return };
        let sel = self.sel.min(f.boxes.len().saturating_sub(1));
        let chosen = f.boxes.get(sel).cloned();
        // ---- the foot: detail, fields, buttons
        let confirming = self.linux.confirm_for.is_some() && chosen.as_ref().map(|b| &b.name) == self.linux.confirm_for.as_ref();
        let net_label = chosen.as_ref().map_or("Network".to_string(), |b| format!("Network: {} (switch)", b.mode.short()));
        let labels: Vec<String> = if confirming {
            vec!["Delete for good".into(), "Cancel".into()]
        } else {
            ["Terminal", "Install", "Remove", "Upgrade"].iter().map(|s| s.to_string()).chain([net_label.clone(), "Mount read-only".into(), "Mount read-write".into(), "Unmount".into(), "Delete".into()]).collect()
        };
        let ids: Vec<usize> = if confirming { vec![L_DELETE_YES, L_DELETE_NO] } else { vec![L_TERMINAL, L_INSTALL, L_REMOVE, L_UPGRADE, L_NET, L_MOUNT_RO, L_MOUNT_RW, L_UNMOUNT, L_DELETE] };
        let (pos, lines) = Self::flow(g, area.w, size, &labels);
        let detail: Vec<String> = match &chosen {
            None => vec![],
            Some(b) => {
                let mut d = vec![format!("{} \u{b7} {} \u{b7} network {} \u{b7} {} MB on disk", b.name, b.title, b.mode.label(), b.mb)];
                for (src, dst, rw) in b.mounts.iter().take(3) {
                    d.push(format!("{dst}  \u{2190}  {src}  ({})", if *rw { "read-write" } else { "read-only" }));
                }
                if b.mounts.is_empty() {
                    d.push("no folders mounted: it sees only its own files".into());
                }
                d
            }
        };
        let fields = if confirming { 1.0 } else { 2.0 };
        let fixed = lines * bh + fields * (field_h + 6.0 * s) + detail.len() as f32 * 15.0 * s + 8.0 * s;
        let top_end = (area.y + area.h - fixed).max(area.y);
        // ---- the list above
        let rh = 40.0 * s;
        if f.boxes.is_empty() {
            let mut y = area.y + 6.0 * s;
            for l in g.wrap(13.0 * s, "No sandboxes yet. Open New to download Debian or Ubuntu into one; programs installed there cannot see the rest of this machine.", Face::Sans, area.w).iter().take(4) {
                g.text(area.x, y, 13.0 * s, MUTED, l, Face::Sans);
                y += 18.0 * s;
            }
        } else {
            let n = f.boxes.len().min(MAX_ROWS);
            let view = R::new(area.x - 4.0 * s, area.y, area.w + 8.0 * s, (top_end - area.y).max(rh));
            self.linux.layout_list(view, n, rh, s, sel);
            let offset = self.linux.list.offset;
            for (i, b) in f.boxes.iter().enumerate().take(MAX_ROWS) {
                let r = R::new(area.x - 4.0 * s, area.y + i as f32 * rh - offset, area.w + 8.0 * s, rh - 4.0 * s);
                if r.y + r.h < view.y || r.y > view.y + view.h {
                    continue;
                }
                let hot = g.hovered(r);
                crate::fx::row_at(g, r.x, r.y, r.w, r.h, 10.0 * s, i == sel, hot, ACCENT);
                g.text(r.x + 12.0 * s, r.y + 3.0 * s, 14.0 * s, TEXT, &b.name, Face::Bold);
                let sub = g.fit(11.5 * s, &format!("{} \u{b7} network {} \u{b7} {} folder(s) \u{b7} {} MB", b.title, b.mode.short(), b.mounts.len(), b.mb), Face::Sans, r.w - 24.0 * s);
                g.text(r.x + 12.0 * s, r.y + 3.0 * s + g.line_height(14.0 * s, Face::Bold), 11.5 * s, MUTED, &sub, Face::Sans);
                g.hit(r, Event::Tab(L_ROW + i), &b.name);
            }
            self.linux.list.draw_bar(g, s, LINUX_BAR, ACCENT);
        }
        // ---- the foot
        let mut y = top_end;
        for (i, l) in detail.iter().enumerate() {
            let t = g.fit(11.5 * s, l, Face::Sans, area.w);
            g.text(area.x, y, 11.5 * s, if i == 0 { TEXT } else { MUTED }, &t, Face::Sans);
            y += 15.0 * s;
        }
        y += 8.0 * s;
        if confirming {
            self.linux.confirm.render(g, R::new(area.x, y, area.w, field_h), self.linux.focus == Focus::Confirm, s, blink, Face::Mono, L_CONFIRM);
            y += field_h + 6.0 * s;
        } else {
            self.linux.pkgs.render(g, R::new(area.x, y, area.w, field_h), self.linux.focus == Focus::Pkgs, s, blink, Face::Mono, L_PKGS);
            y += field_h + 6.0 * s;
            let bw = g.text_width(12.0 * s, "Browse\u{2026}", Face::Bold) + 12.0 * s * 1.7 + 8.0 * s;
            self.linux.folder.render(g, R::new(area.x, y, (area.w - bw).max(60.0 * s), field_h), self.linux.focus == Focus::Folder, s, blink, Face::Mono, L_FOLDER);
            crate::pathpick::PathPick::button(g, area.x + (area.w - bw).max(60.0 * s) + 8.0 * s, y + (field_h - 12.0 * s * 2.1).max(0.0) / 2.0, s, BROWSE_FOLDER, ACCENT);
            y += field_h + 6.0 * s;
        }
        if chosen.is_some() {
            for ((label, id), p) in labels.iter().zip(&ids).zip(&pos) {
                let accent = match *id {
                    L_DELETE | L_DELETE_YES => ERR,
                    L_TERMINAL => ACCENT,
                    L_INSTALL => ACCENT,
                    _ => MUTED,
                };
                g.button(area.x + p.0, y + p.1 * bh, size, label, accent, Event::Tab(*id));
            }
        }
    }

    fn draw_new(&mut self, g: &mut Gfx, area: R, s: f32, blink: bool) {
        let size = 14.0 * s;
        let bh = size * 2.1 + 6.0 * s;
        let field_h = 34.0 * s;
        let mut y = area.y;
        g.text(area.x, y, 13.0 * s, MUTED, "Base system (downloaded from the distribution, checksum-verified, never part of an image)", Face::Sans);
        y += 20.0 * s;
        let labels: Vec<String> = RELEASES.iter().enumerate().map(|(i, r)| format!("{}{}", if i == self.linux.rel { "\u{25cf} " } else { "" }, r.title)).collect();
        let (pos, lines) = Self::flow(g, area.w, size, &labels);
        for (i, (l, p)) in labels.iter().zip(&pos).enumerate() {
            g.button(area.x + p.0, y + p.1 * bh, size, l, if i == self.linux.rel { ACCENT } else { MUTED }, Event::Tab(L_REL + i));
        }
        y += lines * bh + 4.0 * s;
        self.linux.name.render(g, R::new(area.x, y, area.w, field_h), self.linux.focus == Focus::Name, s, blink, Face::Mono, L_NAME);
        y += field_h + 6.0 * s;
        let bw = g.text_width(12.0 * s, "Browse\u{2026}", Face::Bold) + 12.0 * s * 1.7 + 8.0 * s;
        self.linux.folder.render(g, R::new(area.x, y, (area.w - bw).max(60.0 * s), field_h), self.linux.focus == Focus::Folder, s, blink, Face::Mono, L_FOLDER);
        crate::pathpick::PathPick::button(g, area.x + (area.w - bw).max(60.0 * s) + 8.0 * s, y + (field_h - 12.0 * s * 2.1).max(0.0) / 2.0, s, BROWSE_FOLDER, ACCENT);
        y += field_h + 8.0 * s;
        let toggles = vec![format!("Network: {}", if self.linux.new_net { "on" } else { "off" }), format!("Folder: {}", if self.linux.new_rw { "read-write" } else { "read-only" }), "Create sandbox".to_string()];
        let (tpos, tlines) = Self::flow(g, area.w, size, &toggles);
        let ids = [L_NEWNET, L_NEWRW, L_CREATE];
        for ((l, id), p) in toggles.iter().zip(ids).zip(&tpos) {
            g.button(area.x + p.0, y + p.1 * bh, size, l, if id == L_CREATE { ACCENT } else { MUTED }, Event::Tab(id));
        }
        y += tlines * bh + 2.0 * s;
        let note = format!("{} Network: outbound only reaches the internet but never this computer or your own network; shared uses the computer's own network and can reach its services too. A mounted folder is the only part of your files it can see.", iyakki_shared::linuxapps_dist::LICENCE_NOTE);
        for l in g.wrap(11.0 * s, &note, Face::Sans, area.w) {
            if y + 14.0 * s > area.y + area.h {
                break;
            }
            g.text(area.x, y, 11.0 * s, DIM, &l, Face::Sans);
            y += 14.0 * s;
        }
    }

    fn draw_archive(&mut self, g: &mut Gfx, area: R, s: f32, blink: bool) {
        let size = 14.0 * s;
        let field_h = 34.0 * s;
        let mut y = area.y;
        for l in g.wrap(12.0 * s, "Make an image app from the archived root of a sandbox: inside it run `tar -C / -czf root.tar.gz ...`, then give the archive's place in your home. The assistant can do the same with the tool linuxapp_image. Nothing is made until you say Yes on the card.", Face::Sans, area.w) {
            g.text(area.x, y, 12.0 * s, MUTED, &l, Face::Sans);
            y += 16.0 * s;
        }
        y += 4.0 * s;
        if let Some((text, _)) = &self.linux.card {
            for l in text.lines() {
                for w in g.wrap(12.5 * s, l, Face::Sans, area.w) {
                    g.text(area.x, y, 12.5 * s, TEXT, &w, Face::Sans);
                    y += 16.0 * s;
                }
            }
            y += 6.0 * s;
            let x = area.x + g.button(area.x, y, size, "Yes, make it", ACCENT, Event::Tab(L_IMGYES)) + 10.0 * s;
            g.button(x, y, size, "No", MUTED, Event::Tab(L_IMGNO));
            return;
        }
        for (f, id, focus) in [(0, L_ARCHIVE, Focus::Archive), (1, L_NAME, Focus::Name), (2, L_PORT, Focus::Port), (3, L_LICENCE, Focus::Licence), (4, L_CMD, Focus::Cmd)] {
            let field = match f {
                0 => &mut self.linux.archive,
                1 => &mut self.linux.name,
                2 => &mut self.linux.port,
                3 => &mut self.linux.licence,
                _ => &mut self.linux.command,
            };
            field.render(g, R::new(area.x, y, area.w, field_h), self.linux.focus == focus, s, blink, Face::Mono, id);
            y += field_h + 6.0 * s;
        }
        g.button(area.x, y, size, "Make image app", ACCENT, Event::Tab(L_IMGMAKE));
    }

    fn draw_images(&mut self, g: &mut Gfx, area: R, s: f32) {
        let size = 14.0 * s;
        let bh = size * 2.1 + 6.0 * s;
        let Some(f) = self.linux.facts.clone() else { return };
        let sel = self.sel.min(f.images.len().saturating_sub(1));
        let chosen = f.images.get(sel).cloned();
        let labels: Vec<String> = vec!["Run".into(), "Stop".into()];
        let (pos, lines) = Self::flow(g, area.w, size, &labels);
        let desc: Vec<String> = chosen
            .as_ref()
            .map(|a| {
                let mut v: Vec<String> = g.wrap(12.0 * s, &a.description, Face::Sans, area.w).into_iter().take(3).collect();
                v.push(format!("Licence: {} \u{b7} {}", a.licence, if a.pinned { "image pinned by digest" } else { "digest not pinned yet: cannot run" }));
                if !a.status.is_empty() {
                    v.push(format!("State: {}", a.status));
                }
                v
            })
            .unwrap_or_default();
        let note = if f.cluster { "Runs in the dev cluster, namespace iyakki-apps. Put your own files in <data>/image-apps/ (docs/BUILD-YOUR-OWN-IMAGE-APP.md)." } else { "Needs the dev cluster: Store \u{2192} Setup \u{2192} k3s. Put your own files in <data>/image-apps/." };
        let note_lines = g.wrap(11.0 * s, note, Face::Sans, area.w);
        let prob_lines = f.problems.len().min(2);
        let fixed = lines * bh + desc.len() as f32 * 16.0 * s + note_lines.len().min(2) as f32 * 14.0 * s + prob_lines as f32 * 14.0 * s + 8.0 * s;
        let top_end = (area.y + area.h - fixed).max(area.y);
        let rh = 40.0 * s;
        let n = f.images.len().min(MAX_ROWS);
        let view = R::new(area.x - 4.0 * s, area.y, area.w + 8.0 * s, (top_end - area.y).max(rh));
        self.linux.layout_list(view, n, rh, s, sel);
        let offset = self.linux.list.offset;
        for (i, a) in f.images.iter().enumerate().take(MAX_ROWS) {
            let r = R::new(area.x - 4.0 * s, area.y + i as f32 * rh - offset, area.w + 8.0 * s, rh - 4.0 * s);
            if r.y + r.h < view.y || r.y > view.y + view.h {
                continue;
            }
            let hot = g.hovered(r);
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, 10.0 * s, i == sel, hot, ACCENT);
            let title = g.fit(14.0 * s, &a.name, Face::Bold, r.w - 24.0 * s);
            g.text(r.x + 12.0 * s, r.y + 3.0 * s, 14.0 * s, TEXT, &title, Face::Bold);
            let sub = g.fit(11.5 * s, &format!("{}{} \u{b7} {}", if a.example { "example \u{b7} " } else { "" }, a.licence, if a.status.is_empty() { "not running" } else { a.status.as_str() }), Face::Sans, r.w - 24.0 * s);
            g.text(r.x + 12.0 * s, r.y + 3.0 * s + g.line_height(14.0 * s, Face::Bold), 11.5 * s, MUTED, &sub, Face::Sans);
            g.hit(r, Event::Tab(L_IMG + i), &a.id);
        }
        self.linux.list.draw_bar(g, s, LINUX_BAR, ACCENT);
        let mut y = top_end;
        for (i, l) in desc.iter().enumerate() {
            let t = g.fit(12.0 * s, l, Face::Sans, area.w);
            g.text(area.x, y, 12.0 * s, if i + 1 == desc.len() && !chosen.as_ref().is_some_and(|a| a.pinned) { AMBER } else { MUTED }, &t, Face::Sans);
            y += 16.0 * s;
        }
        y += 4.0 * s;
        if chosen.is_some() {
            for ((l, id), p) in labels.iter().zip([L_RUN, L_STOP]).zip(&pos) {
                g.button(area.x + p.0, y + p.1 * bh, size, l, if id == L_RUN { ACCENT } else { ERR }, Event::Tab(id));
            }
        }
        y += lines * bh + 4.0 * s;
        for l in note_lines.iter().take(2) {
            g.text(area.x, y, 11.0 * s, DIM, l, Face::Sans);
            y += 14.0 * s;
        }
        for p in f.problems.iter().take(2) {
            let t = g.fit(11.0 * s, &format!("not listed: {p}"), Face::Sans, area.w);
            g.text(area.x, y, 11.0 * s, AMBER, &t, Face::Sans);
            y += 14.0 * s;
        }
    }
}

#[cfg(test)]
#[path = "brandmenu_linux_tests.rs"]
mod tests;
