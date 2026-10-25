//! Settings → Dev cluster: the profile list, geometry and click paths at 1280x720 and 1024x600, Test / default / hide / disconnect,
//! vault profiles, the registry plan, and the bare-metal set-up flow with a stand-in `k3s` (no real download, no real cluster).

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_agent::iyakki_proxy::StreamHead;
use iyakki_agent::k3sdev::Machine;
use iyakki_agent::wsexec::FakeRunner;
use std::io::Cursor;

struct Offline;
impl iyakki_agent::iyakki_proxy::Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_agent::iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}
fn press(a: &mut SettingsApp, cx: &AppCx, c: char) {
    key(a, cx, Key::Char(c));
}
fn typ(a: &mut SettingsApp, cx: &AppCx, s: &str) {
    s.chars().for_each(|c| press(a, cx, c));
}
fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}
/// Enter through a form to its end (the last field submits); stops when it closes or complains.
fn submit(a: &mut SettingsApp, cx: &AppCx) {
    for _ in 0..8 {
        if !a.has_modal() {
            return;
        }
        if let Some(Modal::Form(_, f)) = &a.modal {
            if f.error.is_some() {
                return;
            }
        }
        key(a, cx, Key::Enter);
    }
}
fn settle(a: &mut SettingsApp, cx: &AppCx) {
    for _ in 0..4000 {
        a.handle(&Event::Tick, cx);
        if !a.is_busy() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("job did not finish");
}
fn until(a: &mut SettingsApp, cx: &AppCx, what: &str, mut f: impl FnMut(&mut SettingsApp) -> bool) {
    for _ in 0..3000 {
        a.handle(&Event::Tick, cx);
        if f(a) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("never happened: {what}; status: {}", status(a));
}
fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}
fn overlay_text(a: &SettingsApp) -> String {
    a.dc.overlay.as_ref().map(|o| o.log.lines.iter().map(|l| l.0.clone()).collect::<Vec<_>>().join("\n")).unwrap_or_default()
}
fn audit_has(cx: &AppCx, what: &str) -> bool {
    std::fs::read_to_string(cx.audit.path()).unwrap_or_default().contains(what)
}

/// Two kubeconfigs on a made-up machine: this machine's k3s (answers, local) and a remote production-looking cluster from `$KUBECONFIG`.
struct Rig {
    cx: AppCx,
    a: SettingsApp,
    run: Arc<FakeRunner>,
    dir: PathBuf,
    k3s: PathBuf,
    remote: PathBuf,
}

fn answers(run: &FakeRunner, path: &Path, ctx: &str, server: &str, version: &str, node: &str) {
    let p = path.display();
    // `kubectl --kubeconfig P [--context C] --request-timeout=Ns <args>`: the context is added once it is known
    for c in [String::new(), format!("--context {ctx} ")] {
        run.answer(&format!("--kubeconfig {p} {c}--request-timeout=3s config current-context"), ctx);
        run.answer(&format!("--kubeconfig {p} {c}--request-timeout=3s config view --minify"), server);
        run.answer(&format!("--kubeconfig {p} {c}--request-timeout=3s version -o json"), &format!("{{\"serverVersion\":{{\"gitVersion\":\"{version}\"}}}}"));
        run.answer(&format!("--kubeconfig {p} {c}--request-timeout=3s auth can-i"), "yes");
        run.answer(&format!("--kubeconfig {p} {c}--request-timeout=8s get nodes"), node);
    }
}

fn rig(tag: &str) -> Rig {
    let cx = admin(tag);
    let dir = temp_dir(&format!("{tag}-kc"));
    let (k3s, remote) = (dir.join("k3s.yaml"), dir.join("remote.yaml"));
    std::fs::write(&k3s, "apiVersion: v1\n").unwrap();
    std::fs::write(&remote, "apiVersion: v1\n").unwrap();
    let run = Arc::new(FakeRunner::new(&["kubectl"]));
    answers(&run, &k3s, "default", "https://127.0.0.1:6443", "v1.31.1+k3s1", "iyakki-dev");
    answers(&run, &remote, "prod-eks", "https://203.0.113.9:6443", "v1.29.4", "ip-10-0-0-5");
    let env = ClusterEnv { home: dir.join("nohome"), k3s_files: vec![k3s.clone()], kubeconfig_env: Some(remote.display().to_string()), ..Default::default() };
    let a = SettingsApp::new(&cx).with_cluster(run.clone(), env);
    Rig { cx, a, run, dir, k3s, remote }
}

impl Rig {
    fn open(&mut self) {
        self.a.go_to(Tab::DevCluster);
        until(&mut self.a, &self.cx, "the profiles are listed", |a| a.dc.rows.len() >= 2);
    }
    fn row(&self, id: &Path) -> Row {
        self.a.dc.rows.iter().find(|r| r.id == id.display().to_string()).cloned().unwrap_or_else(|| panic!("no row for {}: {:?}", id.display(), self.a.dc.rows.iter().map(|r| r.id.clone()).collect::<Vec<_>>()))
    }
    fn select(&mut self, id: &Path) {
        let i = self.a.dc.rows.iter().position(|r| r.id == id.display().to_string()).expect("row");
        self.a.handle(&Event::Row(i), &self.cx);
    }
    fn workspace(&self, name: &str) -> PathBuf {
        let root = temp_dir(&format!("{}-ws-{name}", self.dir.file_name().unwrap().to_string_lossy()));
        ProjectConfig::ensure(&root, name).unwrap();
        WsRegistry::load(self.cx.services.dir(), "root").add(name, &root).unwrap();
        root
    }
}

fn chips(r: &Row) -> Vec<&str> {
    r.chips.iter().map(|c| c.0.as_str()).collect()
}

fn overlaps(a: R, b: R) -> bool {
    a.x < b.x + b.w - 0.5 && b.x < a.x + a.w - 0.5 && a.y < b.y + b.h - 0.5 && b.y < a.y + a.h - 0.5
}

#[test]
fn the_page_lists_every_cluster_with_server_node_version_reachability_remote_and_why_the_assistant_only_reads() {
    let mut r = rig("dc-list");
    r.open();
    let k = r.row(&r.k3s);
    let c = chips(&k);
    assert!(c.contains(&"reachable") && c.contains(&"k3s") && !c.contains(&"remote") && !c.contains(&"read-only"), "{c:?}");
    for want in ["https://127.0.0.1:6443", "node iyakki-dev", "v1.31.1+k3s1", "from k3s on this machine"] {
        assert!(k.sub.contains(want), "{want} in {}", k.sub);
    }
    let rm = r.row(&r.remote);
    let c = chips(&rm);
    assert!(c.contains(&"reachable") && c.contains(&"remote") && c.contains(&"read-only") && c.contains(&"production-looking"), "{c:?}");
    for want in ["https://203.0.113.9:6443", "node ip-10-0-0-5", "v1.29.4", "$KUBECONFIG"] {
        assert!(rm.sub.contains(want), "{want} in {}", rm.sub);
    }
    let why = devcluster::read_only_reason(rm.profile.as_ref().unwrap()).unwrap();
    assert!(why.contains("never chosen for this workspace") || why.contains("looks like production"), "{why}");
    // the page is for administrators; ordinary accounts choose in the AI IDE's K8S panel
    assert!(r.a.tabs().contains(&Tab::DevCluster));
    let mut user_cx = r.cx.clone();
    user_cx.session.role = iyakki_sec::policy::Role::User;
    user_cx.session.policy = iyakki_sec::policy::Role::User.template();
    assert!(!SettingsApp::new(&user_cx).tabs().contains(&Tab::DevCluster));
}

#[test]
fn geometry_and_click_paths_hold_at_1280x720_and_1024x600() {
    let mut r = rig("dc-geom");
    r.open();
    for (w, h) in [(1280u32, 720u32), (1024, 600)] {
        let area = R::new(0.0, 72.0, w as f32, h as f32 - 72.0);
        let g = draw(&mut r.a, &r.cx, w, h);
        let sel = r.a.sel();
        assert!(g.hits.iter().any(|x| x.ev == Event::Row(sel)), "{w}x{h}: the selected row is on screen");
        let keys: Vec<(char, R)> = g.hits.iter().filter_map(|x| if let Event::Key(Key::Char(c)) = x.ev { Some((c, x.rect)) } else { None }).collect();
        for want in ['t', 'd', 'w', 'k', 'h', 'x', 'g', 'v', 'V', 'r'] {
            assert!(keys.iter().any(|k| k.0 == want), "{w}x{h}: button {want} is there; got {:?}", keys.iter().map(|k| k.0).collect::<String>());
        }
        // every control is inside the window, none covers another, and the buttons stay clear of the rows
        let rows: Vec<R> = g.hits.iter().filter(|x| matches!(x.ev, Event::Row(_))).map(|x| x.rect).collect();
        assert!(!rows.is_empty());
        for (c, rect) in &keys {
            assert!(rect.x >= area.x && rect.y >= area.y && rect.x + rect.w <= area.x + area.w + 0.5 && rect.y + rect.h <= area.y + area.h + 0.5, "{w}x{h}: button {c} {rect:?} is outside {area:?}");
        }
        for (i, (ca, ra)) in keys.iter().enumerate() {
            for (cb, rb) in keys.iter().skip(i + 1) {
                assert!(!overlaps(*ra, *rb), "{w}x{h}: buttons {ca} and {cb} overlap");
            }
            for row_r in &rows {
                assert!(!overlaps(*ra, *row_r), "{w}x{h}: button {ca} covers a row");
            }
        }
        for rr in &rows {
            assert!(rr.x >= area.x && rr.y >= area.y && rr.x + rr.w <= area.x + area.w + 0.5 && rr.y + rr.h <= area.y + area.h + 0.5, "{w}x{h}: row {rr:?} outside {area:?}");
        }
        // clicking a row selects it
        let row_hit = g.hits.iter().filter_map(|x| if let Event::Row(i) = x.ev { Some(i) } else { None }).max().unwrap();
        r.a.handle(&Event::Row(row_hit), &r.cx);
        assert_eq!(r.a.sel(), row_hit);
        r.a.handle(&Event::Row(0), &r.cx);
        // the machine card: no cluster on this made-up machine means the set-up button is offered (as the card says, to administrators)
        let sit = r.cx.services.k3sdev.situation();
        assert_eq!(keys.iter().any(|k| k.0 == 's'), matches!(sit, Situation::Free { ref problems } if problems.is_empty()), "{w}x{h}: {sit:?}");
    }
}

#[test]
fn test_connection_default_for_you_and_for_a_workspace_hide_show_and_disconnect_are_audited_and_hidden_clusters_are_never_probed() {
    let mut r = rig("dc-actions");
    let ws = r.workspace("shop");
    r.open();
    // Test connection: version, node, what the identity may do, and for the remote one why the assistant only reads
    r.select(&r.k3s.clone());
    press(&mut r.a, &r.cx, 't');
    settle(&mut r.a, &r.cx);
    let s = status(&r.a);
    for want in ["https://127.0.0.1:6443", "v1.31.1+k3s1", "node iyakki-dev", "get pods yes"] {
        assert!(s.contains(want), "{want} in {s}");
    }
    assert!(r.a.status().unwrap().1);
    r.select(&r.remote.clone());
    press(&mut r.a, &r.cx, 't');
    settle(&mut r.a, &r.cx);
    assert!(status(&r.a).contains("assistant: read-only"), "{}", status(&r.a));
    assert!(audit_has(&r.cx, "devcluster.test"));
    // default for you
    r.select(&r.k3s.clone());
    press(&mut r.a, &r.cx, 'd');
    let dir = r.cx.services.dir().to_path_buf();
    assert_eq!(Choice::load(&dir, "root").unwrap().kubeconfig, r.k3s.display().to_string());
    assert!(status(&r.a).contains("your default cluster") && audit_has(&r.cx, "devcluster.default.user"));
    // default for the workspace in view
    press(&mut r.a, &r.cx, 'w');
    assert_eq!(ProjectConfig::load(&ws).unwrap().k8s.kubeconfig, r.k3s.display().to_string(), "{}", status(&r.a));
    assert!(audit_has(&r.cx, "devcluster.default.workspace"));
    until(&mut r.a, &r.cx, "the badges follow", |a| a.dc.rows.iter().any(|x| x.chips.iter().any(|c| c.0 == "your default")));
    let c = r.row(&r.k3s);
    assert!(chips(&c).contains(&"your default") && chips(&c).iter().any(|x| x.starts_with("default for shop")), "{:?}", chips(&c));
    // the default cannot be hidden; another cluster can
    press(&mut r.a, &r.cx, 'h');
    assert!(status(&r.a).contains("your default cluster") && r.a.status().is_some_and(|s| !s.1), "{}", status(&r.a));
    r.select(&r.remote.clone());
    press(&mut r.a, &r.cx, 'h');
    assert!(status(&r.a).contains("Hidden in IYAKKI") && audit_has(&r.cx, "devcluster.hide"));
    assert_eq!(devcluster::hidden_load(&dir, "root"), vec![r.remote.display().to_string()], "only IYAKKI's list changes");
    assert!(r.remote.exists(), "the kubeconfig file itself is untouched");
    until(&mut r.a, &r.cx, "the list is looked at again", |a| a.dc.rows.iter().any(|x| x.kind == RowKind::Hidden));
    let hidden = r.row(&r.remote);
    assert_eq!(chips(&hidden), vec!["hidden"]);
    // one more complete look: from here on nothing may touch the hidden cluster
    let (before, scans) = (r.run.calls().len(), r.a.dc.scans.load(Ordering::SeqCst));
    press(&mut r.a, &r.cx, 'r');
    until(&mut r.a, &r.cx, "another complete look", |a| a.dc.scans.load(Ordering::SeqCst) > scans);
    let after = r.run.calls();
    assert!(after[before..].iter().all(|c| !c.line().contains("remote.yaml")), "no call reaches a hidden cluster: {:?}", after[before..].iter().map(|c| c.line()).collect::<Vec<_>>());
    // test and default are refused on a hidden row; h shows it again
    r.select(&r.remote.clone());
    press(&mut r.a, &r.cx, 't');
    assert!(status(&r.a).contains("never probed"));
    press(&mut r.a, &r.cx, 'd');
    assert!(status(&r.a).contains("hidden"));
    press(&mut r.a, &r.cx, 'h');
    assert!(status(&r.a).contains("Shown again") && devcluster::hidden_load(&dir, "root").is_empty());
    until(&mut r.a, &r.cx, "shown", |a| a.dc.rows.iter().all(|x| x.kind != RowKind::Hidden));
    // disconnect: the plan names what is forgotten, nothing happens before Approve
    r.select(&r.remote.clone());
    press(&mut r.a, &r.cx, 'x');
    assert!(status(&r.a).contains("Nothing of yours points at this cluster"), "{}", status(&r.a));
    r.select(&r.k3s.clone());
    press(&mut r.a, &r.cx, 'x');
    let t = overlay_text(&r.a);
    assert!(t.contains("your default cluster") && t.contains("workspace shop") && t.contains("no kubeconfig file is changed"), "{t}");
    assert!(Choice::load(&dir, "root").is_some(), "asked, not yet done");
    key(&mut r.a, &r.cx, Key::Esc);
    assert!(Choice::load(&dir, "root").is_some() && !r.a.dc_has_overlay(), "Cancel changes nothing");
    press(&mut r.a, &r.cx, 'x');
    press(&mut r.a, &r.cx, 'y');
    assert!(Choice::load(&dir, "root").is_none());
    assert_eq!(ProjectConfig::load(&ws).unwrap().k8s.kubeconfig, "");
    assert!(audit_has(&r.cx, "devcluster.disconnect"));
}

#[test]
fn a_vault_held_kubeconfig_is_added_with_a_checked_entry_tested_through_the_runner_and_removed() {
    let mut r = rig("dc-vault");
    r.cx.services.user_set("root", "staging-kc", "apiVersion: v1\nkind: Config\n").unwrap();
    answers(&r.run, Path::new("uvault:staging-kc"), "staging", "https://198.51.100.7:6443", "v1.30.2", "stg-1");
    r.open();
    press(&mut r.a, &r.cx, 'v');
    assert!(r.a.has_modal());
    typ(&mut r.a, &r.cx, "Staging EKS");
    key(&mut r.a, &r.cx, Key::Enter);
    typ(&mut r.a, &r.cx, "no-such-entry");
    submit(&mut r.a, &r.cx);
    assert!(r.a.has_modal(), "an entry that does not exist keeps the form open");
    // fix the entry name
    let form_entry = |a: &SettingsApp| if let Some(Modal::Form(_, f)) = &a.modal { f.value(1) } else { String::new() };
    assert_eq!(form_entry(&r.a), "no-such-entry");
    if let Some(Modal::Form(_, f)) = r.a.modal.as_mut() {
        f.set_value(1, "staging-kc");
        f.error = None;
    }
    submit(&mut r.a, &r.cx);
    assert!(!r.a.has_modal(), "{}", status(&r.a));
    let dir = r.cx.services.dir().to_path_buf();
    assert_eq!(kubeprofiles::vault_load(&dir, "root")[0].reference, "uvault:staging-kc");
    assert!(!std::fs::read_to_string(dir.join("devcluster-vault-root.json")).unwrap().contains("kind: Config"), "only the reference is stored");
    assert!(audit_has(&r.cx, "devcluster.vault_profile.add") && !audit_has(&r.cx, "kind: Config"));
    r.a.dc.rows_dirty = true;
    r.a.dc_prepare(&r.cx);
    let v = r.a.dc.rows.iter().find(|x| matches!(x.kind, RowKind::Vault(_))).cloned().expect("the vault row");
    assert!(chips(&v).contains(&"vault") && v.sub.contains("0600 temp file"), "{:?} {}", chips(&v), v.sub);
    // test through the runner (which swaps the reference for a temp file in real life)
    let i = r.a.dc.rows.iter().position(|x| x.id == "uvault:staging-kc").unwrap();
    r.a.handle(&Event::Row(i), &r.cx);
    press(&mut r.a, &r.cx, 't');
    settle(&mut r.a, &r.cx);
    assert!(status(&r.a).contains("https://198.51.100.7:6443") && status(&r.a).contains("node stg-1"), "{}", status(&r.a));
    assert!(r.run.calls().iter().any(|c| c.line().contains("uvault:staging-kc")), "kubectl is given the reference; the runner resolves it");
    // a vault profile is chosen like any other, and removed with V (the vault entry itself stays)
    press(&mut r.a, &r.cx, 'd');
    assert_eq!(Choice::load(&dir, "root").unwrap().kubeconfig, "uvault:staging-kc");
    press(&mut r.a, &r.cx, 'h');
    assert!(status(&r.a).contains("removed with V") || status(&r.a).contains("default"), "{}", status(&r.a));
    press(&mut r.a, &r.cx, 'V');
    assert!(r.a.has_modal());
    press(&mut r.a, &r.cx, 'y');
    assert!(kubeprofiles::vault_load(&dir, "root").is_empty());
    assert!(r.cx.services.user_names("root").unwrap().contains(&"staging-kc".to_string()));
}

#[test]
fn the_card_follows_the_situation_and_setup_is_only_offered_when_no_cluster_exists() {
    let mut r = rig("dc-situation");
    r.open();
    let k = r.cx.services.k3sdev.clone();
    let machine = |d: &Path| Machine { k3s_yaml: d.join("etc/k3s.yaml"), lib_dir: d.join("lib/rancher"), bins: vec![d.join("bin/k3s")], units: vec![], proc_dir: d.join("proc") };
    let m = machine(&r.dir);
    std::fs::create_dir_all(&m.proc_dir).unwrap();
    let set = |root: bool, window: bool| k.assume(root, true, false, window, m.clone(), Duration::from_millis(10), Duration::from_millis(20));
    let has_setup = |r: &mut Rig| {
        r.a.dc.sit = None;
        let g = draw(&mut r.a, &r.cx, 1280, 720);
        g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('s')))
    };
    k.assume_connected(None);
    // free and root: offered
    set(true, false);
    assert!(has_setup(&mut r), "{:?}", k.situation());
    // not root: the card says why and offers nothing
    set(false, false);
    assert!(!has_setup(&mut r));
    press(&mut r.a, &r.cx, 's');
    assert!(status(&r.a).contains("not running as root"), "{}", status(&r.a));
    // a desktop window never installs
    set(true, true);
    assert!(!has_setup(&mut r));
    press(&mut r.a, &r.cx, 's');
    assert!(status(&r.a).contains("desktop window"), "{}", status(&r.a));
    assert!(matches!(k.situation(), Situation::Window { found } if found.is_empty()));
    // a cluster that is not IYAKKI's: the page says so and manages profiles only
    set(true, false);
    std::fs::create_dir_all(m.k3s_yaml.parent().unwrap()).unwrap();
    std::fs::write(&m.k3s_yaml, "x").unwrap();
    assert!(!has_setup(&mut r));
    press(&mut r.a, &r.cx, 's');
    assert!(status(&r.a).contains("already has a cluster") && status(&r.a).contains("Nothing was changed"), "{}", status(&r.a));
    assert!(matches!(k.situation(), Situation::Foreign(f) if f[0].contains("k3s.yaml")));
    set(true, true);
    assert!(matches!(k.situation(), Situation::Window { found } if !found.is_empty()), "in a window the same finding is reported");
    // the machine buttons do nothing when IYAKKI has not installed anything
    for c in ['b', 'o', 'e', 'p', 'a', 'z', 'U', 'l'] {
        press(&mut r.a, &r.cx, c);
        assert!(!r.a.has_modal() && !r.a.dc_has_overlay() && !r.a.is_busy(), "{c}: {}", status(&r.a));
    }
    // profile management still works next to a foreign cluster
    r.select(&r.k3s.clone());
    press(&mut r.a, &r.cx, 'd');
    assert!(status(&r.a).contains("your default cluster"));
}

