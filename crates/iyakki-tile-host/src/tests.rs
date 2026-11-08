use super::*;
use iyakki_tile_abi::{Color, DisplayList, Event, Op, Rect, ABI_VERSION};
use std::sync::Arc;

const THEME: Event = Event::Theme {
    dark: true,
    accent: Color(1, 2, 3, 255),
    background: Color(0, 0, 0, 255),
    foreground: Color(255, 255, 255, 255),
};

/// A tile that always returns the given encoded frame; `frame_body` may add extra WAT.
fn static_tile(frame: &[u8], abi: u32, extra_funcs: &str, frame_body: &str) -> String {
    let data: String = frame.iter().map(|b| format!("\\{b:02x}")).collect();
    let mut prefix = (frame.len() as u32).to_le_bytes().iter().map(|b| format!("\\{b:02x}")).collect::<String>();
    prefix.push_str(&data);
    format!(
        r#"(module
  (import "iyakki_tile" "call" (func $call (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
  (import "iyakki_tile" "random_u32" (func $rand (result i32)))
  (memory (export "memory") 2)
  (data (i32.const 1024) "{prefix}")
  (data (i32.const 900) "svc") (data (i32.const 910) "get") (data (i32.const 920) "{{}}")
  (global $h (mut i32) (i32.const 8192))
  (func (export "iyakki_tile_abi") (result i32) (i32.const {abi}))
  (func (export "iyakki_alloc") (param i32) (result i32) (global.get $h))
  (func (export "iyakki_tile_init") (param i32 i32 i32) (result i32) (i32.const 0))
  (func (export "iyakki_tile_event") (param i32 i32) (result i32) (i32.const 0))
  {extra_funcs}
  (func (export "iyakki_tile_frame") (result i32) {frame_body} (i32.const 1024)))"#
    )
}

fn one_rect(x: i32) -> Vec<u8> {
    let mut l = DisplayList::new();
    l.push(Op::Rect { r: Rect::new(x, 0, 10, 10), color: Color(1, 2, 3, 255) });
    l.encode().unwrap()
}

fn load(wat: &str) -> Result<TileRunner, TileError> {
    TileRunner::load(wat.as_bytes(), TileManifest::new("t", &[]), Budgets::default(), None, 1)
}

#[test]
fn a_good_static_frame_is_accepted() {
    let mut r = load(&static_tile(&one_rect(5), ABI_VERSION, "", "")).unwrap();
    r.init(320, 200, 100, THEME).unwrap();
    assert_eq!(r.frame().unwrap().ops.len(), 1);
}

#[test]
fn version_mismatch_refuses_to_load_with_a_plain_message() {
    let e = load(&static_tile(&one_rect(5), 2, "", "")).err().unwrap();
    assert!(matches!(e, TileError::VersionMismatch { tile: 2, host: 1 }));
    let msg = e.to_string();
    assert!(msg.contains("version 2") && msg.contains("version 1") && msg.contains("Update"), "{msg}");
    let mut m = TileManifest::new("t", &[]);
    m.abi = 7;
    let e = TileRunner::load(static_tile(&one_rect(5), 1, "", "").as_bytes(), m, Budgets::default(), None, 1).err().unwrap();
    assert!(matches!(e, TileError::VersionMismatch { tile: 7, .. }));
}

#[test]
fn a_module_without_the_exports_is_not_a_tile() {
    let e = load(r#"(module (memory (export "memory") 1))"#).err().unwrap();
    assert!(matches!(e, TileError::MissingExport(_)), "{e}");
}

#[test]
fn a_tile_that_loops_forever_is_stopped_by_the_fuel_budget() {
    let wat = static_tile(&one_rect(5), 1, "", "(loop $l (br $l))");
    let b = Budgets { frame_fuel: 100_000, init_fuel: 100_000, event_fuel: 100_000, wall: std::time::Duration::from_secs(5), ..Budgets::default() };
    let mut r = TileRunner::load(wat.as_bytes(), TileManifest::new("t", &[]), b, None, 1).unwrap();
    r.init(320, 200, 100, THEME).unwrap();
    let t = std::time::Instant::now();
    let e = r.frame().err().unwrap();
    assert!(matches!(e, TileError::Budget("computing time")), "{e}");
    assert!(t.elapsed() < std::time::Duration::from_secs(2));
    assert!(e.to_string().contains("stopped"));
}

#[test]
fn the_wall_clock_budget_also_stops_a_tile() {
    // fuel is huge, the wall limit is small
    let wat = static_tile(&one_rect(5), 1, "", "(loop $l (br $l))");
    let b = Budgets { frame_fuel: u64::MAX / 8, init_fuel: 1000, event_fuel: 1000, wall: std::time::Duration::from_millis(50), ..Budgets::default() };
    let mut r = TileRunner::load(wat.as_bytes(), TileManifest::new("t", &[]), b, None, 1).unwrap();
    let e = r.frame().err().unwrap();
    assert!(matches!(e, TileError::Budget("real time")), "{e}");
}

#[test]
fn an_out_of_bounds_draw_list_is_refused() {
    let mut r = load(&static_tile(&one_rect(5000), 1, "", "")).unwrap();
    r.init(320, 200, 100, THEME).unwrap();
    assert!(matches!(r.frame().err().unwrap(), TileError::OutOfBounds { op: 0 }));
    // beyond the absolute coordinate limit it does not even decode
    let mut l = DisplayList::new();
    l.push(Op::Rect { r: Rect::new(1_000_000, 0, 1, 1), color: Color(0, 0, 0, 255) });
    let mut r = load(&static_tile(&l.encode().unwrap(), 1, "", "")).unwrap();
    assert!(matches!(r.frame().err().unwrap(), TileError::BadDisplayList(_)));
}

#[test]
fn a_frame_pointer_outside_guest_memory_is_refused() {
    let wat = static_tile(&one_rect(5), 1, "", "").replace("(i32.const 1024)))", "(i32.const 200000)))");
    let mut r = load(&wat).unwrap();
    assert!(matches!(r.frame().err().unwrap(), TileError::BadPointer));
}

#[test]
fn garbage_frames_are_refused_not_crashed_on() {
    let mut r = load(&static_tile(b"not a display list at all", 1, "", "")).unwrap();
    assert!(matches!(r.frame().err().unwrap(), TileError::BadDisplayList(_)));
}

struct Echo;
impl ServiceBackend for Echo {
    fn call(&self, d: &str, m: &str, j: &str) -> Result<String, String> {
        Ok(format!("{d}.{m}:{j}"))
    }
}

/// The tile calls svc.get with "{}" into its output at 2000 and stores the result code at 2400.
fn caller_tile() -> String {
    static_tile(
        &one_rect(5),
        1,
        "",
        "(i32.store (i32.const 2400) (call $call (i32.const 900) (i32.const 3) (i32.const 910) (i32.const 3) (i32.const 920) (i32.const 2) (i32.const 2000) (i32.const 64)))",
    )
}

#[test]
fn service_calls_need_a_declared_capability() {
    let backend: Arc<dyn ServiceBackend> = Arc::new(Echo);
    let wat = caller_tile();
    for (caps, want) in [(vec![], iyakki_wasm::errno::DENIED), (vec!["other"], iyakki_wasm::errno::DENIED), (vec!["svc"], 10), (vec!["svc.get"], 10), (vec!["svc.*"], 10), (vec!["svc.put"], iyakki_wasm::errno::DENIED)] {
        let m = TileManifest { name: "t".into(), abi: 1, caps: caps.iter().map(|s| s.to_string()).collect() };
        let mut r = TileRunner::load(wat.as_bytes(), m, Budgets::default(), Some(backend.clone()), 1).unwrap();
        r.frame().unwrap();
        let code = r.session_read_i32(2400);
        assert_eq!(code, want, "caps {caps:?}");
    }
}

#[test]
fn the_service_reply_lands_in_guest_memory() {
    let m = TileManifest::new("t", &["svc"]);
    let mut r = TileRunner::load(caller_tile().as_bytes(), m, Budgets::default(), Some(Arc::new(Echo)), 1).unwrap();
    r.frame().unwrap();
    assert_eq!(r.session_read_bytes(2000, 7), b"svc.get");
}

#[test]
fn randomness_comes_only_from_the_seeded_host_import() {
    let wat = static_tile(&one_rect(5), 1, "", "(i32.store (i32.const 2400) (call $rand))");
    let get = |seed| {
        let mut r = TileRunner::load(wat.as_bytes(), TileManifest::new("t", &[]), Budgets::default(), None, seed).unwrap();
        r.frame().unwrap();
        r.session_read_i32(2400)
    };
    assert_eq!(get(5), get(5));
    assert_ne!(get(5), get(6));
}

#[test]
fn the_same_events_give_the_same_bytes() {
    let run = || {
        let mut r = load(&static_tile(&one_rect(5), 1, "", "")).unwrap();
        r.init(320, 200, 100, THEME).unwrap();
        r.frame().unwrap().encode().unwrap()
    };
    assert_eq!(run(), run());
}

#[test]
fn memory_is_capped() {
    // the tile tries to grow to 200 pages (12.8 MB) under a 1 MiB cap: grow returns -1
    let wat = static_tile(&one_rect(5), 1, "", "(i32.store (i32.const 2400) (memory.grow (i32.const 200)))");
    let b = Budgets { max_memory: 1 << 20, ..Budgets::default() };
    let mut r = TileRunner::load(wat.as_bytes(), TileManifest::new("t", &[]), b, None, 1).unwrap();
    r.frame().unwrap();
    assert_eq!(r.session_read_i32(2400), -1);
}

#[test]
fn hit_regions_come_back_as_events_only_for_the_topmost_region() {
    let mut l = DisplayList::new();
    l.push(Op::Hit { id: 1, r: Rect::new(0, 0, 100, 100) });
    l.push(Op::Hit { id: 2, r: Rect::new(10, 10, 20, 20) });
    let mut r = load(&static_tile(&l.encode().unwrap(), 1, "", "")).unwrap();
    r.frame().unwrap();
    assert_eq!(r.click(15, 15).unwrap(), Some(2));
    assert_eq!(r.click(50, 50).unwrap(), Some(1));
    assert_eq!(r.click(150, 150).unwrap(), None);
}

#[test]
fn reference_renderer_clip_and_alpha() {
    use crate::render::render;
    let mut l = DisplayList::new();
    l.push(Op::PushClip { r: Rect::new(10, 10, 10, 10) });
    l.push(Op::Rect { r: Rect::new(0, 0, 40, 40), color: Color(255, 0, 0, 255) });
    l.push(Op::PopClip);
    l.push(Op::Rect { r: Rect::new(30, 30, 4, 4), color: Color(0, 0, 255, 128) });
    let img = render(&l, 40, 40, Color::rgb(0, 0, 0));
    assert_eq!(img.pixel(15, 15), Color(255, 0, 0, 255));
    assert_eq!(img.pixel(5, 5), Color(0, 0, 0, 255));
    assert_eq!(img.pixel(25, 15), Color(0, 0, 0, 255));
    assert_eq!(img.pixel(31, 31), Color(0, 0, 128, 255));
}

#[test]
fn exporters_escape_text() {
    let mut l = DisplayList::new();
    l.push(Op::Text { x: 0, y: 0, face: 1, size: 10, color: Color(0, 0, 0, 255), text: "a<b>&\"q\"".into() });
    let svg = crate::export::to_svg(&l, 100, 20);
    assert!(svg.contains("a&lt;b&gt;&amp;&quot;q&quot;"));
    let json = crate::export::to_json(&l, 100, 20);
    assert!(json.contains("a<b>&\\\"q\\\""));
}
