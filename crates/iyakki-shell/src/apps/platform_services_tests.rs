// #[cfg(test)] -- this whole file is test code (platform_services.rs includes it under #[cfg(test)]); the marker keeps the BOUNDARY check from counting it.
//! Platform services: geometry and click paths at four sizes, the deploy wizard end to end with a fake cluster, the masked reveal, the
//! refusal of a production-looking cluster, the typed confirmation of a removal and connecting an existing Gitea. No cluster, no network.

use super::*;
use iyakki_agent::devcluster::{Choice, Env};
use iyakki_agent::platform::connect::FakeHttp;
use iyakki_agent::platform::{MemVault, VaultApi};
use iyakki_agent::wsexec::FakeRunner;
use std::path::PathBuf;
use std::time::Duration;

const SIZES: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (1024, 600), (800, 480)];
const NODES: &str = r#"{"items":[{"metadata":{"name":"n1"},"status":{"conditions":[{"type":"Ready","status":"True"}],"nodeInfo":{"architecture":"amd64"},"allocatable":{"pods":"110"}}}]}"#;
const SC: &str = r#"{"items":[{"metadata":{"name":"local-path","annotations":{"storageclass.kubernetes.io/is-default-class":"true"}}}]}"#;
const PLANTED: &str = "PANELSECRETVALUE77";
const TOKEN: &str = "abcdef0123456789abcdef0123456789abcdef01";

struct Rig {
    p: PlatformPanel,
    dir: PathBuf,
    vault: Arc<MemVault>,
    run: Arc<FakeRunner>,
    http: Arc<FakeHttp>,
}

fn answers(r: &FakeRunner, server: &str) {
    r.answer("config current-context", "default");
    r.answer("config view --minify", server);
    r.answer("version -o json", r#"{"serverVersion":{"gitVersion":"v1.30.0+k3s1"}}"#);
    r.answer("auth can-i", "Resources Non-Resource URLs Resource Names Verbs\npods [] [] [get]\n");
    r.answer("get nodes -o json", NODES);
    r.answer("get storageclass", SC);
    r.answer("get services -A", "default web NodePort 30300\n");
    r.answer("get deployments,statefulsets,daemonsets -A", "kube-system coredns rancher/coredns:1\n");
    r.answer("get pods -A", "pod/a\n");
    r.answer("get services -n", "");
    r.answer("get deployments,pods", r#"{"items":[{"kind":"Deployment","metadata":{"name":"gitea"},"spec":{"replicas":1,"template":{"spec":{"containers":[{"image":"docker.io/gitea/gitea:1.24.5-rootless"}]}}},"status":{"readyReplicas":1}}]}"#);
    r.fail("get namespace iyakki-gitea", 1, "Error from server (NotFound): namespaces not found");
    r.fail("get service registry", 1, "Error from server (NotFound): services \"registry\" not found");
    r.fail("get secret gitea-admin", 1, "Error from server (NotFound): secrets not found");
    for (k, v) in [("create namespace", "ok"), ("label namespace", "ok"), ("create secret generic", "ok"), ("label secret", "ok"), ("apply -f", "ok"), ("rollout status", "ok"), ("user create", "created"), ("generate-access-token", TOKEN)] {
        r.answer(k, v);
    }
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-psvc-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("ram")).unwrap();
    d
}

fn rig(tag: &str, server: &str) -> Rig {
    let dir = tmp(tag);
    let kc = dir.join("kubeconfig.yaml");
    std::fs::write(&kc, "apiVersion: v1\n").unwrap();
    Choice { kubeconfig: kc.display().to_string(), context: String::new() }.save(&dir, "alice").unwrap();
    let run = Arc::new(FakeRunner::new(&["kubectl", "curl"]));
    answers(&run, server);
    let http = Arc::new(FakeHttp::default());
    let vault = Arc::new(MemVault::default());
    let pl = Platform {
        run: run.clone(),
        dir: dir.clone(),
        user: "alice".into(),
        vault: vault.clone(),
        audit: Arc::new(|_, _| {}),
        env: Env { home: dir.clone(), ..Default::default() },
        tmp: dir.join("ram"),
        http: http.clone(),
        gen: Arc::new(|n| format!("{:A<n$}", PLANTED)),
        live: None,
    };
    Rig { p: PlatformPanel::new(Arc::new(pl)), dir, vault, run, http }
}

fn draw(p: &mut PlatformPanel, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    p.draw(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s);
    g
}

fn until(p: &mut PlatformPanel, what: &str, mut f: impl FnMut(&mut PlatformPanel) -> bool) {
    for _ in 0..3000 {
        p.handle(&Event::Tick);
        if f(p) && !p.is_busy() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("never happened: {what}; page {:?}, message {:?}, text {}", p.page(), p.message(), p.text());
}

fn click(p: &mut PlatformPanel, w: u32, h: u32, label: &str) {
    let g = draw(p, w, h);
    let hit = g.hits.iter().rev().find(|x| x.label == label || x.label.starts_with(label)).unwrap_or_else(|| panic!("{w}x{h}: no `{label}` in {:?}", g.hits.iter().map(|x| x.label.clone()).collect::<Vec<_>>()));
    let ev = hit.ev.clone();
    p.handle(&ev);
}

fn inside(g: &Gfx, w: u32, h: u32, what: &str) {
    for hit in &g.hits {
        let r = hit.rect;
        assert!(r.x >= -1.0 && r.y >= -1.0 && r.x + r.w <= w as f32 + 1.0 && r.y + r.h <= h as f32 + 1.0, "{what} at {w}x{h}: {} sticks out {r:?}", hit.label);
    }
}

fn buttons_apart(g: &Gfx, w: u32, h: u32, what: &str) {
    let rs: Vec<(String, R)> = g.hits.iter().filter(|x| matches!(x.ev, Event::Key(_))).map(|x| (x.label.clone(), x.rect)).collect();
    for (i, (la, a)) in rs.iter().enumerate() {
        for (lb, b) in &rs[i + 1..] {
            let apart = a.y + a.h <= b.y + 0.5 || b.y + b.h <= a.y + 0.5 || a.x + a.w <= b.x + 0.5 || b.x + b.w <= a.x + 0.5;
            assert!(apart, "{what} at {w}x{h}: `{la}` {a:?} overlaps `{lb}` {b:?}");
        }
    }
}

fn type_text(p: &mut PlatformPanel, t: &str) {
    for c in t.chars() {
        p.handle(&Event::Key(Key::Char(c)));
    }
}

#[test]
fn the_wizard_pages_fit_and_do_not_overlap_at_every_size_and_the_click_path_deploys_gitea() {
    for (w, h) in SIZES {
        let mut r = rig(&format!("wiz-{w}"), "https://127.0.0.1:6443");
        r.http.answer("GET http://localhost:30301/api/healthz", 200, "{}");
        r.p.open_list();
        until(&mut r.p, "the list", |p| p.page() == &Page::List && p.rows.len() >= 3 && !p.is_busy());
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "list");
        buttons_apart(&g, w, h, "list");
        // the first row is Gitea: Deploy to k3s…
        click(&mut r.p, w, h, "Deploy to k3s");
        assert_eq!(r.p.page(), &Page::Cluster);
        until(&mut r.p, "the clusters", |p| p.wiz.as_ref().is_some_and(|w| !w.loading));
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "clusters");
        buttons_apart(&g, w, h, "clusters");
        click(&mut r.p, w, h, "Next");
        assert_eq!(r.p.page(), &Page::Options, "{}", r.p.message());
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "options");
        buttons_apart(&g, w, h, "options");
        // the keyboard walks the form; a bad value stays on the page and says why
        let ns = r.p.wiz.as_ref().unwrap().fields.iter().position(|f| f.def.key == "namespace").unwrap();
        r.p.handle(&Event::Row(ns));
        for _ in 0..30 {
            r.p.handle(&Event::Key(Key::Backspace));
        }
        type_text(&mut r.p, "kube-system");
        click(&mut r.p, w, h, "Review the plan");
        assert_eq!(r.p.page(), &Page::Options, "a protected namespace stays on the form");
        assert!(r.p.message().contains("protected"), "{}", r.p.message());
        for _ in 0..30 {
            r.p.handle(&Event::Key(Key::Backspace));
        }
        type_text(&mut r.p, "iyakki-gitea");
        click(&mut r.p, w, h, "Review the plan");
        assert_eq!(r.p.page(), &Page::Review);
        until(&mut r.p, "the plan", |p| p.text().contains("DEPLOY Gitea"));
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "review");
        buttons_apart(&g, w, h, "review");
        let text = r.p.text();
        for want in ["create namespace iyakki-gitea", "MANIFEST", "uvault:svc.gitea-iyakki-gitea.admin-password", "docker.io/gitea/gitea:1.24.5-rootless", "https://127.0.0.1:6443"] {
            assert!(text.contains(want), "{want}\n{text}");
        }
        assert!(!text.contains(PLANTED));
        // nothing ran yet
        assert!(r.run.lines().iter().all(|l| !l.contains("create namespace iyakki") && !l.contains("apply -f")), "{:?}", r.run.lines());
        click(&mut r.p, w, h, "Approve and deploy");
        until(&mut r.p, "the result", |p| p.page() == &Page::Result);
        assert!(r.p.text().contains("DEPLOYED") && r.p.text().contains("http://localhost:30301"), "{}", r.p.text());
        assert!(!r.p.text().contains(PLANTED));
        // the masked reveal
        assert!(r.p.reveal_open());
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "reveal");
        buttons_apart(&g, w, h, "reveal");
        assert!(!r.p.reveal[0].shown);
        click(&mut r.p, w, h, "Show");
        assert!(r.p.reveal[0].shown);
        click(&mut r.p, w, h, "Copy");
        assert!(clipboard::history().iter().any(|t| t.starts_with(PLANTED)));
        click(&mut r.p, w, h, "Done");
        assert!(!r.p.reveal_open());
        assert!(!clipboard::history().iter().any(|t| t.starts_with(PLANTED)), "Done takes the copy out of the history again");
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "result");
        click(&mut r.p, w, h, "Close");
        until(&mut r.p, "back on the list", |p| p.page() == &Page::List && p.rows.iter().any(|x| matches!(&x.kind, RowKind::Inst(_))));
        assert!(r.vault.has("svc.gitea-iyakki-gitea.token"));
        assert!(installed::load(&r.dir, "alice").len() == 1);
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "list with an instance");
        buttons_apart(&g, w, h, "list with an instance");
    }
}

