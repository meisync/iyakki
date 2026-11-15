//! Each test is a small page that reproduces something a real site tripped over (found by the compat probe, see
//! `docs/development/BROWSER-COMPAT.md`), with no network: style sheets and scripts come from a table.

use std::collections::HashMap;
use std::sync::Arc;

use iyakki_web::css::Viewport;
use iyakki_web::fetch::Response;
use iyakki_web::{Fetch, Url};

use crate::{run_page_with, Limits, SiteData};

struct Table(HashMap<String, String>);

impl Fetch for Table {
    fn get(&self, url: &Url) -> Result<Response, String> {
        match self.0.get(&url.to_string()) {
            Some(b) => Ok(Response { status: 200, content_type: if url.path.ends_with(".css") { "text/css".into() } else { "text/javascript".into() }, body: b.clone().into_bytes(), ..Response::default() }),
            None => Err("offline".into()),
        }
    }
}

/// Load `html` (with the given files at `https://a.org/...`), let it settle, run `js` and let its promises settle; the
/// text `js` evaluates to comes back.
fn run_with(html: &str, files: &[(&str, &str)], vp: Option<Viewport>, allow: fn(&str) -> bool, js: &str) -> String {
    let web: Arc<dyn Fetch> = Arc::new(Table(files.iter().map(|(k, v)| (format!("https://a.org{k}"), v.to_string())).collect()));
    let data = SiteData { viewport: vp, ..SiteData::default() };
    let mut out = String::new();
    let res = run_page_with(html, &Url::parse("https://a.org/").unwrap(), web, allow, &data, Limits::default(), |e| {
        let r = e.eval("test", js).unwrap_or_else(|x| format!("THROWN {x}"));
        let _ = e.run_jobs();
        let _ = crate::modules::run_timers(e, 1_000, 100);
        out = if r.starts_with("THROWN") { r } else { e.eval("after", "typeof __out === 'undefined' ? '' : String(__out)").map(|x| if x.is_empty() { r.clone() } else { x }).unwrap_or(r) };
    });
    assert_eq!(res.report.failed, 0, "page scripts failed: {:?}", res.report.notes);
    out
}

fn run(html: &str, js: &str) -> String {
    run_with(html, &[], Some(Viewport { width: 1280.0, height: 720.0, ..Viewport::default() }), |_| true, js)
}

#[test]
fn elements_have_sizes_from_their_styles_instead_of_zero() {
    let page = "<style>html,body{margin:0} .a{width:200px;height:50px;margin:10px} .b{padding:5px;border:2px solid red}</style><div class=a id=a></div><div class=b id=b>hello</div>";
    let r = run(page, "const a = document.getElementById('a'), b = document.getElementById('b'); const ra = a.getBoundingClientRect(); [ra.width, ra.height, ra.x, ra.y, a.offsetWidth, a.offsetHeight, a.clientWidth, b.offsetWidth, b.offsetTop, Math.round(b.offsetHeight), b.clientWidth].join()");
    // b is the full 1280 wide window, its text is one 18 px line (a browser rounds the face's 18.4) plus 2*5 padding and 2*2 border
    assert_eq!(r, "200,50,10,10,200,50,200,1280,70,32,1276", "{r}");
}

#[test]
fn text_wraps_by_the_width_of_its_box() {
    let word = "x".repeat(39);
    let page = format!("<style>#c{{width:100px}}</style><div id=c>{word}</div><div id=d style=\"width:1000px\">{word}</div>");
    let r = run(&page, "[document.getElementById('c').offsetHeight, document.getElementById('d').offsetHeight].join()");
    // 39 glyphs of 8 px are 312 px: four lines of 100 px; in 1000 px they are one line
    assert_eq!(r, "72,18", "{r}");
}

#[test]
fn flex_rows_put_items_side_by_side_and_grow_fills_the_rest() {
    let page = "<style>html,body{margin:0} .row{display:flex;width:600px} .i{width:100px;height:30px} .g{flex-grow:1;height:20px}</style><div class=row id=r><div class=i id=i1></div><div class=i id=i2></div><div class=g id=g></div></div>";
    let r = run(page, "const q = (id) => document.getElementById(id).getBoundingClientRect(); [q('i1').x, q('i2').x, q('g').x, q('g').width, q('i1').y === q('g').y, q('r').height].join()");
    assert_eq!(r, "0,100,200,400,true,30", "{r}");
}

#[test]
fn boxes_that_are_not_shown_or_not_in_the_page_have_no_size() {
    let page = "<style>.n{display:none;width:50px;height:50px}</style><div class=n id=n></div><div id=v style=width:30px;height:30px></div>";
    let r = run(page, "const d = document.createElement('div'); d.style.width = '40px'; const n = document.getElementById('n'); [n.offsetWidth, n.getClientRects().length, d.offsetWidth, d.getBoundingClientRect().height, String(n.offsetParent), document.getElementById('v').offsetParent === document.body].join()");
    assert_eq!(r, "0,0,0,0,null,true", "{r}");
}

