//! In-memory virtual filesystem (MemFS).
//!
//! Nothing here touches host disk. File contents are `Arc`-shared, so
//! [`MemFs::fork`] is O(files) pointer copies and writes after a fork never
//! leak back — the basis for speculative branching in later phases.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VfsError {
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("is a directory: {0}")]
    IsDir(String),
    #[error("quota exceeded ({limit} bytes)")]
    QuotaExceeded { limit: usize },
}

/// Normalize to an absolute path: collapses `.`/`//`, rejects `..` escapes.
pub fn normalize(path: &str) -> Result<String, VfsError> {
    if !path.starts_with('/') {
        return Err(VfsError::InvalidPath(path.to_string()));
    }
    let mut parts: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(VfsError::InvalidPath(path.to_string()));
                }
            }
            s if s.contains('\0') => return Err(VfsError::InvalidPath(path.to_string())),
            s => parts.push(s),
        }
    }
    Ok(format!("/{}", parts.join("/")))
}

#[derive(Clone, Debug)]
pub struct MemFs {
    files: BTreeMap<String, Arc<Vec<u8>>>,
    quota: usize,
}

impl Default for MemFs {
    fn default() -> Self {
        Self::new()
    }
}

impl MemFs {
    pub fn new() -> Self {
        Self::with_quota(64 * 1024 * 1024)
    }

    pub fn with_quota(bytes: usize) -> Self {
        Self { files: BTreeMap::new(), quota: bytes }
    }

    pub fn used(&self) -> usize {
        self.files.values().map(|v| v.len()).sum()
    }

    pub fn write(&mut self, path: &str, data: &[u8]) -> Result<(), VfsError> {
        let p = normalize(path)?;
        if p == "/" || self.is_dir_norm(&p) {
            return Err(VfsError::IsDir(p));
        }
        let old = self.files.get(&p).map_or(0, |v| v.len());
        if self.used() - old + data.len() > self.quota {
            return Err(VfsError::QuotaExceeded { limit: self.quota });
        }
        self.files.insert(p, Arc::new(data.to_vec()));
        Ok(())
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>, VfsError> {
        let p = normalize(path)?;
        match self.files.get(&p) {
            Some(v) => Ok(v.as_ref().clone()),
            None if p == "/" || self.is_dir_norm(&p) => Err(VfsError::IsDir(p)),
            None => Err(VfsError::NotFound(p)),
        }
    }

    pub fn remove(&mut self, path: &str) -> Result<(), VfsError> {
        let p = normalize(path)?;
        self.files.remove(&p).map(|_| ()).ok_or(VfsError::NotFound(p))
    }

    pub fn exists(&self, path: &str) -> bool {
        normalize(path).is_ok_and(|p| self.files.contains_key(&p) || self.is_dir_norm(&p))
    }

    fn is_dir_norm(&self, p: &str) -> bool {
        if p == "/" {
            return true;
        }
        let prefix = format!("{p}/");
        self.files.range(prefix.clone()..).next().is_some_and(|(k, _)| k.starts_with(&prefix))
    }

    /// Immediate children (names only) of a directory, sorted.
    pub fn list(&self, dir: &str) -> Result<Vec<String>, VfsError> {
        let d = normalize(dir)?;
        if self.files.contains_key(&d) {
            return Err(VfsError::NotFound(d));
        }
        if !self.is_dir_norm(&d) {
            return Err(VfsError::NotFound(d));
        }
        let prefix = if d == "/" { "/".to_string() } else { format!("{d}/") };
        let names: BTreeSet<String> = self
            .files
            .range(prefix.clone()..)
            .take_while(|(k, _)| k.starts_with(&prefix))
            .map(|(k, _)| k[prefix.len()..].split('/').next().unwrap().to_string())
            .collect();
        Ok(names.into_iter().collect())
    }

    /// Serialize all files (deterministic order).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend((self.files.len() as u64).to_le_bytes());
        for (k, v) in &self.files {
            out.extend((k.len() as u64).to_le_bytes());
            out.extend(k.as_bytes());
            out.extend((v.len() as u64).to_le_bytes());
            out.extend(v.iter());
        }
        out
    }

