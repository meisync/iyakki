//! The fast-start ("lean boot") files are left behind when a boot volume is copied to a disk.
//!
//! `dist/buildroot/lean-install.sh` adds the fast-start menu entries (a block between `# BEGIN lean-boot` and `# END lean-boot`
//! in `boot/grub/grub.cfg`, managed by `dist/buildroot/lean-sync.py`) and `boot/initrd-lean` to a stick whose data volume holds the
//! lean tree. A new install has no lean tree and no data volume yet, so a copied block would make fast start the first entry and
//! the first boot would wait for a data volume that does not exist. The installer therefore copies the boot volume without the
//! block and without the lean-only files. [`LeanFree`] is the [`Editor`] that does it while the volume is cloned.

use crate::fatclone::{Edit, Editor};

/// The markers of the block, exactly as `lean-sync.py` writes and finds them (`B1`, `B2`).
pub const BEGIN: &str = "# BEGIN lean-boot";
pub const END: &str = "# END lean-boot";

/// The grub.cfg without the lean block (and the one blank line after it): the same algorithm as `lean-sync.py grubstrip`.
/// Unchanged when there is no block.
pub fn strip_lean_block(cfg: &str) -> String {
    let (Some(a), true) = (cfg.find(BEGIN), cfg.contains(END)) else { return cfg.to_string() };
    let Some(e) = cfg[a..].find(END).map(|i| a + i) else { return cfg.to_string() };
    // the block ends at the end of the line that holds the END marker
    let mut b = cfg[e..].find('\n').map_or(cfg.len(), |i| e + i + 1);
    if cfg[b..].starts_with('\n') {
        b += 1;
    }
    format!("{}{}", &cfg[..a], &cfg[b..])
}

