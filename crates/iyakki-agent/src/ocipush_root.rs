//! A sandbox's archived root as a cluster image (F-LINAPP-16; docs/BUILD-YOUR-OWN-IMAGE-APP.md route B). The root is a `tar` (made inside a
//! sandbox with `tar -C / -czf`, as the page describes); this module rewrites it into ONE image layer without unpacking anything on disk:
//!
//! * regular files, folders, links and hard links are kept; devices, fifos and sockets are dropped (a container gets its own `/dev`);
//! * set-user-id and set-group-id bits are cleared, owners become root:root and times become 0, so the same root is always the same image;
//! * a name with `..` in it is refused, a leading `./` or `/` is dropped, and a name too long for a ustar header is refused with its name;
//! * the result is a layer plus the config and manifest of [`crate::ocipush`], pushed or saved as an OCI layout like the one-file image.
//!
//! It never starts a process and needs no container tool. The image is only as good as the root: whether the program starts is the
//! cluster's to say (nothing here was run on a cluster; see KI-781).

use crate::ocipush::{sha256_hex, Image, MEDIA_CONFIG, MEDIA_LAYER, MEDIA_MANIFEST};

/// What the rewrite did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RootReport {
    pub files: usize,
    pub dirs: usize,
    pub links: usize,
    pub bytes: u64,
    /// Entries left out: devices, fifos and sockets (their names).
    pub dropped: Vec<String>,
    /// Files whose set-user-id or set-group-id bit was cleared.
    pub setid_cleared: usize,
}

