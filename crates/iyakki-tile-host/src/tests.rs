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
    let e = load(&static_tile(&one_rect(5), 3, "", "")).err().unwrap();
    assert!(matches!(e, TileError::VersionMismatch { tile: 3, host: 2 }));
    let msg = e.to_string();
    assert!(msg.contains("version 3") && msg.contains("version 2") && msg.contains("Update"), "{msg}");
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


mod pictures {
    use crate::export::to_json_with_images;
    use crate::render::{render_with, ImageTable, TableError, TABLE_BYTES, TABLE_PICTURES};
    use iyakki_tile_abi::{Color, DisplayList, Op, Rect};

    const BG: Color = Color(10, 20, 30, 255);

    fn one(handle: u32, r: Rect) -> DisplayList {
        let mut l = DisplayList::new();
        l.push(Op::Image { handle, r });
        l
    }

    fn two_by_two() -> ImageTable {
        let mut t = ImageTable::new();
        // red, green / blue, see-through yellow
        t.put(5, 2, 2, vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 0]).unwrap();
        t
    }

    #[test]
    fn a_picture_is_scaled_by_nearest_neighbour_and_laid_over_by_its_alpha() {
        let t = two_by_two();
        let img = render_with(&one(5, Rect::new(2, 2, 8, 8)), 12, 12, BG, &t);
        assert_eq!(img.pixel(2, 2), Color(255, 0, 0, 255), "top left quarter is red");
        assert_eq!(img.pixel(5, 5), Color(255, 0, 0, 255));
        assert_eq!(img.pixel(6, 2), Color(0, 255, 0, 255));
        assert_eq!(img.pixel(2, 6), Color(0, 0, 255, 255));
        assert_eq!(img.pixel(7, 7), Color(10, 20, 30, 255), "a fully see-through pixel leaves the background");
        assert_eq!(img.pixel(0, 0), BG, "outside the rectangle nothing changed");
        // a half see-through pixel mixes
        let mut t2 = ImageTable::new();
        t2.put(1, 1, 1, vec![200, 100, 0, 128]).unwrap();
        let m = render_with(&one(1, Rect::new(0, 0, 2, 2)), 2, 2, BG, &t2);
        assert_eq!(m.pixel(1, 1), Color(((200u32 * 128 + 10 * 127 + 127) / 255) as u8, ((100u32 * 128 + 20 * 127 + 127) / 255) as u8, ((0u32 * 128 + 30 * 127 + 127) / 255) as u8, 255));
    }

    #[test]
    fn clips_and_the_surface_edge_cut_a_picture_and_unknown_handles_stay_grey_boxes() {
        let t = two_by_two();
        let mut l = DisplayList::new();
        l.push(Op::PushClip { r: Rect::new(4, 4, 4, 4) });
        l.push(Op::Image { handle: 5, r: Rect::new(0, 0, 12, 12) });
        l.push(Op::PopClip);
        let img = render_with(&l, 12, 12, BG, &t);
        assert_eq!(img.pixel(3, 3), BG);
        assert_ne!(img.pixel(4, 4), BG);
        assert_eq!(img.pixel(8, 8), BG);
        // partly off the surface: no panic, the visible part is right
        let off = render_with(&one(5, Rect::new(8, 8, 8, 8)), 12, 12, BG, &t);
        assert_eq!(off.pixel(8, 8), Color(255, 0, 0, 255));
        // unknown handle, empty rectangle
        let grey = render_with(&one(77, Rect::new(1, 1, 3, 3)), 6, 6, BG, &t);
        assert_eq!(grey.pixel(2, 2), Color(128, 128, 128, 255));
        let none = render_with(&one(5, Rect::new(1, 1, 0, 5)), 6, 6, BG, &t);
        assert_eq!(none.pixel(1, 1), BG);
    }

    #[test]
    fn the_table_is_capped_by_size_count_and_bytes_and_replacing_frees_the_old_bytes() {
        let mut t = ImageTable::new();
        assert_eq!(t.put(1, 0, 1, vec![]), Err(TableError::BadSize));
        assert_eq!(t.put(1, 2, 2, vec![0; 15]), Err(TableError::BadSize));
        assert_eq!(t.put(1, 1025, 1, vec![0; 1025 * 4]), Err(TableError::TooBig));
        for h in 0..TABLE_PICTURES as u32 {
            t.put(h, 1, 1, vec![1, 2, 3, 4]).unwrap();
        }
        assert_eq!(t.put(9999, 1, 1, vec![0; 4]), Err(TableError::TableFull));
        t.put(3, 2, 2, vec![0; 16]).unwrap();
        assert_eq!(t.bytes(), (TABLE_PICTURES - 1) * 4 + 16, "replacing a picture counts only the new bytes");
        assert!(t.remove(3) && !t.remove(3));
        let mut big = ImageTable::new();
        for h in 0..4u32 {
            big.put(h, 1024, 256, vec![0; 1024 * 256 * 4]).unwrap();
        }
        assert_eq!(big.bytes(), TABLE_BYTES);
        assert_eq!(big.put(4, 1, 1, vec![0; 4]), Err(TableError::TableFull));
        assert_eq!(big.len(), 4);
    }

    #[test]
    fn the_browser_frame_carries_only_the_pictures_the_list_names_in_base64() {
        let mut t = two_by_two();
        t.put(6, 1, 1, vec![1, 2, 3, 4]).unwrap();
        let j = to_json_with_images(&one(5, Rect::new(0, 0, 4, 4)), 10, 10, &t);
        assert!(j.contains("\"images\":{\"5\":{\"w\":2,\"h\":2,\"rgba\":\"/wAA/wD/AP8AAP////8AAA==\"}}"), "{j}");
        assert!(!j.contains("\"6\":"), "{j}");
        assert!(j.ends_with("}}"));
        // no picture named: the frame is the plain one
        assert_eq!(to_json_with_images(&DisplayList::new(), 5, 5, &t), crate::export::to_json(&DisplayList::new(), 5, 5));
        // a handle that is not in the table adds nothing
        assert!(!to_json_with_images(&one(42, Rect::new(0, 0, 1, 1)), 5, 5, &t).contains("images"));
    }
}

#[test]
fn a_host_loads_abi_1_and_abi_2_tiles_and_an_abi_2_tile_gives_its_access_tree() {
    use iyakki_tile_abi::{access_flags, Role};
    // ABI 1 tile: loads, gives no access nodes, its hit is unnamed
    let mut l = DisplayList::new();
    l.push(Op::Hit { id: 5, r: Rect::new(0, 0, 40, 20) });
    let mut r = load(&static_tile(&l.encode().unwrap(), 1, "", "")).unwrap();
    r.init(320, 200, 100, THEME).unwrap();
    let f = r.frame().unwrap();
    assert!(f.access_tree().is_empty());
    assert_eq!(f.unnamed_hits(), [5]);
    // ABI 2 tile
    l.push(Op::Access { id: 5, role: Role::Toggle, r: Rect::new(0, 0, 40, 20), name: "Wi-Fi".into(), value: String::new(), flags: access_flags::CHECKED });
    let mut r = load(&static_tile(&l.encode().unwrap(), 2, "", "")).unwrap();
    r.init(320, 200, 100, THEME).unwrap();
    let f = r.frame().unwrap();
    assert_eq!(f.access_tree()[0].name, "Wi-Fi");
    assert!(f.unnamed_hits().is_empty());
    // the reference renderer and both exporters accept the list
    assert!(crate::export::to_json(&f, 320, 200).contains("\"op\":\"access\""));
}