#[test]
fn linked_style_sheets_count_if_the_account_may_reach_their_host() {
    let page = "<link rel=stylesheet href=/s.css><div class=x id=x></div>";
    let files = [("/s.css", ".x{width:120px;height:30px}")];
    let js = "const x = document.getElementById('x'); x.offsetWidth + 'x' + x.offsetHeight";
    let win = Some(Viewport { width: 1280.0, height: 720.0, ..Viewport::default() });
    assert_eq!(run_with(page, &files, win, |_| true, js), "120x30");
    // the sheet is fetched through the same host policy: a refused host leaves the box unstyled
    assert_eq!(run_with(page, &files, win, |h| h == "a.org", js), "120x30");
    let blocked = "<link rel=stylesheet href=https://other.example/s.css><div class=x id=x></div>";
    let files = [("/s.css", ".x{width:120px;height:30px}")];
    let r = run_with(blocked, &files, win, |h| h == "a.org", js);
    // no style sheet came, so the page has the browser's own 8 px body margin
    assert_eq!(r, "1264x0", "{r}");
}

#[test]
fn the_tree_changing_changes_the_sizes_and_unchanged_trees_are_measured_once() {
    let page = "<style>.p{height:10px}</style><div id=w></div>";
    let r = run(page, "const w = document.getElementById('w'); const a = w.offsetHeight; for (let i = 0; i < 3; i++) { const d = document.createElement('div'); d.className = 'p'; w.appendChild(d); } const b = w.offsetHeight; w.firstChild.style.height = '25px'; [a, b, w.offsetHeight].join()");
    assert_eq!(r, "0,30,45", "{r}");
}

#[test]
fn computed_style_has_the_values_from_the_sheets_and_inline_style_wins() {
    let page = "<style>.x{color:#ff0000;font-size:20px;display:none} .y{display:flex;padding:3px 4px}</style><div id=x class=x style=\"font-size:30px\"></div><div id=y class=y></div><span id=s>t</span>";
    let r = run(page, "const g = (id, p) => getComputedStyle(document.getElementById(id)).getPropertyValue(p); [g('x', 'color'), g('x', 'display'), g('x', 'font-size'), getComputedStyle(document.getElementById('x')).fontSize, g('y', 'display'), g('y', 'padding-left'), getComputedStyle(document.getElementById('y')).paddingTop, g('s', 'display'), g('y', 'visibility')].join('|')");
    assert_eq!(r, "rgb(255, 0, 0)|none|30px|30px|flex|4px|3px|inline|visible", "{r}");
}

#[test]
fn the_window_and_media_queries_follow_the_given_viewport() {
    let js = "[innerWidth, innerHeight, document.documentElement.clientWidth, matchMedia('(max-width: 700px)').matches, matchMedia('(min-width: 900px)').matches, matchMedia('(min-width: 500px) and (max-height: 500px)').matches, matchMedia('(max-width: 700px)').media].join()";
    let narrow = Viewport { width: 600.0, height: 400.0, ..Viewport::default() };
    assert_eq!(run_with("<p>x", &[], Some(narrow), |_| true, js), "600,400,600,true,false,true,(max-width: 700px)");
    assert_eq!(run("<p>x", js), "1280,720,1280,false,true,false,(max-width: 700px)");
    // a host that gave no window gets 1280 by 720 and no sizes at all (every box is zero, as in an engine without layout)
    assert_eq!(run_with("<p style=width:50px>x</p>", &[], None, |_| true, "[innerWidth, document.querySelector('p').offsetWidth].join()"), "1280,0");
}

#[test]
fn the_pages_device_pixel_ratio_is_the_density_the_page_is_drawn_at_and_follows_a_change() {
    let js = "[devicePixelRatio, innerWidth].join()";
    assert_eq!(run("<p>x", js), "1,1280", "no density given: one device pixel per CSS pixel");
    let dense = Viewport { width: 800.0, height: 600.0, dppx: 2.0, ..Viewport::default() };
    assert_eq!(run_with("<p>x", &[], Some(dense), |_| true, js), "2,800");
    let odd = Viewport { width: 800.0, height: 600.0, dppx: 1.25, ..Viewport::default() };
    assert_eq!(run_with("<p>x", &[], Some(odd), |_| true, js), "1.25,800");
    // the window changes after the page ran: the number follows (the embedder calls resize)
    let mut out = String::new();
    let data = SiteData { viewport: Some(dense), ..SiteData::default() };
    let web: Arc<dyn Fetch> = Arc::new(Table(HashMap::new()));
    run_page_with("<p>x", &Url::parse("https://a.org/").unwrap(), web, |_| true, &data, Limits::default(), |e| {
        let first = e.eval("a", "String(devicePixelRatio)").unwrap();
        crate::resize(e, Viewport { width: 600.0, height: 400.0, dppx: 3.0, ..Viewport::default() }).unwrap();
        let second = e.eval("b", "[devicePixelRatio, innerWidth].join()").unwrap();
        out = format!("{first} then {second}");
    });
    assert_eq!(out, "2 then 3,600");
}

#[test]
fn intersection_observers_report_what_is_inside_the_window_and_resize_observers_report_sizes() {
    let page = "<div style=height:2000px></div><div id=top style=\"height:10px;width:20px\"></div>";
    let js = "var __out = ''; const io = new IntersectionObserver((es) => { for (const e of es) __out += (e.target.id || 'tall') + ':' + e.isIntersecting + ' '; }); io.observe(document.body.firstElementChild); io.observe(document.getElementById('top')); const ro = new ResizeObserver((es) => { __out += 'size ' + es[0].contentRect.width + 'x' + es[0].contentRect.height; }); ro.observe(document.getElementById('top')); 'started'";
    let r = run(page, js);
    // the tall box starts at the top and is inside the window; the second is below the 720 px window
    assert_eq!(r, "tall:true top:false size 20x10", "{r}");
}

