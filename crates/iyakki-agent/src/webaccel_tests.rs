use super::*;
use iyakki_shared::webaccel::{COMPOSITING, VIDEO};

fn root(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-agentaccel-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn pci(root: &Path, addr: &str, vendor: &str, driver: Option<&str>, render: Option<&str>) {
    let d = root.join("sys/bus/pci/devices").join(addr);
    std::fs::create_dir_all(d.join("drm")).unwrap();
    std::fs::write(d.join("class"), "0x030000\n").unwrap();
    std::fs::write(d.join("vendor"), format!("{vendor}\n")).unwrap();
    std::fs::write(d.join("device"), "0x1681\n").unwrap();
    if let Some(drv) = driver {
        let t = root.join("sys/bus/pci/drivers").join(drv);
        std::fs::create_dir_all(&t).unwrap();
        std::os::unix::fs::symlink(&t, d.join("driver")).unwrap();
    }
    if let Some(r) = render {
        std::fs::create_dir_all(d.join("drm").join(r)).unwrap();
    }
}

fn touch(root: &Path, rel: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b"x").unwrap();
}

#[test]
fn a_real_card_with_its_driver_and_the_libraries_turns_the_parts_on_and_video_needs_its_own_driver() {
    let r = root("on");
    pci(&r, "0000:01:00.0", "0x1002", Some("amdgpu"), Some("renderD128"));
    touch(&r, "usr/lib/libEGL.so.1");
    touch(&r, "usr/lib/libGLESv2.so.2");
    let m = machine_from(&r, false);
    assert!(m.node_usable, "{}", m.node_reason);
    assert!(!m.va_driver);
    touch(&r, "usr/lib/dri/radeonsi_drv_video.so");
    assert!(!machine_from(&r, false).va_driver, "the driver alone is not enough");
    touch(&r, "usr/lib/gstreamer-1.0/libgstva.so");
    assert!(machine_from(&r, false).va_driver);
    let data = r.join("data");
    let p = plan_for(&data, &r, "ann", "a.example.org", false);
    assert!(p.item(COMPOSITING).unwrap().on && p.item(VIDEO).unwrap().on);
    let _ = std::fs::remove_dir_all(&r);
}

#[test]
fn a_missing_library_a_missing_driver_and_a_virtual_display_are_named() {
    let r = root("off");
    assert!(machine_from(&r, false).node_reason.contains("no graphics card"));
    pci(&r, "0000:00:02.0", "0x8086", None, None);
    assert!(machine_from(&r, false).node_reason.contains("driver is not loaded"));
    let r2 = root("nolib");
    pci(&r2, "0000:00:02.0", "0x8086", Some("i915"), Some("renderD128"));
    assert!(machine_from(&r2, false).node_reason.contains("not installed"));
    let r3 = root("virt");
    pci(&r3, "0000:00:02.0", "0x1af4", Some("virtio-pci"), Some("renderD128"));
    touch(&r3, "usr/lib/libEGL.so.1");
    touch(&r3, "usr/lib/libGLESv2.so.2");
    assert!(!machine_from(&r3, false).node_usable);
    for d in [r, r2, r3] {
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[test]
fn the_persons_file_changes_the_plan_for_that_person_and_site_only() {
    let r = root("pol");
    pci(&r, "0000:01:00.0", "0x10de", Some("nvidia"), Some("renderD128"));
    touch(&r, "usr/lib/libEGL.so.1");
    touch(&r, "usr/lib/libGLESv2.so.2");
    let data = r.join("data");
    let mut pol = iyakki_shared::webaccel::Policy::default();
    pol.set_site_off("ann", "slow.example.org", true).unwrap();
    iyakki_shared::webaccel::save(&data.join("web-profile"), &pol).unwrap();
    assert!(!plan_for(&data, &r, "ann", "www.slow.example.org", false).item(COMPOSITING).unwrap().on);
    assert!(plan_for(&data, &r, "ann", "other.example.org", false).item(COMPOSITING).unwrap().on);
    assert!(plan_for(&data, &r, "bob", "www.slow.example.org", false).item(COMPOSITING).unwrap().on);
    assert_eq!(user_of_profile(&data.join("web-profile/ann")), "ann");
    let _ = std::fs::remove_dir_all(&r);
}
