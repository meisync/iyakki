//! The payload of a driver pack (an `extension` package with `ext_kind: driver`): only data. Firmware blobs, modprobe
//! option lines, module names to load, kernel command line suggestions and, for people who build them elsewhere,
//! prebuilt kernel modules. Nothing in a pack is ever compiled or run on the device.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::b64;

pub const MAX_FILES: usize = 64;
pub const MAX_FILE_BYTES: usize = 3 * 1024 * 1024;
/// One kernel module. The proprietary Wi-Fi module of a Mac mini is about 7.5 MB because it carries the vendor's object file.
pub const MAX_MODULE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_MODULES: usize = 4;

/// When does this pack apply to a machine? Every field that is set must match (AND); a list of matches is an OR.
/// `*` at the start or end of a value is a wildcard; comparison ignores case.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default)]
pub struct Match {
    /// DMI `product_name`, such as `Macmini7,1` or `MacBookPro15,*`.
    pub dmi_product: String,
    /// DMI `sys_vendor`, such as `Apple*`.
    pub dmi_vendor: String,
    /// `vendor:device` PCI ids in hex, `14e4:43a0` or `14e4:*`.
    pub pci: String,
    /// `vendor:product` USB ids in hex.
    pub usb: String,
}

/// Case-insensitive match where `*` stands for any run of characters, anywhere in the pattern (`brcmfmac*.bin`,
/// `MacBookPro15,*`, `*book*`); a pattern without `*` must equal the value.
pub fn glob(pattern: &str, value: &str) -> bool {
    let (p, v) = (pattern.to_ascii_lowercase(), value.to_ascii_lowercase());
    let parts: Vec<&str> = p.split('*').collect();
    if parts.len() == 1 {
        return p == v;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !v.starts_with(first) || v.len() < first.len() + last.len() || !v.ends_with(last) {
        return false;
    }
    let mut rest = &v[first.len()..v.len() - last.len()];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    true
}

impl Match {
    pub fn product(p: &str) -> Match {
        Match { dmi_product: p.into(), ..Default::default() }
    }

    pub fn is_empty(&self) -> bool {
        self.dmi_product.is_empty() && self.dmi_vendor.is_empty() && self.pci.is_empty() && self.usb.is_empty()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default)]
pub struct FwFile {
    /// Relative path under the firmware directory, `brcm/BCM20702A1-0a5c-21e6.hcd`.
    pub path: String,
    /// Base64 of the bytes (inside a package; the copy kept on the data volume has the bytes in a file instead).
    pub data: String,
    /// Optional SHA-256 of the bytes; if present it must match.
    pub sha256: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default)]
pub struct KoFile {
    /// Module name, `wl`.
    pub name: String,
    /// The kernel release it was built for (`6.12.112`), checked against the running kernel and the module itself.
    pub kernel: String,
    pub data: String,
    pub sha256: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default)]
pub struct Pack {
    pub v: u32,
    pub matches: Vec<Match>,
    pub files: Vec<FwFile>,
    /// Lines for `/etc/modprobe.d`: only `options`, `blacklist` and `softdep`.
    pub modprobe: Vec<String>,
    /// Modules to load after the firmware is in place.
    pub load: Vec<String>,
    /// Modules to unload (when loaded) before this pack's own modules are inserted, because they would take the same
    /// hardware (for `wl`: `b43`, `bcma`, `ssb`, `brcmsmac`).
    pub unload: Vec<String>,
    /// Modules to unload and load again when the pack is installed, so a driver that already tried finds its firmware.
    pub reload: Vec<String>,
    /// Kernel command line suggestions: shown to the person, never applied.
    pub cmdline: Vec<String>,
    pub modules: Vec<KoFile>,
    /// What this is and why, in plain words (also shown by "Why").
    pub why: String,
}

pub fn valid_module_name(n: &str) -> bool {
    (1..=56).contains(&n.len()) && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A path that stays inside the firmware directory: relative, plain characters, no `..`, no hidden names.
pub fn safe_rel_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 160
        && !p.starts_with('/')
        && p.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && !c.starts_with('.') && c.chars().all(|ch| ch.is_ascii_alphanumeric() || "-_.,+".contains(ch)))
}