fn field_str(b: &[u8]) -> String {
    let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

fn field_oct(b: &[u8]) -> Result<u64, String> {
    if b.first().is_some_and(|c| c & 0x80 != 0) {
        return Err("a size in the binary tar form is not supported".into());
    }
    let s = field_str(b);
    let s = s.trim();
    if s.is_empty() { Ok(0) } else { u64::from_str_radix(s, 8).map_err(|_| format!("`{s}` is not an octal number in a tar header")) }
}

/// The `path` and `linkpath` of a pax header.
fn pax(data: &[u8]) -> (Option<String>, Option<String>) {
    let (mut path, mut link) = (None, None);
    let mut rest = data;
    while let Some(sp) = rest.iter().position(|c| *c == b' ') {
        let Ok(len) = String::from_utf8_lossy(&rest[..sp]).parse::<usize>() else { break };
        if len == 0 || len > rest.len() {
            break;
        }
        let rec = String::from_utf8_lossy(&rest[sp + 1..len]).trim_end_matches('\n').to_string();
        if let Some(v) = rec.strip_prefix("path=") {
            path = Some(v.to_string());
        } else if let Some(v) = rec.strip_prefix("linkpath=") {
            link = Some(v.to_string());
        }
        rest = &rest[len..];
    }
    (path, link)
}

fn clean_name(raw: &str) -> Result<Option<String>, String> {
    let t = raw.trim_start_matches("./").trim_start_matches('/').trim_end_matches('/');
    if t.is_empty() || t == "." {
        return Ok(None);
    }
    if t.split('/').any(|p| p == ".." || p.is_empty()) || t.contains('\0') {
        return Err(format!("`{raw}` has a `..` or an empty part: refused"));
    }
    Ok(Some(t.to_string()))
}

fn header(name: &str, mode: u64, kind: u8, size: u64, link: &str) -> Result<[u8; 512], String> {
    let (n, prefix) = if name.len() <= 100 {
        (name.to_string(), String::new())
    } else {
        // split at a `/` so that the prefix is at most 155 and the rest at most 100
        let at = name.char_indices().filter(|(i, c)| *c == '/' && *i <= 155 && name.len() - i - 1 <= 100).map(|(i, _)| i).last().ok_or_else(|| format!("the path `{name}` is too long for an image layer"))?;
        (name[at + 1..].to_string(), name[..at].to_string())
    };
    if link.len() > 100 {
        return Err(format!("the link target of `{name}` is longer than 100 bytes: refused"));
    }
    let mut h = [0u8; 512];
    h[..n.len()].copy_from_slice(n.as_bytes());
    let oct = |f: &mut [u8], v: u64| {
        let s = format!("{:0width$o}\0", v, width = f.len() - 1);
        f.copy_from_slice(s.as_bytes());
    };
    oct(&mut h[100..108], mode);
    oct(&mut h[108..116], 0);
    oct(&mut h[116..124], 0);
    oct(&mut h[124..136], size);
    oct(&mut h[136..148], 0);
    h[148..156].copy_from_slice(b"        ");
    h[156] = kind;
    h[157..157 + link.len()].copy_from_slice(link.as_bytes());
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    h[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    let sum: u32 = h.iter().map(|b| *b as u32).sum();
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    Ok(h)
}

/// Rewrite an archived root (an uncompressed tar) into one image layer. `max_bytes` caps the size of the files that are kept.
pub fn layer_from_root_tar(tar: &[u8], max_bytes: u64) -> Result<(Vec<u8>, RootReport), String> {
    let mut out = Vec::with_capacity(tar.len());
    let mut rep = RootReport::default();
    let mut at = 0usize;
    let (mut long_name, mut long_link): (Option<String>, Option<String>) = (None, None);
    let mut seen = std::collections::BTreeSet::new();
    while at + 512 <= tar.len() {
        let h = &tar[at..at + 512];
        if h.iter().all(|b| *b == 0) {
            break;
        }
        let want: u32 = h.iter().enumerate().map(|(i, b)| if (148..156).contains(&i) { 32 } else { *b as u32 }).sum();
        let have = field_oct(&h[148..156]).map_err(|e| format!("tar header at byte {at}: {e}"))?;
        if u64::from(want) != have {
            return Err(format!("the tar is damaged: the header at byte {at} fails its checksum"));
        }
        let size = field_oct(&h[124..136])? as usize;
        let data_at = at + 512;
        let data_end = data_at.checked_add(size).filter(|e| *e <= tar.len()).ok_or("the tar is cut off inside a file")?;
        let data = &tar[data_at..data_end];
        at = data_at + size.div_ceil(512) * 512;
        let kind = h[156];
        match kind {
            b'L' => {
                long_name = Some(field_str(data));
                continue;
            }
            b'K' => {
                long_link = Some(field_str(data));
                continue;
            }
            b'x' => {
                let (p, l) = pax(data);
                long_name = p.or(long_name);
                long_link = l.or(long_link);
                continue;
            }
            b'g' => continue,
            _ => {}
        }
        let mut name = field_str(&h[0..100]);
        if &h[257..262] == b"ustar" {
            let prefix = field_str(&h[345..500]);
            if !prefix.is_empty() {
                name = format!("{prefix}/{name}");
            }
        }
        let name = long_name.take().unwrap_or(name);
        let link = long_link.take().unwrap_or_else(|| field_str(&h[157..257]));
        let Some(name) = clean_name(&name)? else { continue };
        let mode = field_oct(&h[100..108])?;
        let kept_mode = mode & 0o1777;
        if mode & 0o6000 != 0 && matches!(kind, b'0' | 0) {
            rep.setid_cleared += 1;
        }
        if !seen.insert(name.clone()) {
            // the last copy of a name wins in a tar extraction; keep the first and say nothing: the root came from one archive of one tree
            continue;
        }
        match kind {
            b'0' | 0 => {
                if rep.bytes + size as u64 > max_bytes {
                    return Err(format!("the root is bigger than {max_bytes} bytes: refused (nothing was built)"));
                }
                rep.bytes += size as u64;
                rep.files += 1;
                out.extend_from_slice(&header(&name, kept_mode, b'0', size as u64, "")?);
                out.extend_from_slice(data);
                out.resize(out.len() + (512 - size % 512) % 512, 0);
            }
            b'5' => {
                rep.dirs += 1;
                out.extend_from_slice(&header(&format!("{name}/"), kept_mode, b'5', 0, "")?);
            }
            b'2' => {
                rep.links += 1;
                out.extend_from_slice(&header(&name, 0o777, b'2', 0, &link)?);
            }
            b'1' => {
                let Some(target) = clean_name(&link)? else { return Err(format!("the hard link `{name}` points nowhere")) };
                rep.links += 1;
                out.extend_from_slice(&header(&name, kept_mode, b'1', 0, &target)?);
            }
            _ => rep.dropped.push(name),
        }
    }
    out.extend_from_slice(&[0u8; 1024]);
    if rep.files + rep.dirs + rep.links == 0 {
        return Err("the archive holds no files: it is not the root of a sandbox".into());
    }
    Ok((out, rep))
}

/// How the image starts.
#[derive(Clone, Debug)]
pub struct RootImageSpec<'a> {
    /// `amd64` or `arm64`.
    pub arch: &'a str,
    /// `10001:10001`; never root unless the person says so by giving `0:0`.
    pub user: &'a str,
    pub entrypoint: Vec<String>,
    pub cmd: Vec<String>,
    pub env: Vec<(String, String)>,
    pub workdir: &'a str,
    pub port: Option<u16>,
}

/// The image (layer, config, manifest) of an archived root.
pub fn build_image_from_root(tar: &[u8], spec: &RootImageSpec, max_bytes: u64) -> Result<(Image, RootReport), String> {
    if !matches!(spec.arch, "amd64" | "arm64") {
        return Err(format!("`{}` is not a CPU architecture an image can be built for (amd64 or arm64)", spec.arch));
    }
    if spec.entrypoint.is_empty() && spec.cmd.is_empty() {
        return Err("say what the image runs: an entry point or a command".into());
    }
    if spec.user.is_empty() || !spec.user.chars().all(|c| c.is_ascii_digit() || c == ':') {
        return Err("the user is numbers like 10001:10001".into());
    }
    let (layer, rep) = layer_from_root_tar(tar, max_bytes)?;
    let layer_digest = format!("sha256:{}", sha256_hex(&layer));
    let env: Vec<String> = spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let mut cfg = serde_json::json!({ "User": spec.user, "Entrypoint": spec.entrypoint, "Cmd": spec.cmd, "Env": env });
    if !spec.workdir.is_empty() {
        cfg["WorkingDir"] = serde_json::json!(spec.workdir);
    }
    if let Some(p) = spec.port {
        cfg["ExposedPorts"] = serde_json::json!({ format!("{p}/tcp"): {} });
    }
    let config = serde_json::json!({ "architecture": spec.arch, "os": "linux", "config": cfg, "rootfs": { "type": "layers", "diff_ids": [layer_digest.clone()] }, "history": [{ "created_by": "iyakki: image of a sandbox root" }] });
    let config = serde_json::to_vec(&config).map_err(|e| e.to_string())?;
    let config_digest = format!("sha256:{}", sha256_hex(&config));
    let manifest = serde_json::json!({ "schemaVersion": 2, "mediaType": MEDIA_MANIFEST, "config": { "mediaType": MEDIA_CONFIG, "digest": config_digest, "size": config.len() }, "layers": [{ "mediaType": MEDIA_LAYER, "digest": layer_digest, "size": layer.len() }] });
    let manifest = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
    Ok((Image { layer, layer_digest, config, config_digest, manifest }, rep))
}

#[cfg(test)]
#[path = "ocipush_root_tests.rs"]
mod tests;