struct FakeNet(std::sync::Mutex<std::collections::HashMap<String, (u16, Vec<(String, String)>, Vec<u8>)>>);
impl iyakki_agent::iyakki_proxy::Streamer for FakeNet {
    fn open(&self, url: &str, _h: &[(String, String)]) -> Result<StreamHead, String> {
        let r = self.0.lock().unwrap().get(url).cloned().ok_or_else(|| format!("no route for {url}"))?;
        let mut headers = r.1.clone();
        headers.push(("content-length".into(), r.2.len().to_string()));
        Ok(StreamHead { status: r.0, headers, body: Box::new(Cursor::new(r.2)) })
    }
}

/// A stand-in `k3s`: `server` writes the kubeconfig it is asked for and stays; any `kubectl` answers `True`.
const STAND_IN: &str = "#!/bin/sh\ncase \"$1\" in\nserver)\n  kc=\n  while [ $# -gt 0 ]; do if [ \"$1\" = \"--write-kubeconfig\" ]; then kc=\"$2\"; fi; shift; done\n  echo \"stand-in k3s started\"\n  printf 'apiVersion: v1\\n' > \"$kc\"\n  exec sleep 600;;\nkubectl) echo True;;\nesac\n";

fn net(body: &str) -> Arc<FakeNet> {
    let (url, sum_url, file) = k3sdev::release_urls("v1.31.1+k3s1").unwrap();
    let hash = iyakki_agent::clis::sha256_hex(body.as_bytes());
    let mut m = std::collections::HashMap::new();
    m.insert("https://github.com/k3s-io/k3s/releases/latest/download/k3s".to_string(), (302, vec![("location".to_string(), "https://github.com/k3s-io/k3s/releases/download/v1.31.1%2Bk3s1/k3s".to_string())], vec![]));
    m.insert(url, (200, vec![], body.as_bytes().to_vec()));
    m.insert(sum_url, (200, vec![], format!("{hash}  {file}\n").into_bytes()));
    Arc::new(FakeNet(std::sync::Mutex::new(m)))
}

