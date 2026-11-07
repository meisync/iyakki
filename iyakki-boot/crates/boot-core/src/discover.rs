// SPDX-License-Identifier: MIT OR Apache-2.0
//! Model of other systems found on the machine. The scan itself lives in the front ends; this is the data and the rules.

use crate::choose::sort_items;
use crate::config::{Config, Kind, Override, DEFAULT_ORDER, MAX_TITLE};
use crate::item::{Action, Disabled, Item};
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// `order` of discovered items without an override: after configured entries.
pub const DISCOVERED_ORDER: i32 = 1000;

/// How to start a discovered or configured chain item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// An EFI program on the boot volume itself.
    BootVolume {
        /// Path on the volume.
        path: String,
    },
    /// An EFI program on a partition of a disk (disk and partition numbers as the scan counted them).
    EfiChain {
        /// Disk number.
        disk: u8,
        /// Partition number.
        part: u8,
        /// Path on the partition.
        path: String,
    },
    /// A BIOS boot sector.
    BiosChain {
        /// Disk number.
        disk: u8,
        /// Partition number (0 for the disk's own boot sector).
        part: u8,
    },
}

/// Where the scan found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A well-known path on an EFI system partition.
    EspScan,
    /// A firmware boot entry (`Boot####`).
    FirmwareEntry(u16),
    /// A boot sector with the `0x55AA` mark.
    BootSector,
}

/// Why a loader counts as unsigned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsignedReason {
    /// No signature at all.
    NoSignature,
    /// A signature that no trusted key verifies.
    BadSignature,
    /// Not checked (BIOS boot sectors carry no signature).
    NotApplicable,
}

/// Signature state of the loader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// Signed by a trusted key.
    Signed,
    /// Not signed, with the reason.
    Unsigned(UnsignedReason),
}

/// One found system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    /// How to start it.
    pub target: Target,
    /// Where it came from.
    pub source: Source,
    /// Signature state.
    pub trust: Trust,
    /// Text of an os-release file next to it, when one could be read.
    pub os_release: Option<String>,
}

impl Discovered {
    /// Stable id, without spaces, used by config overrides and the one-shot variable.
    pub fn id(&self) -> String {
        match &self.target {
            Target::BootVolume { path } => format!("efi:boot:{}", clean(path)),
            Target::EfiChain { disk, part, path } => format!("efi:{disk}:{part}:{}", clean(path)),
            Target::BiosChain { disk, part } => format!("bios:{disk}:{part}"),
        }
    }

    /// Name derived from the path and os-release.
    pub fn title(&self) -> String {
        match &self.target {
            Target::BootVolume { path } | Target::EfiChain { path, .. } => derive_name(path, self.os_release.as_deref()),
            Target::BiosChain { disk, part } => match self.os_release.as_deref().and_then(os_release_name) {
                Some(n) => cut(&n),
                None if *part == 0 => format!("Disk {disk}"),
                None => format!("Disk {disk} partition {part}"),
            },
        }
    }

    fn location(&self) -> String {
        match &self.target {
            Target::BootVolume { .. } => "boot volume".to_string(),
            Target::EfiChain { disk, part, .. } | Target::BiosChain { disk, part } => format!("disk {disk} part {part}"),
        }
    }
}

fn clean(path: &str) -> String {
    path.chars()
        .map(|c| match c.to_ascii_lowercase() {
            c @ ('a'..='z' | '0'..='9' | '.' | '/' | '-') => c,
            _ => '_',
        })
        .collect()
}

/// Remove control characters and cut to the title limit.
fn cut(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(MAX_TITLE).collect::<String>().trim().to_string()
}

/// `PRETTY_NAME`, else `NAME` (with `VERSION`), from os-release text.
pub fn os_release_name(text: &str) -> Option<String> {
    let (mut pretty, mut name, mut version) = (None, None, None);
    for line in text.lines() {
        let Some((k, v)) = line.trim().split_once('=') else { continue };
        let v = v.trim().trim_matches(|c| c == '"' || c == '\'').trim();
        if v.is_empty() {
            continue;
        }
        match k {
            "PRETTY_NAME" => pretty = Some(v.to_string()),
            "NAME" => name = Some(v.to_string()),
            "VERSION" => version = Some(v.to_string()),
            _ => {}
        }
    }
    let n = pretty.or_else(|| name.map(|n| match version {
        Some(v) => format!("{n} {v}"),
        None => n,
    }))?;
    let n = cut(&n);
    (!n.is_empty()).then_some(n)
}

/// Name for an EFI loader path: os-release first, then well-known loaders, then the directory name.
pub fn derive_name(path: &str, os_release: Option<&str>) -> String {
    if let Some(n) = os_release.and_then(os_release_name) {
        return n;
    }
    let lower = path.to_ascii_lowercase().replace('\\', "/");
    let parts: Vec<&str> = lower.split('/').filter(|p| !p.is_empty()).collect();
    if lower.ends_with("microsoft/boot/bootmgfw.efi") {
        return "Windows Boot Manager".to_string();
    }
    if lower.ends_with("coreservices/boot.efi") {
        return "macOS".to_string();
    }
    if let [.., dir, file] = parts.as_slice() {
        if *dir == "boot" && file.starts_with("boot") {
            return "Fallback boot loader".to_string();
        }
        if *dir != "efi" && *dir != "boot" {
            let mut c = dir.chars();
            let name: String = c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default();
            return cut(&name);
        }
    }
    let stem = parts.last().map(|f| f.trim_end_matches(".efi")).unwrap_or("");
    if stem.is_empty() { "Unknown system".to_string() } else { cut(stem) }
}

