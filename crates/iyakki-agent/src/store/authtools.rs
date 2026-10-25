//! The assistant's authoring tools: `app_scaffold` (writes a starting project into the workspace, asks), `store_validate`
//! (read-only checks), `store_publish` (pushes a signed package to a private store; always asks, even in Auto, and shows the
//! server, the package and its trust level first) and `store_server_status`.

use std::path::Path;

use iyakki_sec::policy::{App, Policy};
use serde_json::{json, Value};

use super::install::Actor;
use super::{author, vet, Store};
use crate::agent::{Risk, ToolDef};
use crate::chat::ToolSpec;
use crate::services::Services;

pub const TOOLS: [&str; 13] = ["tile_ship", "app_scaffold", "store_validate", "store_publish", "store_server_status", "tile_scaffold", "tile_validate", "tile_test", "tile_install_dev", "tile_pack", "app_preflight", "app_bump_version", "publisher_keys"];

pub fn is_author_tool(name: &str) -> bool {
    TOOLS.contains(&name)
}

/// The publish token for a catalog source: the one the person put in their vault (`store-token-<id>`), else, for a private store that
/// IYAKKI deployed on this device and the person added, the admin token that was made and kept in the vault at deploy
/// (`svc.<instance>.admin-token`). Never shown to anyone.
pub fn publish_token(svc: &Services, user: &str, src: &super::catalog::SourceCfg) -> Option<zeroize::Zeroizing<String>> {
    if let Ok(t) = svc.user_get(user, &token_entry(&src.id)) {
        return Some(t);
    }
    let inst = crate::platform::installed::load(svc.dir(), user).into_iter().find(|i| i.service == super::privatestore::ID && i.urls.iter().any(|u| u.kind == "catalog" && super::privatepub::same_location(&u.url, &src.location)))?;
    let name = inst.vault.iter().find(|v| v.ends_with(".admin-token"))?.clone();
    svc.user_get(user, &name).ok()
}

/// Why nothing can be published yet, in the words the agent passes on to the person (one sentence, the one next step).
fn no_target_text(svc: &Services, user: &str, st: &Store) -> String {
    let info = super::privatestore::status_info(svc.dir(), user, st);
    match (&info.instance, &info.added) {
        (None, _) => "No private store exists on this device yet. Tell the person exactly this and stop: \"Deploy the private store first (STORE \u{2192} Private store \u{2192} Deploy; one click now creates the registry), then ask me again.\" Do not try other ways to publish and do not ask for keys or tokens.".to_string(),
        (Some(_), None) => match super::privatepub::offer(svc, user, st, &super::catalog::HttpFetch(svc.quick_upstream())) {
            Some(o) => match &o.probe {
                Ok(p) => format!("The private store {} is deployed on this device but not added to its STORE yet. Its catalog key fingerprint is {} (read just now from {}). Do NOT send the person to Settings: call this tool again with add_deployed_store set to exactly that fingerprint. The person then gets one approval card that shows the fingerprint; when they approve, the store is added with that key pinned, chosen as the publishing target, and the publish goes on.", p.name, p.fingerprint, o.dep.catalog_url.trim_end_matches("/catalog.json")),
                Err(why) => format!("The private store is deployed on this device but not added to its STORE yet, and it cannot be read now. Tell the person exactly this and stop: \"{why}\""),
            },
            None => "The private store is deployed but not added to this device's STORE yet. Tell the person: \"Add it in STORE \u{2192} Settings \u{2192} Private store \u{2192} Add this store (check the fingerprint), then ask me again.\"".to_string(),
        },
        (Some(_), Some(_)) => "The private store is not chosen as the publishing target. Tell the person: \"Choose it as the publishing target in STORE \u{2192} Settings \u{2192} Private store, then ask me again.\"".to_string(),
    }
}

/// [`target_source`] with the explanation of what is missing.
pub fn target_for(svc: &Services, user: &str, store: &Store, target: &str) -> Result<super::catalog::SourceCfg, String> {
    target_source(store, target).map_err(|e| {
        if target.trim().is_empty() || store.config().sources.iter().all(|s| s.pin.is_empty()) {
            no_target_text(svc, user, store)
        } else {
            e
        }
    })
}

