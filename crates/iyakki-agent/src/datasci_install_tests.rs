use super::*;
use iyakki_archive::archive::tests::{gzip, make_tar, make_zip};
use std::sync::Mutex;

struct Fake {
    files: Vec<(String, Vec<u8>)>,
    asked: Mutex<Vec<String>>,
}

impl iyakki_proxy::Streamer for Fake {
    fn open(&self, url: &str, _h: &[(String, String)]) -> Result<iyakki_proxy::StreamHead, String> {
        self.asked.lock().unwrap().push(url.into());
        if let Some(to) = url.strip_prefix("https://github.com/redirect/") {
            return Ok(iyakki_proxy::StreamHead { status: 302, headers: vec![("location".into(), format!("https://release-assets.githubusercontent.com/{to}"))], body: Box::new(std::io::empty()) });
        }
        let name = url.rsplit('/').next().unwrap();
        let body = self.files.iter().find(|(n, _)| n == name).ok_or("not found")?.1.clone();
        Ok(iyakki_proxy::StreamHead { status: 200, headers: vec![], body: Box::new(std::io::Cursor::new(body)) })
    }
}

fn hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

fn tmp(tag: &str) -> PathBuf {
    iyakki_archive::archive::tests::tmp(&format!("datasci-{tag}"))
}

/// A made-up toolkit: a python tarball and one wheel that also carries things that must never be unpacked into place.
fn world() -> (Fake, Vec<Pin>) {
    let py = gzip(&make_tar(&[("python/", b'5', "", 0o755), ("python/bin/", b'5', "", 0o755), ("python/bin/python3", b'0', "#!fake", 0o755), ("python/lib/", b'5', "", 0o755), ("python/lib/python3.12/", b'5', "", 0o755), ("python/lib/python3.12/test/", b'5', "", 0o755), ("python/lib/python3.12/test/big.py", b'0', "x", 0o644), ("python/lib/python3.12/os.py", b'0', "ok", 0o644)]));
    let whl = make_zip(&[("numpy/__init__.py", "x = 1", 0o644, true), ("evil.pth", "import os", 0o644, false), ("sitecustomize.py", "boom", 0o644, false), ("numpy-9.9.9.data/scripts/run", "#!", 0o755, false), ("numpy-9.9.9.dist-info/METADATA", "Name: numpy", 0o644, false)]);
    let pin = |name: &'static str, version: &'static str, kind, url: &'static str, body: &[u8]| Pin { name, version, kind, url, sha256: Box::leak(hex(body).into_boxed_str()), mb: 1, licence: "x" };
    let pins = vec![pin("cpython", "3.12.15+20261021", Kind::Python, "https://github.com/redirect/cpython-3.12.15+20261021-x86_64-unknown-linux-gnu-install_only.tar.gz", &py), pin("numpy", "9.9.9", Kind::Wheel, "https://files.pythonhosted.org/packages/aa/numpy-9.9.9-cp312-cp312-manylinux_2_28_x86_64.whl", &whl)];
    let fake = Fake { files: vec![("cpython-3.12.15+20261021-x86_64-unknown-linux-gnu-install_only.tar.gz".into(), py), ("numpy-9.9.9-cp312-cp312-manylinux_2_28_x86_64.whl".into(), whl)], asked: Mutex::new(vec![]) };
    (fake, pins)
}

#[test]
fn the_shipped_table_stops_the_install_before_anything_is_fetched() {
    let d = tmp("closed");
    let (fake, _) = world();
    let mut said = vec![];
    let e = install(&fake, &d, &datasci::PINS, &mut |_| Ok("never".into()), &mut |l| said.push(l.to_string())).unwrap_err();
    assert!(e.contains("Nothing was downloaded"), "{e}");
    assert!(fake.asked.lock().unwrap().is_empty());
    assert!(!datasci::dir(&d).exists() && !d.join("datasci.new").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_filled_table_downloads_through_the_allowed_redirect_checks_unpacks_and_leaves_out_what_could_run() {
    let d = tmp("ok");
    let (fake, pins) = world();
    let mut said = vec![];
    let seen = install(&fake, &d, &pins, &mut |data| {
        assert!(datasci::python_bin(data).exists() && datasci::site_dir(data).join("numpy/__init__.py").exists(), "the self-test sees the finished tree");
        Ok("numpy 9.9.9 fits a model".into())
    }, &mut |l| said.push(l.to_string()))
    .unwrap();
    assert_eq!(seen, "numpy 9.9.9 fits a model");
    let site = datasci::site_dir(&d);
    assert!(!site.join("evil.pth").exists() && !site.join("sitecustomize.py").exists() && !site.join("numpy-9.9.9.data/scripts").exists());
    assert!(site.join("numpy-9.9.9.dist-info/METADATA").exists());
    assert!(!datasci::dir(&d).join("python/lib/python3.12/test").exists() && datasci::dir(&d).join("python/lib/python3.12/os.py").exists());
    assert!(fake.asked.lock().unwrap().iter().any(|u| u.starts_with("https://release-assets.githubusercontent.com/")), "the redirect was followed to an allowed host");
    let st = datasci::installed(&d).expect("installed");
    assert_eq!(st.files.len(), 2);
    assert!(!d.join("datasci.download").exists() && !d.join("datasci.new").exists());
    assert!(said.last().unwrap().starts_with("[7/7] done"), "{said:?}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_wrong_file_a_bad_redirect_and_a_failed_self_test_leave_nothing_behind() {
    let d = tmp("bad");
    let (mut fake, pins) = world();
    // the wheel's bytes change on the server: the pinned hash no longer matches
    fake.files[1].1.push(0);
    let e = install(&fake, &d, &pins, &mut |_| Ok(String::new()), &mut |_| {}).unwrap_err();
    assert!(e.contains("is not the one pinned") && e.contains("deleted"), "{e}");
    assert!(!datasci::dir(&d).exists() && !d.join("datasci.new").exists());
    assert!(std::fs::read_dir(d.join("datasci.download")).map(|r| r.flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".part") && !e.file_name().to_string_lossy().contains("numpy"))).unwrap_or(true));
    // a redirect to a host that is not allowed
    struct Evil;
    impl iyakki_proxy::Streamer for Evil {
        fn open(&self, _u: &str, _h: &[(String, String)]) -> Result<iyakki_proxy::StreamHead, String> {
            Ok(iyakki_proxy::StreamHead { status: 302, headers: vec![("location".into(), "https://evil.example/x".into())], body: Box::new(std::io::empty()) })
        }
    }
    let (_, pins) = world();
    let e = install(&Evil, &d, &pins, &mut |_| Ok(String::new()), &mut |_| {}).unwrap_err();
    assert!(e.contains("not on the toolkit's download hosts"), "{e}");
    // a failed self-test removes the folder again; the real runner is not wired and says so
    let (fake, pins) = world();
    let e = install(&fake, &d, &pins, &mut |data| no_runner(data), &mut |_| {}).unwrap_err();
    assert!(e.contains("not wired"), "{e}");
    assert!(datasci::installed(&d).is_none() && !datasci::dir(&d).exists());
    let _ = std::fs::remove_dir_all(&d);
}
