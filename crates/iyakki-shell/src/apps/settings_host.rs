//! Settings → Host access: the folders of the host computer shared with this account (window mode), what each one allows, its
//! size and whether it can be reached, the actions on it, the audit trail of everything done with host files, and the
//! first-use explanation. Everywhere except the desktop window the page says that host files are not available and why.

use super::*;
use iyakki_agent::hostfiles as hf;

use crate::apps::hostfiles::{HostDialog, HostEnv, Out as HostOut};

/// What the page keeps between frames.
#[derive(Default)]
pub(super) struct HostPanel {
    dlg: Option<Box<HostDialog>>,
    /// Folder counts by path: `None` while being counted.
    stats: std::collections::HashMap<String, Option<hf::FolderStats>>,
    pending: Vec<(String, Receiver<hf::FolderStats>)>,
    /// Show the audit trail instead of the folders.
    trail: bool,
}

fn mb(b: u64) -> String {
    let m = b as f64 / (1 << 20) as f64;
    if m >= 1024.0 { format!("{:.1} GB", m / 1024.0) } else { format!("{m:.0} MB") }
}

impl SettingsApp {
    /// The shared folders (valid or not), for the list.
    pub(super) fn host_rows(cx: &AppCx) -> Vec<(hf::HostMount, bool)> {
        iyakki_agent::media::MediaStore::new(cx.services.dir(), &cx.session.user).host_mount_states()
    }

    fn host_env(cx: &AppCx) -> HostEnv {
        HostEnv::for_cx(cx)
    }

    /// Keys and clicks for a dialog opened from this page; true when one is open (it takes everything).
    /// A Host access dialog is open (it takes every key, click and wheel step).
    pub(super) fn host_dialog_open(&self) -> bool {
        self.host.dlg.is_some()
    }

    pub(super) fn host_dialog_event(&mut self, ev: &Event) -> bool {
        let Some(d) = self.host.dlg.as_mut() else { return false };
        match d.handle(ev) {
            HostOut::Close => self.host.dlg = None,
            HostOut::Stay => {}
            HostOut::Imported(n) => self.ok(format!("Imported {} file{}.", n.len(), if n.len() == 1 { "" } else { "s" })),
            HostOut::Linked(n) => self.ok(format!("host:{n} is shared read-only.")),
            HostOut::Unlinked(n) => self.ok(format!("host:{n} is no longer shared.")),
        }
        true
    }

    pub(super) fn handle_host(&mut self, k: &Key, cx: &AppCx) {
        if !hf::enabled() {
            return;
        }
        let rows = Self::host_rows(cx);
        let sel = self.sel[self.tab].min(rows.len().saturating_sub(1));
        let env = Self::host_env(cx);
        match k {
            Key::Char('a') => self.host.dlg = Some(Box::new(HostDialog::share(env, ACCENT))),
            Key::Char('i') => self.host.dlg = Some(Box::new(HostDialog::open(env, ACCENT))),
            Key::Char('l') => self.host.trail = !self.host.trail,
            Key::Char('m') => {
                if let Some((m, _)) = rows.get(sel) {
                    let mut store = hf::HostMounts::load(cx.services.dir(), &cx.session.user);
                    let to = !m.write;
                    match store.set_write(&m.name, to) {
                        Ok(()) => {
                            cx.audit.record(&cx.session.user, "host.mount.mode", &format!("host:{} {}", m.name, if to { "read-write" } else { "read-only" }));
                            self.ok(format!("host:{} is now {}.", m.name, if to { "read-write: changes are made on the computer" } else { "read-only" }));
                        }
                        Err(e) => self.fail(e),
                    }
                }
            }
            // convert to a copy, unlink, browse: the options of the chosen folder
            Key::Char('c') | Key::Char('u') | Key::Char('o') | Key::Enter => {
                if let Some((m, _)) = rows.get(sel) {
                    self.host.dlg = Some(Box::new(HostDialog::mount_menu(env, ACCENT, &m.name)));
                }
            }
            _ => {}
        }
    }

    pub(super) fn host_buttons() -> Vec<Btn> {
        vec![("Add folder…", ACCENT, 'a'), ("Options…", TEXT, 'o'), ("Change mode", TEXT, 'm'), ("Convert to copy", TEXT, 'c'), ("Unlink", ERR, 'u'), ("Import files…", TEXT, 'i'), ("Audit trail", TEXT, 'l')]
    }

