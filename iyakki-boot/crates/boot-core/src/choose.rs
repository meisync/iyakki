// SPDX-License-Identifier: MIT OR Apache-2.0
//! Which item starts: default, one-shot, timeout, hold-a-key recovery.

use crate::assess::Tree;
use crate::config::Entry;
use crate::item::{Action, Item};
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

/// The start the loader chose by itself is `idx`. When the boot state says the previous version is due (`tree`) and that entry names
/// an entry of the previous version (`previous` in the config) that is listed and startable, return that item's index: it starts in
/// place of `idx`. `None`: start `idx` as it is (the current tree, or no previous version on this volume: the entries whose
/// `requires` file is missing are not items, so they are never found here).
pub fn previous_item(entries: &[Entry], items: &[Item], idx: usize, tree: Tree) -> Option<usize> {
    if tree != Tree::Previous {
        return None;
    }
    let Action::Linux(e) = &items.get(idx)?.action else { return None };
    entries.get(*e)?.previous.iter().find_map(|id| items.iter().position(|i| &i.id == id && i.enabled()))
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

    mod fallback {
        use super::*;
        use crate::assess::{BootState, Outcome};
        use crate::config::Config;
        use crate::discover::all_items;

        const CFG: &str = "timeout 0\ntries 2\n\
entry iyakki\n kernel /boot/bzImage\n initrd /boot/initrd\n default\n previous prev prev-oldroot\n\
entry prev\n title IYAKKI (previous version)\n kernel /EFI/p/bzImage\n initrd /EFI/p/initrd\n more\n\
entry safe\n kernel /boot/bzImage\n more\n";

        /// One start of the loader the way `main` does it: spend a try, pick the item.
        fn start(state: &mut BootState, cfg: &Config, items: &[Item]) -> Result<usize, ()> {
            let p = Params { items, default_name: None, timeout_secs: cfg.timeout_secs, oneshot: &none(), keys: KeyState::default(), force_menu: false, wait: false };
            let mut trial = *state;
            let outcome = trial.on_start(cfg.tries);
            let wait = matches!(outcome, Outcome::StopAndWait(_));
            match decide(&Params { wait, ..p }) {
                Decision::Boot { index, .. } => {
                    *state = trial;
                    Ok(previous_item(&cfg.entries, items, index, trial.tree).unwrap_or(index))
                }
                _ => Err(()), // the menu opens and waits
            }
        }

        fn setup(text: &str) -> (Config, Vec<Item>) {
            let mut cfg = Config::parse(text).unwrap();
            // the way main does it: entries whose file is not on the volume are dropped before the items are made
            cfg.drop_unavailable(|_| true, true);
            let items = all_items(&cfg, Vec::new(), false);
            (cfg, items)
        }

        fn id_of(items: &[Item], i: usize) -> &str {
            &items[i].id
        }

        #[test]
        fn a_failed_update_falls_back_to_the_previous_version_by_itself() {
            let (cfg, items) = setup(CFG);
            let mut st = BootState::fresh(cfg.tries);
            st.sync_stamp(1, cfg.tries); // the update left stamp 1
            // the new set never confirms: it gets `tries` starts
            for _ in 0..2 {
                assert_eq!(id_of(&items, start(&mut st, &cfg, &items).unwrap()), "iyakki");
            }
            // the next start is the previous version, without the menu
            let i = start(&mut st, &cfg, &items).unwrap();
            assert_eq!(id_of(&items, i), "prev");
            assert_eq!(st.reason, crate::assess::Reason::TooManyFailures);
            // and it stays there: the previous version, once it is confirmed, keeps being the one that starts
            st.mark_good(cfg.tries);
            assert_eq!(id_of(&items, start(&mut st, &cfg, &items).unwrap()), "prev");
            // another update (new stamp) is tried again
            st.sync_stamp(2, cfg.tries);
            assert_eq!(id_of(&items, start(&mut st, &cfg, &items).unwrap()), "iyakki");
        }

        #[test]
        fn a_confirmed_start_never_falls_back() {
            let (cfg, items) = setup(CFG);
            let mut st = BootState::fresh(cfg.tries);
            for _ in 0..20 {
                assert_eq!(id_of(&items, start(&mut st, &cfg, &items).unwrap()), "iyakki");
                st.mark_good(cfg.tries);
            }
        }

        #[test]
        fn when_the_previous_version_fails_too_the_menu_opens_and_waits() {
            let (cfg, items) = setup(CFG);
            let mut st = BootState::fresh(cfg.tries);
            for _ in 0..4 {
                start(&mut st, &cfg, &items).unwrap();
            }
            assert_eq!(start(&mut st, &cfg, &items), Err(()));
        }

        #[test]
        fn no_previous_version_keeps_the_old_behaviour_and_ends_in_the_menu() {
            // the `requires` file of the previous rows is missing: their entries are dropped, the default has nothing to fall back to
            let mut cfg = Config::parse(CFG).unwrap();
            cfg.entries.retain(|e| e.id != "prev");
            let items = all_items(&cfg, Vec::new(), false);
            let mut st = BootState::fresh(cfg.tries);
            for _ in 0..4 {
                assert_eq!(id_of(&items, start(&mut st, &cfg, &items).unwrap()), "iyakki");
            }
            assert_eq!(start(&mut st, &cfg, &items), Err(()));
        }

        #[test]
        fn the_second_candidate_is_used_when_the_first_is_missing_and_other_entries_are_not_replaced() {
            let (mut cfg, _) = setup(CFG);
            cfg.entries[1].id = "prev-oldroot".into();
            let items = all_items(&cfg, Vec::new(), false);
            assert_eq!(previous_item(&cfg.entries, &items, 0, Tree::Previous).map(|i| id_of(&items, i)), Some("prev-oldroot"));
            assert_eq!(previous_item(&cfg.entries, &items, 0, Tree::Current), None);
            let safe = items.iter().position(|i| i.id == "safe").unwrap();
            assert_eq!(previous_item(&cfg.entries, &items, safe, Tree::Previous), None);
        }
    }

    #[test]
    fn listed_skips_hidden() {
        let mut v = vec![it("a", 1), it("b", 2)];
        v[0].hidden = true;
        assert_eq!(listed(&v), ["b"]);
    }
}
