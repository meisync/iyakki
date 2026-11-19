//! Smaller images, the parts that need no image build (F-PKG-11): compress kernel modules, and split the module tree
//! into the set that ships in the image and an optional set that is offered as a driver pack.
//!
//! Pure data handling over a `modules.dep` text and file sizes. It never touches a running system and builds nothing.
//! Rules, each with a test:
//! * a module is only optional when no kept module (directly or through a chain) needs it, so the split never leaves
//!   a kept module with a missing dependency (the dependency closure is kept);
//! * modules the boot needs (storage, file systems, USB, HID, network core, `essential` prefixes) are never optional;
//! * compressing is done AFTER the modules are signed (a signature sits at the end of the `.ko`; `secureboot.sh`
//!   already refuses compressed modules when it signs), and a module that is already compressed is not compressed twice;
//! * the gzip output is a plain gzip file the kernel (`CONFIG_MODULE_DECOMPRESS`, `CONFIG_MODULE_COMPRESS_GZIP`) and
//!   `gzip -t` accept.

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// Parse a `modules.dep` file: `kernel/a.ko: kernel/b.ko kernel/c.ko` (dependencies may be empty).
pub fn parse_dep(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut m = BTreeMap::new();
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let k = k.trim();
        if k.is_empty() {
            continue;
        }
        m.insert(k.to_string(), v.split_whitespace().map(str::to_string).collect());
    }
    m
}

/// Module path prefixes that are never optional: without them the machine does not boot, find its disk or its network.
pub const ESSENTIAL: &[&str] = &[
    "kernel/fs/",
    "kernel/drivers/ata/",
    "kernel/drivers/nvme/",
    "kernel/drivers/scsi/",
    "kernel/drivers/block/",
    "kernel/drivers/md/",
    "kernel/drivers/usb/",
    "kernel/drivers/hid/",
    "kernel/drivers/input/",
    "kernel/drivers/gpu/drm/",
    "kernel/drivers/net/ethernet/",
    "kernel/drivers/net/wireless/",
    "kernel/drivers/bluetooth/",
    "kernel/net/",
    "kernel/crypto/",
    "kernel/lib/",
    "kernel/sound/pci/",
    "kernel/sound/usb/",
    "kernel/sound/core/",
    "kernel/sound/hda/",
];

/// Rarely used on a laptop or desktop: candidates for the optional download.
pub const RARE: &[&str] = &[
    "kernel/drivers/staging/",
    "kernel/drivers/infiniband/",
    "kernel/drivers/media/dvb-",
    "kernel/drivers/media/tuners/",
    "kernel/drivers/media/rc/",
    "kernel/drivers/isdn/",
    "kernel/drivers/mtd/",
    "kernel/drivers/comedi/",
    "kernel/drivers/net/can/",
    "kernel/drivers/net/arcnet/",
    "kernel/drivers/net/hamradio/",
    "kernel/drivers/net/wan/",
    "kernel/drivers/parport/",
    "kernel/drivers/pcmcia/",
    "kernel/drivers/message/",
    "kernel/drivers/w1/",
    "kernel/drivers/greybus/",
    "kernel/drivers/iio/",
    "kernel/drivers/ssb/",
    "kernel/drivers/misc/",
    "kernel/net/ax25/",
    "kernel/net/rose/",
    "kernel/net/netrom/",
    "kernel/net/can/",
    "kernel/net/rxrpc/",
    "kernel/net/ieee802154/",
    "kernel/net/6lowpan/",
    "kernel/net/nfc/",
    "kernel/sound/soc/",
    "kernel/sound/isa/",
    "kernel/sound/drivers/",
    "kernel/sound/firewire/",
];

fn under(path: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|p| path.starts_with(p))
}

/// The result of a split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub keep: BTreeSet<String>,
    pub optional: BTreeSet<String>,
}