/// [`target_for`], and when the private store of this device is deployed but not added, add it (with the key fingerprint the person
/// approved on the card, passed back as `add_fp`) and make it the target. Without `add_fp` the explanation comes back instead.
pub fn target_or_add(svc: &Services, user: &str, store: &Store, actor: &Actor, target: &str, add_fp: &str) -> Result<super::catalog::SourceCfg, String> {
    match target_for(svc, user, store, target) {
        Ok(s) => Ok(s),
        Err(e) if add_fp.trim().is_empty() => Err(e),
        Err(e) => {
            let info = super::privatestore::status_info(svc.dir(), user, store);
            if info.instance.is_none() || info.added.is_some() {
                return Err(e);
            }
            super::privatepub::add_for_assistant(svc, user, store, actor, &super::catalog::HttpFetch(svc.quick_upstream()), add_fp)
        }
    }
}

/// The vault entry that holds the publish token of a catalog source.
pub fn token_entry(source_id: &str) -> String {
    format!("store-token-{source_id}")
}

pub fn defs(p: &Policy) -> Vec<ToolDef> {
    if !p.apps.contains(&App::Store) {
        return vec![];
    }
    let obj = |props: Value, req: &[&str]| json!({"type": "object", "properties": props, "required": req});
    let tool = |name: &str, desc: &str, schema: Value, risk: Risk| ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema }, risk, untrusted: false };
    let mut d = vec![
        tool("store_validate", "Check a project folder (made by app_scaffold, with iyakki-app.json) or a package file (.iyp) in the workspace the way the store's install review does: manifest, size, permissions in plain words, the licence against the permissive list, a scan for secrets, the name against look-alikes. Fix every PROBLEM before packaging or publishing. `path` is inside the workspace. Read-only.", obj(json!({"path": {"type": "string"}}), &["path"]), Risk::Safe),
        tool("store_server_status", "Say whether a private IYAKKI store is deployed from this device, its catalog address, whether it is added to the STORE with its key pinned, and where publishing goes by default. Read-only. To deploy one use platform_deploy with service private-store (the person picks the cluster and approves).", obj(json!({}), &[]), Risk::Safe),
        tool("store_publish", "Publish to a private store: `package_path` is a tile app / project FOLDER (it is checked, packed and signed with the person\'s key, made automatically if needed, then published; same as tile_ship) or an already signed package (.iyp). The person is shown the server, the package (name, version, permissions, trust level) and must approve; this always asks, even in Auto mode, and the publish token comes from their vault by itself, never shown to you. `target` is the catalog name or id of a private store added in STORE (default: the one chosen as the publishing target).", obj(json!({"package_path": {"type": "string"}, "target": {"type": "string"}, "add_deployed_store": {"type": "string", "description": "Only when the tool said the private store is deployed on this device but not added: the key fingerprint it printed, passed back unchanged. The person approves adding that store with that key."}}), &["package_path"]), Risk::Ask),
    ];
    if !p.fs_write.is_empty() {
        d.push(tool("app_scaffold", &format!("Create a starting project in the workspace for something that extends IYAKKI, with a README and a test: `kind` is one of {}. `name` is lowercase letters, digits and dashes; `dir` is a new or empty folder inside the workspace (default: the name). Then edit the files, run the project's test, store_validate it, pack and sign it, and store_publish it. Read the skills iyakki-app-author, iyakki-mcp-author and iyakki-skill-author first.", author::SCAFFOLDS.iter().map(|s| format!("{} ({})", s.0, s.1)).collect::<Vec<_>>().join("; ")), obj(json!({"kind": {"type": "string", "enum": author::SCAFFOLDS.iter().map(|s| s.0).collect::<Vec<_>>()}, "name": {"type": "string"}, "dir": {"type": "string"}}), &["kind", "name"]), Risk::Ask));
    }
    d.push(tool("app_preflight", "Everything to check before publishing the project in `path` (a workspace folder): validation problems and warnings, for a tile app its headless tests, the change log, and the version the target store already has (`target` as for store_publish). Read-only.", obj(json!({"path": {"type": "string"}, "target": {"type": "string"}}), &["path"]), Risk::Safe));
    d.push(tool("publisher_keys", "List the person's publisher signing keys by name and fingerprint (never the key itself) and say which one signs by default. You never need one: packing and publishing make and use the key themselves. Read-only.", obj(json!({}), &[]), Risk::Safe));
    if !p.fs_write.is_empty() {
        d.push(tool("app_bump_version", "Raise the version of the project in `path` (`bump`: patch, minor or major) in iyakki-app.json (and app.json of a tile app) and add `line`, one sentence saying what changed, to CHANGELOG.md. A store takes only a version higher than the ones it has. The person approves.", obj(json!({"path": {"type": "string"}, "bump": {"type": "string", "enum": ["patch", "minor", "major"]}, "line": {"type": "string"}}), &["path", "bump", "line"]), Risk::Ask));
    }
    // tile apps: an app that gets its own tile in the launcher
    d.push(tool("tile_validate", "Check an IYAKKI tile app folder (index.html, app.json, scripts): the manifest, the icon, sizes, remote scripts that are not in capabilities.network, eval, capabilities used but not declared, file: addresses and secrets. Fix every PROBLEM. `path` is inside the workspace. Read-only.", obj(json!({"path": {"type": "string"}}), &["path"]), Risk::Safe));
    d.push(tool("tile_test", "Run the tile app's headless tests (tests/*.js with T.test, T.click, T.type, T.text, T.eq, T.ok) on IYAKKI's own browser engine, with real clicks, and report each test. `path` is the app folder in the workspace. Nothing is written or opened; the app gets no network.", obj(json!({"path": {"type": "string"}}), &["path"]), Risk::Safe));
    if !p.fs_write.is_empty() {
        d.push(tool("tile_scaffold", "Create an IYAKKI tile app in the workspace: index.html, app.js, style.css, app.json, iyakki-app.json, README.md and a headless test. `name` is lowercase letters, digits and dashes; `dir` is a new or empty folder inside the workspace (default: the name). Use this, not a server and a Dockerfile, when the person wants an app for IYAKKI, a tile or the launcher. Read the skill iyakki-tile-author first.", obj(json!({"name": {"type": "string"}, "dir": {"type": "string"}}), &["name"]), Risk::Ask));
        d.push(tool("tile_pack", "Pack the tile app folder into a package file (.iyp) in the workspace, signed with the person's publisher key (made automatically when there is none), after the same checks as tile_validate. `path` is the app folder; `output` is the file to write (default <name>.iyp next to the folder). Then store_publish it.", obj(json!({"path": {"type": "string"}, "output": {"type": "string"}}), &["path"]), Risk::Ask));
    }
    if !p.fs_write.is_empty() {
        d.push(tool("tile_ship", "ONE STEP from a tile app folder to the private store: checks it (the same checks as tile_validate and app_preflight, headless tests included), packs it, signs it with the person's own publisher key (made automatically), and publishes it. The person sees one approval card with the name, version, permissions in plain words and the store. If the store already has this version, the patch number is raised for you. Returns the tile name and the result. Never ask the person for keys, accounts or tokens: none are needed. `path` (or `dir`) is the app folder; `target` as for store_publish.", obj(json!({"path": {"type": "string"}, "dir": {"type": "string"}, "target": {"type": "string"}, "add_deployed_store": {"type": "string", "description": "Only when the tool said the private store is deployed on this device but not added: the key fingerprint it printed, passed back unchanged."}}), &[]), Risk::Ask));
    }
    if p.manage_settings {
        d.push(tool("tile_install_dev", "Add the tile app in a workspace folder to the person's launcher as an UNSIGNED DEV APP (shown clearly as such; it reloads when its files change). The person is asked; only administrators can do it. `path` is the app folder.", obj(json!({"path": {"type": "string"}}), &["path"]), Risk::Ask));
    }
    d
}