    pub(super) fn render_host(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        if !hf::enabled() {
            let mut y = a.y + controls::section_title(g, a.x, a.y, s, "Not available here");
            for l in g.wrap(15.0 * s, hf::UNAVAILABLE, Face::Sans, a.w) {
                g.text(a.x, y, 15.0 * s, MUTED, &l, Face::Sans);
                y += 22.0 * s;
            }
            return;
        }
        // finish the folder counts that came back
        let mut done = Vec::new();
        for (i, (path, rx)) in self.host.pending.iter().enumerate() {
            if let Ok(st) = rx.try_recv() {
                self.host.stats.insert(path.clone(), Some(st));
                done.push(i);
            }
        }
        for i in done.into_iter().rev() {
            self.host.pending.remove(i);
        }
        let mut y = a.y + controls::section_title(g, a.x, a.y, s, "What you are allowing");
        let home = hf::host_home().map_or("the computer's home folder".to_string(), |h| h.display().to_string());
        for l in [
            format!("Host files lets IYAKKI use files from the computer this window runs on ({home}), only what you choose:"),
            "Import copies files into your account. A shared folder is read in place, read-only unless you turn writing on.".to_string(),
            "Hidden folders and everything outside your home folder stay out of reach. Stopping a share ends access at once.".to_string(),
        ] {
            for w in g.wrap(13.5 * s, &l, Face::Sans, a.w) {
                g.text(a.x, y, 13.5 * s, MUTED, &w, Face::Sans);
                y += 19.0 * s;
            }
        }
        y += 8.0 * s;
        if self.host.trail {
            y += controls::section_title(g, a.x, y, s, "Audit trail: host files");
            let lines: Vec<String> = cx.audit.recent(400).into_iter().filter(|l| l.contains(" host.")).take(40).collect();
            if lines.is_empty() {
                g.text(a.x, y, 14.0 * s, DIM, "Nothing yet.", Face::Sans);
            }
            for l in lines {
                if y > a.y + a.h - 20.0 * s {
                    break;
                }
                let t = g.fit(12.5 * s, &l, Face::Mono, a.w);
                g.text(a.x, y, 12.5 * s, TEXT, &t, Face::Mono);
                y += 18.0 * s;
            }
            return;
        }
        y += controls::section_title(g, a.x, y, s, "Shared folders");
        let rows = Self::host_rows(cx);
        if rows.is_empty() {
            g.text(a.x, y, 14.0 * s, DIM, "No folder is shared. Press a to share one (read-only), or i to import files.", Face::Sans);
            return;
        }
        let sel = self.sel[self.tab].min(rows.len() - 1);
        for (i, (m, ok)) in rows.iter().enumerate() {
            if y + 64.0 * s > a.y + a.h {
                break;
            }
            if *ok && !self.host.stats.contains_key(&m.path) {
                self.host.stats.insert(m.path.clone(), None);
                let (tx, rx) = channel();
                let p = std::path::PathBuf::from(&m.path);
                std::thread::spawn(move || {
                    let _ = tx.send(hf::folder_stats(&p));
                });
                self.host.pending.push((m.path.clone(), rx));
            }
            let size = match (ok, self.host.stats.get(&m.path)) {
                (false, _) => "folder not available".to_string(),
                (true, Some(Some(st))) => format!("{}{} files · {}", if st.capped { "over " } else { "" }, st.files, mb(st.bytes)),
                _ => "counting…".to_string(),
            };
            let mode = if m.write { "read-write" } else { "read-only" };
            let status = if *ok { "available" } else { "unplugged or moved" };
            let col = if !*ok { ERR } else if m.write { rgb(255, 196, 94) } else { OK };
            row(g, &mut self.tip, i, a.x, y, a.w, 58.0 * s, s, i == sel, &format!("host:{}", m.name), &format!("{} · {size}", m.path), &[(mode, col), (status, if *ok { OK } else { ERR })]);
            y += 66.0 * s;
        }
    }

    /// The dialog opened from this page, drawn over it.
    pub(super) fn render_host_dialog(&mut self, g: &mut Gfx, area: R, s: f32, tick: u64) {
        if let Some(d) = self.host.dlg.as_mut() {
            d.render(g, area, s, tick);
        }
    }
}