#[test]
fn a_production_looking_cluster_is_listed_with_its_reason_and_cannot_be_chosen() {
    let mut r = rig("prod", "https://prod.example.com:6443");
    r.p.open_wizard("gitea");
    until(&mut r.p, "the clusters", |p| p.wiz.as_ref().is_some_and(|w| !w.loading));
    for (w, h) in SIZES {
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "refused");
    }
    r.p.handle(&Event::Key(Key::Enter));
    assert_eq!(r.p.page(), &Page::Cluster);
    assert!(r.p.message().contains("looks like production"), "{}", r.p.message());
    assert!(r.run.lines().iter().all(|l| !l.contains("create namespace iyakki") && !l.contains("apply -f")));
    // Esc goes back to the list, not out of the panel
    r.p.handle(&Event::Key(Key::Esc));
    assert_eq!(r.p.page(), &Page::List);
    assert!(r.p.is_open());
    r.p.handle(&Event::Key(Key::Esc));
    assert!(!r.p.is_open());
}

#[test]
fn removing_needs_the_typed_id_and_connecting_an_existing_gitea_keeps_the_token_in_the_vault_only() {
    let mut r = rig("remove", "https://127.0.0.1:6443");
    // an existing Gitea somewhere
    r.http.answer("GET http://192.168.1.10:3000/api/v1/version", 200, r#"{"version":"1.22.3"}"#);
    r.http.answer("GET http://192.168.1.10:3000/api/v1/user", 200, r#"{"login":"casey"}"#);
    r.p.open_list();
    until(&mut r.p, "the list", |p| p.rows.len() >= 3);
    for (w, h) in SIZES {
        click(&mut r.p, w, h, "Connect to existing");
        assert_eq!(r.p.page(), &Page::Existing);
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "existing");
        buttons_apart(&g, w, h, "existing");
        r.p.handle(&Event::Key(Key::Esc));
        assert_eq!(r.p.page(), &Page::List);
    }
    r.p.handle(&Event::Key(Key::Char('c')));
    type_text(&mut r.p, "http://192.168.1.10:3000");
    r.p.handle(&Event::Key(Key::Tab));
    type_text(&mut r.p, TOKEN);
    assert!(r.p.token.mask, "the token field is masked");
    r.p.handle(&Event::Key(Key::Ctrl('s')));
    until(&mut r.p, "connected", |p| p.page() == &Page::List && p.rows.iter().any(|x| matches!(&x.kind, RowKind::Inst(_))));
    let inst = installed::load(&r.dir, "alice");
    assert_eq!(inst.len(), 1);
    assert!(inst[0].connected && r.vault.has(&inst[0].vault[0]));
    assert!(!std::fs::read_to_string(r.dir.join("installed-services-alice.json")).unwrap().contains(TOKEN));
    assert!(r.p.token.value.is_empty(), "the token is wiped from the form");
    // details, then remove with a wrong and the right id
    let i = r.p.rows.iter().position(|x| matches!(&x.kind, RowKind::Inst(_))).unwrap();
    r.p.handle(&Event::Row(i));
    for (w, h) in SIZES {
        r.p.handle(&Event::Key(Key::Enter));
        assert_eq!(r.p.page(), &Page::Confirm);
        assert!(r.p.text().contains("http://192.168.1.10:3000") && r.p.text().contains("connected as casey"), "{}", r.p.text());
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "details");
        buttons_apart(&g, w, h, "details");
        r.p.handle(&Event::Key(Key::Esc));
    }
    r.p.handle(&Event::Key(Key::Char('x')));
    assert_eq!(r.p.page(), &Page::Confirm);
    assert!(r.p.text().contains("FORGET the connection"));
    for (w, h) in SIZES {
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "remove");
        buttons_apart(&g, w, h, "remove");
    }
    type_text(&mut r.p, "wrong");
    r.p.handle(&Event::Key(Key::Enter));
    until(&mut r.p, "refused", |p| p.text().contains("NOT DONE"));
    assert!(r.p.text().contains("TYPE the service id"), "{}", r.p.text());
    assert_eq!(installed::load(&r.dir, "alice").len(), 1, "nothing was removed");
}

#[test]
fn the_registered_private_store_opens_the_same_wizard_and_an_unknown_service_says_so() {
    let mut r = rig("hook", "https://127.0.0.1:6443");
    r.p.open_wizard("no-such-service");
    assert_eq!(r.p.page(), &Page::List);
    assert!(r.p.message().contains("no platform service"));
    r.p.open_wizard("gitea");
    assert_eq!(r.p.page(), &Page::Cluster);
    assert_eq!(r.p.wiz.as_ref().unwrap().service, "gitea");
}

