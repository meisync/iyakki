//! The installer's pages as a state machine: Drive -> Mode -> (Choose / Sizes) -> Plan -> Confirm -> Progress -> Done.
//!
//! It holds no terminal and no window: the terminal installer ([`crate::tui`]) and the window installer (`iyakki-gui`) both drive
//! this one type and draw it, so the rules (which drives, which modes, what must be typed) exist once. Everything that decides is
//! in [`crate::planner`]; this file only walks a person through it. The Confirm page is a summary with an Install button (and, when
//! the plan erases data, a ticked box first): no typing (owner's decision, 2026-10-24; docs/INSTALLER.md rule 2).

use crate::planner::{build_plan, check_confirmation, Action, Confirmation, DiskModel, Mode, Part, Plan, Request, SourceSpec};
use crate::ui::{human_size, BOLD, CLEAR, DIM, GREEN, RED, RESET, REVERSE, YELLOW};
use crate::gpt_install::TableKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Drive,
    Mode,
    /// Pick free space (free-space and custom modes) or the BOOT and DATA partitions (reuse mode).
    Choose,
    /// Custom sizes.
    Sizes,
    Plan,
    Confirm,
    Progress,
    Done,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WKey {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Esc,
    Backspace,
    Tab,
    Char(char),
}

/// Terminal bytes to keys (arrows, Enter, Esc, Backspace, Tab, printable ASCII).
pub fn parse_wkeys(b: &[u8]) -> Vec<WKey> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            0x1b if matches!(b.get(i + 1), Some(b'[') | Some(b'O')) => {
                match b.get(i + 2) {
                    Some(b'A') => out.push(WKey::Up),
                    Some(b'B') => out.push(WKey::Down),
                    Some(b'C') => out.push(WKey::Right),
                    Some(b'D') => out.push(WKey::Left),
                    _ => {}
                }
                i += 3;
                continue;
            }
            0x1b | 0x03 => out.push(WKey::Esc),
            b'\r' | b'\n' => out.push(WKey::Enter),
            0x7f | 0x08 => out.push(WKey::Backspace),
            b'\t' => out.push(WKey::Tab),
            c if (0x20..0x7f).contains(&c) => out.push(WKey::Char(c as char)),
            _ => {}
        }
        i += 1;
    }
    out
}

/// What the caller must do after a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    Nothing,
    /// Leave the installer (nothing was written).
    Quit,
    /// The person confirmed: write the plan (the caller runs [`crate::apply::apply`] and then [`Wizard::finished`]).
    Write,
}

/// What has the keyboard focus on the Confirm page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CFocus {
    /// The "I understand that everything on this drive will be erased" box (only when the plan erases data).
    Ack,
    Install,
    Back,
    Cancel,
}

/// Everything the Confirm page shows, as plain data (the terminal and the window draw the same facts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmSummary {
    pub model: String,
    pub name: String,
    pub path: String,
    pub size: String,
    /// What will be created, kept and erased (partition names with what they are).
    pub create: Vec<String>,
    pub keep: Vec<String>,
    pub erase: Vec<String>,
    /// Visible warning lines: the running system's disk, another system on an internal disk, partitions unmounted first.
    pub warnings: Vec<String>,
    /// The plan erases data: the box must be ticked and the button says what it does.
    pub needs_ack: bool,
    pub button: String,
}

/// A row of the Choose page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Gap { first_lba: u64, bytes: u64 },
    Part { index: u32, text: String, bytes: u64, usable: bool },
}

pub struct Wizard {
    pub page: Page,
    pub disks: Vec<DiskModel>,
    pub source: SourceSpec,
    pub uefi: bool,
    pub disk_sel: usize,
    pub mode_sel: usize,
    pub row_sel: usize,
    pub req: Request,
    pub plan: Option<Plan>,
    pub error: Option<String>,
    /// The Confirm page: the ticked erase acknowledgement and the focused control.
    pub erase_ack: bool,
    pub cfocus: CFocus,
    /// The Sizes page: 0 boot, 1 data.
    pub focus: usize,
    pub sizes: [String; 2],
    pub progress: (String, u64, u64),
    pub result: String,
}

