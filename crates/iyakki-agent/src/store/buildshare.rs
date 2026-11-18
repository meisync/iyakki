//! The shared build list: which update packs (one per machine type and channel) a team's store holds, signed by the UPDATE key
//! (never the Secure Boot key, never in the cluster). The packs' files are plain blobs (`/v1/blobs/<sha256>`, `compmanifest`'s
//! store route); this list says which blobs make which build. Same document signing as the component list (`trust::sign_doc`), its own
//! purpose string `builds`, so a component list can never pass for a build list. Plan and use: docs/development/BUILD-SHARING.md.

use std::collections::BTreeSet;
use std::path::Path;

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::trust::{sign_doc, verify_doc};
use crate::pkg::public_key_hex;

pub const PATH: &str = "/v1/builds.json";
pub const PURPOSE: &str = "builds";
/// The most bytes read for the list.
pub const MAX_DOC: usize = 2 << 20;
pub const MAX_FILES: usize = 400;
pub const MAX_KEEP: usize = 50;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Build {
    /// The machine type (`pc-x86_64`).
    pub variant: String,
    pub channel: String,
    pub build_id: String,
    pub version: String,
    /// The `variant` the pack's own `update.json` says (`pc`): what `dev update` checks on the device.
    #[serde(default)]
    pub pack_variant: String,
    #[serde(default)]
    pub commit: String,
    /// Unix seconds the build was published.
    pub published: i64,
    /// The list's serial when it was published: orders builds exactly (clocks may not).
    pub seq: u64,
    pub total: u64,
    pub files: Vec<BFile>,
    /// Layered packs only (format 2): the SHA-256 of every layer the device must ALREADY hold for this build to install (the `keep`
    /// layers and the bases of deltas). A full pack needs nothing. Part of the signed list, so a store cannot hide a dependency.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct List {
    pub id: String,
    pub serial: u64,
    pub issued: i64,
    pub key: String,
    pub builds: Vec<Build>,
}

pub fn is_sha(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn plain(s: &str, max: usize, extra: &str) -> bool {
    !s.is_empty() && s.len() <= max && s.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || extra.as_bytes().contains(&b))
}

/// `pc-x86_64`, `pc-aarch64`, `mac-arm64`: lower case, digits, `-`, `_`, `.`.
pub fn valid_variant(s: &str) -> bool {
    plain(s, 40, "-_.") && !s.bytes().any(|b| b.is_ascii_uppercase())
}

pub fn valid_channel(s: &str) -> bool {
    valid_variant(s)
}

pub fn valid_build_id(s: &str) -> bool {
    plain(s, 64, "-_.")
}

pub fn valid_file_path(p: &str) -> bool {
    !p.is_empty() && p.len() <= 200 && !p.starts_with('/') && p.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && c.bytes().all(|b| b.is_ascii_graphic() && b != b'\\'))
}

/// One build's own rules (names, files, sizes).
pub fn check_build(b: &Build) -> Result<(), String> {
    if !valid_variant(&b.variant) || !valid_channel(&b.channel) || !valid_build_id(&b.build_id) {
        return Err("a build names its machine type, channel and id with letters, digits, - _ . only".into());
    }
    if b.version.is_empty() || b.version.len() > 64 {
        return Err("a build has a version".into());
    }
    if b.files.is_empty() || b.files.len() > MAX_FILES {
        return Err(format!("a build carries 1 to {MAX_FILES} files"));
    }
    if b.needs.len() > 64 || b.needs.iter().any(|n| !is_sha(n)) {
        return Err("a build's needs are layer hashes (at most 64)".into());
    }
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    for f in &b.files {
        if !valid_file_path(&f.path) || !is_sha(&f.sha256) || !seen.insert(f.path.as_str()) {
            return Err(format!("{:?} is not a usable file entry", f.path));
        }
        total = total.saturating_add(f.size);
    }
    if total != b.total {
        return Err("the build's total does not add up".into());
    }
    Ok(())
}