#[test]
fn connecting_a_workspace_reviews_the_repository_then_asks_again_before_the_push_and_the_token_never_shows() {
    let mut r = rig("ws", "https://127.0.0.1:6443");
    r.http.answer("GET http://localhost:30300/api/v1/version", 200, r#"{"version":"1.24.5"}"#);
    r.http.answer("GET http://localhost:30300/api/v1/user", 200, r#"{"login":"casey"}"#);
    r.http.answer("POST http://localhost:30300/api/v1/user/repos", 201, r#"{"clone_url":"http://localhost:30300/casey/shop.git","html_url":"http://localhost:30300/casey/shop"}"#);
    iyakki_agent::platform::gitea::connect_existing(&r.p.pl, "http://localhost:30300", TOKEN, None, "", true).unwrap();
    r.run.answer("rev-parse --is-inside-work-tree", "true");
    r.run.answer("symbolic-ref --short HEAD", "main");
    r.run.answer("rev-list --count HEAD", "3");
    r.run.answer("remote get-url origin", "http://localhost:30300/casey/shop.git");
    r.run.answer("push --set-upstream", "ok");
    let ws = WsInfo { name: "Shop".into(), root: r.dir.join("ws") };
    std::fs::create_dir_all(&ws.root).unwrap();
    r.p.open_connect_workspace(ws);
    until(&mut r.p, "the connect review", |p| p.page() == &Page::Confirm && p.text().contains("CONNECT the workspace Shop"));
    for (w, h) in SIZES {
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "connect review");
        buttons_apart(&g, w, h, "connect review");
    }
    assert!(r.http.lines().iter().all(|l| !l.starts_with("POST")), "nothing is created before the approval");
    r.p.handle(&Event::Key(Key::Char('y')));
    until(&mut r.p, "the push review", |p| p.page() == &Page::Confirm && p.text().contains("PUSH the branch main"));
    assert!(r.http.lines().iter().any(|l| l.starts_with("POST") && l.contains("/user/repos")), "the repository was created after the approval");
    assert!(r.run.lines().iter().all(|l| !l.contains(" push ")), "the push is its own approval");
    assert!(!r.p.text().contains(TOKEN));
    r.p.handle(&Event::Key(Key::Char('y')));
    until(&mut r.p, "pushed", |p| p.text().contains("pushed main"));
    assert!(r.run.calls().iter().all(|c| !c.argv.join(" ").contains(TOKEN)));
}

// ------------------------------------------------------------------------------------------------ a program the image is built from

const ARM_NODES: &str = r#"{"items":[{"metadata":{"name":"n1"},"status":{"conditions":[{"type":"Ready","status":"True"}],"nodeInfo":{"architecture":"arm64"},"allocatable":{"pods":"110"}}}]}"#;

/// A tiny ELF header the reader calls static for `machine` (62 = x86_64, 183 = aarch64); `interp` makes it dynamic.
fn elf_bytes(machine: u16, interp: bool) -> Vec<u8> {
    let nph = if interp { 1 } else { 0 };
    let mut b = vec![0u8; 64 + 56 * nph];
    b[..4].copy_from_slice(b"\x7fELF");
    b[4] = 2;
    b[5] = 1;
    b[18..20].copy_from_slice(&machine.to_le_bytes());
    if interp {
        b[32..40].copy_from_slice(&64u64.to_le_bytes());
        b[54..56].copy_from_slice(&56u16.to_le_bytes());
        b[56..58].copy_from_slice(&1u16.to_le_bytes());
        b[64..68].copy_from_slice(&3u32.to_le_bytes());
    }
    b
}

fn host_machine() -> u16 {
    if Arch::host() == Arch::X86_64 {
        62
    } else {
        183
    }
}

struct SRig {
    p: PlatformPanel,
    cx: crate::apps::AppCx,
    dir: PathBuf,
    run: Arc<FakeRunner>,
    /// The pretend source tree.
    src: PathBuf,
    /// A static executable for this machine's CPU, to give a fake build or a chosen file.
    good: PathBuf,
}

impl Drop for SRig {
    fn drop(&mut self) {
        for j in self.cx.services.jobs.list("alice") {
            let _ = self.cx.services.jobs.stop("alice", &j.id, Duration::from_millis(300));
        }
    }
}

/// A private-store deploy against a fake cluster whose nodes are `arch`, with no server program anywhere and a pretend source tree.
fn store_rig(tag: &str, nodes: &str, build: Option<Vec<String>>) -> SRig {
    store_rig_with(tag, nodes, build, false)
}

/// Answers the registry questions from a file: `<dir>/registry-ready` exists once a (fake) registry job has "created" it.
struct Switch {
    inner: Arc<FakeRunner>,
    flag: PathBuf,
}

impl iyakki_agent::wsexec::Runner for Switch {
    fn run(&self, cwd: &std::path::Path, argv: &[String], env: &[(String, String)], t: Duration, cap: usize, sink: Option<iyakki_agent::wsexec::Sink>) -> iyakki_agent::wsexec::Out {
        let line = argv.join(" ");
        let up = self.flag.exists();
        let out = |code: i32, so: &str, se: &str| iyakki_agent::wsexec::Out { code: Some(code), stdout: so.into(), stderr: se.into(), ..Default::default() };
        if line.contains("get service registry") {
            self.inner.run(cwd, argv, env, t, cap, sink);
            return if up { out(0, r#"{"spec":{"ports":[{"nodePort":30500}]}}"#, "") } else { out(1, "", "Error from server (NotFound): services \"registry\" not found") };
        }
        if line.contains("get deployment registry") {
            return out(0, r#"{"status":{"readyReplicas":1}}"#, "");
        }
        if line.contains("delete namespace iyakki-registry") {
            self.inner.run(cwd, argv, env, t, cap, sink);
            let _ = std::fs::remove_file(&self.flag);
            return out(0, "namespace deleted", "");
        }
        self.inner.run(cwd, argv, env, t, cap, sink)
    }
    fn has(&self, p: &str) -> bool {
        self.inner.has(p)
    }
}

/// `no_registry`: the cluster has no registry until the (fake) registry job runs.
fn store_rig_with(tag: &str, nodes: &str, build: Option<Vec<String>>, no_registry: bool) -> SRig {
    let dir = tmp(tag);
    let kc = dir.join("kubeconfig.yaml");
    std::fs::write(&kc, "apiVersion: v1\n").unwrap();
    Choice { kubeconfig: kc.display().to_string(), context: String::new() }.save(&dir, "alice").unwrap();
    let run = Arc::new(FakeRunner::new(&["kubectl", "curl", "podman", "cargo", "rustup", "musl-gcc", "bash"]));
    run.answer("get nodes -o json", nodes);
    if !no_registry {
        run.answer("get service registry", r#"{"spec":{"ports":[{"nodePort":30500}]}}"#);
    }
    run.fail("get namespace iyakki-private-store", 1, "Error from server (NotFound): namespaces not found");
    run.fail("get secret private-store", 1, "Error from server (NotFound): secrets not found");
    run.answer("rustup target list --installed", &format!("{}\n", Arch::host().triple()));
    run.answer("podman", "ok");
    answers(&run, "https://127.0.0.1:6443");
    let src = dir.join("src");
    std::fs::create_dir_all(src.join("crates/iyakki-store-server")).unwrap();
    std::fs::write(src.join("Cargo.toml"), "[workspace]\n").unwrap();
    std::fs::write(src.join("crates/iyakki-store-server/Cargo.toml"), "[package]\n").unwrap();
    std::fs::create_dir_all(dir.join("store-server")).unwrap();
    std::fs::write(dir.join("store-server/source"), src.display().to_string()).unwrap();
    let good = dir.join("good.elf");
    std::fs::write(&good, elf_bytes(host_machine(), false)).unwrap();
    let (cx, _d) = crate::apps::tests::cx_with_backend(&format!("sr-{tag}"), iyakki_agent::chat::Unconfigured);
    let runner: Arc<dyn iyakki_agent::wsexec::Runner> = if no_registry { Arc::new(Switch { inner: run.clone(), flag: dir.join("registry-ready") }) } else { run.clone() };
    let pl = Platform {
        run: runner,
        dir: dir.clone(),
        user: "alice".into(),
        vault: Arc::new(MemVault::default()),
        audit: Arc::new(|_, _| {}),
        env: Env { home: dir.clone(), ..Default::default() },
        tmp: dir.join("ram"),
        http: Arc::new(FakeHttp::default()),
        gen: Arc::new(|n| format!("{:A<n$}", PLANTED)),
        live: None,
    };
    let mut p = PlatformPanel::new(Arc::new(pl)).with_services(cx.services.clone());
    if let Some(b) = build {
        p = p.with_build_command(b);
    }
    SRig { p, cx, dir, run, src, good }
}

/// The fake build: prints a few lines and "builds" a static executable where cargo would put it.
fn fake_build(r: &SRig) -> Vec<String> {
    let out = r.src.join("target").join(Arch::host().triple()).join("release");
    vec!["sh".into(), "-c".into(), format!("echo compiling; while [ ! -f {go} ]; do sleep 0.1; done; mkdir -p {o} && cp {g} {o}/iyakki-store-server; echo finished", o = out.display(), g = r.good.display(), go = r.dir.join("go").display())]
}

/// Wizard to the result of the first approval (which stops for want of the program).
fn store_to_card(r: &mut SRig) {
    r.p.open_wizard("private-store");
    until(&mut r.p, "the clusters", |p| p.wiz.as_ref().is_some_and(|w| !w.loading));
    click(&mut r.p, 1280, 720, "Next");
    until(&mut r.p, "the options (the store is looked at first)", |p| p.page() == &Page::Options);
    click(&mut r.p, 1280, 720, "Review the plan");
    until(&mut r.p, "the plan", |p| p.text().contains("DEPLOY Private store"));
    assert!(r.p.text().contains("There is no static iyakki-store-server"), "the review warns before approving:\n{}", r.p.text());
    click(&mut r.p, 1280, 720, "Approve and deploy");
    until(&mut r.p, "the card", |p| p.page() == &Page::Result);
}

fn nothing_changed(r: &SRig) {
    let l = r.run.lines();
    assert!(l.iter().all(|x| !x.contains("create namespace iyakki-private-store") && !x.contains("apply -f") && !x.contains("create secret generic") && !x.contains("build -t")), "nothing was created: {l:?}");
}

#[test]
fn a_missing_server_program_is_a_card_with_the_fixes_at_hand_at_every_size_and_nothing_was_changed() {
    let mut r = store_rig("card", NODES, None);
    store_to_card(&mut r);
    let t = r.p.text();
    assert!(t.contains("CANNOT DEPLOY: No static iyakki-store-server for x86_64 was found") && t.contains("Nothing further was done"), "{t}");
    assert!(t.contains("dist/store-server/Dockerfile") && t.contains("release page"), "{t}");
    nothing_changed(&r);
    for (w, h) in SIZES {
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "card");
        buttons_apart(&g, w, h, "card");
        for label in ["Choose a binary file", "Open docs", "Try again", "Close"] {
            assert!(g.hits.iter().any(|x| x.label.starts_with(label)), "{w}x{h}: no `{label}`");
        }
        assert!(g.hits.iter().any(|x| x.label == "Build now"), "{w}x{h}: Build now is offered because the tree, cargo, the target and a linker exist");
    }
    // the docs, then back
    click(&mut r.p, 1024, 600, "Open docs");
    assert!(r.p.text().contains("THE SERVER PROGRAM") && r.p.text().contains("Choose a binary file"), "{}", r.p.text());
    for (w, h) in SIZES {
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "docs");
        buttons_apart(&g, w, h, "docs");
    }
    click(&mut r.p, 1024, 600, "Back to the result");
    assert!(r.p.text().contains("CANNOT DEPLOY"));
    // Close leaves the wizard
    click(&mut r.p, 1280, 720, "Close");
    assert_eq!(r.p.page(), &Page::List);
}

#[test]
fn the_docs_section_the_card_opens_is_found_in_store_md() {
    let t = server_program_docs();
    assert!(t.contains("13.1 The server program") && t.contains("ELF headers") && t.contains("Build now") && !t.contains("## 14."), "{t}");
}

#[test]
fn choosing_a_file_checks_the_headers_refuses_dynamic_wrong_cpu_and_junk_and_a_good_file_continues_the_deploy() {
    let mut r = store_rig("choose", NODES, None);
    if Arch::host() != Arch::X86_64 {
        return; // the fake cluster is x86_64 and the good file is this machine's; the wrong-CPU part is checked in localbin's tests
    }
    store_to_card(&mut r);
    click(&mut r.p, 1280, 720, "Choose a binary file");
    assert_eq!(r.p.page(), &Page::Choose);
    for (w, h) in SIZES {
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "choose");
        buttons_apart(&g, w, h, "choose");
    }
    let marker = r.dir.join("ran");
    let script = r.dir.join("evil.sh");
    std::fs::write(&script, format!("#!/bin/sh\ntouch {}\n", marker.display())).unwrap();
    let dynamic = r.dir.join("dyn.elf");
    std::fs::write(&dynamic, elf_bytes(62, true)).unwrap();
    let arm = r.dir.join("arm.elf");
    std::fs::write(&arm, elf_bytes(183, false)).unwrap();
    let try_file = |r: &mut SRig, p: &std::path::Path, want: &str| {
        r.p.choose.set("");
        type_text(&mut r.p, &p.display().to_string());
        click(&mut r.p, 1280, 720, "Use this file");
        assert_eq!(r.p.page(), &Page::Choose, "stays on the page");
        assert!(r.p.choose_err.contains(want), "{}", r.p.choose_err);
        let g = draw(&mut r.p, 1024, 600);
        inside(&g, 1024, 600, "choose error");
    };
    try_file(&mut r, &script, "no ELF header");
    try_file(&mut r, &dynamic, "dynamically linked");
    try_file(&mut r, &arm, "built for aarch64 but the cluster is x86_64");
    let missing = r.dir.join("missing");
    try_file(&mut r, &missing, "cannot be read");
    assert!(!marker.exists(), "a chosen file is never run");
    nothing_changed(&r);
    // a good one: kept in the data folder, and the deploy goes on by itself
    r.p.choose.set("");
    let good = r.good.clone();
    type_text(&mut r.p, &good.display().to_string());
    click(&mut r.p, 1280, 720, "Use this file");
    until(&mut r.p, "deployed", |p| p.page() == &Page::Result && p.text().contains("DEPLOYED"));
    assert!(r.dir.join("store-server/iyakki-store-server-x86_64").is_file());
    let lines = r.run.lines();
    let pos = |n: &str| lines.iter().position(|l| l.contains(n)).unwrap_or_else(|| panic!("{n} in {lines:?}"));
    assert!(pos("create namespace iyakki-private-store") < pos("podman build -t localhost/iyakki-store-server:1.0.0 --platform linux/amd64"));
    assert!(lines.iter().any(|l| l.contains("podman push")) && lines.iter().any(|l| l.contains("apply -f")));
}

#[test]
fn build_now_asks_first_runs_as_a_job_with_a_bar_and_the_deploy_continues_when_it_succeeds() {
    let mut r = store_rig("build", NODES, None);
    r.p.build_override = Some(fake_build(&r));
    store_to_card(&mut r);
    click(&mut r.p, 1280, 720, "Build now");
    assert_eq!(r.p.page(), &Page::Confirm);
    let t = r.p.text();
    assert!(t.contains("BUILD iyakki-store-server") && t.contains("COMMAND") && t.contains("compiling") && t.contains("WRITES") && t.contains("background job"), "{t}");
    for (w, h) in SIZES {
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "build review");
        buttons_apart(&g, w, h, "build review");
    }
    assert!(r.cx.services.jobs.list("alice").is_empty(), "nothing started before the approval");
    // cancelling returns to the card
    r.p.handle(&Event::Key(Key::Esc));
    assert_eq!(r.p.page(), &Page::Result);
    click(&mut r.p, 1280, 720, "Build now");
    click(&mut r.p, 1280, 720, "Approve");
    assert_eq!(r.p.page(), &Page::Progress, "{}", r.p.message());
    assert_eq!(r.cx.services.jobs.list("alice").len(), 1);
    assert_eq!(r.cx.services.jobs.list("alice")[0].label, "Build iyakki-store-server");
    // while it runs: a bar that moves, a Stop button, and nothing deployed yet
    for (w, h) in SIZES {
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "building");
        buttons_apart(&g, w, h, "building");
        assert!(g.hits.iter().any(|x| x.label.starts_with("Stop the build")));
    }
    let f0 = draw(&mut r.p, 1280, 720).rgba().to_vec();
    for _ in 0..10 {
        r.p.handle(&Event::Tick);
    }
    let f1 = draw(&mut r.p, 1280, 720).rgba().to_vec();
    assert_ne!(f0, f1, "the bar moves between frames");
    nothing_changed(&r);
    std::fs::write(r.dir.join("go"), "go").unwrap(); // let the fake build finish
    until(&mut r.p, "deployed after the build", |p| p.page() == &Page::Result && p.text().contains("DEPLOYED"));
    let j = &r.cx.services.jobs.list("alice")[0];
    assert_eq!(j.state, iyakki_agent::jobs::State::Succeeded);
    let lines = r.run.lines();
    assert!(lines.iter().any(|l| l.contains("create namespace iyakki-private-store")) && lines.iter().any(|l| l.contains("podman build -t localhost/iyakki-store-server:1.0.0")), "{lines:?}");
    assert!(r.src.join("target").join(Arch::host().triple()).join("release/iyakki-store-server").is_file());
}

#[test]
fn a_failed_build_stops_with_the_reason_and_the_last_lines_and_nothing_was_changed() {
    let mut r = store_rig("buildfail", NODES, Some(vec!["sh".into(), "-c".into(), "echo compiling; echo 'error: boom-line-7' >&2; exit 3".into()]));
    store_to_card(&mut r);
    click(&mut r.p, 1280, 720, "Build now");
    click(&mut r.p, 1280, 720, "Approve");
    until(&mut r.p, "the failure", |p| p.page() == &Page::Result && p.text().contains("CANNOT DEPLOY"));
    let t = r.p.text();
    assert!(t.contains("failed (exit 3)") && t.contains("boom-line-7") && t.contains("Nothing further was done"), "{t}");
    nothing_changed(&r);
    for (w, h) in SIZES {
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "build failed");
        buttons_apart(&g, w, h, "build failed");
        assert!(g.hits.iter().any(|x| x.label == "Build now"), "the build can be tried again");
    }
}