impl Wizard {
    pub fn new(disks: Vec<DiskModel>, source: SourceSpec, uefi: bool) -> Wizard {
        let disk_sel = disks.iter().position(|d| Wizard::disk_reason_of(d).is_none()).unwrap_or(0);
        Wizard { page: Page::Drive, disks, source, uefi, disk_sel, mode_sel: 1, row_sel: 0, req: Request::default(), plan: None, error: None, erase_ack: false, cfocus: CFocus::Install, focus: 0, sizes: [String::new(), String::new()], progress: (String::new(), 0, 0), result: String::new() }
    }

    /// Why a disk cannot be used in any mode.
    pub fn disk_reason_of(d: &DiskModel) -> Option<String> {
        if d.booted_from {
            return Some("the stick IYAKKI started from".into());
        }
        if d.read_only {
            return Some("write-protected".into());
        }
        if d.size_bytes == 0 {
            return Some("no media".into());
        }
        if !matches!(d.sector, 512 | 4096) {
            return Some(format!("{}-byte sectors are not supported", d.sector));
        }
        if d.unusual_gpt {
            return Some("unusual partition table layout".into());
        }
        None
    }

    pub fn disk(&self) -> &DiskModel {
        &self.disks[self.disk_sel]
    }

    pub fn mode(&self) -> Mode {
        Mode::ALL[self.mode_sel]
    }

    /// Can `mode` be planned on the chosen disk with its default choices? `Err` is the reason shown beside it.
    pub fn mode_available(&self, mode: Mode) -> Result<(), String> {
        let d = self.disk();
        let mut r = Request { mode, confirm_running_disk: self.req.confirm_running_disk || d.running_system, ..Request::default() };
        match mode {
            Mode::Reuse => {
                let usable = d.parts.iter().filter(|p| !p.extended).count();
                if d.table.is_none() {
                    return Err("the disk has no partitions".into());
                }
                if usable < 2 && !(usable >= 1 && d.parts.iter().any(|p| p.is_esp())) {
                    return Err("needs two partitions (one for BOOT, one for DATA)".into());
                }
                return Ok(());
            }
            Mode::Custom => r.mode = Mode::FreeSpace,
            _ => {}
        }
        build_plan(d, &r, &self.source, self.uefi).map(|_| ()).map_err(|e| e.replace(&d.path, "this disk"))
    }

    pub fn rows(&self) -> Vec<Row> {
        let d = self.disk();
        match self.mode() {
            Mode::Reuse => d
                .parts
                .iter()
                .map(|p| Row::Part { index: p.index, text: part_text(p, d.sector), bytes: p.bytes(d.sector), usable: !p.extended && p.in_use().is_none() })
                .collect(),
            _ => d.free_gaps(d.table.unwrap_or(TableKind::Gpt)).into_iter().map(|g| Row::Gap { first_lba: g.first_lba, bytes: g.bytes(d.sector) }).collect(),
        }
    }

    fn move_sel(sel: &mut usize, n: usize, up: bool) {
        if n == 0 {
            return;
        }
        *sel = if up { sel.saturating_sub(1) } else { (*sel + 1).min(n - 1) };
    }

    pub fn back(&mut self) -> Next {
        self.error = None;
        match self.page {
            Page::Drive => return Next::Quit,
            Page::Mode => self.page = Page::Drive,
            Page::Choose => self.page = Page::Mode,
            Page::Sizes => self.page = Page::Choose,
            Page::Plan => self.page = if matches!(self.mode(), Mode::WholeDisk) { Page::Mode } else { Page::Choose },
            Page::Confirm => self.page = Page::Plan,
            Page::Done | Page::Failed => return Next::Quit,
            Page::Progress => {}
        }
        Next::Nothing
    }

    pub fn start_mode(&mut self) {
        self.req = Request { mode: self.mode(), zram: self.req.zram.clone(), swapfile: self.req.swapfile.clone(), hostname: self.req.hostname.clone(), keep_esp: self.req.keep_esp, ..Request::default() };
        self.row_sel = 0;
    }

