//! DISKS: the drawing (left: the drives; right: the chosen drive with its partitions and actions; modal forms for the partition
//! manager). Every size follows the UI scale `s`; every computed clamp is `clamp_safe`; nothing is drawn or made clickable
//! outside the area it was given.

use iyakki_agent::disks::{Disk, Fs, Part, Table, Transport};
use iyakki_agent::disks_crypt::CryptKind;

use super::disks_tile::*;
use super::AppCx;
use crate::fx::{self, CYAN, WELL};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::safeclamp::SafeClamp;
use crate::ui::{Event, DIM, ERR, MUTED, OK, R, TEXT};

const WARN: [u8; 4] = rgb(255, 196, 94);

/// Where each part of the screen is, for the drawing and for the tests.
#[derive(Clone, Copy, Debug)]
pub(super) struct Lay {
    pub wide: bool,
    pub left: R,
    pub right: R,
    pub strip: R,
}

/// The colour of a file system's segment.
pub(super) fn fs_color(fs: Fs) -> [u8; 4] {
    match fs {
        Fs::Ext2 | Fs::Ext3 | Fs::Ext4 => rgb(110, 200, 140),
        Fs::Fat12 | Fs::Fat16 | Fs::Fat32 | Fs::Exfat => rgb(90, 160, 255),
        Fs::Ntfs => rgb(150, 120, 255),
        Fs::Btrfs | Fs::Xfs | Fs::F2fs => rgb(80, 210, 200),
        Fs::Iso9660 => rgb(140, 160, 200),
        Fs::Swap => WARN,
        Fs::Luks | Fs::BitLocker => rgb(255, 140, 100),
        Fs::Apfs | Fs::HfsPlus => rgb(255, 130, 180),
        Fs::Empty | Fs::Unknown => rgb(96, 106, 138),
    }
}

/// A small picture of the kind of drive: a stick for USB, a chip for NVMe, a flat drive for SATA, a card for SD, else the tile's own icon.
pub(super) fn drive_icon(g: &mut Gfx, t: Transport, cx: f32, cy: f32, size: f32, c: [u8; 4]) {
    let lw = (size * 0.075).max(1.5);
    match t {
        Transport::Usb => {
            g.rrect_outline(cx - size * 0.22, cy - size * 0.1, size * 0.44, size * 0.55, size * 0.08, lw, c);
            g.rect(cx - size * 0.13, cy - size * 0.4, size * 0.26, size * 0.3, c);
            g.rect(cx - size * 0.07, cy - size * 0.33, size * 0.05, size * 0.12, rgb(14, 18, 34));
            g.rect(cx + size * 0.02, cy - size * 0.33, size * 0.05, size * 0.12, rgb(14, 18, 34));
        }
        Transport::Nvme => {
            g.rrect_outline(cx - size * 0.3, cy - size * 0.22, size * 0.6, size * 0.44, size * 0.06, lw, c);
            for i in 0..3 {
                let x = cx - size * 0.16 + i as f32 * size * 0.16;
                g.rect(x - size * 0.02, cy - size * 0.34, size * 0.04, size * 0.12, c);
                g.rect(x - size * 0.02, cy + size * 0.22, size * 0.04, size * 0.12, c);
            }
            g.rect(cx - size * 0.12, cy - size * 0.08, size * 0.24, size * 0.16, crate::fx::with_alpha(c, 150));
        }
        Transport::Sata => {
            g.rrect_outline(cx - size * 0.38, cy - size * 0.22, size * 0.76, size * 0.44, size * 0.08, lw, c);
            g.rect(cx - size * 0.28, cy, size * 0.34, lw, c);
            g.circle(cx + size * 0.24, cy, size * 0.05, c);
        }
        Transport::Mmc => {
            g.rrect_outline(cx - size * 0.26, cy - size * 0.36, size * 0.52, size * 0.72, size * 0.06, lw, c);
            for i in 0..3 {
                g.rect(cx - size * 0.15 + i as f32 * size * 0.11, cy - size * 0.3, size * 0.06, size * 0.16, c);
            }
            g.rect(cx + size * 0.2, cy - size * 0.38, size * 0.1, size * 0.14, rgb(14, 18, 34));
        }
        Transport::Virtual | Transport::Other => g.icon(Icon::Disks, cx, cy, size, c),
    }
}

fn human(b: u64) -> String {
    iyakki_storage::human(b)
}

/// A line of the status of a partition, for the table.
fn part_status(p: &Part) -> (String, [u8; 4]) {
    if p.mounted() {
        let mp = p.mountpoints[0].clone();
        return match p.space {
            Some((free, total)) => (format!("{} free of {} · {mp}{}", human(free), human(total), if p.mounted_ro { " (read-only)" } else { "" }), OK),
            None => (format!("open at {mp}"), OK),
        };
    }
    match p.fs {
        Fs::Luks | Fs::BitLocker => match &p.crypt {
            Some(c) if c.problem.is_some() => (format!("{} (cannot be read: {})", c.headline(), c.problem.clone().unwrap_or_default()), rgb(255, 140, 100)),
            Some(c) if p.unlocked => (format!("{} - unlocked, not open", c.headline()), WARN),
            Some(c) => (format!("{} - locked: unlock needs the {}", c.headline(), c.kind.secret_word()), rgb(255, 140, 100)),
            None => ("Encrypted: unlock needs the key".into(), rgb(255, 140, 100)),
        },
        Fs::Apfs => ("Mac APFS: not readable here".into(), MUTED),
        Fs::Swap => (if p.swap_in_use { "swap, in use".into() } else { "swap space".into() }, MUTED),
        Fs::Empty => ("empty".into(), DIM),
        Fs::Unknown => ("unknown format".into(), DIM),
        _ => ("not open".into(), MUTED),
    }
}

