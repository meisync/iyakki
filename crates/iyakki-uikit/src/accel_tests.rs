use super::golden::*;
use super::*;
use std::cell::Cell;
use std::rc::Rc;

/// A stand-in for a card: floating point maths (so it rounds differently from the integer reference), and it can lose its device.
struct FakeGpu {
    inner: PlainCpu,
    /// Calls left before the device is lost; `None` never.
    lose_in: Option<u32>,
    declines_yuv: bool,
    broken_blit: bool,
    calls: Rc<Cell<u32>>,
}

impl FakeGpu {
    fn new() -> FakeGpu {
        FakeGpu { inner: PlainCpu::default(), lose_in: None, declines_yuv: false, broken_blit: false, calls: Rc::new(Cell::new(0)) }
    }
    fn tick(&mut self) -> Result<(), AccelError> {
        self.calls.set(self.calls.get() + 1);
        match self.lose_in {
            Some(0) => Err(AccelError::DeviceLost("the driver reset the device".into())),
            Some(n) => {
                self.lose_in = Some(n - 1);
                Ok(())
            }
            None => Ok(()),
        }
    }
}

impl Backend for FakeGpu {
    fn tier(&self) -> Tier {
        Tier::Gpu
    }
    fn blit(&mut self, dst: &mut Surface, src: &Surface, dx: i32, dy: i32) -> Result<(), AccelError> {
        self.tick()?;
        let mut out = dst.clone();
        for y in 0..src.h as i32 {
            for x in 0..src.w as i32 {
                let (tx, ty) = (x + dx, y + dy);
                if tx < 0 || ty < 0 || tx >= dst.w as i32 || ty >= dst.h as i32 {
                    continue;
                }
                let s = src.px[y as usize * src.w + x as usize];
                let d = &mut out.px[ty as usize * dst.w + tx as usize];
                let k = 1.0 - (s >> 24) as f32 / 255.0;
                let mut v = 0u32;
                for sh in [24u32, 16, 8, 0] {
                    let c = ((s >> sh) & 255) as f32 + ((*d >> sh) & 255) as f32 * k;
                    v |= (c.round().min(255.0) as u32) << sh;
                }
                *d = if self.broken_blit { v ^ 0x0010_1010 } else { v };
            }
        }
        *dst = out;
        self.inner.done += (src.w * src.h) as u64;
        Ok(())
    }
    fn scale(&mut self, src: &Surface, dw: usize, dh: usize) -> Result<Surface, AccelError> {
        self.tick()?;
        self.inner.scale(src, dw, dh)
    }
    fn yuv420(&mut self, y: &[u8], u: &[u8], v: &[u8], w: usize, h: usize) -> Result<Surface, AccelError> {
        self.tick()?;
        if self.declines_yuv {
            return Err(AccelError::Unsupported);
        }
        let mut out = Surface::new(w, h);
        for r in 0..h {
            for c in 0..w {
                let i = (r / 2) * (w / 2) + c / 2;
                let yy = 1.164 * (y[r * w + c] as f32 - 16.0).max(0.0);
                let (d, e) = (u[i] as f32 - 128.0, v[i] as f32 - 128.0);
                let cl = |f: f32| f.round().clamp(0.0, 255.0) as u32;
                out.px[r * w + c] = 0xff00_0000 | (cl(yy + 1.596 * e) << 16) | (cl(yy - 0.392 * d - 0.813 * e) << 8) | cl(yy + 2.017 * d);
            }
        }
        Ok(out)
    }
    fn blur(&mut self, s: &mut Surface, radius: usize) -> Result<(), AccelError> {
        self.tick()?;
        self.inner.blur(s, radius)
    }
    fn scroll(&mut self, s: &mut Surface, dy: i32) -> Result<(), AccelError> {
        self.tick()?;
        self.inner.scroll(s, dy)
    }
    fn pixels_done(&self) -> u64 {
        self.inner.done
    }
}