/// The menu text for a disk that has no lean tree: the block is removed and the default entry is the normal one, "IYAKKI".
pub fn clean_grub_cfg(cfg: &str) -> String {
    let base = strip_lean_block(cfg);
    let first = base.lines().find_map(|l| l.trim_start().strip_prefix("menuentry \"")).and_then(|r| r.split('"').next());
    let has_normal = base.contains("menuentry \"IYAKKI\"");
    if first == Some("IYAKKI") || !has_normal {
        return base;
    }
    // the first entry is not the normal one (the template is unexpected): name the default instead of relying on position
    let mut done = false;
    let mut out: Vec<String> = base
        .lines()
        .map(|l| {
            if !done && l.trim_start().starts_with("set default=") {
                done = true;
                "set default=\"IYAKKI\"".to_string()
            } else {
                l.to_string()
            }
        })
        .collect();
    if !done {
        out.insert(0, "set default=\"IYAKKI\"".to_string());
    }
    let mut s = out.join("\n");
    if base.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Files that only the fast start uses or leaves behind (path as the volume stores it, any case).
pub fn is_lean_only(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    let name = p.rsplit('/').next().unwrap_or("");
    name.starts_with("initrd-lean") || name == "stage1.log" || name == "iyakki-lean-failed" || name.ends_with(".before-lean")
}

/// Does this boot volume carry anything the installer would leave out? (Only then the copy goes through the editing cloner.)
pub fn needs_cleaning(paths: &[String], grub_cfg: Option<&str>) -> bool {
    paths.iter().any(|p| is_lean_only(p)) || grub_cfg.is_some_and(|c| c.contains(BEGIN))
}

/// The [`Editor`]: drops the lean-only files and rewrites `boot/grub/grub.cfg`.
pub struct LeanFree {
    pub dropped: Vec<String>,
    pub menu_changed: bool,
}

impl LeanFree {
    pub fn new() -> LeanFree {
        LeanFree { dropped: vec![], menu_changed: false }
    }
}

impl Default for LeanFree {
    fn default() -> Self {
        Self::new()
    }
}

fn is_grub_cfg(path: &str) -> bool {
    path.eq_ignore_ascii_case("/boot/grub/grub.cfg")
}

impl Editor for LeanFree {
    fn decide(&mut self, path: &str) -> Edit {
        if is_lean_only(path) {
            self.dropped.push(path.to_string());
            Edit::Skip
        } else if is_grub_cfg(path) {
            Edit::Rewrite
        } else {
            Edit::Keep
        }
    }

    fn rewrite(&mut self, path: &str, data: Vec<u8>) -> Vec<u8> {
        if !is_grub_cfg(path) {
            return data;
        }
        let Ok(text) = String::from_utf8(data.clone()) else { return data };
        let clean = clean_grub_cfg(&text);
        self.menu_changed = clean != text;
        clean.into_bytes()
    }
}

/// The sample menu of the tests, for the cloner's test too.
#[cfg(test)]
pub(crate) fn tests_sample() -> &'static str {
    tests::SAMPLE
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of dist/buildroot/board/iyakki/grub.cfg after lean-install.sh (the block text is the real one).
    pub(crate) const SAMPLE: &str = "set default=\"0\"\nset timeout=\"3\"\n\n# BEGIN lean-boot (this block is managed by dist/buildroot/lean-install.sh and lean-sync.py; the markers are how they find the block again)\ninsmod test\nif [ -f /boot/initrd-lean ]; then\n\tif [ -f $prefix/iyakki-lean-failed ]; then\n\t\tset default=\"IYAKKI\"\n\tfi\n\tmenuentry \"IYAKKI - fast start (lean boot)\" {\n\t\tlinux /boot/bzImage quiet\n\t\tinitrd /boot/initrd-lean\n\t}\nfi\n# END lean-boot\n\nmenuentry \"IYAKKI\" {\n\tlinux /boot/bzImage quiet\n\tinitrd /boot/initrd\n}\n\nmenuentry \"IYAKKI - Install to a disk\" {\n}\n";

    #[test]
    fn the_lean_block_is_taken_out_with_the_same_markers_as_lean_sync() {
        // the markers are the ones lean-sync.py defines (B1, B2) and the template carries
        for f in ["../../dist/buildroot/lean-sync.py", "../../dist/buildroot/board/iyakki/grub.cfg"] {
            if let Ok(t) = std::fs::read_to_string(f) {
                assert!(t.contains(BEGIN) && t.contains(END), "{f}");
            }
        }
        let s = strip_lean_block(SAMPLE);
        assert!(!s.contains("lean") && !s.contains("fast start") && !s.contains("insmod test"), "{s}");
        assert!(s.contains("menuentry \"IYAKKI\" {") && s.contains("Install to a disk"));
        // the blank line after the block goes with it, the one before stays: nothing else changes
        assert_eq!(s, SAMPLE.replace(&SAMPLE[SAMPLE.find(BEGIN).unwrap()..SAMPLE.find("menuentry \"IYAKKI\" {").unwrap()], ""));
        // no block: unchanged; stripping twice changes nothing more
        assert_eq!(strip_lean_block("menuentry \"IYAKKI\" {\n}\n"), "menuentry \"IYAKKI\" {\n}\n");
        assert_eq!(strip_lean_block(&s), s);
        // a begin marker without an end marker is left alone
        assert_eq!(strip_lean_block("# BEGIN lean-boot\nx\n"), "# BEGIN lean-boot\nx\n");
    }

    #[test]
    fn the_default_entry_of_the_cleaned_menu_is_the_normal_iyakki_entry() {
        let c = clean_grub_cfg(SAMPLE);
        assert!(c.starts_with("set default=\"0\""));
        let entries: Vec<&str> = c.lines().filter_map(|l| l.strip_prefix("menuentry \"")).collect();
        assert_eq!(entries[0], "IYAKKI\" {", "{entries:?}");
        // a template whose first entry is not the normal one gets the default named
        let odd = "set default=\"0\"\nmenuentry \"Other\" {\n}\nmenuentry \"IYAKKI\" {\n}\n";
        assert!(clean_grub_cfg(odd).starts_with("set default=\"IYAKKI\"\n"));
        let plain = "set default=\"0\"\nmenuentry \"IYAKKI\" {\n}\n";
        assert_eq!(clean_grub_cfg(plain), plain);
    }

    #[test]
    fn only_fast_start_files_are_dropped() {
        for p in ["/boot/initrd-lean", "/BOOT/INITRD-LEAN", "/boot/initrd-lean.tmp", "/boot/stage1.log", "/boot/grub/grub.cfg.before-lean", "/boot/grub/iyakki-lean-failed"] {
            assert!(is_lean_only(p), "{p}");
        }
        for p in ["/boot/initrd", "/boot/bzImage", "/boot/grub/grub.cfg", "/EFI/BOOT/BOOTX64.EFI", "/boot/grub/fonts/unicode.pf2"] {
            assert!(!is_lean_only(p), "{p}");
        }
        assert!(needs_cleaning(&["/boot/stage1.log".into()], None));
        assert!(needs_cleaning(&[], Some(SAMPLE)));
        assert!(!needs_cleaning(&["/boot/initrd".into()], Some("menuentry \"IYAKKI\" {\n}\n")));
    }

    #[test]
    fn the_editor_drops_files_and_rewrites_only_the_menu() {
        let mut e = LeanFree::new();
        assert_eq!(e.decide("/boot/initrd-lean"), Edit::Skip);
        assert_eq!(e.decide("/boot/initrd"), Edit::Keep);
        assert_eq!(e.decide("/boot/grub/GRUB.CFG"), Edit::Rewrite);
        let out = e.rewrite("/boot/grub/GRUB.CFG", SAMPLE.as_bytes().to_vec());
        assert!(e.menu_changed && !String::from_utf8(out).unwrap().contains("lean-boot"));
        assert_eq!(e.dropped, vec!["/boot/initrd-lean".to_string()]);
    }
}
