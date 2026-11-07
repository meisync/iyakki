// SPDX-License-Identifier: MIT OR Apache-2.0
//! Boot assessment: count starts that were never confirmed, roll back to `previous`, report why.
//!
//! The loader calls `on_start` before it starts a kernel and stores the state. The running system calls `mark_good`
//! once it is up and stores the state again. If `tries` starts in a row are never confirmed, the next start uses `previous`.
//! If `previous` also fails `tries` times, the loader does not loop: `on_start` answers `StopAndWait` and the menu opens with no
//! timeout, shows why, and waits for the person (who may pick another system found by the scan, or any entry).

use alloc::string::String;
use core::fmt;

/// Size of the stored state.
pub const STATE_LEN: usize = 16;
const MAGIC: [u8; 4] = *b"IBT1";

/// Which installed tree to start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tree {
    /// The newest tree.
    Current,
    /// The one before it.
    Previous,
}

impl Tree {
    /// Name used for `{tree}` in config paths.
    pub fn name(&self) -> &'static str {
        match self {
            Tree::Current => "current",
            Tree::Previous => "previous",
        }
    }
}

/// Why something happened, shown on the next good start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Nothing to report.
    None,
    /// The current tree failed too often and `previous` was used.
    TooManyFailures,
    /// The stored state was damaged and was reset.
    StateCorrupt,
    /// `current` and `previous` both failed too often; the loader stopped and waits for the person.
    AllTreesFailed,
}

/// What `on_start` tells the loader to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Start this tree.
    Start(Tree),
    /// Start nothing by itself: open the menu with no timeout, show the reason, wait for the person.
    StopAndWait(Reason),
}

impl Reason {
    fn code(self) -> u8 {
        match self {
            Reason::None => 0,
            Reason::TooManyFailures => 1,
            Reason::StateCorrupt => 2,
            Reason::AllTreesFailed => 3,
        }
    }

    fn from_code(c: u8) -> Option<Reason> {
        match c {
            0 => Some(Reason::None),
            1 => Some(Reason::TooManyFailures),
            2 => Some(Reason::StateCorrupt),
            3 => Some(Reason::AllTreesFailed),
            _ => None,
        }
    }

    /// Short text for the user.
    pub fn text(&self) -> &'static str {
        match self {
            Reason::None => "",
            Reason::TooManyFailures => "the newest system failed to start several times; the previous one was started",
            Reason::StateCorrupt => "the boot counter was damaged and was reset",
            Reason::AllTreesFailed => "the newest and the previous system both failed to start several times; choose what to start",
        }
    }
}

/// The stored state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootState {
    /// Tree to start.
    pub tree: Tree,
    /// Starts left before a rollback.
    pub tries_left: u8,
    /// Reason waiting to be reported.
    pub reason: Reason,
    /// Number of starts so far (diagnostic, wraps).
    pub starts: u32,
}

/// Why stored bytes were refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// Wrong length.
    Length,
    /// Wrong magic.
    Magic,
    /// Checksum differs.
    Checksum,
    /// A field has an impossible value.
    Field,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s: String = match self {
            DecodeError::Length => "wrong length".into(),
            DecodeError::Magic => "wrong magic".into(),
            DecodeError::Checksum => "checksum mismatch".into(),
            DecodeError::Field => "bad field".into(),
        };
        write!(f, "boot state: {s}")
    }
}

/// CRC-32 (IEEE, reflected).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

impl BootState {
    /// A new good state.
    pub fn fresh(tries: u8) -> BootState {
        BootState { tree: Tree::Current, tries_left: clamp(tries), reason: Reason::None, starts: 0 }
    }

    /// Serialise: magic, tree, tries, reason, reserved, starts (LE), CRC-32 of the first 12 bytes (LE).
    pub fn encode(&self) -> [u8; STATE_LEN] {
        let mut b = [0u8; STATE_LEN];
        b[..4].copy_from_slice(&MAGIC);
        b[4] = matches!(self.tree, Tree::Previous) as u8;
        b[5] = self.tries_left;
        b[6] = self.reason.code();
        b[8..12].copy_from_slice(&self.starts.to_le_bytes());
        let c = crc32(&b[..12]);
        b[12..].copy_from_slice(&c.to_le_bytes());
        b
    }

    /// Parse and check.
    pub fn decode(b: &[u8]) -> Result<BootState, DecodeError> {
        if b.len() != STATE_LEN {
            return Err(DecodeError::Length);
        }
        if b[..4] != MAGIC {
            return Err(DecodeError::Magic);
        }
        if crc32(&b[..12]) != u32::from_le_bytes([b[12], b[13], b[14], b[15]]) {
            return Err(DecodeError::Checksum);
        }
        let tree = match b[4] {
            0 => Tree::Current,
            1 => Tree::Previous,
            _ => return Err(DecodeError::Field),
        };
        let reason = Reason::from_code(b[6]).ok_or(DecodeError::Field)?;
        if b[5] > 9 || b[7] != 0 {
            return Err(DecodeError::Field);
        }
        Ok(BootState { tree, tries_left: b[5], reason, starts: u32::from_le_bytes([b[8], b[9], b[10], b[11]]) })
    }