/// Read a pack folder: `update.json` must exist and every listed file match its size and hash (the device's own check, reused). The
/// build's files are those and `update.json` itself.
pub fn read_pack(dir: &Path) -> Result<(String, String, Vec<BFile>), String> {
    use crate::devmode::update as up;
    let m = up::read_manifest(dir)?;
    up::verify_payload(dir, &m)?;
    let mut files: Vec<BFile> = m.files.iter().map(|f| BFile { path: f.path.clone(), sha256: f.sha256.clone(), size: f.size }).collect();
    let raw = std::fs::read(dir.join(up::MANIFEST)).map_err(|e| e.to_string())?;
    files.push(BFile { path: up::MANIFEST.into(), sha256: up::sha256_hex(&raw), size: raw.len() as u64 });
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((m.version, m.variant, files))
}

/// The layers a layered pack (format 2) needs the device to hold already: the `keep` layers by their hash in the promised set, and the
/// base of every delta. Sorted, no duplicates. A format 1 pack or a pack of whole layers only needs nothing.
pub fn pack_needs(dir: &Path) -> Result<Vec<String>, String> {
    let m = crate::devmode::update::read_manifest(dir)?;
    let Some(pl) = m.layers else { return Ok(vec![]) };
    let mut out = BTreeSet::new();
    for (it, l) in pl.items.iter().zip(&pl.set.layers) {
        match it.how.as_str() {
            "keep" => {
                out.insert(l.sha256.clone());
            }
            "delta" => {
                out.insert(it.delta_from.clone());
            }
            _ => {}
        }
    }
    Ok(out.into_iter().collect())
}

/// The newest build of a machine type and channel (or the named one) that this device can install. `held` is the SHA-256 of every
/// layer the device has (`None`: a single-root device, which holds none). A build whose `needs` are not all held is skipped; when
/// every candidate is skipped the error names the missing layers, so the publisher knows to publish a full pack too (keep 2 or more).
pub fn choose_for_device<'a>(l: &'a List, variant: &str, channel: &str, which: &str, held: Option<&BTreeSet<String>>) -> Result<&'a Build, String> {
    let empty = BTreeSet::new();
    let held = held.unwrap_or(&empty);
    if which != "latest" {
        let b = find(l, variant, channel, which)?;
        return match b.needs.iter().find(|n| !held.contains(*n)) {
            None => Ok(b),
            Some(n) => Err(format!("build {which} needs a layer this device does not have ({}): it was made against another base", &n[..12])),
        };
    }
    let mut cands: Vec<&Build> = l.builds.iter().filter(|b| b.variant == variant && b.channel == channel).collect();
    cands.sort_by(|a, b| b.seq.cmp(&a.seq));
    let first_missing = cands.first().and_then(|b| b.needs.iter().find(|n| !held.contains(*n)).cloned());
    cands.into_iter().find(|b| b.needs.iter().all(|n| held.contains(n))).ok_or_else(|| match first_missing {
        Some(n) => format!("the newest build needs a layer this device does not have ({}) and the store keeps no full pack to fall back to: ask the publisher for a full pack (publish with --keep 2 or more)", &n[..12]),
        None => format!("the store has no build for {variant} in channel {channel}"),
    })
}

/// A folder holding a build's files at their paths: every file there, the right size, the right hash, then the pack's own check.
pub fn verify_dir(dir: &Path, b: &Build) -> Result<(), String> {
    for f in &b.files {
        let p = dir.join(&f.path);
        let meta = std::fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", f.path))?;
        if !meta.is_file() || meta.len() != f.size {
            return Err(format!("{} is not the size the list says", f.path));
        }
        if crate::devmode::files::sha256_file(&p)? != f.sha256 {
            return Err(format!("{} does not match the list's SHA-256: damaged or changed", f.path));
        }
    }
    use crate::devmode::update as up;
    let m = up::read_manifest(dir)?;
    up::verify_payload(dir, &m)
}