fn arg<'a>(args: &'a Value, k: &str) -> &'a str {
    args.get(k).and_then(Value::as_str).map(str::trim).unwrap_or("")
}

/// Which catalog source a publish goes to: the named one, else the default target.
pub fn target_source(store: &Store, target: &str) -> Result<super::catalog::SourceCfg, String> {
    let cfg = store.config();
    let t = target.trim();
    let want = if t.is_empty() { cfg.publish_target.clone() } else { t.to_string() };
    if want.is_empty() {
        return Err("no publishing target: add a private store in STORE, Settings, Private store (or name one with `target`) and choose it as the publishing target".into());
    }
    let s = cfg.sources.iter().find(|s| s.id == want || s.name.eq_ignore_ascii_case(&want)).ok_or_else(|| format!("there is no catalog called {want:?} in STORE, Settings, Catalogs"))?;
    if s.pin.is_empty() || s.kind != super::catalog::SourceKind::Url {
        return Err(format!("{} is not a private store with a pinned key: only those can be published to", s.name));
    }
    Ok(s.clone())
}

/// `https://host` of a source's catalog address.
pub fn server_of(s: &super::catalog::SourceCfg) -> String {
    s.location.trim_end_matches("/catalog.json").trim_end_matches('/').to_string()
}

fn package_line(svc: &Services, path: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|e| crate::scope::explain_io(&e, path, "read"))?;
    let pk = crate::pkg::parse(&text).map_err(|e| format!("not a valid package: {e}"))?;
    let st = Store::for_services(svc);
    let own = svc.packages.trusted_keys();
    let revs = st.revocations();
    let v = st.judge(&own, &revs).verdict(&pk.manifest);
    let rep = vet::vet_package(&text, &[]);
    let perms = rep.facts.iter().find_map(|f| f.strip_prefix("permissions: ")).unwrap_or("none");
    Ok(format!("{} {} {} (licence {}, {} bytes), trust level {} ({}), permissions: {perms}", pk.manifest.kind, pk.manifest.name, pk.manifest.version, pk.manifest.license, text.len(), v.level.label(), v.label))
}

