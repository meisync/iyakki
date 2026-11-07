// SPDX-License-Identifier: MIT OR Apache-2.0
//! Which item starts: default, one-shot, timeout, hold-a-key recovery.

use crate::item::Item;
use alloc::string::String;
use alloc::vec::Vec;

/// A key that can be held at start to pick an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldKey {
    /// A printable ASCII character, lower case.
    Char(char),
    /// Shift.
    Shift,
    /// Escape.
    Esc,
    /// Space bar.
    Space,
    /// A finger on the touch screen.
    Touch,
}

impl HoldKey {
    /// Parse `shift`, `esc`, `space`, `touch` or one character.
    pub fn parse(s: &str) -> Option<HoldKey> {
        match s {
            "shift" => Some(HoldKey::Shift),
            "esc" => Some(HoldKey::Esc),
            "space" => Some(HoldKey::Space),
            "touch" => Some(HoldKey::Touch),
            _ => {
                let mut it = s.chars();
                match (it.next(), it.next()) {
                    (Some(c), None) if c.is_ascii_alphanumeric() => Some(HoldKey::Char(c.to_ascii_lowercase())),
                    _ => None,
                }
            }
        }
    }
}

/// Which keys are down right now (read by the front end).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyState {
    /// Shift is down.
    pub shift: bool,
    /// Escape is down.
    pub esc: bool,
    /// Space is down.
    pub space: bool,
    /// The touch screen is touched.
    pub touch: bool,
    chars: u128,
}

impl KeyState {
    /// Mark a character key as down (ASCII only).
    pub fn press_char(&mut self, c: char) {
        if c.is_ascii() {
            self.chars |= 1u128 << (c.to_ascii_lowercase() as u32);
        }
    }

    /// Is this key down?
    pub fn is_held(&self, k: HoldKey) -> bool {
        match k {
            HoldKey::Shift => self.shift,
            HoldKey::Esc => self.esc,
            HoldKey::Space => self.space,
            HoldKey::Touch => self.touch,
            HoldKey::Char(c) => c.is_ascii() && self.chars & (1u128 << (c as u32)) != 0,
        }
    }

    /// Does the person ask for the menu (Esc, Space or Shift held)?
    pub fn wants_menu(&self) -> bool {
        self.esc || self.space || self.shift
    }
}

/// Sort by `order`, keeping the given order for equal values.
pub fn sort_items(items: &mut [Item]) {
    items.sort_by_key(|i| i.order); // stable
}

/// Index of the first enabled item whose hold key is held.
pub fn hold_pick(items: &[Item], keys: &KeyState) -> Option<usize> {
    items.iter().position(|i| i.enabled() && i.hold.is_some_and(|k| keys.is_held(k)))
}

/// Default item: the one marked default, else the named one, else the first enabled and listed, else the first enabled.
pub fn default_index(items: &[Item], named: Option<&str>) -> Option<usize> {
    items
        .iter()
        .position(|i| i.default && i.enabled())
        .or_else(|| named.and_then(|n| items.iter().position(|i| i.id == n && i.enabled())))
        .or_else(|| items.iter().position(|i| i.enabled() && !i.hidden))
        .or_else(|| items.iter().position(|i| i.enabled()))
}

/// The one-shot variable: an entry id to boot once. Read it, then always clear it before booting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OneShot {
    /// Index of the item to boot, if the stored id names a startable item.
    pub index: Option<usize>,
    /// The variable was set and must be deleted now (even when it named nothing usable).
    pub clear: bool,
}

/// Resolve the stored variable text (`None` when unset).
pub fn resolve_oneshot(stored: Option<&str>, items: &[Item]) -> OneShot {
    match stored.map(str::trim) {
        None => OneShot { index: None, clear: false },
        Some(id) => OneShot { index: items.iter().position(|i| i.id == id && i.enabled()), clear: true },
    }
}

/// What the start does first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Start this item at once, no menu.
    Boot { index: usize, why: Why },
    /// Show the menu with this item selected; `timeout_secs` is `None` when it waits for ever.
    Menu { selected: usize, timeout_secs: Option<u32> },
    /// Nothing can be started.
    Nothing,
}