fn tree(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-accel-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn node(root: &Path, name: &str, vendor: &str, driver: Option<&str>) {
    let dev = root.join("sys/class/drm").join(name).join("device");
    std::fs::create_dir_all(&dev).unwrap();
    std::fs::write(dev.join("vendor"), format!("{vendor}\n")).unwrap();
    std::fs::write(dev.join("device"), "0x1681\n").unwrap();
    if let Some(d) = driver {
        let target = root.join("sys/bus/pci/drivers").join(d);
        std::fs::create_dir_all(&target).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, dev.join("driver")).unwrap();
    }
}

fn libs(root: &Path, names: &[&str]) {
    let d = root.join("usr/lib");
    std::fs::create_dir_all(&d).unwrap();
    for n in names {
        std::fs::write(d.join(n), b"x").unwrap();
    }
}

#[test]
fn the_rounded_division_by_255_is_exact_for_every_pair() {
    for x in 0..=255u32 {
        for a in 0..=255u32 {
            assert_eq!(mul255(x, a), ((x * a) as f64 / 255.0).round() as u32, "{x} {a}");
        }
    }
}

#[test]
fn the_wide_path_gives_the_reference_pixels_exactly_on_every_pass() {
    let lines = run_suite(&mut SimdCpu::default(), EXACT);
    assert_eq!(lines.len(), 16);
    for l in &lines {
        assert!(l.ok(), "{} differs by {} on {} pixels", l.pass, l.diff.max, l.diff.over);
        assert_eq!(l.diff.max, 0, "{}", l.pass);
    }
    // an opaque source, a transparent source and a half one all meet the same formula
    let mut a = Surface::filled(8, 8, 0xff10_2030);
    let mut b = a.clone();
    let layer = Surface { w: 3, h: 1, px: vec![0xff01_0203, 0x0000_0000, 0x8040_2010] };
    PlainCpu::default().blit(&mut a, &layer, 1, 1).unwrap();
    SimdCpu::default().blit(&mut b, &layer, 1, 1).unwrap();
    assert_eq!(a, b);
}

#[test]
fn a_card_that_rounds_in_floating_point_stays_inside_the_stated_tolerance_and_a_wrong_one_is_caught() {
    let lines = run_suite(&mut FakeGpu::new(), CARD);
    assert!(lines.iter().all(|l| l.ok()), "{lines:?}");
    // the float card really does differ from the reference somewhere, so the tolerance is doing work
    assert!(lines.iter().any(|l| l.diff.max > 0));
    let mut bad = FakeGpu::new();
    bad.broken_blit = true;
    let lines = run_suite(&mut bad, CARD);
    assert!(lines.iter().any(|l| l.pass == "blit" && !l.ok()));
    // a picture of the wrong size never passes
    let d = compare(&Surface::new(2, 2), &Surface::new(3, 2), 255);
    assert!(!d.size_matches);
}

#[test]
fn a_lost_device_is_reported_once_the_call_still_gives_the_right_pixels_and_the_card_is_not_asked_again() {
    let caps = Caps { devices: vec![], chosen: Some(0), api: ApiLevel::Gles3, reason_off: String::new() };
    let mut gpu = FakeGpu::new();
    gpu.lose_in = Some(1);
    let calls = gpu.calls.clone();
    let mut a = Accel::new(&Caps { devices: vec![Device { node: "/dev/dri/renderD128".into(), vendor: "AMD", pci_device: 1, driver: Some("amdgpu".into()), is_software: false }], ..caps }, true, Some(Box::new(gpu)));
    assert_eq!(a.tier(), Tier::Gpu);
    let base = fixture(20, 20, 9);
    let layer = fixture(10, 10, 4);
    let mut first = base.clone();
    a.blit(&mut first, &layer, 2, 2);
    assert_eq!(a.tier(), Tier::Gpu);
    assert!(a.take_report().is_none());
    let mut second = first.clone();
    a.blit(&mut second, &layer, 5, 5);
    let mut want = first.clone();
    PlainCpu::default().blit(&mut want, &layer, 5, 5).unwrap();
    assert_eq!(second, want, "the call that lost the card still draws, on the processor");
    assert_eq!(a.tier(), Tier::SimdCpu);
    let rep = a.take_report().expect("one report");
    assert_eq!(rep.now_using, Tier::SimdCpu);
    assert!(rep.reason.contains("reset"));
    assert!(a.take_report().is_none());
    let before = calls.get();
    a.blur(&mut second, 2);
    a.scroll(&mut second, 3);
    assert_eq!(calls.get(), before, "a lost card is never asked again this session");
    assert_eq!(a.fell_through(), 1);
}