/// Merge scan results with the person's overrides into menu items.
///
/// Duplicates (same id) keep the first. Order: override `order`, else `DISCOVERED_ORDER`; ties by lower-case title, then id,
/// so the result does not depend on scan order. Equal titles get the location appended. Unsigned loaders are greyed out
/// when Secure Boot is on.
pub fn merge(found: Vec<Discovered>, overrides: &[Override], secure_boot: bool) -> Vec<Item> {
    let mut seen = BTreeSet::new();
    let mut list: Vec<(String, Discovered)> = Vec::new();
    for d in found {
        let id = d.id();
        if seen.insert(id.clone()) {
            list.push((id, d));
        }
    }
    list.sort_by_cached_key(|(id, d)| (d.title().to_lowercase(), id.clone()));
    let titles: Vec<String> = list.iter().map(|(_, d)| d.title()).collect();
    let mut items = Vec::new();
    for (i, (id, d)) in list.into_iter().enumerate() {
        let ov = overrides.iter().find(|o| o.id == id);
        let renamed = ov.and_then(|o| o.name.clone());
        let mut title = renamed.clone().unwrap_or_else(|| titles[i].clone());
        if renamed.is_none() && titles.iter().filter(|t| **t == titles[i]).count() > 1 {
            title = cut(&format!("{} ({})", titles[i], d.location()));
        }
        let disabled = match d.trust {
            Trust::Unsigned(_) if secure_boot => Some(Disabled::UnsignedSecureBoot),
            _ => None,
        };
        items.push(Item {
            id,
            title,
            hidden: ov.and_then(|o| o.hidden).unwrap_or(false),
            order: ov.and_then(|o| o.order).unwrap_or(DISCOVERED_ORDER),
            default: ov.is_some_and(|o| o.default),
            hold: None,
            more: true, // other systems live under "More options"
            disabled,
            action: Action::Chain(d.target),
        });
    }
    sort_items(&mut items);
    items
}

