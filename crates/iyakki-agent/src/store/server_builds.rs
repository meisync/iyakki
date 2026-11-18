//! The shared build list on the private store: `PUT /v1/builds.json` (a list signed offline by the update key; the store checks the
//! signature, the pinned key when one is set, that every blob it names is there with the right size, and that the serial goes up),
//! `GET /v1/builds.json`, `POST /v1/builds/purge` (the blobs of dropped builds, after the new build is read back) and `POST /v1/builds/gc` (what the store holds, and with `apply=1` removal of blobs nothing refers to).
//! The list is never signed here: the update key stays on the publisher's disk. docs/development/BUILD-SHARING.md.
//!
//! Order that keeps a fetch working during a publish: blobs first, then the list, then the blobs only the PREVIOUS list named.

use std::collections::BTreeSet;

use serde_json::json;

use super::super::buildshare as bs;
use super::super::compmanifest as cm;
use super::{atomic_write, Req, Resp, StoreServer};

impl StoreServer {
    /// Reads need the read token when the store has one (the administrator and publishers may always read).
    pub(super) fn may_read(&self, r: &Req) -> bool {
        self.reader(r).is_some()
    }

    /// Who is reading: `Some("")` when the store is open, the person's name for their own token, `shared` for the shared token and
    /// `publisher` for an administrator or publisher; `None` when the request is refused.
    pub(super) fn reader(&self, r: &Req) -> Option<String> {
        if self.cfg.read_token_hash.is_none() && self.cfg.person_read_tokens.is_empty() {
            return Some(String::new());
        }
        let tok = r.headers.get("authorization").and_then(|h| h.strip_prefix("Bearer ").or_else(|| h.strip_prefix("bearer "))).map(str::trim).unwrap_or("");
        if tok.is_empty() || tok.len() > 200 {
            return None;
        }
        let h = super::hash_token(tok);
        if let Some((name, _)) = self.cfg.person_read_tokens.iter().find(|(_, want)| super::ct_eq(want, &h)) {
            return Some(name.clone());
        }
        if self.cfg.read_token_hash.as_deref().is_some_and(|want| super::ct_eq(want, &h)) {
            return Some("shared".into());
        }
        self.auth(r).map(|_| "publisher".to_string())
    }

    fn builds_list(&self) -> Option<(Vec<u8>, bs::List)> {
        let raw = std::fs::read(self.path("builds.json")).ok()?;
        let l = bs::verify(&raw, "").ok()?;
        Some((raw, l))
    }

    /// Every blob the build list names (the cleanups of the component list must not remove them).
    pub(super) fn build_blob_refs(&self) -> BTreeSet<String> {
        self.builds_list().map(|(_, l)| bs::referenced(&l)).unwrap_or_default()
    }

    pub(super) fn builds_get(&self, r: &Req, peer: &str) -> Resp {
        if !self.may_read(r) {
            return self.unauthorised(peer, &r.path);
        }
        let who = self.reader(r).unwrap_or_default();
        match std::fs::read(self.path("builds.json")) {
            Ok(b) => {
                if !who.is_empty() {
                    self.audit(peer, &who, "builds.read", "list", "200");
                }
                Resp::new(200, "application/json", b)
            }
            Err(_) => Resp::err(404, "no builds have been published"),
        }
    }

    /// `PUT /v1/builds.json`.
    pub(super) fn builds_put(&self, r: &Req, peer: &str) -> Resp {
        let owner = match self.writer(r, peer) {
            Ok(o) => o,
            Err(e) => return e,
        };
        let pin = self.cfg.builds_key.clone().unwrap_or_default();
        let new = match bs::verify(&r.body, &pin) {
            Ok(l) => l,
            Err(e) => {
                self.audit(peer, &owner, "builds", "list refused", "400");
                return Resp::err(400, &e);
            }
        };
        let _g = self.write.lock().unwrap_or_else(|e| e.into_inner());
        let old = self.builds_list().map(|(_, l)| l);
        if old.as_ref().is_some_and(|o| new.serial <= o.serial) {
            return Resp::err(409, "the list's serial must be higher than the stored one (an older list is not accepted back)");
        }
        let mut missing = vec![];
        for b in &new.builds {
            for f in &b.files {
                match std::fs::metadata(self.blob_path(&f.sha256)) {
                    Ok(m) if m.len() == f.size => {}
                    _ => missing.push(format!("{} {}", b.build_id, f.path)),
                }
            }
        }
        if !missing.is_empty() {
            return Resp::err(409, &format!("{} file(s) of the list are not in the store (upload the blobs first): {}", missing.len(), missing.iter().take(5).cloned().collect::<Vec<_>>().join(", ")));
        }
        if let Err(e) = atomic_write(&self.path("builds.json"), &r.body) {
            return Resp::err(500, &e);
        }
        // The previous list's blobs are only NOTED here: a device that is fetching the old build keeps working until the publisher has
        // read the new build back and calls `POST /v1/builds/purge`.
        let mut pending = self.pending_purge();
        pending.extend(old.iter().flat_map(bs::referenced));
        let _ = atomic_write(&self.path("purge-pending.json"), &serde_json::to_vec(&pending).unwrap_or_default());
        self.audit(peer, &owner, "builds", &format!("serial {} builds {}", new.serial, new.builds.len()), "200");
        Resp::json(200, json!({"ok": true, "serial": new.serial, "builds": new.builds.len(), "pending_purge": pending.len()}))
    }