#[test]
fn mutation_observers_see_children_attributes_and_text_changes() {
    let page = "<div id=t><span id=s>a</span></div>";
    let js = "var __out = ''; const t = document.getElementById('t'); const mo = new MutationObserver((rs) => { for (const r of rs) __out += r.type + ':' + (r.attributeName || (r.addedNodes.length + '+' + r.removedNodes.length)) + (r.oldValue ? '(' + r.oldValue + ')' : '') + ' '; }); mo.observe(t, { childList: true, attributes: true, subtree: true, attributeOldValue: true, characterData: true }); t.appendChild(document.createElement('b')); t.setAttribute('class', 'k'); document.getElementById('s').setAttribute('title', 'x'); t.removeChild(t.firstChild); t.innerHTML = '<i></i>'; 'queued'";
    let r = run(page, js);
    assert_eq!(r, "childList:1+0 attributes:class attributes:title childList:0+1 childList:1+1 ", "{r}");
    // a record is delivered once, and nothing after disconnect
    let r = run("<div id=t></div>", "var __out = 'n='; const t = document.getElementById('t'); const mo = new MutationObserver((rs) => { __out += rs.length; }); mo.observe(t, { childList: true }); t.append(document.createElement('a')); mo.disconnect(); t.append(document.createElement('b')); 'x'");
    assert_eq!(r, "n=", "{r}");
}

#[test]
fn a_2d_canvas_context_accepts_drawing_and_measures_text() {
    // the fingerprint check medium.com runs at start-up: it set `fillStyle` on `getContext('2d')`, which was null
    let js = "const c = document.createElement('canvas'); c.width = 1; c.height = 1; const m = c.getContext('2d'); m.fillStyle = 'rgba(0,0,0,0)'; m.fillRect(0, 0, 1, 1); m.font = '20px serif'; [m === c.getContext('2d'), m.canvas === c, m.getImageData(0, 0, 1, 1).data.length, m.measureText('abcd').width, String(c.getContext('webgl')), c.width].join()";
    assert_eq!(run("<p>x", js), "true,true,4,40,null,1");
}

#[test]
fn object_urls_exist_for_blobs() {
    // openstreetmap.org: `URL.createObjectURL(new Blob([code], { type: 'text/javascript' }))`
    let r = run("<p>x", "const u = URL.createObjectURL(new Blob(['x'], { type: 'text/javascript' })); const ok = u.startsWith('blob:https://a.org/'); URL.revokeObjectURL(u); ok + ',' + (URL.createObjectURL(new Blob([])) !== u)");
    assert_eq!(r, "true,true");
}

#[test]
fn attributes_can_be_read_by_name_and_attribute_nodes_work() {
    // duckduckgo.com: `f.attributes[u].expando`; npr.org's consent script: `document.createAttribute(...)` and `setAttributeNode`
    let js = "const d = document.createElement('div'); d.setAttribute('onclick', 't'); d.setAttribute('data-x', '1'); const a = document.createAttribute('data-ot-ignore'); d.setAttributeNode(a); a.value = 'y'; [d.attributes.onclick.value, d.attributes['data-x'].name, d.attributes[0].name, d.attributes.length, d.attributes.nope === undefined, String(d.attributes.onclick.expando), d.hasAttribute('data-ot-ignore'), d.getAttributeNode('data-x').value, d.attributes.getNamedItem('onclick').value, 'onclick' in d.attributes].join()";
    assert_eq!(run("<p>x", js), "t,data-x,onclick,3,true,undefined,true,1,t,true");
}

#[test]
fn small_members_pages_probe_for_exist_and_behave() {
    let js = "const d = document.createElement('dialog'); d.show(); const open1 = d.open; d.close(); const v = document.createElement('video'); const i = document.createElement('input'); i.value = 'hello'; i.setSelectionRange(1, 3); const f = document.createElement('form'); let sent = 0; f.addEventListener('submit', (e) => { e.preventDefault(); sent++; }); f.requestSubmit(); const e = document.createElement('div'); e.setAttributeNS(null, 'role', 'button'); e.ariaLabel = 'Go'; [open1, d.open, Number.isNaN(v.duration), v.currentTime, v.canPlayType('video/mp4') === '', i.files.length, i.selectionStart + '-' + i.selectionEnd, i.validity.valid, sent, e.getAttribute('aria-label'), e.role, typeof navigator.locks.request, navigator.mediaSession.playbackState, typeof ReportingObserver, typeof navigator.storage.estimate, navigator.connection.effectiveType, typeof navigator.credentials.get, document.scrollingElement === document.documentElement, new Intl.Locale('en-US').maximize().language, e.webkitMatchesSelector('div'), e.checkVisibility()].join()";
    assert_eq!(run("<p>x", js), "true,false,true,0,true,0,1-3,true,1,Go,button,function,none,function,function,4g,function,true,en,true,false");
}