/// Reason for an immediate start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// A hold key was down.
    HoldKey,
    /// The one-shot variable.
    OneShot,
    /// Default with timeout 0 and no key.
    Default,
}

/// Inputs of the start decision.
#[derive(Debug, Clone)]
pub struct Params<'a> {
    /// Ordered items.
    pub items: &'a [Item],
    /// Default named in the config.
    pub default_name: Option<&'a str>,
    /// Config timeout in seconds; 0 means start at once.
    pub timeout_secs: u32,
    /// Resolved one-shot.
    pub oneshot: &'a OneShot,
    /// Keys down at start.
    pub keys: KeyState,
    /// Always show the menu (for example after a rollback was reported).
    pub force_menu: bool,
    /// Boot assessment said stop (`Outcome::StopAndWait`): open the menu with no timeout and start nothing by itself.
    /// A held hold-key still starts its item (the person is there and asked); a stored one-shot is ignored.
    pub wait: bool,
}

/// Decide. Priority: hold key, one-shot, menu request, default.
pub fn decide(p: &Params<'_>) -> Decision {
    let Some(def) = default_index(p.items, p.default_name) else { return Decision::Nothing };
    if let Some(index) = hold_pick(p.items, &p.keys) {
        return Decision::Boot { index, why: Why::HoldKey };
    }
    if p.wait {
        return Decision::Menu { selected: def, timeout_secs: None };
    }
    if let Some(index) = p.oneshot.index {
        return Decision::Boot { index, why: Why::OneShot };
    }
    if p.keys.wants_menu() || p.force_menu {
        return Decision::Menu { selected: def, timeout_secs: None };
    }
    if p.timeout_secs == 0 {
        return Decision::Boot { index: def, why: Why::Default };
    }
    Decision::Menu { selected: def, timeout_secs: Some(p.timeout_secs) }
}

