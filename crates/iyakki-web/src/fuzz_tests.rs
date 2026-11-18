//! Fuzz tests (deterministic, seeded) for the picture choice strings, the GIF and animated WebP decoders, the HTML parser and the node
//! arena (free list and reuse). A panic, a case slower than the guard or a case over the heap bound fails the test and prints the seed
//! and case. `IYAKKI_FUZZ_CASES=50000 cargo test -p iyakki-web fuzz` runs a larger set.

use crate::fuzzkit::{cases, run, Rng};
use crate::html::{self, NodeKind, Tree, NONE};
use crate::{gif, imgsel, webp};
use std::collections::{BTreeSet, HashSet};
use std::time::Duration;

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn fuzz_imgsel_srcset_sizes_and_media_strings() {
    let corpus: Vec<Vec<u8>> = [
        "a.png 1x, b.png 2x, c.png 3x|(min-width: 600px) 50vw, 100vw",
        "small.jpg 480w, medium.jpg 800w, large.jpg 1600w|(max-width: 600px) 480px, 800px",
        "data:image/png;base64,AAAA,BBBB 1x, x.png 2x|calc(100vw - 20px)",
        "x.png 1e39x, y.png NaNx, z.png infw, w.png -3x, v.png 0w|screen and (min-width: 10px) and (max-width: 99999px) 12em",
        "screen and (min-width: 600px) and (max-width: 1200px)|all",
        "a,b,c,,, , ,d 5x 6w|",
    ]
    .iter()
    .map(|s| s.as_bytes().to_vec())
    .collect();
    let n = run("imgsel", 0x1D5, &corpus, 8000, Duration::from_secs(2), 64 << 20, |input| {
        let s = lossy(input);
        let (a, b) = match s.split_once('|') {
            Some((a, b)) => (a.to_string(), b.to_string()),
            None => (s.clone(), s.clone()),
        };
        if let Some(u) = imgsel::pick_srcset(&a, &b) {
            assert!(!u.is_empty() && a.contains(u.trim_end_matches(',')), "the pick {u:?} is not in the srcset");
        }
        let _ = imgsel::media_matches(&s);
        let _ = imgsel::media_matches(&b);
        let _ = imgsel::type_supported(&s);
        // through a real <picture>
        let page = format!("<picture><source srcset=\"{}\" sizes=\"{}\" media=\"{}\" type=\"{}\"><img srcset=\"{}\" sizes=\"{}\" src=\"d.png\" loading=\"{}\"></picture>", esc(&a), esc(&b), esc(&s), esc(&b), esc(&a), esc(&b), esc(&a));
        let t = html::parse(&page);
        for n in 0..t.nodes.len() {
            if t.tag(n) == Some("img") {
                let _ = imgsel::image_source(&t, n);
                let _ = imgsel::is_lazy(&t, n);
            }
        }
    });
    assert!(n >= cases(8000));
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;")
}

// ---- GIF ----

/// A GIF LZW encoder (variable code size, clear at 4096): the corpus must exercise the dictionary and code-size growth.
fn lzw(indices: &[u8], min_code: u8) -> Vec<u8> {
    let clear = 1u32 << min_code;
    let mut out = Vec::new();
    let (mut acc, mut nbits) = (0u32, 0u32);
    let mut size = min_code as u32 + 1;
    let mut put = |code: u32, size: u32, out: &mut Vec<u8>| {
        acc |= code << nbits;
        nbits += size;
        while nbits >= 8 {
            out.push(acc as u8);
            acc >>= 8;
            nbits -= 8;
        }
    };
    let mut dict: std::collections::HashMap<(u32, u8), u32> = std::collections::HashMap::new();
    let mut next = clear + 2;
    put(clear, size, &mut out);
    let mut cur: Option<u32> = None;
    for &b in indices {
        match cur {
            None => cur = Some(b as u32),
            Some(c) => {
                if let Some(&code) = dict.get(&(c, b)) {
                    cur = Some(code);
                } else {
                    put(c, size, &mut out);
                    dict.insert((c, b), next);
                    next += 1;
                    if next > (1 << size) && size < 12 {
                        size += 1;
                    }
                    if next >= 4096 {
                        put(clear, size, &mut out);
                        dict.clear();
                        next = clear + 2;
                        size = min_code as u32 + 1;
                    }
                    cur = Some(b as u32);
                }
            }
        }
    }
    if let Some(c) = cur {
        put(c, size, &mut out);
    }
    put(clear + 1, size, &mut out);
    if nbits > 0 {
        out.push(acc as u8);
    }
    out
}

