//! Tests of the private-store pieces on the device: pinning a catalog with a confirmed fingerprint, the ServiceSpec, the scaffolds,
//! the authoring tools and the publish client. Everything runs against temp folders, fakes and a loopback server.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::json;

use super::catalog::tests::FakeFetch;
use super::catalog::{CatalogSigner, SourceKind};
use super::install::tests::{admin, fx, user};
use super::server::{Config, Req, StoreServer};
use super::{author, authtools, privatestore, vet, Store};
use crate::agent::{hard_ask, Risk};
use crate::platform::localbin;
use crate::pkg::{build_skill, new_signing_key, resign, signing_key_hex};
use crate::platform::{BuildCtx, Options};

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-priv-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn server(tag: &str) -> (Arc<StoreServer>, std::path::PathBuf) {
    let d = tmp(tag);
    let mut cfg = Config::new(&d.join("data"));
    cfg.writes_per_min = 1000;
    cfg.admin_token_hash = Some(super::server::hash_token("admin-token-for-tests-0123456789"));
    (Arc::new(StoreServer::open(cfg, &signing_key_hex(&new_signing_key().unwrap())).unwrap()), d)
}

fn get(s: &StoreServer, path: &str) -> Vec<u8> {
    s.handle(&Req { method: "GET".into(), path: path.into(), query: Default::default(), headers: Default::default(), body: vec![] }, "1.1.1.1").body
}

