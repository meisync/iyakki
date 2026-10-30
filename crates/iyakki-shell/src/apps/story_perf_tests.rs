// TALES frame cost: a big picture, a long story, the reader playing (see docs/project/BACKLOG.md, performance).

fn big_picture(w: u32, h: u32) -> Vec<u8> {
    let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
    for (i, p) in pm.data_mut().chunks_exact_mut(4).enumerate() {
        let (x, y) = ((i as u32 % w) as u8, (i as u32 / w) as u8);
        p.copy_from_slice(&[x, y, x ^ y, 255]);
    }
    pm.encode_png().unwrap()
}

fn long_text_app(tag: &str) -> (StoryApp, AppCx, Arc<Mutex<Option<(usize, usize)>>>) {
    let (mut a, cx, at, _t) = reading_app(tag, 2, Some(2));
    // the drawings the story asked for are made on other threads: wait for them (a picture being made is animated)
    until(&mut a, &cx, "the pictures", |a| a.pics.iter().all(|p| !matches!(p.state, PicState::Working | PicState::Queued)));
    let para = "The little fox walked along the winding river path while the evening sky turned from gold to violet, and every rustle in the reeds made her ears twitch with curiosity about the night. ";
    for i in 0..90 {
        a.story.blocks.push(Block::Para(para.repeat(1 + i % 2).trim().to_string()));
    }
    let bytes = big_picture(1344, 768);
    let sc = st::Scene { place_before: 3, title: "The river".into(), text: "a river".into() };
    a.pics.push(Pic { scene: sc, state: PicState::Ready, bytes: Some(Arc::new(bytes)), ext: "png".into(), prompt: String::new(), mode: a.story_opts.pictures, style: a.story_opts.style, key: 77, call: None, how: String::new(), log: String::new(), feedback: String::new(), size: String::new(), look: illus::Look::default(), seed: None, provider: String::new(), model: String::new(), ms: 0, alts: Vec::new() });
    (a, cx, at)
}

/// One whole frame of the story screen into `g` (cleared first), the way the shell draws it.
fn whole_frame(a: &mut StoryApp, cx: &AppCx, g: &mut Gfx, w: u32, h: u32, tick: u64) {
    let s = crate::shell::scale_for(w, h);
    g.clear(rgb(9, 11, 20));
    let top = 46.0 * s;
    a.render(g, R::new(0.0, top, w as f32, h as f32 - top), s, cx, tick);
}

/// Draw whole frames until the page has stopped following the word and the lit word has arrived.
fn settled(a: &mut StoryApp, cx: &AppCx, g: &mut Gfx, w: u32, h: u32, tick: u64) {
    for _ in 0..80 {
        whole_frame(a, cx, g, w, h, tick);
    }
}

fn move_word_to(a: &mut StoryApp, cx: &AppCx, at: &Arc<Mutex<Option<(usize, usize)>>>, k: usize) {
    let n = iyakki_agent::wordtime::words_of(&a.chunks[2].text).len();
    *at.lock().unwrap() = Some((k, n));
    let (c0, sp) = (a.chunks[2].start, iyakki_agent::wordtime::words_of(&a.chunks[2].text)[k]);
    until(a, cx, "the word", |a| a.reader.snapshot().word == Some((c0 + sp.start, c0 + sp.end)));
}

fn area_of(rects: &[(i32, i32, i32, i32)]) -> i64 {
    rects.iter().map(|r| r.2 as i64 * r.3 as i64).sum()
}

fn least(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[0]
}