#[test]
fn set_up_a_local_cluster_shows_the_plan_asks_then_installs_starts_and_every_machine_action_is_audited() {
    let mut r = rig("dc-setup");
    let k = r.cx.services.k3sdev.clone();
    let m = Machine { k3s_yaml: r.dir.join("etc/k3s.yaml"), lib_dir: r.dir.join("lib/rancher"), bins: vec![r.dir.join("bin/k3s")], units: vec![], proc_dir: r.dir.join("proc") };
    std::fs::create_dir_all(&m.proc_dir).unwrap();
    k.assume(true, true, false, false, m, Duration::from_millis(10), Duration::from_millis(20));
    k.assume_connected(None);
    r.cx.services.set_streamer(net(STAND_IN));
    r.open();
    // 's': a short form (how to run it, which registry); Enter on the defaults
    press(&mut r.a, &r.cx, 's');
    assert!(r.a.has_modal());
    // podman is asked for explicitly and refused when it is missing
    if !iyakki_agent::wsexec::on_path("podman") {
        if let Some(Modal::Form(_, f)) = r.a.modal.as_mut() {
            f.set_choice(0, 1);
        }
        submit(&mut r.a, &r.cx);
        assert!(r.a.has_modal(), "the form stays open");
        if let Some(Modal::Form(_, f)) = &r.a.modal {
            assert!(f.error.as_deref().is_some_and(|e| e.contains("podman is not installed") && e.contains("podman packages")), "{:?}", f.error);
        }
        if let Some(Modal::Form(_, f)) = r.a.modal.as_mut() {
            f.set_choice(0, 0);
            f.error = None;
        }
    }
    submit(&mut r.a, &r.cx);
    settle(&mut r.a, &r.cx);
    assert!(r.a.dc_has_overlay(), "{}", status(&r.a));
    let plan = overlay_text(&r.a);
    for want in ["Set up a local cluster: native k3s v1.31.1+k3s1", "https://github.com/k3s-io/k3s/releases/download/v1.31.1%2Bk3s1/k3s", "sha256sum-", "k3s-dev/bin/k3s", "Reset deletes only this folder", "6443/tcp", "A, a registry pod inside the cluster", "never uses sudo"] {
        assert!(plan.contains(want), "{want} in\n{plan}");
    }
    if !iyakki_agent::wsexec::on_path("podman") {
        assert!(plan.contains("Podman variant is not offered here"), "{plan}");
    }
    // nothing is on disk before Approve
    let root = k.layout().root.clone();
    assert!(!root.exists(), "the plan only reads");
    assert!(audit_has(&r.cx, "devcluster.setup.lookup") && !audit_has(&r.cx, "devcluster.installed"));
    // Cancel leaves nothing; asking again and approving installs and starts
    key(&mut r.a, &r.cx, Key::Esc);
    assert!(!r.a.dc_has_overlay() && !root.exists());
    press(&mut r.a, &r.cx, 's');
    submit(&mut r.a, &r.cx);
    settle(&mut r.a, &r.cx);
    press(&mut r.a, &r.cx, 'y');
    settle(&mut r.a, &r.cx);
    assert!(status(&r.a).contains("Installed native k3s v1.31.1+k3s1") && status(&r.a).contains("press g"), "{}", status(&r.a));
    assert_eq!(std::fs::read_to_string(root.join("bin/k3s")).unwrap(), STAND_IN);
    let st = k.installed().unwrap();
    assert_eq!((st.version.as_str(), st.autostart, st.registry_choice.as_str()), ("v1.31.1+k3s1", true, "in-cluster"));
    until(&mut r.a, &r.cx, "the node is Ready", |_| k.state() == Svc::Running);
    assert!(root.join("kubeconfig").exists());
    for a in ["devcluster.setup", "devcluster.download", "devcluster.verified", "devcluster.installed", "devcluster.start", "devcluster.ready"] {
        assert!(audit_has(&r.cx, a), "{a} in the audit log");
    }
    // the card now has the machine buttons; geometry holds at both sizes
    for (w, h) in [(1280u32, 720u32), (1024, 600)] {
        r.a.dc.sit = None;
        let g = draw(&mut r.a, &r.cx, w, h);
        let area = R::new(0.0, 72.0, w as f32, h as f32 - 72.0);
        let keys: Vec<(char, R)> = g.hits.iter().filter_map(|x| if let Event::Key(Key::Char(c)) = x.ev { Some((c, x.rect)) } else { None }).collect();
        for want in ['o', 'e', 'l', 'p', 'a', 'z', 'U'] {
            assert!(keys.iter().any(|k| k.0 == want), "{w}x{h}: {want}");
        }
        for (i, (ca, ra)) in keys.iter().enumerate() {
            assert!(ra.x >= area.x && ra.y >= area.y && ra.x + ra.w <= area.x + area.w + 0.5 && ra.y + ra.h <= area.y + area.h + 0.5, "{w}x{h}: {ca} {ra:?} outside {area:?}");
            for (cb, rb) in keys.iter().skip(i + 1) {
                assert!(!overlaps(*ra, *rb), "{w}x{h}: {ca} and {cb} overlap");
            }
            for rr in g.hits.iter().filter(|h| matches!(h.ev, Event::Row(_))) {
                assert!(!overlaps(*ra, rr.rect), "{w}x{h}: {ca} covers a row");
            }
        }
    }
    // a second set-up is refused
    press(&mut r.a, &r.cx, 's');
    assert!(status(&r.a).contains("already manages"), "{}", status(&r.a));
    // logs: selectable text of the service log
    press(&mut r.a, &r.cx, 'l');
    assert!(overlay_text(&r.a).contains("stand-in k3s started"), "{}", overlay_text(&r.a));
    key(&mut r.a, &r.cx, Key::Esc);
    // stop: the command and what happens are shown first; approving stops it
    press(&mut r.a, &r.cx, 'o');
    assert!(overlay_text(&r.a).contains("SIGTERM") && k.state() == Svc::Running, "asked, not yet done");
    press(&mut r.a, &r.cx, 'y');
    settle(&mut r.a, &r.cx);
    assert_eq!(k.state(), Svc::Stopped);
    // start again, with the command shown
    press(&mut r.a, &r.cx, 'b');
    assert!(overlay_text(&r.a).contains("k3s-dev/bin/k3s server --data-dir"), "{}", overlay_text(&r.a));
    press(&mut r.a, &r.cx, 'y');
    settle(&mut r.a, &r.cx);
    until(&mut r.a, &r.cx, "running again", |_| k.state() == Svc::Running);
    // restart
    press(&mut r.a, &r.cx, 'e');
    press(&mut r.a, &r.cx, 'y');
    settle(&mut r.a, &r.cx);
    until(&mut r.a, &r.cx, "running after the restart", |_| k.state() == Svc::Running);
    // Autostart switch
    press(&mut r.a, &r.cx, 'a');
    assert!(!k.installed().unwrap().autostart && status(&r.a).contains("Autostart is off"));
    press(&mut r.a, &r.cx, 'a');
    assert!(k.installed().unwrap().autostart);
    // reset needs the typed name; a wrong one changes nothing
    std::fs::write(k.layout().data().join("marker"), "x").unwrap();
    press(&mut r.a, &r.cx, 'z');
    typ(&mut r.a, &r.cx, "reset");
    key(&mut r.a, &r.cx, Key::Enter);
    assert!(r.a.has_modal() && k.layout().data().join("marker").exists());
    if let Some(Modal::Form(_, f)) = r.a.modal.as_mut() {
        f.set_value(0, "iyakki-dev");
        f.error = None;
    }
    key(&mut r.a, &r.cx, Key::Enter);
    settle(&mut r.a, &r.cx);
    assert!(!k.layout().data().join("marker").exists() && k.layout().bin().exists(), "{}", status(&r.a));
    assert_eq!(k.state(), Svc::Stopped, "a running cluster is stopped before its data goes");
    assert!(audit_has(&r.cx, "devcluster.reset.done"));
    // uninstall
    press(&mut r.a, &r.cx, 'U');
    if let Some(Modal::Form(_, f)) = r.a.modal.as_mut() {
        f.set_value(0, "iyakki-dev");
    }
    key(&mut r.a, &r.cx, Key::Enter);
    settle(&mut r.a, &r.cx);
    assert!(!root.exists() && k.installed().is_none(), "{}", status(&r.a));
    assert!(audit_has(&r.cx, "devcluster.uninstall.done"));
}

