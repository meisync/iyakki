use crate::k8s::Kube;
use crate::linuxpod::*;
use std::path::Path;
use crate::wsexec::Out;
use std::sync::Mutex;

/// A tar of (name, kind, body, mode) with correct checksums, like the root a sandbox archives.
pub(crate) fn tar(entries: &[(&str, u8, &str, u64)]) -> Vec<u8> {
    let mut t = Vec::new();
    for (name, kind, body, mode) in entries {
        let (size, link) = if *kind == b'2' { (0, *body) } else { (body.len(), "") };
        let mut h = [0u8; 512];
        h[..name.len().min(100)].copy_from_slice(&name.as_bytes()[..name.len().min(100)]);
        h[100..108].copy_from_slice(format!("{mode:07o}\0").as_bytes());
        h[108..116].copy_from_slice(b"0001750\0");
        h[116..124].copy_from_slice(b"0001750\0");
        h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
        h[136..148].copy_from_slice(b"14000000000\0");
        h[156] = *kind;
        h[157..157 + link.len()].copy_from_slice(link.as_bytes());
        h[257..263].copy_from_slice(b"ustar\0");
        h[148..156].copy_from_slice(b"        ");
        let sum: u32 = h.iter().map(|b| *b as u32).sum();
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        t.extend_from_slice(&h);
        if size > 0 {
            t.extend_from_slice(body.as_bytes());
            t.extend(std::iter::repeat_n(0u8, size.div_ceil(512) * 512 - size));
        }
    }
    t.extend(std::iter::repeat_n(0u8, 1024));
    t
}

pub(crate) fn gzip_stored(data: &[u8]) -> Vec<u8> {
    let mut g = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3];
    let chunks: Vec<&[u8]> = data.chunks(60000).collect();
    for (i, c) in chunks.iter().enumerate() {
        g.push(u8::from(i + 1 == chunks.len()));
        g.extend_from_slice(&(c.len() as u16).to_le_bytes());
        g.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        g.extend_from_slice(c);
    }
    g.extend_from_slice(&iyakki_archive::xz::crc32(data).to_le_bytes());
    g.extend_from_slice(&(data.len() as u32).to_le_bytes());
    g
}

/// A small root: a shell, a sleep, a set-user-id program, a device (left out) and a link.
pub(crate) fn root() -> Vec<u8> {
    tar(&[("bin/", b'5', "", 0o755), ("bin/sh", b'0', "#!shell", 0o755), ("bin/sleep", b'0', "sleep", 0o755), ("bin/su", b'0', "su", 0o4755), ("dev/null", b'3', "", 0o666), ("etc/", b'5', "", 0o755), ("etc/hostname", b'0', "box", 0o644), ("bin/ash", b'2', "sh", 0o777)])
}

pub(crate) fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-linuxpod-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A cluster that answers the pod's phase from a script and keeps what it was asked and the file it was asked to apply.
#[derive(Default)]
pub(crate) struct Spy {
    pub calls: Mutex<Vec<String>>,
    pub applied: Mutex<Vec<String>>,
    pub phases: Mutex<Vec<&'static str>>,
    pub refuse_apply: bool,
    pub waiting_reason: &'static str,
    pub pods: &'static str,
    pub has_pod: bool,
}

impl Kube for Spy {
    fn run(&self, a: &[String]) -> Out {
        let line = a.join(" ");
        self.calls.lock().unwrap().push(line.clone());
        let ok = |s: &str| Out { code: Some(0), stdout: s.into(), ..Default::default() };
        if a.first().map(String::as_str) == Some("apply") {
            let file = a.last().unwrap();
            self.applied.lock().unwrap().push(std::fs::read_to_string(file).unwrap_or_default());
            return if self.refuse_apply { Out { code: Some(1), stderr: "admission webhook denied the request\nforbidden".into(), ..Default::default() } } else { ok("applied") };
        }
        if line.contains("jsonpath={.status.phase}") {
            let mut p = self.phases.lock().unwrap();
            let v = if p.len() > 1 { p.remove(0) } else { p.first().copied().unwrap_or("") };
            return ok(v);
        }
        if line.contains("waiting.reason") {
            return ok(self.waiting_reason);
        }
        if line.contains("get pods -o name") {
            return ok(self.pods);
        }
        if line.contains("get pod ") && line.ends_with("-o name") {
            return if self.has_pod { ok("pod/x") } else { Out { code: Some(1), stderr: "not found".into(), ..Default::default() } };
        }
        ok("")
    }
}