#[test]
fn a_whole_frame_of_a_long_story_with_a_big_picture_is_cheap_and_an_animation_frame_repaints_a_few_percent_of_the_screen() {
    let (w, h) = (2880u32, 1800u32);
    let (mut a, cx, at) = long_text_app("tales-perf-bench");
    assert!(a.story.words() > 3000, "{} words", a.story.words());
    let mut g = Gfx::new(w, h);
    a.ticks = 900;
    settled(&mut a, &cx, &mut g, w, h, 900);
    assert!(a.anim.is_some(), "a story being read, with nothing else going on, is repainted in parts");
    // a whole frame (what scrolling and the first frame cost)
    let full = least((0..8).map(|_| {
        let t0 = Instant::now();
        whole_frame(&mut a, &cx, &mut g, w, h, 900);
        t0.elapsed()
    }).collect());
    // the next word is said: the animation frames
    move_word_to(&mut a, &cx, &at, 3);
    a.ticks = 900;
    a.dirty = false;
    assert!(matches!(a.redraw_kind(), crate::apps::browser_full::Redraw::Live), "reading asks for a partial frame");
    let mut part = Vec::new();
    let mut first_area = 0;
    for k in 0..6 {
        let t0 = Instant::now();
        let r = a.live_draw(&mut g);
        part.push(t0.elapsed());
        assert!(r.is_some(), "partial frame {k} was painted");
        if k == 0 {
            first_area = area_of(&a.live_rects());
        }
    }
    let part = least(part);
    let ratio = first_area as f64 / (w as f64 * h as f64);
    eprintln!("TALES {w}x{h}: whole frame {full:?}, animation frame {part:?}, animation frame repaints {:.1}% of the screen", ratio * 100.0);
    assert!(ratio < 0.15, "an animation frame repaints {:.1}% of the screen", ratio * 100.0);
    assert!(part * 3 <= full, "an animation frame ({part:?}) is much cheaper than a whole one ({full:?})");
    // the machine of the owner's tests is not a measuring bench: the absolute bound holds in optimised builds only
    // (20 ms alone on a quiet machine; the other tests run beside this one, so the default bound is three times that)
    if !cfg!(debug_assertions) {
        let bound = if std::env::var_os("IYAKKI_PERF_STRICT").is_some() { 20 } else { 60 };
        assert!(full < Duration::from_millis(bound), "a whole frame took {full:?} (bound {bound} ms)");
    }
}

#[test]
fn animation_frames_paint_what_a_whole_frame_would_and_cover_the_old_and_the_new_word() {
    for (w, h) in [(1280u32, 720u32), (1920, 1080)] {
        let (mut a, cx, at) = long_text_app(&format!("tales-perf-same-{w}"));
        let mut g = Gfx::new(w, h);
        a.ticks = 900;
        settled(&mut a, &cx, &mut g, w, h, 900);
        assert!(a.anim.is_some());
        let old = a.word_rect.expect("a lit word");
        move_word_to(&mut a, &cx, &at, 3);
        a.ticks = 900;
        a.dirty = false;
        assert!(matches!(a.redraw_kind(), crate::apps::browser_full::Redraw::Live), "{w}x{h}");
        let mut bands = Vec::new();
        for k in 0..24 {
            assert!(a.live_draw(&mut g).is_some(), "{w}x{h}: partial frame {k}");
            if k == 0 {
                bands = a.live_rects();
            }
        }
        let new = a.word_rect.expect("the new word is lit");
        assert!(new != old, "{w}x{h}: the lit word moved");
        // the rectangles cover the word that was lit and the one that is lit now (as far as the text area shows them)
        let text = a.text_area;
        for (what, r) in [("old", old), ("new", new)] {
            let (x0, x1) = (r.x.max(text.x), (r.x + r.w).min(text.x + text.w));
            assert!(bands.iter().any(|b| b.0 as f32 <= x0 + 1.0 && (b.0 + b.2) as f32 >= x1 - 1.0 && b.1 as f32 <= r.y + 1.0 && (b.1 + b.3) as f32 >= r.y + r.h - 1.0), "{w}x{h}: the {what} word {r:?} is covered by {bands:?}");
        }
        // the same screen as drawing everything again
        let mut whole = Gfx::new(w, h);
        whole_frame(&mut a, &cx, &mut whole, w, h, 900);
        let (p, q) = (g.rgba(), whole.rgba());
        let bad = p.iter().zip(q.iter()).enumerate().filter(|(_, (x, y))| x.abs_diff(**y) > 3).count();
        if bad > 0 {
            let mut shown = 0;
            for (i, (x, y)) in p.iter().zip(q.iter()).enumerate() {
                if x.abs_diff(*y) > 3 && shown < 12 {
                    eprintln!("DIFF byte {i} pixel ({}, {}) ch{}: partial {x} whole {y}; word {:?} old {:?} bands {:?}", (i / 4) % w as usize, (i / 4) / w as usize, i % 4, new, old, bands);
                    shown += 1;
                }
            }
        }
        assert_eq!(bad, 0, "{w}x{h}: {bad} bytes differ from a whole frame (first at pixel {:?})", p.iter().zip(q.iter()).position(|(x, y)| x.abs_diff(*y) > 3).map(|i| ((i / 4) % w as usize, (i / 4) / w as usize)));
        a.reader.stop();
    }
}