#[test]
fn a_download_that_fails_its_checksum_installs_nothing_and_says_so() {
    let mut r = rig("dc-badsum");
    let k = r.cx.services.k3sdev.clone();
    let m = Machine { k3s_yaml: r.dir.join("etc/k3s.yaml"), lib_dir: r.dir.join("lib/rancher"), bins: vec![], units: vec![], proc_dir: r.dir.join("proc") };
    std::fs::create_dir_all(&m.proc_dir).unwrap();
    k.assume(true, true, false, false, m, Duration::from_millis(10), Duration::from_millis(20));
    k.assume_connected(None);
    let n = net(STAND_IN);
    r.cx.services.set_streamer(n.clone());
    r.open();
    press(&mut r.a, &r.cx, 's');
    submit(&mut r.a, &r.cx);
    settle(&mut r.a, &r.cx);
    assert!(r.a.dc_has_overlay());
    // the server now serves other bytes than the checksum file was made for
    for (u, v) in n.0.lock().unwrap().iter_mut() {
        if u.ends_with("/k3s") && u.contains("download/") {
            v.2 = b"#!/bin/sh\necho tampered\n".to_vec();
        }
    }
    press(&mut r.a, &r.cx, 'y');
    settle(&mut r.a, &r.cx);
    assert!(status(&r.a).contains("does not match the vendor's checksum") && status(&r.a).contains("nothing was installed"), "{}", status(&r.a));
    assert!(r.a.status().is_some_and(|s| !s.1));
    assert!(k.installed().is_none() && !k.layout().bin().exists());
    assert!(audit_has(&r.cx, "devcluster.checksum.mismatch"));
}

