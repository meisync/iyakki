//! `iyakki pkg ...`: what a publisher (IYAKKI, or anyone) runs to make keys, packages, certificates, catalogs,
//! revocation lists and licence tokens. Everything works offline and only reads and writes the files named on the line.

use std::collections::BTreeMap;
use std::path::Path;

use super::catalog::{sha256_hex, Catalog, Entry};
use super::license::Token;
use super::manifest::{Meta, Permission, Publisher};
use super::revocation::{sign_list, Revocations};
use super::trust::{Cert, Judge, Roots};
use crate::pkg::{self, public_key_hex, signing_key_from_hex, signing_key_hex};

pub const USAGE: &str = "iyakki pkg <command>
  keygen <name>                          make <name>.key (private: keep it safe) and <name>.pub
  build <file> --kind K --version V --license L [--name N] [--meta meta.json] [--manifest tool.json --module m.wasm] -o out.iyp
                                         K is skill, tool, mcp, agent, template, theme (a skill's name comes from the file)
  sign <pkg.iyp> --key name.key [--meta meta.json] [--cert cert.json] -o signed.iyp
  verify <pkg.iyp> [--roots roots.txt] [--trusted keys.txt] [--revocations list.json]
  catalog-add <catalog.json> <pkg.iyp> [--key name.key] [--url path-or-address] [--id ID --catalog-name N] [--featured] [--expires-days D]
  cert-issue --root root.key --publisher ID --name NAME --key PUBHEX [--url U] [--days 365] [--scopes skill,tool]
  revoke-sign --root root.key --serial N [--bad-key HEX]... [--publisher ID]... [--package name[@before-version]]... [--license SERIAL]... -o list.json
  license-issue --key pub.key --app NAME --device ID [--account A] [--days N] [--plan P] [--serial S]
  validate <project-dir|pkg.iyp>         the checks the install review runs, plus a secrets scan and the licence list
  pack <project-dir> [--key name.key] -o out.iyp
                                         build (and, with --key, sign) the package of an `iyakki-app.json` project
  inspect-module <file.ko> [--kernel RELEASE] [--reference stock.ko]
                                         read a kernel module's .modinfo: name, vermagic, licence, dependencies; with --kernel (and
                                         --reference, a stock module of that kernel) fail unless the device would accept it
  driver-check <payload.json> [--kernel RELEASE] [--reference stock.ko]
                                         the checks a device runs on a driver pack's payload (names, sizes, modprobe lines, modules)
  publish --server URL --token-env NAME <pkg.iyp>
                                         push a signed package to a private store (the token is read from the environment variable NAME)";

struct Args {
    pos: Vec<String>,
    flags: BTreeMap<String, Vec<String>>,
}

impl Args {
    fn parse(a: &[String]) -> Result<Args, String> {
        let (mut pos, mut flags) = (Vec::new(), BTreeMap::<String, Vec<String>>::new());
        let mut it = a.iter().peekable();
        while let Some(x) = it.next() {
            if let Some(f) = x.strip_prefix("--").or_else(|| (x == "-o").then_some("o")) {
                let boolean = matches!(f, "featured");
                if boolean {
                    flags.entry(f.into()).or_default().push("1".into());
                } else {
                    let v = it.next().ok_or_else(|| format!("--{f} needs a value"))?;
                    flags.entry(f.into()).or_default().push(v.clone());
                }
            } else {
                pos.push(x.clone());
            }
        }
        Ok(Args { pos, flags })
    }

    fn one(&self, k: &str) -> Option<&str> {
        self.flags.get(k).and_then(|v| v.last()).map(String::as_str)
    }

    fn need(&self, k: &str) -> Result<&str, String> {
        self.one(k).ok_or_else(|| format!("--{k} is required"))
    }

    fn many(&self, k: &str) -> Vec<String> {
        self.flags.get(k).cloned().unwrap_or_default()
    }
}

fn read(p: &str) -> Result<String, String> {
    std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))
}

