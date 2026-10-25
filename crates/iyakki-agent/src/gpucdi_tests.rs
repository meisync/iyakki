use super::*;

pub(crate) fn tmp(tag: &str) -> PathBuf {
    let p = PathBuf::from(std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into())).join(format!("iyakki-cdi-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Compare with `fixtures/gpu/<name>`; `GOLDEN_UPDATE=1` rewrites the file (review the diff before committing it).
pub(crate) fn golden(name: &str, actual: &str) {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/fixtures/gpu").join(name);
    if std::env::var("GOLDEN_UPDATE").is_ok_and(|v| v == "1") {
        std::fs::write(&p, actual).unwrap();
    }
    let want = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e} (run once with GOLDEN_UPDATE=1)", p.display()));
    assert_eq!(actual, want, "{name} differs from its golden file");
}

/// A small valid spec for the tests that need one.
fn sample() -> Spec {
    let dev = |n: &str, p: &str| Device { name: n.into(), edits: Edits { device_nodes: vec![DeviceNode::open(p)], ..Default::default() } };
    Spec { kind: "example.dev/gpu".into(), devices: vec![dev("0", "/dev/example0"), dev("all", "/dev/example0")], common: Edits::default() }
}

#[test]
fn the_kinds_are_the_documented_names() {
    assert_eq!(NVIDIA_KIND, "nvidia.com/gpu");
    assert_eq!(PLUGIN_KIND, "k8s.device-plugin.nvidia.com/gpu");
    sample().validate().unwrap();
    assert_eq!(sample().file_name(), "example.dev-gpu.json");
}

#[test]
fn validation_refuses_what_a_runtime_would_refuse() {
    let dev = |n: &str| Device { name: n.into(), edits: Edits::default() };
    let ok = Spec { kind: "example.dev/gpu".into(), devices: vec![dev("a")], common: Edits::default() };
    ok.validate().unwrap();
    for (kind, why) in [("gpu", "no slash"), ("nodots/gpu", "vendor without a domain"), ("1x.dev/gpu", "starts with a digit"), ("example.dev/", "empty class"), ("example.dev/gpu-", "ends with a dash")] {
        let s = Spec { kind: kind.into(), ..ok.clone() };
        assert!(s.validate().is_err(), "{why}: {kind}");
    }
    assert!(Spec { devices: vec![], ..ok.clone() }.validate().unwrap_err().contains("at least one device"));
    assert!(Spec { devices: vec![dev("a"), dev("a")], ..ok.clone() }.validate().unwrap_err().contains("twice"));
    assert!(Spec { devices: vec![dev("bad name")], ..ok.clone() }.validate().is_err());
    let rel = Spec { common: Edits { device_nodes: vec![DeviceNode::open("dev/x")], ..Default::default() }, ..ok.clone() };
    assert!(rel.validate().unwrap_err().contains("not absolute"));
    let rel = Spec { common: Edits { mounts: vec![Mount::ro("/a", "b")], ..Default::default() }, ..ok };
    assert!(rel.validate().unwrap_err().contains("not absolute"));
}

#[test]
fn a_spec_is_written_atomically_listed_and_removed() {
    let d = tmp("write");
    let s = sample();
    let p = write_spec(&d.join("var/run/cdi"), &s).unwrap();
    assert_eq!(p, d.join("var/run/cdi/example.dev-gpu.json"));
    assert_eq!(std::fs::read_to_string(&p).unwrap(), s.to_json());
    assert!(!d.join("var/run/cdi/.example.dev-gpu.json.tmp").exists(), "no temporary file is left");
    assert_eq!(list_specs(&d.join("var/run/cdi")), vec![("example.dev-gpu.json".to_string(), "example.dev/gpu".to_string(), vec!["0".to_string(), "all".to_string()])]);
    // the written file parses as the JSON containerd reads
    let j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    assert_eq!(j["cdiVersion"], CDI_VERSION);
    assert_eq!(j["devices"][0]["containerEdits"]["deviceNodes"][0]["path"], "/dev/example0");
    assert_eq!(j["devices"][0]["containerEdits"]["deviceNodes"][0]["fileMode"], 0o666);
    remove_spec(&d.join("var/run/cdi"), "example.dev/gpu");
    assert!(list_specs(&d.join("var/run/cdi")).is_empty());
    assert!(write_spec(&d, &Spec { kind: "bad".into(), devices: vec![], common: Edits::default() }).is_err(), "an invalid spec is never written");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_containerd_dropins_turn_cdi_on_in_both_config_versions_and_are_written_once() {
    let f = dropin_files();
    assert_eq!(f.len(), 2);
    assert_eq!(f[0].0, "config.toml.d/iyakki-cdi.toml");
    assert!(f[0].1.contains("[plugins.\"io.containerd.grpc.v1.cri\"]") && f[0].1.contains("enable_cdi = true") && f[0].1.contains("/var/run/cdi"));
    assert_eq!(f[1].0, "config-v3.toml.d/iyakki-cdi.toml");
    assert!(f[1].1.contains("[plugins.'io.containerd.cri.v1.runtime']") && f[1].1.contains("enable_cdi = true"));
    // each is valid TOML for the line shapes containerd reads: one table header, then key = value lines
    for (_, t) in &f {
        assert_eq!(t.lines().filter(|l| l.starts_with('[')).count(), 1);
    }
    let d = tmp("dropin");
    assert!(ensure_dropins(&d).unwrap(), "first time: written, k3s has to restart");
    assert!(!ensure_dropins(&d).unwrap(), "second time: nothing changed, no restart");
    assert!(containerd_dir(&d).join("config-v3.toml.d/iyakki-cdi.toml").is_file());
    std::fs::write(containerd_dir(&d).join("config.toml.d/iyakki-cdi.toml"), "stale").unwrap();
    assert!(ensure_dropins(&d).unwrap(), "a changed file is written again");
    remove_dropins(&d);
    assert!(!containerd_dir(&d).join("config.toml.d/iyakki-cdi.toml").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_rendered_containerd_configuration_tells_whether_cdi_is_on() {
    assert!(cdi_enabled_in("version = 2\n[plugins.\"io.containerd.grpc.v1.cri\"]\n  enable_cdi = true\n"));
    assert!(!cdi_enabled_in("version = 2\n[plugins.\"io.containerd.grpc.v1.cri\"]\n  enable_cdi = false\n"));
    assert!(!cdi_enabled_in("version = 2\nimports = [\"/x/config.toml.d/*.toml\"]\n"), "version 2 without the key: off");
    assert!(cdi_enabled_in("version = 3\nimports = [\"/x/config-v3.toml.d/*.toml\"]\n"), "containerd 2 has it on and reads the drop-in folder");
    assert!(!cdi_enabled_in("version = 3\n"), "no imports: the drop-in is not read");
    // on a cluster folder
    let d = tmp("cluster");
    assert!(!cluster_cdi_enabled(&d));
    std::fs::create_dir_all(containerd_dir(&d)).unwrap();
    std::fs::write(containerd_dir(&d).join("config.toml"), "version = 3\nimports = [\"/x/config-v3.toml.d/*.toml\"]\n").unwrap();
    assert!(cluster_cdi_enabled(&d));
    let _ = std::fs::remove_dir_all(&d);
}
