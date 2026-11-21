//! F-INST-30: the shrink step every installer front end draws. Fake file system tool and a made-up disk; the real tools are
//! exercised by `resize_tests`.

use crate::gpt_install::TableKind;
use crate::planner::*;
use crate::resize::*;
use crate::resize_step::*;
use std::path::{Path, PathBuf};

const MB: u64 = 1024 * 1024;

fn disk() -> DiskModel {
    let part = |index: u32, first_lba: u64, sectors: u64, fs: &str| Part { index, name: format!("sdz{index}"), first_lba, sectors, kind: "Linux".into(), type_guid: None, mbr_type: None, label: String::new(), fs: fs.into(), fat_free: None, mounts: vec![], swap: false, extended: false };
    DiskModel {
        name: "sdz".into(), path: "/dev/sdz".into(), model: "x".into(), serial: "x".into(), size_bytes: 1000 * MB, sector: 512, table: Some(TableKind::Gpt), notes: vec![],
        parts: vec![part(1, 2048, 100 * MB / 512, "ext4"), part(2, 300 * MB / 512, 100 * MB / 512, "xfs"), part(3, 500 * MB / 512, 100 * MB / 512, "ntfs")],
        removable: false, usb: false, read_only: false, running_system: false, booted_from: false, raw_data: false, unusual_gpt: false, disk_guid: None, bios: Local(None), fingerprint: "fp".into(),
    }
}

struct Failing;
impl FsTool for Failing {
    fn minimum(&self, _: &str, _: &Path) -> Result<u64, String> { Ok(10 * MB) }
    fn check(&self, _: &str, _: &Path) -> Result<(), String> { Ok(()) }
    fn shrink(&self, _: &str, _: &Path, _: u64) -> Result<(), String> { Err("resize2fs failed: disk full".into()) }
}

fn flow(tag: &str) -> (ShrinkFlow, PathBuf) {
    let f = std::env::temp_dir().join(format!("iyakki-step-{}-{tag}.img", std::process::id()));
    std::fs::write(&f, vec![7u8; 4096]).unwrap();
    let located = Located { path: f.clone(), model: disk(), node: Some(PathBuf::from("/dev/sdz1")), proc_mounts: String::new() };
    (ShrinkFlow::new(located, Box::new(Failing)), f)
}

#[test]
fn the_pick_list_names_what_can_be_shrunk_and_says_why_the_rest_cannot() {
    let c = candidates(&disk());
    assert_eq!(c.len(), 3);
    assert!(c[0].refused.is_none() && c[2].refused.is_none());
    assert!(c[1].refused.as_deref().unwrap().contains("xfs"));
    let mut d = disk();
    d.parts[0].mounts = vec!["/mnt/x".into()];
    d.parts[2].type_guid = Some(iyakki_storage::gpt::TYPE_ESP);
    let c = candidates(&d);
    assert!(c[0].refused.as_deref().unwrap().contains("mounted at /mnt/x"));
    assert!(c[2].refused.as_deref().unwrap().contains("EFI System Partition"));
    d.booted_from = true;
    assert!(candidates(&d).iter().all(|x| x.refused.is_some()));
}

#[test]
fn the_plan_comes_first_the_second_step_needs_the_name_and_the_box_and_a_failed_tool_changes_nothing() {
    let (mut f, img) = flow("a");
    assert!(f.confirm("sdz", true).unwrap_err().contains("no plan"));
    assert!(f.choose(2, 50).unwrap_err().contains("xfs"), "a refusal stays on the pick page");
    assert_eq!(f.stage, Stage::Pick);
    assert!(f.choose(1, 5).unwrap_err().contains("Choose at least"));
    let plan = f.choose(1, 50).unwrap().clone();
    assert_eq!(plan.new_bytes(), 50 * MB);
    assert!(f.plan_text().unwrap().contains("Shrink sdz1"));
    // wrong name, a missing box: the plan stays, nothing runs
    assert!(f.confirm("sda", true).unwrap_err().contains("exactly as shown"));
    assert!(f.confirm("sdz", false).unwrap_err().contains("tick the box"));
    assert!(matches!(f.stage, Stage::Plan(_)));
    // going back is free
    f.back();
    assert_eq!(f.stage, Stage::Pick);
    f.choose(1, 50).unwrap();
    // both given, the tool fails: the stage says so and the image is untouched
    let before = std::fs::read(&img).unwrap();
    let e = f.confirm(" sdz ", true).unwrap_err();
    assert!(e.contains("disk full"));
    assert_eq!(f.stage, Stage::Failed(e));
    assert_eq!(std::fs::read(&img).unwrap(), before);
    let _ = std::fs::remove_file(&img);
}