    /// Load stored bytes. Missing gives a fresh state; damaged gives a fresh state that reports `StateCorrupt`
    /// (no rollback: damage is no evidence that the kernel is bad).
    pub fn load(stored: Option<&[u8]>, tries: u8) -> BootState {
        match stored {
            None => BootState::fresh(tries),
            Some(b) => BootState::decode(b).unwrap_or(BootState { reason: Reason::StateCorrupt, ..BootState::fresh(tries) }),
        }
    }

    /// The loader is about to start a kernel from the tree it would pick by itself: spend a try, switch to `previous` when
    /// none are left, and when `previous` has none left either return `StopAndWait(AllTreesFailed)` (nothing is spent; the
    /// state stays as it is, so asking again gives the same answer until a start is confirmed or a new tree is installed).
    pub fn on_start(&mut self, tries: u8) -> Outcome {
        let tries = clamp(tries);
        self.tries_left = self.tries_left.min(tries);
        if self.tries_left == 0 {
            if self.tree == Tree::Previous {
                self.reason = Reason::AllTreesFailed;
                return Outcome::StopAndWait(Reason::AllTreesFailed);
            }
            self.tree = Tree::Previous;
            self.reason = Reason::TooManyFailures;
            self.tries_left = tries;
        }
        self.tries_left -= 1;
        self.starts = self.starts.wrapping_add(1);
        Outcome::Start(self.tree)
    }

    /// The person chose an entry by hand (after a stop, or from the menu). Nothing destructive: tree, tries and reason stay,
    /// only the diagnostic counter moves. If that system comes up, `mark_good` refills the tries.
    pub fn manual_start(&mut self) {
        self.starts = self.starts.wrapping_add(1);
    }

    /// The system is up: refill the tries. The reason stays until `take_reason`.
    pub fn mark_good(&mut self, tries: u8) {
        self.tries_left = clamp(tries);
    }

    /// Hand out the waiting reason once.
    pub fn take_reason(&mut self) -> Reason {
        core::mem::replace(&mut self.reason, Reason::None)
    }

    /// A new tree was installed as `current`: start trusting it again.
    pub fn new_install(&mut self, tries: u8) {
        self.tree = Tree::Current;
        self.tries_left = clamp(tries);
        self.reason = Reason::None;
    }
}

