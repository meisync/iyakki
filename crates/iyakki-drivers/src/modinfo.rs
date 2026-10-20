//! Reads `.modinfo` from a kernel module (an ELF64 little-endian relocatable object) to check that a prebuilt module was
//! made for the kernel that is running. A module with a different `vermagic` is refused by the kernel anyway; we refuse
//! it first, with a sentence a person can act on.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Modinfo {
    pub name: String,
    pub vermagic: String,
    pub license: String,
    /// Modules this one needs loaded first (`depends=cfg80211`), as the build recorded them.
    pub depends: Vec<String>,
}

impl Modinfo {
    /// The kernel release the module was built for: the first word of `vermagic`.
    pub fn release(&self) -> &str {
        self.vermagic.split_whitespace().next().unwrap_or("")
    }

    /// Licences the kernel treats as free; anything else taints the kernel when loaded.
    pub fn gpl_compatible(&self) -> bool {
        matches!(self.license.as_str(), "GPL" | "GPL v2" | "GPL and additional rights" | "Dual BSD/GPL" | "Dual MIT/GPL" | "Dual MPL/GPL")
    }
}

fn u16le(b: &[u8], o: usize) -> Option<usize> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as usize)
}
fn u32le(b: &[u8], o: usize) -> Option<usize> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
}
fn u64le(b: &[u8], o: usize) -> Option<usize> {
    b.get(o..o + 8).and_then(|s| usize::try_from(u64::from_le_bytes(s.try_into().ok()?)).ok())
}

pub fn parse(ko: &[u8]) -> Result<Modinfo, String> {
    if ko.len() < 64 || &ko[..4] != b"\x7fELF" {
        return Err("not a kernel module file (an uncompressed .ko is needed)".into());
    }
    if ko[4] != 2 || ko[5] != 1 {
        return Err("only 64-bit little-endian modules are supported".into());
    }
    let bad = || "the module file is damaged".to_string();
    let (shoff, shentsize, shnum, shstrndx) = (u64le(ko, 0x28).ok_or_else(bad)?, u16le(ko, 0x3A).ok_or_else(bad)?, u16le(ko, 0x3C).ok_or_else(bad)?, u16le(ko, 0x3E).ok_or_else(bad)?);
    if shentsize < 64 || shnum == 0 || shnum > 4096 || shstrndx >= shnum {
        return Err(bad());
    }
    let section = |i: usize| -> Option<(usize, usize, usize)> {
        let base = shoff.checked_add(i.checked_mul(shentsize)?)?;
        Some((u32le(ko, base)?, u64le(ko, base + 0x18)?, u64le(ko, base + 0x20)?))
    };
    let (_, stroff, strsize) = section(shstrndx).ok_or_else(bad)?;
    let strtab = ko.get(stroff..stroff.checked_add(strsize).ok_or_else(bad)?).ok_or_else(bad)?;
    for i in 0..shnum {
        let (name_off, off, size) = section(i).ok_or_else(bad)?;
        let name = strtab.get(name_off..).and_then(|s| s.split(|b| *b == 0).next()).unwrap_or(b"");
        if name == b".modinfo" {
            let data = ko.get(off..off.checked_add(size).ok_or_else(bad)?).ok_or_else(bad)?;
            let mut m = Modinfo::default();
            for kv in data.split(|b| *b == 0) {
                let Ok(s) = std::str::from_utf8(kv) else { continue };
                // entries may be prefixed `module.key=value`; the plain ones are what we need
                if let Some(v) = s.strip_prefix("vermagic=") {
                    m.vermagic = v.into();
                } else if let Some(v) = s.strip_prefix("name=") {
                    m.name = v.into();
                } else if let Some(v) = s.strip_prefix("license=") {
                    m.license = v.into();
                } else if let Some(v) = s.strip_prefix("depends=") {
                    m.depends = v.split(',').filter(|d| !d.is_empty()).map(str::to_string).collect();
                }
            }
            if m.vermagic.is_empty() {
                return Err("the module has no vermagic: it was not built for a kernel".into());
            }
            return Ok(m);
        }
    }
    Err("the file has no module information (.modinfo)".into())
}

/// Is this module loadable by the running kernel? `running` is its release (`uname -r`); `reference` is the full
/// vermagic of one stock module of the running kernel, when there is one to compare with (it also catches a different
/// configuration such as `SMP preempt` flags, not just a different release).
pub fn check(ko: &[u8], running: &str, reference: Option<&str>) -> Result<Modinfo, String> {
    let m = parse(ko)?;
    if m.release() != running {
        return Err(format!("this module was built for kernel {} but this system runs {}. A module only loads into the exact kernel it was built for; build it for {} and make a new pack.", m.release(), if running.is_empty() { "an unknown release" } else { running }, running));
    }
    if let Some(r) = reference.filter(|r| !r.is_empty()) {
        if r != m.vermagic {
            return Err(format!("this module's build settings (\"{}\") differ from the running kernel's (\"{}\"), so the kernel would refuse it", m.vermagic, r));
        }
    }
    Ok(m)
}

