//! A braille display that speaks the standard USB HID braille protocol (usage page 0x41), driven through `/dev/hidrawN` (F-A11Y-07).
//!
//! No library and no table file: the report descriptor the display gives about itself says where its cells are in the output report and which
//! bits of the input report are the routing keys and the buttons, so one driver serves every display that follows the standard. A display
//! with a protocol of its own (a serial or vendor one) is not served here.
//!
//! Pure parts (descriptor, report building, report reading) work on bytes; the device part at the end only opens the node and moves bytes.

use crate::Roots;
use std::io::{Read, Write};
use std::path::PathBuf;

const BRAILLE_PAGE: u32 = 0x41;
const U_CELL8: u32 = 0x03;
const U_CELL6: u32 = 0x04;
const U_ROUTER: u32 = 0x100;
const U_BUTTON_FIRST: u32 = 0x200;
const U_BUTTON_LAST: u32 = 0x2ff;

/// One key of the display as a report said it was pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// The routing key above display cell `n` (from 0, left to right).
    Router(usize),
    /// A button by its HID usage (0x21A pan left, 0x21B pan right, 0x211 and 0x212 up and down, 0x213 and 0x214 left and right, ...).
    Button(u32),
}

/// What a key means to the screen reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    PanLeft,
    PanRight,
    /// Move the focus to the previous or the next element.
    FocusPrev,
    FocusNext,
    /// Put the cursor on the character under display cell `n`.
    Route(usize),
}

/// The meaning of a key; keys that have none are `None`.
pub fn action(k: Key) -> Option<Action> {
    Some(match k {
        Key::Router(n) => Action::Route(n),
        Key::Button(0x21a) | Key::Button(0x213) | Key::Button(0x218) => Action::PanLeft,
        Key::Button(0x21b) | Key::Button(0x214) | Key::Button(0x219) => Action::PanRight,
        Key::Button(0x211) | Key::Button(0x216) | Key::Button(0x21c) => Action::FocusPrev,
        Key::Button(0x212) | Key::Button(0x217) | Key::Button(0x21d) => Action::FocusNext,
        Key::Button(_) => return None,
    })
}

/// Where one input bit is and what it is.
#[derive(Clone, Debug, PartialEq, Eq)]
struct InBit {
    id: u8,
    bit: u32,
    key: Key,
}

/// What the display's report descriptor says about its reports.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    /// The output report that holds the cells: its id (0 when the device uses no ids) and the bit where each cell's eight dots start.
    out_id: u8,
    cells: Vec<u32>,
    out_bits: u32,
    ins: Vec<InBit>,
    /// The device numbers its reports (any item had a report id).
    ids: bool,
}

impl Layout {
    /// How many cells the display has.
    pub fn cells(&self) -> usize {
        self.cells.len()
    }

    /// Parse a report descriptor; `None` when it has no braille cells.
    pub fn parse(desc: &[u8]) -> Option<Layout> {
        let (mut page, mut size, mut count, mut id) = (0u32, 0u32, 0u32, 0u8);
        let mut usages: Vec<u32> = Vec::new();
        let (mut umin, mut umax) = (None::<u32>, None::<u32>);
        let mut stack: Vec<(u32, u32, u32, u8)> = Vec::new();
        let mut in_off: std::collections::BTreeMap<u8, u32> = Default::default();
        let mut out_off: std::collections::BTreeMap<u8, u32> = Default::default();
        let mut lay = Layout::default();
        let mut router = 0usize;
        let mut have_out = false;
        let mut i = 0usize;
        while i < desc.len() {
            let b = desc[i];
            if b == 0xfe {
                // long item: skip it
                let n = *desc.get(i + 1)? as usize;
                i += 3 + n;
                continue;
            }
            let n = match b & 3 {
                3 => 4,
                x => x as usize,
            };
            let data = desc.get(i + 1..i + 1 + n)?;
            let v = data.iter().rev().fold(0u32, |a, &x| (a << 8) | x as u32);
            let (kind, tag) = ((b >> 2) & 3, b >> 4);
            i += 1 + n;
            match (kind, tag) {
                (1, 0) => page = v,
                (1, 7) => size = v,
                (1, 8) => {
                    id = v as u8;
                    lay.ids = true;
                }
                (1, 9) => count = v,
                (1, 10) => stack.push((page, size, count, id)),
                (1, 11) => {
                    if let Some(s) = stack.pop() {
                        (page, size, count, id) = s;
                    }
                }
                (2, 0) => usages.push(if n == 4 { v } else { (page << 16) | v }),
                (2, 1) => umin = Some(if n == 4 { v } else { (page << 16) | v }),
                (2, 2) => umax = Some(if n == 4 { v } else { (page << 16) | v }),
                (0, 8..=9 | 11) => {
                    // Input, Output, Feature
                    let mut list = std::mem::take(&mut usages);
                    if let (Some(a), Some(z)) = (umin, umax) {
                        if z >= a && z - a < 4096 {
                            list.extend(a..=z);
                        }
                    }
                    (umin, umax) = (None, None);
                    let constant = v & 1 == 1;
                    let variable = v & 2 == 2;
                    let total = size.saturating_mul(count);
                    let off = match tag {
                        8 => &mut in_off,
                        9 => &mut out_off,
                        _ => {
                            continue;
                        }
                    };
                    let base = *off.entry(id).or_insert(0);
                    if !constant && variable {
                        for e in 0..count {
                            let u = *list.get(e as usize).or(list.last()).unwrap_or(&0);
                            let (p, u) = (u >> 16, u & 0xffff);
                            if p != BRAILLE_PAGE {
                                continue;
                            }
                            let at = base + e * size;
                            if tag == 9 && (u == U_CELL8 || u == U_CELL6) && size == 8 && (!have_out || lay.out_id == id) {
                                have_out = true;
                                lay.out_id = id;
                                lay.cells.push(at);
                            } else if tag == 8 && size == 1 {
                                if u == U_ROUTER {
                                    lay.ins.push(InBit { id, bit: at, key: Key::Router(router) });
                                    router += 1;
                                } else if (U_BUTTON_FIRST..=U_BUTTON_LAST).contains(&u) {
                                    lay.ins.push(InBit { id, bit: at, key: Key::Button(u) });
                                }
                            }
                        }
                    }
                    *off.get_mut(&id).expect("just inserted") = base + total;
                }
                (0, _) => {
                    usages.clear();
                    (umin, umax) = (None, None);
                }
                _ => {}
            }
        }
        if lay.cells.is_empty() {
            return None;
        }
        lay.out_bits = out_off.get(&lay.out_id).copied().unwrap_or(0);
        Some(lay)
    }