fn skill(name: &str, version: &str) -> String {
    let md = format!("---\nname: {name}\ndescription: Test {name}\ntriggers: test\n---\nDo it.\n");
    let k = new_signing_key().unwrap();
    resign(&build_skill(&md, version, "MIT", Some(&k)).unwrap(), Some(r#"{"title":"T","summary":"S"}"#), None, &k).unwrap()
}

#[test]
fn adding_a_private_store_shows_the_fingerprint_and_pins_the_key_only_when_it_matches() {
    let (srv, d) = server("tofu");
    let f = fx("tofu");
    let fetch = FakeFetch::default();
    fetch.put("https://store.example.lan/catalog.json", &get(&srv, "/catalog.json"));
    let probe = f.store.probe_private(&fetch, "https://store.example.lan/").unwrap();
    assert_eq!((probe.key.as_str(), probe.url.as_str()), (srv.catalog_key().as_str(), "https://store.example.lan/catalog.json"));
    assert_eq!(probe.fingerprint, super::server::fingerprint(&srv.catalog_key()));
    // nothing is saved by looking, a wrong or empty fingerprint adds nothing, and only administrators add
    assert!(f.store.config().sources.is_empty());
    for bad in ["", "0000 0000 0000 0000 0000 0000 0000 0000"] {
        assert!(f.store.add_private_source(&admin(), &fetch, "Mine", "https://store.example.lan", bad).unwrap_err().contains("fingerprint"));
    }
    assert!(f.store.add_private_source(&user("bob"), &fetch, "Mine", "https://store.example.lan", &probe.fingerprint).is_err());
    assert!(f.store.config().sources.is_empty());
    // the confirmed fingerprint (spacing and case do not matter) pins the key
    let id = f.store.add_private_source(&admin(), &fetch, "Mine", "https://store.example.lan", &probe.fingerprint.to_uppercase().replace(' ', "")).unwrap();
    let cfg = f.store.config();
    let s = cfg.sources.iter().find(|s| s.id == id).unwrap();
    assert_eq!((s.pin.as_str(), s.kind), (srv.catalog_key().as_str(), SourceKind::Url));
    // refresh: the catalog counts as signed by a key the person trusts; the package is Community unless the key is trusted
    let r = f.store.refresh(&f.svc, &fetch);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert!(matches!(&r.loaded[0].check.signer, CatalogSigner::Trusted(l) if l.contains("pinned key")), "{:?}", r.loaded[0].check.signer);
    assert!(!r.loaded[0].check.warnings.iter().any(|w| w.contains("nobody vouches")));
    // a different key at the same address (a swapped server) is refused, and its serial is not remembered
    let (evil, d2) = server("tofu-evil");
    fetch.put("https://store.example.lan/catalog.json", &get(&evil, "/catalog.json"));
    let r = f.store.refresh(&f.svc, &fetch);
    assert!(r.errors.iter().any(|e| e.1.contains("different key")) || r.loaded.iter().all(|l| l.catalog.key == srv.catalog_key()), "{:?}", r.errors);
    assert!(f.store.local_catalogs(&f.svc).iter().all(|l| l.catalog.key != evil.catalog_key()));
    // the default publishing target is a catalog that exists
    assert!(f.store.set_publish_target(&admin(), "nope").is_err());
    f.store.set_publish_target(&admin(), &id).unwrap();
    assert_eq!(authtools::target_source(&f.store, "").unwrap().id, id);
    assert!(authtools::target_source(&f.store, "unknown").is_err());
    let _ = (std::fs::remove_dir_all(&d), std::fs::remove_dir_all(&d2), std::fs::remove_dir_all(&f.dir));
}

#[test]
fn a_yanked_package_disappears_from_the_pinned_catalog_on_the_device() {
    let (srv, d) = server("yankdev");
    let f = fx("yankdev");
    let tok = srv.create_publisher("acme", "Acme").unwrap();
    let post = |body: String| srv.handle(&Req { method: "POST".into(), path: "/v1/publish".into(), query: Default::default(), headers: [("authorization".to_string(), format!("Bearer {tok}"))].into(), body: body.into_bytes() }, "2.2.2.2");
    assert_eq!(post(skill("keep-me", "1.0.0")).status, 200);
    assert_eq!(post(skill("drop-me", "1.0.0")).status, 200);
    let fetch = FakeFetch::default();
    let put = |srv: &StoreServer| {
        fetch.put("https://s.example.lan/catalog.json", &get(srv, "/catalog.json"));
        fetch.put("https://s.example.lan/yanked.json", &get(srv, "/yanked.json"));
    };
    put(&srv);
    let probe = f.store.probe_private(&fetch, "https://s.example.lan").unwrap();
    f.store.add_private_source(&admin(), &fetch, "S", "https://s.example.lan", &probe.fingerprint).unwrap();
    assert_eq!(f.store.refresh(&f.svc, &fetch).loaded[0].catalog.entries.len(), 2);
    let del = srv.handle(&Req { method: "DELETE".into(), path: "/v1/packages/drop-me".into(), query: Default::default(), headers: [("authorization".to_string(), "Bearer admin-token-for-tests-0123456789".to_string())].into(), body: vec![] }, "3.3.3.3");
    assert_eq!(del.status, 200);
    put(&srv);
    let l = f.store.refresh(&f.svc, &fetch).loaded.remove(0);
    assert_eq!(l.catalog.entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), vec!["keep-me"]);
    // a yanked list not signed by the pinned key is ignored
    let (other, d2) = server("yankdev2");
    assert!(super::catalog::read_yanked(&String::from_utf8(get(&other, "/yanked.json")).unwrap(), &srv.catalog_key()).is_err());
    let _ = (std::fs::remove_dir_all(&d), std::fs::remove_dir_all(&d2), std::fs::remove_dir_all(&f.dir));
}

fn ctx(opts: &[(&str, &str)]) -> BuildCtx {
    let mut o = Options::default();
    for (k, v) in [("storage_size", "5Gi"), ("exposure", "NodePort"), ("namespace", "iyakki-private-store"), ("publishers", ""), ("read_only", "false"), ("require_trusted", "false"), ("public_url", "")].iter().chain(opts.iter()) {
        o.set(k, v);
    }
    BuildCtx { namespace: "iyakki-private-store".into(), opts: o, version: "1.0.0".into(), images: vec!["localhost:5000/iyakki-platform/iyakki-store-server:1.0.0".into()], storage_class: None, host: "10.0.0.5".into(), http_port: 30300, ssh_port: 30222, vault_prefix: "svc.private-store-iyakki-private-store".into(), instance: "private-store-iyakki-private-store".into() }
}

#[test]
fn the_service_spec_is_registered_and_its_manifest_passes_the_linter_with_no_warnings() {
    let def = crate::platform::spec("private-store").expect("registered in the platform services");
    assert_eq!((def.kind(), def.versions()), ("store", vec!["1.0.0".to_string()]));
    assert!(def.always_mirror());
    for opts in [vec![], vec![("exposure", "ClusterIP")], vec![("exposure", "Ingress"), ("ingress_host", "store.example.lan")], vec![("read_only", "true"), ("require_trusted", "true")]] {
        let b = def.build(&ctx(&opts)).unwrap();
        let l = crate::deploylint::lint(&b.manifest, "iyakki-private-store", &|_| false);
        assert!(l.ok() && l.warnings.is_empty(), "{opts:?}: {}", l.report());
        let m = &b.manifest;
        for must in ["runAsNonRoot: true", "runAsUser: 10001", "readOnlyRootFilesystem: true", "allowPrivilegeEscalation: false", "limits:", "readinessProbe:", "livenessProbe:", "startupProbe:", "drop:", "automountServiceAccountToken: false", "secretName: private-store", "app.kubernetes.io/managed-by: iyakki", "iyakki.dev/service: private-store"] {
            assert!(m.contains(must), "{must} in {opts:?}");
        }
        assert!(!m.contains("hostPath") && !m.contains("hostNetwork") && !m.contains("privileged: true") && !m.contains(":latest"));
        assert!(!m.contains("kind: Secret\n") && !m.contains("password") && !m.contains("iyk_"), "secrets are only referenced");
        assert!(m.contains("--publish-token-file") && m.contains("/secrets/admin-token") && m.contains("--catalog-seed-file"));
    }
    let b = def.build(&ctx(&[("read_only", "true")])).unwrap();
    assert!(b.manifest.contains("--read-only"));
    let sec = &b.secrets[0];
    assert_eq!(sec.name, "private-store");
    assert_eq!(sec.entries.iter().map(|e| (e.key.as_str(), e.reveal)).collect::<Vec<_>>(), vec![("admin-token", true), ("catalog-seed", false)]);
    assert!(sec.entries.iter().all(|e| e.vault.starts_with("svc.private-store-")));
    assert!(b.urls.iter().any(|u| u.kind == "catalog" && u.url == "http://10.0.0.5:30300/catalog.json"));
    // options are checked
    let bad = |k: &str, v: &str| {
        let mut o = Options::default();
        o.set(k, v);
        crate::platform::validate_options(&*def, &o)
    };
    assert!(bad("publishers", "alice,bob").is_ok());
    assert!(bad("publishers", "admin").is_err() && bad("publishers", "Bad Name").is_err());
    assert!(bad("public_url", "ftp://x").is_err() && bad("public_url", "https://store.example.lan").is_ok());
    assert!(bad("exposure", "LoadBalancer").is_err(), "a load balancer needs the public address");
    // the image is built locally and never comes from a public registry
    assert!(def.images("1.0.0")[0].image.starts_with("localhost/") && !def.images("1.0.0")[0].image.contains("docker.io"));
}

#[test]
fn the_image_is_built_from_a_static_binary_only_and_through_the_runner() {
    let d = tmp("elf");
    let elf = |interp: bool| {
        let mut h = vec![0u8; 64 + 2 * 56];
        h[..4].copy_from_slice(b"\x7fELF");
        h[4] = 2;
        h[5] = 1;
        h[18..20].copy_from_slice(&62u16.to_le_bytes());
        h[32..40].copy_from_slice(&64u64.to_le_bytes());
        h[54..56].copy_from_slice(&56u16.to_le_bytes());
        h[56..58].copy_from_slice(&2u16.to_le_bytes());
        h[64..68].copy_from_slice(&1u32.to_le_bytes());
        h[120..124].copy_from_slice(&(if interp { 3u32 } else { 1 }).to_le_bytes());
        h
    };
    std::fs::write(d.join("static"), elf(false)).unwrap();
    std::fs::write(d.join("dynamic"), elf(true)).unwrap();
    std::fs::write(d.join("text"), "#!/bin/sh").unwrap();
    assert!(privatestore::is_static_elf(&d.join("static")));
    assert!(!privatestore::is_static_elf(&d.join("dynamic")) && !privatestore::is_static_elf(&d.join("text")) && !privatestore::is_static_elf(&d.join("missing")));
    // the places searched are the test's own: the real source tree and install are never looked at
    let places = localbin::Places { shipped: vec![], cache: d.join("store-server"), source: None, build_override: None };
    let run = Arc::new(crate::wsexec::FakeRunner::new(&["podman"]));
    run.answer("podman build", "ok");
    let pl = crate::platform::Platform {
        run: run.clone(),
        dir: d.clone(),
        user: "root".into(),
        vault: Arc::new(crate::platform::MemVault::default()),
        audit: Arc::new(|_, _| {}),
        env: crate::devcluster::Env::real(),
        tmp: d.clone(),
        http: Arc::new(crate::platform::connect::FakeHttp::default()),
        gen: Arc::new(|n| "x".repeat(n)),
        live: None,
    };
    // without a static binary the error says how to get one
    let e = privatestore::PrivateStore.prepare_with(&pl, &places, "1.0.0", "amd64", &mut |_| {}).unwrap_err();
    assert!(e.contains("No static") && e.contains("source tree"), "{e}");
    // with one placed in the cache the build goes through the runner
    std::fs::create_dir_all(d.join("store-server")).unwrap();
    std::fs::copy(d.join("static"), d.join("store-server/iyakki")).unwrap();
    privatestore::PrivateStore.prepare_with(&pl, &places, "1.0.0", "amd64", &mut |_| {}).unwrap();
    assert!(run.lines().iter().any(|l| l.starts_with("podman build -t localhost/iyakki-store-server:1.0.0")), "{:?}", run.lines());
    let df = std::fs::read_to_string(d.join("store-server/build-1.0.0/Dockerfile")).unwrap();
    assert!(df.starts_with("FROM scratch") && df.contains("USER 10001") && df.contains("EXPOSE 8080"));
    assert!(d.join("store-server/build-1.0.0/iyakki").is_file());
    // no engine, no build
    let none = crate::platform::Platform { run: Arc::new(crate::wsexec::FakeRunner::new(&[])), ..pl };
    assert!(privatestore::PrivateStore.prepare_with(&none, &places, "1.0.0", "amd64", &mut |_| {}).unwrap_err().contains("podman or docker"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_wrong_cpu_is_named_and_the_image_builds_for_the_clusters_cpu() {
    let d = tmp("cpu");
    let mut arm = vec![0u8; 64];
    arm[..4].copy_from_slice(b"\x7fELF");
    arm[4] = 2;
    arm[5] = 1;
    arm[18..20].copy_from_slice(&183u16.to_le_bytes());
    std::fs::create_dir_all(d.join("store-server")).unwrap();
    std::fs::write(d.join("store-server/iyakki-store-server-aarch64"), &arm).unwrap();
    let places = localbin::Places { shipped: vec![], cache: d.join("store-server"), source: None, build_override: None };
    let run = Arc::new(crate::wsexec::FakeRunner::new(&["podman"]));
    run.answer("podman build", "ok");
    let pl = crate::platform::Platform {
        run: run.clone(),
        dir: d.clone(),
        user: "root".into(),
        vault: Arc::new(crate::platform::MemVault::default()),
        audit: Arc::new(|_, _| {}),
        env: crate::devcluster::Env::real(),
        tmp: d.clone(),
        http: Arc::new(crate::platform::connect::FakeHttp::default()),
        gen: Arc::new(|n| "x".repeat(n)),
        live: None,
    };
    // the file is named for aarch64: an amd64 cluster does not take it
    let e = privatestore::PrivateStore.prepare_with(&pl, &places, "1.0.0", "amd64", &mut |_| {}).unwrap_err();
    assert!(e.contains("No static iyakki-store-server for x86_64"), "{e}");
    assert!(run.lines().is_empty(), "no image was built");
    // an arm64 cluster takes it, and the image is built for linux/arm64
    privatestore::PrivateStore.prepare_with(&pl, &places, "1.0.0", "arm64", &mut |_| {}).unwrap();
    assert!(run.lines().iter().any(|l| l.contains("--platform linux/arm64")), "{:?}", run.lines());
    // the small server and the full program take the same arguments, so the manifest's `store-server ...` works with either
    let main = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../iyakki-store-server/src/main.rs")).unwrap();
    assert!(main.contains("\"store-server\"") && main.contains("run_cli"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_dist_dockerfile_is_multi_stage_non_root_and_ends_in_scratch() {
    let t = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../dist/store-server/Dockerfile")).unwrap();
    let from: Vec<&str> = t.lines().filter(|l| l.starts_with("FROM ")).collect();
    assert!(from.len() >= 2 && from.last().unwrap().starts_with("FROM scratch"), "{from:?}");
    assert!(t.contains("USER 10001") && t.contains("EXPOSE 8080") && t.contains("ENTRYPOINT") && !t.contains("USER root") && !t.contains(":latest"));
    assert!(t.contains("-p iyakki-store-server") && !t.contains("-p iyakki-core"), "the image holds the small static server only");
}

#[test]
fn every_scaffold_passes_its_own_checks_packs_verifies_and_the_python_and_node_ones_pass_their_tests() {
    let root = tmp("scaf");
    for (kind, tool) in [("skill", Some("python3")), ("mcp-python", Some("python3")), ("mcp-node", Some("node")), ("wasm-tool", Some("python3"))] {
        let name = format!("my-{kind}");
        let out = author::scaffold(kind, &name, &root, &name).unwrap();
        assert!(out.contains("Next:"), "{out}");
        let dir = root.join(&name);
        let r = vet::vet_dir(&dir);
        assert!(r.ok(), "{kind}: {}", r.text());
        assert!(r.warnings.iter().all(|w| !w.contains("README") && !w.contains("test file")), "{kind}: {:?}", r.warnings);
        // unsigned, then signed; the signed one verifies and is a valid package for a store
        let (plain, notes) = author::pack_dir(&dir, None).unwrap();
        assert!(notes.iter().any(|n| n.contains("not signed")) && crate::pkg::parse(&plain).is_ok());
        let key = new_signing_key().unwrap();
        let (signed, _) = author::pack_dir(&dir, Some(&key)).unwrap();
        let p = crate::pkg::parse(&signed).unwrap();
        assert!(p.manifest.signer.is_some() && p.manifest.name == name && p.manifest.meta.is_some(), "{kind}");
        let rep = vet::vet_package(&signed, &[]);
        assert!(rep.ok(), "{kind}: {}", rep.text());
        // the template's own tests, where the toolchain exists
        let (prog, test) = match kind {
            "mcp-node" => ("node", "tests/test_server.js"),
            "skill" => ("python3", "tests/test_skill.py"),
            "mcp-python" => ("python3", "tests/test_server.py"),
            _ => ("python3", "tests/check_tool.py"),
        };
        let have = tool.is_some_and(|t| std::process::Command::new(t).arg("--version").output().is_ok());
        if have {
            let o = std::process::Command::new(prog).arg(test).current_dir(&dir).output().unwrap();
            assert!(o.status.success(), "{kind}: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        } else {
            eprintln!("skipping {kind}: {prog} not installed");
        }
    }
    // the WASM tool package really is a tool package the sandbox accepts
    let (signed, _) = author::pack_dir(&root.join("my-wasm-tool"), None).unwrap();
    let p = crate::pkg::parse(&signed).unwrap();
    assert!(crate::wtool::parse_payload(&p.payload).is_ok());
    // an MCP package carries the program in its payload and asks to run exactly that program
    let (mcp, _) = author::pack_dir(&root.join("my-mcp-python"), None).unwrap();
    let pm = crate::pkg::parse(&mcp).unwrap();
    let v: serde_json::Value = serde_json::from_str(&pm.payload).unwrap();
    assert_eq!((v["command"].as_str(), v["args"][0].as_str()), (Some("python3"), Some("-c")));
    assert!(vet::vet_package(&mcp, &[]).facts.iter().any(|f| f == "permissions: run the program python3"));
    // folders must be new, names valid, kinds known, and nothing climbs out of the workspace
    assert!(author::scaffold("skill", "my-skill", &root, "my-skill").unwrap_err().contains("already has files"));
    assert!(author::scaffold("skill", "Bad Name", &root, "x").is_err() && author::scaffold("game", "ok", &root, "x").is_err());
    assert!(author::scaffold("skill", "ok-name", &root, "../outside").unwrap_err().contains("outside"));
    assert!(author::scaffold("skill", "ok-name", &root, "/etc/evil").is_err());
    assert!(!root.parent().unwrap().join("outside").exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn validation_finds_secrets_wide_permissions_bad_licences_and_missing_files() {
    let root = tmp("vet");
    author::scaffold("skill", "leaky", &root, "leaky").unwrap();
    let dir = root.join("leaky");
    assert!(vet::vet_dir(&dir).ok());
    let manifest = std::fs::read_to_string(dir.join("iyakki-app.json")).unwrap();
    let change = |from: &str, to: &str| std::fs::write(dir.join("iyakki-app.json"), manifest.replace(from, to)).unwrap();
    change("\"permissions\": []", "\"permissions\": [\"net:*\"]");
    assert!(vet::vet_dir(&dir).problems.iter().any(|p| p.contains("permission")));
    change("\"permissions\": []", "\"permissions\": [\"fly:moon\"]");
    assert!(!vet::vet_dir(&dir).ok());
    change("Apache-2.0", "GPL-3.0-only");
    assert!(vet::vet_dir(&dir).problems.iter().any(|p| p.contains("permissive")));
    change("skill.md", "nope.md");
    assert!(vet::vet_dir(&dir).problems.iter().any(|p| p.contains("entry")));
    std::fs::write(dir.join("iyakki-app.json"), &manifest).unwrap();
    std::fs::write(dir.join("config.py"), "API_TOKEN = \"abcd1234efgh5678ijkl9012\"\nkey = os.environ['K']\n").unwrap();
    let r = vet::vet_dir(&dir);
    assert!(r.problems.iter().any(|p| p.contains("config.py line 1") && p.contains("hard-coded secret")), "{:?}", r.problems);
    assert!(!r.problems.iter().any(|p| p.contains("abcd1234")), "the secret is never echoed");
    for leak in ["-----BEGIN RSA PRIVATE KEY-----", "token ghp_abcdefghijklmnopqrstuvwxyz0123456789", "AKIAABCDEFGHIJKLMNOP", "Authorization: Bearer abcdefghijklmnopqrstuvwxyz012345"] {
        assert!(!vet::secrets_scan(leak).is_empty(), "{leak}");
    }
    for fine in ["password = os.environ['PASSWORD']", "let token = process.env.TOKEN;", "api_key: <your key here>", "the token field is described here"] {
        assert!(vet::secrets_scan(fine).is_empty(), "{fine}");
    }
    assert!(vet::vet_dir(&root.join("nothing")).problems[0].contains("iyakki-app.json"));
    assert!(vet::vet_package("junk", &[]).problems[0].contains("not a valid package"));
    assert!(vet::permissive("MIT") && !vet::permissive("GPL-3.0-only") && !vet::permissive(""));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_authoring_tools_follow_the_rules_publish_always_asks_scaffold_stays_in_the_workspace() {
    let f = fx("tools");
    let ws = tmp("ws");
    let admin_p = iyakki_sec::policy::Role::Admin.template();
    let names: Vec<String> = authtools::defs(&admin_p).iter().map(|d| d.spec.name.clone()).collect();
    for n in ["app_scaffold", "store_validate", "store_publish", "store_server_status"] {
        assert!(names.contains(&n.to_string()) && authtools::is_author_tool(n), "{n}");
    }
    let risk = |n: &str| authtools::defs(&admin_p).into_iter().find(|d| d.spec.name == n).unwrap().risk;
    assert_eq!((risk("app_scaffold"), risk("store_publish"), risk("store_validate"), risk("store_server_status")), (Risk::Ask, Risk::Ask, Risk::Safe, Risk::Safe));
    assert!(hard_ask("store_publish", &json!({})).is_some() && hard_ask("app_scaffold", &json!({})).is_none() && hard_ask("store_validate", &json!({})).is_none());
    let mut nostore = admin_p.clone();
    nostore.apps.remove(&iyakki_sec::policy::App::Store);
    assert!(authtools::defs(&nostore).is_empty());
    let mut nowrite = admin_p.clone();
    nowrite.fs_write.clear();
    assert!(!authtools::defs(&nowrite).iter().any(|d| d.spec.name == "app_scaffold"));
    assert!(authtools::call("app_scaffold", &json!({"kind": "skill", "name": "x1"}), &f.svc, &nowrite, "root", &ws).is_err());
    // scaffold, validate and the secrets scan through the tools
    let call = |n: &str, a: serde_json::Value| authtools::call(n, &a, &f.svc, &admin_p, "root", &ws);
    assert!(call("app_scaffold", json!({"kind": "skill", "name": "demo-skill", "dir": "apps/demo"})).unwrap().contains("Created skill"));
    assert!(ws.join("apps/demo/skill.md").is_file());
    assert!(call("app_scaffold", json!({"kind": "skill", "name": "demo-skill", "dir": "../escape"})).unwrap_err().contains("outside"));
    assert!(call("app_scaffold", json!({"kind": "skill", "name": "demo-skill", "dir": "/tmp/escape-x"})).is_err());
    assert!(call("store_validate", json!({"path": "apps/demo"})).unwrap().contains("result: ok"));
    std::fs::write(ws.join("apps/demo/notes.txt"), "token ghp_abcdefghijklmnopqrstuvwxyz0123456789").unwrap();
    let v = call("store_validate", json!({"path": "apps/demo"})).unwrap();
    assert!(v.contains("PROBLEM") && v.contains("NOT ok") && !v.contains("ghp_abc"), "{v}");
    assert!(call("store_validate", json!({"path": "../../etc"})).is_err());
    // a signed package: validate shows the trust level; publish has no target until a private store is added
    std::fs::remove_file(ws.join("apps/demo/notes.txt")).unwrap();
    let key = new_signing_key().unwrap();
    let (pkgtext, _) = author::pack_dir(&ws.join("apps/demo"), Some(&key)).unwrap();
    std::fs::write(ws.join("demo.iyp"), &pkgtext).unwrap();
    let v = call("store_validate", json!({"path": "demo.iyp"})).unwrap();
    assert!(v.contains("trust level Community") && v.contains("result: ok"), "{v}");
    assert!(call("store_publish", json!({"package_path": "demo.iyp"})).unwrap_err().contains("Deploy the private store first (STORE \u{2192} Private store \u{2192} Deploy; one click now creates the registry)"));
    let d = authtools::describe(&f.svc, &ws, "store_publish", &json!({"package_path": "demo.iyp"}));
    assert!(d.starts_with("PUBLISH to") && d.contains("demo-skill 0.1.0") && d.contains("Community"), "{d}");
    assert!(call("store_server_status", json!({})).unwrap().contains("No private store"));
    // a store is added by the person; the token comes from the vault, so a locked or empty vault stops the publish
    let (srv, sd) = server("tools");
    let fetch = FakeFetch::default();
    fetch.put("http://127.0.0.1:9/catalog.json", &get(&srv, "/catalog.json"));
    let probe = f.store.probe_private(&fetch, "http://127.0.0.1:9").unwrap();
    let id = f.store.add_private_source(&admin(), &fetch, "Lab", "http://127.0.0.1:9", &probe.fingerprint).unwrap();
    let e = call("store_publish", json!({"package_path": "demo.iyp", "target": "Lab"})).unwrap_err();
    assert!(e.contains("Lab has no publish token on this device") && e.contains("one sentence"), "{e}");
    let _ = id;
    assert!(call("store_publish", json!({"package_path": "../x.iyp", "target": "Lab"})).is_err());
    assert!(authtools::describe(&f.svc, &ws, "store_publish", &json!({"package_path": "demo.iyp", "target": "Lab"})).contains("Lab (http://127.0.0.1:9)"));
    let _ = (std::fs::remove_dir_all(&ws), std::fs::remove_dir_all(&sd), std::fs::remove_dir_all(&f.dir));
}

#[test]
fn the_publish_client_talks_to_a_real_server_refuses_clear_text_to_the_internet_and_pkg_publish_reads_the_token_from_the_environment() {
    let (srv, d) = server("client");
    let tok = srv.create_publisher("acme", "Acme").unwrap();
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let (s2, st2) = (srv.clone(), stop.clone());
    let th = std::thread::spawn(move || super::server_http::serve(s2, l, st2));
    let base = format!("http://{addr}");
    let up = iyakki_proxy::https::UreqUpstream::default();
    let pkg = skill("from-client", "1.0.0");
    let p = author::publish_to_server(&up, &base, &tok, &pkg).unwrap();
    assert_eq!((p.name.as_str(), p.version.as_str(), p.trust.as_str()), ("from-client", "1.0.0", "Community"));
    assert!(author::publish_to_server(&up, &base, "iyk_wrong", &pkg).unwrap_err().contains("401"));
    assert!(author::publish_to_server(&up, &base, &tok, &pkg).unwrap_err().contains("versions only go up"));
    assert!(author::publish_to_server(&up, "http://store.example.com", &tok, &pkg).unwrap_err().contains("unencrypted"));
    assert!(author::publish_to_server(&up, "ftp://x", &tok, &pkg).is_err() && author::publish_to_server(&up, &base, " ", &pkg).is_err());
    // the command line: the token only from the environment
    let f = |s: &[&str]| super::publish::run_at(&s.iter().map(|x| x.to_string()).collect::<Vec<_>>(), 1_800_000_000);
    std::fs::write(d.join("next.iyp"), skill("from-client", "1.1.0")).unwrap();
    let file = d.join("next.iyp").display().to_string();
    assert!(f(&["publish", "--server", &base, "--token-env", "IYAKKI_TEST_NO_SUCH_TOKEN", &file]).unwrap_err().contains("not set"));
    std::env::set_var("IYAKKI_TEST_STORE_TOKEN", &tok);
    let out = f(&["publish", "--server", &base, "--token-env", "IYAKKI_TEST_STORE_TOKEN", &file]).unwrap();
    assert!(out.contains("published from-client 1.1.0"), "{out}");
    assert!(f(&["publish", "--server", &base, "--token", "x", &file]).is_err(), "there is no token flag");
    stop.store(true, Ordering::SeqCst);
    th.join().unwrap();
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_status_card_text_follows_the_state() {
    let f = fx("status");
    let none = privatestore::status_info(f.svc.dir(), "root", &Store::for_services(&f.svc));
    assert!(privatestore::status_text(&none).contains("No private store"));
    let mut rec = crate::platform::Installed { id: "private-store-iyakki-private-store".into(), service: "private-store".into(), title: "Private store".into(), kind: "store".into(), ..Default::default() };
    rec.urls = vec![crate::platform::UrlInfo { label: "Catalog".into(), url: "http://10.0.0.5:30300/catalog.json".into(), kind: "catalog".into() }];
    crate::platform::installed::upsert(f.svc.dir(), "root", rec).unwrap();
    let info = privatestore::status_info(f.svc.dir(), "root", &f.store);
    let t = privatestore::status_text(&info);
    assert!(t.contains("is deployed") && t.contains("not added"), "{t}");
    let mut c = f.store.config();
    let key = signing_key_hex(&new_signing_key().unwrap());
    let pin = crate::pkg::public_key_hex(&crate::pkg::signing_key_from_hex(&key).unwrap());
    let id = c.add_pinned("Mine", "http://10.0.0.5:30300/catalog.json", &pin).unwrap();
    c.publish_target = id.clone();
    c.save(f.store.dir()).unwrap();
    let t = privatestore::status_text(&privatestore::status_info(f.svc.dir(), "root", &f.store));
    assert!(t.contains("key pinned") && t.contains("by default"), "{t}");
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn the_store_status_text_says_deployed_added_and_the_default_target() {
    use privatestore::{status_text, StatusInfo};
    let deployed = StatusInfo { instance: Some(("ps1".into(), "http://10.0.0.5:8080/catalog.json".into())), ..Default::default() };
    let t = status_text(&deployed);
    assert!(t.contains("Private store ps1 is deployed") && t.contains("http://10.0.0.5:8080/catalog.json") && t.contains("not added"), "{t}");
    let added = StatusInfo { added: Some(("s9".into(), "Lab".into())), default_target: Some("Lab".into()), ..deployed };
    let t = status_text(&added);
    assert!(t.contains("added as the catalog Lab (s9), key pinned") && t.contains("Publishing goes to Lab by default") && !t.contains("not added"), "{t}");
    assert!(status_text(&StatusInfo::default()).starts_with("No private store is deployed"));
}