/// Build a tiny valid module image (for tests): ELF64 with a `.modinfo` and a section name table.
#[doc(hidden)]
pub fn synthetic_module(name: &str, vermagic: &str, license: &str) -> Vec<u8> {
    synthetic_module_with(name, vermagic, license, "")
}

#[doc(hidden)]
pub fn synthetic_module_with(name: &str, vermagic: &str, license: &str, depends: &str) -> Vec<u8> {
    let mut modinfo = Vec::new();
    for kv in [format!("license={license}"), format!("depends={depends}"), format!("name={name}"), format!("vermagic={vermagic}")] {
        modinfo.extend_from_slice(kv.as_bytes());
        modinfo.push(0);
    }
    let shstr = b"\0.modinfo\0.shstrtab\0".to_vec();
    let mut f = vec![0u8; 64];
    f[..4].copy_from_slice(b"\x7fELF");
    f[4] = 2;
    f[5] = 1;
    let off_modinfo = f.len();
    f.extend_from_slice(&modinfo);
    let off_str = f.len();
    f.extend_from_slice(&shstr);
    while f.len() % 8 != 0 {
        f.push(0);
    }
    let shoff = f.len();
    for (name_off, off, size) in [(0usize, 0usize, 0usize), (1, off_modinfo, modinfo.len()), (10, off_str, shstr.len())] {
        let mut sh = vec![0u8; 64];
        sh[..4].copy_from_slice(&(name_off as u32).to_le_bytes());
        sh[0x18..0x20].copy_from_slice(&(off as u64).to_le_bytes());
        sh[0x20..0x28].copy_from_slice(&(size as u64).to_le_bytes());
        f.extend_from_slice(&sh);
    }
    f[0x28..0x30].copy_from_slice(&(shoff as u64).to_le_bytes());
    f[0x3A..0x3C].copy_from_slice(&64u16.to_le_bytes());
    f[0x3C..0x3E].copy_from_slice(&3u16.to_le_bytes());
    f[0x3E..0x40].copy_from_slice(&2u16.to_le_bytes());
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_vermagic_name_and_licence() {
        let ko = synthetic_module("wl", "6.12.112 SMP mod_unload ", "MIXED/Proprietary");
        let m = parse(&ko).unwrap();
        assert_eq!((m.name.as_str(), m.release(), m.license.as_str()), ("wl", "6.12.112", "MIXED/Proprietary"));
        assert!(!m.gpl_compatible());
        assert!(parse(&synthetic_module("x", "6.1 SMP", "GPL")).unwrap().gpl_compatible());
    }

    #[test]
    fn dependencies_are_read_from_modinfo() {
        let m = parse(&synthetic_module_with("wl", "6.12.112 SMP preempt mod_unload ", "MIXED/Proprietary", "cfg80211,lib80211")).unwrap();
        assert_eq!(m.depends, vec!["cfg80211", "lib80211"]);
        assert!(parse(&synthetic_module("x", "1 SMP", "GPL")).unwrap().depends.is_empty());
    }

    #[test]
    fn a_module_for_another_kernel_is_refused_with_a_clear_message() {
        let ko = synthetic_module("wl", "6.12.100 SMP mod_unload ", "GPL");
        let e = check(&ko, "6.12.112", None).unwrap_err();
        assert!(e.contains("6.12.100") && e.contains("6.12.112") && e.contains("exact kernel"), "{e}");
        assert!(check(&ko, "6.12.100", None).is_ok());
        let e = check(&ko, "6.12.100", Some("6.12.100 SMP preempt mod_unload ")).unwrap_err();
        assert!(e.contains("build settings"), "{e}");
        assert!(check(&ko, "6.12.100", Some("6.12.100 SMP mod_unload ")).is_ok());
    }

    #[test]
    fn junk_is_not_a_module() {
        assert!(parse(b"hello").is_err());
        assert!(parse(&[0u8; 100]).is_err());
        let mut ko = synthetic_module("x", "1 SMP", "GPL");
        ko.truncate(ko.len() - 100);
        assert!(parse(&ko).is_err());
        let mut ko = synthetic_module("x", "1 SMP", "GPL");
        ko[5] = 2;
        assert!(parse(&ko).is_err());
    }
}