#[test]
fn cache_storage_keeps_responses_for_the_visit() {
    // service-worker style code and "offline" libraries call caches.open(...).put/match; the stub always said "nothing here"
    let js = "var __out = ''; (async () => { const c = await caches.open('v1'); await c.put('/a.json', new Response('{\"n\":1}', { status: 200 })); const hit = await c.match('/a.json'); const miss = await c.match('/b.json'); __out = [await hit.text(), miss === undefined, (await c.keys()).length, await caches.has('v1'), await caches.has('v2'), (await caches.keys()).join('+'), await c.delete('/a.json'), (await c.keys()).length].join(); })(); 'started'";
    assert_eq!(run("<p>x", js), "{\"n\":1},true,1,true,false,v1,true,0");
}

#[test]
fn href_and_src_are_worked_out_against_the_page_address_even_when_it_changes() {
    // the address of a link is remembered per value (a table of contents reads every link twice); pushState moves the base
    let js = "const a = document.createElement('a'); a.setAttribute('href', 'y'); const first = a.href; const again = a.href; history.pushState(null, '', '/dir/'); const moved = a.href; history.pushState(null, '', '/other/'); const i = document.createElement('img'); i.setAttribute('src', 'p.png'); [first, again, moved, a.href, i.src].join()";
    assert_eq!(run("<p>x", js), "https://a.org/y,https://a.org/y,https://a.org/dir/y,https://a.org/other/y,https://a.org/other/p.png");
}

#[test]
fn the_content_of_a_shadow_root_has_boxes_like_the_light_content_it_stands_for() {
    let page = "<style>x-card{display:block}</style><x-card id=card></x-card><script>const r = document.getElementById('card').attachShadow({ mode: 'open' }); r.innerHTML = '<p id=in style=\"width:90px;height:20px\"></p>';</script>";
    let r = run(page, "const p = document.getElementById('card').shadowRoot.querySelector('#in'); const b = p.getBoundingClientRect(); [b.width, b.height, document.getElementById('card').offsetHeight, p.offsetParent === document.body].join()");
    assert_eq!(r, "90,20,20,true", "{r}");
}

/// How many nodes the page's tree has taken up after `js` ran (released numbers are used again, so this stays small for a loop).
fn node_count_after(html: &str, js: &str) -> (usize, String) {
    let web: Arc<dyn Fetch> = Arc::new(Table(HashMap::new()));
    let mut out = (0, String::new());
    let res = run_page_with(html, &Url::parse("https://a.org/").unwrap(), web, |_| true, &SiteData::default(), Limits { wall: std::time::Duration::from_secs(300), ..Limits::default() }, |e| {
        let r = e.eval("test", js).unwrap_or_else(|x| format!("THROWN {x}"));
        let _ = e.run_jobs();
        out = (e.module::<crate::modules::DomModule>("dom").map_or(0, |d| d.tree.nodes.len()), r);
    });
    assert_eq!(res.report.failed, 0, "{:?}", res.report.notes);
    out
}

#[test]
fn nodes_a_script_made_and_dropped_are_given_back_so_a_long_loop_never_runs_out() {
    // the Rust book's searcher makes an `a` element for every address it parses, hundreds of thousands of times: the limit is 400 000 nodes
    let (n, r) = node_count_after("<p>x", "let len = 0; for (let i = 0; i < 410000; i++) { const a = document.createElement('a'); a.setAttribute('href', '/p' + i); len += a.href.length > 0 ? 1 : 0; } len");
    assert_eq!(r, "410000");
    assert!(n < 5000, "the tree grew to {n} nodes");
}

#[test]
fn replaced_content_nobody_holds_is_given_back_and_ids_still_find_the_right_nodes() {
    let page = "<div id=box></div>";
    let js = "const box = document.getElementById('box'); for (let i = 0; i < 20000; i++) { box.innerHTML = '<p id=\"a' + (i % 3) + '\">row ' + i + '</p><span>s</span>'; } const p = document.getElementById('a' + (19999 % 3)); const gone = document.getElementById('a' + ((19999 + 1) % 3)); [p.textContent, gone === null, box.children.length].join()";
    let (n, r) = node_count_after(page, js);
    assert_eq!(r, "row 19999,true,2");
    assert!(n < 100, "the tree grew to {n} nodes");
}

#[test]
fn a_node_a_script_still_holds_or_has_put_somewhere_is_never_taken_back() {
    let js = "const keep = []; for (let i = 0; i < 3000; i++) { const d = document.createElement('div'); d.id = 'k' + i; d.expando = i; if (i % 2) keep.push(d); else document.body.appendChild(d); } const dropped = document.createElement('i'); dropped.expando = 'x'; document.body.appendChild(dropped); const after = []; for (let i = 0; i < 5000; i++) document.createElement('b'); [keep[10].expando, keep[10].id, document.getElementById('k8').expando, document.getElementById('k9'), dropped.expando, document.querySelectorAll('div').length].join()";
    let (_, r) = node_count_after("<p>x", js);
    // k9 was kept in the array and never inserted, so the page cannot find it; k8 was inserted and keeps its own property
    assert_eq!(r, "21,k21,8,,x,1500");
}

const WIN: Viewport = Viewport { width: 1280.0, height: 720.0, reduce_motion: false, more_contrast: false, dppx: 1.0 };