#[test]
fn the_registry_choice_shows_the_plan_first_and_changes_nothing_until_approved() {
    let mut r = rig("dc-registry");
    let ws = r.workspace("shop");
    r.run.fail("get namespace iyakki-registry", 1, "Error from server (NotFound): namespaces \"iyakki-registry\" not found");
    r.run.answer("create namespace iyakki-registry", "namespace/iyakki-registry created");
    r.run.answer("apply -f", "created");
    r.run.answer("rollout status", "deployment \"registry\" successfully rolled out");
    r.open();
    r.select(&r.k3s.clone());
    press(&mut r.a, &r.cx, 'd');
    // no workspace yet / a remote cluster for B / A on a production-looking cluster are refused with the reason
    press(&mut r.a, &r.cx, 'g');
    assert!(r.a.has_modal());
    // option B on this machine's k3s needs podman or docker: the plan says so if missing, else it is shown
    // (use A here)
    submit(&mut r.a, &r.cx);
    settle(&mut r.a, &r.cx);
    assert!(r.a.dc_has_overlay(), "{}", status(&r.a));
    let t = overlay_text(&r.a);
    for want in ["Workspace shop", "Registry option: A", "kubectl", "create namespace iyakki-registry", "Root: not needed", "k8s.registry = \"localhost:30500\""] {
        assert!(t.contains(want), "{want} in\n{t}");
    }
    let calls = r.run.lines();
    assert!(!calls.iter().any(|l| l.contains("create namespace iyakki-registry") || l.contains(" apply -f")), "the plan only reads: {calls:?}");
    assert_eq!(ProjectConfig::load(&ws).unwrap().k8s.registry_kind, "", "nothing recorded yet");
    // Cancel: still nothing
    key(&mut r.a, &r.cx, Key::Esc);
    assert!(!r.run.lines().iter().any(|l| l.contains("create namespace iyakki-registry")));
    // approve
    press(&mut r.a, &r.cx, 'g');
    submit(&mut r.a, &r.cx);
    settle(&mut r.a, &r.cx);
    press(&mut r.a, &r.cx, 'y');
    settle(&mut r.a, &r.cx);
    assert!(status(&r.a).contains("Registry option in-cluster for shop") && status(&r.a).contains("created namespace iyakki-registry"), "{}", status(&r.a));
    let k8s = ProjectConfig::load(&ws).unwrap().k8s;
    assert_eq!((k8s.registry_kind.as_str(), k8s.registry.as_str(), k8s.registry_pull.as_str()), ("in-cluster", "localhost:30500", ""), "recorded in project.json like the K8S panel does");
    assert!(audit_has(&r.cx, "devcluster.registry.plan") && audit_has(&r.cx, "devcluster.registry.apply"));
    // A refuses a production-looking cluster, B refuses a remote one
    r.select(&r.remote.clone());
    press(&mut r.a, &r.cx, 'w');
    until(&mut r.a, &r.cx, "scan", |a| a.dc.ws.first().is_some_and(|w| w.2.k8s.kubeconfig.ends_with("remote.yaml")));
    press(&mut r.a, &r.cx, 'g');
    submit(&mut r.a, &r.cx);
    assert!(r.a.has_modal(), "A on a production-looking cluster stays in the form");
    if let Some(Modal::Form(_, f)) = &r.a.modal {
        assert!(f.error.as_deref().is_some_and(|e| e.contains("looks like production") || e.contains("only reads")), "{:?}", f.error);
    }
}