fn write(p: &str, text: &str, private: bool) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(if private { 0o600 } else { 0o644 }).open(p).and_then(|mut f| f.write_all(text.as_bytes())).map_err(|e| format!("{p}: {e}"))
}

fn key_of(path: &str) -> Result<ed25519_dalek::SigningKey, String> {
    signing_key_from_hex(&read(path)?).map_err(|e| e.to_string())
}

fn meta_arg(a: &Args) -> Result<Option<String>, String> {
    match a.one("meta") {
        None => Ok(None),
        Some(p) => {
            let m = Meta::parse(&read(p)?)?;
            Ok(Some(m.to_json()))
        }
    }
}

fn days(a: &Args, k: &str, default: i64) -> Result<i64, String> {
    a.one(k).map_or(Ok(default), |d| d.parse().map_err(|_| format!("--{k} is a number of days")))
}

/// Run one `iyakki pkg` command with `now` as the time (tests pass a fixed one). Returns what to print.
pub fn run_at(args: &[String], now: i64) -> Result<String, String> {
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let a = Args::parse(args.get(1..).unwrap_or(&[]))?;
    match cmd {
        "help" | "--help" | "-h" => Ok(USAGE.to_string()),
        "keygen" => {
            let name = a.pos.first().ok_or("keygen needs a name")?;
            let k = pkg::new_signing_key().map_err(|e| e.to_string())?;
            write(&format!("{name}.key"), &format!("{}\n", signing_key_hex(&k)), true)?;
            write(&format!("{name}.pub"), &format!("{}\n", public_key_hex(&k)), false)?;
            Ok(format!("wrote {name}.key (private: keep it off shared and networked machines) and {name}.pub\npublic key: {}", public_key_hex(&k)))
        }
        "build" => {
            let kind = a.need("kind")?;
            let (version, license) = (a.need("version")?, a.need("license")?);
            let key = a.one("key").map(key_of).transpose()?;
            let meta = meta_arg(&a)?;
            let text = match kind {
                "tool" => {
                    let (m, w) = (read(a.need("manifest")?)?, std::fs::read(a.need("module")?).map_err(|e| e.to_string())?);
                    let t = pkg::build_tool(&m, &w, key.as_ref()).map_err(|e| e.to_string())?;
                    if meta.is_some() { pkg::resign(&t, meta.as_deref(), None, key.as_ref().ok_or("--key is needed to add store information to a tool package")?).map_err(|e| e.to_string())? } else { t }
                }
                "skill" => {
                    let md = read(a.pos.first().ok_or("build needs the skill file")?)?;
                    let t = pkg::build_skill(&md, version, license, key.as_ref()).map_err(|e| e.to_string())?;
                    if meta.is_some() { pkg::resign(&t, meta.as_deref(), None, key.as_ref().ok_or("--key is needed to add store information")?).map_err(|e| e.to_string())? } else { t }
                }
                other => {
                    let payload = read(a.pos.first().ok_or("build needs the payload file")?)?;
                    pkg::build_generic(other, a.need("name")?, version, license, &payload, meta.as_deref(), None, key.as_ref()).map_err(|e| e.to_string())?
                }
            };
            write(a.need("o")?, &text, false)?;
            Ok(format!("wrote {} ({} bytes, {})", a.need("o")?, text.len(), if key.is_some() { "signed" } else { "not signed: use `iyakki pkg sign`" }))
        }
        "sign" => {
            let src = read(a.pos.first().ok_or("sign needs a package file")?)?;
            let key = key_of(a.need("key")?)?;
            let meta = meta_arg(&a)?;
            let cert = match a.one("cert") {
                Some(p) => {
                    let c = Cert::parse(&read(p)?).map_err(|e| e.words())?;
                    if c.key != public_key_hex(&key) {
                        return Err("that certificate is for a different key".into());
                    }
                    Some(c.to_json())
                }
                None => None,
            };
            let out = pkg::resign(&src, meta.as_deref(), cert.as_deref(), &key).map_err(|e| e.to_string())?;
            write(a.need("o")?, &out, false)?;
            Ok(format!("signed with {}… and wrote {}", &public_key_hex(&key)[..12], a.need("o")?))
        }
        "verify" => {
            let text = read(a.pos.first().ok_or("verify needs a package file")?)?;
            let roots = match a.one("roots") {
                Some(p) => Roots::parse(&read(p)?),
                None => Roots::pinned(),
            };
            let own: Vec<(String, String)> = match a.one("trusted") {
                Some(p) => read(p)?.lines().filter_map(|l| l.trim().split_once(char::is_whitespace).map(|(k, n)| (k.to_ascii_lowercase(), n.trim().to_string())).or_else(|| (l.trim().len() == 64).then(|| (l.trim().to_ascii_lowercase(), "publisher".to_string())))).collect(),
                None => vec![],
            };
            let mut revs = Revocations::default();
            if let Some(p) = a.one("revocations") {
                revs.accept(&read(p)?, &roots, now).map_err(|e| e.to_string())?;
            }
            let p = match pkg::parse(&text) {
                Ok(p) => p,
                Err(e) => return Err(format!("NOT VALID: {e}")),
            };
            let m = &p.manifest;
            let v = Judge { roots: &roots, own_keys: &own, revocations: &revs, now }.verdict(m);
            let mut out = format!("{} {} {} (licence {})\nhash ok, {}\ntrust: {} ({})\n", m.kind, m.name, m.version, m.license, if m.signer.is_some() { "signature ok" } else { "not signed" }, v.level.label(), v.label);
            for c in &v.chain {
                out.push_str(&format!("  {c}\n"));
            }
            for w in &v.warnings {
                out.push_str(&format!("warning: {w}\n"));
            }
            if let Some(r) = &v.refusal {
                out.push_str(&format!("REFUSED: {r}\n"));
            }
            if let Some(j) = &m.meta {
                match Meta::parse(j) {
                    Ok(meta) => out.push_str(&format!("title: {} · publisher: {} · price: {}\n", meta.title, meta.publisher.name, meta.pricing.label())),
                    Err(e) => out.push_str(&format!("store information NOT VALID: {e}\n")),
                }
            }
            let perms: Vec<String> = match m.kind.as_str() {
                "tool" => crate::wtool::parse_payload(&p.payload).map(|t| t.caps.iter().map(|c| Permission::Cap(c.clone()).words("you")).collect()).unwrap_or_default(),
                "mcp" => serde_json::from_str::<serde_json::Value>(&p.payload).ok().and_then(|v| v["command"].as_str().map(|c| vec![format!("run the program {c}")])).unwrap_or_default(),
                _ => Meta::parse(m.meta.as_deref().unwrap_or("{}")).map(|x| x.permissions.iter().filter_map(|p| Permission::parse(p).ok()).map(|p| p.words("you")).collect()).unwrap_or_default(),
            };
            out.push_str(&format!("permissions: {}\n", if perms.is_empty() { "none".to_string() } else { perms.join("; ") }));
            Ok(out)
        }
        "catalog-add" => {
            let (cat_path, pkg_path) = (a.pos.first().ok_or("catalog-add needs a catalog file")?, a.pos.get(1).ok_or("catalog-add needs a package file")?);
            let bytes = std::fs::read(pkg_path).map_err(|e| format!("{pkg_path}: {e}"))?;
            let text = String::from_utf8(bytes.clone()).map_err(|_| "not a package".to_string())?;
            let p = pkg::parse(&text).map_err(|e| e.to_string())?;
            let m = &p.manifest;
            let meta = m.meta.as_deref().map(Meta::parse).transpose()?.unwrap_or_default();
            let mut cat = if Path::new(cat_path.as_str()).exists() {
                serde_json::from_str::<Catalog>(&read(cat_path)?).map_err(|e| format!("{cat_path}: {e}"))?
            } else {
                Catalog { v: 1, id: a.need("id")?.into(), name: a.one("catalog-name").unwrap_or("My catalog").into(), ..Default::default() }
            };
            let file = Path::new(pkg_path.as_str()).file_name().and_then(|f| f.to_str()).unwrap_or("package.iyp");
            let entry = Entry {
                id: m.name.clone(),
                kind: m.kind.clone(),
                name: m.name.clone(),
                version: m.version.clone(),
                license: m.license.clone(),
                size: bytes.len() as u64,
                sha256: sha256_hex(&bytes),
                url: a.one("url").map_or_else(|| format!("packages/{file}"), str::to_string),
                signer: m.signer.clone().unwrap_or_default(),
                cert: m.cert.as_deref().and_then(|c| Cert::parse(c).ok()),
                featured: a.one("featured").is_some(),
                added: now,
                meta,
                ..Default::default()
            };
            cat.entries.retain(|e| e.id != entry.id);
            cat.entries.push(entry);
            cat.entries.sort_by(|x, y| x.id.cmp(&y.id));
            cat.serial += 1;
            cat.issued = now;
            cat.expires = a.one("expires-days").map(|d| d.parse::<i64>().map(|d| now + d * 86_400)).transpose().map_err(|_| "--expires-days is a number")?.unwrap_or(0);
            let signed = match a.one("key") {
                Some(k) => {
                    cat.sign(&key_of(k)?);
                    true
                }
                None => {
                    cat.sig.clear();
                    false
                }
            };
            write(cat_path, &cat.to_json(), false)?;
            Ok(format!("{} now lists {} {} ({} entries, serial {}, {})", cat_path, m.name, m.version, cat.entries.len(), cat.serial, if signed { "signed" } else { "not signed: networks only accept signed catalogs" }))
        }
        "cert-issue" => {
            let root = key_of(a.need("root")?)?;
            let publisher = Publisher { id: a.need("publisher")?.into(), name: a.need("name")?.into(), url: a.one("url").unwrap_or("").into() };
            if !super::manifest::valid_publisher_id(&publisher.id) {
                return Err("the publisher id: 2 to 32 lowercase letters, digits and dashes".into());
            }
            let key = a.need("key")?.to_ascii_lowercase();
            if crate::pkg::unhex::<32>(&key).is_none() {
                return Err("--key is the publisher's public key (64 hex characters)".into());
            }
            let scopes_s = a.one("scopes").unwrap_or("skill,tool").to_string();
            let scopes: Vec<&str> = scopes_s.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
            let c = Cert::issue(&root, publisher, &key, now, now + days(&a, "days", 365)? * 86_400, &scopes);
            let out = a.one("o").map(str::to_string);
            let json = serde_json::to_string_pretty(&c).unwrap_or_default();
            if let Some(o) = &out {
                write(o, &json, false)?;
            }
            Ok(if out.is_some() { format!("certificate for {} written (valid until {})", c.publisher.name, super::trust::date(c.not_after)) } else { json })
        }
        "revoke-sign" => {
            let root = key_of(a.need("root")?)?;
            let serial: u64 = a.need("serial")?.parse().map_err(|_| "--serial is a number that goes up with every list")?;
            let packages = a.many("package").into_iter().map(|p| match p.split_once('@') { Some((n, v)) => (n.to_string(), Some(v.to_string())), None => (p, None) }).collect();
            let list = sign_list(&root, serial, now, now + days(&a, "days", 90)? * 86_400, a.many("bad-key"), a.many("publisher"), packages, a.many("license"));
            write(a.need("o")?, &list, false)?;
            Ok(format!("revocation list {serial} written to {}", a.need("o")?))
        }
        "license-issue" => {
            let key = key_of(a.need("key")?)?;
            let app = a.need("app")?;
            let expires = a.one("days").map(|d| d.parse::<i64>().map(|d| now + d * 86_400)).transpose().map_err(|_| "--days is a number")?.unwrap_or(0);
            let serial = a.one("serial").map_or_else(|| format!("L-{}", &sha256_hex(format!("{app}{now}{}", a.need("device").unwrap_or("")).as_bytes())[..10]), str::to_string);
            let t = Token::issue(&key, app, a.one("publisher").unwrap_or(""), a.need("device")?, a.one("account").unwrap_or(""), &serial, a.one("plan").unwrap_or("standard"), now, expires);
            let json = t.to_json();
            match a.one("o") {
                Some(o) => {
                    write(o, &json, false)?;
                    Ok(format!("licence {serial} written to {o}"))
                }
                None => Ok(json),
            }
        }
        "validate" => {
            let target = a.pos.first().ok_or("validate needs a project folder or a package file")?;
            let r = if Path::new(target.as_str()).is_dir() { super::vet::vet_dir(Path::new(target.as_str())) } else { super::vet::vet_package(&read(target)?, &[]) };
            if r.ok() { Ok(r.text()) } else { Err(r.text()) }
        }
        "pack" => {
            let dir = a.pos.first().ok_or("pack needs a project folder")?;
            let key = a.one("key").map(key_of).transpose()?;
            let (text, notes) = super::author::pack_dir(Path::new(dir.as_str()), key.as_ref())?;
            write(a.need("o")?, &text, false)?;
            Ok(format!("wrote {} ({} bytes, {}){}", a.need("o")?, text.len(), if key.is_some() { "signed" } else { "not signed" }, notes.iter().map(|n| format!("\nnote: {n}")).collect::<String>()))
        }
        "inspect-module" => {
            let file = a.pos.first().ok_or("inspect-module needs a .ko file")?;
            let bytes = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
            let reference = match a.one("reference") {
                Some(r) => Some(iyakki_drivers::modinfo::parse(&std::fs::read(r).map_err(|e| format!("{r}: {e}"))?).map_err(|e| format!("{r}: {e}"))?.vermagic),
                None => None,
            };
            let m = match a.one("kernel") {
                Some(k) => iyakki_drivers::modinfo::check(&bytes, k, reference.as_deref()).map_err(|e| format!("{file}: {e}"))?,
                None => iyakki_drivers::modinfo::parse(&bytes).map_err(|e| format!("{file}: {e}"))?,
            };
            Ok(format!("name: {}\nvermagic: {}\nrelease: {}\nlicense: {}{}\ndepends: {}\nsize: {} bytes\n{}", m.name, m.vermagic, m.release(), m.license, if m.gpl_compatible() { "" } else { " (NON-FREE: loading it taints the kernel)" }, m.depends.join(","), bytes.len(), a.one("kernel").map_or("kernel release not checked".to_string(), |k| format!("OK: built for kernel {k}{}", if reference.is_some() { " with the same build flags as the stock modules" } else { "" }))))
        }
        "driver-check" => {
            let file = a.pos.first().ok_or("driver-check needs a payload file")?;
            let p = iyakki_drivers::Pack::parse(&read(file)?)?;
            let reference = match a.one("reference") {
                Some(r) => Some(iyakki_drivers::modinfo::parse(&std::fs::read(r).map_err(|e| format!("{r}: {e}"))?).map_err(|e| format!("{r}: {e}"))?.vermagic),
                None => None,
            };
            let mut out = format!("pack ok: {} firmware file(s), {} module(s), {} modprobe line(s), loads {}\n", p.files.len(), p.modules.len(), p.modprobe.len(), if p.load.is_empty() { "nothing".to_string() } else { p.load.join(", ") });
            for m in &p.modules {
                let bytes = iyakki_drivers::b64::decode(&m.data).ok_or_else(|| format!("module {} is not valid base64", m.name))?;
                let info = match a.one("kernel") {
                    Some(k) if m.kernel != k => return Err(format!("module {} says kernel {} but you asked for {k}", m.name, m.kernel)),
                    Some(k) => iyakki_drivers::modinfo::check(&bytes, k, reference.as_deref()).map_err(|e| format!("module {}: {e}", m.name))?,
                    None => iyakki_drivers::modinfo::parse(&bytes).map_err(|e| format!("module {}: {e}", m.name))?,
                };
                out.push_str(&format!("module {}: vermagic \"{}\"{}\n", m.name, info.vermagic.trim(), if info.gpl_compatible() { "" } else { ", non-free" }));
            }
            Ok(out)
        }
        "publish" => {
            let file = a.pos.first().ok_or("publish needs a package file")?;
            let server = a.need("server")?;
            let var = a.need("token-env")?;
            let token = std::env::var(var).map_err(|_| format!("the environment variable {var} is not set (it holds your publish token; never put the token on the command line)"))?;
            let text = read(file)?;
            let r = super::vet::vet_package(&text, &[]);
            if !r.ok() {
                return Err(r.text());
            }
            let up = iyakki_proxy::https::UreqUpstream::default();
            let p = super::author::publish_to_server(&up, server, &token, &text)?;
            Ok(format!("published {} {} to {} (listed as {}, catalog number {}, sha256 {})", p.name, p.version, server, p.trust, p.serial, p.sha256))
        }
        other => Err(format!("unknown command {other:?}\n{USAGE}")),
    }
}