/// An importer that keeps the layout it was given.
#[derive(Default)]
pub(crate) struct Keep {
    pub seen: Mutex<Vec<Vec<u8>>>,
    pub fail: Option<&'static str>,
}

impl Importer for Keep {
    fn import(&self, file: &Path) -> Result<(), String> {
        self.seen.lock().unwrap().push(std::fs::read(file).map_err(|e| e.to_string())?);
        match self.fail {
            Some(m) => Err(m.into()),
            None => Ok(()),
        }
    }
}

pub(crate) fn spec(name: &str) -> PodSpec {
    PodSpec { user: "Admin".into(), name: name.into(), arch: "amd64".into(), entrypoint: vec![], cmd: vec!["/bin/sh".into()], workdir: "/".into(), env: vec![] }
}

fn entries(layout: &[u8]) -> Vec<String> {
    let mut at = 0;
    let mut v = vec![];
    while at + 512 <= layout.len() && layout[at..at + 512].iter().any(|b| *b != 0) {
        let h = &layout[at..at + 512];
        let end = h[..100].iter().position(|c| *c == 0).unwrap_or(100);
        let size = usize::from_str_radix(std::str::from_utf8(&h[124..135]).unwrap().trim_matches(|c: char| c == '\0' || c == ' '), 8).unwrap();
        v.push(String::from_utf8_lossy(&h[..end]).into_owned());
        at += 512 + size.div_ceil(512) * 512;
    }
    v
}

#[test]
fn an_archived_root_becomes_an_imported_image_and_a_locked_down_pod_that_runs() {
    let d = tmp("start");
    let spy = Spy { phases: Mutex::new(vec!["Pending", "Pending", "Running"]), ..Default::default() };
    let imp = Keep::default();
    let slept = Mutex::new(0);
    let s = start(&spy, &imp, &gzip_stored(&root()), &spec("dev"), &d, 10, &|| *slept.lock().unwrap() += 1).unwrap();
    // the image: one OCI layout the cluster's containerd can import, named for the person and the sandbox
    let layout = imp.seen.lock().unwrap()[0].clone();
    let names = entries(&layout);
    assert!(names.contains(&"oci-layout".to_string()) && names.contains(&"index.json".to_string()) && names.iter().filter(|n| n.starts_with("blobs/sha256/")).count() == 3, "{names:?}");
    let text = String::from_utf8_lossy(&layout);
    assert!(text.contains(&s.image) && s.image.starts_with("iyakki.local/linuxapp-admin-dev:"), "{}", s.image);
    // the rewrite: a device left out, the set-user-id bit cleared
    assert_eq!((s.report.files, s.report.setid_cleared, s.report.dropped.len()), (4, 1, 1), "{:?}", s.report);
    // the pod: never pulls, no token, no privilege, all capabilities dropped, not root
    let applied = spy.applied.lock().unwrap()[0].clone();
    let docs: Vec<serde_json::Value> = applied.split("\n---\n").map(|d| serde_json::from_str(d.trim()).unwrap()).collect();
    assert_eq!(docs[0]["kind"], "Namespace");
    let pod = &docs[1];
    assert_eq!(pod["metadata"]["name"], "linuxapp-admin-dev");
    assert_eq!(pod["spec"]["containers"][0]["image"], s.image.as_str());
    assert_eq!(pod["spec"]["containers"][0]["imagePullPolicy"], "Never");
    assert_eq!(pod["spec"]["containers"][0]["securityContext"]["runAsNonRoot"], true);
    assert_eq!(pod["spec"]["containers"][0]["securityContext"]["capabilities"]["drop"][0], "ALL");
    assert_eq!((pod["spec"]["automountServiceAccountToken"].clone(), pod["spec"]["hostNetwork"].clone(), pod["spec"]["containers"][0]["securityContext"]["privileged"].clone()), (false.into(), false.into(), false.into()));
    // it waited for the pod to run, then named it and the command that runs a program in it
    assert_eq!(*slept.lock().unwrap(), 2);
    assert_eq!((s.pod.as_str(), s.exec.as_str()), ("linuxapp-admin-dev", "kubectl exec -n iyakki-apps linuxapp-admin-dev -- /bin/sh"));
    // nothing is left in the work folder
    assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);
}

