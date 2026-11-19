//! The widget board on the home screen's machine page (LAUNCHER-PLAN slice 4, F-LAUNCH-08): which widgets show, in which order and at
//! which width. The model is pure (no drawing, no machine); the shell draws it and the chat tools change it through the same functions.
//! It is kept per person in `ui-<user>.json` (key `board`) beside the theme, wallpaper and fonts, so every account has its own board.

/// What a widget shows. Each one reads only what the core already says (`iyakki_core_api`): the apps' statuses, the load and memory, the
/// token ledger, the interfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// What needs a person, with the real problem named.
    Attention,
    /// Tool calls the assistant waits on.
    Approvals,
    /// Load, memory and how long the machine has been up.
    Vitals,
    /// Jobs and runs going now.
    Runs,
    /// Tokens and calls of the last 24 hours.
    Tokens,
    /// The network interfaces and their addresses.
    Network,
    /// The groups of apps with their counts (what the page showed before the board).
    Apps,
}

pub const ALL: [Kind; 7] = [Kind::Attention, Kind::Approvals, Kind::Vitals, Kind::Runs, Kind::Tokens, Kind::Network, Kind::Apps];

impl Kind {
    pub fn key(self) -> &'static str {
        match self {
            Kind::Attention => "attention",
            Kind::Approvals => "approvals",
            Kind::Vitals => "vitals",
            Kind::Runs => "runs",
            Kind::Tokens => "tokens",
            Kind::Network => "network",
            Kind::Apps => "apps",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Kind::Attention => "NEEDS A PERSON",
            Kind::Approvals => "APPROVALS",
            Kind::Vitals => "THE MACHINE",
            Kind::Runs => "RUNS",
            Kind::Tokens => "TOKENS, 24 HOURS",
            Kind::Network => "NETWORK",
            Kind::Apps => "APPS",
        }
    }
    pub fn from_key(k: &str) -> Option<Kind> {
        ALL.into_iter().find(|x| x.key() == k.trim().to_lowercase())
    }
}

/// How much of the page's width a widget takes: half, or all of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Half,
    Full,
}

impl Size {
    pub fn key(self) -> &'static str {
        match self {
            Size::Half => "half",
            Size::Full => "full",
        }
    }
    pub fn from_key(k: &str) -> Option<Size> {
        match k.trim().to_lowercase().as_str() {
            "half" => Some(Size::Half),
            "full" => Some(Size::Full),
            _ => None,
        }
    }
    pub fn other(self) -> Size {
        match self {
            Size::Half => Size::Full,
            Size::Full => Size::Half,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub kind: Kind,
    pub size: Size,
}

/// The widgets in order. Every kind appears at most once; the board is never longer than [`MAX`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Board {
    pub slots: Vec<Slot>,
}

pub const MAX: usize = ALL.len();

impl Default for Board {
    /// What a person sees before they change anything: what needs them (full width), then the apps, the machine, the runs and the network.
    fn default() -> Board {
        let s = |kind, size| Slot { kind, size };
        Board { slots: vec![s(Kind::Attention, Size::Full), s(Kind::Apps, Size::Half), s(Kind::Vitals, Size::Half), s(Kind::Runs, Size::Half), s(Kind::Network, Size::Half)] }
    }
}

impl Board {
    pub fn has(&self, k: Kind) -> bool {
        self.slots.iter().any(|s| s.kind == k)
    }
    /// The kinds that are not on the board, in the usual order (what "Add" offers).
    pub fn missing(&self) -> Vec<Kind> {
        ALL.into_iter().filter(|k| !self.has(*k)).collect()
    }
    /// Take the widget at `i` off the board. The Attention widget can be taken off too: a person may want a quiet page.
    pub fn remove(&mut self, i: usize) -> Option<Slot> {
        (i < self.slots.len()).then(|| self.slots.remove(i))
    }
    /// Move the widget at `i` to place `to` (clamped), the others shifting. Returns where it ended.
    pub fn move_to(&mut self, i: usize, to: usize) -> Option<usize> {
        if i >= self.slots.len() {
            return None;
        }
        let to = to.min(self.slots.len() - 1);
        let s = self.slots.remove(i);
        self.slots.insert(to, s);
        Some(to)
    }
    pub fn set_size(&mut self, i: usize, size: Size) -> bool {
        self.slots.get_mut(i).map(|s| s.size = size).is_some()
    }
    /// Add a kind at the end (half width, Attention and Apps full). False when it is already there.
    pub fn add(&mut self, k: Kind) -> bool {
        if self.has(k) || self.slots.len() >= MAX {
            return false;
        }
        self.slots.push(Slot { kind: k, size: if matches!(k, Kind::Attention) { Size::Full } else { Size::Half } });
        true
    }
    /// `attention:full,apps:half`
    pub fn to_text(&self) -> String {
        self.slots.iter().map(|s| format!("{}:{}", s.kind.key(), s.size.key())).collect::<Vec<_>>().join(",")
    }
    /// Read a board back; an unknown word or a repeat is dropped, never trusted. `None` for text that holds nothing usable.
    pub fn from_text(t: &str) -> Option<Board> {
        let mut b = Board { slots: Vec::new() };
        for part in t.split(',') {
            let (k, z) = part.split_once(':').unwrap_or((part, "half"));
            if let (Some(kind), Some(size)) = (Kind::from_key(k), Size::from_key(z)) {
                if !b.has(kind) {
                    b.slots.push(Slot { kind, size });
                }
            }
        }
        (!b.slots.is_empty() || t.trim() == "none").then_some(b)
    }
}

// --------------------------------------------------------------------------------------------------- saved per person

fn path(dir: &std::path::Path, user: &str) -> std::path::PathBuf {
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    dir.join(format!("ui-{who}.json"))
}

/// The person's board, the default one when none is kept or the kept one cannot be read. An empty board is kept as `none`.
pub fn load(dir: &std::path::Path, user: &str) -> Board {
    let v: Option<serde_json::Value> = std::fs::read_to_string(path(dir, user)).ok().and_then(|t| serde_json::from_str(&t).ok());
    match v.as_ref().and_then(|v| v["board"].as_str()) {
        Some("none") => Board { slots: Vec::new() },
        Some(t) => Board::from_text(t).unwrap_or_default(),
        None => Board::default(),
    }
}

/// Keep the board beside the person's other display settings, leaving everything else in the file alone.
pub fn save(dir: &std::path::Path, user: &str, b: &Board) {
    let p = path(dir, user);
    let mut v = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).filter(|v| v.is_object()).unwrap_or_else(|| serde_json::json!({}));
    v["board"] = serde_json::json!(if b.slots.is_empty() { "none".to_string() } else { b.to_text() });
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

#[cfg(test)]
#[path = "board_tests.rs"]
mod tests;