fn clamp(t: u8) -> u8 {
    t.clamp(1, 9)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_known_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn success_keeps_current() {
        let mut s = BootState::fresh(3);
        for _ in 0..10 {
            assert_eq!(s.on_start(3), Outcome::Start(Tree::Current));
            s.mark_good(3);
        }
        assert_eq!(s.reason, Reason::None);
        assert_eq!(s.tries_left, 3);
    }

    #[test]
    fn crash_loop_rolls_back_after_n_failures() {
        let mut s = BootState::fresh(3);
        assert_eq!(s.on_start(3), Outcome::Start(Tree::Current));
        assert_eq!(s.on_start(3), Outcome::Start(Tree::Current));
        assert_eq!(s.on_start(3), Outcome::Start(Tree::Current));
        assert_eq!(s.on_start(3), Outcome::Start(Tree::Previous));
        assert_eq!(s.reason, Reason::TooManyFailures);
        // previous comes up: reason is reported once and previous stays
        s.mark_good(3);
        assert_eq!(s.take_reason(), Reason::TooManyFailures);
        assert_eq!(s.take_reason(), Reason::None);
        assert_eq!(s.on_start(3), Outcome::Start(Tree::Previous));
    }

    #[test]
    fn failing_previous_stops_and_waits() {
        let mut s = BootState::fresh(2);
        assert_eq!(s.on_start(2), Outcome::Start(Tree::Current));
        assert_eq!(s.on_start(2), Outcome::Start(Tree::Current));
        assert_eq!(s.on_start(2), Outcome::Start(Tree::Previous));
        assert_eq!(s.on_start(2), Outcome::Start(Tree::Previous));
        let starts = s.starts;
        for _ in 0..5 {
            assert_eq!(s.on_start(2), Outcome::StopAndWait(Reason::AllTreesFailed));
        }
        assert_eq!(s.starts, starts); // nothing was started
        assert_eq!(s.reason, Reason::AllTreesFailed);
        // the stop survives a store and load
        let mut t = BootState::load(Some(&s.encode()), 2);
        assert_eq!(t, s);
        assert_eq!(t.on_start(2), Outcome::StopAndWait(Reason::AllTreesFailed));
    }

    #[test]
    fn manual_choice_resets_nothing_destructive() {
        let mut s = BootState::fresh(1);
        s.on_start(1);
        s.on_start(1);
        assert_eq!(s.on_start(1), Outcome::StopAndWait(Reason::AllTreesFailed));
        let before = s;
        s.manual_start(); // person picks another OS or an entry
        assert_eq!((s.tree, s.tries_left, s.reason), (before.tree, before.tries_left, before.reason));
        assert_eq!(s.starts, before.starts + 1);
        // still stopped on the next automatic start
        assert_eq!(s.on_start(1), Outcome::StopAndWait(Reason::AllTreesFailed));
    }

    #[test]
    fn success_after_stop_clears_state() {
        let mut s = BootState::fresh(1);
        s.on_start(1);
        s.on_start(1);
        assert_eq!(s.on_start(1), Outcome::StopAndWait(Reason::AllTreesFailed));
        s.manual_start();
        s.mark_good(1); // the chosen system came up
        assert_eq!(s.take_reason(), Reason::AllTreesFailed);
        assert_eq!(s.take_reason(), Reason::None);
        assert_eq!(s.on_start(1), Outcome::Start(Tree::Previous)); // tries are back
        // a new install trusts current again
        let mut s = BootState::fresh(1);
        s.on_start(1);
        s.on_start(1);
        s.on_start(1);
        s.new_install(1);
        assert_eq!(s.on_start(1), Outcome::Start(Tree::Current));
        assert_eq!(s.reason, Reason::None);
    }

    #[test]
    fn two_failures_then_success_does_not_roll_back() {
        let mut s = BootState::fresh(3);
        s.on_start(3);
        s.on_start(3);
        s.mark_good(3);
        assert_eq!(s.on_start(3), Outcome::Start(Tree::Current));
        assert_eq!(s.tries_left, 2);
    }

    #[test]
    fn new_install_resets() {
        let mut s = BootState::fresh(2);
        s.on_start(2);
        s.on_start(2);
        assert_eq!(s.on_start(2), Outcome::Start(Tree::Previous));
        s.new_install(2);
        assert_eq!((s.tree, s.reason, s.tries_left), (Tree::Current, Reason::None, 2));
    }

    #[test]
    fn config_tries_lowered_is_clamped() {
        let mut s = BootState::fresh(9);
        assert_eq!(s.on_start(2), Outcome::Start(Tree::Current));
        assert_eq!(s.tries_left, 1);
        assert_eq!(BootState::fresh(0).tries_left, 1);
        assert_eq!(BootState::fresh(200).tries_left, 9);
    }

    #[test]
    fn round_trip() {
        let mut s = BootState::fresh(3);
        for _ in 0..4 {
            s.on_start(3);
        }
        let b = s.encode();
        assert_eq!(b.len(), STATE_LEN);
        assert_eq!(BootState::decode(&b), Ok(s));
        assert_eq!(BootState::load(Some(&b), 3), s);
    }

    #[test]
    fn corrupted_state() {
        let good = BootState::fresh(3).encode();
        assert_eq!(BootState::decode(&good[..10]), Err(DecodeError::Length));
        let mut b = good;
        b[0] = b'X';
        assert_eq!(BootState::decode(&b), Err(DecodeError::Magic));
        let mut b = good;
        b[5] ^= 1;
        assert_eq!(BootState::decode(&b), Err(DecodeError::Checksum));
        // valid checksum but impossible field
        let mut b = good;
        b[4] = 7;
        let c = crc32(&b[..12]);
        b[12..].copy_from_slice(&c.to_le_bytes());
        assert_eq!(BootState::decode(&b), Err(DecodeError::Field));
        // damaged state: fresh, current, reported, no rollback
        let mut s = BootState::load(Some(&b), 3);
        assert_eq!((s.tree, s.reason, s.tries_left), (Tree::Current, Reason::StateCorrupt, 3));
        assert_eq!(s.on_start(3), Outcome::Start(Tree::Current));
        assert_eq!(s.reason, Reason::StateCorrupt);
        // all-zero and all-ones garbage
        assert_eq!(BootState::load(Some(&[0u8; 16]), 3).reason, Reason::StateCorrupt);
        assert_eq!(BootState::load(Some(&[0xffu8; 16]), 3).reason, Reason::StateCorrupt);
        // missing is not damage
        assert_eq!(BootState::load(None, 3).reason, Reason::None);
    }

    #[test]
    fn every_single_bit_flip_is_detected_or_equal() {
        let mut s = BootState::fresh(3);
        s.on_start(3);
        let good = s.encode();
        for i in 0..STATE_LEN * 8 {
            let mut b = good;
            b[i / 8] ^= 1 << (i % 8);
            assert_ne!(BootState::decode(&b), Ok(s), "bit {i}");
        }
    }
}