#[test]
fn this_pods_service_account_is_listed_as_in_cluster_with_what_its_role_allows() {
    let mut r = rig("dc-incluster");
    let sa = r.dir.join("sa");
    std::fs::create_dir_all(&sa).unwrap();
    std::fs::write(sa.join("token"), "TOKEN").unwrap();
    std::fs::write(sa.join("ca.crt"), "CA").unwrap();
    std::fs::write(sa.join("namespace"), "iyakki").unwrap();
    let pod = kubeprofiles::PodEnv { host: Some("10.43.0.1".into()), port: Some("443".into()), token: sa.join("token"), ca: sa.join("ca.crt"), namespace: sa.join("namespace"), out: r.dir.join("incluster.yaml") };
    let kc = kubeprofiles::in_cluster_kubeconfig_for(&pod).unwrap();
    answers(&r.run, &kc, "in-cluster", "https://10.43.0.1:443", "v1.31.1+k3s1", "n1");
    r.run.answer(&format!("--kubeconfig {} --context in-cluster --request-timeout=6s auth can-i --list", kc.display()), "Resources   Non-Resource URLs   Resource Names   Verbs\npods   []   []   [get list watch]\ndeployments.apps   []   []   [get list watch create]\n");
    let env = ClusterEnv { home: r.dir.join("nohome"), extra: vec![(Source::InCluster, kc.clone())], ..Default::default() };
    r.a = SettingsApp::new(&r.cx).with_cluster(r.run.clone(), env);
    r.a.go_to(Tab::DevCluster);
    until(&mut r.a, &r.cx, "listed", |a| !a.dc.rows.is_empty());
    let row = r.row(&kc);
    let c = chips(&row);
    assert!(c.contains(&"in-cluster") && c.contains(&"remote") && c.contains(&"reachable"), "{c:?}");
    assert!(row.sub.contains("this pod's service account"), "{}", row.sub);
    assert!(row.profile.as_ref().unwrap().can_i.contains("deployments.apps: get list watch create"), "{}", row.profile.as_ref().unwrap().can_i);
}