    pub fn from_bytes(mut b: &[u8], quota: usize) -> Result<MemFs, VfsError> {
        let bad = || VfsError::InvalidPath("corrupt snapshot".into());
        fn take<'a>(b: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
            if b.len() < n { return None; }
            let (x, y) = b.split_at(n);
            *b = y;
            Some(x)
        }
        let u64_ = |b: &mut &[u8]| take(b, 8).map(|x| u64::from_le_bytes(x.try_into().unwrap()) as usize);
        let n = u64_(&mut b).ok_or_else(bad)?;
        let mut fs = MemFs::with_quota(quota);
        for _ in 0..n {
            let kl = u64_(&mut b).ok_or_else(bad)?;
            let k = std::str::from_utf8(take(&mut b, kl).ok_or_else(bad)?).map_err(|_| bad())?.to_string();
            let dl = u64_(&mut b).ok_or_else(bad)?;
            let d = take(&mut b, dl).ok_or_else(bad)?;
            fs.write(&k, d)?;
        }
        Ok(fs)
    }

    /// Replace this filesystem's contents with another's (used to commit a branch).
    pub fn replace_with(&mut self, other: MemFs) {
        self.files = other.files;
    }

    /// Independent copy sharing file contents until either side writes.
    pub fn fork(&self) -> MemFs {
        self.clone()
    }

    /// Paths whose content differs from, or is absent in, `base`, plus removals.
    pub fn diff(&self, base: &MemFs) -> Vec<(String, DiffKind)> {
        let mut out = Vec::new();
        for (k, v) in &self.files {
            match base.files.get(k) {
                None => out.push((k.clone(), DiffKind::Added)),
                Some(b) if !Arc::ptr_eq(b, v) && b != v => out.push((k.clone(), DiffKind::Modified)),
                _ => {}
            }
        }
        for k in base.files.keys() {
            if !self.files.contains_key(k) {
                out.push((k.clone(), DiffKind::Removed));
            }
        }
        out.sort();
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiffKind {
    Added,
    Modified,
    Removed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_paths() {
        assert_eq!(normalize("/a//b/./c").unwrap(), "/a/b/c");
        assert_eq!(normalize("/a/b/../c").unwrap(), "/a/c");
        assert!(normalize("/..").is_err());
        assert!(normalize("/a/../../etc").is_err());
        assert!(normalize("relative").is_err());
    }

    #[test]
    fn read_write_list() {
        let mut fs = MemFs::new();
        fs.write("/w/a.txt", b"hi").unwrap();
        fs.write("/w/sub/b.txt", b"yo").unwrap();
        assert_eq!(fs.read("/w/a.txt").unwrap(), b"hi");
        assert_eq!(fs.list("/w").unwrap(), vec!["a.txt", "sub"]);
        assert_eq!(fs.list("/").unwrap(), vec!["w"]);
        assert!(matches!(fs.read("/w"), Err(VfsError::IsDir(_))));
        assert!(matches!(fs.write("/w", b"x"), Err(VfsError::IsDir(_))));
        assert!(matches!(fs.read("/nope"), Err(VfsError::NotFound(_))));
    }

    #[test]
    fn quota_enforced_and_overwrite_counts_net() {
        let mut fs = MemFs::with_quota(10);
        fs.write("/a", &[0; 8]).unwrap();
        assert!(matches!(fs.write("/b", &[0; 3]), Err(VfsError::QuotaExceeded { .. })));
        fs.write("/a", &[0; 10]).unwrap();
    }

    #[test]
    fn serialize_roundtrip() {
        let mut fs = MemFs::new();
        fs.write("/a/b", b"123").unwrap();
        fs.write("/c", b"").unwrap();
        let r = MemFs::from_bytes(&fs.to_bytes(), 1 << 20).unwrap();
        assert_eq!(r.read("/a/b").unwrap(), b"123");
        assert!(r.exists("/c"));
        assert!(MemFs::from_bytes(&fs.to_bytes()[..5], 1 << 20).is_err());
    }

    #[test]
    fn fork_is_isolated() {
        let mut base = MemFs::new();
        base.write("/a", b"1").unwrap();
        let mut f = base.fork();
        f.write("/a", b"2").unwrap();
        f.write("/b", b"3").unwrap();
        f.remove("/a").unwrap();
        assert_eq!(base.read("/a").unwrap(), b"1");
        assert!(!base.exists("/b"));
        assert_eq!(
            f.diff(&base),
            vec![("/a".to_string(), DiffKind::Removed), ("/b".to_string(), DiffKind::Added)]
        );
    }
}