/// The first half of an approval card when the publish also adds the private store deployed on this device: the address and the key
/// fingerprint the person is approving. None when nothing is to be added (a target exists, or `add_deployed_store` was not given).
fn add_card(svc: &Services, user: &str, st: &Store, args: &Value) -> Option<String> {
    let fp = arg(args, "add_deployed_store");
    if fp.is_empty() || target_source(st, arg(args, "target")).is_ok() {
        return None;
    }
    let o = super::privatepub::offer(svc, user, st, &super::catalog::HttpFetch(svc.quick_upstream()))?;
    Some(match &o.probe {
        Ok(p) if p.fingerprint.split_whitespace().collect::<String>().eq_ignore_ascii_case(&fp.split_whitespace().collect::<String>()) => format!("ADD the private store {} ({}) to this device's STORE with its catalog key pinned, key fingerprint {} (compare it with the one printed after the deploy), and CHOOSE it as the publishing target; then", p.name, o.dep.catalog_url.trim_end_matches("/catalog.json"), p.fingerprint),
        Ok(p) => format!("(REFUSED: the fingerprint {fp:?} is not the key of the private store, which shows {}; nothing will be added) then", p.fingerprint),
        Err(why) => format!("(cannot add the private store yet: {why}) then"),
    })
}

pub fn describe(svc: &Services, cwd: &Path, name: &str, args: &Value) -> String {
    describe_for(svc, "", cwd, name, args)
}

pub fn describe_for(svc: &Services, user: &str, cwd: &Path, name: &str, args: &Value) -> String {
    match name {
        "app_scaffold" => format!("create a {} project named {} in {}", arg(args, "kind"), arg(args, "name"), if arg(args, "dir").is_empty() { arg(args, "name") } else { arg(args, "dir") }),
        "store_validate" => format!("check {} for problems", arg(args, "path")),
        "store_server_status" => "look at the private store".into(),
        "app_preflight" => format!("check {} before publishing", arg(args, "path")),
        "app_bump_version" => format!("raise the {} version of {} and add to its change log: {}", arg(args, "bump"), arg(args, "path"), arg(args, "line")),
        "publisher_keys" => "list your publisher signing keys (names and fingerprints only)".into(),
        "tile_scaffold" => format!("create the tile app {} in {}", arg(args, "name"), if arg(args, "dir").is_empty() { arg(args, "name") } else { arg(args, "dir") }),
        "tile_validate" => format!("check the tile app {} for problems", arg(args, "path")),
        "tile_test" => format!("run the headless tests of {}", arg(args, "path")),
        "tile_pack" => format!("pack the tile app {} into a package file", arg(args, "path")),
        "tile_install_dev" => format!("ADD {} to your launcher as an UNSIGNED DEV APP: a tile that runs the files in that folder, marked unsigned, only for you, reloading when the files change", arg(args, "path")),
        "tile_ship" => ship_card(svc, cwd, args, user),
        "store_publish" if author::inside(cwd, arg(args, "package_path")).is_ok_and(|p| p.is_dir()) => ship_card(svc, cwd, &json!({"path": arg(args, "package_path"), "target": arg(args, "target"), "add_deployed_store": arg(args, "add_deployed_store")}), user),
        "store_publish" => {
            let st = Store::for_services(svc);
            let adding = add_card(svc, user, &st, args);
            let to = match target_source(&st, arg(args, "target")) {
                Ok(s) => format!("{} ({}), whose key you pinned", s.name, server_of(&s)),
                Err(_) if adding.is_some() => "that store".to_string(),
                Err(e) => format!("(no usable target: {e})"),
            };
            let pkg = author::inside(cwd, arg(args, "package_path")).and_then(|p| package_line(svc, &p)).unwrap_or_else(|e| format!("(unreadable: {e})"));
            format!("{}PUBLISH to {to}: {pkg}. Everyone who uses that store can then install it. You use your vault's publish token for it.", adding.map(|a| format!("{a} ")).unwrap_or_default())
        }
        _ => name.into(),
    }
}

