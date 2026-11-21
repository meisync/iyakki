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

/// The day the store stops accepting tiles with unnamed clickable parts, as `YYYY-MM-DD`; `None` until the owner sets it (decision
/// D-A11Y-LINT-STRICT in docs/development/ALPHA-7-DECISIONS.md: warnings only, no date, until the tiles the store hosts have caught up).
pub const STRICT_FROM: Option<&str> = None;

/// What the store's review shows the person who installs a tile: the warning words, whether anything was found, and the date strictness
/// begins (so an author can see how long a warning is allowed to stay).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    pub warning: String,
    pub unnamed: usize,
    pub empty_names: usize,
    pub strict_from: Option<String>,
    /// Would this tile be refused today (strict and a problem found)?
    pub refused: bool,
}

impl AccessLint {
    /// The review for a store screen or an install answer. `today` is `YYYY-MM-DD` (it only decides whether `strict_from` has come).
    pub fn review(&self, strict_from: Option<&str>, today: &str) -> Review {
        let strict = strict_from.is_some_and(|d| today >= d);
        Review { warning: self.warning(), unnamed: self.unnamed.len(), empty_names: self.empty_names, strict_from: strict_from.map(String::from), refused: self.is_error(strict) }
    }

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

/// The lint of one picture a tile drew (what [`access_lint`] reads from the first one; the shell reads the one on its screen).
pub fn lint_of_list(l: &iyakki_tile_abi::DisplayList) -> AccessLint {
    let nodes = l.access_tree();
    AccessLint { hits: l.ops.iter().filter(|o| matches!(o, iyakki_tile_abi::Op::Hit { .. })).count(), empty_names: nodes.iter().filter(|n| n.name.trim().is_empty()).count(), nodes: nodes.len(), unnamed: l.unnamed_hits() }
}

impl Review {
    /// The lines of the drawn review screen: the verdict, the counts, and when strictness begins.
    pub fn screen_lines(&self, lint: &AccessLint) -> Vec<String> {
        let mut v = vec![if self.warning.is_empty() { "Every clickable part has a name a screen reader can say.".to_string() } else { format!("{}.", self.warning) }];
        v.push(format!("{} clickable parts, {} described parts.", lint.hits, lint.nodes));
        v.push(match &self.strict_from {
            Some(d) if self.refused => format!("Since {d} a tile with an unnamed part is refused by the store: this one would be."),
            Some(d) => format!("From {d} the store refuses a tile with an unnamed part."),
            None => "Today this is a warning only: no day is set on which the store refuses such a tile.".to_string(),
        });
        v
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
                return Ok(lint_of_list(&r.frame()?));
            }
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or(TileError::BadPointer))
}

#[cfg(test)]
mod review_tests {
    use super::*;

    fn found() -> AccessLint {
        AccessLint { hits: 4, nodes: 2, unnamed: vec![1, 2], empty_names: 0 }
    }

    #[test]
    fn the_drawn_review_says_the_verdict_the_counts_and_the_strict_day() {
        let l = found();
        let w = l.review(None, "2030-01-01").screen_lines(&l);
        assert!(w[0].contains("2 of 4 clickable parts") && w[1].contains("4 clickable parts, 2 described") && w[2].contains("warning only"), "{w:?}");
        let r = l.review(Some("2027-06-01"), "2027-06-02").screen_lines(&l);
        assert!(r[2].contains("this one would be"), "{r:?}");
        let before = l.review(Some("2027-06-01"), "2027-05-01").screen_lines(&l);
        assert!(before[2].contains("From 2027-06-01"), "{before:?}");
        let clean = AccessLint { hits: 3, nodes: 3, unnamed: vec![], empty_names: 0 };
        assert!(clean.review(None, "2030-01-01").screen_lines(&clean)[0].starts_with("Every clickable part"));
    }

    #[test]
    fn the_review_is_a_warning_until_the_strict_day_and_then_a_refusal() {
        assert_eq!(STRICT_FROM, None, "no date is set until the owner sets one");
        let r = found().review(None, "2030-01-01");
        assert!(!r.refused && r.unnamed == 2 && r.warning.contains("2 of 4 clickable parts"), "{r:?}");
        let before = found().review(Some("2027-06-01"), "2027-05-31");
        assert!(!before.refused && before.strict_from.as_deref() == Some("2027-06-01"));
        assert!(found().review(Some("2027-06-01"), "2027-06-01").refused);
        let clean = AccessLint { hits: 3, nodes: 3, unnamed: vec![], empty_names: 0 }.review(Some("2020-01-01"), "2030-01-01");
        assert!(!clean.refused && clean.warning.is_empty());
    }
}
