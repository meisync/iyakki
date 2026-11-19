//! Rebuild a FAT32 volume with another sector size.
//!
//! The boot partition of the install template is FAT32 with 512-byte sectors and 512-byte clusters. A disk with 4096-byte
//! logical sectors ("4Kn") cannot use that: the kernel refuses a filesystem whose sectors are smaller than the disk's, and the
//! firmware's FAT driver does too. So for such disks the installer reads the template's files and writes a new FAT32 volume
//! (one 4096-byte sector per cluster). Directory entries (including long file names) are copied byte for byte; only the cluster
//! numbers in them change. Nothing here needs a mounted filesystem or an external tool.
//!
//! [`Fat32`] reads a volume; [`clone_to`] writes the new one as one sequential stream, so the installer can hash it while it
//! writes and read it back later.

use std::io::{Read, Seek, SeekFrom};

const EOC: u32 = 0x0FFF_FFFF;
const RESERVED_SECTORS: u32 = 32;

fn le16(b: &[u8], o: usize) -> u32 {
    u16::from_le_bytes([b[o], b[o + 1]]) as u32
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// A FAT32 volume being read: `base` is where its first sector is in the stream.
pub struct Fat32<'a, R: Read + Seek> {
    r: &'a mut R,
    base: u64,
    bps: u64,
    spc: u64,
    data_start: u64,
    clusters: u32,
    root: u32,
    fat: Vec<u32>,
    pub label: String,
}

/// One directory entry that is a file or a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub path: String,
    pub dir: bool,
    pub size: u32,
    pub first: u32,
}