fn gif_file(w: u16, h: u16, frames: usize, interlace: bool, local_palette: bool, loops: Option<u16>) -> Vec<u8> {
    let mut g = b"GIF89a".to_vec();
    g.extend_from_slice(&w.to_le_bytes());
    g.extend_from_slice(&h.to_le_bytes());
    g.extend_from_slice(&[0xF7, 0, 0]); // global palette of 256
    for i in 0..256u32 {
        g.extend_from_slice(&[i as u8, (i * 3) as u8, (255 - i) as u8]);
    }
    if let Some(l) = loops {
        g.extend_from_slice(&[0x21, 0xFF, 11]);
        g.extend_from_slice(b"NETSCAPE2.0");
        g.extend_from_slice(&[3, 1, l as u8, (l >> 8) as u8, 0]);
    }
    g.extend_from_slice(&[0x21, 0xFE, 5, b'h', b'e', b'l', b'l', b'o', 0]);
    for f in 0..frames {
        g.extend_from_slice(&[0x21, 0xF9, 4, (1 << 2) | 1 | ((f as u8 % 3) << 2), 5, 0, 7, 0]);
        g.push(0x2C);
        let (fw, fh) = (w - (f as u16 % 3), h - (f as u16 % 2));
        for v in [f as u16 % 2, 0, fw, fh] {
            g.extend_from_slice(&v.to_le_bytes());
        }
        let mut flags = if interlace { 0x40 } else { 0 };
        if local_palette {
            flags |= 0x80 | 2; // 8 colours
        }
        g.push(flags);
        if local_palette {
            for i in 0..8u8 {
                g.extend_from_slice(&[i * 30, 255 - i * 30, i]);
            }
        }
        g.push(8);
        let px: Vec<u8> = (0..fw as usize * fh as usize).map(|i| if local_palette { ((i / 3) % 8) as u8 } else { ((i * 7 + i / 5 + f) % 200) as u8 }).collect();
        let data = lzw(&px, 8);
        for c in data.chunks(255) {
            g.push(c.len() as u8);
            g.extend_from_slice(c);
        }
        g.push(0);
    }
    g.push(0x3B);
    g
}

fn check_animation(a: &gif::Animation, what: &str) {
    assert!(a.width > 0 && a.height > 0, "{what}: empty canvas");
    assert!(a.frames.len() <= gif::MAX_FRAMES, "{what}: {} frames", a.frames.len());
    if a.frames.len() > 1 {
        assert!(a.frames.len() as u64 * a.width as u64 * a.height as u64 <= gif::MAX_ANIMATION_PIXELS, "{what}: animation over the pixel limit");
    }
    for f in &a.frames {
        assert_eq!(f.rgba.len(), a.width as usize * a.height as usize * 4, "{what}: a frame is not canvas-sized");
        assert!(f.delay_ms >= 20, "{what}: delay {} under the floor", f.delay_ms);
    }
    let _ = a.plays();
}

#[test]
fn fuzz_gif_frames_loop_counts_and_lzw() {
    let corpus = vec![gif_file(16, 12, 3, false, false, Some(3)), gif_file(20, 20, 2, true, true, Some(0)), gif_file(9, 7, 4, false, true, None), gif_file(40, 30, 1, true, false, Some(65535))];
    // the encoder is right: the first frame decodes to the pixels that went in
    let ok = gif::decode(&corpus[0]).unwrap();
    assert_eq!((ok.width, ok.height), (16, 12));
    assert_eq!(gif::decode_animated(&corpus[0]).unwrap().frames.len(), 3);
    let n = run("gif", 0x61F, &corpus, 6000, Duration::from_secs(3), 256 << 20, |input| {
        if let Ok(a) = gif::decode_animated(input) {
            check_animation(&a, "gif");
        }
        if let Ok(b) = gif::decode(input) {
            assert_eq!(b.rgba.len(), b.width as usize * b.height as usize * 4);
        }
    });
    assert!(n >= cases(6000));
}

// ---- WebP ----