pub fn valid_modprobe_line(l: &str) -> bool {
    let mut w = l.split_whitespace();
    let ok_verb = matches!(w.next(), Some("options" | "blacklist" | "softdep"));
    ok_verb && l.len() <= 200 && l.chars().all(|c| c.is_ascii_alphanumeric() || " _-=,.:".contains(c)) && l.split_whitespace().count() >= 2
}

pub fn valid_cmdline_token(t: &str) -> bool {
    (1..=64).contains(&t.len()) && t.chars().all(|c| c.is_ascii_alphanumeric() || "_.,=:-".contains(c))
}

pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

impl Pack {
    pub fn parse(json: &str) -> Result<Pack, String> {
        let p: Pack = serde_json::from_str(json).map_err(|e| format!("the driver pack is not readable: {e}"))?;
        p.validate()?;
        Ok(p)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Decode and check every blob; the sizes and names are limited so a pack cannot fill the volume or escape its folder.
    pub fn validate(&self) -> Result<(), String> {
        if self.v != 1 {
            return Err("this driver pack has a format version this system does not know".into());
        }
        if self.files.len() > MAX_FILES || self.modules.len() > MAX_MODULES || self.modprobe.len() > 32 || self.load.len() > 16 || self.reload.len() > 16 || self.unload.len() > 16 || self.cmdline.len() > 8 || self.matches.len() > 64 {
            return Err("the driver pack lists too many files, modules or options".into());
        }
        let mut total = 0usize;
        let mut seen = std::collections::BTreeSet::new();
        for f in &self.files {
            if !safe_rel_path(&f.path) {
                return Err(format!("`{}` is not an allowed firmware path (relative, plain letters, no `..`)", f.path));
            }
            if !seen.insert(f.path.clone()) {
                return Err(format!("`{}` is listed twice", f.path));
            }
            let bytes = b64::decode(&f.data).ok_or_else(|| format!("`{}` is not valid base64", f.path))?;
            if bytes.is_empty() || bytes.len() > MAX_FILE_BYTES {
                return Err(format!("`{}` is empty or larger than {} MiB", f.path, MAX_FILE_BYTES >> 20));
            }
            if !f.sha256.is_empty() && !f.sha256.eq_ignore_ascii_case(&sha256_hex(&bytes)) {
                return Err(format!("`{}` does not match its checksum", f.path));
            }
            total += bytes.len();
        }
        for m in &self.modules {
            if !valid_module_name(&m.name) {
                return Err(format!("`{}` is not a module name", m.name));
            }
            if m.kernel.is_empty() || m.kernel.len() > 64 || !m.kernel.chars().all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c)) {
                return Err(format!("module {} must name the kernel release it was built for", m.name));
            }
            let bytes = b64::decode(&m.data).ok_or_else(|| format!("module {} is not valid base64", m.name))?;
            if bytes.len() < 64 || bytes.len() > MAX_MODULE_BYTES {
                return Err(format!("module {} is not a plausible module file", m.name));
            }
            if !m.sha256.is_empty() && !m.sha256.eq_ignore_ascii_case(&sha256_hex(&bytes)) {
                return Err(format!("module {} does not match its checksum", m.name));
            }
            total += bytes.len();
        }
        if total > MAX_TOTAL_BYTES {
            return Err(format!("the driver pack is larger than {} MiB", MAX_TOTAL_BYTES >> 20));
        }
        if let Some(l) = self.modprobe.iter().find(|l| !valid_modprobe_line(l)) {
            return Err(format!("`{l}`: only `options`, `blacklist` and `softdep` lines of plain words are allowed (nothing that runs a command)"));
        }
        if let Some(n) = self.load.iter().chain(&self.reload).chain(&self.unload).find(|n| !valid_module_name(n)) {
            return Err(format!("`{n}` is not a module name"));
        }
        if let Some(t) = self.cmdline.iter().find(|t| !valid_cmdline_token(t)) {
            return Err(format!("`{t}` is not an allowed kernel option"));
        }
        if self.why.chars().count() > 2000 {
            return Err("the explanation is longer than 2000 characters".into());
        }
        Ok(())
    }

    /// Does the pack have code the kernel will run (modules)? Those need the strongest trust.
    pub fn has_modules(&self) -> bool {
        !self.modules.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fw(path: &str, bytes: &[u8]) -> FwFile {
        FwFile { path: path.into(), data: b64::encode(bytes), sha256: String::new() }
    }

    fn base() -> Pack {
        Pack { v: 1, files: vec![fw("brcm/a.hcd", b"abc")], modprobe: vec!["options hid_apple fnmode=2".into()], load: vec!["btusb".into()], cmdline: vec!["acpi_osi=Darwin".into()], ..Default::default() }
    }

    #[test]
    fn globs_match_prefix_suffix_contains_and_exact_ignoring_case() {
        assert!(glob("MacBookPro15,*", "MacBookPro15,2") && glob("*book*", "MacBookPro15,2") && glob("*,1", "Macmini7,1"));
        assert!(glob("Macmini7,1", "macmini7,1") && !glob("Macmini7,1", "Macmini7,10") && !glob("MacBookPro15,*", "MacBookPro16,1"));
        assert!(glob("*", "anything"));
        // a star in the middle (the folder names a firmware entry accepts)
        assert!(glob("brcmfmac*.bin", "brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.bin"));
        assert!(glob("brcmbt4377*.ptb", "brcmbt4377b3-apple,formosa-u.ptb") && !glob("brcmbt4377*.ptb", "brcmbt4377b3-apple,formosa-u.bin"));
        assert!(!glob("brcmfmac*.bin", "brcmfmac.txt") && !glob("ab*ba", "aba") && glob("a*b*c", "a-x-b-y-c") && !glob("a*b*c", "a-c-b"));
    }

    #[test]
    fn a_good_pack_validates_and_round_trips() {
        let p = base();
        assert!(p.validate().is_ok());
        assert_eq!(Pack::parse(&p.to_json()).unwrap(), p);
    }

    #[test]
    fn paths_that_could_escape_or_hide_are_refused() {
        for bad in ["../x", "/etc/passwd", "a//b", "a/./b", ".hidden", "a/.b", "", "a b", "a;b", "brcm/../../x"] {
            let mut p = base();
            p.files[0].path = bad.into();
            assert!(p.validate().is_err(), "{bad:?}");
        }
        assert!(safe_rel_path("facetimehd/firmware.bin") && safe_rel_path("brcm/brcmfmac43602-pcie.bin"));
    }

    #[test]
    fn modprobe_lines_that_run_commands_are_refused() {
        for bad in ["install btusb /bin/sh", "remove x /bin/true", "options x y; reboot", "options x `id`", "options x $(id)", "alias a b", "options"] {
            let mut p = base();
            p.modprobe = vec![bad.into()];
            assert!(p.validate().is_err(), "{bad}");
        }
        for ok in ["options hid_apple fnmode=2 iso_layout=0", "blacklist b43", "softdep brcmfmac pre: cfg80211"] {
            assert!(valid_modprobe_line(ok), "{ok}");
        }
    }

    #[test]
    fn checksums_sizes_counts_and_cmdline_tokens_are_enforced() {
        let mut p = base();
        p.files[0].sha256 = sha256_hex(b"abc");
        assert!(p.validate().is_ok());
        p.files[0].sha256 = sha256_hex(b"abd");
        assert!(p.validate().unwrap_err().contains("checksum"));
        let mut p = base();
        p.cmdline = vec!["a b".into()];
        assert!(p.validate().is_err());
        p.cmdline = vec!["quiet;rm".into()];
        assert!(p.validate().is_err());
        let mut p = base();
        p.files = (0..65).map(|i| fw(&format!("a/f{i}"), b"x")).collect();
        assert!(p.validate().is_err());
        let mut p = base();
        p.files[0].data = "!!!".into();
        assert!(p.validate().is_err());
        let mut p = base();
        p.files = vec![fw("a/big", &vec![1u8; MAX_FILE_BYTES + 1])];
        assert!(p.validate().is_err());
        let mut p = base();
        p.v = 2;
        assert!(p.validate().is_err());
    }
}