fn rect_of(page: &str, ids: &[&str]) -> Vec<Vec<i32>> {
    let js = format!("const q = (id) => {{ const r = document.getElementById(id).getBoundingClientRect(); return [r.x, r.y, r.width, r.height].map(Math.round).join(); }}; {}.map(q).join('|')", serde_json::to_string(ids).unwrap());
    run(page, &js).split('|').map(|s| s.split(',').map(|v| v.parse().unwrap()).collect()).collect()
}

#[test]
fn box_sizing_border_box_counts_padding_and_border_inside_the_width() {
    let page = "<style>.a{width:200px;height:100px;padding:10px;border:5px solid}.b{box-sizing:border-box}</style><div class='a b' id=a></div><div class=a id=c></div>";
    let r = rect_of(page, &["a", "c"]);
    assert_eq!(r[0][2..], [200, 100]);
    assert_eq!(r[1][2..], [230, 130]);
    assert_eq!(run(page, "document.getElementById('a').clientWidth + ',' + getComputedStyle(document.getElementById('a')).boxSizing"), "190,border-box");
}

#[test]
fn absolute_boxes_sit_against_the_positioned_box_around_them_and_fixed_ones_against_the_window() {
    let page = "<style>html,body{margin:0} .p{position:relative;margin:20px;width:300px;height:200px} .c{position:absolute;right:10px;bottom:5px;width:50px;height:40px} .d{position:absolute;top:7px;left:3px} .f{position:fixed;top:0;right:0;width:100px;height:30px} .s{height:10px}</style><div class=s></div><div class=p id=p><div class=c id=c></div><span class=d id=d>hi</span><div class=f id=f></div></div><div id=after style=height:5px></div>";
    let r = rect_of(page, &["p", "c", "d", "f", "after"]);
    // the first block is 10 px high, so the positioned box starts at 20 (its margin) + 10 with collapsing margins ignored by the 10 px sibling
    let (px, py) = (r[0][0], r[0][1]);
    assert_eq!((px, py, r[0][2], r[0][3]), (20, 30, 300, 200));
    assert_eq!(r[1], vec![px + 300 - 10 - 50, py + 200 - 5 - 40, 50, 40], "right/bottom");
    assert_eq!((r[2][0], r[2][1], r[2][2]), (px + 3, py + 7, 12), "top/left, shrink to the text (h and i are 0.778 em of 16 px)");
    assert_eq!(r[3], vec![1180, 0, 100, 30], "fixed against the window");
    // absolute and fixed boxes take no room in the flow: the box after the positioned one follows its 200 px
    assert_eq!(r[4][1], py + 200 + 20);
}

#[test]
fn relative_offsets_move_a_box_but_not_what_follows_it() {
    let page = "<style>html,body{margin:0} .s{height:10px} .r{position:relative;left:15px;top:5px;width:50px;height:50px}</style><div class=s></div><div class=r id=r></div><div class=s id=n></div>";
    let r = rect_of(page, &["r", "n"]);
    assert_eq!(r[0], vec![15, 15, 50, 50]);
    assert_eq!(r[1][1], 60);
}

#[test]
fn floats_sit_at_the_edges_text_gets_narrower_and_only_a_containing_box_keeps_them_in_its_height() {
    let page = "<style>html,body{margin:0} p{margin:0} .l{float:left;width:100px;height:80px} .r{float:right;width:50px;height:30px} #w{width:400px} #o{width:400px;overflow:hidden} #k{clear:both;height:10px}</style><div id=w><div class=l id=l></div><div class=r id=r></div><p id=t>text</p></div><div id=o><div class=l></div><p>text</p></div><div id=k></div>";
    let r = rect_of(page, &["l", "r", "w", "o", "k"]);
    assert_eq!(r[0], vec![0, 0, 100, 80]);
    assert_eq!(r[1], vec![350, 0, 50, 30]);
    assert_eq!(r[2][3], 18, "a block that does not contain its floats is as high as its text");
    assert_eq!(r[3][3], 80, "overflow:hidden contains them");
    assert_eq!(r[4][1], 18 + 80, "clear: both goes below the floats");
    // the text is narrower beside a float
    let lines = |floated: &str| {
        let words = "word ".repeat(30);
        let page = format!("<style>p{{margin:0}} div{{width:300px}} .l{{float:left;width:150px;height:200px}}</style><div>{floated}<p id=t>{words}</p></div>");
        run(&page, "document.getElementById('t').offsetHeight + ''").parse::<i32>().unwrap()
    };
    assert!(lines("<div class=l></div>") > lines(""), "{} vs {}", lines("<div class=l></div>"), lines(""));
}

