//! Settings → Install to disk: copy IYAKKI from the stick it is running from onto one of this computer's disks.
//!
//! Only on the real system, only for administrators, only when the system was started from an IYAKKI boot stick (there has to
//! be something to copy). The page lists the disks with model, size, serial and what is on them, says why a disk is refused,
//! and asks a few questions (zram, swap file, host name). The disk is erased only after the drive name and its size are typed
//! exactly. Logic and rules: the `install` module of `iyakki-usb` (the one `iyakki install` uses). The whole disk is erased;
//! installing next to another system is planned. The page has a "Partition table: Auto / GPT / MBR" button (`t`) with a one-line
//! explanation; the plan shows the table the disk has now and the layout it gets.

use super::*;
use iyakki_usb::install::{self, Choices, DiskInfo, SourceInfo, TableChoice};
use std::sync::Mutex;

/// Where the installer looks. Real paths in production, a made-up machine in tests.
#[derive(Clone)]
pub struct InstallEnv {
    pub sys: std::path::PathBuf,
    pub dev: std::path::PathBuf,
    pub mounts: std::path::PathBuf,
    pub swaps: std::path::PathBuf,
    /// Copy from this template file instead of the boot disk.
    pub template: Option<std::path::PathBuf>,
    /// This computer was started through UEFI (what "Auto" looks at, besides the disk).
    pub uefi: bool,
}

impl InstallEnv {
    pub fn real() -> InstallEnv {
        InstallEnv { sys: "/sys".into(), dev: "/dev".into(), mounts: "/proc/mounts".into(), swaps: "/proc/swaps".into(), template: None, uefi: install::booted_uefi() }
    }

    fn disks(&self) -> Vec<DiskInfo> {
        let rd = |p: &std::path::Path| std::fs::read_to_string(p).unwrap_or_default();
        let mut d = install::list_disks(&self.sys, &self.dev, &rd(&self.mounts), &rd(&self.swaps));
        install::probe_contents(&mut d, &self.dev);
        d
    }

    fn source(&self, disks: &[DiskInfo]) -> Result<SourceInfo, String> {
        match &self.template {
            Some(t) => install::read_source(t),
            None => install::find_source(disks, &self.dev, None),
        }
    }

    /// Is there anything to install from?
    pub fn available(&self) -> bool {
        self.source(&self.disks()).is_ok()
    }
}

pub(super) struct InstallPanel {
    pub(super) env: InstallEnv,
    pub(super) disks: Vec<DiskInfo>,
    pub(super) source: Option<Result<SourceInfo, String>>,
    /// The plan shown by Plan, one line per entry.
    pub(super) plan: crate::apps::ScrollLog,
    /// What the running installation is doing: (phase, done, total).
    pub(super) progress: Arc<Mutex<(String, u64, u64)>>,
    /// Partition table choice: Auto, GPT or MBR.
    pub(super) table: TableChoice,
}

impl Default for InstallPanel {
    fn default() -> InstallPanel {
        InstallPanel { env: InstallEnv::real(), disks: Vec::new(), source: None, plan: Default::default(), progress: Default::default(), table: TableChoice::Auto }
    }
}

fn human(b: u64) -> String {
    iyakki_usb::ui::human_size(b)
}

impl SettingsApp {
    /// Use another machine's disks (tests). The page appears when something can be installed from.
    pub fn with_install(mut self, env: InstallEnv) -> SettingsApp {
        self.inst.env = env;
        self.inst_refresh();
        if matches!(self.inst.source, Some(Ok(_))) && !self.tabs.contains(&Tab::Install) {
            let at = self.tabs.iter().position(|t| *t == Tab::Memory).map_or(self.tabs.len(), |i| i + 1);
            self.tabs.insert(at, Tab::Install);
        }
        self
    }

    pub(super) fn inst_refresh(&mut self) {
        let disks = self.inst.env.disks();
        self.inst.source = Some(self.inst.env.source(&disks));
        self.inst.disks = disks;
        let n = self.inst_len();
        let i = self.tabs.iter().position(|t| *t == Tab::Install).unwrap_or(0);
        self.sel[i] = self.sel[i].min(n.saturating_sub(1));
    }

    /// The disks that may be shown: everything except the one we copy from.
    pub(super) fn inst_candidates(&self) -> Vec<&DiskInfo> {
        let src = match &self.inst.source {
            Some(Ok(s)) => s.path.file_name().map(|n| n.to_os_string()),
            _ => None,
        };
        self.inst.disks.iter().filter(|d| src.as_deref() != Some(std::ffi::OsStr::new(&d.drive.name))).collect()
    }

    pub(super) fn inst_len(&self) -> usize {
        self.inst_candidates().len()
    }