impl DisksApp {
    pub(super) fn lay(&self, area: R, s: f32, strip_h: f32) -> Lay {
        let pad = 12.0 * s;
        let h = (area.h - strip_h - pad * 1.5).max(0.0);
        let top = area.y + pad * 0.6;
        let wide = area.w >= 620.0 * s;
        let strip = R::new(area.x + pad, area.y + area.h - strip_h.max(0.0), (area.w - 2.0 * pad).max(0.0), strip_h.max(0.0));
        if wide {
            let lw = (area.w * 0.34).clamp_safe(230.0 * s, 380.0 * s).min((area.w - 340.0 * s).max(160.0 * s));
            let left = R::new(area.x + pad, top, lw, h);
            let right = R::new(left.x + lw + pad, top, (area.w - lw - 3.0 * pad).max(0.0), h);
            Lay { wide, left, right, strip }
        } else {
            let full = R::new(area.x + pad, top, (area.w - 2.0 * pad).max(0.0), h);
            let none = R::new(area.x + pad, top, 0.0, 0.0);
            match self.page {
                Page::List => Lay { wide, left: full, right: none, strip },
                Page::Detail => Lay { wide, left: none, right: full, strip },
            }
        }
    }

    pub(super) fn draw(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        let _ = cx;
        self.derive();
        // the line at the bottom: the answer to the last action, or the job that is running
        let text_w = (area.w - 48.0 * s).max(40.0 * s);
        let (strip_lines, strip_ok, working) = match (&self.job, &self.msg) {
            (Some(j), _) => (vec![format!("{}{}", j.what, ".".repeat(1 + (tick / 10 % 3) as usize))], true, true),
            (None, Some((m, ok))) => (g.wrap(13.0 * s, m, Face::Sans, text_w).into_iter().take(3).collect::<Vec<_>>(), *ok, false),
            _ => (vec![], true, false),
        };
        let strip_h = if strip_lines.is_empty() { 0.0 } else { strip_lines.len() as f32 * 18.0 * s + 14.0 * s };
        let lay = self.lay(area, s, strip_h);
        if !strip_lines.is_empty() {
            let r = lay.strip;
            let c = if working { CYAN } else if strip_ok { OK } else { ERR };
            g.rrect(r.x, r.y, r.w, r.h, 10.0 * s, over_bg(c));
            g.rrect_outline(r.x, r.y, r.w, r.h, 10.0 * s, 1.0 * s, crate::fx::with_alpha(c, 150));
            g.circle(r.x + 14.0 * s, r.y + 17.0 * s, 4.0 * s, c);
            for (i, l) in strip_lines.iter().enumerate() {
                g.text(r.x + 28.0 * s, r.y + 8.0 * s + i as f32 * 18.0 * s, 13.0 * s, TEXT, l, Face::Sans);
            }
            if !working {
                g.hit(r, Event::Tab(T_DISMISS), "dismiss message");
            }
        }
        if self.who.role == iyakki_sec::policy::Role::Guest {
            fx::empty_state(g, area, s, Icon::Disks, "Drives are not available to guests", "Ask an administrator for an account that can use drives.", ACCENT);
            return;
        }
        if lay.left.w > 0.0 {
            self.draw_list(g, lay.left, s, tick);
        }
        if lay.right.w > 0.0 {
            self.draw_detail(g, lay.right, s, tick, lay.wide);
        }
        match self.modal.clone() {
            Modal::None => {}
            Modal::Form(f) => self.draw_form(g, area, s, &f, tick),
            Modal::Review(r) => self.draw_review(g, area, s, &r, tick),
            Modal::Unlock(f) => self.draw_unlock(g, area, s, &f, tick),
        }
    }

    // ----------------------------------------------------------------------------------------------------------------
    // the drives
    // ----------------------------------------------------------------------------------------------------------------

