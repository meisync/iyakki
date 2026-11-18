//! Settings → Memory: compressed memory (zram), compressor, swappiness, an optional swap file and zswap.
//!
//! The page shows the machine's memory figures (RAM, pressure, swap areas, zram size and compression ratio) and the settings
//! as they will be at the next boot. **Preview** says what applying would change; **Apply now** reconfigures the running
//! system (zram is taken out of use, reset and set up again; refused when what it holds would not fit back into free memory);
//! **Save for next boots** writes `/var/lib/iyakki/system.conf`. Settings fixed by a kernel boot option are shown as locked.
//! In a desktop window the page shows the host's state and changes nothing. Logic: the `iyakki-zram` crate.

use super::*;
use iyakki_core_api::memory::config::{self, human, Config, Resolved, ZramMode};
use iyakki_core_api::memory::{bootspeed::{self, BootSpeed}, compressed::{self, Experiment}, Memory, MemStats};

/// Which setting a form edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MemField {
    Zram,
    Algo,
    Swappiness,
    Swapfile,
}

pub(super) struct MemPanel {
    /// The machine's compressed memory, swap and `system.conf`: `cx.os.memory()`.
    pub(super) memory: std::sync::Arc<dyn Memory>,
    pub(super) cmdline: String,
    /// What the kernel command line and `system.conf` say now.
    pub(super) resolved: Resolved,
    /// What the page is editing.
    pub(super) draft: Config,
    /// The experimental compressed-memory switch as edited / as saved, and whether a boot option fixes it.
    pub(super) exp: Experiment,
    pub(super) exp_saved: Experiment,
    pub(super) exp_locked: bool,
    /// The boot-order record and prefetch switches (F-BOOT-30) as edited / as saved (with what a boot option fixes).
    pub(super) bs: BootSpeed,
    pub(super) bs_saved: BootSpeed,
    pub(super) stats: MemStats,
    pub(super) preview: Vec<String>,
    pub(super) report: crate::apps::ScrollLog,
    /// May this page change the machine (bare metal) or only look (a window on someone's computer)?
    pub(super) writable: bool,
    pub(super) ram: u64,
    loaded: bool,
}

impl MemPanel {
    pub(super) fn new(memory: std::sync::Arc<dyn Memory>) -> MemPanel {
        let cmdline = crate::host().cmdline();
        MemPanel { memory, cmdline, resolved: config::resolve("", ""), draft: Config::default(), exp: Experiment::Off, exp_saved: Experiment::Off, exp_locked: false, bs: BootSpeed::default(), bs_saved: BootSpeed::default(), stats: MemStats::default(), preview: Vec::new(), report: Default::default(), writable: !hf_on(), ram: 0, loaded: false }
    }
}

fn gib(b: u64) -> String {
    human(b)
}

/// The figures as lines for the page (also what Copy takes).
pub(super) fn report_lines(m: &MemStats) -> Vec<String> {
    let mut v = vec![format!("RAM        {} total, {} available, {} free", gib(m.total), gib(m.available), gib(m.free))];
    v.push(match m.pressure {
        Some([a, b, c]) => format!("Pressure   memory stall {a:.1}% (10 s)  {b:.1}% (1 min)  {c:.1}% (5 min)"),
        None => "Pressure   not available on this kernel".into(),
    });
    v.push(format!("Swap       {} used of {}", gib(m.swap_used), gib(m.swap_total)));
    for (path, used, size, prio) in &m.swaps {
        v.push(format!("  {path}  priority {prio}  {} of {}", gib(*used), gib(*size)));
    }
    match &m.zram {
        Some(z) => {
            v.push(format!("{:<10} {}  disk size {}  algorithms: {}", z.device, if z.algorithm.is_empty() { "?".into() } else { z.algorithm.clone() }, gib(z.disksize), z.available_algorithms.join(" ")));
            v.push(format!("           stores {} as {} (ratio {}), RAM used {}", gib(z.orig), gib(z.compressed), z.ratio().map_or("-".into(), |r| format!("{r:.1}:1")), gib(z.mem_used)));
        }
        None => v.push("zram       not present in this kernel".into()),
    }
    v
}

impl SettingsApp {
    /// Use another machine's files and `system.conf` (tests, and the window when it points at a fake root).
    pub fn with_memory(mut self, memory: std::sync::Arc<dyn Memory>, cmdline: &str, writable: bool) -> SettingsApp {
        self.mem = MemPanel { cmdline: cmdline.into(), writable, ..MemPanel::new(memory) };
        self.mem.ram = 0;
        self.mem_load(true);
        self
    }

