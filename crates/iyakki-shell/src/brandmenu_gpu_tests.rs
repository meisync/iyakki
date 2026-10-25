use super::*;
use iyakki_agent::gpu::{Gpu, GpuPod, Report, Vendor};

const SIZES: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (1024, 600), (800, 480)];

fn gpu(vendor: u16, device: u16, driver: &str, render: bool) -> Gpu {
    Gpu { pci: "0000:01:00.0".into(), vendor_id: vendor, device_id: device, vendor: Vendor::from_id(vendor), name: gpu::device_name(vendor, device), driver: Some(driver.into()), card: Some("card0".into()), render: render.then(|| "renderD128".to_string()), boot_vga: false, vram: Some(24 << 30) }
}

fn report() -> Report {
    Report {
        gpus: vec![gpu(0x10de, 0x2684, "nouveau", true), gpu(0x1002, 0x15bf, "amdgpu", true)],
        driver: gpu::DriverState { nouveau_loaded: true, nvidia_module_present: true, missing_firmware: vec!["nvidia/ad102/gsp/gsp-535.113.01.bin".into(), "amdgpu/gc_11_0_1_pfp.bin".into()], kernel_lines: vec!["nouveau 0000:01:00.0: gsp ctor failed: -2".into(); 6], ..Default::default() },
        pods: (0..8).map(|i| GpuPod { namespace: "iyakki-ai".into(), name: format!("lemonade-with-a-rather-long-pod-name-{i}"), how: "CDI iyakki.dev/gpu=all".into() }).collect(),
        pods_note: String::new(),
        last_test: Some("48.0 tokens/s (96 tokens in 2.0 s, model Llama-3.2-1B-GGUF)".into()),
        stage1: Some("AMD Radeon 780M (renderD128); pods ask with cdi.k8s.io/iyakki-gpu: iyakki.dev/gpu=all".into()),
        stage3: None,
    }
}

#[test]
fn the_gpu_page_has_its_keys_and_its_way_back_is_store_setup() {
    let mut m = BrandMenu::new();
    m.toggle();
    // the page is reached from the GPU cards of Store -> Setup (the shell opens it); it is not an entry of the first page
    assert!(matches!(m.open_page(Page::Gpu), Outcome::Opened(Page::Gpu)));
    assert_eq!(m.page, Page::Gpu);
    assert_eq!(m.handle_key(&Key::Char('t')), Outcome::GpuTest);
    assert_eq!(m.handle_key(&Key::Enter), Outcome::GpuTest);
    assert_eq!(m.handle_hit(&Event::Tab(GPU_TEST)), Outcome::GpuTest);
    assert_eq!(m.handle_key(&Key::Char('r')), Outcome::Opened(Page::Gpu));
    assert_eq!(m.handle_hit(&Event::Tab(GPU_REFRESH)), Outcome::Opened(Page::Gpu));
    assert_eq!(m.handle_key(&Key::Char('i')), Outcome::Jump("store", "setup".into()));
    assert_eq!(m.handle_key(&Key::Esc), Outcome::Jump("store", "setup".into()), "back goes to Store -> Setup");
    assert_eq!(m.handle_hit(&Event::Tab(GPU_INSTALL)), Outcome::Jump("store", "setup".into()));
    assert_eq!(m.handle_hit(&Event::Tab(GPU_PAGE)), Outcome::Opened(Page::Gpu));
    assert!(!ENTRIES.iter().any(|e| e.0 == Page::Gpu), "not an entry of the first page: the GPU cards of Store -> Setup open it");
}

#[test]
fn the_gpu_page_fits_at_four_sizes_with_a_long_report_and_its_buttons_inside_the_panel() {
    for (w, h) in SIZES {
        let s = crate::shell::scale_for(w, h);
        for with_report in [true, false] {
            let mut m = BrandMenu::new();
            m.toggle();
            m.anchor = (40.0 * s, 76.0 * s);
            m.open_page(Page::Gpu);
            if with_report {
                m.set_gpu(report());
            }
            let mut g = Gfx::new(w, h);
            m.render(&mut g, w, h, s, 0);
            let p = m.panel(w, h, s);
            assert!(p.x >= 0.0 && p.y >= 0.0 && p.x + p.w <= w as f32 + 0.5 && p.y + p.h <= h as f32 + 0.5, "{w}x{h}: {p:?}");
            for hit in &g.hits {
                let r = hit.rect;
                assert!(r.x >= p.x - 0.5 && r.y >= p.y - 0.5 && r.x + r.w <= p.x + p.w + 0.5 && r.y + r.h <= p.y + p.h + 0.5, "{w}x{h}: {} {r:?} outside {p:?}", hit.label);
            }
            for id in [GPU_TEST, GPU_REFRESH, GPU_INSTALL] {
                assert!(g.hits.iter().any(|x| x.ev == Event::Tab(id) && x.rect.h >= 24.0 * s), "{w}x{h}: button {id}");
            }
            assert!(g.hits.iter().any(|x| x.ev == Event::Tab(CLOSE)) && g.hits.iter().any(|x| x.ev == Event::Tab(BACK)));
        }
    }
}

#[test]
fn the_gpu_report_names_cards_driver_pods_and_the_last_test() {
    let lines: Vec<String> = gpu::report_lines(&report()).into_iter().map(|l| l.1).collect();
    let t = lines.join("\n");
    for needle in ["GeForce RTX 4090", "Radeon 780M", "24 GB", "nouveau", "amdgpu", "lemonade-with-a-rather-long-pod-name-0", "Last test prompt: 48.0 tokens/s", "Firmware the kernel could not load: nvidia/ad102"] {
        assert!(t.contains(needle), "`{needle}` missing:\n{t}");
    }
}
