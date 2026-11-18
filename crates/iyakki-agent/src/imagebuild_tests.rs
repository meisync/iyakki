use std::sync::Mutex;

use super::*;
use crate::wsexec::Out;

const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn spec() -> BuildSpec {
    BuildSpec { repo: "my-notes".into(), tag: "1".into(), dockerfile: "Containerfile".into(), builder_image: format!("quay.io/buildah/stable@{DIGEST}"), registry: "localhost:30500".into() }
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-imgbuild-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn put(d: &Path, rel: &str, text: &str) {
    let p = d.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn names(tar: &[u8]) -> Vec<String> {
    let mut at = 0;
    let mut v = vec![];
    while at + 512 <= tar.len() && tar[at..at + 512].iter().any(|b| *b != 0) {
        let h = &tar[at..at + 512];
        let end = h[..100].iter().position(|b| *b == 0).unwrap_or(100);
        v.push(String::from_utf8_lossy(&h[..end]).into_owned());
        let size = usize::from_str_radix(String::from_utf8_lossy(&h[124..135]).trim_matches('\0').trim(), 8).unwrap();
        at += 512 + size.div_ceil(512) * 512;
    }
    v
}

#[test]
fn the_folder_is_packed_without_secrets_git_or_links_and_needs_its_dockerfile() {
    let d = tmp("pack");
    put(&d, "Containerfile", "FROM scratch\n");
    put(&d, "app/main.py", "print(1)\n");
    put(&d, ".env", "TOKEN=abc\n");
    put(&d, "deploy.key", "secret");
    put(&d, ".git/config", "x");
    put(&d, "node_modules/x/index.js", "x");
    std::os::unix::fs::symlink("/etc/passwd", d.join("link")).unwrap();
    let c = package_context(&d, "Containerfile").unwrap();
    assert_eq!(names(&c.tar), ["Containerfile", "app/main.py"]);
    assert_eq!((c.files, c.bytes), (2, "FROM scratch\n".len() + "print(1)\n".len()));
    let s = c.skipped.join(" | ");
    assert!(s.contains(".env (looks like a secret)") && s.contains("deploy.key (looks like a secret)") && s.contains(".git (not part of a build)") && s.contains("node_modules") && s.contains("link (a link or special file)"), "{s}");
    assert!(package_context(&d, "Dockerfile").unwrap_err().contains("no Dockerfile"));
    assert!(package_context(&d, "../Containerfile").is_err() && package_context(&d, "/etc/passwd").is_err());
    put(&d, "big.bin", &"x".repeat(MAX_CONTEXT));
    assert!(package_context(&d, "Containerfile").unwrap_err().contains("at most"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_job_has_no_token_a_deadline_limits_and_a_fixed_script_and_a_bad_value_is_refused_before_anything_is_written() {
    let d = tmp("job");
    put(&d, "Containerfile", "FROM scratch\n");
    let c = package_context(&d, "Containerfile").unwrap();
    let m = manifests(&spec(), &c, "a1b2").unwrap();
    for want in ["\"kind\": \"ConfigMap\"", "\"kind\": \"Job\"", "\"name\": \"build-a1b2\"", "\"automountServiceAccountToken\": false", "\"activeDeadlineSeconds\": 900", "\"ttlSecondsAfterFinished\": 300", "\"memory\": \"2Gi\"", "localhost:30500/my-notes:1", "--digestfile", "IYAKKI-DIGEST=", "\"namespace\": \"iyakki-build\""] {
        assert!(m.contains(want), "{want}\\n{m}");
    }
    assert!(!m.contains("privileged") && !m.contains("hostPath") && !m.contains("hostNetwork"), "no host access");
    // the context travels in the ConfigMap and decodes to the same tar
    let cm: serde_json::Value = serde_json::from_str(m.split("\n---\n").nth(1).unwrap()).unwrap();
    let b64 = cm["binaryData"]["context.tar"].as_str().unwrap();
    assert_eq!(crate::models::b64_decode(b64).unwrap(), c.tar);
    let bad = |f: &dyn Fn(&mut BuildSpec)| {
        let mut s = spec();
        f(&mut s);
        manifests(&s, &c, "a1b2").unwrap_err()
    };
    assert!(bad(&|s| s.builder_image = "quay.io/buildah/stable:latest".into()).contains("builder image"));
    assert!(bad(&|s| s.builder_image = format!("quay.io/buildah/stable@sha256:{}", "0".repeat(64))).contains("real digest"));
    assert!(bad(&|s| s.repo = "My App".into()).contains("image name"));
    assert!(bad(&|s| s.tag = "a b".into()).contains("tag"));
    assert!(bad(&|s| s.registry = "nowhere".into()).contains("host:port"));
    assert!(bad(&|s| s.dockerfile = "x'; rm -rf /; '".into()).contains("shell"));
    assert!(manifests(&spec(), &c, "BAD ID").is_err());
    let _ = std::fs::remove_dir_all(&d);
}

struct Fake {
    calls: Mutex<Vec<String>>,
    looks: Mutex<Vec<&'static str>>,
    log: String,
    apply_ok: bool,
}

impl Kube for Fake {
    fn run(&self, args: &[String]) -> Out {
        let line = args.join(" ");
        self.calls.lock().unwrap().push(line.clone());
        let out = |ok: bool, text: &str| Out { code: Some(if ok { 0 } else { 1 }), stdout: text.into(), ..Out::default() };
        if line.starts_with("apply") {
            return out(self.apply_ok, if self.apply_ok { "created" } else { "forbidden" });
        }
        if line.contains("get job/") {
            let mut l = self.looks.lock().unwrap();
            let r = if l.is_empty() { "," } else { l.remove(0) };
            return out(true, r);
        }
        if line.contains("logs job/") {
            return out(true, &self.log);
        }
        out(true, "")
    }
}

fn fake(looks: Vec<&'static str>, log: &str, apply_ok: bool) -> Fake {
    Fake { calls: Mutex::new(vec![]), looks: Mutex::new(looks), log: log.into(), apply_ok }
}

fn ctx(d: &Path) -> Context {
    put(d, "Containerfile", "FROM scratch\n");
    package_context(d, "Containerfile").unwrap()
}

#[test]
fn a_build_applies_waits_reads_the_digest_and_removes_what_it_made() {
    let d = tmp("run");
    let k = fake(vec![",", ",", "1,"], &format!("STEP 1/1\nIYAKKI-DIGEST={DIGEST}\n"), true);
    let looked = Mutex::new(0u32);
    let b = build(&k, &spec(), &ctx(&d), "a1b2", &d.join("apply.json"), 10, &|| *looked.lock().unwrap() += 1).unwrap();
    assert_eq!(b.digest, DIGEST);
    assert_eq!(b.image, format!("localhost:30500/my-notes:1@{DIGEST}"));
    assert_eq!(*looked.lock().unwrap(), 2, "it looked until the job said done");
    let calls = k.calls.lock().unwrap().join("\n");
    assert!(calls.contains("delete job build-a1b2") && calls.contains("delete configmap build-a1b2-ctx"), "{calls}");
    assert!(!d.join("apply.json").exists(), "the applied file is removed");
    // what comes out is a valid, pinned image app
    let app = image_app_json("my-notes", "My notes", &b, 8080, "MIT").unwrap();
    assert!(app.contains(&b.image) && app.contains("\"licence\": \"MIT\""));
    assert!(image_app_json("my-notes", "My notes", &b, 8080, "").is_err(), "an image app needs its licence");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_refused_apply_a_failed_build_and_a_build_that_never_ends_are_said_plainly_and_cleaned_up() {
    let d = tmp("fail");
    let c = ctx(&d);
    let k = fake(vec![], "", false);
    let e = build(&k, &spec(), &c, "a1b2", &d.join("a.json"), 3, &|| {}).unwrap_err();
    assert!(e.contains("refused the build") && e.contains("forbidden"), "{e}");
    assert!(k.calls.lock().unwrap().iter().any(|l| l.contains("delete job")), "cleaned up even when the apply was refused");
    let k = fake(vec![",1"], "STEP 2/3: RUN make\nerror: make: *** [all] Error 2\n", true);
    let e = build(&k, &spec(), &c, "a1b2", &d.join("a.json"), 3, &|| {}).unwrap_err();
    assert!(e.starts_with("the build failed"), "{e}");
    assert!(k.calls.lock().unwrap().iter().any(|l| l.contains("delete configmap")));
    let k = fake(vec![",", ",", ","], "", true);
    let e = build(&k, &spec(), &c, "a1b2", &d.join("a.json"), 3, &|| {}).unwrap_err();
    assert!(e.contains("did not finish in time"), "{e}");
    // finished but silent about the digest
    let k = fake(vec!["1,"], "all done\n", true);
    assert!(build(&k, &spec(), &c, "a1b2", &d.join("a.json"), 3, &|| {}).unwrap_err().contains("no image digest"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn only_a_well_formed_digest_line_counts() {
    assert_eq!(digest_from_log(&format!("x\n  IYAKKI-DIGEST={DIGEST}  \n")).as_deref(), Some(DIGEST));
    for bad in ["IYAKKI-DIGEST=sha256:abc", "IYAKKI-DIGEST=sha256:ZZZZ456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", "IYAKKI-DIGEST=md5:abc", "digest=sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", ""] {
        assert_eq!(digest_from_log(bad), None, "{bad}");
    }
}