    /// Build the plan for the current choices and go to the Plan page; on refusal stay and show why.
    pub fn make_plan(&mut self) {
        let mut r = self.req.clone();
        r.mode = self.mode();
        // a disk that holds the running system is changed only after the extra warning on the Confirm page
        r.confirm_running_disk = r.confirm_running_disk || self.disk().running_system;
        match build_plan(self.disk(), &r, &self.source, self.uefi) {
            Ok(p) => {
                self.plan = Some(p);
                self.error = None;
                self.page = Page::Plan;
            }
            Err(e) => self.error = Some(e),
        }
    }

    pub fn handle(&mut self, k: WKey) -> Next {
        if k == WKey::Esc {
            return self.back();
        }
        match self.page {
            Page::Drive => match k {
                WKey::Up => Self::move_sel(&mut self.disk_sel, self.disks.len(), true),
                WKey::Down => Self::move_sel(&mut self.disk_sel, self.disks.len(), false),
                WKey::Enter => match self.disks.get(self.disk_sel).and_then(Wizard::disk_reason_of) {
                    Some(r) => self.error = Some(format!("That drive cannot be used: {r}.")),
                    None if self.disks.is_empty() => {}
                    None => {
                        self.error = None;
                        self.req = Request::default();
                        self.page = Page::Mode;
                    }
                },
                WKey::Char('q') => return Next::Quit,
                _ => {}
            },
            Page::Mode => match k {
                WKey::Up => Self::move_sel(&mut self.mode_sel, Mode::ALL.len(), true),
                WKey::Down => Self::move_sel(&mut self.mode_sel, Mode::ALL.len(), false),
                WKey::Char('e') => self.req.keep_esp = !self.req.keep_esp,
                WKey::Enter => match self.mode_available(self.mode()) {
                    Err(e) => self.error = Some(e),
                    Ok(()) => {
                        self.error = None;
                        self.start_mode();
                        if self.mode() == Mode::WholeDisk {
                            self.make_plan();
                        } else {
                            self.page = Page::Choose;
                        }
                    }
                },
                _ => {}
            },
            Page::Choose => {
                let n = self.rows().len();
                match k {
                    WKey::Up => Self::move_sel(&mut self.row_sel, n, true),
                    WKey::Down => Self::move_sel(&mut self.row_sel, n, false),
                    WKey::Char('e') if self.mode() == Mode::Reuse => self.req.keep_esp = !self.req.keep_esp,
                    WKey::Char('b') if self.mode() == Mode::Reuse => {
                        if let Some(Row::Part { index, .. }) = self.rows().get(self.row_sel) {
                            self.req.boot_part = if self.req.boot_part == Some(*index) { None } else { Some(*index) };
                        }
                    }
                    WKey::Char('d') if self.mode() == Mode::Reuse => {
                        if let Some(Row::Part { index, .. }) = self.rows().get(self.row_sel) {
                            self.req.data_part = if self.req.data_part == Some(*index) { None } else { Some(*index) };
                        }
                    }
                    WKey::Enter => match (self.mode(), self.rows().get(self.row_sel).cloned()) {
                        (Mode::Reuse, _) => self.make_plan(),
                        (Mode::Custom, Some(Row::Gap { first_lba, bytes })) => {
                            self.req.gap_start = Some(first_lba);
                            self.sizes = [(self.source.boot_required(self.disk().sector) / (1 << 20)).to_string(), ((bytes.saturating_sub(self.source.boot_required(self.disk().sector))) / (1 << 20)).to_string()];
                            self.focus = 0;
                            self.page = Page::Sizes;
                        }
                        (_, Some(Row::Gap { first_lba, .. })) => {
                            self.req.gap_start = Some(first_lba);
                            self.make_plan();
                        }
                        _ => self.error = Some("There is no free space on this disk to choose. Shrink a partition with the other system's disk tool first.".into()),
                    },
                    _ => {}
                }
            }
            Page::Sizes => match k {
                WKey::Tab | WKey::Up | WKey::Down => self.focus = 1 - self.focus,
                WKey::Backspace => {
                    self.sizes[self.focus].pop();
                }
                WKey::Char(c) if c.is_ascii_digit() => self.sizes[self.focus].push(c),
                WKey::Enter => {
                    let parse = |s: &str| s.trim().parse::<u64>().ok().filter(|v| *v > 0);
                    match (parse(&self.sizes[0]), parse(&self.sizes[1])) {
                        (Some(b), Some(d)) => {
                            self.req.boot_mib = if self.req.keep_esp { None } else { Some(b) };
                            self.req.data_mib = Some(d);
                            self.make_plan();
                        }
                        _ => self.error = Some("Type the sizes in MiB as whole numbers.".into()),
                    }
                }
                _ => {}
            },
            Page::Plan => {
                if k == WKey::Enter {
                    self.erase_ack = false;
                    self.cfocus = self.default_cfocus();
                    self.page = Page::Confirm;
                }
            }
            Page::Confirm => {
                let order = self.confirm_order();
                let at = order.iter().position(|f| *f == self.cfocus).unwrap_or(0);
                match k {
                    WKey::Tab | WKey::Down | WKey::Right => self.cfocus = order[(at + 1) % order.len()],
                    WKey::Up | WKey::Left => self.cfocus = order[(at + order.len() - 1) % order.len()],
                    WKey::Char(' ') if self.cfocus == CFocus::Ack => self.erase_ack = !self.erase_ack,
                    WKey::Enter => return self.activate(self.cfocus),
                    _ => {}
                }
            }
            Page::Progress => {}
            Page::Done | Page::Failed => {
                if k == WKey::Enter {
                    return Next::Quit;
                }
            }
        }
        Next::Nothing
    }