#[test]
fn platform_services_opens_from_the_dev_cluster_page_owns_the_keyboard_and_fits_at_four_sizes() {
    let mut r = rig("dc-platform");
    r.open();
    // the button is on the page and a click opens the panel
    let g = draw(&mut r.a, &r.cx, 1280, 720);
    let hit = g.hits.iter().rev().find(|h| h.label == "Platform services").expect("the Platform services button").ev.clone();
    r.a.handle(&hit, &r.cx);
    assert!(r.a.dc_platform_open());
    // the page's own button bar and keys are not live while the panel is open
    for (w, h) in [(1280, 720), (1920, 1080), (1024, 600), (800, 480)] {
        let g = draw(&mut r.a, &r.cx, w, h);
        assert!(g.hits.iter().all(|x| x.label != "Disconnect" && x.label != "My default"), "{w}x{h}: the page's buttons are hidden");
        for hit in g.hits.iter().filter(|x| !matches!(x.ev, Event::Tab(_))) {
            let q = hit.rect;
            assert!(q.x >= -1.0 && q.x + q.w <= w as f32 + 1.0 && q.y + q.h <= h as f32 + 1.0, "{w}x{h}: {} sticks out {q:?}", hit.label);
        }
    }
    let before = Choice::load(r.cx.services.dir(), "root").is_some();
    press(&mut r.a, &r.cx, 'x'); // x is Disconnect on the page: here it is Remove on the panel, which has nothing selected to remove
    press(&mut r.a, &r.cx, 'd');
    assert_eq!(Choice::load(r.cx.services.dir(), "root").is_some(), before, "no page key reached the profile list");
    // Esc closes the panel only; a second Esc leaves Settings
    let f = r.a.handle(&Event::Key(Key::Esc), &r.cx);
    assert!(matches!(f, Flow::Stay));
    assert!(r.a.dc_platform_open(), "Esc went back from the wizard to the list first");
    r.a.handle(&Event::Key(Key::Esc), &r.cx);
    assert!(!r.a.dc_platform_open() && r.a.tab() == Tab::DevCluster);
    assert!(matches!(r.a.handle(&Event::Key(Key::Esc), &r.cx), Flow::Back));
}

#[test]
fn the_private_store_card_hands_the_shell_a_link_that_opens_the_deploy_wizard_with_that_service() {
    use super::App;
    let mut r = rig("dc-private-link");
    r.open();
    assert!(!r.a.dc_platform_open());
    r.a.open_item("something-else", &r.cx);
    assert!(!r.a.dc_platform_open());
    r.a.open_item("platform:private-store", &r.cx);
    assert!(r.a.tab() == Tab::DevCluster && r.a.dc_platform_open());
    // straight into the wizard (not the list): Esc goes back to the list first
    assert!(!matches!(r.a.dc.platform.as_ref().unwrap().page(), crate::apps::platform_services::Page::List));
}

#[test]
fn the_store_link_for_repair_reset_and_remove_opens_the_panel_on_the_store_not_the_deploy_options() {
    use super::App;
    let mut r = rig("dc-private-manage");
    r.open();
    r.a.open_item("platform:private-store:manage", &r.cx);
    assert!(r.a.tab() == Tab::DevCluster && r.a.dc_platform_open());
    // no store is recorded here, so the cluster is chosen first, and the wizard is the one that looks after the store
    assert!(matches!(r.a.dc.platform.as_ref().unwrap().page(), crate::apps::platform_services::Page::Cluster));
}
