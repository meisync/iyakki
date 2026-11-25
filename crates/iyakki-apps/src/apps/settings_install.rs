//! Settings → Install to disk: copy IYAKKI from the stick it is running from onto one of this computer's disks.
//!
//! Only on the real system, only for administrators, only when the system was started from an IYAKKI boot stick (there has to
//! be something to copy). The page lists the disks with model, size, serial and what is on them, says why a disk is refused,
//! and asks a few questions (zram, swap file, host name). The disk is erased only after the drive name and its size are typed
//! exactly. Logic and rules: the `install` module of `iyakki-usb` (the one `iyakki install` uses). The whole disk is erased;
//! installing next to another system is planned. The page has a "Partition table: Auto / GPT / MBR" button (`t`) with a one-line
//! explanation; the plan shows the table the disk has now and the layout it gets.

use super::super::settings_port::PagePort;
use super::system_port::RestartPort;
use super::*;
use iyakki_core_api::installer::{confirmation_ok, make_plan_with, Choices, DiskInfo, Installer, SourceInfo, TableChoice};
use std::sync::Mutex;

pub(super) struct InstallPanel {
    /// The machine side: disks, source, writing (the core's installer).
    pub(super) env: Arc<dyn Installer>,
    pub(super) disks: Vec<DiskInfo>,
    pub(super) source: Option<Result<SourceInfo, String>>,
    /// The plan shown by Plan, one line per entry.
    pub(super) plan: crate::apps::ScrollLog,
    /// What the running installation is doing: (phase, done, total).
    pub(super) progress: Arc<Mutex<(String, u64, u64)>>,
    /// Partition table choice: Auto, GPT or MBR.
    pub(super) table: TableChoice,
}

impl InstallPanel {
    pub(super) fn new(env: Arc<dyn Installer>) -> InstallPanel {
        InstallPanel { env, disks: Vec::new(), source: None, plan: Default::default(), progress: Default::default(), table: TableChoice::Auto }
    }
}

/// A disk size the way disk makers count it (powers of a thousand: a "64 GB" stick is 64.0 GB).
fn human(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let (mut v, mut i) = (b as f64, 0);
    while v >= 1000.0 && i < U.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", U[i]) }
}

#[cfg(test)]
#[test]
fn disk_sizes_are_counted_in_thousands_like_the_installer_program_does() {
    assert_eq!(human(999), "999 B");
    assert_eq!(human(64_000_000_000), "64.0 GB");
    assert_eq!(human(1_500_000), "1.5 MB");
    assert_eq!(human(1_000), "1.0 KB");
    assert_eq!(human(2_000_000_000_000), "2.0 TB");
}

impl SettingsApp {
    /// Use another machine's disks (tests). The page appears when something can be installed from.
    pub fn with_install(mut self, env: Arc<dyn Installer>) -> SettingsApp {
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
            Some(Ok(src)) => make_plan_with(&self.inst.disks, &d.drive.name, src, &Choices::default(), self.inst.table, self.inst.env.uefi()).err().map(|e| e.replacen(&format!("{} cannot be used: ", d.drive.path), "", 1)),
            Some(Err(e)) => Some(e.clone()),
            None => Some("not looked yet".into()),
        }
    }

    pub(super) fn handle_install(&mut self, k: &Key, p: &dyn RestartPort) {
        let n = self.inst_len();
        match k {
            Key::Up => self.move_sel(-1, n),
            Key::Down => self.move_sel(1, n),
            Key::Char('r') => {
                self.inst_refresh();
                self.ok("Disks looked at again.");
            }
            Key::Char('p') | Key::Char('i') if self.admin(p) => {
                let Some(d) = self.inst_candidates().get(self.sel()).map(|d| (*d).clone()) else { return self.fail("No disk to use.") };
                let Some(Ok(src)) = self.inst.source.clone() else { return self.fail("Nothing to copy from: this system was not started from an IYAKKI stick made with iyakki-usb --install.") };
                let plan = match make_plan_with(&self.inst.disks, &d.drive.name, &src, &Choices::default(), self.inst.table, self.inst.env.uefi()) {
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
            Key::Char('b') if self.admin(p) => {
                if p.allows_power() {
                    self.modal = Some(Modal::Confirm(ConfirmKind::Reboot, "Restart this machine now?".into()));
                } else {
                    self.fail("Your account may not restart the machine.");
                }
            }
            _ => {}
        }
    }

    pub(super) fn inst_submit(&mut self, p: &dyn PagePort, disk: &str, form: &Form) -> Result<String, String> {
        if !self.admin(p) {
            return Err("Only administrators can install.".into());
        }
        let hostname = form.value(2).trim().to_string();
        let choices = Choices { zram: form.value(0).trim().to_string(), swapfile: form.value(1).trim().to_string(), hostname: (!hostname.is_empty()).then_some(hostname) };
        let Some(Ok(src)) = self.inst.source.clone() else { return Err("nothing to copy from".into()) };
        let plan = make_plan_with(&self.inst.disks, disk, &src, &choices, self.inst.table, self.inst.env.uefi())?;
        if !confirmation_ok(&form.value(3), &form.value(4), &plan.target.drive) {
            return Err(format!("Type the drive name ({}) and its size ({}) exactly as shown, or press Esc.", plan.target.drive.name, human(plan.target.drive.size_bytes)));
        }
        p.audit("install.disk", &format!("{} table={} zram={} swapfile={}", plan.target.drive.path, plan.table.label(), choices.zram, choices.swapfile));
        let env = self.inst.env.clone();
        let prog = self.inst.progress.clone();
        let path = plan.target.drive.path.clone();
        *prog.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = ("Starting".into(), 0, 1);
        self.start("Installing", move || {
            let r = env.execute(&plan, &mut |p| {
                *prog.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = (p.phase.label().to_string(), p.done, p.total);
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