impl<'a, R: Read + Seek> Fat32<'a, R> {
    pub fn open(r: &'a mut R, base: u64) -> Result<Fat32<'a, R>, String> {
        let mut b = vec![0u8; 512];
        r.seek(SeekFrom::Start(base)).and_then(|_| r.read_exact(&mut b)).map_err(|e| format!("reading the boot volume: {e}"))?;
        let (bps, spc, reserved, nfats) = (le16(&b, 11) as u64, b[13] as u64, le16(&b, 14) as u64, b[16] as u64);
        let (fatsz, total) = (le32(&b, 36) as u64, le32(&b, 32) as u64);
        if b[510..512] != [0x55, 0xAA] || le16(&b, 22) != 0 || le16(&b, 17) != 0 || fatsz == 0 || !matches!(bps, 512 | 1024 | 2048 | 4096) || spc == 0 || !spc.is_power_of_two() || nfats == 0 {
            return Err("the boot partition is not a FAT32 volume".into());
        }
        let data_start = reserved + nfats * fatsz;
        if total <= data_start {
            return Err("the boot volume is damaged (no data area)".into());
        }
        let clusters = ((total - data_start) / spc) as u32;
        let mut raw = vec![0u8; (fatsz * bps) as usize];
        r.seek(SeekFrom::Start(base + reserved * bps)).and_then(|_| r.read_exact(&mut raw)).map_err(|e| format!("reading the FAT: {e}"))?;
        let n = (clusters as usize + 2).min(raw.len() / 4);
        let fat = (0..n).map(|i| le32(&raw, 4 * i) & 0x0FFF_FFFF).collect();
        let label = String::from_utf8_lossy(&b[71..82]).trim().to_string();
        Ok(Fat32 { r, base, bps, spc, data_start, clusters, root: le32(&b, 44), fat, label })
    }

    pub fn cluster_bytes(&self) -> u64 {
        self.bps * self.spc
    }

    fn chain(&self, first: u32) -> Result<Vec<u32>, String> {
        let mut v = Vec::new();
        let mut c = first;
        while (2..0x0FFF_FFF0).contains(&c) {
            if c as usize >= self.fat.len() || v.len() > self.clusters as usize {
                return Err("the boot volume is damaged (a file's cluster chain is broken)".into());
            }
            v.push(c);
            c = self.fat[c as usize];
        }
        if v.is_empty() && first >= 2 {
            return Err("the boot volume is damaged (an empty cluster chain)".into());
        }
        Ok(v)
    }

    fn offset(&self, c: u32) -> u64 {
        self.base + (self.data_start + (c as u64 - 2) * self.spc) * self.bps
    }

    /// The bytes of a cluster chain, `limit` at most.
    fn read_chain(&mut self, chain: &[u32], limit: u64) -> Result<Vec<u8>, String> {
        let cb = self.cluster_bytes();
        let mut out = Vec::with_capacity((chain.len() as u64 * cb).min(limit) as usize);
        let mut i = 0;
        while i < chain.len() && (out.len() as u64) < limit {
            let mut j = i + 1;
            while j < chain.len() && chain[j] == chain[j - 1] + 1 && (j - i) as u64 * cb < 1 << 20 {
                j += 1;
            }
            let want = ((j - i) as u64 * cb).min(limit - out.len() as u64) as usize;
            let at = out.len();
            out.resize(at + want, 0);
            self.r.seek(SeekFrom::Start(self.offset(chain[i]))).and_then(|_| self.r.read_exact(&mut out[at..])).map_err(|e| format!("reading the boot volume: {e}"))?;
            i = j;
        }
        Ok(out)
    }

    /// Every file and directory (names as stored: short names, upper case), depth first.
    pub fn walk(&mut self) -> Result<Vec<Item>, String> {
        let mut out = Vec::new();
        let mut stack = vec![(String::new(), self.root)];
        while let Some((prefix, first)) = stack.pop() {
            let chain = self.chain(first)?;
            let bytes = self.read_chain(&chain, u64::MAX)?;
            for e in bytes.chunks_exact(32) {
                if e[0] == 0 {
                    break;
                }
                if e[0] == 0xE5 || e[11] == 0x0F || e[11] & 0x08 != 0 || e[0] == b'.' {
                    continue;
                }
                let base = String::from_utf8_lossy(&e[0..8]).trim_end().to_string();
                let ext = String::from_utf8_lossy(&e[8..11]).trim_end().to_string();
                let name = if ext.is_empty() { base } else { format!("{base}.{ext}") };
                let path = format!("{prefix}/{name}");
                let cl = (le16(e, 20) << 16) | le16(e, 26);
                let dir = e[11] & 0x10 != 0;
                out.push(Item { path: path.clone(), dir, size: le32(e, 28), first: cl });
                if dir {
                    stack.push((path, cl));
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    /// Like [`Fat32::walk`] but with the real (long) names: a file whose directory entry has long-name entries before it is listed
    /// under that name (`bzImage`, `grub.cfg`), the others under their short name. Paths use `/`.
    pub fn walk_long(&mut self) -> Result<Vec<Item>, String> {
        let mut out = Vec::new();
        let mut stack = vec![(String::new(), self.root)];
        while let Some((prefix, first)) = stack.pop() {
            let chain = self.chain(first)?;
            let bytes = self.read_chain(&chain, u64::MAX)?;
            let mut lfn: Vec<(u8, [u16; 13], u8)> = Vec::new();
            for e in bytes.chunks_exact(32) {
                if e[0] == 0 {
                    break;
                }
                if e[0] == 0xE5 {
                    lfn.clear();
                    continue;
                }
                if e[11] == 0x0F {
                    let mut u = [0u16; 13];
                    for (k, off) in [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30].into_iter().enumerate() {
                        u[k] = u16::from_le_bytes([e[off], e[off + 1]]);
                    }
                    lfn.push((e[0], u, e[13]));
                    continue;
                }
                let parts = std::mem::take(&mut lfn);
                if e[11] & 0x08 != 0 || e[0] == b'.' {
                    continue;
                }
                let base = String::from_utf8_lossy(&e[0..8]).trim_end().to_string();
                let ext = String::from_utf8_lossy(&e[8..11]).trim_end().to_string();
                let short = if ext.is_empty() { base } else { format!("{base}.{ext}") };
                let name = long_name(&parts, &e[0..11]).unwrap_or(short);
                let path = format!("{prefix}/{name}");
                let cl = (le16(e, 20) << 16) | le16(e, 26);
                let dir = e[11] & 0x10 != 0;
                out.push(Item { path: path.clone(), dir, size: le32(e, 28), first: cl });
                if dir {
                    stack.push((path, cl));
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    pub fn read_file(&mut self, item: &Item) -> Result<Vec<u8>, String> {
        if item.size == 0 {
            return Ok(vec![]);
        }
        let chain = self.chain(item.first)?;
        self.read_chain(&chain, item.size as u64)
    }
}

/// The name held by the long-name entries that came before a short entry (stored last part first), if they are well formed.
fn long_name(parts: &[(u8, [u16; 13], u8)], short11: &[u8]) -> Option<String> {
    if parts.is_empty() {
        return None;
    }
    let sum = short11.iter().fold(0u8, |sum, c| (sum >> 1).wrapping_add((sum & 1) << 7).wrapping_add(*c));
    let mut ordered: Vec<&(u8, [u16; 13], u8)> = parts.iter().collect();
    ordered.sort_by_key(|(seq, ..)| seq & 0x1F);
    if ordered.iter().enumerate().any(|(i, (seq, _, ck))| (seq & 0x1F) as usize != i + 1 || *ck != sum) || ordered.last().is_none_or(|(seq, ..)| seq & 0x40 == 0) {
        return None;
    }
    let mut units: Vec<u16> = Vec::new();
    for (_, u, _) in ordered {
        units.extend_from_slice(u);
    }
    if let Some(end) = units.iter().position(|c| *c == 0) {
        units.truncate(end);
    }
    let s = String::from_utf16(&units).ok()?;
    (!s.is_empty() && !s.contains('/') && s != "." && s != "..").then_some(s)
}

#[cfg(test)]
pub(crate) fn long_name_for_tests(parts: &[(u8, [u16; 13], u8)], short11: &[u8]) -> Option<String> {
    long_name(parts, short11)
}

enum Data {
    Dir { src: Vec<u32>, bytes: Vec<u8> },
    File { src: Vec<u32>, size: u64 },
    /// A file whose content was rewritten while cloning.
    Mem { bytes: Vec<u8> },
}

struct Node {
    first: u32,
    len: u32,
    parent: u32,
    data: Data,
    /// The directory's own path (long names, `/` separated); empty for the root and for files.
    path: String,
}

/// What an [`Editor`] decides for a file while a volume is cloned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    Keep,
    /// The file is left out of the new volume (its directory entry and long-name entries are marked deleted).
    Skip,
    /// The file is copied with new content from [`Editor::rewrite`].
    Rewrite,
}

/// Changes made to the files of a volume while [`clone_to_with`] copies it. Paths are the long names, `/` separated, from the root.
pub trait Editor {
    fn decide(&mut self, path: &str) -> Edit;
    fn rewrite(&mut self, path: &str, data: Vec<u8>) -> Vec<u8>;
}

/// The marker file that makes the boot loader show "Install to a disk" on its first level: only install media carry it (a disk that was installed
/// from them does not, and neither does an update pack).
pub const INSTALL_MEDIA_MARKER: &str = "/EFI/iyakki-boot/install-media";

/// Is this the install-media marker (long names, any case)?
pub fn is_install_media_marker(path: &str) -> bool {
    path.eq_ignore_ascii_case(INSTALL_MEDIA_MARKER)
}

/// Copies everything as it is.
pub struct Unchanged;

/// Another editor's decisions, and the install-media marker left out. Used when a disk is installed FROM a running stick (a block device):
/// the new disk is not install media. Writing a stick from an image file keeps the marker (it uses the plain editor).
pub struct WithoutInstallMarker<'a>(pub &'a mut dyn Editor);

impl Editor for WithoutInstallMarker<'_> {
    fn decide(&mut self, path: &str) -> Edit {
        if is_install_media_marker(path) {
            Edit::Skip
        } else {
            self.0.decide(path)
        }
    }
    fn rewrite(&mut self, path: &str, data: Vec<u8>) -> Vec<u8> {
        self.0.rewrite(path, data)
    }
}

impl Editor for Unchanged {
    fn decide(&mut self, _: &str) -> Edit {
        Edit::Keep
    }
    fn rewrite(&mut self, _: &str, data: Vec<u8>) -> Vec<u8> {
        data
    }
}

/// What [`clone_to`] made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cloned {
    pub files: usize,
    pub dirs: usize,
    pub used_clusters: u32,
    pub total_clusters: u32,
}

/// Write a new FAT32 volume of `total_bytes` with `sector` -byte sectors and one sector per cluster, holding what `src` holds,
/// as a sequential stream through `emit` (always exactly `total_bytes` in the end).
pub fn clone_to<R: Read + Seek>(src: &mut Fat32<R>, sector: u32, total_bytes: u64, label: &str, volume_id: u32, emit: &mut dyn FnMut(&[u8]) -> Result<(), String>) -> Result<Cloned, String> {
    clone_to_with(src, sector, total_bytes, label, volume_id, &mut Unchanged, emit)
}

/// [`clone_to`] with an [`Editor`]: files can be left out or rewritten on the way (the installer drops the fast-start files).
pub fn clone_to_with<R: Read + Seek>(src: &mut Fat32<R>, sector: u32, total_bytes: u64, label: &str, volume_id: u32, editor: &mut dyn Editor, emit: &mut dyn FnMut(&[u8]) -> Result<(), String>) -> Result<Cloned, String> {
    let ss = sector as u64;
    if !matches!(sector, 512 | 1024 | 2048 | 4096) || total_bytes % ss != 0 {
        return Err("bad sector size for the new boot volume".into());
    }
    let total = total_bytes / ss;
    // geometry: one sector per cluster; the FAT must hold every cluster
    let mut fatsz = 1u64;
    let clusters = loop {
        let c = total.saturating_sub(RESERVED_SECTORS as u64 + 2 * fatsz);
        let need = ((c + 2) * 4).div_ceil(ss);
        if need <= fatsz {
            break c;
        }
        fatsz = need;
    };
    if clusters < 65525 {
        return Err(format!("a FAT32 volume needs at least 65525 clusters; {} MB of {sector}-byte sectors gives {clusters}", total_bytes / 1_000_000));
    }
    let cb_old = src.cluster_bytes();
    let cb = ss;

    // pass 1: give every file and directory its new clusters, patching the directory entries as they are read
    let mut nodes: Vec<Node> = Vec::new();
    let mut next = 2u32;
    let root_chain = src.chain(src.root)?;
    let root_len = ((root_chain.len() as u64 * cb_old).div_ceil(cb)).max(1) as u32;
    nodes.push(Node { first: next, len: root_len, parent: 0, data: Data::Dir { src: root_chain, bytes: vec![] }, path: String::new() });
    next += root_len;
    let (mut files, mut dirs) = (0usize, 1usize);
    let mut i = 0;
    while i < nodes.len() {
        let (chain, parent, own) = match &nodes[i].data {
            Data::Dir { src: c, .. } => (c.clone(), nodes[i].parent, nodes[i].first),
            Data::File { .. } | Data::Mem { .. } => {
                i += 1;
                continue;
            }
        };
        let mut bytes = src.read_chain(&chain, u64::MAX)?;
        let prefix = nodes[i].path.clone();
        let mut off = 0;
        // the long-name entries before a short one: their offsets (to delete them with it) and their text parts (for the name)
        let mut lfn_at: Vec<usize> = Vec::new();
        let mut lfn: Vec<(u8, [u16; 13], u8)> = Vec::new();
        while off + 32 <= bytes.len() {
            let at = off;
            off += 32;
            let e = &mut bytes[at..at + 32];
            if e[0] == 0 {
                break;
            }
            if e[0] == 0xE5 {
                lfn_at.clear();
                lfn.clear();
                continue;
            }
            if e[11] == 0x0F {
                let mut u = [0u16; 13];
                for (k, o) in [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30].into_iter().enumerate() {
                    u[k] = u16::from_le_bytes([e[o], e[o + 1]]);
                }
                lfn.push((e[0], u, e[13]));
                lfn_at.push(at);
                continue;
            }
            let parts = std::mem::take(&mut lfn);
            let lfn_offsets = std::mem::take(&mut lfn_at);
            if e[11] & 0x08 != 0 {
                continue;
            }
            let put = |e: &mut [u8], c: u32| {
                e[20..22].copy_from_slice(&((c >> 16) as u16).to_le_bytes());
                e[26..28].copy_from_slice(&(c as u16).to_le_bytes());
            };
            if e[0] == b'.' {
                // "." is this directory, ".." its parent (0 stands for the root)
                put(e, if e[1] == b'.' { parent } else { own });
                continue;
            }
            let first = (le16(e, 20) << 16) | le16(e, 26);
            let name = {
                let base = String::from_utf8_lossy(&e[0..8]).trim_end().to_string();
                let ext = String::from_utf8_lossy(&e[8..11]).trim_end().to_string();
                long_name(&parts, &e[0..11]).unwrap_or(if ext.is_empty() { base } else { format!("{base}.{ext}") })
            };
            let path = format!("{prefix}/{name}");
            if e[11] & 0x10 != 0 {
                let c = src.chain(first)?;
                let len = ((c.len() as u64 * cb_old).div_ceil(cb)).max(1) as u32;
                put(e, next);
                nodes.push(Node { first: next, len, parent: if i == 0 { 0 } else { own }, data: Data::Dir { src: c, bytes: vec![] }, path });
                next += len;
                dirs += 1;
            } else {
                let size = le32(e, 28) as u64;
                let edit = editor.decide(&path);
                if edit == Edit::Skip {
                    e[0] = 0xE5;
                    for o in lfn_offsets {
                        bytes[o] = 0xE5;
                    }
                    continue;
                }
                if size == 0 && edit == Edit::Keep {
                    put(e, 0);
                    continue;
                }
                let c = if size == 0 { vec![] } else { src.chain(first)? };
                if (c.len() as u64) * cb_old < size {
                    return Err("the boot volume is damaged (a file is longer than its clusters)".into());
                }
                if edit == Edit::Rewrite {
                    let old = if size == 0 { vec![] } else { src.read_chain(&c, size)? };
                    let new = editor.rewrite(&path, old);
                    let len = (new.len() as u64).div_ceil(cb).max(1) as u32;
                    e[28..32].copy_from_slice(&(new.len() as u32).to_le_bytes());
                    put(e, if new.is_empty() { 0 } else { next });
                    if !new.is_empty() {
                        nodes.push(Node { first: next, len, parent: 0, data: Data::Mem { bytes: new }, path: String::new() });
                        next += len;
                    }
                    files += 1;
                } else {
                    let len = size.div_ceil(cb) as u32;
                    put(e, next);
                    nodes.push(Node { first: next, len, parent: 0, data: Data::File { src: c, size }, path: String::new() });
                    next += len;
                    files += 1;
                }
            }
            if next as u64 > clusters + 2 {
                return Err(format!("the boot files do not fit a {} MB boot partition", total_bytes / 1_000_000));
            }
        }
        if let Data::Dir { bytes: b, .. } = &mut nodes[i].data {
            *b = bytes;
        }
        i += 1;
    }
    let used = next - 2;

    // the FAT
    let mut fat = vec![0u8; (fatsz * ss) as usize];
    let setf = |fat: &mut Vec<u8>, c: u32, v: u32| fat[4 * c as usize..4 * c as usize + 4].copy_from_slice(&v.to_le_bytes());
    setf(&mut fat, 0, 0x0FFF_FFF8);
    setf(&mut fat, 1, EOC);
    for n in &nodes {
        for k in 0..n.len {
            setf(&mut fat, n.first + k, if k + 1 == n.len { EOC } else { n.first + k + 1 });
        }
    }

    // the reserved area: boot sector, FSInfo, and their copies at sectors 6 and 7
    let mut res = vec![0u8; (RESERVED_SECTORS as u64 * ss) as usize];
    {
        let b = &mut res[..ss as usize];
        b[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
        b[3..11].copy_from_slice(b"IYAKKI  ");
        b[11..13].copy_from_slice(&(sector as u16).to_le_bytes());
        b[13] = 1;
        b[14..16].copy_from_slice(&(RESERVED_SECTORS as u16).to_le_bytes());
        b[16] = 2;
        b[21] = 0xF8;
        b[24..26].copy_from_slice(&63u16.to_le_bytes());
        b[26..28].copy_from_slice(&255u16.to_le_bytes());
        b[32..36].copy_from_slice(&(total as u32).to_le_bytes());
        b[36..40].copy_from_slice(&(fatsz as u32).to_le_bytes());
        b[44..48].copy_from_slice(&2u32.to_le_bytes());
        b[48..50].copy_from_slice(&1u16.to_le_bytes());
        b[50..52].copy_from_slice(&6u16.to_le_bytes());
        b[64] = 0x80;
        b[66] = 0x29;
        b[67..71].copy_from_slice(&volume_id.to_le_bytes());
        let mut l = [b' '; 11];
        for (d, s) in l.iter_mut().zip(label.bytes().take(11)) {
            *d = s.to_ascii_uppercase();
        }
        b[71..82].copy_from_slice(&l);
        b[82..90].copy_from_slice(b"FAT32   ");
        b[510..512].copy_from_slice(&[0x55, 0xAA]);
        let end = ss as usize;
        b[end - 2..end].copy_from_slice(&[0x55, 0xAA]);
    }
    {
        let f = &mut res[ss as usize..2 * ss as usize];
        f[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
        f[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
        f[488..492].copy_from_slice(&(clusters as u32 - used).to_le_bytes());
        f[492..496].copy_from_slice(&next.to_le_bytes());
        f[508..512].copy_from_slice(&0xAA55_0000u32.to_le_bytes());
    }
    let (s0, s1) = (res[..ss as usize].to_vec(), res[ss as usize..2 * ss as usize].to_vec());
    res[6 * ss as usize..7 * ss as usize].copy_from_slice(&s0);
    res[7 * ss as usize..8 * ss as usize].copy_from_slice(&s1);

    // pass 2: the stream, strictly in order
    emit(&res)?;
    emit(&fat)?;
    emit(&fat)?;
    let mut written = (RESERVED_SECTORS as u64 + 2 * fatsz) * ss;
    for n in &nodes {
        let want = n.len as u64 * cb;
        let mut got = 0u64;
        match &n.data {
            Data::Dir { bytes, .. } => {
                emit(bytes)?;
                got += bytes.len() as u64;
            }
            Data::Mem { bytes } => {
                emit(bytes)?;
                got += bytes.len() as u64;
            }
            Data::File { src: chain, size } => {
                // in runs of up to 1 MiB, so a 50 MB kernel is a few dozen reads
                let per = (1u64 << 20) / cb_old;
                for run in chain.chunks(per as usize) {
                    let left = size - got;
                    let part = src.read_chain(run, left)?;
                    emit(&part)?;
                    got += part.len() as u64;
                }
            }
        }
        if got > want {
            return Err("internal error: more data than clusters".into());
        }
        emit(&vec![0u8; (want - got) as usize])?;
        written += want;
    }
    // zeros to the end of the partition
    let zero = vec![0u8; 1 << 20];
    while written < total_bytes {
        let n = (total_bytes - written).min(zero.len() as u64) as usize;
        emit(&zero[..n])?;
        written += n as u64;
    }
    Ok(Cloned { files, dirs, used_clusters: used, total_clusters: clusters as u32 })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn the_install_media_marker_is_never_copied_to_an_installed_disk() {
        assert!(is_install_media_marker("/EFI/iyakki-boot/install-media") && is_install_media_marker("/efi/IYAKKI-BOOT/Install-Media"));
        assert!(!is_install_media_marker("/EFI/iyakki-boot/boot.cfg") && !is_install_media_marker("/install-media"));
        // writing a stick from an image file keeps the marker (the plain and the lean editors do not touch it) ...
        assert!(matches!(Unchanged.decide("/EFI/iyakki-boot/install-media"), Edit::Keep));
        assert!(matches!(crate::leanstrip::LeanFree::new().decide("/EFI/iyakki-boot/install-media"), Edit::Keep));
        // ... installing from a running stick onto a disk leaves it out, whichever editor does the rest
        let mut plain = Unchanged;
        assert!(matches!(WithoutInstallMarker(&mut plain).decide("/EFI/iyakki-boot/install-media"), Edit::Skip));
        assert!(matches!(WithoutInstallMarker(&mut plain).decide("/EFI/iyakki-boot/boot.cfg"), Edit::Keep));
        let mut lean = crate::leanstrip::LeanFree::new();
        assert!(matches!(WithoutInstallMarker(&mut lean).decide("/EFI/iyakki-boot/install-media"), Edit::Skip), "also over the lean cloner");
    }

    /// A small FAT32 volume built by hand: `bps`-byte sectors, `spc` sectors per cluster, 8.3 names only, files in the
    /// root, in `BOOT` and in `BOOT/GRUB`, plus a made-up long-name entry (copied verbatim by the cloner).
    pub(crate) fn mini_fat(bps: u32, spc: u32, total_bytes: u64, files: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let (bps, spc) = (bps as u64, spc as u64);
        let total = total_bytes / bps;
        let reserved = 32u64;
        let mut fatsz = 1;
        let clusters = loop {
            let c = (total - reserved - 2 * fatsz) / spc;
            let need = ((c + 2) * 4).div_ceil(bps);
            if need <= fatsz {
                break c;
            }
            fatsz = need;
        };
        let cb = (bps * spc) as usize;
        let mut img = vec![0u8; total_bytes as usize];
        let data = ((reserved + 2 * fatsz) * bps) as usize;
        let mut fat = vec![0u32; clusters as usize + 2];
        fat[0] = 0x0FFFFFF8;
        fat[1] = EOC;
        let mut next = 2u32;
        let mut alloc = |fat: &mut Vec<u32>, bytes: usize| -> u32 {
            let n = bytes.div_ceil(cb).max(1) as u32;
            let first = next;
            for k in 0..n {
                fat[(first + k) as usize] = if k + 1 == n { EOC } else { first + k + 1 };
            }
            next += n;
            first
        };
        let put = |img: &mut Vec<u8>, c: u32, bytes: &[u8]| {
            let at = data + (c as usize - 2) * cb;
            img[at..at + bytes.len()].copy_from_slice(bytes);
        };
        let entry = |name: &[u8; 11], attr: u8, first: u32, size: u32| -> Vec<u8> {
            let mut e = vec![0u8; 32];
            e[..11].copy_from_slice(name);
            e[11] = attr;
            e[20..22].copy_from_slice(&((first >> 16) as u16).to_le_bytes());
            e[26..28].copy_from_slice(&(first as u16).to_le_bytes());
            e[28..32].copy_from_slice(&size.to_le_bytes());
            e
        };
        let sfn = |s: &str| -> [u8; 11] {
            let mut n = [b' '; 11];
            let (b, x) = s.split_once('.').unwrap_or((s, ""));
            n[..b.len()].copy_from_slice(b.as_bytes());
            n[8..8 + x.len()].copy_from_slice(x.as_bytes());
            n
        };
        // directories: (path, entries)
        let root = alloc(&mut fat, 4 * 32);
        let boot = alloc(&mut fat, 4 * 32);
        let grub = alloc(&mut fat, 4 * 32);
        let (mut re, mut be, mut ge) = (vec![], vec![], vec![]);
        re.extend(entry(b"IYAKKI-BOOT", 0x08, 0, 0));
        // a long-name entry for the BOOT folder (the real image has them for "bzImage" and others)
        let mut lfn = vec![0u8; 32];
        lfn[0] = 0x41;
        lfn[11] = 0x0F;
        for (k, o) in [1usize, 3, 5, 7].into_iter().enumerate() {
            lfn[o] = b"BOOT"[k];
        }
        lfn[13] = sfn("BOOT").iter().fold(0u8, |sum, c| (sum >> 1).wrapping_add((sum & 1) << 7).wrapping_add(*c));
        re.extend(lfn);
        re.extend(entry(&sfn("BOOT"), 0x10, boot, 0));
        be.extend(entry(b".          ", 0x10, boot, 0));
        be.extend(entry(b"..         ", 0x10, 0, 0));
        be.extend(entry(&sfn("GRUB"), 0x10, grub, 0));
        ge.extend(entry(b".          ", 0x10, grub, 0));
        ge.extend(entry(b"..         ", 0x10, boot, 0));
        for (path, content) in files {
            let first = if content.is_empty() { 0 } else { alloc(&mut fat, content.len()) };
            if !content.is_empty() {
                put(&mut img, first, content);
            }
            let (dir, name) = match path.rsplit_once('/') {
                Some((d, n)) => (d, n),
                None => ("", *path),
            };
            // a name with lower-case letters (at most 13 characters) is stored as one long-name entry plus a made-up short name
            let mut e = Vec::new();
            let short = if name.chars().any(|c| c.is_ascii_lowercase()) {
                let (b, x) = name.rsplit_once('.').unwrap_or((name, ""));
                let b6: String = b.chars().filter(|c| c.is_ascii_alphanumeric()).take(6).collect::<String>().to_uppercase();
                let mut n = [b' '; 11];
                let sb = format!("{b6}~1");
                n[..sb.len()].copy_from_slice(sb.as_bytes());
                n[8..8 + x.len().min(3)].copy_from_slice(&x.to_uppercase().as_bytes()[..x.len().min(3)]);
                let mut l = vec![0xFFu8; 32];
                l[0] = 0x41;
                l[11] = 0x0F;
                l[12] = 0;
                l[13] = n.iter().fold(0u8, |sum, c| (sum >> 1).wrapping_add((sum & 1) << 7).wrapping_add(*c));
                l[26] = 0;
                l[27] = 0;
                let units: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
                for (k, o) in [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30].into_iter().enumerate() {
                    let u = units.get(k).copied().unwrap_or(0xFFFF);
                    l[o..o + 2].copy_from_slice(&u.to_le_bytes());
                }
                e.extend(l);
                n
            } else {
                sfn(name)
            };
            e.extend(entry(&short, 0x20, first, content.len() as u32));
            match dir {
                "" => re.extend(e),
                "BOOT" => be.extend(e),
                _ => ge.extend(e),
            }
        }
        put(&mut img, root, &re);
        put(&mut img, boot, &be);
        put(&mut img, grub, &ge);
        // deleted entry in the middle must be ignored
        let b = &mut img[..bps as usize];
        b[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
        b[11..13].copy_from_slice(&(bps as u16).to_le_bytes());
        b[13] = spc as u8;
        b[14..16].copy_from_slice(&(reserved as u16).to_le_bytes());
        b[16] = 2;
        b[32..36].copy_from_slice(&(total as u32).to_le_bytes());
        b[36..40].copy_from_slice(&(fatsz as u32).to_le_bytes());
        b[44..48].copy_from_slice(&root.to_le_bytes());
        b[71..82].copy_from_slice(b"IYAKKI-BOOT");
        b[510..512].copy_from_slice(&[0x55, 0xAA]);
        for (i, f) in fat.iter().enumerate() {
            let at = (reserved * bps) as usize + 4 * i;
            img[at..at + 4].copy_from_slice(&f.to_le_bytes());
            let at2 = ((reserved + fatsz) * bps) as usize + 4 * i;
            img[at2..at2 + 4].copy_from_slice(&f.to_le_bytes());
        }
        img
    }

    fn pseudo(n: usize, seed: u8) -> Vec<u8> {
        (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed).wrapping_add((i >> 8) as u8)).collect()
    }

    pub(crate) fn sample_files() -> Vec<(&'static str, Vec<u8>)> {
        vec![("BZIMAGE", pseudo(3_000_001, 1)), ("BOOT/INITRD", pseudo(5_000_000, 2)), ("BOOT/GRUB/GRUB.CFG", b"set default=0\n".to_vec()), ("EMPTY.TXT", vec![]), ("BOOT/ONE.BIN", vec![7; 1])]
    }

    fn contents(img: &[u8]) -> Vec<(String, bool, Vec<u8>)> {
        let mut c = Cursor::new(img.to_vec());
        let mut f = Fat32::open(&mut c, 0).unwrap();
        let items = f.walk().unwrap();
        items.iter().map(|i| (i.path.clone(), i.dir, if i.dir { vec![] } else { f.read_file(i).unwrap() })).collect()
    }

    fn do_clone(src: &[u8], sector: u32, total: u64) -> (Vec<u8>, Cloned) {
        let mut c = Cursor::new(src.to_vec());
        let mut f = Fat32::open(&mut c, 0).unwrap();
        let mut out = Vec::new();
        let r = clone_to(&mut f, sector, total, "IYAKKI-BOOT", 0x1234_5678, &mut |b| {
            out.extend_from_slice(b);
            Ok(())
        })
        .unwrap();
        (out, r)
    }

    #[test]
    fn the_reader_lists_and_reads_a_fat32_volume() {
        let img = mini_fat(512, 1, 40 << 20, &sample_files());
        let c = contents(&img);
        let names: Vec<&str> = c.iter().map(|x| x.0.as_str()).collect();
        assert_eq!(names, ["/BOOT", "/BOOT/GRUB", "/BOOT/GRUB/GRUB.CFG", "/BOOT/INITRD", "/BOOT/ONE.BIN", "/BZIMAGE", "/EMPTY.TXT"]);
        assert_eq!(c.iter().find(|x| x.0 == "/BZIMAGE").unwrap().2, pseudo(3_000_001, 1));
        let mut cur = Cursor::new(img);
        assert_eq!(Fat32::open(&mut cur, 0).unwrap().label, "IYAKKI-BOOT");
        assert!(Fat32::open(&mut Cursor::new(vec![0u8; 4096]), 0).is_err());
    }

    #[test]
    fn a_512_byte_volume_is_rebuilt_with_4096_byte_sectors_and_keeps_every_file() {
        let src = mini_fat(512, 1, 40 << 20, &sample_files());
        let (out, r) = do_clone(&src, 4096, 300 << 20);
        assert_eq!(out.len(), 300 << 20, "exactly the partition");
        assert_eq!((r.files, r.dirs), (4, 3), "the empty file has no clusters");
        assert_eq!(contents(&out), contents(&src), "same tree, same bytes");
        assert_eq!((le16(&out, 11), out[13], &out[71..82]), (4096, 1, &b"IYAKKI-BOOT"[..]));
        assert_eq!(&out[4094..4096], &[0x55, 0xAA], "signature at the end of a 4096-byte sector too");
        assert_eq!(&out[..4096], &out[6 * 4096..7 * 4096], "backup boot sector");
        // a source with bigger clusters works too
        let big = mini_fat(512, 8, 40 << 20, &sample_files());
        assert_eq!(contents(&do_clone(&big, 4096, 300 << 20).0), contents(&src));
        // and the same volume at 512 bytes per sector
        let (same, _) = do_clone(&src, 512, 128 << 20);
        assert_eq!(contents(&same), contents(&src));
    }

    #[test]
    fn the_lean_files_and_menu_block_are_left_out_of_the_copy_and_everything_else_is_kept() {
        use crate::leanstrip::{LeanFree, BEGIN};
        let cfg = crate::leanstrip::tests_sample().as_bytes().to_vec();
        let files = vec![
            ("BOOT/bzImage", pseudo(2_000_000, 3)),
            ("BOOT/INITRD", pseudo(1_500_000, 4)),
            ("BOOT/initrd-lean", pseudo(300_000, 5)),
            ("BOOT/STAGE1.LOG", b"stage1 log\n".to_vec()),
            ("BOOT/GRUB/grub.cfg", cfg.clone()),
            ("BOOT/GRUB/x.before-lean", cfg.clone()),
            ("BOOT/GRUB/ONE.BIN", vec![9; 3]),
        ];
        let src = mini_fat(512, 1, 40 << 20, &files);
        // the reader sees the long names
        let mut sc = Cursor::new(src.clone());
        let seen: Vec<String> = Fat32::open(&mut sc, 0).unwrap().walk_long().unwrap().into_iter().map(|i| i.path).collect();
        assert!(seen.contains(&"/BOOT/initrd-lean".to_string()) && seen.contains(&"/BOOT/GRUB/grub.cfg".to_string()), "{seen:?}");
        for sector in [512u32, 4096] {
            let mut c = Cursor::new(src.clone());
            let mut f = Fat32::open(&mut c, 0).unwrap();
            let mut out = Vec::new();
            let mut ed = LeanFree::new();
            let total: u64 = if sector == 512 { 128 << 20 } else { 300 << 20 };
            clone_to_with(&mut f, sector, total, "IYAKKI-BOOT", 7, &mut ed, &mut |b| {
                out.extend_from_slice(b);
                Ok(())
            })
            .unwrap();
            assert_eq!(out.len() as u64, total);
            let mut oc = Cursor::new(out.clone());
            let mut o = Fat32::open(&mut oc, 0).unwrap();
            let items = o.walk_long().unwrap();
            let names: Vec<&str> = items.iter().map(|i| i.path.as_str()).collect();
            assert_eq!(names, ["/BOOT", "/BOOT/GRUB", "/BOOT/GRUB/ONE.BIN", "/BOOT/GRUB/grub.cfg", "/BOOT/INITRD", "/BOOT/bzImage"], "{sector}: no lean file, no short-name leftovers");
            let get = |o: &mut Fat32<Cursor<Vec<u8>>>, p: &str| o.read_file(items.iter().find(|i| i.path == p).unwrap()).unwrap();
            assert_eq!(get(&mut o, "/BOOT/bzImage"), pseudo(2_000_000, 3));
            assert_eq!(get(&mut o, "/BOOT/INITRD"), pseudo(1_500_000, 4));
            assert_eq!(get(&mut o, "/BOOT/GRUB/ONE.BIN"), vec![9; 3]);
            let new_cfg = String::from_utf8(get(&mut o, "/BOOT/GRUB/grub.cfg")).unwrap();
            assert!(!new_cfg.contains(BEGIN) && !new_cfg.contains("fast start") && new_cfg.contains("menuentry \"IYAKKI\" {"), "{new_cfg}");
            assert_eq!(items.iter().find(|i| i.path == "/BOOT/GRUB/grub.cfg").unwrap().size as usize, new_cfg.len(), "the size in the entry is the new size");
            assert_eq!(ed.dropped.len(), 3, "{:?}", ed.dropped);
            assert!(ed.menu_changed);
        }
    }

    #[test]
    fn the_new_volume_passes_fsck_when_it_is_installed() {
        let src = mini_fat(512, 1, 40 << 20, &sample_files());
        let (out, _) = do_clone(&src, 4096, 300 << 20);
        let dir = std::env::temp_dir().join(format!("iyakki-fatclone-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("esp.img");
        std::fs::write(&f, &out).unwrap();
        match std::process::Command::new("fsck.fat").args(["-n", "-v"]).arg(&f).output() {
            Ok(o) => {
                let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
                assert!(o.status.success(), "fsck.fat: {text}");
                assert!(text.contains("4096 bytes per logical sector") || text.contains("4096 bytes per cluster"), "{text}");
            }
            Err(_) => eprintln!("fsck.fat is not installed: skipped"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refusals_are_clear() {
        let src = mini_fat(512, 1, 40 << 20, &sample_files());
        let mut c = Cursor::new(src.clone());
        let mut f = Fat32::open(&mut c, 0).unwrap();
        assert!(clone_to(&mut f, 4096, 100 << 20, "X", 1, &mut |_| Ok(())).unwrap_err().contains("65525"), "too few clusters for FAT32");
        assert!(clone_to(&mut f, 4096, 5 << 20, "X", 1, &mut |_| Ok(())).is_err());
        assert!(clone_to(&mut f, 1000, 300 << 20, "X", 1, &mut |_| Ok(())).is_err());
        // a volume whose chain is broken
        let mut bad = src;
        let fat0 = 32 * 512;
        bad[fat0 + 4 * 5..fat0 + 4 * 5 + 4].copy_from_slice(&5u32.to_le_bytes()); // a cluster pointing at itself
        let mut c = Cursor::new(bad);
        let mut f = Fat32::open(&mut c, 0).unwrap();
        assert!(clone_to(&mut f, 4096, 300 << 20, "X", 1, &mut |_| Ok(())).is_err());
    }
}