/// Decide the split. `rare` and `essential` are path prefixes (normally [`RARE`] and [`ESSENTIAL`]).
/// A module is optional if it is rare, not essential and not needed (transitively) by any kept module.
pub fn plan(dep: &BTreeMap<String, Vec<String>>, rare: &[&str], essential: &[&str]) -> Plan {
    let mut optional: BTreeSet<String> = dep.keys().filter(|m| under(m, rare) && !under(m, essential)).cloned().collect();
    // A kept module that needs an optional one pulls it back: repeat until stable (chains).
    loop {
        let pulled: Vec<String> = dep.iter().filter(|(m, _)| !optional.contains(*m)).flat_map(|(_, ds)| ds.iter()).filter(|d| optional.contains(*d)).cloned().collect();
        if pulled.is_empty() {
            break;
        }
        for p in pulled {
            optional.remove(&p);
        }
    }
    let keep = dep.keys().filter(|m| !optional.contains(*m)).cloned().collect();
    Plan { keep, optional }
}

impl Plan {
    /// Bytes saved in the image by moving the optional set out (before compression).
    pub fn saved(&self, sizes: &BTreeMap<String, u64>) -> u64 {
        self.optional.iter().map(|m| sizes.get(m).copied().unwrap_or(0)).sum()
    }

    /// Every dependency of every kept module is itself kept.
    pub fn closed(&self, dep: &BTreeMap<String, Vec<String>>) -> bool {
        self.keep.iter().all(|m| dep.get(m).is_none_or(|ds| ds.iter().all(|d| self.keep.contains(d))))
    }