    fn draw_list(&mut self, g: &mut Gfx, r: R, s: f32, tick: u64) {
        fx::panel(g, r, s, ACCENT);
        let pad = 10.0 * s;
        // the filter
        let fh = 38.0 * s;
        let clear_w = if self.filter.value.is_empty() { 0.0 } else { 34.0 * s };
        let field = R::new(r.x + pad, r.y + pad, (r.w - 2.0 * pad - clear_w).max(40.0 * s), fh);
        self.filter.render(g, field, self.focus == Focus::Filter, s, super::blink(tick), Face::Sans, F_FILTER);
        if clear_w > 0.0 {
            let b = R::new(field.x + field.w + 4.0 * s, r.y + pad + 3.0 * s, 30.0 * s, 32.0 * s);
            let hot = g.hovered(b);
            let c = if hot { TEXT } else { MUTED };
            g.text(b.x + 9.0 * s, b.y + 6.0 * s, 17.0 * s, c, "×", Face::Bold);
            g.hit(b, Event::Tab(T_CLEAR_FILTER), "clear filter");
        }
        let view = R::new(r.x + pad * 0.6, r.y + pad * 2.0 + fh, (r.w - pad * 1.2).max(0.0), (r.h - fh - pad * 3.0).max(0.0));
        let vis = self.visible();
        let row_h = 66.0 * s;
        let gap = 6.0 * s;
        let content = vis.len() as f32 * (row_h + gap);
        self.list.layout(view, content, row_h + gap, s);
        if let Some(p) = vis.iter().position(|i| *i == self.sel) {
            let top = p as f32 * (row_h + gap);
            self.list.show(top, top + row_h);
        }
        if !self.loaded {
            fx::empty_state(g, view, s, Icon::Disks, "Looking for drives", "This takes a moment.", ACCENT);
            return;
        }
        if vis.is_empty() {
            let (t, h) = if self.snap.disks.is_empty() { ("No drives found", "Plug in a USB drive or an SD card; it appears here by itself.") } else { ("Nothing matches", "Clear the filter to see every drive.") };
            fx::empty_state(g, view, s, Icon::Disks, t, h, ACCENT);
            return;
        }
        let disks = self.snap.disks.clone();
        let (sel, offset) = (self.sel, self.list.offset);
        let who = self.who.clone();
        // the strip of the scroll bar is always kept free, so a list that starts to scroll does not move its rows
        let bar_w = 16.0 * s;
        g.clipped(view, |g| {
            for (k, i) in vis.iter().enumerate() {
                let Some(d) = disks.get(*i) else { continue };
                let y = view.y + k as f32 * (row_h + gap) - offset;
                if y + row_h < view.y || y > view.y + view.h {
                    continue;
                }
                let row = R::new(view.x, y, (view.w - bar_w).max(0.0), row_h);
                let hot = g.hovered(row);
                fx::list_row(g, row, s, *i == sel, hot, ACCENT);
                g.hit(row, Event::Row(*i), "drive");
                // the icon
                let tcol = if d.system { WARN } else { fx::mix(ACCENT, TEXT, 0.35) };
                drive_icon(g, d.transport, row.x + 28.0 * s, row.y + row.h * 0.5 - 4.0 * s, 28.0 * s, tcol);
                // eject on the right (only where it can be done)
                let eject_w = 34.0 * s;
                let mut right_edge = row.x + row.w - 10.0 * s;
                let can_eject = !d.system && iyakki_agent::disks::may_use(&who, d).is_ok();
                if can_eject {
                    let b = R::new(right_edge - eject_w, row.y + (row.h - eject_w) / 2.0, eject_w, eject_w);
                    let hot = g.hovered(b);
                    g.rrect(b.x, b.y, b.w, b.h, b.h / 2.0, if hot { rgb(60, 52, 40) } else { rgb(30, 28, 38) });
                    g.rrect_outline(b.x, b.y, b.w, b.h, b.h / 2.0, 1.0 * s, crate::fx::with_alpha(ACCENT, if hot { 220 } else { 110 }));
                    // the eject sign: a triangle over a bar
                    let (cx0, cy0, u) = (b.x + b.w / 2.0, b.y + b.h / 2.0, b.w * 0.2);
                    g.rect(cx0 - u, cy0 + u * 0.7, u * 2.0, u * 0.4, ACCENT);
                    for t in 0..4 {
                        let tt = t as f32 / 4.0;
                        g.rect(cx0 - u * (1.0 - tt), cy0 - u * 1.1 + tt * u * 1.7, u * 2.0 * (1.0 - tt), u * 0.45, ACCENT);
                    }
                    g.hit(b, Event::Tab(T_EJECT_ROW + *i), "eject");
                    right_edge = b.x - 6.0 * s;
                }
                if d.mounted() {
                    g.circle(right_edge - 6.0 * s, row.y + 16.0 * s, 4.0 * s, OK);
                    right_edge -= 16.0 * s;
                }
                let tx = row.x + 52.0 * s;
                let tw = (right_edge - tx - 4.0 * s).max(20.0 * s);
                let title = g.fit(15.0 * s, &d.title(), Face::Bold, tw);
                g.text(tx, row.y + 8.0 * s, 15.0 * s, TEXT, &title, Face::Bold);
                let sub = format!("{} · {}{}", human(d.size_bytes), d.transport.name(), if d.system { " · SYSTEM" } else { "" });
                let sub = g.fit(12.0 * s, &sub, Face::Sans, tw);
                g.text(tx, row.y + 28.0 * s, 12.0 * s, if d.system { WARN } else { MUTED }, &sub, Face::Sans);
                // how full the open partitions are
                let (used, total) = d.parts.iter().filter_map(|p| p.space).fold((0u64, 0u64), |(u, t), (free, tot)| (u + tot.saturating_sub(free), t + tot));
                let bar = R::new(tx, row.y + row.h - 15.0 * s, tw, 5.0 * s);
                let frac = if total > 0 { used as f32 / total as f32 } else { 0.0 };
                fx::progress_bar(g, bar, frac, if frac > 0.9 { ERR } else { ACCENT }, s);
            }
        });
        self.list.draw_bar(g, s, A_LIST, ACCENT);
    }

    // ----------------------------------------------------------------------------------------------------------------
    // the chosen drive
    // ----------------------------------------------------------------------------------------------------------------

    fn draw_detail(&mut self, g: &mut Gfx, r: R, s: f32, tick: u64, wide: bool) {
        let _ = tick;
        fx::panel(g, r, s, ACCENT);
        let view = R::new(r.x + 4.0 * s, r.y + 4.0 * s, (r.w - 8.0 * s).max(0.0), (r.h - 8.0 * s).max(0.0));
        let Some(d) = self.disk().cloned() else {
            let t = if self.loaded { "Choose a drive" } else { "Looking for drives" };
            fx::empty_state(g, view, s, Icon::Disks, t, "Its partitions, free space and actions appear here.", ACCENT);
            return;
        };
        self.detail.layout(view, self.detail_h, 40.0 * s, s);
        let y0 = view.y - self.detail.offset;
        let bar_w = 16.0 * s;
        let inner = R::new(view.x + 14.0 * s, y0, (view.w - 28.0 * s - bar_w).max(40.0 * s), view.h);
        let mut y = y0 + 12.0 * s;
        let manager = self.manager;
        let this: &mut DisksApp = self;
        g.clipped(view, |g| {
            y = this.detail_body(g, inner, y, s, &d, wide, manager);
        });
        let h = (y - y0) + 14.0 * s;
        if (h - self.detail_h).abs() > 0.5 {
            self.detail_h = h;
            self.detail.layout(view, h, 40.0 * s, s);
        }
        self.detail.draw_bar(g, s, A_DETAIL, ACCENT);
    }

