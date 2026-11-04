//! `iyakki cloud ...`: make a tenant's image and manifests and put them on a cluster (docs/development/CLOUD-MULTITENANT-PLAN.md).
//!
//! * `example TENANT`: print a configuration file with every default.
//! * `manifests --config FILE [-o OUT]`: the Kubernetes objects of the tenant, as text.
//! * `image [--out FILE] [--name REF] [--binary PATH] [--push HOST:PORT/REPO:TAG]`: a container image made by IYAKKI itself from its own
//!   program (a one-file image, see `ocipush.rs`), saved as an OCI layout tar that `docker load`, `podman load` and `k3s ctr images
//!   import` read, or sent to a plain-HTTP registry. No builder is needed.
//! * `snapshot create|list|verify|restore|prune|cronjob`: backups of a tenant's volume (`cloudsnapshot.rs`).
//! * `bundle --config FILE --out DIR`: configuration, manifests, image and a README in one folder, to take anywhere.
//! * `common <init|set|names|delete|secret> --dir DIR`: the operator's common vault (`cvault:NAME` in every tenant, read-only there).
//! * `deploy --config FILE [--kubeconfig FILE] [--context NAME] [--apply]`: show what would be applied; with `--apply` run
//!   `kubectl apply` (kubectl is the person's own; nothing is downloaded).

use std::path::{Path, PathBuf};

use crate::cloudtenant::{manifests, reach_text, secret_command, TenantConfig};

pub fn run(a: &[String], out: &mut dyn FnMut(&str)) -> Result<(), String> {
    match a.first().map(String::as_str) {
        Some("example") => {
            let t = a.get(1).ok_or("usage: iyakki cloud example TENANT")?;
            let c = TenantConfig::example(t);
            c.check()?;
            out(&serde_json::to_string_pretty(&c).map_err(|e| e.to_string())?);
            Ok(())
        }
        Some("manifests") => {
            let c = load(&a[1..])?;
            let y = manifests(&c)?;
            match opt(&a[1..], "-o").or_else(|| opt(&a[1..], "--out")) {
                Some(p) => {
                    write(Path::new(&p), y.as_bytes())?;
                    out(&format!("wrote {p}"));
                }
                None => out(&y),
            }
            Ok(())
        }
        Some("image") => {
            let img = make_image(&a[1..])?;
            if let Some(dest) = opt(&a[1..], "--push") {
                let (reg, repo, tag) = split_ref(&dest)?;
                let digest = crate::ocipush::push(&crate::ocipush::TcpHttp::default(), &reg, &repo, &tag, &img.image, &mut |l| out(l))?;
                out(&format!("pushed {dest} ({digest})"));
                return Ok(());
            }
            let path = opt(&a[1..], "--out").unwrap_or_else(|| "iyakki-image.tar".into());
            write(Path::new(&path), &img.tar)?;
            out(&format!("wrote {path} ({} MB, {} image for {}): load it with  docker load -i {path}  or  podman load -i {path}  or  k3s ctr images import {path}", img.tar.len() / 1_000_000 + 1, img.reference, img.arch));
            for w in &img.warnings {
                out(&format!("warning: {w}"));
            }
            Ok(())
        }
        Some("bundle") => {
            let c = load(&a[1..])?;
            let dir = PathBuf::from(opt(&a[1..], "--out").ok_or("usage: iyakki cloud bundle --config FILE --out DIR")?);
            let mut ia = a[1..].to_vec();
            if opt(&ia, "--name").is_none() {
                ia.extend(["--name".into(), c.image.clone()]);
            }
            let img = make_image(&ia)?;
            write(&dir.join("tenant.json"), serde_json::to_string_pretty(&c).map_err(|e| e.to_string())?.as_bytes())?;
            write(&dir.join("manifests.yaml"), manifests(&c)?.as_bytes())?;
            write(&dir.join("iyakki-image.tar"), &img.tar)?;
            write(&dir.join("README.txt"), readme(&c).as_bytes())?;
            out(&format!("wrote {} (tenant.json, manifests.yaml, iyakki-image.tar, README.txt)", dir.display()));
            for w in &img.warnings {
                out(&format!("warning: {w}"));
            }
            Ok(())
        }
        Some("common") => common(&a[1..], out),
        Some("routes") => {
            let f = opt(&a[1..], "--file").ok_or("usage: iyakki cloud routes --file ROUTES.json [--namespace NS]")?;
            let t = crate::cloudroute::RouteTable::parse(&std::fs::read_to_string(&f).map_err(|e| format!("{f}: {e}"))?)?;
            out(&crate::cloudroute::ingress(&opt(&a[1..], "--namespace").unwrap_or_else(|| "default".into()), &t)?);
            Ok(())
        }
        Some("cert") => cert(&a[1..], out),
        Some("listen") => listen(&a[1..], out),
        Some("roles") => roles(&a[1..], out),
        Some("snapshot") => crate::cloudsnapshot::cli(&a[1..], out),
        Some("reconcile") => reconcile(&a[1..], out),
        Some("deploy") => {
            let c = load(&a[1..])?;
            let y = manifests(&c)?;
            if !a.iter().any(|x| x == "--apply") {
                out(&y);
                out("---");
                out("Nothing was applied. Make the accounts' Secret, then run again with --apply:");
                out(&format!("  {}", secret_command(&c)));
                return Ok(());
            }
            let mut args: Vec<String> = Vec::new();
            for (flag, key) in [("--kubeconfig", "--kubeconfig"), ("--context", "--context")] {
                if let Some(v) = opt(&a[1..], flag) {
                    args.push(format!("{key}={v}"));
                }
            }
            args.extend(["apply".into(), "-f".into(), "-".into()]);
            let o = kubectl(&args, y.as_bytes())?;
            out(o.trim_end());
            out(&reach_text(&c));
            Ok(())
        }
        _ => Err("usage: iyakki cloud <example TENANT | manifests | image | bundle | common | deploy | reconcile | routes | cert | listen | roles | snapshot> (docs/development/CLOUD-MULTITENANT-PLAN.md)\n  listen --routes FILE [--cert-dir DIR] [--bind ADDR] [--redirect-port PORT]\n  \
                  manifests --config FILE [-o OUT]\n  image [--out FILE] [--name REF] [--binary PATH] [--arch amd64|arm64] [--push HOST:PORT/REPO:TAG]\n  \
                  bundle --config FILE --out DIR\n  deploy --config FILE [--kubeconfig FILE] [--context NAME] [--apply]\n  \
                  reconcile --dir DIR [--kubeconfig FILE] [--context NAME] [--apply] [--prune]"
            .into()),
    }
}