pub fn call(name: &str, args: &Value, svc: &Services, p: &Policy, user: &str, cwd: &Path) -> Result<String, String> {
    if !p.apps.contains(&App::Store) {
        return Err("your account may not use the store".into());
    }
    let st = Store::for_services(svc);
    match name {
        "app_scaffold" => {
            if p.fs_write.is_empty() {
                return Err("your account may not write files".into());
            }
            author::scaffold(arg(args, "kind"), arg(args, "name"), cwd, arg(args, "dir"))
        }
        "store_validate" => {
            let path = author::inside(cwd, arg(args, "path"))?;
            if path.is_dir() {
                let r = vet::vet_dir(&path);
                Ok(r.text())
            } else {
                let text = std::fs::read_to_string(&path).map_err(|e| crate::scope::explain_io(&e, &path, "read"))?;
                let r = vet::vet_package(&text, &[]);
                let trust = package_line(svc, &path).ok();
                Ok(format!("{}{}", r.text(), trust.map(|t| format!("\n{t}")).unwrap_or_default()))
            }
        }
        "app_preflight" => {
            let path = author::inside(cwd, arg(args, "path"))?;
            let pf = super::publishflow::preflight(&path);
            let mut o = format!("{} {} {}\n", pf.kind, pf.name, pf.version);
            for x in &pf.problems {
                o.push_str(&format!("PROBLEM: {x}\n"));
            }
            for x in &pf.warnings {
                o.push_str(&format!("warning: {x}\n"));
            }
            if let Ok(src) = target_source(&st, arg(args, "target")) {
                match super::publishflow::remote_version(&super::catalog::HttpFetch(svc.quick_upstream()), &server_of(&src), &pf.name) {
                    Some(v) => o.push_str(&format!("{} has version {v}; yours is {}{}\n", src.name, pf.version, if crate::pkg::newer(&pf.version, &v) { " (higher: ok)" } else { ": raise it with app_bump_version" })),
                    None => o.push_str(&format!("{} does not list {} yet (or could not be reached)\n", src.name, pf.name)),
                }
            }
            o.push_str(if pf.ok() { "result: ready to pack and publish" } else { "result: NOT ready" });
            Ok(o)
        }
        "publisher_keys" => {
            let k = super::keys::list(svc, user);
            Ok(if k.is_empty() { "No publisher key yet; packing or publishing makes one by itself, there is nothing to do.".to_string() } else { k.iter().map(|k| format!("{}{}: {}", k.label, if k.default { " (default)" } else { "" }, k.fingerprint)).collect::<Vec<_>>().join("\n") })
        }
        "app_bump_version" => {
            if p.fs_write.is_empty() {
                return Err("your account may not write files".into());
            }
            let path = author::inside(cwd, arg(args, "path"))?;
            let b = super::publishflow::Bump::from_label(arg(args, "bump")).ok_or("bump: patch, minor or major")?;
            let v = super::publishflow::apply_bump(&path, b, arg(args, "line"), &crate::clock::today())?;
            Ok(format!("Version is now {v}; CHANGELOG.md has a line for it."))
        }
        "tile_scaffold" => {
            if p.fs_write.is_empty() {
                return Err("your account may not write files".into());
            }
            let r = author::scaffold("tile", arg(args, "name"), cwd, arg(args, "dir"))?;
            if crate::target::read(cwd).is_none() {
                let _ = crate::target::write(cwd, "", crate::target::Target::IyakkiTile);
            }
            Ok(r)
        }
        "tile_validate" => {
            let path = author::inside(cwd, arg(args, "path"))?;
            if !path.is_dir() {
                return Err("`path` is the app's folder".into());
            }
            Ok(vet::vet_dir(&path).text())
        }
        "tile_test" => {
            let path = author::inside(cwd, arg(args, "path"))?;
            Ok(super::tile::run_tests(&path)?.text())
        }
        "tile_pack" => {
            if p.fs_write.is_empty() {
                return Err("your account may not write files".into());
            }
            let path = author::inside(cwd, arg(args, "path"))?;
            let key = super::keys::ensure_default(svc, user).ok().map(|(k, fp, _)| (k, fp));
            let (text, notes) = author::pack_dir(&path, key.as_ref().map(|k| &k.0))?;
            let name = crate::pkg::parse(&text).map(|p| p.manifest.name).unwrap_or_default();
            let out = author::inside(cwd, if arg(args, "output").is_empty() { format!("{name}.iyp") } else { arg(args, "output").to_string() }.as_str())?;
            if let Some(d) = out.parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            std::fs::write(&out, &text).map_err(|e| crate::scope::explain_io(&e, &out, "write"))?;
            Ok(format!("Packed {} ({} bytes){}. {}", out.display(), text.len(), key.as_ref().map(|k| format!(", signed with your key {}", k.1)).unwrap_or_default(), notes.join("; ")))
        }
        "tile_install_dev" => {
            let path = author::inside(cwd, arg(args, "path"))?;
            let e = super::tilehub::add_dev(svc, p, user, &path)?;
            Ok(format!("Added “{}” to your launcher in group {} as an unsigned dev app. Open My apps; it reloads when you change the files.", e.title, e.group))
        }
        "store_server_status" => Ok(super::privatestore::status_text(&super::privatestore::status_info(svc.dir(), user, &st))),
        "tile_ship" => {
            if p.fs_write.is_empty() {
                return Err("your account may not write files".into());
            }
            let a = Actor::from_policy(user, p);
            if !a.admin && !a.can_own {
                return Err("your account may not publish".into());
            }
            let dir = if arg(args, "path").is_empty() { arg(args, "dir") } else { arg(args, "path") };
            ship(svc, user, &a, cwd, dir, arg(args, "target"), arg(args, "add_deployed_store"))
        }
        "store_publish" => {
            let a = Actor::from_policy(user, p);
            if !a.admin && !a.can_own {
                return Err("your account may not publish".into());
            }
            let path = author::inside(cwd, arg(args, "package_path"))?;
            if path.is_dir() {
                return ship(svc, user, &a, cwd, arg(args, "package_path"), arg(args, "target"), arg(args, "add_deployed_store"));
            }
            let src = target_or_add(svc, user, &st, &a, arg(args, "target"), arg(args, "add_deployed_store"))?;
            let text = std::fs::read_to_string(&path).map_err(|e| crate::scope::explain_io(&e, &path, "read"))?;
            let rep = vet::vet_package(&text, &[]);
            if !rep.ok() {
                return Err(format!("not published, the package has problems:\n{}", rep.text()));
            }
            let token = publish_token(svc, user, &src).ok_or_else(|| no_token_text(&src))?;
            let r = author::publish_to_server(&*svc.quick_upstream(), &server_of(&src), &token, &text)?;
            Ok(format!("Published {} {} to {} (listed as {}, catalog number {}). People who added that store see it after they check for updates.", r.name, r.version, src.name, r.trust, r.serial))
        }
        _ => Err(format!("unknown tool `{name}`")),
    }
}