    #[allow(clippy::too_many_arguments)]
    fn detail_body(&mut self, g: &mut Gfx, inner: R, mut y: f32, s: f32, d: &Disk, wide: bool, manager: bool) -> f32 {
        let x = inner.x;
        let w = inner.w;
        if !wide {
            let bw = g.text_width(14.0 * s, "‹ Drives", Face::Bold) + 14.0 * s * 1.7;
            g.button(x, y, 14.0 * s, "‹ Drives", MUTED, Event::Tab(T_BACK));
            y += 14.0 * s * 2.1 + 10.0 * s;
            let _ = bw;
        }
        // the heading
        drive_icon(g, d.transport, x + 22.0 * s, y + 24.0 * s, 40.0 * s, if d.system { WARN } else { ACCENT });
        let tx = x + 58.0 * s;
        let tw = (w - 58.0 * s).max(40.0 * s);
        let title = g.fit(21.0 * s, &d.title(), Face::Bold, tw);
        g.text(tx, y, 21.0 * s, TEXT, &title, Face::Bold);
        let sub = format!("{} · {} · {} · {}", d.name, human(d.size_bytes), d.transport.name(), d.table.name());
        let sub = g.fit(13.0 * s, &sub, Face::Sans, tw);
        g.text(tx, y + 28.0 * s, 13.0 * s, MUTED, &sub, Face::Sans);
        let ser = if d.serial.is_empty() { "no serial number".to_string() } else { format!("serial {}", d.serial) };
        let ser = g.fit(13.0 * s, &ser, Face::Mono, tw);
        g.text(tx, y + 46.0 * s, 13.0 * s, DIM, &ser, Face::Mono);
        y += 70.0 * s;
        // badges
        let mut bx = x;
        let mut chips: Vec<(&str, [u8; 4], bool)> = Vec::new();
        if d.system {
            chips.push(("SYSTEM DRIVE", WARN, true));
        }
        if d.read_only {
            chips.push(("WRITE-PROTECTED", ERR, true));
        }
        chips.push((if d.removable_media() { "REMOVABLE" } else { "INTERNAL" }, CYAN, false));
        if !d.holders.is_empty() {
            chips.push(("IN USE BY ANOTHER DRIVER", WARN, false));
        }
        for (t, c, f) in chips {
            let cw = g.text_width(12.0 * s, t, Face::Bold) + 12.0 * s * 1.4;
            if bx + cw > x + w && bx > x {
                bx = x;
                y += 26.0 * s;
            }
            bx += fx::chip(g, bx, y, s, t, c, f) + 8.0 * s;
        }
        y += 28.0 * s;
        if d.system {
            let t = format!("{} {}. It cannot be unmounted, ejected or changed.", d.name, d.system_why);
            for l in g.wrap(13.0 * s, &t, Face::Sans, w).into_iter().take(3) {
                g.text(x, y, 13.0 * s, WARN, &l, Face::Sans);
                y += 18.0 * s;
            }
            y += 4.0 * s;
        }
        for n in &d.table_notes {
            for l in g.wrap(12.0 * s, n, Face::Sans, w).into_iter().take(2) {
                g.text(x, y, 12.0 * s, WARN, &l, Face::Sans);
                y += 17.0 * s;
            }
        }
        // the layout bar and the table
        y += fx::section_title(g, x, y, w, s, "Partitions", ACCENT);
        y = self.draw_bar(g, x, y, w, s, d);
        y += 10.0 * s;
        y = self.draw_parts(g, x, y, w, s, d);
        y += 8.0 * s;
        if manager {
            y = self.draw_manager(g, x, y, w, s, d);
        } else {
            y = self.draw_actions(g, x, y, w, s, d);
        }
        y
    }

    /// The layout as a bar: one segment per partition, as wide as the partition is large; the rest of the drive is free space.
    fn draw_bar(&mut self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, d: &Disk) -> f32 {
        let h = 26.0 * s;
        g.rrect(x, y, w, h, 7.0 * s, WELL);
        if d.parts.is_empty() {
            g.text(x + 12.0 * s, y + (h - g.line_height(12.0 * s, Face::Sans)) / 2.0, 12.0 * s, DIM, "no partitions", Face::Sans);
            return y + h;
        }
        let total = d.size_bytes.max(1) as f32;
        let min = 8.0 * s;
        let mut widths: Vec<f32> = d.parts.iter().map(|p| (w * p.size_bytes as f32 / total).max(min)).collect();
        let sum: f32 = widths.iter().sum();
        if sum > w {
            let k = w / sum;
            widths.iter_mut().for_each(|v| *v *= k);
        }
        let mut sx = x;
        for (j, (p, pw)) in d.parts.iter().zip(widths).enumerate() {
            let seg = R::new(sx + 1.0 * s, y + 2.0 * s, (pw - 2.0 * s).max(1.0), h - 4.0 * s);
            let c = fs_color(p.fs);
            let selected = j == self.psel;
            let hot = g.hovered(seg);
            g.rrect(seg.x, seg.y, seg.w, seg.h, 5.0 * s, crate::fx::with_alpha(c, if p.mounted() { 230 } else if hot { 170 } else { 120 }));
            if selected {
                g.rrect_outline(seg.x - 1.0, seg.y - 1.0, seg.w + 2.0, seg.h + 2.0, 6.0 * s, 1.6 * s, TEXT);
            }
            if seg.w > 34.0 * s {
                let t = g.fit(11.0 * s, &if p.label.is_empty() { p.name.clone() } else { p.label.clone() }, Face::Bold, seg.w - 8.0 * s);
                g.text(seg.x + 5.0 * s, seg.y + (seg.h - g.line_height(11.0 * s, Face::Bold)) / 2.0, 11.0 * s, rgb(10, 14, 28), &t, Face::Bold);
            }
            g.hit(seg, Event::Tab(T_PART + j), "partition");
            sx += pw;
        }
        y + h
    }

    fn draw_parts(&mut self, g: &mut Gfx, x: f32, mut y: f32, w: f32, s: f32, d: &Disk) -> f32 {
        let rh = 50.0 * s;
        for (j, p) in d.parts.iter().enumerate() {
            let row = R::new(x, y, w, rh);
            let hot = g.hovered(row);
            let selected = j == self.psel;
            fx::list_row(g, row, s, selected && self.focus != Focus::Drives || selected, hot, fs_color(p.fs));
            g.hit(row, Event::Tab(T_PART + j), "partition row");
            g.circle(row.x + 16.0 * s, row.y + rh / 2.0, 5.0 * s, if p.mounted() { OK } else { rgb(70, 78, 104) });
            let tx = row.x + 32.0 * s;
            let right_w = (w * 0.46).max(110.0 * s);
            let left_w = (w - 44.0 * s - right_w).max(60.0 * s);
            let title = g.fit(14.0 * s, &if p.number == 0 { p.title() } else { format!("{}  {}", p.number, p.title()) }, Face::Bold, left_w);
            g.text(tx, row.y + 8.0 * s, 14.0 * s, TEXT, &title, Face::Bold);
            let kind = if p.type_name.is_empty() { human(p.size_bytes) } else { format!("{} · {}", human(p.size_bytes), p.type_name) };
            let kind = g.fit(12.0 * s, &kind, Face::Sans, left_w);
            g.text(tx, row.y + 28.0 * s, 12.0 * s, MUTED, &kind, Face::Sans);
            let (st, c) = part_status(p);
            let st = g.fit(12.0 * s, &st, Face::Sans, right_w);
            let sw = g.text_width(12.0 * s, &st, Face::Sans);
            g.text(row.x + row.w - sw - 12.0 * s, row.y + 8.0 * s, 12.0 * s, c, &st, Face::Sans);
            if let Some((free, total)) = p.space {
                let frac = if total > 0 { total.saturating_sub(free) as f32 / total as f32 } else { 0.0 };
                fx::progress_bar(g, R::new(row.x + row.w - right_w.min(row.w * 0.4) - 12.0 * s, row.y + rh - 14.0 * s, right_w.min(row.w * 0.4), 5.0 * s), frac, if frac > 0.9 { ERR } else { fs_color(p.fs) }, s);
            }
            y += rh + 6.0 * s;
        }
        y
    }