/// `iyakki cloud reconcile --dir DIR [--kubeconfig F] [--context C] [--apply] [--prune]`: make the cluster match the tenant files of DIR
/// (`cloudcontrol.rs`). Shows the plan and does nothing without `--apply`; namespaces without a file are removed only with `--prune`.
fn reconcile(a: &[String], out: &mut dyn FnMut(&str)) -> Result<(), String> {
    use crate::cloudcontrol as cc;
    // vault keys of people removed from roles are rotated when the vault could not be reached or the key was missing at the time
    if a.iter().any(|x| x == "--retry-rotations") {
        return crate::cloudrotate::retry_command(a, out);
    }
    let dir = opt(a, "--dir").ok_or("usage: iyakki cloud reconcile --dir DIR [--kubeconfig FILE] [--context NAME] [--apply] [--prune]  |  --retry-rotations --tenant-data DIR --as USER --vault-key FILE [--vault URL]")?;
    let (apply, prune) = (a.iter().any(|x| x == "--apply"), a.iter().any(|x| x == "--prune"));
    let files = cc::read_dir(Path::new(&dir))?;
    let mut k = cc::run_kubectl(opt(a, "--kubeconfig").as_deref(), opt(a, "--context").as_deref());
    let existing = cc::list_namespaces(&mut k)?;
    let p = cc::plan(&files, &existing, prune);
    out(p.render().trim_end());
    if !apply {
        out("Nothing was changed. Run again with --apply to do this.");
        return Ok(());
    }
    let work = std::env::temp_dir().join(format!("iyakki-reconcile-{}", std::process::id()));
    let o = cc::execute(&p, &work, &mut k);
    let _ = std::fs::remove_dir_all(&work);
    for l in &o.said {
        out(l);
    }
    match o.failed {
        Some(f) => Err(format!("stopped: {f}")),
        None => Ok(()),
    }
}

