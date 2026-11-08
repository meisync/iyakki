//! Verify before mark, the record of a failed last attempt, and what a card says about it: fakes and temporary folders only. No cluster,
//! no network, no install.

use super::*;
use crate::quickinstall::verify as v;

struct Tmp(PathBuf);
impl Tmp {
    fn new(tag: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("iyakki-qiv-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Tmp(d)
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A step environment where the install itself works and the independent check says what the test wants.
#[derive(Default)]
struct F {
    verify_says: Option<Result<String, String>>,
    calls: Vec<String>,
    marks: Vec<(Item, String)>,
    notes: Vec<String>,
    lines: Vec<String>,
}

impl StepEnv for F {
    fn say(&mut self, line: &str) {
        self.lines.push(line.into());
    }
    fn dep_state(&mut self, _: Item) -> DepState {
        DepState::Ready
    }
    fn k3s_present(&mut self) -> bool {
        true
    }
    fn k3s_elsewhere(&mut self) -> Option<String> {
        None
    }
    fn install_k3s(&mut self) -> Result<String, String> {
        Err("not here".into())
    }
    fn request_k3s_start(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn cluster_ready(&mut self) -> bool {
        true
    }
    fn install_cli(&mut self, id: &str) -> Result<String, String> {
        self.calls.push(format!("cli {id}"));
        Ok("1.3.2".into())
    }
    fn kubectl_apply(&mut self, _: &str, _: &str) -> Result<String, String> {
        Ok(String::new())
    }
    fn rollout(&mut self, ns: &str, name: &str, _: Duration) -> Result<(), String> {
        self.calls.push(format!("rollout {ns}/{name}"));
        Ok(())
    }
    fn sleep(&mut self, _: Duration) {}
    fn mark(&mut self, item: Item, detail: &str) -> Result<(), String> {
        self.calls.push(format!("mark {}", item.id()));
        self.marks.push((item, detail.into()));
        Ok(())
    }
    fn request_after(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn lemonade_ready(&mut self) -> bool {
        true
    }
    fn lemonade_pull(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn verify(&mut self, item: Item, _detail: &str) -> Result<String, String> {
        self.calls.push(format!("verify {}", item.id()));
        self.verify_says.clone().unwrap_or_else(|| Ok(String::new()))
    }
    fn set_verified_note(&mut self, note: &str) {
        self.notes.push(note.into());
    }
}

const WAIT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_secs(5);

#[test]
fn the_check_runs_before_the_marker_and_what_it_saw_goes_with_the_marker() {
    let mut f = F { verify_says: Some(Ok("verified: `duckdb` ran and said \"v1.3.2\"".into())), ..Default::default() };
    run_step(Item::DuckDb, &mut f, WAIT, POLL).unwrap();
    let verify_at = f.calls.iter().position(|c| c == "verify duckdb").unwrap();
    let mark_at = f.calls.iter().position(|c| c == "mark duckdb").unwrap();
    assert!(verify_at < mark_at, "verify first: {:?}", f.calls);
    assert_eq!(f.notes, ["verified: `duckdb` ran and said \"v1.3.2\""]);
    assert!(f.lines.iter().any(|l| l.contains("checked: verified")), "{:?}", f.lines);
}

#[test]
fn a_failed_check_fails_the_step_with_the_reason_and_writes_no_marker() {
    for item in [Item::DuckDb, Item::Node, Item::Elasticsearch, Item::Prometheus, Item::Lemonade, Item::Kokoro] {
        let mut f = F { verify_says: Some(Err("it does not answer".into())), ..Default::default() };
        let e = run_step(item, &mut f, WAIT, POLL).unwrap_err();
        assert!(e.starts_with("Verify failed: it does not answer") && e.contains("is not marked as installed"), "{item:?}: {e}");
        assert!(f.marks.is_empty(), "{item:?} must not be marked: {:?}", f.marks);
        assert_eq!(super::quickinstall_cause::classify(&e), super::quickinstall_cause::Cause::VerifyFailed);
    }
}

#[test]
fn an_item_without_a_check_is_marked_as_before() {
    let mut f = F::default();
    run_step(Item::Python, &mut f, WAIT, POLL).unwrap();
    assert_eq!(f.marks, [(Item::Python, "version 1.3.2".to_string())]);
    assert_eq!(f.notes, [""], "nothing was seen, nothing is claimed");
}

#[test]
fn the_deployment_check_reads_ready_pods_and_a_service_with_an_address() {
    assert_eq!(v::ready_from("1 1"), Ok((1, 1)));
    assert!(v::ready_from("1").unwrap_err().contains("0 of 1 pod(s) are Ready"));
    assert!(v::ready_from("2 1").unwrap_err().contains("1 of 2"));
    assert!(v::ready_from("0 0").unwrap_err().contains("scaled to zero"));
    assert_eq!(v::endpoints_from(true, "10.42.0.7"), Ok(1));
    assert!(v::endpoints_from(true, "").unwrap_err().contains("no address"));
    assert!(v::endpoints_from(false, "").unwrap_err().contains("does not exist"));
    for it in [Item::Elasticsearch, Item::GraphDb, Item::Prometheus, Item::OpenSearch, Item::ClickHouse, Item::Matrix, Item::SeaweedFs, Item::RustFs, Item::Gitea, Item::Ollama, Item::Lemonade] {
        assert!(v::target(it).is_some(), "{it:?}");
    }
    // the deployments the card's health reading knows are the ones this check looks at
    for it in Item::ALL {
        if let Some((ns, name)) = cluster_object(it) {
            let t = v::target(it).unwrap();
            assert_eq!((t.0, t.1), (ns, name), "{it:?}");
        }
    }
    assert!(v::deployment_note("graphdb", "graphdb", 1, 1).starts_with("verified: graphdb has 1 Ready pod"));
}

#[test]
fn a_failed_attempt_is_recorded_shown_in_the_presence_and_cleared_by_a_success() {
    let t = Tmp::new("record");
    let up: std::sync::Arc<dyn iyakki_proxy::Upstream> = std::sync::Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(2)));
    let svc = crate::services::Services::open(&t.0, up.clone(), up).unwrap();
    let none = || None;
    // the registry step has only a marker: an old install
    mark_done(&t.0, Item::Registry, "pods pull localhost:30500").unwrap();
    assert_eq!(presence_with(&svc, Item::Registry, &none), Presence::Installed("pods pull localhost:30500".into()));
    // a Retry fails: the old marker no longer says "installed"
    assert_eq!(failed_exit(&t.0, Item::Registry, "network problem: no data from ghcr.io for 60 s (the connection went quiet)"), 1);
    let (at, why) = last_failure(&t.0, Item::Registry).unwrap();
    assert!(at > 0 && why.starts_with("Network problem: no data from ghcr.io for 60 s"), "{why}");
    let p = presence_with(&svc, Item::Registry, &none);
    assert!(matches!(&p, Presence::Earlier(d) if d.starts_with(LAST_FAILED)), "{p:?}");
    assert_eq!(p.word(), "installed earlier; the last attempt failed: Network problem: no data from ghcr.io for 60 s (the connection went quiet)");
    assert!(p.present(), "the old install is still there (Remove works)");
    assert_eq!(failure_reason(&p), Some("Network problem: no data from ghcr.io for 60 s (the connection went quiet)"));
    // the plain reading (what is there, whatever the newest attempt did) is still available
    assert_eq!(presence_plain_with(&svc, Item::Registry, &none), Presence::Installed("pods pull localhost:30500".into()));
    // nothing installed before: a failure leaves "not installed"
    record_failure(&t.0, Item::Matrix, "Timed out: x");
    assert_eq!(presence_with(&svc, Item::Matrix, &none), Presence::Missing);
    // a success clears the record
    mark_done(&t.0, Item::Registry, "pods pull localhost:30500").unwrap();
    assert!(last_failure(&t.0, Item::Registry).is_none());
    assert_eq!(presence_with(&svc, Item::Registry, &none), Presence::Installed("pods pull localhost:30500".into()));
    // so does removing the item
    record_failure(&t.0, Item::Registry, "Other: x");
    unmark(&t.0, Item::Registry);
    assert!(last_failure(&t.0, Item::Registry).is_none());
}

#[test]
fn the_marker_keeps_the_note_on_a_line_of_its_own_and_the_detail_reads_as_before() {
    let t = Tmp::new("note");
    mark_done_note(&t.0, Item::Registry, "pods pull x", "verified: it answers").unwrap();
    assert_eq!(marked(&t.0, Item::Registry).as_deref(), Some("pods pull x"));
    assert_eq!(marked_note(&t.0, Item::Registry).as_deref(), Some("verified: it answers"));
    mark_done(&t.0, Item::Registry, "pods pull x").unwrap();
    assert_eq!(marked_note(&t.0, Item::Registry), None);
}

#[test]
fn a_timeout_exit_line_and_the_record_carry_the_class_first() {
    let t = Tmp::new("timeout");
    failed_exit(&t.0, Item::K3s, "Install k3s timed out: it ran for 120 minutes without finishing (it is not marked as installed; see the log for the last step, then press Retry)");
    let (_, why) = last_failure(&t.0, Item::K3s).unwrap();
    assert!(why.starts_with("Timed out: "), "{why}");
    assert!(!why.contains('\n'));
}

#[test]
fn the_add_on_check_names_the_source_and_fails_when_a_library_or_licence_file_is_missing() {
    let t = Tmp::new("addon");
    let sonames = vec!["libavcodec.so.61".to_string(), "libavutil.so.59".to_string()];
    let dir = crate::avaddon::addon_dir(&t.0);
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    for s in &sonames {
        std::fs::write(dir.join("lib").join(s), vec![7u8; 4096]).unwrap();
    }
    let e = crate::avaddon_install::verify_installed(&t.0, &sonames, "FFmpeg 7 from /x.tar.xz").unwrap_err();
    assert!(e.contains("fails its check"), "{e}");
    for f in crate::avaddon::LEGAL_FILES {
        std::fs::write(dir.join(f), "x").unwrap();
    }
    let ok = crate::avaddon_install::verify_installed(&t.0, &sonames, "FFmpeg 7 from /x.tar.xz").unwrap();
    assert!(ok.contains("2 library file(s)") && ok.ends_with("source: supplied"), "{ok}");
    assert!(crate::avaddon_install::verify_installed(&t.0, &sonames, "libraries of the installed ffmpeg").unwrap().ends_with("source: existing ffmpeg"));
    std::fs::write(dir.join("BUILD-STAMP"), "entry x\n").unwrap();
    assert!(crate::avaddon_install::verify_installed(&t.0, &sonames, "FFmpeg 7 from /x.tar.xz").unwrap().ends_with("source: built here"));
    // a library that is gone fails the check
    std::fs::remove_file(dir.join("lib/libavutil.so.59")).unwrap();
    assert!(crate::avaddon_install::verify_installed(&t.0, &sonames, "x").unwrap_err().contains("libavutil.so.59"));
}

#[test]
fn a_firmware_pack_is_read_back_from_the_data_volume_before_it_counts_as_installed() {
    use iyakki_drivers::{apply, b64, pack, run::Recorder};
    let t = Tmp::new("fw");
    std::fs::create_dir_all(t.0.join("proc/sys/kernel")).unwrap();
    std::fs::write(t.0.join("proc/sys/kernel/osrelease"), "6.12.1\n").unwrap();
    std::fs::write(t.0.join("proc/modules"), "").unwrap();
    let fs = apply::Fs::under(&t.0);
    let p = pack::Pack { v: 1, files: vec![pack::FwFile { path: "brcm/a.hcd".into(), data: b64::encode(b"HCD-BYTES"), sha256: String::new() }], ..Default::default() };
    let rep = crate::fwpack::install_checked(&fs, &Recorder::default(), "t-fw", "1", "x", "test", &p).unwrap();
    assert!(rep.lines.iter().any(|l| l.starts_with("verified: 1 file(s) on the data volume match their SHA-256")), "{:?}", rep.lines);
    // a file that changed on disk is caught
    std::fs::write(fs.pack_dir("t-fw").join("files/brcm/a.hcd"), b"OTHER").unwrap();
    let e = crate::fwpack::verify_pack_files(&fs, "t-fw", &p).unwrap_err();
    assert!(e.starts_with("verify failed:") && e.contains("does not match"), "{e}");
    // a missing file is caught
    std::fs::remove_file(fs.pack_dir("t-fw").join("files/brcm/a.hcd")).unwrap();
    assert!(crate::fwpack::verify_pack_files(&fs, "t-fw", &p).unwrap_err().starts_with("verify failed:"));
    // no record at all
    assert!(crate::fwpack::verify_pack_files(&fs, "other", &p).unwrap_err().contains("record"));
}