    /// Why this disk may not be used, if it may not.
    pub(super) fn inst_refusal(&self, d: &DiskInfo) -> Option<String> {
        match &self.inst.source {
            Some(Ok(src)) => install::make_plan_with(&self.inst.disks, &d.drive.name, src, &Choices::default(), self.inst.table, self.inst.env.uefi).err().map(|e| e.replacen(&format!("{} cannot be used: ", d.drive.path), "", 1)),
            Some(Err(e)) => Some(e.clone()),
            None => Some("not looked yet".into()),
        }
    }

    pub(super) fn handle_install(&mut self, k: &Key, cx: &AppCx) {
        let n = self.inst_len();
        match k {
            Key::Up => self.move_sel(-1, n),
            Key::Down => self.move_sel(1, n),
            Key::Char('r') => {
                self.inst_refresh();
                self.ok("Disks looked at again.");
            }
            Key::Char('p') | Key::Char('i') if self.admin(cx) => {
                let Some(d) = self.inst_candidates().get(self.sel()).map(|d| (*d).clone()) else { return self.fail("No disk to use.") };
                let Some(Ok(src)) = self.inst.source.clone() else { return self.fail("Nothing to copy from: this system was not started from an IYAKKI stick made with iyakki-usb --install.") };
                let plan = match install::make_plan_with(&self.inst.disks, &d.drive.name, &src, &Choices::default(), self.inst.table, self.inst.env.uefi) {
                    Ok(p) => p,
                    Err(e) => return self.fail(e),
                };
                if *k == Key::Char('p') {
                    self.inst.plan.lines = plan.render().lines().map(|l| (l.to_string(), TEXT)).collect();
                    self.inst.plan.nowrap = true;
                    return self.ok("The plan is below. Nothing has been written.");
                }
                let form = Form::new(&format!("Install IYAKKI on {} and ERASE it ({} partition table)", d.drive.path, plan.table.label()))
                    .text("Compressed memory (zram): auto, off, 4G, 50%", "auto", "auto is recommended")
                    .text("Swap file on the disk: off or a size such as 2G", "off", "off unless you need more memory")
                    .text("Name of this computer (optional)", "", "letters, digits and -")
                    .text(&format!("Type the drive name ({}) to erase it", d.drive.name), "", "everything on the disk is lost")
                    .text(&format!("Type its size ({}) to confirm", human(d.drive.size_bytes)), "", "exactly as shown");
                self.open_form(FormKind::Install(d.drive.name.clone()), form);
            }
            Key::Char('t') => {
                self.inst.table = self.inst.table.next();
                self.inst.plan.lines.clear();
                let t = self.inst.table;
                self.ok(&format!("Partition table: {}. {}", t.label(), t.explain()));
            }
            Key::Char('b') if self.admin(cx) => {
                if cx.session.policy.power {
                    self.modal = Some(Modal::Confirm(ConfirmKind::Reboot, "Restart this machine now?".into()));
                } else {
                    self.fail("Your account may not restart the machine.");
                }
            }
            _ => {}
        }
    }

    pub(super) fn inst_submit(&mut self, cx: &AppCx, disk: &str, form: &Form) -> Result<String, String> {
        if !self.admin(cx) {
            return Err("Only administrators can install.".into());
        }
        let hostname = form.value(2).trim().to_string();
        let choices = Choices { zram: form.value(0).trim().to_string(), swapfile: form.value(1).trim().to_string(), hostname: (!hostname.is_empty()).then_some(hostname) };
        let Some(Ok(src)) = self.inst.source.clone() else { return Err("nothing to copy from".into()) };
        let plan = install::make_plan_with(&self.inst.disks, disk, &src, &choices, self.inst.table, self.inst.env.uefi)?;
        if !install::confirmation_ok(&form.value(3), &form.value(4), &plan.target.drive) {
            return Err(format!("Type the drive name ({}) and its size ({}) exactly as shown, or press Esc.", plan.target.drive.name, human(plan.target.drive.size_bytes)));
        }
        cx.audit.record(&cx.session.user, "install.disk", &format!("{} table={} zram={} swapfile={}", plan.target.drive.path, plan.table.label(), choices.zram, choices.swapfile));
        let mounts = self.inst.env.mounts.clone();
        let prog = self.inst.progress.clone();
        let path = plan.target.drive.path.clone();
        *prog.lock().unwrap() = ("Starting".into(), 0, 1);
        self.start("Installing", move || {
            let mounts_text = std::fs::read_to_string(&mounts).unwrap_or_default();
            let r = install::execute(&plan, false, &mounts_text, &mut |p| {
                *prog.lock().unwrap() = (p.phase.label().to_string(), p.done, p.total);
            });
            match r {
                Ok(_) => JobOut::Msg(Ok(format!("IYAKKI is installed on {path}. Remove the USB stick and restart; the first start asks you to create the first account."))),
                Err(e) => JobOut::Msg(Err(e)),
            }
        });
        Ok("The installation has started. Do not switch the computer off.".into())
    }