/// Add `b` (publishing it as the newest) and apply the policy "keep the newest `keep` builds per machine type and channel". Publishing
/// a build id that is already there replaces it. Returns the new list and the builds that fell out.
pub fn add_build(old: Option<&List>, id: &str, mut b: Build, keep: usize, now: i64) -> Result<(List, Vec<Build>), String> {
    if !(1..=MAX_KEEP).contains(&keep) {
        return Err(format!("keep is a number from 1 to {MAX_KEEP}"));
    }
    let mut l = old.cloned().unwrap_or_default();
    l.id = id.to_string();
    l.serial += 1;
    l.issued = now;
    b.seq = l.serial;
    b.total = b.files.iter().map(|f| f.size).sum();
    check_build(&b)?;
    l.builds.retain(|x| !(x.variant == b.variant && x.channel == b.channel && x.build_id == b.build_id));
    let (variant, channel) = (b.variant.clone(), b.channel.clone());
    l.builds.push(b);
    let mut same: Vec<Build> = l.builds.iter().filter(|x| x.variant == variant && x.channel == channel).cloned().collect();
    same.sort_by(|a, b| b.seq.cmp(&a.seq));
    let dropped: Vec<Build> = same.into_iter().skip(keep).collect();
    l.builds.retain(|x| !dropped.iter().any(|d| d.variant == x.variant && d.channel == x.channel && d.build_id == x.build_id));
    l.builds.sort_by(|a, b| a.variant.cmp(&b.variant).then(a.channel.cmp(&b.channel)).then(a.seq.cmp(&b.seq)));
    Ok((l, dropped))
}

/// Promote a build to another channel (`dev` to `beta`): the same files (the store holds each blob once, so nothing is uploaded again)
/// become the newest build of the target channel. `which` is `latest` or a build id of the source channel. Returns the new list, the
/// builds that fell out of the target channel, and the promoted build.
pub fn promote(old: &List, id: &str, variant: &str, from: &str, to: &str, which: &str, keep: usize, now: i64) -> Result<(List, Vec<Build>, Build), String> {
    if from == to {
        return Err("the source and the target channel are the same".into());
    }
    if !valid_channel(to) {
        return Err("the target channel is lower case letters, digits, - _ . only".into());
    }
    let src = find(old, variant, from, which)?.clone();
    let mut b = src;
    b.channel = to.to_string();
    b.published = now;
    let (l, dropped) = add_build(Some(old), id, b, keep, now)?;
    let promoted = latest(&l, variant, to).cloned().ok_or("the promoted build is missing")?;
    Ok((l, dropped, promoted))
}

/// The newest build of a machine type and channel.
pub fn latest<'a>(l: &'a List, variant: &str, channel: &str) -> Option<&'a Build> {
    l.builds.iter().filter(|b| b.variant == variant && b.channel == channel).max_by_key(|b| b.seq)
}

/// `latest` or a build id.
pub fn find<'a>(l: &'a List, variant: &str, channel: &str, which: &str) -> Result<&'a Build, String> {
    let hit = if which == "latest" { latest(l, variant, channel) } else { l.builds.iter().find(|b| b.variant == variant && b.channel == channel && b.build_id == which) };
    hit.ok_or_else(|| format!("the store has no {which} build for {variant} in channel {channel}"))
}

/// Every blob some build refers to.
pub fn referenced(l: &List) -> BTreeSet<String> {
    l.builds.iter().flat_map(|b| b.files.iter().map(|f| f.sha256.clone())).collect()
}