/// `iyakki cloud listen --routes FILE [--cert-dir DIR] [--bind ADDR] [--redirect-port PORT]`: the local domain listener
/// (`cloudlisten.rs`). Runs until interrupted; follows changes of the routes file and re-reads the certificates once a minute. The
/// firewall is not driven from here (no gate); `members` routes are not enforced (see the module documentation of `cloudlisten.rs`).
fn listen(a: &[String], out: &mut dyn FnMut(&str)) -> Result<(), String> {
    use crate::cloudlisten::{FileCerts, Listener, NoGate, Options};
    let usage = "usage: iyakki cloud listen --routes FILE [--cert-dir DIR] [--bind ADDR] [--redirect-port PORT]";
    let f = opt(a, "--routes").ok_or(usage)?;
    let read = || -> Result<crate::cloudroute::RouteTable, String> { crate::cloudroute::RouteTable::parse(&std::fs::read_to_string(&f).map_err(|e| format!("{f}: {e}"))?) };
    let table = read()?;
    let mut o = Options::default();
    if let Some(b) = opt(a, "--bind") {
        o.bind = b.parse().map_err(|_| format!("--bind \"{b}\" is not an address"))?;
    }
    if let Some(p) = opt(a, "--redirect-port") {
        o.redirect_port = p.parse().map_err(|_| format!("--redirect-port \"{p}\" is not a port"))?;
    }
    let dir = opt(a, "--cert-dir").unwrap_or_else(|| ".".into());
    let l = Listener::start(table, std::sync::Arc::new(FileCerts::new(dir)), std::sync::Arc::new(NoGate), o)?;
    out(&format!("listening on port(s) {:?}; Ctrl-C stops it. Routes marked access members are NOT enforced by this listener.", l.ports()));
    for (h, why) in l.refused() {
        out(&format!("not served over https: {h}: {why}"));
    }
    let stamp = || std::fs::metadata(&f).and_then(|m| m.modified()).ok();
    let mut seen = stamp();
    let mut ticks = 0u32;
    loop {
        std::thread::sleep(std::time::Duration::from_secs(2));
        ticks += 1;
        if stamp() != seen {
            seen = stamp();
            match read().and_then(|t| l.update(t)) {
                Ok(()) => out(&format!("routes reloaded; ports {:?}", l.ports())),
                Err(e) => out(&format!("routes file not applied: {e}")),
            }
        } else if ticks % 30 == 0 {
            for line in l.reload_certs() {
                out(&line);
            }
        }
    }
}

/// `iyakki cloud cert check --cert FILE --key FILE --host HOST` and `cert self-signed HOST --out DIR`: the certificates of HTTPS routes
/// (`cloudroute.rs`). The key is only read to check that it belongs to the certificate; nothing is sent anywhere.
fn cert(a: &[String], out: &mut dyn FnMut(&str)) -> Result<(), String> {
    match a.first().map(String::as_str) {
        Some("check") => {
            let get = |k: &str| -> Result<String, String> {
                let p = opt(a, k).ok_or("usage: iyakki cloud cert check --cert FILE --key FILE --host HOST")?;
                std::fs::read_to_string(&p).map_err(|e| format!("{p}: {e}"))
            };
            let host = opt(a, "--host").ok_or("--host is needed")?;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
            let (info, line) = crate::cloudroute::check_certificate(&get("--cert")?, &get("--key")?, &host, now)?;
            out(&format!("{host}: {line} (names: {})", info.names.join(", ")));
            Ok(())
        }
        Some("self-signed") => {
            let host = a.get(1).filter(|h| !h.starts_with("--")).ok_or("usage: iyakki cloud cert self-signed HOST --out DIR")?;
            let dir = PathBuf::from(opt(a, "--out").ok_or("--out DIR is needed")?);
            let (chain, key) = crate::cloudroute::self_signed(host)?;
            write(&dir.join(format!("{host}.crt")), chain.as_bytes())?;
            let kp = dir.join(format!("{host}.key"));
            write(&kp, key.as_bytes())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&kp, std::fs::Permissions::from_mode(0o600));
            }
            out(&format!("wrote {host}.crt and {host}.key in {} (self-signed: browsers will not trust it; for a local trial only)", dir.display()));
            Ok(())
        }
        _ => Err("usage: iyakki cloud cert <check --cert FILE --key FILE --host HOST | self-signed HOST --out DIR>".into()),
    }
}