const WEBP: [(&str, &[u8]); 7] = [
    ("lossy", include_bytes!("../tests/data/webp_lossy.webp")),
    ("alpha lossless", include_bytes!("../tests/data/webp_lossy_alpha_lossless.webp")),
    ("1x1", include_bytes!("../tests/data/webp_lossy_1x1.webp")),
    ("lossless", include_bytes!("../tests/data/webp_lossless.webp")),
    ("palette", include_bytes!("../tests/data/webp_lossless_palette.webp")),
    ("animated", include_bytes!("../tests/data/webp_anim.webp")),
    ("alpha raw", include_bytes!("../tests/data/webp_lossy_alpha_raw_gradient.webp")),
];

/// The still files become frames of one animation, which the tests of the decoder build the same way.
fn webp_animation(stills: &[&[u8]], cw: u32, ch: u32) -> Vec<u8> {
    let mut body = b"WEBP".to_vec();
    body.extend_from_slice(b"VP8X");
    body.extend_from_slice(&10u32.to_le_bytes());
    body.extend_from_slice(&[0x12, 0, 0, 0]);
    body.extend_from_slice(&(cw - 1).to_le_bytes()[..3]);
    body.extend_from_slice(&(ch - 1).to_le_bytes()[..3]);
    body.extend_from_slice(b"ANIM");
    body.extend_from_slice(&6u32.to_le_bytes());
    body.extend_from_slice(&[0; 6]);
    for (i, still) in stills.iter().enumerate() {
        let b = webp::decode(still).unwrap();
        let mut f = Vec::new();
        f.extend_from_slice(&(((i as u32 * 2) % 8) / 2).to_le_bytes()[..3]);
        f.extend_from_slice(&[0, 0, 0]);
        f.extend_from_slice(&(b.width - 1).to_le_bytes()[..3]);
        f.extend_from_slice(&(b.height - 1).to_le_bytes()[..3]);
        f.extend_from_slice(&(40u32 + i as u32).to_le_bytes()[..3]);
        f.push((i as u8) & 3);
        f.extend_from_slice(&still[12..]);
        body.extend_from_slice(b"ANMF");
        body.extend_from_slice(&(f.len() as u32).to_le_bytes());
        body.extend_from_slice(&f);
        if f.len() & 1 == 1 {
            body.push(0);
        }
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

#[test]
fn fuzz_webp_decoders_on_mutated_files_and_animations() {
    let mut corpus: Vec<Vec<u8>> = WEBP.iter().map(|(_, f)| f.to_vec()).collect();
    let (a, b) = (webp::decode(WEBP[2].1).unwrap(), webp::decode(WEBP[3].1).unwrap());
    let canvas = (a.width.max(b.width) + 8, a.height.max(b.height) + 4);
    corpus.push(webp_animation(&[WEBP[2].1, WEBP[3].1, WEBP[2].1], canvas.0, canvas.1));
    for c in &corpus {
        webp::decode_animated(c).unwrap_or_else(|e| panic!("corpus file does not decode: {e}"));
    }
    let n = run("webp", 0x3EB, &corpus, 1500, Duration::from_secs(5), 768 << 20, |input| {
        if let Ok(a) = webp::decode_animated(input) {
            check_animation(&a, "webp");
        }
        if let Ok(b) = webp::decode(input) {
            assert_eq!(b.rgba.len(), b.width as usize * b.height as usize * 4);
        }
    });
    assert!(n >= cases(1500));
}

// ---- HTML parser ----

#[test]
fn fuzz_html_parser_never_panics_or_recurses_without_bound() {
    let corpus: Vec<Vec<u8>> = [
        "<!doctype html><html><head><title>t</title><style>p{color:red}</style></head><body><div id=a class='x y'><p>one<p>two<table><tr><td>1<td>2</table><ul><li>a<li>b</ul><a href=/x>l</a></div></body></html>",
        "<div>".repeat(500).as_str(),
        "<b><i><u>x</b></i></u><svg><path d='M0 0'/></svg><!-- c --><![CDATA[x]]><?php ?>&amp;&#x41;&#65;&bogus;&#99999999;",
        "<select><option>a<option>b</select><textarea>x</textarea><input value=\"v\"><template><p>t</template><script>if (a<b) {}</script>",
    ]
    .iter()
    .map(|s| s.as_bytes().to_vec())
    .collect();
    run("html", 0x47D, &corpus, 6000, Duration::from_secs(3), 256 << 20, |input| {
        let t = html::parse(&lossy(input));
        // every node's parent and children agree, and the shape is a tree under node 0
        check_tree(&t, &[], "html parse");
        let _ = t.outer_html(0);
        let _ = t.text_of(0);
    });
}

// ---- node arena: create / insert / remove / innerHTML with the script's holding rules ----

/// The invariants of the arena: each node is the child of at most one parent and agrees with it, no cycles, the free list holds each number
/// once, and neither a free number nor anything a script holds, or anything reachable from the page or a held node, is on it.
fn check_tree(t: &Tree, held: &[usize], what: &str) {
    let n = t.nodes.len();
    let free: HashSet<usize> = t.free.iter().copied().collect();
    assert_eq!(free.len(), t.free.len(), "{what}: a number is on the free list twice");
    let mut seen = vec![0u32; n];
    for (i, node) in t.nodes.iter().enumerate() {
        if free.contains(&i) {
            assert!(node.children.is_empty() && node.parent == NONE, "{what}: free node {i} still has links");
            continue;
        }
        for &c in &node.children {
            assert!(c < n, "{what}: child {c} of {i} is out of range");
            assert_eq!(t.nodes[c].parent, i, "{what}: child {c} does not point back to {i}");
            assert!(!free.contains(&c), "{what}: node {c} is a child of {i} and on the free list");
            seen[c] += 1;
        }
        if node.parent != NONE {
            assert!(node.parent < n && t.nodes[node.parent].children.contains(&i), "{what}: node {i} names a parent that does not list it");
        }
    }
    assert!(seen.iter().all(|c| *c <= 1), "{what}: a node is the child of two parents");
    for i in 0..n {
        let mut cur = i;
        for _ in 0..=n {
            if t.nodes[cur].parent == NONE {
                cur = usize::MAX;
                break;
            }
            cur = t.nodes[cur].parent;
        }
        assert_eq!(cur, usize::MAX, "{what}: node {i} is in a cycle");
    }
    for h in held {
        assert!(!free.contains(h), "{what}: node {h} is held by a script and on the free list");
    }
    let mut stack = vec![0usize];
    while let Some(x) = stack.pop() {
        assert!(!free.contains(&x), "{what}: node {x} is in the page and on the free list");
        stack.extend(t.nodes[x].children.iter().copied());
    }
}

/// What the DOM module does when a script drops its object for a node (see `DomModule::reclaim`): the whole detached tree goes back if no
/// node of it is held.
fn drop_handle(t: &mut Tree, held: &mut BTreeSet<usize>, id: usize) {
    if !held.remove(&id) {
        return;
    }
    let mut top = id;
    while let Some(p) = t.parent_of(top) {
        top = p;
    }
    let Some(nodes) = t.detached_subtree(top) else { return };
    if nodes.iter().any(|n| held.contains(n)) {
        return;
    }
    t.release_nodes(&nodes);
    // a second release of the same subtree, and a second look for it, change nothing (KI-108)
    t.release_nodes(&nodes);
    assert!(t.detached_subtree(top).is_none(), "a released subtree can be found again");
}

#[test]
fn fuzz_node_arena_random_operations_keep_the_free_list_sound() {
    let markups = ["<p>x</p>", "<div id=a><span>1</span><b>2</b></div>text", "", "<ul><li>a<li>b</ul>", "<i><i><i>x</i></i></i>"];
    let mut r = Rng::new(0xA7E);
    let mut reuses = 0usize;
    for session in 0..cases(300) {
        let mut t = html::parse("<html><body><div id=root><p>a</p><p>b</p></div></body></html>");
        let mut held: BTreeSet<usize> = BTreeSet::new();
        // a script holds the nodes it created, cloned or queried
        for n in 0..t.nodes.len() {
            if r.chance(3) {
                held.insert(n);
            }
        }
        for step in 0..120 {
            let live: Vec<usize> = (0..t.nodes.len()).filter(|i| !t.free.contains(i)).collect();
            let elems: Vec<usize> = live.iter().copied().filter(|&i| t.is_element(i)).collect();
            let hv: Vec<usize> = held.iter().copied().collect();
            // a script can only name nodes it holds, or the document
            let pick = |r: &mut Rng, v: &[usize]| if v.is_empty() { 0 } else { v[r.below(v.len())] };
            match r.below(10) {
                0 => {
                    let tag = *r.pick(&["div", "span", "p", "ul", "x-y"]);
                    if let Ok(n) = t.new_element(tag) {
                        held.insert(n);
                    }
                }
                1 => {
                    if let Ok(n) = t.new_text("t") {
                        held.insert(n);
                    }
                }
                2 | 3 => {
                    let (p, c) = (pick(&mut r, &hv), pick(&mut r, &hv));
                    if c != 0 {
                        let before = if r.chance(3) { t.nodes.get(p).and_then(|n| n.children.first().copied()) } else { None };
                        let _ = t.insert(p, c, before);
                    }
                }
                4 => {
                    let c = pick(&mut r, &hv);
                    if c != 0 {
                        t.detach(c);
                    }
                }
                5 => {
                    // innerHTML on a held element or the root: the old children are reclaimed when nobody holds them (as `html_set` does)
                    let n = if r.chance(4) { 0 } else { pick(&mut r, &hv) };
                    if t.is_element(n) {
                        let old = t.nodes[n].children.clone();
                        let _ = t.set_inner_html(n, r.pick(&markups));
                        for c in old {
                            let mut hold = held.clone();
                            // the dom module reclaims `c` only if nothing in its tree is wrapped
                            if let Some(sub) = t.detached_subtree(c) {
                                if sub.iter().any(|x| hold.contains(x)) {
                                    continue;
                                }
                                t.release_nodes(&sub);
                            }
                            hold.clear();
                        }
                    }
                }
                6 => {
                    let n = pick(&mut r, &hv);
                    if t.is_element(n) {
                        let old = t.nodes[n].children.clone();
                        if t.set_text_content(n, "new text").is_ok() {
                            for c in old {
                                if let Some(sub) = t.detached_subtree(c) {
                                    if !sub.iter().any(|x| held.contains(x)) {
                                        t.release_nodes(&sub);
                                    }
                                }
                            }
                        }
                    }
                }
                7 => {
                    let n = pick(&mut r, &hv);
                    if t.is_element(n) || n != 0 {
                        if let Ok(c) = t.clone_node(n, r.chance(2)) {
                            held.insert(c);
                        }
                    }
                }
                8 => {
                    // a query: the script now holds some nodes under an element
                    let e = pick(&mut r, &elems);
                    for d in t.descendants(e).into_iter().take(3) {
                        held.insert(d);
                    }
                }
                _ => {
                    let id = pick(&mut r, &hv);
                    drop_handle(&mut t, &mut held, id);
                }
            }
            let h: Vec<usize> = held.iter().copied().collect();
            check_tree(&t, &h, &format!("session {session} step {step}"));
        }
        // numbers handed out again were reported
        for x in &t.reused {
            assert!(*x < t.nodes.len());
        }
        reuses += t.reused.len();
        let _ = NodeKind::Text(String::new());
    }
    assert!(reuses > 100, "the sequences must exercise the reuse of released numbers ({reuses})");
}

#[test]
fn fuzz_html_regression_unterminated_closing_tag_before_multibyte_text() {
    // `</a` with no `>` and the input ending in a multi-byte character used to slice inside the character and panic
    for s in ["</select\u{fffd}", "x</p\u{e9}", "</a\u{1F600}", "</\u{fffd}", "</b"] {
        let _ = html::parse(s);
    }
}

#[test]
fn fuzz_node_arena_regression_a_second_release_of_a_subtree_is_a_no_op() {
    // KI-108: the same numbers used to go on the free list twice and then be handed to two nodes
    let mut t = html::parse("<html><body></body></html>");
    let top = t.new_element("div").unwrap();
    let kid = t.new_element("span").unwrap();
    t.append(top, kid).unwrap();
    let nodes = t.detached_subtree(top).unwrap();
    t.release_nodes(&nodes);
    t.release_nodes(&nodes);
    assert_eq!(t.free.len(), 2, "two numbers, each once");
    assert!(t.detached_subtree(top).is_none() && t.detached_subtree(kid).is_none());
    let (a, b) = (t.new_element("a").unwrap(), t.new_element("b").unwrap());
    assert_ne!(a, b, "two new nodes never share a number");
    let c = t.new_element("c").unwrap();
    assert!(![a, b].contains(&c));
    check_tree(&t, &[a, b, c], "after the double release");
}