    pub(super) fn mem_ram(&self) -> u64 {
        if self.mem.ram > 0 { self.mem.ram } else { self.mem.stats.total }
    }

    /// Read `system.conf`, the command line and the kernel's figures. `reset_draft`: discard unsaved edits.
    pub(super) fn mem_load(&mut self, reset_draft: bool) {
        let m = &mut self.mem;
        let conf = m.memory.read_conf();
        m.resolved = config::resolve(&m.cmdline, &conf);
        let (exp, locked) = compressed::resolve_experiment(&m.cmdline, &conf);
        (m.exp_saved, m.exp_locked) = (exp, locked);
        m.bs_saved = bootspeed::resolve(&m.cmdline, &conf);
        if reset_draft || !m.loaded {
            m.bs = m.bs_saved;
            m.exp = exp;
            m.draft = m.resolved.config.clone();
            m.preview.clear();
        }
        m.loaded = true;
        m.stats = m.memory.stats();
        m.report.lines = report_lines(&m.stats).into_iter().map(|l| (l, TEXT)).collect();
        m.report.nowrap = true;
    }

    fn mem_on_disk(&self) -> bool {
        self.mem.memory.data_on_disk(&self.storage.data_dir)
    }

    pub(super) fn mem_dirty(&self) -> bool {
        self.mem.draft != self.mem.resolved.config || self.mem.exp != self.mem.exp_saved || (self.mem.bs.record, self.mem.bs.prefetch) != (self.mem.bs_saved.record, self.mem.bs_saved.prefetch)
    }

    /// What applying would set up: the person's own choices, or, with the experiment on, the plan made for this machine (zswap and a small
    /// swap file when the data disk has room, zram otherwise).
    pub(super) fn mem_effective(&self) -> compressed::Plan {
        let free = if self.mem_on_disk() { self.mem.memory.data_free(&self.storage.data_dir) } else { None };
        compressed::plan(&self.mem.draft, self.mem.exp, free, self.mem_ram())
    }