#[test]
fn a_static_file_for_the_other_cpu_is_called_out_and_the_cluster_cpu_decides() {
    let other = if Arch::host() == Arch::X86_64 { Arch::Aarch64 } else { Arch::X86_64 };
    let mut r = store_rig("arch", if other == Arch::Aarch64 { ARM_NODES } else { NODES }, None);
    // a good static file for this machine's CPU is in the cache, but the cluster is the other CPU
    std::fs::copy(&r.good, r.dir.join("store-server/iyakki-store-server-x86_64")).ok();
    std::fs::write(r.dir.join(format!("store-server/iyakki-store-server-{}", other.uname())), elf_bytes(host_machine(), false)).unwrap();
    store_to_card(&mut r);
    let t = r.p.text();
    assert!(t.contains(&format!("is a static {} build, but this cluster needs {}", Arch::host().uname(), other.uname())) || t.contains(&format!("built for {} but the cluster is {}", Arch::host().uname(), other.uname())), "{t}");
    nothing_changed(&r);
}

// ------------------------------------------------------------------ the local image registry from the deploy screen

/// A launcher script standing in for `iyakki quick-install registry`: it prints like the real job and, when it works, makes the
/// fake cluster answer that the registry exists.
fn registry_launch(r: &SRig, ok: bool) -> iyakki_agent::quickinstall::Launch {
    use std::os::unix::fs::PermissionsExt;
    let exe = r.dir.join("fake-iyakki.sh");
    let body = if ok {
        format!("#!/bin/sh\necho '[1/6] checking the cluster'\necho \"args: $*\"\ntouch {}\necho '[6/6] done: registry ready'\n", r.dir.join("registry-ready").display())
    } else {
        "#!/bin/sh\necho '[1/6] checking the cluster'\necho 'error: The cluster does not answer (connection refused)'\nexit 1\n".to_string()
    };
    std::fs::write(&exe, body).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    iyakki_agent::quickinstall::Launch { exe, data: r.dir.join("qi") }
}

