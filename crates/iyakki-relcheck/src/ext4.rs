//! File lists of an ext2/ext3/ext4 volume held in memory (F-QA-05): superblock, group descriptors, inodes, extent trees or block maps and
//! directory entries, read with no dependency and nothing extracted. Reading only. Every number comes from the image, so each read is
//! bounded and loops (a directory that points back, a block map that points at itself) are cut: a volume that is cut short or damaged is
//! an error for the whole image, never a pass. Not read: inline data (small files and directories stored in the inode), file contents
//! (names, kinds, sizes, modes and symlink targets are enough for the scan).
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashSet;

use crate::cpio::{norm, Entry, Kind};

const MAX_ENTRIES: usize = 1_000_000;
const MAX_DEPTH: usize = 40;
const ROOT: u32 = 2;

fn le16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn le32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// Does this volume start with an ext superblock (magic 0xEF53 at byte 1080)?
pub fn is_ext(vol: &[u8]) -> bool {
    le16(vol, 1080) == Some(0xEF53)
}

struct Fs<'a> {
    v: &'a [u8],
    bs: usize,
    ipg: u32,
    isz: usize,
    first_data: usize,
    desc_size: usize,
    big: bool,
    out: Vec<Entry>,
    seen: HashSet<u32>,
}

struct Inode {
    mode: u32,
    size: u64,
    flags: u32,
    block: [u8; 60],
}

impl<'a> Fs<'a> {
    fn open(v: &'a [u8]) -> Result<Fs<'a>, String> {
        let sb = v.get(1024..2048).ok_or("the volume is cut short (no superblock)")?;
        if le16(sb, 56) != Some(0xEF53) {
            return Err("not an ext file system".into());
        }
        let log = le32(sb, 24).ok_or("damaged superblock")?;
        if log > 6 {
            return Err("the block size in the superblock is not believable".into());
        }
        let bs = 1024usize << log;
        let incompat = le32(sb, 96).unwrap_or(0);
        if incompat & 0x8000 != 0 {
            // INLINE_DATA is a feature of the volume, but an inode only uses it when its flag says so; checked per inode
        }
        let big = incompat & 0x80 != 0;
        let desc_size = if big { le16(sb, 254).unwrap_or(64).max(64) as usize } else { 32 };
        let rev = le32(sb, 76).unwrap_or(0);
        let isz = if rev >= 1 { le16(sb, 88).unwrap_or(128) as usize } else { 128 };
        let ipg = le32(sb, 40).ok_or("damaged superblock")?;
        if ipg == 0 || isz < 128 || isz > bs || !isz.is_power_of_two() {
            return Err("the inode geometry in the superblock is not believable".into());
        }
        Ok(Fs { v, bs, ipg, isz, first_data: le32(sb, 20).unwrap_or(0) as usize, desc_size, big, out: Vec::new(), seen: HashSet::new() })
    }

    fn block(&self, n: u64) -> Result<&'a [u8], String> {
        let start = (n as usize).checked_mul(self.bs).ok_or("a block number is outside the volume")?;
        self.v.get(start..start + self.bs).ok_or_else(|| format!("block {n} lies outside the volume (cut short?)"))
    }

    fn inode(&self, n: u32) -> Result<Inode, String> {
        if n == 0 {
            return Err("inode 0".into());
        }
        let (group, idx) = (((n - 1) / self.ipg) as usize, ((n - 1) % self.ipg) as usize);
        let gdt = (self.first_data + 1) * self.bs;
        let d = self.v.get(gdt + group * self.desc_size..gdt + (group + 1) * self.desc_size).ok_or("a group descriptor lies outside the volume")?;
        let mut table = le32(d, 8).ok_or("damaged group descriptor")? as u64;
        if self.big && self.desc_size >= 64 {
            table |= (le32(d, 0x28).unwrap_or(0) as u64) << 32;
        }
        let at = (table as usize).checked_mul(self.bs).and_then(|b| b.checked_add(idx * self.isz)).ok_or("an inode lies outside the volume")?;
        let r = self.v.get(at..at + self.isz.min(256).max(128)).ok_or_else(|| format!("inode {n} lies outside the volume (cut short?)"))?;
        let mode = le16(r, 0).ok_or("damaged inode")? as u32;
        let mut size = le32(r, 4).ok_or("damaged inode")? as u64;
        if mode & 0o170000 == 0o100000 {
            size |= (le32(r, 108).unwrap_or(0) as u64) << 32;
        }
        let mut block = [0u8; 60];
        block.copy_from_slice(r.get(40..100).ok_or("damaged inode")?);
        Ok(Inode { mode, size, flags: le32(r, 32).ok_or("damaged inode")?, block })
    }

    /// The data blocks of an inode in file order (at most `limit` blocks).
    fn blocks(&self, ino: &Inode, limit: usize) -> Result<Vec<u64>, String> {
        if ino.flags & 0x1000_0000 != 0 {
            return Err("this entry keeps its data inside the inode (inline data), which the scan does not read".into());
        }
        let mut out = Vec::new();
        if ino.flags & 0x8_0000 != 0 {
            self.extents(&ino.block, 0, limit, &mut out)?;
        } else {
            let p = |i: usize| le32(&ino.block, i * 4).unwrap_or(0) as u64;
            for i in 0..12 {
                if out.len() >= limit {
                    return Ok(out);
                }
                out.push(p(i));
            }
            for (lvl, slot) in [(1u32, 12usize), (2, 13), (3, 14)] {
                if out.len() >= limit {
                    break;
                }
                self.indirect(p(slot), lvl, limit, &mut out, 0)?;
            }
        }
        out.truncate(limit);
        Ok(out)
    }