#[test]
fn a_word_on_a_new_line_scrolls_the_page_and_asks_for_a_whole_frame() {
    let (w, h) = (1280u32, 720u32);
    let (mut a, cx, at) = long_text_app("tales-perf-newline");
    // the paragraph being read is made long enough to wrap onto several lines
    a.story.blocks[1] = Block::Para("One night the moon was hiding behind the clouds and the whole forest was dark. \"Do not worry,\" said an old owl from the highest branch of the oak tree. The little fox listened carefully, took a deep breath and walked on through the tall wet grass toward the river.".into());
    a.reader.stop();
    a.chunks = st::chunks_for(&a.story.units(), 0, st::MAX_CHUNK_CHARS);
    a.chunked_units = a.story.units().len();
    a.reader.start(a.chunks.clone(), 2, false, "t", Arc::new(ToneSynth), Arc::new(WordSink { at: at.clone() }));
    a.manual_unit = None;
    until(&mut a, &cx, "playing", |a| a.reader_status() == Status::Playing);
    let mut g = Gfx::new(w, h);
    a.ticks = 900;
    settled(&mut a, &cx, &mut g, w, h, 900);
    let n = iyakki_agent::wordtime::words_of(&a.chunks[2].text).len();
    let line_of = |a: &StoryApp, k: usize| {
        let sp = iyakki_agent::wordtime::words_of(&a.chunks[2].text)[k];
        let u = a.reader.snapshot().unit;
        let first = a.chunks[2].start + sp.start;
        a.lay[u].lines.iter().rposition(|(_, st0)| *st0 <= first).unwrap_or(0)
    };
    // a word on another line than the first one
    let far = (0..n).find(|&k| line_of(&a, k) != line_of(&a, 2)).unwrap_or_else(|| panic!("the paragraph wraps: unit {} lines {:?} chunk {:?} n {n}", a.reader.snapshot().unit, a.lay[a.reader.snapshot().unit].lines, (a.chunks[2].start, a.chunks[2].text.clone())));
    move_word_to(&mut a, &cx, &at, far);
    a.ticks = 900;
    a.dirty = false;
    assert!(a.live_draw(&mut g).is_none(), "the page has to scroll to the word: a whole frame does it");
    assert!(a.anim.is_none(), "and the next partial frame waits for a whole one");
    assert!(!matches!(a.redraw_kind(), crate::apps::browser_full::Redraw::Live));
    settled(&mut a, &cx, &mut g, w, h, 900);
    assert!(a.anim.is_some(), "after the scroll stopped, parts again");
    a.reader.stop();
}

#[test]
fn partial_frames_stop_when_anything_else_on_the_screen_changes() {
    let (w, h) = (1280u32, 720u32);
    let (mut a, cx, _at) = long_text_app("tales-perf-guard");
    let mut g = Gfx::new(w, h);
    a.ticks = 900;
    settled(&mut a, &cx, &mut g, w, h, 900);
    a.dirty = false;
    assert!(matches!(a.redraw_kind(), crate::apps::browser_full::Redraw::Live));
    assert!(a.live_draw(&mut g).is_some());
    // a note appears, an error, a selection, a menu: each wants the whole frame
    a.note = Some(("Audio saved".into(), true));
    assert!(a.live_draw(&mut g).is_none(), "a new note");
    settled(&mut a, &cx, &mut g, w, h, 900);
    a.error = Some("boom".into());
    assert!(a.live_draw(&mut g).is_none(), "an error");
    a.error = None;
    settled(&mut a, &cx, &mut g, w, h, 900);
    a.dirty = true;
    assert!(matches!(a.redraw_kind(), crate::apps::browser_full::Redraw::Full), "an event marked the app dirty");
    a.reader.stop();
}