fn store_to_review_without_registry(r: &mut SRig) {
    r.p.open_wizard("private-store");
    until(&mut r.p, "the clusters", |p| p.wiz.as_ref().is_some_and(|w| !w.loading));
    click(&mut r.p, 1280, 720, "Next");
    until(&mut r.p, "the options (the store is looked at first)", |p| p.page() == &Page::Options);
    click(&mut r.p, 1280, 720, "Review the plan");
    until(&mut r.p, "the plan", |p| p.text().contains("DEPLOY Private store"));
}

#[test]
fn without_a_registry_the_review_says_why_and_one_click_creates_it_then_the_deploy_continues() {
    let mut r = store_rig_with("regflow", NODES, None, true);
    std::fs::copy(&r.good, r.dir.join("store-server/iyakki")).unwrap();
    let l = registry_launch(&r, true);
    r.p = std::mem::replace(&mut r.p, PlatformPanel::new(Arc::new(Platform { run: r.run.clone(), dir: r.dir.clone(), user: "alice".into(), vault: Arc::new(MemVault::default()), audit: Arc::new(|_, _| {}), env: Env::default(), tmp: r.dir.join("ram"), http: Arc::new(FakeHttp::default()), gen: Arc::new(|n| "x".repeat(n)), live: None }))).with_registry_launch(l);
    store_to_review_without_registry(&mut r);
    let t = r.p.text();
    assert!(t.contains("built on this device") && t.contains("Docker Hub") && t.contains("Create registry"), "{t}");
    for (w, h) in SIZES {
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "review without registry");
        buttons_apart(&g, w, h, "review without registry");
        assert!(g.hits.iter().any(|x| x.label.starts_with("Create registry")), "{w}x{h}");
        assert!(!g.hits.iter().any(|x| x.label.starts_with("Approve and deploy")));
    }
    nothing_changed(&r);
    click(&mut r.p, 1280, 720, "Create registry");
    assert_eq!(r.p.page(), &Page::Confirm);
    let t = r.p.text();
    assert!(t.contains("NOT restarted") && t.contains("registry:2") && t.contains("10 Gi") && t.contains("continues by itself"), "{t}");
    r.p.handle(&Event::Key(Key::Ctrl('k')));
    assert!(r.p.text().contains("20 Gi"));
    assert!(r.cx.services.jobs.list("alice").is_empty(), "nothing started before the approval");
    click(&mut r.p, 1280, 720, "Approve");
    assert_eq!(r.p.page(), &Page::Progress, "{}", r.p.message());
    let jobs = r.cx.services.jobs.list("alice");
    assert_eq!(jobs.len(), 1);
    assert!(jobs[0].label.starts_with("Install Local image registry") && jobs[0].workspace == iyakki_agent::quickinstall::WORKSPACE, "{:?}", jobs[0]);
    assert!(jobs[0].command.contains("--size 20") && jobs[0].command.contains("quick-install registry"), "{}", jobs[0].command);
    until(&mut r.p, "deployed after the registry", |p| p.page() == &Page::Result && p.text().contains("DEPLOYED"));
    let lines = r.run.lines();
    assert!(lines.iter().any(|l| l.contains("create namespace iyakki-private-store")) && lines.iter().any(|l| l.contains("podman push")), "{lines:?}");
}