#[test]
fn a_resize_moves_the_window_numbers_fires_media_query_changes_and_makes_observers_look_again() {
    let page = "<style>html,body{margin:0} #b{width:50%;height:10px}@media (max-width: 700px){#b{width:100%}}</style><div id=b></div>";
    let web: Arc<dyn Fetch> = Arc::new(Table(HashMap::new()));
    let data = SiteData { viewport: Some(WIN), ..SiteData::default() };
    let mut out = String::new();
    run_page_with(page, &Url::parse("https://a.org/").unwrap(), web, |_| true, &data, Limits::default(), |e| {
        e.eval("setup", "var log = []; const mq = matchMedia('(max-width: 700px)'); mq.addEventListener('change', (ev) => log.push('mq:' + ev.matches)); window.addEventListener('resize', () => log.push('resize:' + innerWidth)); const ro = new ResizeObserver((es) => log.push('ro:' + Math.round(es[0].contentRect.width))); ro.observe(document.getElementById('b')); log.push('w0:' + document.getElementById('b').offsetWidth)").unwrap();
        let _ = e.run_jobs();
        crate::resize(e, Viewport { width: 600.0, height: 800.0, ..Viewport::default() }).unwrap();
        crate::resize(e, Viewport { width: 650.0, height: 800.0, ..Viewport::default() }).unwrap();
        out = e.eval("read", "log.join(' ') + ' w1:' + document.getElementById('b').offsetWidth").unwrap();
    });
    // 640 of 1280, then the narrow rule gives the whole 600 (observer first sees 640); a second resize that keeps the answer fires no change
    assert_eq!(out, "w0:640 ro:640 mq:true resize:600 ro:600 resize:650 ro:650 w1:650", "{out}");
}

#[test]
fn intl_formatters_can_be_called_without_new() {
    // learn.microsoft.com: `Intl.DateTimeFormat().resolvedOptions()` threw "class constructors must be invoked with 'new'"
    let js = "[typeof Intl.DateTimeFormat().format, Intl.NumberFormat('en').format(1234.5).length > 0, new Intl.DateTimeFormat() instanceof Intl.DateTimeFormat, typeof Intl.Collator().compare].join()";
    assert_eq!(run("<p>x", js), "function,true,true,function");
}

#[test]
fn a_tree_walker_without_a_usable_root_ends_instead_of_throwing() {
    // learn.microsoft.com's docs script walks text nodes of a node it may not have found
    let js = "const w = document.createTreeWalker(document.getElementById('missing') || undefined, 4); const t = document.createTreeWalker(document.body, 4); [w.nextNode(), t.nextNode() !== null].join()";
    assert_eq!(run("<p>x", js), ",true");
}

#[test]
fn percentage_heights_follow_a_parent_whose_height_is_known_and_are_auto_otherwise() {
    let page = "<style>html,body{height:100%;margin:0} #a{height:50%} #w{height:300px} #h{height:50%} #x{height:50%} #u{height:50%;border:0}</style><div id=a></div><div id=w><div id=h></div></div><div><div id=x>text</div></div><div id=o style=\"position:relative;height:200px\"><div id=u style=\"position:absolute;height:25%\"></div></div>";
    let r = rect_of(page, &["a", "h", "x", "u"]);
    assert_eq!(r[0][3], 360, "html and body are as high as the 720 px window, so half of that");
    assert_eq!(r[1][3], 150, "half of a 300 px parent");
    assert_eq!(r[2][3], 18, "the parent has no height of its own, so the percentage is auto and the line decides");
    assert_eq!(r[3][3], 50, "an absolute box takes a share of its containing block");
}

#[test]
fn line_height_comes_from_styles_and_is_inherited_as_a_number_or_a_length() {
    let page = "<style>p{margin:0} #a{line-height:30px} #b{line-height:2;font-size:10px} #c{line-height:150%;font-size:20px} #d{font:italic 12px/18px serif} #e{line-height:2} #e span{font-size:30px} #n{line-height:normal}</style><p id=a>x</p><p id=b>x</p><p id=c>x</p><p id=d>x</p><p id=e><span>x</span></p><p id=n>x</p>";
    let r = rect_of(page, &["a", "b", "c", "d", "e", "n"]);
    let h: Vec<i32> = r.iter().map(|v| v[3]).collect();
    assert_eq!(h, vec![30, 20, 30, 18, 60, 18], "{h:?}");
    let cs = run(page, "const g = (i) => getComputedStyle(document.getElementById(i)).lineHeight; [g('a'), g('b'), g('n')].join()");
    assert_eq!(cs, "30px,20px,normal");
}

#[test]
fn text_is_measured_with_the_widths_of_the_letters() {
    // 20 letters: narrow ones fit one line of 100 px, wide ones do not
    let page = "<style>p{margin:0;width:100px;font-size:10px} </style><p id=n>iiiiiiiiiiiiiiiiiiii</p><p id=w>MMMMMMMMMMMMMMMMMMMM</p><p id=m style=\"font-family:monospace;width:100px\">xxxxxxxxxxxxxxxxxxxx</p>";
    let r = rect_of(page, &["n", "w", "m"]);
    assert_eq!(r[0][3], 11, "20 * 2.2 px is one line");
    assert_eq!(r[1][3], 11 * 2, "20 * 8.3 px is two lines");
    assert_eq!(r[2][3], 11 * 2, "20 * 6 px is two lines in a fixed-pitch face");
}