pub fn build_doc(l: &List, key: &SigningKey) -> String {
    let mut latest_map = serde_json::Map::new();
    for b in &l.builds {
        if latest(l, &b.variant, &b.channel).is_some_and(|x| x.build_id == b.build_id) {
            latest_map.insert(format!("{}/{}", b.variant, b.channel), Value::String(b.build_id.clone()));
        }
    }
    let mut doc = json!({"v": 1, "id": l.id, "serial": l.serial, "issued": l.issued, "builds": l.builds, "latest": latest_map, "key": public_key_hex(key)});
    let sig = sign_doc(PURPOSE, &doc, key);
    doc["sig"] = Value::String(sig);
    doc.to_string()
}

/// Parse a list and check its signature against `pin` (the update public key, hex); `pin` empty accepts any key that signed it (the
/// server's own sanity check). Every build is checked, and the `latest` pointers must name builds that are in the list.
pub fn verify(body: &[u8], pin: &str) -> Result<List, String> {
    if body.len() > MAX_DOC {
        return Err("the build list is too large: refused".into());
    }
    let v: Value = serde_json::from_slice(body).map_err(|_| "the answer is not a build list".to_string())?;
    let (key, sig) = (v["key"].as_str().unwrap_or(""), v["sig"].as_str().unwrap_or(""));
    if sig.is_empty() || (!pin.is_empty() && !key.eq_ignore_ascii_case(pin)) || !verify_doc(PURPOSE, &v, key, sig) {
        return Err("the build list is not signed by the key you pinned: it is refused".into());
    }
    let builds: Vec<Build> = serde_json::from_value(v["builds"].clone()).map_err(|e| format!("the build list is damaged: {e}"))?;
    for b in &builds {
        check_build(b).map_err(|e| format!("the build list has a bad build: {e}"))?;
    }
    let l = List { id: v["id"].as_str().unwrap_or("").to_string(), serial: v["serial"].as_u64().unwrap_or(0), issued: v["issued"].as_i64().unwrap_or(0), key: key.to_ascii_lowercase(), builds };
    if let Some(m) = v["latest"].as_object() {
        for (k, id) in m {
            let ok = k.split_once('/').zip(id.as_str()).is_some_and(|((va, ch), id)| latest(&l, va, ch).is_some_and(|b| b.build_id == id));
            if !ok {
                return Err("the build list's latest pointer names a build that is not newest".into());
            }
        }
    }
    Ok(l)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> SigningKey {
        SigningKey::from_bytes(&[n; 32])
    }

    fn build(id: &str, variant: &str, blobs: &[(&str, char)]) -> Build {
        Build { variant: variant.into(), channel: "dev".into(), build_id: id.into(), version: "0.1.0".into(), pack_variant: "pc".into(), commit: "abc".into(), published: 1, seq: 0, total: 0, files: blobs.iter().map(|(p, c)| BFile { path: (*p).into(), sha256: c.to_string().repeat(64), size: 10 }).collect(), needs: vec![] }
    }

    #[test]
    fn keep_one_drops_the_old_build_but_not_blobs_the_new_one_reuses() {
        let (l1, d1) = add_build(None, "s", build("a", "pc-x86_64", &[("boot/ROOT.SFS", '1'), ("boot/bzImage", '2')]), 1, 10).unwrap();
        assert!(d1.is_empty());
        let (l2, d2) = add_build(Some(&l1), "s", build("b", "pc-x86_64", &[("boot/ROOT.SFS", '3'), ("boot/bzImage", '2')]), 1, 20).unwrap();
        assert_eq!(d2.len(), 1);
        assert_eq!(d2[0].build_id, "a");
        assert_eq!(l2.builds.len(), 1);
        let (old, new) = (referenced(&l1), referenced(&l2));
        let gone: Vec<_> = old.difference(&new).collect();
        assert_eq!(gone.len(), 1, "only the root image of the old build is unique");
        assert!(new.contains(&"2".repeat(64)), "the shared blob stays");
    }

    #[test]
    fn retention_is_per_machine_type_and_channel() {
        let (l, _) = add_build(None, "s", build("a", "pc-x86_64", &[("a", '1')]), 1, 1).unwrap();
        let (l, _) = add_build(Some(&l), "s", build("b", "pc-aarch64", &[("a", '2')]), 1, 2).unwrap();
        let mut c = build("c", "pc-x86_64", &[("a", '3')]);
        c.channel = "beta".into();
        let (l, d) = add_build(Some(&l), "s", c, 1, 3).unwrap();
        assert!(d.is_empty() && l.builds.len() == 3);
        let (l, d) = add_build(Some(&l), "s", build("d", "pc-x86_64", &[("a", '4')]), 3, 4).unwrap();
        assert!(d.is_empty());
        let (l, d) = add_build(Some(&l), "s", build("e", "pc-x86_64", &[("a", '5')]), 2, 5).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].build_id, "a");
        assert_eq!(latest(&l, "pc-x86_64", "dev").unwrap().build_id, "e");
        assert!(add_build(Some(&l), "s", build("f", "pc-x86_64", &[("a", '6')]), 0, 6).is_err());
    }

    #[test]
    fn promoting_copies_the_build_into_another_channel_without_new_blobs() {
        let (l, _) = add_build(None, "s", build("a", "pc-x86_64", &[("boot/ROOT.SFS", '1')]), 2, 1).unwrap();
        let (l, _) = add_build(Some(&l), "s", build("b", "pc-x86_64", &[("boot/ROOT.SFS", '2')]), 2, 2).unwrap();
        let (p, dropped, got) = promote(&l, "s", "pc-x86_64", "dev", "beta", "latest", 1, 9).unwrap();
        assert!(dropped.is_empty());
        assert_eq!((got.build_id.as_str(), got.channel.as_str()), ("b", "beta"));
        assert_eq!(referenced(&p), referenced(&l), "no blob is new");
        assert_eq!(latest(&p, "pc-x86_64", "dev").unwrap().build_id, "b", "the source channel keeps its builds");
        assert_eq!(p.builds.len(), l.builds.len() + 1);
        // a named older build, keep 1 in beta: the earlier promotion falls out of beta only
        let (p2, dropped, got2) = promote(&p, "s", "pc-x86_64", "dev", "beta", "a", 1, 10).unwrap();
        assert_eq!((got2.build_id.as_str(), dropped.len(), dropped[0].build_id.as_str()), ("a", 1, "b"));
        assert_eq!(latest(&p2, "pc-x86_64", "beta").unwrap().build_id, "a");
        // the signed list verifies with the promotion in it
        let doc = build_doc(&p2, &key(1));
        assert!(verify(doc.as_bytes(), &public_key_hex(&key(1))).is_ok());
        assert!(promote(&l, "s", "pc-x86_64", "dev", "dev", "latest", 1, 1).is_err());
        assert!(promote(&l, "s", "pc-x86_64", "dev", "Beta!", "latest", 1, 1).is_err());
        assert!(promote(&l, "s", "pc-x86_64", "alpha", "beta", "latest", 1, 1).is_err(), "nothing in the source channel");
        assert!(promote(&l, "s", "pc-x86_64", "dev", "beta", "zzz", 1, 1).is_err());
    }

    #[test]
    fn a_layered_build_is_chosen_only_for_a_device_that_holds_its_base_otherwise_the_newest_full_one() {
        let h = |c: char| c.to_string().repeat(64);
        let (l, _) = add_build(None, "s", build("full1", "pc-x86_64", &[("a", '1')]), 3, 1).unwrap();
        let mut lay = build("lay2", "pc-x86_64", &[("b", '2')]);
        lay.needs = vec![h('a'), h('b')];
        let (l, _) = add_build(Some(&l), "s", lay, 3, 2).unwrap();
        let both: BTreeSet<String> = [h('a'), h('b'), h('c')].into_iter().collect();
        let one: BTreeSet<String> = [h('a')].into_iter().collect();
        assert_eq!(choose_for_device(&l, "pc-x86_64", "dev", "latest", Some(&both)).unwrap().build_id, "lay2", "the device holds the base: the newest");
        assert_eq!(choose_for_device(&l, "pc-x86_64", "dev", "latest", Some(&one)).unwrap().build_id, "full1", "a missing base falls back to the full pack");
        assert_eq!(choose_for_device(&l, "pc-x86_64", "dev", "latest", None).unwrap().build_id, "full1", "a single-root device holds no layers");
        let e = choose_for_device(&l, "pc-x86_64", "dev", "lay2", Some(&one)).unwrap_err();
        assert!(e.contains("another base") && e.contains(&h('b')[..12]), "{e}");
        // only a layered build in the store: the error says what to do
        let (only, _) = add_build(None, "s", { let mut b = build("lay3", "pc-x86_64", &[("c", '3')]); b.needs = vec![h('d')]; b }, 1, 1).unwrap();
        let e = choose_for_device(&only, "pc-x86_64", "dev", "latest", Some(&one)).unwrap_err();
        assert!(e.contains("full pack") && e.contains("--keep 2"), "{e}");
        assert!(choose_for_device(&only, "pc-x86_64", "beta", "latest", None).unwrap_err().contains("no build"));
        // the needs are signed: changing one in the document breaks the signature, and a bad hash is refused outright
        let mut b2 = build("bad", "pc-x86_64", &[("a", '1')]);
        b2.needs = vec!["xyz".into()];
        assert!(check_build(&{ let mut x = b2.clone(); x.total = 10; x }).is_err());
        let doc = build_doc(&only, &key(1));
        assert!(verify(doc.replace(&h('d'), &h('e')).as_bytes(), &public_key_hex(&key(1))).is_err());
        assert!(verify(doc.as_bytes(), &public_key_hex(&key(1))).unwrap().builds[0].needs == vec![h('d')]);
    }

    #[test]
    fn republishing_an_id_replaces_it() {
        let (l, _) = add_build(None, "s", build("a", "pc-x86_64", &[("a", '1')]), 3, 1).unwrap();
        let (l, d) = add_build(Some(&l), "s", build("a", "pc-x86_64", &[("a", '2')]), 3, 2).unwrap();
        assert!(d.is_empty());
        assert_eq!(l.builds.len(), 1);
        assert_eq!(l.builds[0].files[0].sha256, "2".repeat(64));
    }

    #[test]
    fn signature_pin_and_tamper() {
        let (l, _) = add_build(None, "s", build("a", "pc-x86_64", &[("boot/x", '1')]), 1, 1).unwrap();
        let doc = build_doc(&l, &key(1));
        let pin = public_key_hex(&key(1));
        let back = verify(doc.as_bytes(), &pin).unwrap();
        assert_eq!(back.builds, l.builds);
        assert!(verify(doc.as_bytes(), &public_key_hex(&key(2))).is_err(), "another key is refused");
        assert!(verify(doc.replace(&"1".repeat(64), &"9".repeat(64)).as_bytes(), &pin).is_err(), "a changed hash is refused");
        assert!(verify(doc.replace("\"dev\"", "\"zzz\"").as_bytes(), &pin).is_err());
        // a component list (another purpose) is not a build list
        let mut other = serde_json::from_str::<Value>(&doc).unwrap();
        other["sig"] = Value::String(sign_doc("components", &other, &key(1)));
        assert!(verify(other.to_string().as_bytes(), &pin).is_err());
    }

    #[test]
    fn names_and_paths() {
        assert!(valid_variant("pc-x86_64") && !valid_variant("PC") && !valid_variant("a/b") && !valid_variant(""));
        assert!(valid_build_id("alpha4b-0b1c") && !valid_build_id("../x"));
        assert!(valid_file_path("boot/ROOT.SFS") && !valid_file_path("../x") && !valid_file_path("/x") && !valid_file_path("a//b"));
    }
}