#[test]
fn a_failed_registry_job_says_why_and_offers_retry() {
    let mut r = store_rig_with("regfail", NODES, None, true);
    let l = registry_launch(&r, false);
    r.p = std::mem::replace(&mut r.p, PlatformPanel::new(Arc::new(Platform { run: r.run.clone(), dir: r.dir.clone(), user: "alice".into(), vault: Arc::new(MemVault::default()), audit: Arc::new(|_, _| {}), env: Env::default(), tmp: r.dir.join("ram"), http: Arc::new(FakeHttp::default()), gen: Arc::new(|n| "x".repeat(n)), live: None }))).with_registry_launch(l);
    store_to_review_without_registry(&mut r);
    click(&mut r.p, 1280, 720, "Create registry");
    click(&mut r.p, 1280, 720, "Approve");
    until(&mut r.p, "the failure", |p| p.page() == &Page::Result);
    let t = r.p.text();
    assert!(t.contains("THE REGISTRY WAS NOT CREATED: The cluster does not answer") && t.contains("Retry"), "{t}");
    nothing_changed(&r);
    for (w, h) in SIZES {
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "registry failure");
        assert!(g.hits.iter().any(|x| x.label.starts_with("Retry")));
    }
    click(&mut r.p, 1280, 720, "Retry");
    assert_eq!(r.p.page(), &Page::Confirm);
}

#[test]
fn the_registry_row_creates_when_absent_and_shows_ready_with_a_typed_remove() {
    let mut r = store_rig_with("regrow", NODES, None, true);
    r.p.open_list();
    until(&mut r.p, "the list", |p| p.page() == &Page::List && p.rows.len() >= 3 && !p.is_busy());
    let reg = r.p.rows.iter().position(|x| x.kind == RowKind::Registry).unwrap();
    r.p.sel = reg;
    r.p.handle(&Event::Key(Key::Enter));
    until(&mut r.p, "the create review", |p| p.page() == &Page::Confirm);
    assert!(r.p.text().contains("A local image registry will be created"), "{}", r.p.text());
    r.p.handle(&Event::Key(Key::Esc));
    // now it exists
    std::fs::write(r.dir.join("registry-ready"), "1").unwrap();
    r.p.sel = reg;
    r.p.handle(&Event::Key(Key::Enter));
    until(&mut r.p, "the ready view", |p| p.page() == &Page::Confirm);
    let t = r.p.text();
    assert!(t.contains("registry ready") && t.contains("localhost:30500") && t.contains("Type iyakki-registry"), "{t}");
    // a wrong word removes nothing
    type_text(&mut r.p, "no");
    r.p.handle(&Event::Key(Key::Enter));
    until(&mut r.p, "refused", |p| p.text().contains("NOT DONE"));
    assert!(!r.run.lines().iter().any(|l| l.contains("delete namespace")));
    assert!(r.dir.join("registry-ready").exists());
}

// ------------------------------------------------------------------ the private store: state, repair, reset, remove

const OURS_NS: &str = r#"{"metadata":{"name":"iyakki-private-store","labels":{"app.kubernetes.io/managed-by":"iyakki","iyakki.dev/service":"private-store"}},"status":{"phase":"Active"}}"#;
const REFUSED: &str = "The connection to the server 127.0.0.1:6443 was refused - did you specify the right host or port? connection refused";
const INSTANCE: &str = "private-store-iyakki-private-store";

#[derive(Clone, Copy, PartialEq)]
enum Ms {
    /// Only the namespace exists.
    Partial,
    /// Everything but the address (Service).
    ServiceMissing,
    Down,
}

struct MRig {
    p: PlatformPanel,
    cx: crate::apps::AppCx,
    dir: PathBuf,
    run: Arc<FakeRunner>,
    vault: Arc<MemVault>,
}

impl Drop for MRig {
    fn drop(&mut self) {
        for j in self.cx.services.jobs.list("alice") {
            let _ = self.cx.services.jobs.stop("alice", &j.id, Duration::from_millis(300));
        }
    }
}