#[test]
fn a_pass_the_card_declines_goes_to_the_processor_and_the_card_stays() {
    let caps = Caps { devices: vec![Device { node: "/dev/dri/renderD128".into(), vendor: "AMD", pci_device: 1, driver: None, is_software: false }], chosen: Some(0), api: ApiLevel::Gles2, reason_off: String::new() };
    let mut gpu = FakeGpu::new();
    gpu.declines_yuv = true;
    let mut a = Accel::new(&caps, true, Some(Box::new(gpu)));
    let (y, u, v) = fixture_yuv(8, 8, 2);
    let s = a.yuv420(&y, &u, &v, 8, 8).unwrap();
    assert_eq!(s, PlainCpu::default().yuv420(&y, &u, &v, 8, 8).unwrap());
    assert_eq!(a.tier(), Tier::Gpu);
    assert!(a.take_report().is_none());
    assert!(a.yuv420(&y[..10], &u, &v, 8, 8).is_none(), "planes too short are refused, not drawn");
}

#[test]
fn the_card_path_is_in_the_chain_only_when_the_switches_and_the_probe_allow_it() {
    let usable = Caps { devices: vec![Device { node: "/dev/dri/renderD128".into(), vendor: "Intel", pci_device: 1, driver: Some("i915".into()), is_software: false }], chosen: Some(0), api: ApiLevel::Gles3, reason_off: String::new() };
    assert_eq!(Accel::new(&usable, true, Some(Box::new(FakeGpu::new()))).tier(), Tier::Gpu);
    assert_eq!(Accel::new(&usable, false, Some(Box::new(FakeGpu::new()))).tier(), Tier::SimdCpu, "a switch that is off");
    assert_eq!(Accel::new(&usable, true, None).tier(), Tier::SimdCpu, "no card path written for this machine");
    let off = Caps { reason_off: "the graphics libraries are not installed".into(), ..usable };
    assert_eq!(Accel::new(&off, true, Some(Box::new(FakeGpu::new()))).tier(), Tier::SimdCpu, "libraries the person did not install are never assumed");
    assert_eq!(Accel::cpu_only().tier(), Tier::SimdCpu);
}

#[test]
fn the_probe_names_the_card_the_node_the_driver_and_the_level() {
    let r = tree("ok");
    node(&r, "renderD128", "0x1002", Some("amdgpu"));
    libs(&r, &["libEGL.so.1", "libGLESv2.so.2"]);
    let c = probe(&r, None);
    assert!(c.usable());
    assert_eq!(c.api, ApiLevel::Gles3);
    assert_eq!(c.node().unwrap(), r.join("dev/dri/renderD128"));
    let s = c.summary();
    assert!(s.contains("AMD") && s.contains("amdgpu") && s.contains("OpenGL ES 3") && s.contains("fallback"), "{s}");
    libs(&r, &["libvulkan.so.1"]);
    assert_eq!(probe(&r, None).api, ApiLevel::Vulkan);
    let _ = std::fs::remove_dir_all(&r);
}