/// After a removal from the lists in `file`: rotate the vault scopes the person held (`cloudrotate.rs`) with the acting person's secret
/// (`--vault-key FILE`), to the vault of `--vault URL` (or `IYAKKI_VAULT_URL`; `--vault-fingerprint`, `--tenant`). With no vault given there
/// is nothing to rotate; with one and no key, or a vault that does not answer, the rotation is written to `DIR/cloud/rotation-pending.json`
/// (`DIR` is `--tenant-data`, or the folder two levels above the lists file) and `iyakki cloud reconcile --retry-rotations` finishes it.
fn rotate_after(a: &[String], file: &Path, by: &str, who: &str, scopes: &[crate::cloudvaultd::Scope], out: &mut dyn FnMut(&str)) {
    use crate::cloudvaultlink::{Link, ENV_FINGERPRINT, ENV_TENANT, ENV_URL};
    if scopes.is_empty() {
        return;
    }
    let env = |k: &str| std::env::var(k).ok();
    let link = Link::from_values(
        opt(a, "--vault").or_else(|| env(ENV_URL)).as_deref(),
        opt(a, "--vault-fingerprint").or_else(|| env(ENV_FINGERPRINT)).as_deref(),
        opt(a, "--tenant").or_else(|| env(ENV_TENANT)).as_deref(),
    );
    let dir = opt(a, "--tenant-data").map(PathBuf::from).or_else(|| file.parent().and_then(Path::parent).map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from("."));
    let keys = opt(a, "--vault-key").and_then(|f| std::fs::read_to_string(f).ok()).and_then(|t| crate::cloudkeys::MemberKeys::from_hex(&t).ok());
    let rep = crate::cloudrotate::rotate(&dir, &link, by, keys.as_ref(), who, scopes);
    if !rep.rotated.is_empty() {
        out(&format!("vault keys rotated: {}", rep.rotated.join(", ")));
    }
    if !rep.pending.is_empty() {
        out(&format!("vault rotation PENDING for {}: {} (run: iyakki cloud reconcile --retry-rotations --tenant-data {} --as USER --vault-key FILE)", who, rep.pending.join(", "), dir.display()));
    }
}

/// `iyakki cloud roles ...`: the tenant's membership lists (`cloudroles.rs`), kept in a file. Every change says who makes it (`--as`) and
/// prints the audit line; deny by default.
fn roles(a: &[String], out: &mut dyn FnMut(&str)) -> Result<(), String> {
    use crate::cloudroles::{Action, NsRole, Roles, TenantRole};
    let usage = "usage: iyakki cloud roles <init OWNER | show [--as USER] | can --as USER (enter|manage-members|manage-namespaces|see|read|write|manage-ns-members) [NAMESPACE] | member <add|remove> NAME [--role owner|admin|member] | namespace <add|remove> NAME | ns-member <add|remove> NAMESPACE NAME [--role admin|editor|viewer]> --file FILE [--as USER]";
    let file = PathBuf::from(opt(a, "--file").ok_or(usage)?);
    let pos: Vec<&String> = {
        let mut v = Vec::new();
        let mut skip = false;
        for x in a {
            if skip {
                skip = false;
            } else if x.starts_with("--") {
                skip = true;
            } else {
                v.push(x);
            }
        }
        v
    };
    let load = || -> Result<Roles, String> { Roles::parse(&std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?) };
    let by = || opt(a, "--as").ok_or_else(|| "--as USER says who is acting".to_string());
    let tenant_role = |s: Option<String>| match s.as_deref() {
        None | Some("member") => Ok(TenantRole::Member),
        Some("admin") => Ok(TenantRole::Admin),
        Some("owner") => Ok(TenantRole::Owner),
        Some(x) => Err(format!("role \"{x}\": owner, admin or member")),
    };
    let ns_role = |s: Option<String>| match s.as_deref() {
        None | Some("viewer") => Ok(NsRole::Viewer),
        Some("editor") => Ok(NsRole::Editor),
        Some("admin") => Ok(NsRole::Admin),
        Some(x) => Err(format!("role \"{x}\": admin, editor or viewer")),
    };
    let arg = |i: usize, what: &str| pos.get(i).map(|s| s.as_str()).ok_or_else(|| format!("{what} is needed\n{usage}"));
    let save = |r: &Roles| write(&file, r.to_json().as_bytes());
    match arg(0, "a command")? {
        "init" => {
            let r = Roles::new(arg(1, "the owner's name")?)?;
            save(&r)?;
            out(&format!("made {} with owner {}", file.display(), arg(1, "")?));
            Ok(())
        }
        "show" => {
            let r = load()?;
            match opt(a, "--as") {
                Some(u) => {
                    out(&format!("{u}: {}", r.tenant_role(&u).map(|t| format!("{t:?}")).unwrap_or_else(|| "not in the tenant".into())));
                    for (n, m) in r.visible_namespaces(&u) {
                        out(&format!("  namespace {n}: {}", m.map(|x| format!("{x:?}")).unwrap_or_else(|| "visible to you, not joined".into())));
                    }
                }
                None => out(&r.to_json()),
            }
            Ok(())
        }
        "can" => {
            let r = load()?;
            let u = by()?;
            let ns = || arg(2, "the namespace").map(str::to_string);
            let what = match arg(1, "an action")? {
                "enter" => Action::EnterTenant,
                "manage-members" => Action::ManageTenantMembers,
                "manage-namespaces" => Action::ManageNamespaces,
                "see" => Action::SeeNamespace(ns()?),
                "read" => Action::ReadNamespace(ns()?),
                "write" => Action::WriteNamespace(ns()?),
                "manage-ns-members" => Action::ManageNamespaceMembers(ns()?),
                x => return Err(format!("unknown action \"{x}\"\n{usage}")),
            };
            out(&format!("{u}: {what:?} -> {:?}", r.can(&u, &what)));
            Ok(())
        }
        "member" => {
            let (mut r, u) = (load()?, by()?);
            let name = arg(2, "the person's name")?;
            let mut scopes = Vec::new();
            let c = match arg(1, "add or remove")? {
                "add" => r.add_tenant_member(&u, name, tenant_role(opt(a, "--role"))?)?,
                "remove" => {
                    scopes = crate::cloudrotate::scopes_for_tenant_removal(&r, name);
                    r.remove_tenant_member(&u, name)?
                }
                x => return Err(format!("\"{x}\": add or remove")),
            };
            save(&r)?;
            out(&c.0);
            rotate_after(a, &file, &u, name, &scopes, out);
            Ok(())
        }
        "namespace" => {
            let (mut r, u) = (load()?, by()?);
            let name = arg(2, "the namespace's name")?;
            let c = match arg(1, "add or remove")? {
                "add" => r.create_namespace(&u, name)?,
                "remove" => r.delete_namespace(&u, name)?,
                x => return Err(format!("\"{x}\": add or remove")),
            };
            save(&r)?;
            out(&c.0);
            Ok(())
        }
        "ns-member" => {
            let (mut r, u) = (load()?, by()?);
            let (ns, name) = (arg(2, "the namespace")?, arg(3, "the person's name")?);
            let mut scopes = Vec::new();
            let c = match arg(1, "add or remove")? {
                "add" => r.add_namespace_member(&u, ns, name, ns_role(opt(a, "--role"))?)?,
                "remove" => {
                    scopes = vec![crate::cloudvaultd::Scope::Namespace(ns.to_string())];
                    r.remove_namespace_member(&u, ns, name)?
                }
                x => return Err(format!("\"{x}\": add or remove")),
            };
            save(&r)?;
            out(&c.0);
            rotate_after(a, &file, &u, name, &scopes, out);
            Ok(())
        }
        _ => Err(usage.into()),
    }
}