#[test]
fn the_boot_time_pages_walk_the_shrink_step_with_the_second_deliberate_step_and_read_the_drives_again() {
    use crate::wizard::{Page, ShrinkFocus, WKey, Wizard};
    use std::sync::{atomic::{AtomicUsize, Ordering}, Arc};
    let (_, img) = flow("pages");
    let model = disk();
    let spec = SourceSpec { path: PathBuf::from("/x.img"), boot_bytes: 40 * MB, payload_bytes: 10 * MB, core_bytes: 0, secure_boot: false };
    let mut w = Wizard::new(vec![model.clone()], spec, true);
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Mode);
    // no opener: the page says where the command is instead of failing silently
    w.handle(WKey::Char('s'));
    assert_eq!(w.page, Page::Mode);
    assert!(w.error.as_deref().unwrap().contains("--shrink"), "{:?}", w.error);
    let (img2, model2) = (img.clone(), model.clone());
    w.shrink_opener = Some(Arc::new(move |_d: &DiskModel| {
        let located = Located { path: img2.clone(), model: model2.clone(), node: Some(PathBuf::from("/dev/sdz1")), proc_mounts: String::new() };
        Ok(ShrinkFlow::new(located, Box::new(Failing)))
    }));
    let reads = Arc::new(AtomicUsize::new(0));
    let r2 = reads.clone();
    let m3 = model.clone();
    w.rereader = Some(Arc::new(move || {
        r2.fetch_add(1, Ordering::SeqCst);
        vec![m3.clone()]
    }));
    w.handle(WKey::Char('s'));
    assert_eq!(w.page, Page::Shrink);
    let text = w.render(100);
    assert!(text.contains("sdz1") && text.contains("cannot:") && text.contains("Back up first"), "{text}");
    // a size is needed, and a partition that cannot be shrunk says why
    w.handle(WKey::Enter);
    assert!(w.error.as_deref().unwrap().contains("new size"), "{:?}", w.error);
    w.handle(WKey::Down);
    for c in "50".chars() {
        w.handle(WKey::Char(c));
    }
    w.handle(WKey::Enter);
    assert!(w.error.as_deref().unwrap().contains("xfs"), "{:?}", w.error);
    w.handle(WKey::Up);
    w.handle(WKey::Enter);
    assert!(w.error.is_none(), "{:?}", w.error);
    assert!(w.render(100).contains("Shrink sdz1") && w.render(100).contains("Type the drive's name again"));
    // the box and the name are both needed; nothing runs without them
    let before = std::fs::read(&img).unwrap();
    w.handle(WKey::Tab);
    w.handle(WKey::Tab);
    assert_eq!(w.shrink.as_ref().unwrap().focus, ShrinkFocus::Run);
    w.handle(WKey::Enter);
    assert!(w.error.as_deref().unwrap().contains("tick the box"), "{:?}", w.error);
    w.handle(WKey::Tab);
    for c in "sda".chars() {
        w.handle(WKey::Char(c));
    }
    w.handle(WKey::Tab);
    w.handle(WKey::Char(' '));
    w.handle(WKey::Tab);
    w.handle(WKey::Enter);
    assert!(w.error.as_deref().unwrap().contains("exactly as shown"), "{:?}", w.error);
    assert_eq!(std::fs::read(&img).unwrap(), before);
    // Esc goes back to the list without changing anything; the right name runs the tool, which fails here and says so
    w.handle(WKey::Esc);
    assert_eq!(w.page, Page::Shrink);
    assert!(w.render(100).contains("New size"));
    w.handle(WKey::Enter);
    assert_eq!(w.shrink.as_ref().unwrap().focus, ShrinkFocus::Name);
    for c in "sdz".chars() {
        w.handle(WKey::Char(c));
    }
    w.handle(WKey::Tab);
    w.handle(WKey::Char(' '));
    w.handle(WKey::Tab);
    w.handle(WKey::Enter);
    assert!(w.error.as_deref().unwrap().contains("disk full"), "{:?}", w.error);
    assert!(w.render(100).contains("The shrink stopped"));
    assert_eq!(std::fs::read(&img).unwrap(), before, "a failed tool changes nothing");
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    // leaving after a run reads the drives again and returns to the Mode page
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Mode);
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert!(w.shrink.is_none());
    let _ = std::fs::remove_file(&img);
}