fn no_token_text(src: &super::catalog::SourceCfg) -> String {
    format!("{} has no publish token on this device (it was not deployed from here). Tell the person in one sentence: \"Add the publish token of {} in STORE \u{2192} Settings \u{2192} Private store, then ask me again.\"", src.name, src.name)
}

/// The card text of a one-step publish: what, where, with which permissions, and as what trust. Packs with a throw-away key when the
/// person has none yet (nothing is stored; the real key is made when the person approves).
fn ship_card(svc: &Services, cwd: &Path, args: &Value, user: &str) -> String {
    let dir_arg = if arg(args, "path").is_empty() { arg(args, "dir") } else { arg(args, "path") };
    let st = Store::for_services(svc);
    let src = match target_for(svc, user, &st, arg(args, "target")) {
        Ok(s) => s,
        Err(_) if add_card(svc, user, &st, args).is_some() => return format!("{} PUBLISH {dir_arg} (checked, packed and signed with your own key) to it. Everyone who uses that store can then install it.", add_card(svc, user, &st, args).unwrap_or_default()),
        Err(e) => return format!("PUBLISH {dir_arg}: (not possible yet: {e})"),
    };
    let Ok(dir) = author::inside(cwd, dir_arg) else { return format!("PUBLISH {dir_arg}: (the folder must be inside the workspace)") };
    let key = super::keys::load_default(svc, user).unwrap_or_else(|| {
        let k = crate::pkg::new_signing_key().expect("a random key");
        let fp = super::server::fingerprint(&crate::pkg::public_key_hex(&k));
        (k, fp)
    });
    match super::publishflow::card(svc, user, &dir, &src, &key, &super::catalog::HttpFetch(svc.quick_upstream())) {
        Ok((c, _)) => format!(
            "PUBLISH {} {} ({}) to {} ({}). Permissions: {}. Files: {}. Signed with your own publisher key (made for you if you have none) and listed as {} for you. Everyone who uses that store can then install it.",
            c.name,
            c.version,
            c.kind,
            c.store,
            c.server,
            if c.permissions.is_empty() { "none".to_string() } else { c.permissions.join("; ") },
            c.files.len(),
            "Trusted (your own key)"
        ),
        Err(e) => format!("PUBLISH {dir_arg} to {}: (cannot be published yet: {})", src.name, e.lines().next().unwrap_or("")),
    }
}