/// `iyakki cloud common <init|set NAME|names|delete NAME|secret> --dir DIR [--namespace NS]`: the operator's common vault (`commonvault.rs`).
/// A value is read from standard input, never from the command line.
fn common(a: &[String], out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let dir = PathBuf::from(opt(a, "--dir").ok_or("usage: iyakki cloud common <init|set NAME|names|delete NAME|secret> --dir DIR")?);
    let name = || a.get(1).filter(|n| !n.starts_with("--")).cloned().ok_or_else(|| "a secret name is needed".to_string());
    match a.first().map(String::as_str) {
        Some("init") => {
            crate::commonvault::init(&dir)?;
            out(&format!("made {} (vault.json and key; keep the folder safe, the key opens the vault)", dir.display()));
            out(&format!("put it where the tenants can mount it:  {}", crate::commonvault::secret_command(&dir, &opt(a, "--namespace").unwrap_or_else(|| "iyakki-platform".into()))));
            Ok(())
        }
        Some("set") => {
            let mut v = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin().lock(), &mut v).map_err(|e| e.to_string())?;
            crate::commonvault::set(&dir, &name()?, v.trim_end_matches(['\n', '\r']))?;
            out("saved");
            Ok(())
        }
        Some("names") => {
            for n in crate::commonvault::names(&dir)? {
                out(&n);
            }
            Ok(())
        }
        Some("delete") => {
            crate::commonvault::delete(&dir, &name()?)?;
            out("deleted");
            Ok(())
        }
        Some("secret") => {
            out(&crate::commonvault::secret_command(&dir, &opt(a, "--namespace").unwrap_or_else(|| "iyakki-platform".into())));
            Ok(())
        }
        _ => Err("usage: iyakki cloud common <init|set NAME|names|delete NAME|secret> --dir DIR [--namespace NS]".into()),
    }
}