#[test]
fn z_index_is_reported_for_positioned_boxes_and_decides_what_is_under_a_point() {
    let page = "<style>body{margin:0} div{width:100px;height:100px} .p{position:absolute;left:0;top:0} #lo{z-index:1} #hi{z-index:5} #neg{z-index:-1} #fl{float:left}</style><div id=flow></div><div class=p id=hi></div><div class=p id=lo></div><div class=p id=neg></div>";
    let r = run(page, "const z = (i) => getComputedStyle(document.getElementById(i)).zIndex; const top = document.elementFromPoint(10, 10); const all = document.elementsFromPoint(10, 10).map((e) => e.id).filter(Boolean); [z('hi'), z('lo'), z('flow'), top.id, all.join('>')].join(' ')");
    assert_eq!(r, "5 1 auto hi hi>lo>flow>neg", "{r}");
    let r = run(page, "[document.elementFromPoint(-1, 5), document.elementFromPoint(5000, 5)].join()");
    assert_eq!(r, ",", "{r}");
    // a later box is on top of an earlier one at the same level; a hidden one is not hit
    let page = "<style>body{margin:0} div{width:50px;height:50px;position:absolute;top:0;left:0}</style><div id=a></div><div id=b></div><div id=c style=\"visibility:hidden\"></div>";
    assert_eq!(run(page, "document.elementFromPoint(5, 5).id"), "b");
}

#[test]
fn the_window_scrolls_within_the_page_and_fires_one_scroll_event_a_task_later() {
    let page = "<style>body{margin:0} #t{height:3000px;width:100px}</style><div id=t></div>";
    let js = "var log = []; window.addEventListener('scroll', () => log.push('w:' + scrollY)); document.addEventListener('scroll', () => log.push('d')); \
        scrollTo(0, 500); scrollBy(0, 100); scroll({ top: 700, left: 10 }); log.push('now:' + scrollY + ',' + scrollX + ',' + pageYOffset); \
        __out = 1";
    let web: Arc<dyn Fetch> = Arc::new(Table(HashMap::new()));
    let data = SiteData { viewport: Some(WIN), ..SiteData::default() };
    let mut out = String::new();
    run_page_with(page, &Url::parse("https://a.org/").unwrap(), web, |_| true, &data, Limits::default(), |e| {
        e.eval("t", js).unwrap();
        let _ = e.run_jobs();
        let early = e.eval("a", "log.join(' ')").unwrap();
        let _ = crate::modules::run_timers(e, 1_000, 100);
        let late = e.eval("b", "scrollTo(0, 99999); const far = scrollY; scrollTo(-5, -5); [far, scrollY, document.documentElement.scrollTop, document.scrollingElement === document.documentElement].join()").unwrap();
        let _ = crate::modules::run_timers(e, 1_000, 100);
        out = format!("{early} | {} | {late}", e.eval("c", "log.join(' ')").unwrap());
    });
    // 3000 px of page in a 720 px window: at most 2280 (the document hears the event before the window); the three calls make one event; x cannot move in a page as wide as the window
    assert_eq!(out, "now:700,0,700 | now:700,0,700 d w:700 d w:0 | 2280,0,0,true", "{out}");
}

#[test]
fn a_box_with_more_content_than_room_scrolls_inside_and_reports_the_content_size() {
    let page = "<style>body{margin:0} #s{width:100px;height:100px;overflow:auto} #in{height:400px;width:250px}</style><div id=s><div id=in></div></div>";
    let js = "const s = document.getElementById('s'); var log = []; s.addEventListener('scroll', () => log.push(s.scrollTop + ',' + s.scrollLeft)); \
        const size = [s.scrollHeight, s.scrollWidth, s.clientHeight].join(); s.scrollTop = 150; s.scrollTop = 99999; s.scrollTo({ left: 40 }); s.scrollBy(0, -20); \
        __out = size + ' ' + s.scrollTop + ',' + s.scrollLeft";
    let r = run(page, js);
    assert_eq!(r, "400,250,100 280,40", "{r}");
    let r = run(page, "const s = document.getElementById('s'); var log = []; s.addEventListener('scroll', () => log.push(s.scrollTop)); s.scrollTop = 10; s.scrollTop = 20; __out = 1; setTimeout(() => { __out = log.join(); }, 5)");
    assert_eq!(r, "20", "two writes in one task make one event: {r}");
}

#[test]
fn scroll_into_view_moves_the_window_to_the_box() {
    let page = "<style>body{margin:0} #a{height:2000px} #b{height:100px}</style><div id=a></div><div id=b></div>";
    let r = run(page, "document.getElementById('b').scrollIntoView(); const a = scrollY; document.getElementById('b').scrollIntoView({ block: 'end' }); [a, scrollY, document.getElementById('b').getBoundingClientRect().top].join()");
    assert_eq!(r, "1380,1380,620", "{r}");
}

#[test]
fn an_absolute_box_with_no_offsets_moves_with_the_parents_that_were_moved_after_it_was_found() {
    // the flex row pushes its item to the right edge after the item's content (the absolute box) was met: the box stays with the item
    let page = "<style>body{margin:0} #cb{position:relative;width:400px;height:100px} #row{display:flex;justify-content:flex-end;width:400px} #item{width:100px;height:20px} #abs{position:absolute;width:10px;height:10px}</style><div id=cb><div id=row><div id=item><div id=abs></div></div></div></div>";
    let r = rect_of(page, &["item", "abs"]);
    assert_eq!((r[0][0], r[0][1]), (300, 0), "the item sits at the end of the row");
    assert_eq!((r[1][0], r[1][1], r[1][2], r[1][3]), (300, 0, 10, 10), "the absolute box with no offsets stands where it would have stood inside the item");
}