#[test]
fn a_bad_name_a_bad_archive_and_an_unsafe_root_are_refused_before_the_cluster_hears_of_it() {
    let d = tmp("refuse");
    let spy = Spy::default();
    let imp = Keep::default();
    for bad in ["", "Dev", "a b", "-x", "x-", &"a".repeat(31), "../x"] {
        assert!(start(&spy, &imp, &root(), &spec(bad), &d, 3, &|| {}).unwrap_err().contains("lower-case"), "{bad}");
    }
    for (bytes, why) in [(vec![], "empty"), (b"not an archive at all".to_vec(), "neither"), (vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3, 1, 2, 3], "readable")] {
        let e = start(&spy, &imp, &bytes, &spec("dev"), &d, 3, &|| {}).unwrap_err();
        assert!(e.contains(why), "{e}");
    }
    let unsafe_root = tar(&[("../escape", b'0', "x", 0o644)]);
    assert!(start(&spy, &imp, &unsafe_root, &spec("dev"), &d, 3, &|| {}).is_err());
    let no_cmd = PodSpec { cmd: vec![], ..spec("dev") };
    assert!(start(&spy, &imp, &root(), &no_cmd, &d, 3, &|| {}).unwrap_err().contains("entry point or a command"));
    let arm = PodSpec { arch: "riscv".into(), ..spec("dev") };
    assert!(start(&spy, &imp, &root(), &arm, &d, 3, &|| {}).is_err());
    assert!(spy.calls.lock().unwrap().is_empty() && imp.seen.lock().unwrap().is_empty(), "nothing reached the cluster");
}

#[test]
fn a_failed_import_a_refused_pod_and_a_pod_that_never_runs_are_said_plainly_and_cleaned_up() {
    let d = tmp("fail");
    // the import fails: no pod is applied and the layout file is gone
    let spy = Spy::default();
    let imp = Keep { fail: Some("ctr: no space left on device"), ..Default::default() };
    let e = start(&spy, &imp, &root(), &spec("dev"), &d, 3, &|| {}).unwrap_err();
    assert!(e.contains("could not import") && e.contains("no space left"), "{e}");
    assert!(spy.calls.lock().unwrap().is_empty());
    assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);
    // the cluster refuses the pod
    let spy = Spy { refuse_apply: true, ..Default::default() };
    let e = start(&spy, &Keep::default(), &root(), &spec("dev"), &d, 3, &|| {}).unwrap_err();
    assert!(e.contains("refused the pod") && e.contains("forbidden"), "{e}");
    assert!(!spy.calls.lock().unwrap().iter().any(|c| c.contains("get pod")), "no waiting after a refusal");
    // the pod never runs: it is removed and the reason is named
    let spy = Spy { phases: Mutex::new(vec!["Pending"]), waiting_reason: "ErrImageNeverPull", ..Default::default() };
    let looks = Mutex::new(0);
    let e = start(&spy, &Keep::default(), &root(), &spec("dev"), &d, 4, &|| *looks.lock().unwrap() += 1).unwrap_err();
    assert!(e.contains("did not start") && e.contains("Pending") && e.contains("ErrImageNeverPull") && e.contains("removed"), "{e}");
    assert_eq!(*looks.lock().unwrap(), 4, "the waiting is bounded");
    assert!(spy.calls.lock().unwrap().iter().any(|c| c.contains("delete pod linuxapp-admin-dev")));
    assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);
}

#[test]
fn a_pod_is_stopped_by_name_only_when_it_is_there_and_the_list_is_the_persons_own() {
    let spy = Spy { has_pod: true, pods: "pod/linuxapp-admin-dev\npod/linuxapp-admin-web\npod/linuxapp-bob-dev\npod/other", ..Default::default() };
    assert_eq!(pods_of(&spy, "Admin"), vec!["linuxapp-admin-dev".to_string(), "linuxapp-admin-web".to_string()]);
    assert_eq!(stop(&spy, "Admin", "dev").unwrap(), "linuxapp-admin-dev");
    assert!(spy.calls.lock().unwrap().iter().any(|c| c.contains("delete pod linuxapp-admin-dev")));
    assert!(stop(&spy, "Admin", "Bad Name").is_err());
    let none = Spy::default();
    assert!(stop(&none, "Admin", "dev").unwrap_err().contains("no pod"));
    assert!(!none.calls.lock().unwrap().iter().any(|c| c.contains("delete")));
}