fn opt(a: &[String], name: &str) -> Option<String> {
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned()
}

fn load(a: &[String]) -> Result<TenantConfig, String> {
    let p = opt(a, "--config").ok_or("--config FILE is needed (iyakki cloud example TENANT prints one)")?;
    let t = std::fs::read_to_string(&p).map_err(|e| format!("{p}: {e}"))?;
    TenantConfig::parse(&t)
}

fn write(p: &Path, b: &[u8]) -> Result<(), String> {
    if let Some(d) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::write(p, b).map_err(|e| format!("{}: {e}", p.display()))
}

fn kubectl(args: &[String], stdin: &[u8]) -> Result<String, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("kubectl").args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("kubectl could not be started ({e}): install kubectl or use `iyakki cloud bundle` and apply the manifests elsewhere"))?;
    child.stdin.take().ok_or("kubectl: no stdin")?.write_all(stdin).map_err(|e| e.to_string())?;
    let o = child.wait_with_output().map_err(|e| e.to_string())?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    } else {
        Err(format!("kubectl apply failed: {}", String::from_utf8_lossy(&o.stderr).trim()))
    }
}

pub struct Made {
    pub image: crate::ocipush::Image,
    pub tar: Vec<u8>,
    pub reference: String,
    pub arch: String,
    pub warnings: Vec<String>,
}

/// `(architecture, statically linked)` of an ELF program, `None` when it is not one.
pub fn elf_info(b: &[u8]) -> Option<(&'static str, bool)> {
    if b.len() < 64 || &b[..4] != b"\x7fELF" || b[4] != 2 || b[5] != 1 {
        return None;
    }
    let arch = match u16::from_le_bytes([b[18], b[19]]) {
        62 => "amd64",
        183 => "arm64",
        _ => return None,
    };
    let phoff = u64::from_le_bytes(b[32..40].try_into().ok()?) as usize;
    let (phentsize, phnum) = (u16::from_le_bytes([b[54], b[55]]) as usize, u16::from_le_bytes([b[56], b[57]]) as usize);
    let mut has_interp = false;
    for i in 0..phnum.min(64) {
        let o = phoff + i * phentsize;
        if o + 4 <= b.len() && u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) == 3 {
            has_interp = true;
        }
    }
    Some((arch, !has_interp))
}

fn make_image(a: &[String]) -> Result<Made, String> {
    let bin = match opt(a, "--binary") {
        Some(p) => PathBuf::from(p),
        None => std::env::current_exe().map_err(|e| format!("this program's own file: {e}"))?,
    };
    let bytes = std::fs::read(&bin).map_err(|e| format!("{}: {e}", bin.display()))?;
    let mut warnings = Vec::new();
    let (found, is_static) = elf_info(&bytes).map(|(a, s)| (Some(a), s)).unwrap_or((None, false));
    let arch = opt(a, "--arch").or(found.map(String::from)).ok_or_else(|| format!("{} is not a Linux program for amd64 or arm64: name --binary and --arch", bin.display()))?;
    if !is_static {
        warnings.push(format!("{} is not a static program: the image holds nothing else, so it will not start. Use the musl build (the one in the release image).", bin.display()));
    }
    let reference = opt(a, "--name").unwrap_or_else(|| format!("iyakki:{}", env!("CARGO_PKG_VERSION")));
    let image = crate::ocipush::build_image(&crate::ocipush::ImageSpec { file: "iyakki", contents: &bytes, arch: &arch, user: "65532:65532", args: vec!["serve".into()], port: 8443 })?;
    let tar = crate::ocipush::layout_tar(&image, &reference)?;
    Ok(Made { image, tar, reference, arch, warnings })
}

fn split_ref(r: &str) -> Result<(String, String, String), String> {
    let (hostport, rest) = r.split_once('/').ok_or("--push HOST:PORT/REPO:TAG")?;
    let (repo, tag) = rest.rsplit_once(':').unwrap_or((rest, "latest"));
    if hostport.is_empty() || repo.is_empty() {
        return Err("--push HOST:PORT/REPO:TAG".into());
    }
    Ok((hostport.into(), repo.into(), tag.into()))
}