#[test]
fn a_box_inside_a_scrolled_box_reports_its_place_moved_back_by_the_scroll() {
    let page = "<style>body{margin:0} #s{width:200px;height:100px;overflow:auto} #t{width:150px} .tall{height:400px}</style><div id=s><div class=tall id=t></div><div id=in style=\"height:50px\"></div></div>";
    let js = "var s = document.getElementById('s'), e = document.getElementById('in'); var a = e.getBoundingClientRect().top; s.scrollTop = 120; var b = e.getBoundingClientRect().top; var c = e.getClientRects()[0].top; __out = [a, b, c, s.scrollTop, s.getBoundingClientRect().top].join(',')";
    let out = run(page, js);
    assert_eq!(out, "400,280,280,120,0", "the scroller itself does not move, its content does");
}

#[test]
fn the_reports_scroll_is_where_the_scripts_left_the_window() {
    let page = "<style>body{margin:0} .t{height:3000px}</style><div class=t></div><script>window.scrollTo(0, 400)</script>";
    let data = SiteData { viewport: Some(Viewport { width: 800.0, height: 600.0, ..Viewport::default() }), ..SiteData::default() };
    let r = crate::run_page(page, &Url::parse("https://a.org/").unwrap(), Arc::new(Table(HashMap::new())), |_| true, &data, Limits::default());
    assert_eq!(r.report.scroll, Some((0, 400)));
    let none = crate::run_page("<div style=height:3000px></div>", &Url::parse("https://a.org/").unwrap(), Arc::new(Table(HashMap::new())), |_| true, &data, Limits::default());
    assert_eq!(none.report.scroll, None, "a page nobody scrolled starts at the top");
    assert_eq!(crate::scroll_in(&["x".to_string(), format!("{}0,400", crate::SCROLL_NOTE)]), Some((0.0, 400.0)));
    assert_eq!(crate::scroll_in(&[format!("{}-3,x", crate::SCROLL_NOTE)]), None);
}

#[test]
fn margins_pass_through_a_box_with_no_frame_and_collapse_through_empty_boxes_as_a_real_browser_does() {
    // numbers checked against a real Chrome by `boxes_chrome_tests`; these run without one
    let page = "<style>html,body{margin:0;padding:0} #o{margin:30px;height:10px} #i{margin:15px 0;height:20px} #f{margin-top:5px;border-top:1px solid #000} #g{margin:25px 0;height:10px} #e1{margin:15px 0} #e2{margin:40px 0;height:0} #n{margin:10px 0;height:20px}</style><div id=o><div id=i></div></div><div id=f><div id=g></div></div><div id=e1></div><div id=e2></div><div id=n></div>";
    let r = rect_of(page, &["o", "i", "f", "g", "e1", "e2", "n"]);
    let y: Vec<i32> = r.iter().map(|v| v[1]).collect();
    // #o and its first child share the larger margin (30): both start at 30; #f has a border, so #g's margin stays inside it
    assert_eq!(y[0], 30, "{y:?}");
    assert_eq!(y[1], 30, "the child starts at its parent's top edge: {y:?}");
    assert_eq!(y[2], 70 + 5 - 5 + 0, "#o is 10 high (40) then the larger of its 30 and #f's 5: {y:?}");
    assert_eq!(y[3], y[2] + 1 + 25, "{y:?}");
    // the empty boxes' margins (15 and 40) collapse with the 25 below #g's box and the 10 above #n: the largest, 40, is the gap
    let g_bottom = r[3][1] + r[3][3];
    let f_bottom = r[2][1] + r[2][3];
    assert_eq!(r[6][1], f_bottom + 40, "#f ends at {f_bottom} (its last child's margin passes out of the box: {g_bottom}): {y:?}");
}

#[test]
fn percentage_offsets_tables_share_columns_and_max_height_clamps() {
    let page = "<style>html,body{margin:0} #p{position:relative;width:300px;height:200px} #a{position:absolute;left:25%;top:10%;width:10px;height:10px} #t{border-collapse:collapse;width:400px} td{padding:0;height:10px} #t1{width:100px} #m{width:50px;height:100px;max-height:30px}</style><div id=p><div id=a></div></div><table id=t><tr><td id=t1></td><td id=t2></td><td id=t3></td></tr><tr><td id=u1></td><td id=u2></td><td id=u3></td></tr></table><div id=m></div>";
    let r = rect_of(page, &["a", "t1", "t2", "u1", "u2", "u3", "m"]);
    assert_eq!((r[0][0], r[0][1]), (75, 20), "25 percent of the 300 wide block, 10 percent of its 200 height");
    assert_eq!((r[1][2], r[2][2]), (100, 150), "one column of 100 and the others share the rest");
    assert_eq!((r[3][2], r[4][2], r[5][2]), (100, 150, 150), "the second row has the same columns, not its own");
    assert_eq!(r[6][3], 30, "max-height cuts the 100 high box");
}

#[test]
fn the_offset_of_a_box_is_measured_from_the_pages_corner_whatever_the_body_does() {
    let page = "<style>html,body{margin:0} .a{height:50px;margin:10px}</style><div class=a></div><div id=b style=height:10px></div>";
    // the first margin passes through the body (the body starts at 10), and the offset still counts from the corner of the page
    assert_eq!(run(page, "document.getElementById('b').offsetTop + ',' + document.body.getBoundingClientRect().y"), "70,10");
}