    /// The controls of the Confirm page in focus order: the box only when the plan erases data.
    pub fn confirm_order(&self) -> Vec<CFocus> {
        if self.plan.as_ref().is_some_and(Plan::erases_data) {
            vec![CFocus::Ack, CFocus::Install, CFocus::Back, CFocus::Cancel]
        } else {
            vec![CFocus::Install, CFocus::Back, CFocus::Cancel]
        }
    }

    /// Where the focus starts: on Install when nothing is erased, on Cancel when data is (Enter alone never erases).
    pub fn default_cfocus(&self) -> CFocus {
        if self.plan.as_ref().is_some_and(Plan::erases_data) { CFocus::Cancel } else { CFocus::Install }
    }

    /// May the Install button write? (An erasing plan needs the ticked box.)
    pub fn confirm_ok(&self) -> bool {
        self.plan.as_ref().is_some_and(|p| !p.erases_data() || self.erase_ack)
    }

    /// The proof for the plan file: this press of the Install button, and the box.
    pub fn confirmation(&self) -> Option<Confirmation> {
        self.plan.as_ref().filter(|_| self.confirm_ok()).map(|p| Confirmation::Button { drive: p.disk_name.clone(), size: p.disk_size, erase_ack: self.erase_ack })
    }

    /// A press (key or click) on a Confirm control.
    pub fn activate(&mut self, f: CFocus) -> Next {
        self.cfocus = f;
        match f {
            CFocus::Ack => {
                self.erase_ack = !self.erase_ack;
                Next::Nothing
            }
            CFocus::Install => {
                let ok = match (&self.plan, self.confirmation()) {
                    (Some(p), Some(c)) => check_confirmation(p, &c).is_ok(),
                    _ => false,
                };
                if ok {
                    self.error = None;
                    self.page = Page::Progress;
                    Next::Write
                } else {
                    self.error = Some("Tick the box \"I understand that everything on this drive will be erased\" first. Nothing was written.".into());
                    Next::Nothing
                }
            }
            CFocus::Back => self.back(),
            CFocus::Cancel => Next::Quit,
        }
    }