/// A private store in the state `sc`, a fake cluster, a vault with its token, and a stand-in for `iyakki store-manage`.
fn manage_rig(tag: &str, sc: Ms, kubectl: bool, job_body: &str) -> MRig {
    let dir = tmp(tag);
    let kc = dir.join("kubeconfig.yaml");
    std::fs::write(&kc, "apiVersion: v1\n").unwrap();
    Choice { kubeconfig: kc.display().to_string(), context: String::new() }.save(&dir, "alice").unwrap();
    let run = Arc::new(FakeRunner::new(if kubectl { &["kubectl", "curl"] } else { &["curl"] }));
    match sc {
        Ms::Down => {
            run.fail("get namespace iyakki-private-store", 1, REFUSED);
        }
        Ms::Partial => {
            run.answer("get namespace iyakki-private-store", OURS_NS);
            run.answer("get deployments,pods -n iyakki-private-store", r#"{"items":[]}"#);
            run.answer("get services,persistentvolumeclaims,secrets -n iyakki-private-store", "");
        }
        Ms::ServiceMissing => {
            run.answer("get namespace iyakki-private-store", OURS_NS);
            run.answer("get deployments,pods -n iyakki-private-store", r#"{"items":[{"kind":"Deployment","metadata":{"name":"private-store"},"spec":{"replicas":1},"status":{"readyReplicas":1}}]}"#);
            run.answer("get services,persistentvolumeclaims,secrets -n iyakki-private-store", "persistentvolumeclaim/private-store-data\nsecret/private-store\n");
            run.answer("get endpoints private-store", r#"{"subsets":[{"addresses":[{"ip":"10.1.0.5"}]}]}"#);
            run.answer("get secret private-store -n iyakki-private-store -o name", "secret/private-store");
            run.answer("get service registry", r#"{"spec":{"ports":[{"nodePort":30500}]}}"#);
        }
    }
    answers(&run, "https://127.0.0.1:6443");
    std::fs::create_dir_all(dir.join("store-server")).unwrap();
    std::fs::write(dir.join("store-server/iyakki"), elf_bytes(host_machine(), false)).unwrap();
    let vault = Arc::new(MemVault::default());
    vault.set(&format!("svc.{INSTANCE}.admin-token"), "tok").unwrap();
    vault.set(&format!("svc.{INSTANCE}.catalog-seed"), "seed").unwrap();
    vault.set("svc.other-store-ns.admin-token", "other").unwrap();
    let (cx, _d) = crate::apps::tests::cx_with_backend(&format!("ms-{tag}"), iyakki_agent::chat::Unconfigured);
    let pl = Platform {
        run: run.clone(),
        dir: dir.clone(),
        user: "alice".into(),
        vault: vault.clone(),
        audit: Arc::new(|_, _| {}),
        env: Env { home: dir.clone(), ..Default::default() },
        tmp: dir.join("ram"),
        http: Arc::new(FakeHttp::default()),
        gen: Arc::new(|n| format!("{:A<n$}", PLANTED)),
        live: None,
    };
    // the stand-in for the job: it records its arguments and does what the body says
    use std::os::unix::fs::PermissionsExt;
    let exe = dir.join("fake-iyakki.sh");
    std::fs::write(&exe, format!("#!/bin/sh\necho \"$*\" >> {}\n{job_body}\n", dir.join("job-args").display())).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let launch = iyakki_agent::quickinstall::Launch { exe, data: dir.join("qi") };
    let p = PlatformPanel::new(Arc::new(pl)).with_services(cx.services.clone()).with_registry_launch(launch);
    MRig { p, cx, dir, run, vault }
}

fn to_store(r: &mut MRig) {
    r.p.open_store_manage();
    until(&mut r.p, "the clusters", |p| p.wiz.as_ref().is_some_and(|w| !w.loading));
    click(&mut r.p, 1280, 720, "Next");
    until(&mut r.p, "the store page", |p| p.page() == &Page::Store && p.store_rep.is_some());
}

fn cluster_writes(r: &MRig) -> Vec<String> {
    r.run.lines().into_iter().filter(|l| !l.contains("can-i")).filter(|l| ["create ", "apply ", "label ", "delete ", "patch ", "rollout restart"].iter().any(|k| l.contains(k))).collect()
}

fn job_args(r: &MRig) -> String {
    std::fs::read_to_string(r.dir.join("job-args")).unwrap_or_default()
}

#[test]
fn a_half_installed_store_shows_what_is_there_and_missing_with_repair_reset_and_remove_at_every_size_and_changes_nothing() {
    let mut r = manage_rig("partial", Ms::Partial, true, "echo ok");
    to_store(&mut r);
    let t = r.p.text();
    for want in ["PARTLY INSTALLED", "There: the namespace", "Missing: the data volume (PVC), the admin token Secret, the server (Deployment), the address (Service)", "Repair puts back only what is missing"] {
        assert!(t.contains(want), "{want}\n{t}");
    }
    for (w, h) in SIZES {
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "store page");
        buttons_apart(&g, w, h, "store page");
        for b in ["Repair", "Reset and reinstall", "Remove", "Look again", "Back"] {
            assert!(g.hits.iter().any(|x| x.label == b), "{w}x{h}: no `{b}` in {:?}", g.hits.iter().map(|x| x.label.clone()).collect::<Vec<_>>());
        }
        assert!(!g.hits.iter().any(|x| x.label.starts_with("Deploy")), "a half-installed store is not simply deployed over");
    }
    assert!(cluster_writes(&r).is_empty(), "looking changes nothing: {:?}", cluster_writes(&r));
    assert!(r.cx.services.jobs.list("alice").is_empty());
    // Back returns to the list (the wizard was opened to look after the store)
    r.p.handle(&Event::Key(Key::Esc));
    assert_eq!(r.p.page(), &Page::List);
}

#[test]
fn the_deploy_wizard_looks_at_the_store_first_and_offers_repair_instead_of_failing_on_already_exists() {
    let mut r = manage_rig("wizard", Ms::Partial, true, "echo ok");
    r.p.open_wizard("private-store");
    until(&mut r.p, "the clusters", |p| p.wiz.as_ref().is_some_and(|w| !w.loading));
    click(&mut r.p, 1280, 720, "Next");
    until(&mut r.p, "the store page", |p| p.page() == &Page::Store && p.store_rep.is_some());
    assert!(r.p.text().contains("PARTLY INSTALLED"));
    assert!(cluster_writes(&r).is_empty());
    for (w, h) in SIZES {
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "store page in the wizard");
        assert!(g.hits.iter().any(|x| x.label == "Repair") && g.hits.iter().any(|x| x.label == "Reset and reinstall"));
    }
    // Back goes to the cluster list of the wizard, not out
    r.p.handle(&Event::Key(Key::Esc));
    assert_eq!(r.p.page(), &Page::Cluster);
}

#[test]
fn removing_asks_for_the_namespace_keeps_the_data_by_default_and_runs_as_a_job() {
    let mut r = manage_rig("remove-store", Ms::Partial, true, "echo 'namespace is gone'");
    to_store(&mut r);
    click(&mut r.p, 1280, 720, "Remove");
    assert_eq!(r.p.page(), &Page::Confirm);
    let t = r.p.text();
    assert!(t.contains("KEPT: the data volume") && t.contains("registry is a separate thing: it is KEPT") && t.contains("type the namespace (iyakki-private-store)") && !t.contains("WARNING"), "{t}");
    for (w, h) in SIZES {
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "remove review");
        buttons_apart(&g, w, h, "remove review");
        for b in ["Approve", "Keep/delete data", "Registry too", "Cancel"] {
            assert!(g.hits.iter().any(|x| x.label == b), "{w}x{h}: no `{b}`");
        }
    }
    // the choices flip the text
    r.p.handle(&Event::Key(Key::Ctrl('k')));
    assert!(r.p.text().contains("WARNING") && r.p.text().contains("uvault:svc.private-store-iyakki-private-store.admin-token"), "{}", r.p.text());
    r.p.handle(&Event::Key(Key::Ctrl('r')));
    assert!(r.p.text().contains("WILL be deleted"));
    r.p.handle(&Event::Key(Key::Ctrl('r')));
    r.p.handle(&Event::Key(Key::Ctrl('k')));
    assert!(r.p.text().contains("KEPT: the data volume"));
    // a wrong word starts nothing
    type_text(&mut r.p, "nope");
    r.p.handle(&Event::Key(Key::Enter));
    until(&mut r.p, "refused", |p| p.text().contains("NOT DONE"));
    assert!(r.p.text().contains("type the namespace `iyakki-private-store`"), "{}", r.p.text());
    assert!(r.cx.services.jobs.list("alice").is_empty() && cluster_writes(&r).is_empty());
    // closing the refusal looks at the store again
    r.p.handle(&Event::Key(Key::Enter));
    until(&mut r.p, "the store page again", |p| p.page() == &Page::Store && p.store_rep.is_some());
    // the right word, data kept: a job, the top bar's workspace, and the vault stays
    click(&mut r.p, 1280, 720, "Remove");
    type_text(&mut r.p, "iyakki-private-store");
    r.p.handle(&Event::Key(Key::Enter));
    until(&mut r.p, "the job starts", |p| p.page() != &Page::Confirm);
    let jobs = r.cx.services.jobs.list("alice");
    assert_eq!(jobs.len(), 1, "{:?} {}", r.p.page(), r.p.text());
    assert_eq!((jobs[0].label.as_str(), jobs[0].workspace.as_str()), ("Remove private store", iyakki_agent::quickinstall::WORKSPACE));
    assert!(jobs[0].command.contains("store-manage remove --kubeconfig") && jobs[0].command.contains("--namespace iyakki-private-store"), "{}", jobs[0].command);
    if r.p.page() == &Page::Progress {
        for (w, h) in SIZES {
            let g = draw(&mut r.p, w, h);
            inside(&g, w, h, "progress");
            assert!(g.hits.iter().any(|x| x.label.starts_with("Hide")));
        }
    }
    until(&mut r.p, "done", |p| p.page() == &Page::Result && p.text().contains("DONE"));
    assert!(!job_args(&r).contains("--delete-data"), "{}", job_args(&r));
    assert!(r.vault.has(&format!("svc.{INSTANCE}.admin-token")), "the data was kept, so were the vault entries");
    assert!(r.vault.has("svc.other-store-ns.admin-token"));
    assert!(r.p.text().contains("kept the vault entries"), "{}", r.p.text());
    // now delete everything
    r.p.handle(&Event::Key(Key::Enter));
    let mut r2 = r;
    to_store(&mut r2);
    click(&mut r2.p, 1280, 720, "Remove");
    r2.p.handle(&Event::Key(Key::Ctrl('k')));
    type_text(&mut r2.p, "iyakki-private-store");
    r2.p.handle(&Event::Key(Key::Enter));
    until(&mut r2.p, "done", |p| p.page() == &Page::Result && p.text().contains("DONE"));
    assert!(job_args(&r2).lines().last().unwrap().contains("--delete-data"), "{}", job_args(&r2));
    assert!(!r2.vault.has(&format!("svc.{INSTANCE}.admin-token")) && !r2.vault.has(&format!("svc.{INSTANCE}.catalog-seed")));
    assert!(r2.vault.has("svc.other-store-ns.admin-token"), "another store's entries are not this store's");
}

#[test]
fn reset_and_reinstall_runs_the_job_wipes_this_stores_entries_and_hands_over_to_the_deploy_wizard() {
    let mut r = manage_rig("reset-store", Ms::Partial, true, "echo 'namespace is gone'");
    to_store(&mut r);
    click(&mut r.p, 1280, 720, "Reset and reinstall");
    assert_eq!(r.p.page(), &Page::Confirm);
    let t = r.p.text();
    assert!(t.contains("RESET AND REINSTALL") && t.contains("DELETED") && t.contains("add the store again") && t.contains("Ctrl+F"), "{t}");
    for (w, h) in SIZES {
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "reset review");
        buttons_apart(&g, w, h, "reset review");
        assert!(g.hits.iter().any(|x| x.label == "Clear finalizers"));
    }
    type_text(&mut r.p, "wrong");
    r.p.handle(&Event::Key(Key::Enter));
    until(&mut r.p, "refused", |p| p.text().contains("NOT DONE"));
    assert!(r.cx.services.jobs.list("alice").is_empty());
    r.p.handle(&Event::Key(Key::Enter));
    until(&mut r.p, "the store page again", |p| p.page() == &Page::Store && p.store_rep.is_some());
    click(&mut r.p, 1280, 720, "Reset and reinstall");
    type_text(&mut r.p, "iyakki-private-store");
    r.p.handle(&Event::Key(Key::Enter));
    until(&mut r.p, "the job", |p| p.page() == &Page::Progress || p.page() == &Page::Cluster);
    until(&mut r.p, "the wizard continues", |p| p.page() == &Page::Cluster && p.wiz.as_ref().is_some_and(|w| !w.manage));
    assert!(job_args(&r).contains("store-manage reset") && job_args(&r).contains("--delete-data"), "{}", job_args(&r));
    let labels: Vec<String> = r.cx.services.jobs.list("alice").into_iter().map(|j| j.label).collect();
    assert_eq!(labels, vec!["Reset private store".to_string()]);
    assert!(!r.vault.has(&format!("svc.{INSTANCE}.admin-token")) && r.vault.has("svc.other-store-ns.admin-token"));
    assert!(r.p.message().contains("Reset done"), "{}", r.p.message());
}