    pub(super) fn render_install(&mut self, g: &mut Gfx, a: R, s: f32) {
        let mut y = a.y;
        let warn = "This erases the whole disk you choose and puts IYAKKI on it. Other systems on it are lost. Take a backup first.";
        for l in g.wrap(13.0 * s, warn, Face::Bold, a.w).iter().take(2) {
            g.text(a.x, y, 13.0 * s, rgb(255, 196, 94), l, Face::Bold);
            y += 18.0 * s;
        }
        match &self.inst.source {
            Some(Ok(src)) => {
                let t = g.fit(13.0 * s, &format!("Copying from {} (the stick this system started from)", src.path.display()), Face::Sans, a.w);
                g.text(a.x, y, 13.0 * s, MUTED, &t, Face::Sans);
            }
            Some(Err(e)) => {
                for l in g.wrap(13.0 * s, e, Face::Sans, a.w).iter().take(2) {
                    g.text(a.x, y, 13.0 * s, ERR, l, Face::Sans);
                    y += 18.0 * s;
                }
            }
            None => {}
        }
        y += 20.0 * s;
        let line = format!("Partition table: {}. {}", self.inst.table.label(), self.inst.table.explain());
        for l in g.wrap(12.0 * s, &line, Face::Sans, a.w).iter().take(2) {
            g.text(a.x, y, 12.0 * s, MUTED, l, Face::Sans);
            y += 16.0 * s;
        }
        y += 10.0 * s;
        if self.is_busy() {
            let (phase, done, total) = self.inst.progress.lock().map(|p| p.clone()).unwrap_or_default();
            let pct = if total == 0 { 0 } else { done * 100 / total };
            g.text(a.x, y, 15.0 * s, ACCENT, &format!("{phase} {pct}%  (do not switch the computer off)"), Face::Bold);
            y += 26.0 * s;
        }
        let cands: Vec<DiskInfo> = self.inst_candidates().into_iter().cloned().collect();
        if cands.is_empty() {
            g.text(a.x, y, 15.0 * s, DIM, "No disks found. Press r to look again.", Face::Sans);
        }
        let row_h = 56.0 * s;
        let list_bottom = a.y + a.h * if self.inst.plan.lines.is_empty() { 1.0 } else { 0.6 };
        let sel = self.sel().min(cands.len().saturating_sub(1));
        let rows = (((list_bottom - y) / (row_h + 6.0 * s)) as usize).max(1);
        let first = sel.saturating_sub(rows - 1).min(cands.len().saturating_sub(rows));
        for (i, d) in cands.iter().enumerate().skip(first).take(rows) {
            let refusal = self.inst_refusal(d);
            let parts = d.partitions.iter().map(|p| format!("{} {}", p.0, if p.2.is_empty() { human(p.1) } else { p.2.clone() })).collect::<Vec<_>>().join(", ");
            let sub = format!("{}{} · {}", d.drive.model, if d.serial.is_empty() { String::new() } else { format!(" · {}", d.serial) }, refusal.clone().unwrap_or_else(|| if parts.is_empty() { "empty".into() } else { parts }));
            let size = human(d.drive.size_bytes);
            let mut chips: Vec<(&str, [u8; 4])> = vec![(size.as_str(), MUTED)];
            chips.push(if d.drive.usb || d.drive.removable { ("removable", ACCENT) } else { ("internal", MUTED) });
            if refusal.is_some() {
                chips.push(("refused", ERR));
            }
            row(g, &mut self.tip, i, a.x, y, a.w, row_h, s, i == sel, &d.drive.path, &sub, &chips);
            y += row_h + 6.0 * s;
        }
        if !self.inst.plan.lines.is_empty() {
            let area = R::new(a.x, y.max(list_bottom) + 4.0 * s, a.w, (a.y + a.h - y.max(list_bottom) - 4.0 * s).max(0.0));
            if area.h > 40.0 * s {
                self.inst.plan.render(g, area, 13.0 * s, Face::Mono);
            }
        }
    }

    pub(super) fn install_buttons(&self) -> Vec<Btn> {
        let table = match self.inst.table {
            TableChoice::Auto => "Table: Auto",
            TableChoice::Gpt => "Table: GPT",
            TableChoice::Mbr => "Table: MBR",
        };
        vec![("Install…", ERR, 'i'), ("Show the plan", ACCENT, 'p'), (table, TEXT, 't'), ("Look again", TEXT, 'r'), ("Restart", TEXT, 'b')]
    }
}