    /// The facts of the Confirm page (what is created, kept and erased, and the visible warnings).
    pub fn confirm_summary(&self) -> Option<ConfirmSummary> {
        let p = self.plan.as_ref()?;
        let d = self.disks.iter().find(|d| d.name == p.disk_name);
        let line = |s: &crate::planner::Step| format!("{} {} ({})", s.part, human_size(s.bytes), s.what);
        let by = |a: fn(&Action) -> bool| p.steps.iter().filter(|s| a(&s.action)).map(line).collect::<Vec<_>>();
        let create = by(|a| matches!(a, Action::Create));
        let keep = by(|a| matches!(a, Action::Keep | Action::AddFiles));
        let erase: Vec<String> = p.steps.iter().filter(|s| s.action.destructive()).map(|s| if s.loses.is_empty() { line(s) } else { format!("{} [loses: {}]", line(s), s.loses) }).collect();
        let mut warnings = Vec::new();
        if p.running_disk {
            warnings.push("This drive holds the system that is running now. Mounted partitions are never touched.".to_string());
        }
        if let Some(d) = d.filter(|d| d.internal()) {
            let os = other_systems(d);
            if !os.is_empty() {
                warnings.push(format!("This internal drive holds another system ({}). It is {}.", os.join(", "), if p.erases_data() { "affected by this plan: see the erase list" } else { "left as it is" }));
            }
        }
        if !p.unmount.is_empty() {
            warnings.push(format!("Mounted now, unmounted first: {}.", p.unmount.join(", ")));
        }
        let needs_ack = p.erases_data();
        let size = human_size(p.disk_size);
        let button = if needs_ack { format!("Erase {} ({}) and install", p.disk_name, size) } else { "Install".to_string() };
        Some(ConfirmSummary { model: p.disk_model.clone(), name: p.disk_name.clone(), path: p.disk_path.clone(), size, create, keep, erase, warnings, needs_ack, button })
    }

    pub fn finished(&mut self, r: Result<String, String>) {
        match r {
            Ok(s) => {
                self.result = s;
                self.page = Page::Done;
            }
            Err(e) => {
                self.result = e;
                self.page = Page::Failed;
            }
        }
    }

    // -----------------------------------------------------------------------------------------------------------------
    // drawing (plain strings with ANSI codes)