/// validate -> (raise the patch version when the store has this one) -> pack -> sign with the person's own key (made if needed) ->
/// publish with the store's token from the vault. Every failure is one plain message; none asks the agent to fetch a key or token.
fn ship(svc: &Services, user: &str, actor: &Actor, cwd: &Path, dir_arg: &str, target: &str, add_fp: &str) -> Result<String, String> {
    use super::publishflow as pf;
    let st = Store::for_services(svc);
    let src = target_or_add(svc, user, &st, actor, target, add_fp)?;
    let dir = author::inside(cwd, dir_arg)?;
    if !dir.is_dir() {
        return Err("`path` is the app's folder (with app.json and iyakki-app.json); make it with tile_scaffold".into());
    }
    let fetch = super::catalog::HttpFetch(svc.quick_upstream());
    if publish_token(svc, user, &src).is_none() {
        return Err(no_token_text(&src));
    }
    let (key, fp, made) = super::keys::ensure_default(svc, user)?;
    let mut bumped = String::new();
    let mut r = pf::publish(svc, user, &dir, &src, Some(&(key.clone(), fp.clone())), &fetch, crate::clock::now_unix());
    // the store already has this version (a second run of the same request): raise the patch number once and go on
    if let Err(pf::Stop::Refused(m)) = &r {
        if m.contains("is not newer than") {
            let v = pf::apply_bump(&dir, pf::Bump::Patch, "Update.", &crate::clock::today())?;
            bumped = format!(" The store already had the earlier version, so the version was raised to {v}.");
            r = pf::publish(svc, user, &dir, &src, Some(&(key, fp.clone())), &fetch, crate::clock::now_unix());
        }
    }
    match r {
        Ok(p) => Ok(format!("Published the tile app {} {} to {} (listed as {}, catalog number {}). It appears in that store's catalog; the person finds it in STORE after checking for updates.{}{}", p.name, p.version, src.name, p.trust, p.serial, bumped, if made { " A publisher key was made for the person and kept private." } else { "" })),
        Err(pf::Stop::NeedsToken) => Err(no_token_text(&src)),
        Err(pf::Stop::NeedsKey) => Err("the publisher key could not be made on this device; tell the person in one sentence that publishing is not possible right now".into()),
        Err(pf::Stop::Refused(m)) => Err(format!("not published: {m}")),
    }
}