    /// Buttons that wrap: (label, click, enabled, accent). Returns the y below the last row.
    fn button_row(g: &mut Gfx, x: f32, mut y: f32, w: f32, s: f32, items: &[(String, Event, bool, [u8; 4])]) -> f32 {
        let size = 14.0 * s;
        let h = size * 2.1;
        let mut bx = x;
        for (label, ev, on, accent) in items {
            let bw = g.text_width(size, label, Face::Bold) + size * 1.7;
            if bx + bw > x + w && bx > x {
                bx = x;
                y += h + 8.0 * s;
            }
            if *on {
                g.button(bx, y, size, label, *accent, ev.clone());
            } else {
                g.button_off(bx, y, size, label);
            }
            bx += bw + 8.0 * s;
        }
        y + h
    }

    fn draw_actions(&mut self, g: &mut Gfx, x: f32, mut y: f32, w: f32, s: f32, d: &Disk) -> f32 {
        y += fx::section_title(g, x, y, w, s, "Actions", ACCENT);
        let can_mount = self.plan.is_ok();
        let mounted = self.part().is_some_and(Part::mounted);
        let items = vec![
            ("Mount".to_string(), Event::Tab(T_MOUNT), can_mount, ACCENT),
            ("Unmount".to_string(), Event::Tab(T_UNMOUNT), self.can_unmount(), ACCENT),
            ("Open".to_string(), Event::Tab(T_OPEN), mounted, CYAN),
            ("Eject".to_string(), Event::Tab(T_EJECT), self.can_eject(), ACCENT),
            ("Properties".to_string(), Event::Tab(T_PROPS), self.part().is_some(), MUTED),
            ("Refresh".to_string(), Event::Tab(T_REFRESH), true, MUTED),
        ];
        let mut items = items;
        if self.part().is_some_and(|p| p.crypt.is_some()) {
            let unlocked = self.part().is_some_and(|p| p.unlocked);
            items.insert(0, ("Unlock…".to_string(), Event::Tab(T_UNLOCK), self.unlock_plan.is_ok(), WARN));
            items.insert(1, ("Allow writing".to_string(), Event::Tab(T_ALLOW_WRITE), self.can_allow_writing(), WARN));
            items.insert(2, ("Lock".to_string(), Event::Tab(T_LOCK), unlocked && self.can_lock(), ACCENT));
        }
        y = Self::button_row(g, x, y, w, s, &items) + 10.0 * s;
        // the switches
        let mut bx = x;
        let ro = fx::toggle_chip(g, bx, y, s, "Read-only", self.read_only, ACCENT);
        g.hit(R::new(bx, y, ro, 34.0 * s), Event::Tab(T_RO), "read-only switch");
        bx += ro + 8.0 * s;
        if self.is_admin() {
            let label = "Allow running programs";
            let ew = g.text_width(15.0 * s, label, Face::Bold) + 28.0 * s;
            if bx + ew > x + w && bx > x {
                bx = x;
                y += 42.0 * s;
            }
            let e = fx::toggle_chip(g, bx, y, s, label, self.allow_exec, WARN);
            g.hit(R::new(bx, y, e, 34.0 * s), Event::Tab(T_EXEC), "allow running programs switch");
        }
        y += 42.0 * s;
        // why a button is grey
        let why = match (&self.plan, self.part()) {
            // an encrypted partition: say what unlocking needs (the helper program, a damaged header) instead of "choose Unlock"
            (_, Some(p)) if p.crypt.is_some() && !p.unlocked => match &self.unlock_plan {
                Err(e) if !e.is_empty() => Some(e.clone()),
                Ok(k) => Some(format!("Encrypted ({}): choose Unlock and type the {}. It opens read-only first; Allow writing is a separate step.", k.name(), k.secret_word())),
                _ => None,
            },
            (Err(e), Some(_)) if !e.is_empty() => Some(e.clone()),
            _ => None,
        };
        if let Some(t) = why {
            for l in g.wrap(13.0 * s, &t, Face::Sans, w).into_iter().take(4) {
                g.text(x, y, 13.0 * s, MUTED, &l, Face::Sans);
                y += 18.0 * s;
            }
            y += 4.0 * s;
        } else if let Err(e) = iyakki_agent::disks::may_use(&self.who, d) {
            for l in g.wrap(13.0 * s, &e, Face::Sans, w).into_iter().take(3) {
                g.text(x, y, 13.0 * s, MUTED, &l, Face::Sans);
                y += 18.0 * s;
            }
        }
        if self.is_admin() && !d.system {
            y += 4.0 * s;
            let label = "Advanced: change partitions…";
            g.button(x, y, 14.0 * s, label, WARN, Event::Tab(T_ADV));
            y += 14.0 * s * 2.1 + 8.0 * s;
        }
        if self.props {
            y = self.draw_props(g, x, y, w, s, d);
        }
        y
    }

    fn draw_props(&mut self, g: &mut Gfx, x: f32, mut y: f32, w: f32, s: f32, d: &Disk) -> f32 {
        y += fx::section_title(g, x, y, w, s, "Properties", ACCENT);
        let Some(p) = self.part().cloned() else { return y };
        let mut rows: Vec<(&str, String)> = vec![("Device", p.path.clone()), ("File system", p.fs.name().to_string()), ("Label", if p.label.is_empty() { "none".into() } else { p.label.clone() }), ("ID", if p.uuid.is_empty() { "none".into() } else { p.uuid.clone() }), ("Size", human(p.size_bytes)), ("Starts at", human(p.start_bytes))];
        if !p.type_name.is_empty() {
            rows.push(("Type", p.type_name.clone()));
        }
        if let Some(c) = &p.crypt {
            rows.push(("Encryption", format!("{} ({}){}", c.kind.name(), c.detail, if p.unlocked { ", unlocked" } else { ", locked" })));
        }
        rows.push(("Mounted at", if p.mounted() { p.mountpoints.join(", ") } else { "not open".into() }));
        if let Ok(plan) = &self.plan {
            rows.push(("Would open at", plan.target.clone()));
            rows.push(("Driver", plan.fstype.to_string()));
            rows.push(("Owner of files", if plan.data.contains("uid=") { "the person using IYAKKI".into() } else { "as stored on the drive".into() }));
        }
        rows.push(("Drive", format!("{} · serial {}", d.name, if d.serial.is_empty() { "none" } else { &d.serial })));
        for (k, v) in rows {
            fx::kv_row(g, R::new(x, y, w, 24.0 * s), s, k, &v, TEXT);
            y += 25.0 * s;
        }
        y
    }