    fn pending_purge(&self) -> BTreeSet<String> {
        std::fs::read(self.path("purge-pending.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// `POST /v1/builds/purge`: remove the blobs earlier lists named that the current list and the component list do not.
    pub(super) fn builds_purge(&self, r: &Req, peer: &str) -> Resp {
        let owner = match self.writer(r, peer) {
            Ok(o) => o,
            Err(e) => return e,
        };
        let _g = self.write.lock().unwrap_or_else(|e| e.into_inner());
        let mut keep = self.build_blob_refs();
        keep.extend(self.component_blob_refs());
        let (mut removed, mut bytes) = (0usize, 0u64);
        for h in self.pending_purge().iter().filter(|h| !keep.contains(*h) && cm::is_sha(h)) {
            let p = self.blob_path(h);
            let n = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            if std::fs::remove_file(&p).is_ok() {
                removed += 1;
                bytes += n;
            }
        }
        let _ = std::fs::remove_file(self.path("purge-pending.json"));
        self.audit(peer, &owner, "builds-purge", &format!("{removed} blobs {bytes} bytes"), "200");
        Resp::json(200, json!({"ok": true, "removed_blobs": removed, "removed_bytes": bytes}))
    }

    pub(super) fn component_blob_refs(&self) -> BTreeSet<String> {
        let mut s = BTreeSet::new();
        for x in self.cstate().records.iter().filter(|x| !x.purged) {
            s.insert(x.entry.sha256.clone());
            s.extend(x.entry.delta.iter().map(|d| d.sha256.clone()));
        }
        s
    }

    /// `POST /v1/builds/gc?apply=1[&grace=SECONDS]` (administrator): the disk the store uses, and blobs nothing refers to older than
    /// `grace` (default a day: an upload that is waiting for its list is younger).
    pub(super) fn builds_gc(&self, r: &Req, peer: &str) -> Resp {
        match self.writer(r, peer) {
            Ok(o) if o == "admin" => {}
            Ok(_) => return Resp::err(403, "only the administrator runs the cleanup"),
            Err(e) => return e,
        }
        let apply = r.query.get("apply").is_some_and(|v| v == "1" || v == "true");
        let grace: i64 = r.query.get("grace").and_then(|g| g.parse().ok()).unwrap_or(86_400).max(0);
        let _g = self.write.lock().unwrap_or_else(|e| e.into_inner());
        let mut keep = self.build_blob_refs();
        keep.extend(self.component_blob_refs());
        let now = self.now();
        let (mut blobs, mut blob_bytes, mut orphans, mut orphan_bytes, mut removed) = (0usize, 0u64, 0usize, 0u64, 0usize);
        for f in std::fs::read_dir(self.path("blobs")).into_iter().flatten().flatten() {
            let name = f.file_name().to_string_lossy().into_owned();
            let Ok(md) = f.metadata() else { continue };
            if !cm::is_sha(&name) {
                continue;
            }
            blobs += 1;
            blob_bytes += md.len();
            let age = md.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| now - d.as_secs() as i64);
            if !keep.contains(&name) && age >= grace {
                orphans += 1;
                orphan_bytes += md.len();
                if apply && std::fs::remove_file(f.path()).is_ok() {
                    removed += 1;
                }
            }
        }
        let incoming: u64 = std::fs::read_dir(self.path("incoming")).into_iter().flatten().flatten().filter_map(|f| f.metadata().ok()).map(|m| m.len()).sum();
        if apply {
            self.audit(peer, "admin", "builds-gc", &format!("{removed} blobs, {orphan_bytes} bytes"), "200");
        }
        Resp::json(200, json!({"ok": true, "applied": apply, "blobs": blobs, "blob_bytes": blob_bytes, "incoming_bytes": incoming, "unreferenced": orphans, "unreferenced_bytes": orphan_bytes, "removed": removed}))
    }
}