#[test]
fn a_namespace_stuck_terminating_explains_and_offers_to_try_again_clearing_finalizers() {
    let body = "echo 'error: namespace iyakki-private-store is still Terminating after 20s. Kubernetes deletes a namespace only when every object in it has finished deleting.'; exit 1";
    let mut r = manage_rig("stuck", Ms::Partial, true, body);
    to_store(&mut r);
    click(&mut r.p, 1280, 720, "Reset and reinstall");
    type_text(&mut r.p, "iyakki-private-store");
    r.p.handle(&Event::Key(Key::Enter));
    until(&mut r.p, "the failure", |p| p.page() == &Page::Result);
    let t = r.p.text();
    assert!(t.contains("THE RESET STOPPED") && t.contains("still Terminating") && t.contains("clear the finalizers") && t.contains("Nothing outside the store's namespace was touched"), "{t}");
    assert!(r.vault.has(&format!("svc.{INSTANCE}.admin-token")), "this device's entries are not touched when the cluster side failed");
    for (w, h) in SIZES {
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "stuck");
        buttons_apart(&g, w, h, "stuck");
        assert!(g.hits.iter().any(|x| x.label.starts_with("Reset again, clearing finalizers")));
    }
    click(&mut r.p, 1280, 720, "Reset again");
    assert_eq!(r.p.page(), &Page::Confirm);
    assert!(r.p.text().contains("now: ON"), "{}", r.p.text());
    type_text(&mut r.p, "iyakki-private-store");
    r.p.handle(&Event::Key(Key::Enter));
    until(&mut r.p, "failed again", |p| p.page() == &Page::Result && p.text().contains("THE RESET STOPPED"));
    assert!(job_args(&r).lines().last().unwrap().contains("--force"), "{}", job_args(&r));
    assert!(!job_args(&r).lines().next().unwrap().contains("--force"));
}

#[test]
fn a_cluster_that_is_down_is_explained_and_nothing_destructive_is_offered() {
    let mut r = manage_rig("down", Ms::Down, true, "echo ok");
    // a down cluster cannot be chosen from the list, so the store's own record names it
    let kc = std::fs::read_to_string(r.dir.join("platform-profiles-alice.json")).ok();
    let _ = kc;
    let c = platform::ClusterRef { name: "k3s".into(), kubeconfig: r.dir.join("kubeconfig.yaml").display().to_string(), context: String::new(), server: "https://127.0.0.1:6443".into(), source: iyakki_agent::devcluster::Source::Remembered };
    let rec = Installed { id: INSTANCE.into(), service: "private-store".into(), title: "Private store".into(), version: "1.0.0".into(), kind: "store".into(), cluster: Some(c), namespace: "iyakki-private-store".into(), ..Default::default() };
    installed::upsert(&r.dir, "alice", rec).unwrap();
    r.p.open_store_manage();
    until(&mut r.p, "the store page", |p| p.page() == &Page::Store && p.store_rep.is_some());
    let t = r.p.text();
    assert!(t.contains("BROKEN") && t.contains("does not answer") && t.contains("Start the cluster") && t.contains("Nothing was changed"), "{t}");
    for (w, h) in SIZES {
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "cluster down");
        buttons_apart(&g, w, h, "cluster down");
        assert!(g.hits.iter().any(|x| x.label == "Look again"));
        for no in ["Repair", "Reset and reinstall", "Remove"] {
            assert!(!g.hits.iter().any(|x| x.label == no), "{no} must not be offered while the cluster is down");
        }
    }
    assert!(cluster_writes(&r).is_empty());
    // the hotkeys do nothing either
    for k in ['r', 'x', 'm'] {
        r.p.handle(&Event::Key(Key::Char(k)));
        assert_eq!(r.p.page(), &Page::Store);
    }
}

#[test]
fn with_no_kubectl_anywhere_the_store_page_says_so() {
    let mut r = manage_rig("nokubectl", Ms::Partial, false, "echo ok");
    let c = platform::ClusterRef { name: "k3s".into(), kubeconfig: r.dir.join("kubeconfig.yaml").display().to_string(), context: String::new(), server: "https://127.0.0.1:6443".into(), source: iyakki_agent::devcluster::Source::Remembered };
    let rec = Installed { id: INSTANCE.into(), service: "private-store".into(), version: "1.0.0".into(), kind: "store".into(), cluster: Some(c), namespace: "iyakki-private-store".into(), ..Default::default() };
    installed::upsert(&r.dir, "alice", rec).unwrap();
    r.p.open_store_manage();
    until(&mut r.p, "the store page", |p| p.page() == &Page::Store && p.store_rep.is_some());
    let t = r.p.text();
    assert!(t.contains("no kubectl") && t.contains("k3s") && t.contains("Nothing was changed"), "{t}");
    assert!(r.run.lines().iter().all(|l| !l.contains("get namespace")), "{:?}", r.run.lines());
}

#[test]
fn repair_from_the_store_page_asks_without_typing_and_applies_only_the_missing_address() {
    let mut r = manage_rig("repair", Ms::ServiceMissing, true, "echo ok");
    to_store(&mut r);
    assert!(r.p.text().contains("PARTLY INSTALLED") && r.p.text().contains("Missing: the address (Service)"), "{}", r.p.text());
    click(&mut r.p, 1280, 720, "Repair");
    assert_eq!(r.p.page(), &Page::Confirm);
    let t = r.p.text();
    assert!(t.contains("REPAIR") && t.contains("create the address (Service)") && !t.contains("type the namespace"), "{t}");
    for (w, h) in SIZES {
        let g = draw(&mut r.p, w, h);
        inside(&g, w, h, "repair review");
        buttons_apart(&g, w, h, "repair review");
    }
    assert!(cluster_writes(&r).is_empty(), "nothing before the approval");
    r.p.handle(&Event::Key(Key::Char('y')));
    until(&mut r.p, "repaired", |p| p.text().contains("DONE") || p.text().contains("NOT DONE"));
    let t = r.p.text();
    assert!(t.contains("DONE") && !t.contains("NOT DONE"), "{t}");
    let writes = cluster_writes(&r);
    assert!(writes.iter().any(|l| l.contains("apply -f")) && writes.iter().all(|l| !l.contains("create secret") && !l.contains("delete ")), "{writes:?}");
    let applied = std::fs::read_to_string(r.run.lines().iter().find(|l| l.contains("apply -f")).and_then(|l| l.split("apply -f ").nth(1)).and_then(|l| l.split_whitespace().next()).unwrap()).unwrap();
    assert!(applied.contains("kind: Service") && !applied.contains("kind: Deployment") && !applied.contains("PersistentVolumeClaim"), "{applied}");
    assert!(installed::find(&r.dir, "alice", INSTANCE).is_some());
}
