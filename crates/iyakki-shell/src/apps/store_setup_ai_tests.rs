//! The AI hardware profile in Store -> Setup (docs/AI-HARDWARE.md): which AI cards a machine shows, the reasons on the cards, the
//! "Show all AI tools" switch, the facts line, and the layout with and without the hidden cards.
use super::*;
use crate::apps::store::StoreApp;
use crate::apps::App;
use iyakki_agent::aiprofile::{self, Vis};
use iyakki_sec::policy::Role;

fn fake(tag: &str, ram_gb: u64, model: &str, card: Option<(&str, &str)>) -> aiprofile::AiProfile {
    let r = std::env::temp_dir().join(format!("iyakki-ssai-{}-{:?}-{tag}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&r);
    std::fs::create_dir_all(r.join("proc")).unwrap();
    std::fs::create_dir_all(r.join("sys/class/dmi/id")).unwrap();
    std::fs::write(r.join("proc/meminfo"), format!("MemTotal: {} kB\n", ram_gb * 1024 * 1024)).unwrap();
    std::fs::write(r.join("sys/class/dmi/id/product_name"), model).unwrap();
    if let Some((v, d)) = card {
        let c = r.join("sys/bus/pci/devices/0000:01:00.0");
        std::fs::create_dir_all(&c).unwrap();
        for (f, t) in [("class", "0x030000"), ("vendor", v), ("device", d)] {
            std::fs::write(c.join(f), t).unwrap();
        }
    }
    let p = aiprofile::detect(&r);
    let _ = std::fs::remove_dir_all(&r);
    p
}

fn machine(p: Option<aiprofile::AiProfile>, show_all: bool, gpus: usize, nvidia: bool) -> Machine {
    Machine { ram_mb: 16_000, gpus, nvidia, ai: p, show_all_ai: show_all, ..Default::default() }
}

#[test]
fn no_profile_hides_nothing_and_cloud_mode_keeps_its_own_rules() {
    let m = machine(None, false, 0, false);
    assert!(Item::ALL.into_iter().filter(|i| i.fw_def().is_none() && !matches!(i, Item::GpuVulkan | Item::GpuNvidia | Item::MacFirmware | Item::MacCamera)).all(|i| m.shows(i)));
    assert_eq!(m.ai_line(), None);
    assert_eq!(m.hidden_ai_note(), None);
    // cloud: the profile is not consulted, Lemonade stays as before
    let c = Machine { cloud: true, ..machine(Some(fake("cloud", 8, "x", None)), false, 0, false) };
    assert!(c.shows(Item::Lemonade) && c.shows(Item::Ollama) && c.hidden_ai_note().is_none());
}

#[test]
fn a_cpu_only_machine_hides_the_model_servers_with_a_reason_and_show_anyway_brings_them_back() {
    let p = fake("cpu", 8, "PC", None);
    let m = machine(Some(p.clone()), false, 0, false);
    for i in [Item::Lemonade, Item::Ollama] {
        assert!(!m.shows(i), "{i:?}");
        assert!(!m.eligible_for_test().contains(&i));
    }
    assert!(m.shows(Item::Kokoro) && m.shows(Item::IndicTts) && m.shows(Item::K3s) && m.shows(Item::MacHost));
    let note = m.hidden_ai_note().unwrap();
    assert!(note.starts_with("2 AI tools are hidden because this machine has no GPU with 6 GB or more; local models would run on the CPU and be very slow."), "{note}");
    assert!(note.contains("Show all AI tools"));
    assert_eq!(m.ai_line().unwrap(), "No GPU: CPU only, 8 GB RAM; local models would run on the CPU and be slow");
    // the Mac host is recommended where there is no GPU
    assert!(m.recommend(Item::MacHost, &|_| false).is_some());
    // Show anyway
    let all = machine(Some(p), true, 0, false);
    for i in [Item::Lemonade, Item::Ollama] {
        assert!(all.shows(i));
        let n = all.heavy_note(i).unwrap();
        assert!(n.starts_with("Not recommended here: this machine has no GPU with 6 GB or more"), "{n}");
        assert!(all.recommend(i, &|_| false).is_none());
    }
    assert!(all.hidden_ai_note().is_none());
}

#[test]
fn a_gpu_with_enough_memory_shows_everything_and_recommends_the_runtime() {
    let p = fake("rtx", 32, "PC", Some(("0x10de", "0x2503")));
    let m = machine(Some(p), false, 1, true);
    for i in [Item::Lemonade, Item::Ollama, Item::GpuVulkan, Item::GpuNvidia] {
        assert!(m.shows(i), "{i:?}");
    }
    assert!(m.recommend(Item::GpuVulkan, &|_| false).is_some());
    assert!(matches!(m.ai_vis(Item::MacHost), Vis::NotRecommended(_)));
    assert!(m.heavy_note(Item::MacHost).unwrap().contains("optional"));
    assert_eq!(m.ai_line().unwrap(), "NVIDIA GeForce RTX 3060, 12 GB: about 10.5 GB usable for models; runs 7B and 13B models comfortably");
    // a small card (2 GB): the runtime and the driver go, the card is named in the reason
    let small = machine(Some(fake("gtx", 16, "PC", Some(("0x10de", "0x1c81")))), false, 1, true);
    assert!(!small.shows(Item::GpuVulkan) && !small.shows(Item::GpuNvidia) && !small.shows(Item::Lemonade));
    assert!(small.hidden_ai_note().unwrap().contains("under 6 GB"));
}

#[test]
fn apple_silicon_shows_the_local_servers_and_hides_the_bridge() {
    let m = machine(Some(fake("m2", 32, "Mac14,9", None)), false, 0, false);
    assert!(m.shows(Item::Lemonade) && m.shows(Item::Ollama));
    assert!(!m.shows(Item::MacHost));
    assert!(m.hidden_ai_note().unwrap().contains("this machine is the Mac"));
}

#[test]
fn a_remote_cluster_with_a_gpu_brings_the_model_servers_back_and_the_card_says_why() {
    use iyakki_agent::aiprofile_remote::{RemoteGpu, RemoteProfile};
    use iyakki_agent::gpu::Vendor;
    let r = RemoteProfile { cluster: "lab".into(), gpus: vec![RemoteGpu { vendor: Vendor::Nvidia, name: "NVIDIA GeForce RTX 4090".into(), count: 2, mem_gb: Some(24.0), node: "n".into() }], fetched_at: 1, stale: false, error: String::new() };
    let m = machine(Some(fake("remote", 8, "PC", None).with_remote(Some(r))), false, 0, false);
    assert!(m.shows(Item::Lemonade));
    assert_eq!(m.heavy_note(Item::Lemonade).unwrap(), "Shown because your cluster 'lab' has 2 x NVIDIA GeForce RTX 4090 (24 GB each).");
    assert!(!m.shows(Item::GpuVulkan), "the runtime on this machine stays tied to this machine");
}

impl Machine {
    fn eligible_for_test(&self) -> Vec<Item> {
        Item::ALL.into_iter().filter(|i| self.shows(*i)).collect()
    }
}

#[test]
fn the_tab_lists_only_the_cards_the_profile_allows_and_stays_inside_its_rect_at_every_size() {
    for (w, h) in [(360u32, 640u32), (800, 600), (1280, 800), (1920, 1080)] {
        for show_all in [false, true] {
            let cx = super::super::tests::cx(&format!("ssai-{w}-{show_all}"), Role::Admin);
            let mut a = StoreApp::new(&cx);
            a.setup.set_machine(machine(Some(fake("lay", 8, "PC", None)), show_all, 0, false));
            a.setup.enter(&cx);
            assert_eq!(a.setup.eligible().contains(&Item::Lemonade), show_all);
            let s = crate::shell::scale_for(w, h);
            let area = R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s);
            let mut g = Gfx::new(w, h);
            g.clear(rgb(9, 11, 20));
            a.section = crate::apps::store::Section::Setup;
            a.render(&mut g, area, s, &cx, 3);
            for hit in &g.hits {
                let r = hit.rect;
                assert!(r.x >= area.x - 1.0 && r.y >= area.y - 1.0 && r.x + r.w <= area.x + area.w + 1.0 && r.y + r.h <= area.y + area.h + 1.0, "{w}x{h} show_all={show_all}: `{}` sticks out: {r:?}", hit.label);
            }
        }
    }
}