fn readme(c: &TenantConfig) -> String {
    format!(
        "IYAKKI tenant \"{t}\"\n\n1. Load the image where the cluster can pull it:\n     docker load -i iyakki-image.tar      (or podman load, or k3s ctr images import iyakki-image.tar)\n   \
         or send it to a registry:  iyakki cloud image --push HOST:PORT/iyakki:tag  and set \"image\" in tenant.json.\n\n\
         2. Make the namespace and the accounts' Secret:\n     kubectl create namespace {ns}\n     {secret}\n\n\
         3. Apply:   kubectl apply -f manifests.yaml     (or: iyakki cloud deploy --config tenant.json --apply)\n\n4. {reach}\n",
        t = c.tenant,
        ns = c.namespace(),
        secret = secret_command(c),
        reach = reach_text(c)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-cloudcli-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// A tiny static x86-64 ELF header with no interpreter (what `elf_info` reads).
    fn fake_elf(interp: bool) -> Vec<u8> {
        let mut b = vec![0u8; 64 + 56];
        b[..4].copy_from_slice(b"\x7fELF");
        b[4] = 2;
        b[5] = 1;
        b[18] = 62;
        b[32..40].copy_from_slice(&64u64.to_le_bytes());
        b[54..56].copy_from_slice(&56u16.to_le_bytes());
        b[56..58].copy_from_slice(&1u16.to_le_bytes());
        b[64..68].copy_from_slice(&(if interp { 3u32 } else { 1u32 }).to_le_bytes());
        b
    }

    #[test]
    fn the_program_is_recognised_by_its_elf_header() {
        assert_eq!(elf_info(&fake_elf(false)), Some(("amd64", true)));
        assert_eq!(elf_info(&fake_elf(true)), Some(("amd64", false)));
        assert_eq!(elf_info(b"#!/bin/sh\n"), None);
    }

    #[test]
    fn example_and_manifests_commands_work_end_to_end() {
        let d = tmp("m");
        let mut said = Vec::new();
        run(&args(&["example", "acme"]), &mut |s| said.push(s.to_string())).unwrap();
        let cfg = d.join("tenant.json");
        std::fs::write(&cfg, &said[0]).unwrap();
        let mut y = Vec::new();
        run(&args(&["manifests", "--config", cfg.to_str().unwrap()]), &mut |s| y.push(s.to_string())).unwrap();
        assert!(y[0].contains("kind: Deployment"));
        assert!(run(&args(&["example", "Bad Name"]), &mut |_| {}).is_err());
        assert!(run(&args(&["manifests"]), &mut |_| {}).unwrap_err().contains("--config"));
        // deploy without --apply changes nothing and says how to continue
        let mut o = Vec::new();
        run(&args(&["deploy", "--config", cfg.to_str().unwrap()]), &mut |s| o.push(s.to_string())).unwrap();
        assert!(o.last().unwrap().contains("create secret generic iyakki-accounts"));
        assert!(o.iter().any(|l| l.contains("Nothing was applied")));
    }

    #[test]
    fn the_bundle_holds_config_manifests_image_and_instructions() {
        let d = tmp("b");
        let cfg = d.join("t.json");
        std::fs::write(&cfg, r#"{"tenant":"acme","domain":"acme.example.org"}"#).unwrap();
        let bin = d.join("iyakki-bin");
        std::fs::write(&bin, fake_elf(false)).unwrap();
        let out = d.join("out");
        let mut said = Vec::new();
        run(&args(&["bundle", "--config", cfg.to_str().unwrap(), "--out", out.to_str().unwrap(), "--binary", bin.to_str().unwrap()]), &mut |s| said.push(s.to_string())).unwrap();
        for f in ["tenant.json", "manifests.yaml", "iyakki-image.tar", "README.txt"] {
            assert!(out.join(f).is_file(), "{f}");
        }
        assert!(std::fs::read_to_string(out.join("manifests.yaml")).unwrap().contains("kind: Ingress"));
        assert!(std::fs::read_to_string(out.join("README.txt")).unwrap().contains("kubectl apply -f manifests.yaml"));
        assert!(said.iter().all(|l| !l.starts_with("warning")), "a static program gives no warning: {said:?}");
        // the tar is an OCI layout: oci-layout, index.json and three blobs
        let tar = std::fs::read(out.join("iyakki-image.tar")).unwrap();
        let text = String::from_utf8_lossy(&tar);
        assert!(text.contains("oci-layout") && text.contains("index.json") && text.matches("blobs/sha256/").count() == 3);
        assert!(text.contains("acme") || text.contains("iyakki:"), "the image's reference is in the index");
        // a dynamic program is warned about, a non-ELF refused
        std::fs::write(&bin, fake_elf(true)).unwrap();
        let mut w = Vec::new();
        run(&args(&["image", "--binary", bin.to_str().unwrap(), "--out", d.join("i.tar").to_str().unwrap()]), &mut |s| w.push(s.to_string())).unwrap();
        assert!(w.iter().any(|l| l.starts_with("warning: ") && l.contains("not a static")));
        std::fs::write(&bin, b"not a program").unwrap();
        assert!(run(&args(&["image", "--binary", bin.to_str().unwrap()]), &mut |_| {}).is_err());
    }

    #[test]
    fn the_layout_is_the_same_every_time() {
        let d = tmp("s");
        let bin = d.join("p");
        std::fs::write(&bin, fake_elf(false)).unwrap();
        let a = make_image(&args(&["--binary", bin.to_str().unwrap(), "--name", "iyakki:1"])).unwrap();
        let b = make_image(&args(&["--binary", bin.to_str().unwrap(), "--name", "iyakki:1"])).unwrap();
        assert_eq!(a.tar, b.tar);
        assert!(crate::ocipush::layout_tar(&a.image, "bad name").is_err());
    }

    #[test]
    fn a_registry_reference_is_split() {
        assert_eq!(split_ref("localhost:30500/iyakki:1.2").unwrap(), ("localhost:30500".into(), "iyakki".into(), "1.2".into()));
        assert_eq!(split_ref("reg:5000/team/iyakki").unwrap().2, "latest");
        assert!(split_ref("nope").is_err());
    }

    #[test]
    fn routes_cert_and_roles_commands_work_end_to_end() {
        let d = tmp("rcr");
        let mut o = Vec::new();
        let mut say = |v: &mut Vec<String>, a: &[&str]| run(&args(a), &mut |s| v.push(s.to_string()));
        // routes -> an ingress
        let routes = d.join("routes.json");
        std::fs::write(&routes, r#"{"routes":[{"host":"app.example.org","target":{"service":{"name":"iyakki","port":8080}}}]}"#).unwrap();
        say(&mut o, &["routes", "--file", routes.to_str().unwrap(), "--namespace", "iyakki-acme"]).unwrap();
        assert!(o[0].contains("kind: Ingress") && o[0].contains("app.example.org"), "{}", o[0]);
        // a self-signed certificate, then checked
        let mut o = Vec::new();
        say(&mut o, &["cert", "self-signed", "app.example.org", "--out", d.to_str().unwrap()]).unwrap();
        assert!(o[0].contains("self-signed"));
        let (c, k) = (d.join("app.example.org.crt"), d.join("app.example.org.key"));
        let mut o = Vec::new();
        say(&mut o, &["cert", "check", "--cert", c.to_str().unwrap(), "--key", k.to_str().unwrap(), "--host", "app.example.org"]).unwrap();
        assert!(o[0].contains("valid for"), "{}", o[0]);
        assert!(say(&mut Vec::new(), &["cert", "check", "--cert", c.to_str().unwrap(), "--key", k.to_str().unwrap(), "--host", "other.example.org"]).unwrap_err().contains("covers"));
        // roles: deny by default, a tenant admin sees names but cannot read, joining is recorded
        let r = d.join("roles.json");
        let rf = r.to_str().unwrap();
        let mut o = Vec::new();
        say(&mut o, &["roles", "init", "olga", "--file", rf]).unwrap();
        say(&mut o, &["roles", "member", "add", "mia", "--role", "member", "--as", "olga", "--file", rf]).unwrap();
        say(&mut o, &["roles", "namespace", "add", "data", "--as", "olga", "--file", rf]).unwrap();
        let mut o = Vec::new();
        say(&mut o, &["roles", "can", "read", "data", "--as", "mia", "--file", rf]).unwrap();
        say(&mut o, &["roles", "can", "read", "data", "--as", "olga", "--file", rf]).unwrap();
        say(&mut o, &["roles", "can", "read", "data", "--as", "eve", "--file", rf]).unwrap();
        assert!(o[0].contains("Denied(NotInNamespace)") && o[1].contains("Allowed") && o[2].contains("Denied(NotInTenant)"), "{o:?}");
        assert!(say(&mut Vec::new(), &["roles", "member", "add", "zed", "--as", "mia", "--file", rf]).is_err(), "a member cannot add");
        assert!(say(&mut Vec::new(), &["roles", "show", "--file", "/nonexistent/roles.json"]).is_err());
    }
}