#[test]
fn the_scaled_picture_and_the_text_layout_are_kept_between_frames_and_drawn_again_when_their_input_changes() {
    let (w, h) = (1280u32, 720u32);
    let (mut a, cx, _at) = long_text_app("tales-perf-cache");
    let mut g = Gfx::new(w, h);
    whole_frame(&mut a, &cx, &mut g, w, h, 1);
    let (scaled, raster) = (a.pic_scaled.len(), a.pic_cache.rasterised);
    assert!(scaled >= 1, "a picture on screen was scaled once");
    whole_frame(&mut a, &cx, &mut g, w, h, 2);
    whole_frame(&mut a, &cx, &mut g, w, h, 3);
    assert_eq!((a.pic_scaled.len(), a.pic_cache.rasterised), (scaled, raster), "the same picture at the same size is not scaled again");
    // another size of the slot: scaled again
    let mut small = Gfx::new(1024, 600);
    whole_frame(&mut a, &cx, &mut small, 1024, 600, 4);
    assert!(a.pic_scaled.len() > scaled, "another size: another entry ({} -> {})", scaled, a.pic_scaled.len());
    // the picture changes (its key does)
    let before = a.pic_scaled.len();
    for p in a.pics.iter_mut() {
        p.key += 1000;
    }
    whole_frame(&mut a, &cx, &mut small, 1024, 600, 5);
    assert!(a.pic_scaled.len() >= before.min(10), "{} entries", a.pic_scaled.len());
    assert!(a.pic_scaled.iter().any(|e| e.0 >= 1000), "the changed picture was scaled");
    assert!(a.pic_scaled.len() <= 10, "only a few are kept");
    // the layout: one paragraph changes, the others keep theirs
    whole_frame(&mut a, &cx, &mut g, w, h, 6);
    let ptrs: Vec<*const u8> = a.lay.iter().map(|l| l.text.as_ptr()).collect();
    let shift = usize::from(!a.story.title.is_empty());
    let k = 10;
    a.story.blocks[k] = Block::Para("A brand new paragraph that is not what was here before.".into());
    whole_frame(&mut a, &cx, &mut g, w, h, 7);
    assert_eq!(a.lay[k + shift].text, "A brand new paragraph that is not what was here before.");
    for (i, l) in a.lay.iter().enumerate() {
        if i != k + shift {
            assert_eq!(l.text.as_ptr(), ptrs[i], "unit {i} kept its layout");
        }
    }
    // the width changes: all of it is wrapped again
    let lines_before: usize = a.lay.iter().map(|l| l.lines.len()).sum();
    whole_frame(&mut a, &cx, &mut small, 1024, 600, 8);
    let lines_after: usize = a.lay.iter().map(|l| l.lines.len()).sum();
    assert!(lines_after > lines_before, "a narrower column wraps into more lines ({lines_before} -> {lines_after})");
    a.reader.stop();
}

#[test]
fn a_word_change_is_noticed_between_heartbeats_and_asks_for_a_frame_at_once() {
    let (w, h) = (1280u32, 720u32);
    let (mut a, cx, at) = long_text_app("tales-perf-poll");
    let mut g = Gfx::new(w, h);
    a.ticks = 900;
    settled(&mut a, &cx, &mut g, w, h, 900);
    a.dirty = false;
    assert!(a.wants_fast_poll(), "reading with the word highlight on wants to be polled between heartbeats");
    // the heartbeat has looked at the current word: nothing changed since
    let _ = a.redraw_kind();
    let _ = a.poll_redraw_due();
    assert!(!a.poll_redraw_due(), "no change, no frame");
    // the voice moves to the next word; the tick (ticks stays the same) has not come yet
    move_word_to(&mut a, &cx, &at, 4);
    assert!(a.poll_redraw_due(), "noticed at the very poll after the change");
    assert!(matches!(a.redraw_kind(), crate::apps::browser_full::Redraw::Live), "and a tile-only frame is asked for in the same beat");
    assert!(!a.poll_redraw_due(), "once");
    assert!(a.live_draw(&mut g).is_some(), "the lit word is painted");
    a.reader.stop();
    assert!(!a.wants_fast_poll(), "no polling when nothing is read");
}

#[test]
fn an_idle_tales_screen_asks_for_far_fewer_safety_redraws_than_a_reading_one() {
    let (mut a, _cx, _at) = long_text_app("tales-perf-idle");
    assert_eq!(a.idle_redraw_ms(), 100, "reading: the usual safety net");
    a.reader.stop();
    a.level = 0.0;
    wait_stopped(&a);
    assert!(a.idle_redraw_ms() >= 250, "idle: {} ms", a.idle_redraw_ms());
}

fn wait_stopped(a: &StoryApp) {
    let t0 = Instant::now();
    while matches!(a.reader.status(), Status::Playing | Status::Buffering) && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(5));
    }
}