    pub fn render(&self, width: usize) -> String {
        let mut s = String::from(CLEAR);
        let title = |s: &mut String, n: usize, t: &str| s.push_str(&format!("{BOLD}IYAKKI installer{RESET}  {DIM}step {n} of 6: {t}{RESET}\n\n"));
        match self.page {
            Page::Drive => {
                title(&mut s, 1, "choose the drive");
                s.push_str(&format!("Source image: {} (boot volume {}).\n\n", self.source.path.display(), human_size(self.source.boot_bytes)));
                if self.disks.is_empty() {
                    s.push_str("No drives found.\n");
                }
                for (i, d) in self.disks.iter().enumerate() {
                    let kind = if d.usb { "USB" } else if d.removable { "removable" } else { "internal" };
                    let reason = Wizard::disk_reason_of(d).map(|r| format!("  {RED}not usable: {r}{RESET}")).unwrap_or_default();
                    let table = d.table.map_or("no table", |t| t.label());
                    let free: u64 = d.free_gaps(d.table.unwrap_or(TableKind::Gpt)).iter().map(|g| g.bytes(d.sector)).sum();
                    let line = format!("{:<12} {:>9}  {:<22} {:<8} {:<8} {} partition(s), {} free{reason}{}", d.path, human_size(d.size_bytes), d.model.chars().take(22).collect::<String>(), kind, table, d.parts.len(), human_size(free), if d.running_system { format!("  {YELLOW}holds the running system{RESET}") } else { String::new() });
                    if i == self.disk_sel { s.push_str(&format!("{REVERSE} > {line} {RESET}\n")) } else { s.push_str(&format!("   {line}\n")) }
                    if i == self.disk_sel {
                        for p in &d.parts {
                            s.push_str(&format!("      {DIM}{}{RESET}\n", part_text(p, d.sector)));
                        }
                    }
                }
                s.push_str(&format!("\n{DIM}Up/Down: move   Enter: choose   q: quit (nothing is written until the last page){RESET}\n"));
            }
            Page::Mode => {
                title(&mut s, 2, "what to do with the drive");
                s.push_str(&format!("{} ({})\n\n", self.disk().path, human_size(self.disk().size_bytes)));
                for (i, m) in Mode::ALL.iter().enumerate() {
                    let avail = self.mode_available(*m);
                    let note = avail.as_ref().err().map(|e| format!("  {RED}unavailable: {e}{RESET}")).unwrap_or_default();
                    let line = format!("{}{note}", m.title());
                    if i == self.mode_sel { s.push_str(&format!("{REVERSE} > {} {RESET}{note}\n", m.title())) } else { s.push_str(&format!("   {line}\n")) }
                }
                s.push_str(&format!("\n{DIM}Dual boot with another system: free-space or reuse mode, and e = keep the existing EFI System Partition ({}){RESET}\n", if self.req.keep_esp { "ON" } else { "off" }));
                s.push_str(&format!("{DIM}Up/Down: move   Enter: choose   Esc: back{RESET}\n"));
            }
            Page::Choose => {
                title(&mut s, 3, if self.mode() == Mode::Reuse { "choose the BOOT and DATA partitions" } else { "choose the free space" });
                let d = self.disk();
                if self.mode() == Mode::Reuse {
                    s.push_str("b = this partition becomes BOOT (FAT32, formatted)    d = DATA (formatted)    e = keep the existing EFI System Partition as BOOT\n\n");
                }
                for (i, r) in self.rows().iter().enumerate() {
                    let mark = |idx: u32| if self.req.boot_part == Some(idx) { format!("{GREEN}[BOOT]{RESET}") } else if self.req.data_part == Some(idx) { format!("{GREEN}[DATA]{RESET}") } else { "      ".to_string() };
                    let line = match r {
                        Row::Gap { first_lba, bytes } => format!("free space at {:>10}   {:>10}", human_size(first_lba * d.sector as u64), human_size(*bytes)),
                        Row::Part { index, text, usable, .. } => format!("{} {}{}", mark(*index), text, if *usable { String::new() } else { format!("  {RED}(in use or a container: cannot be chosen){RESET}") }),
                    };
                    if i == self.row_sel { s.push_str(&format!("{REVERSE} > {line} {RESET}\n")) } else { s.push_str(&format!("   {line}\n")) }
                }
                if self.rows().is_empty() {
                    s.push_str("   (nothing to choose)\n");
                }
                if self.mode() == Mode::Reuse {
                    s.push_str(&format!("\nkeep the existing EFI System Partition: {}\n", if self.req.keep_esp { "yes (only the folder EFI/IYAKKI and a boot entry are added)" } else { "no" }));
                }
                s.push_str(&format!("\n{DIM}Up/Down: move   Enter: {}   Esc: back{RESET}\n", if self.mode() == Mode::Reuse { "show the plan" } else { "use this free space" }));
            }
            Page::Sizes => {
                title(&mut s, 3, "sizes");
                s.push_str(&format!("{}Boot partition (MiB, image needs at least {}): {}{}\n", if self.focus == 0 { "> " } else { "  " }, self.source.boot_required(self.disk().sector) >> 20, self.sizes[0], if self.focus == 0 { "_" } else { "" }));
                s.push_str(&format!("{}Data partition (MiB):                          {}{}\n", if self.focus == 1 { "> " } else { "  " }, self.sizes[1], if self.focus == 1 { "_" } else { "" }));
                s.push_str(&format!("\n{DIM}Tab: next field   Enter: show the plan   Esc: back{RESET}\n"));
            }
            Page::Plan => {
                title(&mut s, 4, "the plan (nothing is written yet)");
                if let Some(p) = &self.plan {
                    for l in p.render().lines() {
                        let colored = if l.contains(" format ") || l.contains(" delete ") { format!("{RED}{l}{RESET}") } else if l.contains(" create ") || l.contains(" add files ") { format!("{GREEN}{l}{RESET}") } else { l.to_string() };
                        s.push_str(&colored);
                        s.push('\n');
                    }
                }
                s.push_str(&format!("\n{DIM}Enter: continue to the confirmation   Esc: back{RESET}\n"));
            }
            Page::Confirm => {
                title(&mut s, 5, "confirm");
                if let Some(c) = self.confirm_summary() {
                    s.push_str(&format!("{BOLD}{}  {}  {}  ({}){RESET}\n\n", c.name, c.size, c.model, c.path));
                    for (head, list, col) in [("Will be created", &c.create, GREEN), ("Will be KEPT", &c.keep, ""), ("Will be ERASED", &c.erase, RED)] {
                        s.push_str(&format!("{col}{BOLD}{head}{RESET}\n"));
                        if list.is_empty() {
                            s.push_str("   nothing\n");
                        }
                        for l in list {
                            s.push_str(&format!("{col}   {l}{RESET}\n"));
                        }
                        s.push('\n');
                    }
                    for w in &c.warnings {
                        s.push_str(&format!("{YELLOW}{BOLD}! {w}{RESET}\n"));
                    }
                    if !c.warnings.is_empty() {
                        s.push('\n');
                    }
                    let mark = |f: CFocus, text: String, col: &str| if self.cfocus == f { format!("{REVERSE}{col}{BOLD}  [ {text} ]  {RESET}") } else { format!("{col}  [ {text} ]  {RESET}") };
                    if c.needs_ack {
                        let box_line = format!("[{}] I understand that everything on this drive will be erased", if self.erase_ack { "x" } else { " " });
                        s.push_str(&if self.cfocus == CFocus::Ack { format!("{REVERSE}{RED}{BOLD}  {box_line}  {RESET}") } else { format!("{RED}  {box_line}  {RESET}") });
                        s.push_str("\n\n");
                    }
                    s.push_str(&mark(CFocus::Install, c.button.clone(), if c.needs_ack { RED } else { GREEN }));
                    s.push_str(&mark(CFocus::Back, "Back".into(), ""));
                    s.push_str(&mark(CFocus::Cancel, "Cancel".into(), ""));
                    s.push('\n');
                }
                s.push_str(&format!("\n{DIM}Tab / arrows: move   {}Enter: press the button   Esc: back (nothing is written until Install){RESET}\n", if self.plan.as_ref().is_some_and(Plan::erases_data) { "Space: tick the box   " } else { "" }));
            }
            Page::Progress => {
                title(&mut s, 6, "writing");
                let (label, done, total) = &self.progress;
                s.push_str(&format!("{}\n{}\n\nDo not remove the drive or switch off.\n", label, crate::ui::bar(width.saturating_sub(10).clamp(10, 60), if *total == 0 { 0.0 } else { *done as f64 / *total as f64 })));
            }
            Page::Done => {
                s.push_str(&format!("{BOLD}{GREEN}Done.{RESET}\n\n{}\n\n{DIM}Press Enter.{RESET}\n", self.result));
            }
            Page::Failed => {
                s.push_str(&format!("{BOLD}{RED}The installation stopped.{RESET}\n\n{}\n\n{DIM}Press Enter.{RESET}\n", self.result));
            }
        }
        if let Some(e) = &self.error {
            s.push_str(&format!("\n{RED}{e}{RESET}\n"));
        }
        s
    }
}