pub fn run(args: &[String]) -> Result<String, String> {
    run_at(args, crate::clock::now_unix())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_publisher_goes_from_key_to_signed_package_to_catalog_and_the_device_agrees() {
        let d = std::env::temp_dir().join(format!("iyakki-publish-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = |n: &str| d.join(n).display().to_string();
        // keys
        let root_out = run_at(&s(&["keygen", &p("root")]), NOW).unwrap();
        run_at(&s(&["keygen", &p("acme")]), NOW).unwrap();
        let acme_pub = std::fs::read_to_string(p("acme.pub")).unwrap().trim().to_string();
        let root_pub = std::fs::read_to_string(p("root.pub")).unwrap().trim().to_string();
        assert!(root_out.contains(&root_pub));
        // a certificate for the publisher
        run_at(&s(&["cert-issue", "--root", &p("root.key"), "--publisher", "acme", "--name", "Acme Ltd", "--key", &acme_pub, "--days", "30", "--scopes", "skill", "-o", &p("cert.json")]), NOW).unwrap();
        // a skill with store information, built and signed
        std::fs::write(p("notes.md"), "---\nname: meeting-notes\ndescription: Turn meeting notes into decisions\ntriggers: meeting\n---\nList decisions first.\n").unwrap();
        std::fs::write(p("meta.json"), r#"{"title":"Meeting notes","summary":"Notes to decisions","publisher":{"id":"acme","name":"Acme Ltd"},"categories":["work"]}"#).unwrap();
        run_at(&s(&["build", &p("notes.md"), "--kind", "skill", "--version", "1.0.0", "--license", "MIT", "-o", &p("notes.iyp")]), NOW).unwrap();
        run_at(&s(&["sign", &p("notes.iyp"), "--key", &p("acme.key"), "--meta", &p("meta.json"), "--cert", &p("cert.json"), "-o", &p("signed.iyp")]), NOW).unwrap();
        // the device-side check, with the root key pinned
        std::fs::write(p("roots.txt"), format!("r1 {root_pub}\n")).unwrap();
        let v = run_at(&s(&["verify", &p("signed.iyp"), "--roots", &p("roots.txt")]), NOW).unwrap();
        assert!(v.contains("trust: Verified") && v.contains("hash ok, signature ok") && v.contains("Acme Ltd") && v.contains("permissions: none"), "{v}");
        // without the root pinned it is only Community
        let v = run_at(&s(&["verify", &p("signed.iyp")]), NOW).unwrap();
        assert!(v.contains("trust: Community"), "{v}");
        // a certificate for another key is refused at signing
        run_at(&s(&["keygen", &p("other")]), NOW).unwrap();
        assert!(run_at(&s(&["sign", &p("notes.iyp"), "--key", &p("other.key"), "--cert", &p("cert.json"), "-o", &p("x.iyp")]), NOW).unwrap_err().contains("different key"));
        // a catalog: add the package, sign it, and the device reads it back
        run_at(&s(&["catalog-add", &p("catalog.json"), &p("signed.iyp"), "--key", &p("root.key"), "--id", "acme-store", "--catalog-name", "Acme", "--featured"]), NOW).unwrap();
        let out = run_at(&s(&["catalog-add", &p("catalog.json"), &p("signed.iyp"), "--key", &p("root.key")]), NOW + 5).unwrap();
        assert!(out.contains("1 entries, serial 2, signed"), "{out}");
        let roots = Roots::parse(&std::fs::read_to_string(p("roots.txt")).unwrap());
        let (c, check) = Catalog::parse(&std::fs::read_to_string(p("catalog.json")).unwrap(), &roots, &[], NOW + 10, true).unwrap();
        assert!(matches!(check.signer, super::super::catalog::CatalogSigner::Official(_)));
        let e = c.find("meeting-notes").unwrap();
        assert_eq!((e.sha256.clone(), e.url.as_str(), e.signer.clone()), (sha256_hex(&std::fs::read(p("signed.iyp")).unwrap()), "packages/signed.iyp", acme_pub.clone()));
        assert!(e.cert.is_some() && e.meta.title == "Meeting notes");
        // revocation and licence tokens
        run_at(&s(&["revoke-sign", "--root", &p("root.key"), "--serial", "1", "--bad-key", &acme_pub, "--package", "old@2.0.0", "-o", &p("rev.json")]), NOW).unwrap();
        let v = run_at(&s(&["verify", &p("signed.iyp"), "--roots", &p("roots.txt"), "--revocations", &p("rev.json")]), NOW).unwrap();
        assert!(v.contains("REFUSED") && v.contains("trust: Revoked"), "{v}");
        let lic = run_at(&s(&["license-issue", "--key", &p("acme.key"), "--app", "meeting-notes", "--device", "dev1", "--days", "10"]), NOW).unwrap();
        let t = Token::parse(&lic).unwrap();
        assert_eq!((t.app.as_str(), t.device.as_str(), t.expires), ("meeting-notes", "dev1", NOW + 10 * 86_400));
        // errors in plain words
        assert!(run_at(&s(&["build", &p("notes.md"), "--kind", "skill", "-o", "x"]), NOW).unwrap_err().contains("--version is required"));
        assert!(run_at(&s(&["frobnicate"]), NOW).unwrap_err().contains("unknown command"));
        assert!(run_at(&s(&["verify", &p("notes.md")]), NOW).unwrap_err().contains("NOT VALID"));
        assert!(run_at(&s(&["help"]), NOW).unwrap().contains("keygen"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn generic_and_tool_packages_build_from_files() {
        let d = std::env::temp_dir().join(format!("iyakki-publish2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = |n: &str| d.join(n).display().to_string();
        std::fs::write(p("theme.json"), r##"{"accent":"#ff8800"}"##).unwrap();
        run_at(&s(&["build", &p("theme.json"), "--kind", "theme", "--name", "sunset", "--version", "1.0.0", "--license", "MIT", "-o", &p("sunset.iyp")]), NOW).unwrap();
        assert!(run_at(&s(&["verify", &p("sunset.iyp")]), NOW).unwrap().contains("trust: Unsigned"));
        std::fs::write(p("tool.json"), crate::wtool::tests::manifest("shout", &["net:example.com"], serde_json::json!({}))).unwrap();
        std::fs::write(p("m.wat"), crate::wtool::tests::ECHO_WAT).unwrap();
        run_at(&s(&["build", "--kind", "tool", "--version", "1.0.0", "--license", "MIT", "--manifest", &p("tool.json"), "--module", &p("m.wat"), "-o", &p("shout.iyp")]), NOW).unwrap();
        let v = run_at(&s(&["verify", &p("shout.iyp")]), NOW).unwrap();
        assert!(v.contains("permissions: connect to example.com"), "{v}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_driver_pack_is_checked_built_signed_and_verified_the_way_the_wifi_script_does() {
        use iyakki_drivers::{b64, modinfo, pack, Pack};
        let d = std::env::temp_dir().join(format!("iyakki-publish-drv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = |n: &str| d.join(n).display().to_string();
        let ko = modinfo::synthetic_module_with("wl", "6.12.112 SMP preempt mod_unload ", "MIXED/Proprietary", "cfg80211");
        std::fs::write(p("wl.ko"), &ko).unwrap();
        std::fs::write(p("stock.ko"), modinfo::synthetic_module("stock", "6.12.112 SMP preempt mod_unload ", "GPL")).unwrap();
        std::fs::write(p("other.ko"), modinfo::synthetic_module("other", "6.12.100 SMP preempt mod_unload ", "GPL")).unwrap();
        let out = run_at(&s(&["inspect-module", &p("wl.ko"), "--kernel", "6.12.112", "--reference", &p("stock.ko")]), NOW).unwrap();
        assert!(out.contains("name: wl") && out.contains("NON-FREE") && out.contains("depends: cfg80211") && out.contains("OK: built for kernel 6.12.112"), "{out}");
        let e = run_at(&s(&["inspect-module", &p("wl.ko"), "--kernel", "6.12.113"]), NOW).unwrap_err();
        assert!(e.contains("6.12.112") && e.contains("6.12.113") && e.contains("exact kernel"), "{e}");
        let e = run_at(&s(&["inspect-module", &p("wl.ko"), "--kernel", "6.12.112", "--reference", &p("other.ko")]), NOW).unwrap_err();
        assert!(e.contains("build settings"), "{e}");
        assert!(run_at(&s(&["inspect-module", &p("nope.ko")]), NOW).is_err());
        let payload = Pack { v: 1, modules: vec![pack::KoFile { name: "wl".into(), kernel: "6.12.112".into(), data: b64::encode(&ko), sha256: pack::sha256_hex(&ko) }], modprobe: vec!["blacklist bcma".into()], load: vec!["wl".into()], ..Default::default() };
        std::fs::write(p("payload.json"), payload.to_json()).unwrap();
        assert!(run_at(&s(&["driver-check", &p("payload.json"), "--kernel", "6.12.112"]), NOW).unwrap().contains("1 module(s)"));
        assert!(run_at(&s(&["driver-check", &p("payload.json"), "--kernel", "6.12.99"]), NOW).is_err());
        let mut bad = payload.clone();
        bad.modprobe = vec!["install wl /bin/sh".into()];
        std::fs::write(p("bad.json"), bad.to_json()).unwrap();
        assert!(run_at(&s(&["driver-check", &p("bad.json")]), NOW).unwrap_err().contains("nothing that runs a command"));
        // sign with the owner's key and verify against a trusted-keys file
        run_at(&s(&["keygen", &p("owner")]), NOW).unwrap();
        let meta = r#"{"title":"Broadcom Wi-Fi driver","summary":"Locally built wl for BCM4360","description":"Non-free.","publisher":{"id":"owner","name":"Owner","url":""},"target":"system","ext_kind":"driver","permissions":["write:/lib/firmware","run:modprobe","run:insmod"]}"#;
        std::fs::write(p("meta.json"), meta).unwrap();
        run_at(&s(&["build", &p("payload.json"), "--kind", "extension", "--name", "mac-wifi-bcm4360", "--version", "1.0.0", "--license", "LicenseRef-Broadcom-STA", "--meta", &p("meta.json"), "--key", &p("owner.key"), "-o", &p("pack.iyp")]), NOW).unwrap();
        let pubkey = std::fs::read_to_string(p("owner.pub")).unwrap().trim().to_string();
        std::fs::write(p("trusted.txt"), format!("{pubkey} owner\n")).unwrap();
        let v = run_at(&s(&["verify", &p("pack.iyp"), "--trusted", &p("trusted.txt")]), NOW).unwrap();
        assert!(v.contains("signature ok") && v.contains("Trusted"), "{v}");
        let v = run_at(&s(&["verify", &p("pack.iyp")]), NOW).unwrap();
        assert!(!v.contains("Trusted"), "{v}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