    fn indirect(&self, blk: u64, lvl: u32, limit: usize, out: &mut Vec<u64>, depth: usize) -> Result<(), String> {
        let per = self.bs / 4;
        if blk == 0 {
            // a hole: the blocks it would map are zeros
            let n = per.pow(lvl);
            out.extend(std::iter::repeat_n(0, n.min(limit.saturating_sub(out.len()))));
            return Ok(());
        }
        if depth > 3 {
            return Err("a block map is nested deeper than any real file system".into());
        }
        let b = self.block(blk)?;
        for i in 0..per {
            if out.len() >= limit {
                return Ok(());
            }
            let child = le32(b, i * 4).unwrap_or(0) as u64;
            if lvl == 1 {
                out.push(child);
            } else {
                self.indirect(child, lvl - 1, limit, out, depth + 1)?;
            }
        }
        Ok(())
    }

    fn extents(&self, node: &[u8], depth: usize, limit: usize, out: &mut Vec<u64>) -> Result<(), String> {
        if depth > 5 {
            return Err("an extent tree is nested deeper than any real file system".into());
        }
        if le16(node, 0) != Some(0xF30A) {
            return Err("an extent tree has no valid header".into());
        }
        let (entries, level) = (le16(node, 2).ok_or("damaged extent header")? as usize, le16(node, 6).ok_or("damaged extent header")?);
        for i in 0..entries {
            if out.len() >= limit {
                return Ok(());
            }
            let e = node.get(12 + 12 * i..24 + 12 * i).ok_or("an extent entry is cut short")?;
            if level == 0 {
                let (first, mut len) = (le32(e, 0).unwrap() as usize, le16(e, 4).unwrap() as usize);
                if len > 32768 {
                    len -= 32768; // an unwritten extent still owns its blocks
                }
                let start = le32(e, 8).unwrap() as u64 | (le16(e, 6).unwrap() as u64) << 32;
                if first > limit {
                    break;
                }
                if out.len() < first {
                    out.extend(std::iter::repeat_n(0, first - out.len())); // a hole
                }
                for k in 0..len {
                    if out.len() >= limit {
                        return Ok(());
                    }
                    out.push(start + k as u64);
                }
            } else {
                let leaf = le32(e, 4).unwrap() as u64 | (le16(e, 8).unwrap() as u64) << 32;
                let b = self.block(leaf)?;
                self.extents(b, depth + 1, limit, out)?;
            }
        }
        Ok(())
    }

    /// The first `want` bytes of an inode's data.
    fn read(&self, ino: &Inode, want: usize) -> Result<Vec<u8>, String> {
        let want = want.min(ino.size as usize);
        let blocks = self.blocks(ino, want.div_ceil(self.bs))?;
        let mut out = Vec::with_capacity(want);
        for b in blocks {
            let take = (want - out.len()).min(self.bs);
            if b == 0 {
                out.extend(std::iter::repeat_n(0u8, take));
            } else {
                out.extend_from_slice(&self.block(b)?[..take]);
            }
            if out.len() >= want {
                break;
            }
        }
        Ok(out)
    }

    fn dir(&mut self, n: u32, prefix: &str, depth: usize) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("the directories are nested deeper than any real file system".into());
        }
        if !self.seen.insert(n) {
            return Err("a directory points back to one above it".into());
        }
        let ino = self.inode(n)?;
        if ino.mode & 0o170000 != 0o040000 {
            return Err(format!("inode {n} is not a directory"));
        }
        let data = self.read(&ino, ino.size.min(1 << 28) as usize)?;
        let mut at = 0usize;
        while at + 8 <= data.len() {
            let (child, rec, nl) = (le32(&data, at).unwrap(), le16(&data, at + 4).unwrap() as usize, data[at + 6] as usize);
            if rec < 8 || rec % 4 != 0 || at + rec > data.len() || 8 + nl > rec {
                return Err("a directory entry is damaged".into());
            }
            let name = String::from_utf8_lossy(&data[at + 8..at + 8 + nl]).into_owned();
            at += rec;
            if child == 0 || name == "." || name == ".." || name.is_empty() {
                continue;
            }
            if self.out.len() >= MAX_ENTRIES {
                return Err("more entries than any real image holds".into());
            }
            let ci = self.inode(child)?;
            let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            let kind = match ci.mode & 0o170000 {
                0o040000 => Kind::Dir,
                0o100000 => Kind::File,
                0o120000 => Kind::Symlink,
                _ => Kind::Other,
            };
            let target = if kind == Kind::Symlink {
                if ci.size < 60 && ci.flags & 0x8_0000 == 0 && ci.flags & 0x1000_0000 == 0 {
                    // a fast symbolic link keeps its target in the inode's block field
                    String::from_utf8_lossy(&ci.block[..ci.size as usize]).into_owned()
                } else if ci.flags & 0x1000_0000 != 0 {
                    String::new()
                } else {
                    String::from_utf8_lossy(&self.read(&ci, 4096)?).into_owned()
                }
            } else {
                String::new()
            };
            let is_dir = kind == Kind::Dir;
            self.out.push(Entry { path: norm(&path), kind, size: ci.size, mode: ci.mode & 0o7777, target });
            if is_dir {
                self.dir(child, &path, depth + 1)?;
            }
        }
        Ok(())
    }
}

/// Every file, folder and symbolic link of an ext2/3/4 volume held in memory.
pub fn ext_list(vol: &[u8]) -> Result<Vec<Entry>, String> {
    let mut f = Fs::open(vol)?;
    f.dir(ROOT, "", 0)?;
    Ok(f.out)
}