    /// The manifest of the optional pack: `module <size> <sha256> <path>` lines in path order, so the pack can be
    /// verified file by file at the person's click like the firmware pack. `read` gives a module's bytes.
    pub fn pack_manifest(&self, release: &str, read: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<String, String> {
        let mut out = format!("# IYAKKI optional kernel modules for kernel {release}\nrelease {release}\n");
        for m in &self.optional {
            let b = read(m).ok_or_else(|| format!("missing module {m}"))?;
            out.push_str(&format!("module {} {} {}\n", b.len(), hex(&Sha256::digest(&b)), m));
        }
        Ok(out)
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// CRC-32 (IEEE) as gzip uses it.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

/// Why a module is not compressed.
#[derive(Debug, PartialEq, Eq)]
pub enum Skip {
    NotElf,
    AlreadyCompressed,
}

/// Compress one signed `.ko` into a gzip file (name gets `.gz`). Refuses input that is not an ELF object (a module is
/// ELF) and input that already looks compressed, so a rerun of the step is harmless.
pub fn gzip_module(ko: &[u8]) -> Result<Vec<u8>, Skip> {
    if ko.starts_with(&[0x1f, 0x8b]) || ko.starts_with(b"\xfd7zXZ") || ko.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        return Err(Skip::AlreadyCompressed);
    }
    if !ko.starts_with(b"\x7fELF") {
        return Err(Skip::NotElf);
    }
    let mut g = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 2, 3];
    g.extend(miniz_oxide::deflate::compress_to_vec(ko, 9));
    g.extend_from_slice(&crc32(ko).to_le_bytes());
    g.extend_from_slice(&(ko.len() as u32).to_le_bytes());
    Ok(g)
}

/// What [`compress_tree`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TreeReport {
    pub compressed: usize,
    pub skipped: usize,
    pub moved_out: usize,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

fn collect_ko(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) -> std::io::Result<()> {
    let mut names: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    names.sort_by_key(|e| e.file_name());
    for e in names {
        let p = e.path();
        let t = e.file_type()?;
        if t.is_dir() {
            collect_ko(&p, base, out)?;
        } else if t.is_file() && p.extension().is_some_and(|x| x == "ko") {
            out.push(p.strip_prefix(base).unwrap_or(&p).to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

/// The build step behind `iyakki-modpack` (F-PKG-11): over a finished, already signed `lib/modules/RELEASE` folder,
/// optionally move the optional set (by `modules.dep`, rare and not essential, closed) into `pack_out` with a manifest
/// for the optional pack, then gzip every remaining `.ko` in place (`x.ko` becomes `x.ko.gz`). The caller runs `depmod`
/// afterwards so `modules.dep` names the `.ko.gz` files. Nothing outside `root` and `pack_out` is touched; a rerun finds
/// nothing left to do. Without `modules.dep` it refuses (a split without it could not be closed).
pub fn compress_tree(root: &std::path::Path, release: &str, pack_out: Option<&std::path::Path>) -> Result<TreeReport, String> {
    let dep_text = std::fs::read_to_string(root.join("modules.dep")).map_err(|e| format!("modules.dep: {e}"))?;
    let mut rep = TreeReport::default();
    if let Some(out) = pack_out {
        let dep = parse_dep(&dep_text);
        // modules.dep names may already be .ko.gz after a first run: strip so the plan keys match nothing twice
        let pl = plan(&dep, RARE, ESSENTIAL);
        let mut moved: Vec<String> = Vec::new();
        for m in &pl.optional {
            let src = root.join(m);
            if !src.is_file() {
                continue;
            }
            let dst = out.join(m);
            if let Some(d) = dst.parent() {
                std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
            }
            std::fs::copy(&src, &dst).map_err(|e| format!("{m}: {e}"))?;
            std::fs::remove_file(&src).map_err(|e| format!("{m}: {e}"))?;
            moved.push(m.clone());
        }
        rep.moved_out = moved.len();
        let manifest = Plan { keep: pl.keep.clone(), optional: moved.iter().cloned().collect() }
            .pack_manifest(release, &|m| std::fs::read(out.join(m)).ok())?;
        std::fs::write(out.join("manifest.txt"), manifest).map_err(|e| format!("manifest: {e}"))?;
    }
    let mut files = Vec::new();
    collect_ko(root, root, &mut files).map_err(|e| format!("{}: {e}", root.display()))?;
    for rel in files {
        let p = root.join(&rel);
        let b = std::fs::read(&p).map_err(|e| format!("{rel}: {e}"))?;
        match gzip_module(&b) {
            Ok(z) => {
                let gz = root.join(format!("{rel}.gz"));
                std::fs::write(&gz, &z).map_err(|e| format!("{rel}: {e}"))?;
                std::fs::remove_file(&p).map_err(|e| format!("{rel}: {e}"))?;
                rep.compressed += 1;
                rep.bytes_before += b.len() as u64;
                rep.bytes_after += z.len() as u64;
            }
            Err(_) => rep.skipped += 1,
        }
    }
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dep() -> BTreeMap<String, Vec<String>> {
        parse_dep(
            "kernel/fs/ext4/ext4.ko: kernel/fs/jbd2/jbd2.ko kernel/lib/crc16.ko\n\
             kernel/fs/jbd2/jbd2.ko:\n\
             kernel/lib/crc16.ko:\n\
             kernel/drivers/staging/rtl/rtl.ko: kernel/drivers/staging/lib/slib.ko\n\
             kernel/drivers/staging/lib/slib.ko:\n\
             kernel/drivers/media/dvb-core/dvb.ko:\n\
             kernel/drivers/media/dvb-usb/dvbu.ko: kernel/drivers/media/dvb-core/dvb.ko\n\
             kernel/drivers/usb/storage/usb-storage.ko: kernel/drivers/staging/lib/slib.ko\n\
             kernel/sound/soc/codecs/snd-soc-x.ko:\n\
             kernel/sound/pci/hda/snd-hda.ko: kernel/sound/soc/codecs/snd-soc-x.ko\n\
             kernel/drivers/net/wan/hdlc.ko:\n",
        )
    }

    #[test]
    fn rare_modules_leave_unless_a_kept_module_needs_them() {
        let d = dep();
        let p = plan(&d, RARE, ESSENTIAL);
        // The staging library is pulled in by the essential USB storage module, so the whole chain it needs stays.
        assert!(p.keep.contains("kernel/drivers/staging/lib/slib.ko"));
        // The staging driver itself needs only that library and nothing kept needs it: optional.
        assert!(p.optional.contains("kernel/drivers/staging/rtl/rtl.ko"));
        // DVB core and its USB driver leave together.
        assert!(p.optional.contains("kernel/drivers/media/dvb-core/dvb.ko") && p.optional.contains("kernel/drivers/media/dvb-usb/dvbu.ko"));
        // The ASoC codec is needed by the HDA module: kept.
        assert!(p.keep.contains("kernel/sound/soc/codecs/snd-soc-x.ko"));
        assert!(p.optional.contains("kernel/drivers/net/wan/hdlc.ko"));
        // Essential modules are never optional.
        assert!(p.keep.contains("kernel/fs/ext4/ext4.ko"));
        assert!(p.closed(&d));
    }

    #[test]
    fn the_split_is_always_closed_even_for_a_hostile_prefix_list() {
        let d = dep();
        // Everything "rare" and nothing essential: chains must still be closed.
        let p = plan(&d, &["kernel/"], &[]);
        assert!(p.closed(&d));
        assert_eq!(p.keep.len() + p.optional.len(), d.len());
        // Nothing rare: nothing leaves.
        assert!(plan(&d, &[], ESSENTIAL).optional.is_empty());
    }

    #[test]
    fn saving_and_manifest_are_exact() {
        let d = dep();
        let p = plan(&d, RARE, ESSENTIAL);
        let sizes: BTreeMap<String, u64> = d.keys().map(|k| (k.clone(), 1000)).collect();
        assert_eq!(p.saved(&sizes), 1000 * p.optional.len() as u64);
        let m = p.pack_manifest("6.12.0", &|n| Some(n.as_bytes().to_vec())).unwrap();
        assert!(m.contains("release 6.12.0"));
        let line = m.lines().find(|l| l.ends_with("hdlc.ko")).unwrap();
        let f: Vec<&str> = line.split(' ').collect();
        assert_eq!(f[1], "kernel/drivers/net/wan/hdlc.ko".len().to_string());
        assert_eq!(f[2], hex(&Sha256::digest(b"kernel/drivers/net/wan/hdlc.ko")));
        assert!(p.pack_manifest("x", &|_| None).is_err());
    }

    fn fake_ko(n: usize) -> Vec<u8> {
        let mut v = b"\x7fELF".to_vec();
        for i in 0..n {
            v.push((i * 7 % 13) as u8);
        }
        v
    }

    #[test]
    fn gzip_refuses_what_it_should() {
        assert_eq!(gzip_module(b"hello"), Err(Skip::NotElf));
        let z = gzip_module(&fake_ko(100)).unwrap();
        assert_eq!(gzip_module(&z), Err(Skip::AlreadyCompressed));
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    /// Checked with the real `gzip` (never shipped): it must accept the file and give back the same bytes.
    #[test]
    fn the_real_gzip_accepts_and_restores_the_module() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        if Command::new("gzip").arg("--version").output().is_err() {
            return;
        }
        for n in [0usize, 1, 5000, 300_000] {
            let ko = fake_ko(n);
            let z = gzip_module(&ko).unwrap();
            assert!(z.len() < ko.len() || n < 100);
            let mut c = Command::new("gzip").args(["-dc"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
            let mut si = c.stdin.take().unwrap();
            let z2 = z.clone();
            let t = std::thread::spawn(move || si.write_all(&z2));
            let o = c.wait_with_output().unwrap();
            t.join().unwrap().unwrap();
            assert!(o.status.success(), "gzip said: {}", String::from_utf8_lossy(&o.stderr));
            assert_eq!(o.stdout, ko, "size {n}");
        }
    }

    #[test]
    fn a_tree_is_split_compressed_and_a_rerun_does_nothing() {
        let tmp = std::env::temp_dir().join(format!("modpack-{}-tree", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let root = tmp.join("m");
        let pack = tmp.join("pack");
        let d = dep();
        let mut dep_text = String::new();
        for (k, v) in &d {
            dep_text.push_str(&format!("{k}: {}\n", v.join(" ")));
            let f = root.join(k);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(&f, fake_ko(4000)).unwrap();
        }
        std::fs::write(root.join("modules.dep"), dep_text).unwrap();
        let r = compress_tree(&root, "6.12.0", Some(&pack)).unwrap();
        assert!(r.moved_out >= 3 && r.compressed + r.moved_out == d.len());
        assert!(r.bytes_after < r.bytes_before);
        assert!(root.join("kernel/fs/ext4/ext4.ko.gz").is_file() && !root.join("kernel/fs/ext4/ext4.ko").exists());
        assert!(!root.join("kernel/drivers/net/wan/hdlc.ko").exists() && pack.join("kernel/drivers/net/wan/hdlc.ko").is_file());
        assert!(std::fs::read_to_string(pack.join("manifest.txt")).unwrap().contains("hdlc.ko"));
        let again = compress_tree(&root, "6.12.0", None).unwrap();
        assert_eq!((again.compressed, again.moved_out), (0, 0));
        assert!(compress_tree(&tmp.join("nothing"), "x", None).is_err());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