/// Names of the other operating systems recognised on a disk by filesystem signature.
pub fn other_systems(d: &DiskModel) -> Vec<&'static str> {
    let mut v = Vec::new();
    if d.parts.iter().any(|p| p.fs == "ntfs") {
        v.push("Windows (NTFS)");
    }
    if d.parts.iter().any(|p| p.fs == "apfs" || p.kind.contains("Apple")) {
        v.push("macOS (APFS)");
    }
    v
}

fn part_text(p: &Part, ss: u32) -> String {
    let mut s = format!("{:<12} {:>9}  {:<20}", p.name, human_size(p.bytes(ss)), p.kind);
    if !p.fs.is_empty() {
        s.push_str(&format!(" {}", p.fs));
    }
    if !p.label.is_empty() {
        s.push_str(&format!(" “{}”", p.label));
    }
    if let Some(m) = p.in_use() {
        s.push_str(&format!("  [{m}]"));
    }
    s
}

/// A one-line summary of what an action does, for lists.
pub fn action_help(a: Action) -> &'static str {
    match a {
        Action::Create => "a new partition in free space",
        Action::Format => "the partition is reformatted: its contents are lost",
        Action::Delete => "the partition is removed",
        Action::Keep => "left exactly as it is",
        Action::AddFiles => "files are added to an existing FAT volume",
    }
}
