// Where the time goes in styling a big page (F-BROX-15, KI-840): the phases of `styled_doc` timed one by one on the synthetic page of
// `dist/scripts/browser-speed-report.sh`. A measurement, not a pass or fail test: `cargo test --profile fast -p iyakki-web --test
// style_speed -- --ignored --nocapture`.
use iyakki_web::{css::Viewport, doc, html, style, url::Url};

fn thread_cpu_time() -> std::time::Duration {
    #[repr(C)]
    struct Timespec {
        tv_sec: i64,
        tv_nsec: i64,
    }
    extern "C" {
        fn clock_gettime(clock: i32, ts: *mut Timespec) -> i32;
    }
    // CLOCK_THREAD_CPUTIME_ID is 16 on macOS and 3 on Linux
    const ID: i32 = if cfg!(target_os = "macos") { 16 } else { 3 };
    let mut ts = Timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: a valid writable timespec of the layout the C library expects (64-bit targets)
    assert_eq!(unsafe { clock_gettime(ID, &mut ts) }, 0);
    std::time::Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

// CPU time of this thread, not wall time: the build host runs other work, and waiting for a core is not the code's cost
struct Instant(std::time::Duration);
impl Instant {
    fn now() -> Instant {
        Instant(thread_cpu_time())
    }
    fn elapsed(&self) -> std::time::Duration {
        thread_cpu_time() - self.0
    }
}

fn page(blocks: usize) -> String {
    let mut s = String::from("<html><head><title>Speed</title><style>body{margin:8px;font-family:sans-serif}.c0{color:#223}.c1{color:#322;background:#eef}.c2{margin:4px 0;padding:2px}.c3{border:1px solid #ccc}.c4 b{color:#a00}.c5 a{color:#06c}.c6{background:#fafafa;padding:4px}h3{margin:6px 0 2px}p{margin:2px 0;line-height:1.4}@media (max-width:600px){.c1{display:none}}</style></head><body>");
    for i in 0..blocks {
        s.push_str(&format!("<div class=\"c{} c{}\"><h3>Item {i}: a heading of ordinary length</h3><p>The quick brown fox jumps over the lazy dog, then <b>reads</b> a <a href=\"/p/{i}\">link to page {i}</a> and keeps going for another line or two so that wrapping happens.</p></div>", i % 7, (i * 3) % 7));
    }
    s.push_str("</body></html>");
    s
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

#[test]
#[ignore = "a measurement"]
fn style_phases_of_a_big_page() {
    let url = Url::parse("https://speed.test/").unwrap();
    let vp = Viewport::default();
    println!("SIZES block {} textblock {} span {} boxstyle {}", std::mem::size_of::<doc::Block>(), std::mem::size_of::<doc::TextBlock>(), std::mem::size_of::<doc::Span>(), std::mem::size_of::<doc::BoxStyle>());
    for n in [1000usize, 4000] {
        let src = page(n);
        let mut rows: Vec<[f64; 6]> = vec![];
        // IYAKKI_SPEED_REPS: more runs, so a sampling profiler (gdb stack samples) has something to catch
        let reps = std::env::var("IYAKKI_SPEED_REPS").ok().and_then(|v| v.parse().ok()).unwrap_or(25usize);
        for _ in 0..reps {
            let t = Instant::now();
            let tree = html::parse(&src);
            let parse = ms(t);
            let t = Instant::now();
            let sheets = style::collect_sheets(&tree, &url, &vp, &|_: &Url| None);
            let collect = ms(t);
            let t = Instant::now();
            let ss = style::StyleSheets::new(&sheets);
            let index = ms(t);
            let t = Instant::now();
            let cs = style::compute(&tree, &ss, &vp);
            let compute = ms(t);
            let t = Instant::now();
            let d = doc::build_styled(&tree, &ss, &vp);
            let total = ms(t);
            let t = Instant::now();
            let a = iyakki_web::a11y::build(&tree, &|_| false);
            let a11y = ms(t);
            drop(a);
            assert!(!d.blocks.is_empty() && !cs.is_empty());
            rows.push([parse, collect + index, compute, total - compute, total, a11y]);
        }
        let med = |i: usize| {
            let mut v: Vec<f64> = rows.iter().map(|r| r[i]).collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v[0]
        };
        println!("STYLEPHASES (best of 25) blocks {n}: parse {:.1} sheets {:.1} compute {:.1} (build_styled minus a second compute) {:.1} build_styled {:.1}; a11y::build alone {:.1}", med(0), med(1), med(2), med(3), med(4), med(5));
    }
}