/// All items: configured entries first (by `order`), then discovered ones when `scan` is on.
pub fn all_items(cfg: &Config, found: Vec<Discovered>, secure_boot: bool) -> Vec<Item> {
    let mut items: Vec<Item> = cfg
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| Item {
            id: e.id.clone(),
            title: e.title.clone(),
            hidden: e.hidden,
            order: e.order,
            default: e.default,
            hold: e.hold,
            more: e.more,
            disabled: None,
            action: match e.kind {
                Kind::Firmware => Action::Firmware,
                Kind::Linux => Action::Linux(i),
                Kind::Chain => Action::Chain(Target::BootVolume { path: e.kernel.clone() }),
            },
        })
        .collect();
    if cfg.scan {
        items.extend(merge(found, &cfg.overrides, secure_boot));
    }
    sort_items(&mut items);
    debug_assert!(items.iter().all(|i| i.order >= -100_000) && DEFAULT_ORDER < DISCOVERED_ORDER);
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn efi(disk: u8, part: u8, path: &str, trust: Trust) -> Discovered {
        Discovered { target: Target::EfiChain { disk, part, path: path.into() }, source: Source::EspScan, trust, os_release: None }
    }

    const UNS: Trust = Trust::Unsigned(UnsignedReason::NoSignature);

    #[test]
    fn names_from_paths() {
        assert_eq!(derive_name("/EFI/Microsoft/Boot/bootmgfw.efi", None), "Windows Boot Manager");
        assert_eq!(derive_name("\\EFI\\Microsoft\\Boot\\bootmgfw.efi", None), "Windows Boot Manager");
        assert_eq!(derive_name("/EFI/ubuntu/shimx64.efi", None), "Ubuntu");
        assert_eq!(derive_name("/EFI/BOOT/BOOTX64.EFI", None), "Fallback boot loader");
        assert_eq!(derive_name("/System/Library/CoreServices/boot.efi", None), "macOS");
        assert_eq!(derive_name("/shell.efi", None), "shell");
        assert_eq!(derive_name("/", None), "Unknown system");
    }

    #[test]
    fn names_from_os_release() {
        let t = "NAME=\"Fedora Linux\"\nVERSION=\"40 (Thirty)\"\nID=fedora\n";
        assert_eq!(derive_name("/EFI/fedora/shim.efi", Some(t)), "Fedora Linux 40 (Thirty)");
        let t = "NAME=Debian\nPRETTY_NAME='Debian GNU/Linux 12'\n";
        assert_eq!(derive_name("/EFI/debian/x.efi", Some(t)), "Debian GNU/Linux 12");
        assert_eq!(derive_name("/EFI/debian/x.efi", Some("garbage\n=\nFOO=bar")), "Debian");
        let long = format!("PRETTY_NAME={}\n", "x".repeat(200));
        assert_eq!(derive_name("/a/b.efi", Some(&long)).len(), MAX_TITLE);
        assert_eq!(os_release_name("PRETTY_NAME=a\u{7}b"), Some("ab".into()));
    }

    #[test]
    fn ids_have_no_spaces_and_are_case_insensitive() {
        let a = efi(1, 2, "/EFI/My Dir/X.EFI", Trust::Signed);
        let b = efi(1, 2, "/efi/my dir/x.efi", Trust::Signed);
        assert_eq!(a.id(), "efi:1:2:/efi/my_dir/x.efi");
        assert_eq!(a.id(), b.id());
        assert_eq!(Discovered { target: Target::BiosChain { disk: 0, part: 1 }, ..a }.id(), "bios:0:1");
    }

    #[test]
    fn bios_names() {
        let d = |part, os: Option<&str>| Discovered {
            target: Target::BiosChain { disk: 1, part },
            source: Source::BootSector,
            trust: Trust::Unsigned(UnsignedReason::NotApplicable),
            os_release: os.map(Into::into),
        };
        assert_eq!(d(0, None).title(), "Disk 1");
        assert_eq!(d(3, None).title(), "Disk 1 partition 3");
        assert_eq!(d(3, Some("NAME=Old")).title(), "Old");
    }

    #[test]
    fn merge_dedupes_and_orders_independent_of_scan_order() {
        let a = efi(0, 1, "/EFI/Microsoft/Boot/bootmgfw.efi", Trust::Signed);
        let b = efi(0, 1, "/EFI/ubuntu/shimx64.efi", Trust::Signed);
        let c = efi(1, 1, "/EFI/arch/grub.efi", UNS);
        let one = merge(vec![a.clone(), b.clone(), c.clone(), a.clone()], &[], false);
        let two = merge(vec![c, b, a], &[], false);
        assert_eq!(one, two);
        let titles: Vec<_> = one.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["Arch", "Ubuntu", "Windows Boot Manager"]);
    }

    #[test]
    fn overrides_rename_hide_reorder_default() {
        let a = efi(0, 1, "/EFI/Microsoft/Boot/bootmgfw.efi", Trust::Signed);
        let b = efi(0, 1, "/EFI/ubuntu/shimx64.efi", Trust::Signed);
        let ov = vec![
            Override { id: a.id(), name: Some("Games".into()), hidden: None, order: Some(5), default: true },
            Override { id: b.id(), name: None, hidden: Some(true), order: None, default: false },
            Override { id: "efi:9:9:/nothing".into(), name: Some("ghost".into()), hidden: None, order: None, default: false },
        ];
        let items = merge(vec![b, a], &ov, false);
        assert_eq!(items.len(), 2);
        assert_eq!((items[0].title.as_str(), items[0].order, items[0].default), ("Games", 5, true));
        assert!(items[1].hidden && items[1].title == "Ubuntu");
    }

    #[test]
    fn equal_titles_get_a_location() {
        let a = efi(0, 1, "/EFI/Microsoft/Boot/bootmgfw.efi", Trust::Signed);
        let b = efi(2, 1, "/EFI/Microsoft/Boot/bootmgfw.efi", Trust::Signed);
        let items = merge(vec![b, a], &[], false);
        assert_eq!(items[0].title, "Windows Boot Manager (disk 0 part 1)");
        assert_eq!(items[1].title, "Windows Boot Manager (disk 2 part 1)");
    }

    #[test]
    fn greyed_out_only_when_unsigned_and_secure_boot() {
        let s = efi(0, 1, "/EFI/a/a.efi", Trust::Signed);
        let u = efi(0, 1, "/EFI/b/b.efi", Trust::Unsigned(UnsignedReason::BadSignature));
        let on = merge(vec![s.clone(), u.clone()], &[], true);
        assert!(on[0].enabled() && !on[1].enabled());
        assert_eq!(on[1].disabled.unwrap().text(), "not signed, Secure Boot is on");
        let off = merge(vec![s, u], &[], false);
        assert!(off.iter().all(|i| i.enabled()));
    }

    #[test]
    fn all_items_config_first_and_scan_switch() {
        let cfg = Config::parse(
            "scan yes\nentry a\nkernel /a\norder 50\nentry b\nkernel /b\nkind chain\nhold r\noverride efi:0:1:/efi/x/y.efi\norder 175\n",
        )
        .unwrap();
        let found = vec![efi(0, 1, "/EFI/x/y.efi", Trust::Signed), efi(0, 1, "/EFI/z/z.efi", Trust::Signed)];
        let items = all_items(&cfg, found.clone(), false);
        let ids: Vec<_> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "efi:0:1:/efi/x/y.efi", "efi:0:1:/efi/z/z.efi"]);
        assert_eq!(items[0].action, Action::Linux(0));
        assert_eq!(items[1].action, Action::Chain(Target::BootVolume { path: "/b".into() }));
        assert!(items[1].hold.is_some());
        let mut off = cfg;
        off.scan = false;
        assert_eq!(all_items(&off, found, false).len(), 2);
    }
}