    /// The output report that shows `cells` (characters U+2800 to U+28FF, the dots of one cell in the low eight bits); a cell past the end of
    /// the text is blank and a character that is no braille cell is blank too.
    pub fn show(&self, cells: &str) -> Vec<u8> {
        let head = usize::from(self.ids);
        let mut r = vec![0u8; head + (self.out_bits as usize).div_ceil(8)];
        if self.ids {
            r[0] = self.out_id;
        }
        let mut it = cells.chars();
        for &at in &self.cells {
            let dots = match it.next() {
                Some(c) if ('\u{2800}'..='\u{28ff}').contains(&c) => (c as u32 - 0x2800) as u8,
                _ => 0,
            };
            let byte = head + (at / 8) as usize;
            if at % 8 == 0 {
                if let Some(x) = r.get_mut(byte) {
                    *x = dots;
                }
            }
        }
        r
    }

    /// The keys down in one input report (an empty list for a report this layout does not know).
    pub fn keys(&self, report: &[u8]) -> Vec<Key> {
        let (id, body) = if self.ids { (report.first().copied().unwrap_or(0), report.get(1..).unwrap_or(&[])) } else { (0, report) };
        self.ins.iter().filter(|b| b.id == id && body.get((b.bit / 8) as usize).is_some_and(|x| x >> (b.bit % 8) & 1 == 1)).map(|b| b.key).collect()
    }
}

/// A display found on this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub node: PathBuf,
    pub layout: Layout,
}

/// The braille displays among the HID devices (`<sys>/class/hidraw/hidrawN/device/report_descriptor`), their nodes under `dev`.
pub fn find(roots: &Roots, dev: &std::path::Path) -> Vec<Found> {
    let mut out = Vec::new();
    for d in crate::dirs(&roots.sys.join("class/hidraw")) {
        let Some(name) = d.file_name().map(|n| n.to_string_lossy().to_string()) else { continue };
        let Ok(desc) = std::fs::read(d.join("device/report_descriptor")) else { continue };
        if let Some(layout) = Layout::parse(&desc) {
            out.push(Found { node: dev.join(name), layout });
        }
    }
    out
}

/// What the screen reader needs of a display: the cells to show and the keys that come back.
pub trait Display {
    fn cells(&self) -> usize;
    fn show(&mut self, cells: &str) -> Result<(), String>;
    /// The keys pressed since the last call; an empty list when there are none (never waits).
    fn keys(&mut self) -> Vec<Key>;
}

/// A display on a hidraw node.
pub struct Hid {
    file: std::fs::File,
    layout: Layout,
}

impl Hid {
    pub fn open(f: &Found) -> Result<Hid, String> {
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NONBLOCK).open(&f.node).map_err(|e| format!("{}: {e}", f.node.display()))?;
        Ok(Hid { file, layout: f.layout.clone() })
    }
}

impl Display for Hid {
    fn cells(&self) -> usize {
        self.layout.cells()
    }

    fn show(&mut self, cells: &str) -> Result<(), String> {
        self.file.write_all(&self.layout.show(cells)).map_err(|e| format!("the braille display did not take the cells: {e}"))
    }

    fn keys(&mut self) -> Vec<Key> {
        let mut buf = [0u8; 64];
        let mut out = Vec::new();
        while let Ok(n) = self.file.read(&mut buf) {
            if n == 0 {
                break;
            }
            out.extend(self.layout.keys(&buf[..n]));
        }
        out
    }
}

#[cfg(test)]
#[path = "braillehid_tests.rs"]
mod tests;