    // ----------------------------------------------------------------------------------------------------------------
    // the partition manager
    // ----------------------------------------------------------------------------------------------------------------

    fn draw_manager(&mut self, g: &mut Gfx, x: f32, mut y: f32, w: f32, s: f32, d: &Disk) -> f32 {
        let boxh = {
            let text = "Advanced: this changes the drive itself. Every change shows its plan first, asks you to type the drive's name or the last 4 characters of its serial number, and needs a second press. Nothing is changed until then.";
            let lines = g.wrap(13.0 * s, text, Face::Sans, w - 28.0 * s).into_iter().take(5).collect::<Vec<_>>();
            let h = lines.len() as f32 * 18.0 * s + 40.0 * s;
            g.rrect(x, y, w, h, 12.0 * s, rgb(40, 24, 24));
            g.rrect_outline(x, y, w, h, 12.0 * s, 1.2 * s, crate::fx::with_alpha(ERR, 160));
            g.text(x + 14.0 * s, y + 10.0 * s, 14.0 * s, ERR, "CHANGE PARTITIONS", Face::Bold);
            for (i, l) in lines.iter().enumerate() {
                g.text(x + 14.0 * s, y + 32.0 * s + i as f32 * 18.0 * s, 13.0 * s, TEXT, l, Face::Sans);
            }
            h
        };
        y += boxh + 10.0 * s;
        let has_part = self.part().is_some();
        let items = vec![
            ("New table".to_string(), Event::Tab(T_NEWTABLE), true, WARN),
            ("Add partition".to_string(), Event::Tab(T_ADD), d.table != Table::None, ACCENT),
            ("Delete".to_string(), Event::Tab(T_DELETE), has_part, ERR),
            ("Format".to_string(), Event::Tab(T_FORMAT), has_part, WARN),
            ("Rename".to_string(), Event::Tab(T_RENAME), has_part, ACCENT),
            ("Resize".to_string(), Event::Tab(T_RESIZE), has_part, WARN),
            ("Read table again".to_string(), Event::Tab(T_REREAD), true, MUTED),
            ("Done".to_string(), Event::Tab(T_ADV_CLOSE), true, CYAN),
        ];
        y = Self::button_row(g, x, y, w, s, &items) + 10.0 * s;
        let hint = match self.part() {
            Some(p) => format!("Partition {} is chosen: Delete, Format and Rename act on it. Keys: n new table · a add · d delete · f format · r rename · z resize · l read the table again.", p.number),
            None => "This drive has no partitions. Make a new table, then add partitions.".to_string(),
        };
        for l in g.wrap(12.0 * s, &hint, Face::Sans, w).into_iter().take(3) {
            g.text(x, y, 12.0 * s, MUTED, &l, Face::Sans);
            y += 17.0 * s;
        }
        let used: u64 = d.parts.iter().map(|p| p.size_bytes).sum();
        let free = d.size_bytes.saturating_sub(used);
        g.text(x, y + 4.0 * s, 12.0 * s, DIM, &format!("Not in a partition: about {}.", human(free)), Face::Sans);
        y + 28.0 * s
    }