#[test]
fn the_probe_says_why_there_is_no_card_path() {
    let r = tree("none");
    let c = probe(&r, None);
    assert!(!c.usable() && c.summary().contains("no render node"), "{}", c.summary());
    node(&r, "renderD128", "0x1af4", Some("virtio-pci"));
    libs(&r, &["libEGL.so.1", "libGLESv2.so.2"]);
    let c = probe(&r, None);
    assert!(!c.usable() && c.summary().contains("virtual or software"), "{}", c.summary());
    let r2 = tree("nolib");
    node(&r2, "renderD128", "0x8086", Some("i915"));
    let c = probe(&r2, None);
    assert!(!c.usable() && c.summary().contains("not installed"), "{}", c.summary());
    assert_eq!(c.api, ApiLevel::None);
    let _ = std::fs::remove_dir_all(&r);
    let _ = std::fs::remove_dir_all(&r2);
}

#[test]
fn with_two_cards_the_persons_pick_wins_and_a_software_node_is_never_picked() {
    let r = tree("two");
    node(&r, "renderD128", "0x10de", Some("nvidia"));
    node(&r, "renderD129", "0x1002", Some("amdgpu"));
    node(&r, "renderD130", "0x1af4", Some("virtio-pci"));
    libs(&r, &["libEGL.so.1", "libGLESv2.so.2"]);
    assert_eq!(probe(&r, None).node().unwrap().file_name().unwrap(), "renderD128");
    assert_eq!(probe(&r, Some("renderD129")).node().unwrap().file_name().unwrap(), "renderD129");
    assert_eq!(probe(&r, Some("renderD130")).node().unwrap().file_name().unwrap(), "renderD128", "a pick of a software node falls back to the first real card");
    assert_eq!(probe(&r, Some("renderD999")).node().unwrap().file_name().unwrap(), "renderD128");
    let _ = std::fs::remove_dir_all(&r);
}

#[test]
fn the_counts_the_budgets_use_follow_the_visible_work_not_the_picture_size() {
    let mut a = Accel::cpu_only();
    let mut big = Surface::new(1000, 1000);
    let layer = Surface::filled(10, 10, 0xff00_00ff);
    a.blit(&mut big, &layer, 995, 995);
    assert_eq!(a.pixels_by_tier()[0], (Tier::SimdCpu, 25), "only the 5 x 5 corner that is on the surface");
    a.blit(&mut big, &layer, 2000, 0);
    a.blit(&mut big, &layer, -10, 0);
    assert_eq!(a.pixels_by_tier()[0].1, 25, "a layer wholly outside costs nothing");
    let src = Surface::filled(64, 64, 0xff11_2233);
    let small = a.scale(&src, 16, 16);
    assert_eq!((small.w, small.h), (16, 16));
    assert_eq!(a.pixels_by_tier()[0].1, 25 + 256);
    assert!(small.px.iter().all(|&p| p == 0xff11_2233), "a flat picture stays flat when scaled");
}

#[test]
fn scrolling_by_an_offset_moves_rows_and_the_new_rows_are_transparent() {
    let mut s = Surface { w: 2, h: 4, px: vec![1, 1, 2, 2, 3, 3, 4, 4] };
    Accel::cpu_only().scroll(&mut s, 1);
    assert_eq!(s.px, vec![2, 2, 3, 3, 4, 4, 0, 0]);
    Accel::cpu_only().scroll(&mut s, -2);
    assert_eq!(s.px, vec![0, 0, 0, 0, 2, 2, 3, 3]);
    Accel::cpu_only().scroll(&mut s, 9);
    assert!(s.px.iter().all(|&p| p == 0));
}

#[test]
fn blur_of_a_flat_picture_changes_nothing_and_a_dot_spreads_evenly() {
    let mut flat = Surface::filled(9, 9, 0xff40_80c0);
    Accel::cpu_only().blur(&mut flat, 3);
    assert!(flat.px.iter().all(|&p| p == 0xff40_80c0));
    let mut dot = Surface::new(9, 1);
    dot.px[4] = 0x0000_0070;
    PlainCpu::default().blur(&mut dot, 1).unwrap();
    assert_eq!(dot.px[3] & 255, 0x25);
    assert_eq!(dot.px[4] & 255, 0x25);
    assert_eq!(dot.px[5] & 255, 0x25);
    assert_eq!(dot.px[2], 0);
}