/// Ids of the listed (not hidden) items, for tests and the front end.
pub fn listed(items: &[Item]) -> Vec<String> {
    items.iter().filter(|i| !i.hidden).map(|i| i.id.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::{Action, Disabled};

    fn it(id: &str, order: i32) -> Item {
        Item {
            id: id.into(),
            title: id.into(),
            hidden: false,
            order,
            default: false,
            hold: None,
            more: false,
            disabled: None,
            action: Action::Linux(0),
        }
    }

    fn none() -> OneShot {
        OneShot { index: None, clear: false }
    }

    fn p<'a>(items: &'a [Item], os: &'a OneShot) -> Params<'a> {
        Params { items, default_name: None, timeout_secs: 5, oneshot: os, keys: KeyState::default(), force_menu: false, wait: false }
    }

    #[test]
    fn ordering_is_stable() {
        let mut v = vec![it("c", 5), it("a", 1), it("b", 1)];
        sort_items(&mut v);
        assert_eq!(v.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
    }

    #[test]
    fn default_rules() {
        let mut v = vec![it("a", 1), it("b", 2), it("c", 3)];
        assert_eq!(default_index(&v, None), Some(0));
        assert_eq!(default_index(&v, Some("c")), Some(2));
        v[1].default = true;
        assert_eq!(default_index(&v, Some("c")), Some(1));
        v[1].disabled = Some(Disabled::UnsignedSecureBoot);
        assert_eq!(default_index(&v, Some("c")), Some(2));
        v[0].hidden = true;
        v[2].disabled = Some(Disabled::UnsignedSecureBoot);
        assert_eq!(default_index(&v, None), Some(0)); // only a hidden one is left
        v[0].disabled = Some(Disabled::UnsignedSecureBoot);
        assert_eq!(default_index(&v, None), None);
    }

    #[test]
    fn timeout_and_immediate() {
        let v = vec![it("a", 1), it("b", 2)];
        let os = none();
        assert_eq!(decide(&p(&v, &os)), Decision::Menu { selected: 0, timeout_secs: Some(5) });
        let mut q = p(&v, &os);
        q.timeout_secs = 0;
        assert_eq!(decide(&q), Decision::Boot { index: 0, why: Why::Default });
        q.keys.esc = true;
        assert_eq!(decide(&q), Decision::Menu { selected: 0, timeout_secs: None });
        q.keys.esc = false;
        q.force_menu = true;
        assert_eq!(decide(&q), Decision::Menu { selected: 0, timeout_secs: None });
    }

    #[test]
    fn wait_disables_timeout_and_automatic_start() {
        let v = vec![it("a", 1), it("b", 2)];
        let os = OneShot { index: Some(1), clear: true };
        let mut q = p(&v, &os);
        q.wait = true;
        assert_eq!(decide(&q), Decision::Menu { selected: 0, timeout_secs: None });
        q.timeout_secs = 0; // even "start at once" waits
        assert_eq!(decide(&q), Decision::Menu { selected: 0, timeout_secs: None });
        let mut v = v;
        v[1].hold = Some(HoldKey::Char('r'));
        let mut q = p(&v, &os);
        q.wait = true;
        q.keys.press_char('r');
        assert_eq!(decide(&q), Decision::Boot { index: 1, why: Why::HoldKey });
        assert_eq!(decide(&Params { items: &[], ..q }), Decision::Nothing);
    }

    #[test]
    fn oneshot_resolution() {
        let mut v = vec![it("a", 1), it("b", 2)];
        assert_eq!(resolve_oneshot(None, &v), OneShot { index: None, clear: false });
        assert_eq!(resolve_oneshot(Some("b\n"), &v), OneShot { index: Some(1), clear: true });
        assert_eq!(resolve_oneshot(Some("zz"), &v), OneShot { index: None, clear: true });
        v[1].disabled = Some(Disabled::UnsignedSecureBoot);
        assert_eq!(resolve_oneshot(Some("b"), &v), OneShot { index: None, clear: true });
        let v = vec![it("a", 1), it("b", 2)];
        let os = resolve_oneshot(Some("b"), &v);
        assert_eq!(decide(&p(&v, &os)), Decision::Boot { index: 1, why: Why::OneShot });
    }

    #[test]
    fn hold_key_wins_over_oneshot() {
        let mut v = vec![it("a", 1), it("rec", 2), it("b", 3)];
        v[1].hold = Some(HoldKey::Char('r'));
        let os = OneShot { index: Some(2), clear: true };
        let mut q = p(&v, &os);
        q.keys.press_char('R');
        assert_eq!(decide(&q), Decision::Boot { index: 1, why: Why::HoldKey });
        q.keys = KeyState::default();
        assert_eq!(decide(&q), Decision::Boot { index: 2, why: Why::OneShot });
    }

    #[test]
    fn hold_key_ignores_disabled_and_other_keys() {
        let mut v = vec![it("a", 1), it("rec", 2)];
        v[1].hold = Some(HoldKey::Touch);
        let mut keys = KeyState { shift: true, ..Default::default() };
        assert_eq!(hold_pick(&v, &keys), None);
        keys.touch = true;
        assert_eq!(hold_pick(&v, &keys), Some(1));
        v[1].disabled = Some(Disabled::UnsignedSecureBoot);
        assert_eq!(hold_pick(&v, &keys), None);
    }

    #[test]
    fn nothing_to_boot() {
        let os = none();
        assert_eq!(decide(&p(&[], &os)), Decision::Nothing);
    }

    #[test]
    fn hold_key_parse() {
        assert_eq!(HoldKey::parse("R"), Some(HoldKey::Char('r')));
        assert_eq!(HoldKey::parse("shift"), Some(HoldKey::Shift));
        assert_eq!(HoldKey::parse("touch"), Some(HoldKey::Touch));
        assert_eq!(HoldKey::parse("ab"), None);
        assert_eq!(HoldKey::parse(""), None);
        assert_eq!(HoldKey::parse("!"), None);
    }

    #[test]
    fn listed_skips_hidden() {
        let mut v = vec![it("a", 1), it("b", 2)];
        v[0].hidden = true;
        assert_eq!(listed(&v), ["b"]);
    }
}