    fn draw_form(&mut self, g: &mut Gfx, area: R, s: f32, f: &Form, tick: u64) {
        let title = match f.kind {
            FormKind::NewTable => "New partition table".to_string(),
            FormKind::Add => "Add a partition".to_string(),
            FormKind::Format => format!("Format partition {}", f.number),
            FormKind::Rename => format!("Rename partition {}", f.number),
            FormKind::Resize => format!("Resize partition {}", f.number),
        };
        // as tall as its content needs (the chips wrap on a narrow screen), so nothing is drawn over the buttons
        let inner_w = (560.0 * s).min(area.w - 40.0 * s) - 52.0 * s;
        let mut chip_rows = 1.0;
        let mut cx0 = 0.0;
        if matches!(f.kind, FormKind::Add | FormKind::Format) {
            for (fs, label) in FS_CHOICES.iter() {
                if f.kind == FormKind::Format && fs.is_none() {
                    continue;
                }
                let cw = g.text_width(15.0 * s, label, Face::Bold) + 28.0 * s;
                if cx0 + cw > inner_w && cx0 > 0.0 {
                    chip_rows += 1.0;
                    cx0 = 0.0;
                }
                cx0 += cw + 8.0 * s;
            }
        }
        let err_lines = if f.error.is_empty() { 0.0 } else { g.wrap(13.0 * s, &f.error, Face::Sans, inner_w).len().min(3) as f32 };
        let need = match f.kind {
            FormKind::NewTable => 62.0 + 26.0 + 24.0 + 48.0 + 54.0 + 10.0 + 70.0,
            FormKind::Rename => 62.0 + 26.0 + 64.0 + 70.0,
            FormKind::Resize => 62.0 + 26.0 + 64.0 + 40.0 + 70.0,
            FormKind::Add => 62.0 + 26.0 + 64.0 + 64.0 + 24.0 + chip_rows * 40.0 + 8.0 + 70.0,
            FormKind::Format => 62.0 + 26.0 + 64.0 + 24.0 + chip_rows * 40.0 + 8.0 + 70.0,
        } * s
            + err_lines * 18.0 * s;
        let p = fx::modal(g, area, s, 560.0 * s, need, WARN, false);
        let mut y = fx::modal_title(g, p, s, &title, WARN) + 6.0 * s;
        let x = p.x + 26.0 * s;
        let w = p.w - 52.0 * s;
        let name = self.disk().map(|d| d.identity()).unwrap_or_default();
        let name = g.fit(13.0 * s, &name, Face::Sans, w);
        g.text(x, y, 13.0 * s, MUTED, &name, Face::Sans);
        y += 26.0 * s;
        let blink = super::blink(tick);
        match f.kind {
            FormKind::NewTable => {
                g.text(x, y, 14.0 * s, TEXT, "Kind of table", Face::Bold);
                y += 24.0 * s;
                let mut bx = x;
                for (label, ev, on) in [("GPT (modern, any size)", T_GPT, f.table == Table::Gpt), ("MBR (old PCs, up to 2 TB)", T_MBR, f.table == Table::Mbr)] {
                    let want = g.text_width(15.0 * s, label, Face::Bold) + 28.0 * s;
                    if bx + want > x + w && bx > x {
                        bx = x;
                        y += 42.0 * s;
                    }
                    let cw = fx::toggle_chip(g, bx, y, s, label, on, ACCENT);
                    g.hit(R::new(bx, y, cw, 34.0 * s), Event::Tab(ev), label);
                    bx += cw + 8.0 * s;
                }
                y += 48.0 * s;
                for l in g.wrap(13.0 * s, "This erases the whole drive: every partition on it is gone. You will see the plan before anything happens.", Face::Sans, w).into_iter().take(3) {
                    g.text(x, y, 13.0 * s, ERR, &l, Face::Sans);
                    y += 18.0 * s;
                }
            }
            FormKind::Resize => {
                g.text(x, y, 14.0 * s, TEXT, "New size of the partition", Face::Bold);
                f.size.render(g, R::new(x, y + 20.0 * s, w, 38.0 * s), true, s, blink, Face::Sans, F_SIZE);
                y += 64.0 * s;
                for l in g.wrap(13.0 * s, "The start stays where it is. The file system is checked and resized with it, and a shrink stops if the files would not fit. You see the plan next.", Face::Sans, w).into_iter().take(3) {
                    g.text(x, y, 13.0 * s, MUTED, &l, Face::Sans);
                    y += 18.0 * s;
                }
            }
            FormKind::Add | FormKind::Format | FormKind::Rename => {
                if f.kind == FormKind::Add {
                    g.text(x, y, 14.0 * s, TEXT, "Size", Face::Bold);
                    f.size.render(g, R::new(x, y + 20.0 * s, w, 38.0 * s), f.focus == 0, s, blink, Face::Sans, F_SIZE);
                    y += 64.0 * s;
                }
                g.text(x, y, 14.0 * s, TEXT, "Label (name)", Face::Bold);
                f.label.render(g, R::new(x, y + 20.0 * s, w, 38.0 * s), f.focus == 1 || f.kind != FormKind::Add, s, blink, Face::Sans, F_LABEL);
                y += 64.0 * s;
                if f.kind != FormKind::Rename {
                    g.text(x, y, 14.0 * s, TEXT, "File system", Face::Bold);
                    y += 24.0 * s;
                    let mut bx = x;
                    for (i, (fs, label)) in FS_CHOICES.iter().enumerate() {
                        if f.kind == FormKind::Format && fs.is_none() {
                            continue;
                        }
                        let cw = g.text_width(15.0 * s, label, Face::Bold) + 28.0 * s;
                        if bx + cw > x + w && bx > x {
                            bx = x;
                            y += 40.0 * s;
                        }
                        let on = f.fs == i;
                        fx::toggle_chip(g, bx, y, s, label, on, ACCENT);
                        g.hit(R::new(bx, y, cw, 34.0 * s), Event::Tab(T_FS + i), label);
                        bx += cw + 8.0 * s;
                    }
                    y += 44.0 * s;
                }
            }
        }
        if !f.error.is_empty() {
            for l in g.wrap(13.0 * s, &f.error, Face::Sans, w).into_iter().take(3) {
                g.text(x, y, 13.0 * s, ERR, &l, Face::Sans);
                y += 18.0 * s;
            }
        }
        let by = p.y + p.h - 24.0 * s - 14.0 * s * 2.1;
        let b1 = g.button(x, by, 14.0 * s, "Review the plan", ACCENT, Event::Tab(T_REVIEW));
        g.button(x + b1 + 10.0 * s, by, 14.0 * s, "Cancel", MUTED, Event::Tab(T_CANCEL));
    }

    fn draw_unlock(&mut self, g: &mut Gfx, area: R, s: f32, f: &UnlockForm, tick: u64) {
        let inner_w = (520.0 * s).min(area.w - 40.0 * s) - 52.0 * s;
        let note = "The password is typed here only. It goes straight to the unlock program and is wiped; it is never saved, logged or shown to the assistant. The drive opens read-only.";
        let note_lines = g.wrap(13.0 * s, note, Face::Sans, inner_w).len().min(5) as f32;
        let err_lines = if f.error.is_empty() { 0.0 } else { g.wrap(13.0 * s, &f.error, Face::Sans, inner_w).len().min(3) as f32 };
        let need = (62.0 + 26.0 + 64.0 + 70.0) * s + (note_lines + err_lines) * 18.0 * s;
        let p = fx::modal(g, area, s, 520.0 * s, need, WARN, false);
        let mut y = fx::modal_title(g, p, s, &format!("Unlock {}", f.kind.family()), WARN) + 6.0 * s;
        let x = p.x + 26.0 * s;
        let w = p.w - 52.0 * s;
        let name = g.fit(13.0 * s, &f.title, Face::Sans, w);
        g.text(x, y, 13.0 * s, MUTED, &name, Face::Sans);
        y += 26.0 * s;
        let label = match f.kind {
            CryptKind::BitLocker => "Password or 48-digit recovery key",
            _ => "Password",
        };
        g.text(x, y, 14.0 * s, TEXT, label, Face::Bold);
        f.field.render(g, R::new(x, y + 20.0 * s, w, 38.0 * s), f.focus == 0, s, super::blink(tick), Face::Sans, F_SECRET);
        y += 64.0 * s;
        for l in g.wrap(13.0 * s, note, Face::Sans, w).into_iter().take(5) {
            g.text(x, y, 13.0 * s, MUTED, &l, Face::Sans);
            y += 18.0 * s;
        }
        if !f.error.is_empty() {
            for l in g.wrap(13.0 * s, &f.error, Face::Sans, w).into_iter().take(3) {
                g.text(x, y, 13.0 * s, ERR, &l, Face::Sans);
                y += 18.0 * s;
            }
        }
        let by = p.y + p.h - 24.0 * s - 14.0 * s * 2.1;
        let b1 = g.button(x, by, 14.0 * s, "Unlock", WARN, Event::Tab(T_DO_UNLOCK));
        g.button(x + b1 + 10.0 * s, by, 14.0 * s, "Cancel", MUTED, Event::Tab(T_CANCEL));
    }