    fn mem_locked(&self, f: MemField) -> Option<&'static str> {
        let r = &self.mem.resolved;
        match f {
            MemField::Zram | MemField::Algo if r.zram_locked => Some("iyakki.zram"),
            MemField::Swappiness if r.swappiness_locked => Some("iyakki.swappiness"),
            MemField::Swapfile if r.swapfile_locked => Some("iyakki.swapfile"),
            _ => None,
        }
    }

    /// The report area gets the wheel, clicks and drags (selection and copy).
    pub(super) fn memory_event(&mut self, ev: &Event) -> bool {
        if matches!(ev, Event::Tick) {
            self.mem.report.on_tick();
            return false;
        }
        self.mem.report.handle(ev)
    }

    pub(super) fn handle_memory(&mut self, k: &Key, cx: &AppCx) {
        if let Key::Char('r') = k {
            self.mem_load(false);
            return self.ok("Memory figures read again.");
        }
        if !self.admin(cx) {
            return;
        }
        if !self.mem.writable {
            return self.fail("This window shows the memory of the computer it runs on and changes nothing there. Memory settings are changed on the IYAKKI system itself.");
        }
        match k {
            Key::Char('z') => self.mem_open(MemField::Zram),
            Key::Char('c') => self.mem_open(MemField::Algo),
            Key::Char('w') => self.mem_open(MemField::Swappiness),
            Key::Char('f') => self.mem_open(MemField::Swapfile),
            Key::Char('x') => {
                if self.mem.resolved.zswap_locked {
                    return self.fail("Locked by the boot option iyakki.zswap.");
                }
                self.mem.draft.zswap = !self.mem.draft.zswap;
                self.mem.preview.clear();
                self.ok(format!("zswap {} (not applied yet).", if self.mem.draft.zswap { "on" } else { "off" }));
            }
            Key::Char('e') => {
                if self.mem.exp_locked {
                    return self.fail("Locked by the boot option iyakki.zram=exp or iyakki.compressed.");
                }
                self.mem.exp = if self.mem.exp == Experiment::On { Experiment::Off } else { Experiment::On };
                self.mem.preview.clear();
                self.ok(format!("Experimental compressed memory {} (not applied yet). {}", self.mem.exp.id(), self.mem_effective().reason));
            }
            Key::Char('o') | Key::Char('b') => {
                let record = matches!(k, Key::Char('o'));
                let (locked, opt, what) = if record { (self.mem.bs_saved.record_locked, "iyakki.bootrecord", "Recording the boot order") } else { (self.mem.bs_saved.prefetch_locked, "iyakki.prefetch", "Reading the boot files ahead") };
                if locked {
                    return self.fail(format!("Locked by the boot option {opt}."));
                }
                let on = if record { &mut self.mem.bs.record } else { &mut self.mem.bs.prefetch };
                *on = !*on;
                let now = *on;
                self.ok(format!("{what} {} (not saved yet). Developer option, used from the next start; it is not measured to make a start faster.", if now { "on" } else { "off" }));
            }
            Key::Char('p') => {
                self.mem.stats = self.mem.memory.stats();
                self.mem.preview = iyakki_core_api::memory::preview(&self.mem.stats, &self.mem_effective().config, self.mem_ram());
                self.ok(if self.mem_dirty() { "Preview below. Nothing has changed yet." } else { "No changes to apply; the preview shows the current setup." });
            }
            Key::Char('y') => {
                self.mem.stats = self.mem.memory.stats();
                if self.mem.stats.has_zram_swap() {
                    if let Err(why) = iyakki_core_api::memory::swapoff_safe(&self.mem.stats) {
                        return self.fail(why);
                    }
                }
                let lines = iyakki_core_api::memory::preview(&self.mem.stats, &self.mem_effective().config, self.mem_ram());
                self.mem.preview = lines.clone();
                self.modal = Some(Modal::Confirm(ConfirmKind::MemApply, format!("Apply to the running system now?\n\n{}\n\nThis does not change what happens at the next boot; use Save for that.", lines.join("\n"))));
            }
            Key::Char('v') => {
                let d = self.mem.draft.clone();
                let exp = self.mem.exp;
                let bs = self.mem.bs;
                let saved = self.mem.memory.save_conf(&d)
                    .and_then(|_| if self.mem.exp_locked { Ok(()) } else { self.mem.memory.set_conf_key("compressed-memory", Some(compressed::conf_value(exp))) })
                    .and_then(|_| if self.mem.bs_saved.record_locked { Ok(()) } else { self.mem.memory.set_conf_key(bootspeed::KEY_RECORD, Some(bootspeed::conf_value(bs.record))) })
                    .and_then(|_| if self.mem.bs_saved.prefetch_locked { Ok(()) } else { self.mem.memory.set_conf_key(bootspeed::KEY_PREFETCH, Some(bootspeed::conf_value(bs.prefetch))) });
                match saved {
                    Ok(()) => {
                        cx.audit.record(&cx.session.user, "memory.save", &format!("zram={} swappiness={} swapfile={:?} zswap={} compressed-memory={} boot-record={} boot-prefetch={}", d.zram_value(), d.swappiness, d.swapfile, d.zswap, exp.id(), bs.record, bs.prefetch));
                        self.mem_load(true);
                        let locked: Vec<&str> = [MemField::Zram, MemField::Swappiness, MemField::Swapfile].iter().filter_map(|f| self.mem_locked(*f)).collect();
                        self.ok(if locked.is_empty() { "Saved. These settings are used from the next boot.".to_string() } else { format!("Saved, but the boot option{} {} still override{} at boot.", if locked.len() > 1 { "s" } else { "" }, locked.join(", "), if locked.len() > 1 { "" } else { "s" }) });
                    }
                    Err(e) => self.fail(format!("Could not save: {e}")),
                }
            }
            _ => {}
        }
    }

    fn mem_open(&mut self, f: MemField) {
        if let Some(opt) = self.mem_locked(f) {
            return self.fail(format!("Locked by the boot option {opt}. Edit the boot entry to change it."));
        }
        let d = &self.mem.draft;
        let form = match f {
            MemField::Zram => Form::new("Compressed memory (zram)").text("auto, off, a size (4G) or a share of RAM (50%)", &d.zram_value().split(',').next().unwrap_or("auto").to_string(), "auto = half of RAM (all of it up to 2 GiB)"),
            MemField::Algo => Form::new("zram compressor").text("auto, zstd, lz4 or lzo-rle", d.algo.as_deref().unwrap_or("auto"), "auto = zstd, then lz4, then lzo-rle"),
            MemField::Swappiness => Form::new("Swappiness").text("0 to 200 (100 suits zram)", &d.swappiness.to_string(), "how eagerly memory is moved to swap"),
            MemField::Swapfile => Form::new("Swap file on the data disk").text("off, or a size such as 2G", &d.swapfile.map_or("off".to_string(), |b| human(b).replace(" GiB", "G").replace(" MiB", "M")), "written in full on the data disk; no hibernation"),
        };
        self.open_form(FormKind::Mem(f), form);
    }

    pub(super) fn mem_submit(&mut self, f: MemField, form: &Form) -> Result<String, String> {
        let v = form.value(0).trim().to_string();
        let d = &mut self.mem.draft;
        match f {
            MemField::Zram => {
                let (mode, size, algo) = config::parse_zram_value(&v).ok_or("enter auto, off, a size like 4G, or a percentage like 50%")?;
                (d.zram, d.size) = (mode, size);
                if algo.is_some() {
                    d.algo = algo;
                }
            }
            MemField::Algo => {
                d.algo = match v.to_ascii_lowercase().as_str() {
                    "" | "auto" => None,
                    a if ["zstd", "lz4", "lzo-rle", "lzo", "lz4hc", "deflate", "842"].contains(&a) => Some(a.to_string()),
                    _ => return Err("choose auto, zstd, lz4 or lzo-rle".into()),
                }
            }
            MemField::Swappiness => d.swappiness = v.parse::<u32>().ok().filter(|n| *n <= 200).ok_or("enter a number from 0 to 200")?,
            MemField::Swapfile => {
                d.swapfile = if matches!(v.as_str(), "off" | "0" | "") { None } else { Some(config::parse_bytes(&v).filter(|b| *b >= 64 << 20).ok_or("enter off, or a size of at least 64M such as 2G")?) }
            }
        }
        self.mem.preview.clear();
        Ok("Changed here only. Press Preview, then Apply now and/or Save for next boots.".into())
    }

    pub(super) fn mem_apply(&mut self, cx: &AppCx) {
        let (memory, cfg, ram) = (self.mem.memory.clone(), self.mem_effective().config, self.mem_ram());
        cx.audit.record(&cx.session.user, "memory.apply", &format!("zram={} swappiness={} swapfile={:?}", cfg.zram_value(), cfg.swappiness, cfg.swapfile));
        let on_disk = self.mem_on_disk();
        // the press that confirmed the change goes with the job: the job thread is not inside the dispatch
        let press = iyakki_core_api::guarded::gesture_ticket();
        self.start("Applying", move || {
            let work = move || match memory.reconfigure(&cfg, ram) {
                Err(e) => JobOut::Msg(Err(e)),
                Ok(mut steps) => {
                    steps.extend(memory.apply_swapfile(cfg.swapfile, on_disk));
                    let bad = steps.iter().filter(|s| !s.ok).count();
                    let text = steps.iter().map(|s| s.msg.clone()).collect::<Vec<_>>().join("; ");
                    if bad == 0 { JobOut::Msg(Ok(format!("Applied: {text}"))) } else { JobOut::Msg(Err(format!("Applied with problems: {text}"))) }
                }
            };
            match press {
                Some(p) => p.redeem(work),
                None => work(),
            }
        });
    }

    pub(super) fn mem_after_job(&mut self) {
        self.mem_load(false);
    }

    /// The settings rows as the page draws them: (label, value, the boot option that locks it).
    pub(super) fn mem_rows(&self) -> Vec<(&'static str, String, Option<&'static str>)> {
        vec![
            ("Compressed memory", self.mem.draft.zram_value().split(',').next().unwrap_or("auto").to_string() + &match self.mem.draft.zram {
                ZramMode::On => format!("   ({} at this RAM size)", human(config::disk_size(self.mem.draft.size, self.mem_ram()))),
                ZramMode::Off => String::new(),
            }, self.mem_locked(MemField::Zram)),
            ("Compressor", self.mem.draft.algo.clone().unwrap_or_else(|| "auto (zstd, then lz4, then lzo-rle)".into()), self.mem_locked(MemField::Algo)),
            ("Swappiness", self.mem.draft.swappiness.to_string(), self.mem_locked(MemField::Swappiness)),
            ("Swap file", self.mem.draft.swapfile.map_or("off".to_string(), |b| format!("{} on the data disk", human(b))), self.mem_locked(MemField::Swapfile)),
            ("zswap", if self.mem.draft.zswap { "on" } else { "off" }.to_string(), if self.mem.resolved.zswap_locked { Some("iyakki.zswap") } else { None }),
            ("Compressed memory, experimental", format!("{}   ({})", self.mem.exp.id(), self.mem_effective().reason), if self.mem.exp_locked { Some("iyakki.zram=exp") } else { None }),
            ("Record the boot order", if self.mem.bs.record { "on   (developer option: notes the files the first 90 seconds open)" } else { "off" }.to_string(), if self.mem.bs_saved.record_locked { Some("iyakki.bootrecord") } else { None }),
            ("Read boot files ahead", if self.mem.bs.prefetch { "on   (developer option: reads the recorded files into memory after the start; not measured)" } else { "off" }.to_string(), if self.mem.bs_saved.prefetch_locked { Some("iyakki.prefetch") } else { None }),
        ]
    }

    pub(super) fn render_memory(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let _ = cx;
        if !self.mem.loaded {
            self.mem_load(true);
        }
        let rows = self.mem_rows();
        let mut y = a.y;
        if !self.mem.writable {
            let note = "A desktop window shows the memory of the computer it runs on, read-only. Settings are changed on the IYAKKI system itself.";
            for l in g.wrap(13.0 * s, note, Face::Sans, a.w).iter().take(2) {
                g.text(a.x, y, 13.0 * s, ACCENT, l, Face::Sans);
                y += 18.0 * s;
            }
            y += 4.0 * s;
        }
        // Values sit in a column right of the labels; when a label is wider than its column, or the value would be cut short, the value goes under the label instead.
        let label_w = (170.0 * s).min(a.w * 0.4);
        for (label, value, lock) in &rows {
            g.text(a.x, y, 14.0 * s, MUTED, label, Face::Sans);
            let mut v = value.clone();
            if let Some(opt) = lock {
                v.push_str(&format!("   locked by boot option {opt}"));
            }
            let colour = if lock.is_some() { rgb(255, 196, 94) } else { TEXT };
            let label_fits = g.text_width(14.0 * s, label, Face::Sans) + 12.0 * s <= label_w;
            let value_fits = g.text_width(14.0 * s, &v, Face::Bold) <= a.w - label_w;
            if label_fits && value_fits {
                let t = g.fit(14.0 * s, &v, Face::Bold, a.w - label_w);
                g.text(a.x + label_w, y, 14.0 * s, colour, &t, Face::Bold);
            } else {
                y += 20.0 * s;
                let ind = 16.0 * s;
                let t = g.fit(14.0 * s, &v, Face::Bold, a.w - ind);
                g.text(a.x + ind, y, 14.0 * s, colour, &t, Face::Bold);
            }
            y += 24.0 * s;
        }
        if self.mem_dirty() {
            g.text(a.x, y, 13.0 * s, rgb(255, 196, 94), "Changed here, not applied or saved yet.", Face::Sans);
        } else {
            g.text(a.x, y, 13.0 * s, DIM, "Same as the saved settings.", Face::Sans);
        }
        y += 24.0 * s;
        let mut lines: Vec<String> = Vec::new();
        if !self.mem.preview.is_empty() {
            lines.push("If applied:".into());
            lines.extend(self.mem.preview.iter().map(|l| format!("  {l}")));
            lines.push(String::new());
        }
        lines.extend(report_lines(&self.mem.stats));
        self.mem.report.lines = lines.into_iter().map(|l| { let c = if l.starts_with("If applied") { ACCENT } else { TEXT }; (l, c) }).collect();
        let area = R::new(a.x, y, a.w, (a.y + a.h - y).max(0.0));
        if area.h > 40.0 * s {
            self.mem.report.render(g, area, 13.0 * s, Face::Mono);
        }
    }

    pub(super) fn memory_buttons(&self) -> Vec<Btn> {
        let (acc, mut_) = (ACCENT, TEXT);
        vec![("Zram", mut_, 'z'), ("Algorithm", mut_, 'c'), ("Swappiness", mut_, 'w'), ("Swap file", mut_, 'f'), ("zswap", mut_, 'x'), ("Experimental", mut_, 'e'), ("Record boot order", mut_, 'o'), ("Read ahead", mut_, 'b'), ("Preview", acc, 'p'), ("Apply now", acc, 'y'), ("Save for next boots", acc, 'v'), ("Read again", mut_, 'r')]
    }
}
