//! The access lint of a tile (F-A11Y-06, store review): load the tile, draw its first picture and list the clickable regions that no access
//! node names. A screen reader cannot tell the person what such a region is, so the store shows the list as a warning; a tile that gives no
//! access node at all (ABI 1) has every region in the list. The lint runs the tile in the same sandbox and budgets as any other start and
//! changes nothing.

use crate::{Budgets, TileError, TileManifest, TileRunner};
use iyakki_tile_abi::{Color, Event};

/// What the lint found in the first picture of a tile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessLint {
    /// Clickable regions in the picture.
    pub hits: usize,
    /// Access nodes in the picture.
    pub nodes: usize,
    /// The ids of the clickable regions no node names.
    pub unnamed: Vec<u32>,
    /// Nodes with no name (a control read out as nothing).
    pub empty_names: usize,
}

impl AccessLint {
    /// The words the store review shows; empty when there is nothing to say.
    pub fn warning(&self) -> String {
        if self.unnamed.is_empty() && self.empty_names == 0 {
            return String::new();
        }
        let mut s = String::new();
        if !self.unnamed.is_empty() {
            s.push_str(&format!("{} of {} clickable parts have no name for a screen reader", self.unnamed.len(), self.hits));
        }
        if self.empty_names > 0 {
            if !s.is_empty() {
                s.push_str("; ");
            }
            s.push_str(&format!("{} parts are named with nothing", self.empty_names));
        }
        s
    }

    /// Is this a failure rather than a warning? Warnings today; the store turns on strictness (`strict`) when the tiles it hosts have caught up.
    pub fn is_error(&self, strict: bool) -> bool {
        strict && (!self.unnamed.is_empty() || self.empty_names > 0)
    }
}

/// Run the tile at 320 by 240 and read its first picture.
pub fn access_lint(wasm: &[u8]) -> Result<AccessLint, TileError> {
    let mut last = None;
    for abi in [iyakki_tile_abi::ABI_VERSION_ACCESS, iyakki_tile_abi::ABI_VERSION] {
        let mut m = TileManifest::new("lint", &[]);
        m.abi = abi;
        match TileRunner::load(wasm, m, Budgets::default(), None, 1) {
            Ok(mut r) => {
                let theme = Event::Theme { dark: false, accent: Color(0, 90, 200, 255), background: Color(255, 255, 255, 255), foreground: Color(0, 0, 0, 255) };
                r.init(320, 240, 100, theme)?;
                let l = r.frame()?;
                let nodes = l.access_tree();
                return Ok(AccessLint {
                    hits: l.ops.iter().filter(|o| matches!(o, iyakki_tile_abi::Op::Hit { .. })).count(),
                    empty_names: nodes.iter().filter(|n| n.name.trim().is_empty()).count(),
                    nodes: nodes.len(),
                    unnamed: l.unnamed_hits(),
                });
            }
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or(TileError::BadPointer))
}