    fn draw_review(&mut self, g: &mut Gfx, area: R, s: f32, r: &Review, tick: u64) {
        let p = fx::modal(g, area, s, 720.0 * s, 620.0 * s, ERR, true);
        let x = p.x + 26.0 * s;
        let w = p.w - 52.0 * s;
        let mut y = fx::modal_title(g, p, s, "Check this before anything is changed", ERR) + 4.0 * s;
        // the drive, exactly: name, size, serial
        let disk = self.snap.disk(&r.disk).cloned();
        if let Some(d) = &disk {
            g.rrect(x, y, w, 64.0 * s, 10.0 * s, rgb(34, 20, 26));
            g.rrect_outline(x, y, w, 64.0 * s, 10.0 * s, 1.0 * s, crate::fx::with_alpha(ERR, 120));
            let t = g.fit(17.0 * s, &format!("{}  ·  {}", d.name, d.title()), Face::Bold, w - 24.0 * s);
            g.text(x + 12.0 * s, y + 8.0 * s, 17.0 * s, TEXT, &t, Face::Bold);
            let t = g.fit(14.0 * s, &format!("{}  ·  serial {}", human(d.size_bytes), if d.serial.is_empty() { "none" } else { &d.serial }), Face::Mono, w - 24.0 * s);
            g.text(x + 12.0 * s, y + 34.0 * s, 14.0 * s, WARN, &t, Face::Mono);
        }
        y += 74.0 * s;
        // the plan, scrollable
        let foot = 150.0 * s;
        let box_ = R::new(x, y, w, (p.y + p.h - foot - y).max(40.0 * s));
        g.rrect(box_.x, box_.y, box_.w, box_.h, 8.0 * s, rgb(10, 14, 28));
        let lh = 15.0 * s;
        let mut wrapped: Vec<String> = Vec::new();
        for l in &r.lines {
            if l.trim().is_empty() {
                wrapped.push(String::new());
            } else {
                wrapped.extend(g.wrap(12.0 * s, l, Face::Mono, box_.w - 20.0 * s));
            }
        }
        let fit = ((box_.h - 12.0 * s) / lh).floor().max(1.0) as usize;
        let start = r.scroll.min(wrapped.len().saturating_sub(fit));
        for (i, l) in wrapped.iter().skip(start).take(fit).enumerate() {
            g.text(box_.x + 10.0 * s, box_.y + 6.0 * s + i as f32 * lh, 12.0 * s, TEXT, l, Face::Mono);
        }
        if wrapped.len() > fit {
            g.text(box_.x + box_.w - 150.0 * s, box_.y + box_.h - 16.0 * s, 11.0 * s, DIM, "scroll for the rest", Face::Sans);
        }
        y = box_.y + box_.h + 10.0 * s;
        // the typed confirmation
        let ask = match &disk {
            Some(d) => format!("Type one of these exactly to confirm: {}.", iyakki_agent::disks_part::accepted_names(d, &r.op).join(", ")),
            None => "The drive is gone.".to_string(),
        };
        g.text(x, y, 13.0 * s, TEXT, &ask, Face::Bold);
        y += 20.0 * s;
        r.field.render(g, R::new(x, y, w, 36.0 * s), r.focus == 0, s, super::blink(tick), Face::Mono, F_CONFIRM);
        y += 44.0 * s;
        if !r.error.is_empty() {
            let e = g.fit(13.0 * s, &r.error, Face::Sans, w);
            g.text(x, y - 2.0 * s, 13.0 * s, ERR, &e, Face::Sans);
        }
        let by = p.y + p.h - 22.0 * s - 14.0 * s * 2.1;
        let ok = self.review_typed_ok();
        if let Some(left) = self.countdown_left() {
            // the cancel window: nothing has been written; Cancel (or Esc) stops it
            let bw1 = g.button(x, by, 14.0 * s, "Cancel", ERR, Event::Tab(T_CANCEL));
            g.rrect_outline(x - 3.0 * s, by - 3.0 * s, bw1 + 6.0 * s, 14.0 * s * 2.1 + 6.0 * s, 14.0 * s * 1.05 + 3.0 * s, 1.6 * s, TEXT);
            g.text(x + bw1 + 16.0 * s, by + 6.0 * s, 15.0 * s, WARN, &format!("Starting in {left} s. Press Cancel or Esc to stop it."), Face::Bold);
            return;
        }
        let bw1 = g.button(x, by, 14.0 * s, "Cancel", MUTED, Event::Tab(T_CANCEL));
        let (ax, label) = (x + bw1 + 12.0 * s, "Change this drive");
        let aw = if ok { g.button(ax, by, 14.0 * s, label, ERR, Event::Tab(T_APPLY)) } else { g.button_off(ax, by, 14.0 * s, label) };
        let focus_r = match r.focus {
            1 => Some(R::new(x, by, bw1, 14.0 * s * 2.1)),
            2 => Some(R::new(ax, by, aw, 14.0 * s * 2.1)),
            _ => None,
        };
        if let Some(f) = focus_r {
            g.rrect_outline(f.x - 3.0 * s, f.y - 3.0 * s, f.w + 6.0 * s, f.h + 6.0 * s, f.h / 2.0 + 3.0 * s, 1.6 * s, TEXT);
        }
        if !ok {
            g.text(ax + aw + 14.0 * s, by + 8.0 * s, 12.0 * s, DIM, "Type the name first", Face::Sans);
        }
    }
}

/// A dark tint of `c` for a strip's background.
fn over_bg(c: [u8; 4]) -> [u8; 4] {
    fx::over(rgb(14, 18, 34), fx::with_alpha(c, 40))
}
