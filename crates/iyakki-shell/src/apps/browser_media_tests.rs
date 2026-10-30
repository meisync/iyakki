// Camera, microphone and screen questions in the browser (browser_media.rs) over the fake helper's `fake://media/KINDS/HOST` pages
// (KINDS: 1 microphone, 2 camera, 4 screen): one question per kind, each answer, what is remembered, what is refused without a
// question, the screen's stricter rule, the indicator, the shell's sign and its stop, and the screen feed. Included (include!) into
// the browser's test module. The tests that start the fake helper hold the process-wide helper lock, so the shared capture sign and
// feed path are not touched by another test while one of these runs.

fn answers_logged(a: &BrowserApp, allow: bool) -> usize {
    let want = format!("PermissionAnswer {{ id: 1, allow: {allow} }}");
    a.full.as_ref().unwrap().log.iter().filter(|l| l.contains(&want)).count()
}

fn ask_for(a: &mut BrowserApp, cx: &AppCx, kinds: u8) {
    a.full.as_mut().unwrap().navigate(&format!("fake://media/{kinds}/meet.example"));
    pump(a, cx, "the question", |a| a.media_ask.is_some());
}

fn asking(a: &BrowserApp) -> String {
    a.media_ask.as_ref().map(|k| k.question()).unwrap_or_default()
}

#[test]
fn a_camera_and_microphone_request_is_asked_one_kind_at_a_time_and_once_forgets() {
    let (_fake, cx, mut a, _g) = in_full("media_once", "--static");
    ask_for(&mut a, &cx, 3);
    assert_eq!(asking(&a), "Allow meet.example to use your microphone?");
    let g = frame(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(browser_media::TAB_ONCE)) && g.hits.iter().any(|h| h.ev == Event::Tab(browser_media::TAB_BLOCK)));
    key(&mut a, &cx, Key::Char('1'));
    assert_eq!(asking(&a), "Allow meet.example to use your camera?", "the second kind is asked next");
    assert_eq!(answers_logged(&a, true) + answers_logged(&a, false), 0, "the page hears nothing until every kind is answered");
    key(&mut a, &cx, Key::Char('1'));
    assert!(a.media_ask.is_none());
    pump(&mut a, &cx, "the yes at the helper", |a| answers_logged(a, true) == 1);
    assert!(browser_media::load(&cx).list().is_empty(), "once is not remembered");
    pump(&mut a, &cx, "the capture state", |a| a.full.as_ref().is_some_and(|f| f.capture == 3));
    // a no to the first kind ends the request: the camera is not asked about
    ask_for(&mut a, &cx, 3);
    key(&mut a, &cx, Key::Esc);
    pump(&mut a, &cx, "the no at the helper", |a| answers_logged(a, false) == 1);
    assert!(a.media_ask.is_none());
}

#[test]
fn always_and_block_are_remembered_per_person_and_per_kind() {
    let (_fake, cx, mut a, _g) = in_full("media_always", "--static");
    ask_for(&mut a, &cx, 2);
    a.handle(&Event::Tab(browser_media::TAB_ALWAYS), &cx);
    pump(&mut a, &cx, "the yes at the helper", |a| answers_logged(a, true) == 1);
    assert_eq!(browser_media::load(&cx).list(), vec![("meet.example".to_string(), "camera allowed".to_string())]);
    a.full.as_mut().unwrap().navigate("fake://media/2/meet.example");
    pump(&mut a, &cx, "the second yes", |a| answers_logged(a, true) == 2);
    assert!(a.media_ask.is_none(), "no question the second time");
    // the microphone was never allowed: asked, and blocked
    ask_for(&mut a, &cx, 1);
    a.handle(&Event::Tab(browser_media::TAB_BLOCK), &cx);
    pump(&mut a, &cx, "the no", |a| answers_logged(a, false) == 1);
    assert_eq!(browser_media::load(&cx).list(), vec![("meet.example".to_string(), "camera allowed, microphone blocked".to_string())]);
    a.full.as_mut().unwrap().navigate("fake://media/1/meet.example");
    pump(&mut a, &cx, "the refusal", |a| answers_logged(a, false) == 2);
    assert!(a.media_ask.is_none(), "a blocked device is not asked about again");
}

#[test]
fn a_screen_share_is_asked_every_time_with_only_this_time_and_block() {
    let (_fake, cx, mut a, _g) = in_full("media_screen", "--static");
    ask_for(&mut a, &cx, 4);
    assert_eq!(asking(&a), "Allow meet.example to share your screen?");
    let g = frame(&mut a, &cx, 1280, 720);
    assert!(!g.hits.iter().any(|h| h.ev == Event::Tab(browser_media::TAB_ALWAYS) || h.ev == Event::Tab(browser_media::TAB_WHILE_OPEN)), "no 'always' and no 'while open' for the screen");
    key(&mut a, &cx, Key::Char('3'));
    key(&mut a, &cx, Key::Char('2'));
    assert!(a.media_ask.is_some(), "the keys of the other answers do nothing for a screen");
    key(&mut a, &cx, Key::Char('1'));
    pump(&mut a, &cx, "the yes and the share", |a| answers_logged(a, true) == 1 && a.full.as_ref().is_some_and(|f| f.capture == 4));
    let fl = a.full.as_ref().unwrap().log.iter().any(|l| l.contains("ScreenSource(\"/") && l.contains("iyakki-screen-"));
    assert!(fl, "the engine side is told where the shell writes its screen: {:?}", a.full.as_ref().unwrap().log);
    // the shell's backend writes its screen into the feed while the share is on, and closes it when it ends
    let feed = browser_media::feed_path().expect("a feed path while sharing");
    let px = vec![200u8; 8 * 6 * 4];
    browser_media::feed_screen(8, 6, &px);
    assert_eq!(browser_media::read_feed_header(&feed).map(|h| (h.0, h.1)), Some((8, 6)));
    assert_eq!(browser_media::capture_now(), 4, "the shell's sign knows");
    // nothing was remembered: asked again
    ask_for(&mut a, &cx, 4);
    assert!(browser_media::load(&cx).list().is_empty());
    key(&mut a, &cx, Key::Char('4'));
    pump(&mut a, &cx, "the no", |a| answers_logged(a, false) == 1);
    assert_eq!(browser_media::load(&cx).list(), vec![("meet.example".to_string(), "screen blocked".to_string())]);
    a.full.as_mut().unwrap().navigate("fake://media/4/meet.example");
    pump(&mut a, &cx, "the refusal", |a| answers_logged(a, false) == 2);
    assert!(a.media_ask.is_none());
    // the share ended with the navigation: feed closed, sign gone
    pump(&mut a, &cx, "the end of the share", |a| a.full.as_ref().is_some_and(|f| f.capture == 0) && browser_media::capture_now() == 0);
    assert!(browser_media::feed_path().is_none());
    browser_media::feed_screen(8, 6, &px);
    assert!(!feed.exists(), "the feed file is removed when the share ends");
}

#[test]
fn camera_microphone_and_screen_in_one_request_are_three_questions() {
    let (_fake, cx, mut a, _g) = in_full("media_three", "--static");
    ask_for(&mut a, &cx, 7);
    let mut seen = vec![asking(&a)];
    for _ in 0..2 {
        key(&mut a, &cx, Key::Char('1'));
        seen.push(asking(&a));
    }
    assert_eq!(seen, vec!["Allow meet.example to use your microphone?", "Allow meet.example to use your camera?", "Allow meet.example to share your screen?"]);
    key(&mut a, &cx, Key::Char('1'));
    pump(&mut a, &cx, "all three captured", |a| answers_logged(a, true) == 1 && a.full.as_ref().is_some_and(|f| f.capture == 7));
}

#[test]
fn the_shell_sign_stops_what_the_page_captures() {
    let (_fake, cx, mut a, _g) = in_full("media_stop", "--static");
    ask_for(&mut a, &cx, 3);
    key(&mut a, &cx, Key::Char('2')); // microphone while the site is open
    key(&mut a, &cx, Key::Char('1')); // camera once
    pump(&mut a, &cx, "capturing", |a| a.full.as_ref().is_some_and(|f| f.capture == 3) && browser_media::capture_now() == 3);
    // the sign is drawn on every screen with a click target
    let mut g = Gfx::new(1280, 720);
    let r = crate::shell::draw_capture_sign(&mut g, 720, 1.0, 3, false);
    assert!(r.x >= 0.0 && r.y + r.h <= 720.0 && r.x + r.w <= 1280.0, "inside the screen");
    assert!(shown(&g).contains("A page is using your camera and microphone. Click to stop."), "{}", shown(&g));
    // the click: the browser ends the capture and the "while this site is open" allowance
    browser_media::ask_stop();
    pump(&mut a, &cx, "stopped", |a| a.full.as_ref().is_some_and(|f| f.capture == 0) && browser_media::capture_now() == 0);
    a.full.as_mut().unwrap().navigate("fake://media/1/meet.example");
    pump(&mut a, &cx, "asked again", |a| a.media_ask.is_some());
}

#[test]
fn a_question_is_answered_no_when_the_person_opens_another_tab() {
    let (_fake, cx, mut a, _g) = in_full("media_refuse", "--static");
    ask_for(&mut a, &cx, 1);
    a.new_tab();
    assert!(a.media_ask.is_none());
}

#[test]
fn the_bar_says_when_a_page_is_using_the_camera() {
    let (_fake, cx, mut a, _g) = in_full("media_indicator", "--static");
    let g = frame(&mut a, &cx, 1280, 720);
    assert!(!shown(&g).contains("Camera on"));
    a.full.as_mut().unwrap().navigate("fake://capture/2");
    pump(&mut a, &cx, "the capture state", |a| a.full.as_ref().is_some_and(|f| f.capture == 2));
    let g = frame(&mut a, &cx, 1280, 720);
    assert!(shown(&g).contains("Camera on"), "{}", shown(&g));
    a.full.as_mut().unwrap().navigate("fake://capture/0");
    pump(&mut a, &cx, "the end of capture", |a| a.full.as_ref().is_some_and(|f| f.capture == 0));
}

/// A /proc/asound with a laptop's analog input and a webcam's USB microphone (card 1), plus HDMI and a loopback that are no microphones.
fn fake_proc(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("iyakki-mic-proc-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let put = |rel: &str, text: &str| {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    put("asound/cards", " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0xf7e10000 irq 33\n 1 [Device         ]: USB-Audio - USB PnP Sound Device\n");
    put("asound/pcm", "00-00: ALC3246 Analog : ALC3246 Analog : playback 1 : capture 1\n00-03: HDMI 0 : HDMI 0 : playback 1\n00-05: Bridge Loopback : Bridge Loopback : playback 1 : capture 1\n01-00: USB Audio : USB Audio : playback 1 : capture 1\n");
    root
}

#[test]
fn the_microphone_question_has_a_chooser_whose_choice_the_voice_tile_shares() {
    let (_fake, cx, mut a, _g) = in_full("media_micpick", "--static");
    let proc = fake_proc("pick");
    a.mic_proc = proc.clone();
    a.mic_dir = proc.join("shared");
    std::fs::create_dir_all(&a.mic_dir).unwrap();
    ask_for(&mut a, &cx, 1);
    let g = frame(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(browser_media::TAB_MIC)), "the chooser is on the microphone question");
    assert!(shown(&g).contains("Automatic: ALC3246 Analog"), "automatic is the first microphone, never the loopback: {}", shown(&g));
    // keyboard: M opens the list, type to filter, Enter chooses; the choice is the voice setting and the Voice tile reads the same one
    key(&mut a, &cx, Key::Char('m'));
    assert!(a.media_mic_pick.is_some());
    let g = frame(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label.contains("USB Audio (USB)")) && !g.hits.iter().any(|h| h.label.contains("Loopback")));
    for c in "usb".chars() {
        key(&mut a, &cx, Key::Char(c));
    }
    key(&mut a, &cx, Key::Enter);
    assert!(a.media_mic_pick.is_none() && a.media_ask.is_some(), "the question stays open after choosing");
    assert_eq!(cx.services.voice_settings().mic, "plughw:1,0");
    let g = frame(&mut a, &cx, 1280, 720);
    assert!(shown(&g).contains("USB Audio (USB)"), "{}", shown(&g));
    // the mouse does the same: the select box, then Automatic
    a.handle(&Event::Tab(browser_media::TAB_MIC), &cx);
    assert!(a.media_mic_pick.is_some());
    key(&mut a, &cx, Key::Esc);
    assert!(a.media_mic_pick.is_none() && cx.services.voice_settings().mic == "plughw:1,0", "Esc changes nothing");
    a.handle(&Event::Tab(browser_media::TAB_MIC), &cx);
    for c in "automatic".chars() {
        key(&mut a, &cx, Key::Char(c));
    }
    key(&mut a, &cx, Key::Enter);
    assert_eq!(cx.services.voice_settings().mic, "", "automatic is saved as empty");
    // a camera question has no microphone row
    key(&mut a, &cx, Key::Esc);
    ask_for(&mut a, &cx, 2);
    assert!(!frame(&mut a, &cx, 1280, 720).hits.iter().any(|h| h.ev == Event::Tab(browser_media::TAB_MIC)));
    let _ = std::fs::remove_dir_all(&proc);
}

#[test]
fn the_microphone_question_fits_at_four_screen_sizes_without_overlap() {
    let (_fake, cx, mut a, _g) = in_full("media_micfit", "--static");
    let proc = fake_proc("fit");
    a.mic_proc = proc.clone();
    ask_for(&mut a, &cx, 1);
    for (w, h) in [(2560u32, 1600u32), (1920, 1080), (1280, 720), (800, 480)] {
        let g = frame(&mut a, &cx, w, h);
        let screen = R::new(0.0, 0.0, w as f32, h as f32);
        let ids = [browser_media::TAB_MIC, browser_media::TAB_ONCE, browser_media::TAB_WHILE_OPEN, browser_media::TAB_ALWAYS, browser_media::TAB_BLOCK, browser_media::TAB_NOT_NOW];
        let rects: Vec<R> = ids.iter().filter_map(|id| g.hits.iter().find(|x| x.ev == Event::Tab(*id)).map(|x| x.rect)).collect();
        assert_eq!(rects.len(), ids.len(), "{w}x{h}: every control is there");
        for (i, r) in rects.iter().enumerate() {
            assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= screen.w + 0.5 && r.y + r.h <= screen.h + 0.5, "{w}x{h}: control {i} on screen: {r:?}");
            for (j, o) in rects.iter().enumerate().skip(i + 1) {
                assert!(!(r.x < o.x + o.w - 0.5 && o.x < r.x + r.w - 0.5 && r.y < o.y + o.h - 0.5 && o.y < r.y + r.h - 0.5), "{w}x{h}: controls {i} and {j} overlap: {r:?} {o:?}");
            }
        }
    }
    let _ = std::fs::remove_dir_all(&proc);
}

#[test]
fn a_microphone_that_could_not_be_opened_is_named_with_the_reason_in_plain_words() {
    let (_fake, cx, mut a, _g) = in_full("media_micfail", "--static");
    let dir = std::env::temp_dir().join(format!("iyakki-mic-fail-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    a.mic_dir = dir.clone();
    std::fs::write(dir.join(iyakki_agent::voicedesk::BROWSER_MIC_STATUS), "Built-in microphone (Apple T2 Audio)\tCould not open audio device for recording: Device or resource busy\n").unwrap();
    a.mic_checked = None;
    a.mic_failure_tick();
    let (text, _) = a.status.clone().expect("the person is told");
    assert!(text.contains("Built-in microphone (Apple T2 Audio)") && text.contains("another program is using it") && text.contains("Choose another microphone"), "{text}");
    // said once; when the plugin has delivered sound again the file is gone and the next failure is told again
    a.status = None;
    a.mic_checked = None;
    a.mic_failure_tick();
    assert!(a.status.is_none());
    std::fs::remove_file(dir.join(iyakki_agent::voicedesk::BROWSER_MIC_STATUS)).unwrap();
    a.mic_checked = None;
    a.mic_failure_tick();
    std::fs::write(dir.join(iyakki_agent::voicedesk::BROWSER_MIC_STATUS), "USB Audio (USB)\tNo such device\n").unwrap();
    a.mic_checked = None;
    a.mic_failure_tick();
    assert!(a.status.as_ref().is_some_and(|s| s.0.contains("USB Audio (USB)") && s.0.contains("does not offer it")), "{:?}", a.status);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = cx;
}

/// Two cameras in a fake /sys (the built-in one and a USB one, each with its metadata node).
fn fake_cams(proc: &std::path::Path) -> std::path::PathBuf {
    let sys = proc.join("sys");
    for (n, name, idx) in [(0, "FaceTime HD Camera (Built-in): ", "0"), (1, "FaceTime HD Camera (Built-in): ", "1"), (2, "USB Cam: usb-0000", "0"), (3, "USB Cam: usb-0000", "1")] {
        let d = sys.join(format!("class/video4linux/video{n}"));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("name"), format!("{name}\n")).unwrap();
        std::fs::write(d.join("index"), format!("{idx}\n")).unwrap();
    }
    sys
}

#[test]
fn the_camera_question_has_a_chooser_and_a_hidden_camera_leaves_the_list_and_the_plugin_files() {
    let (_fake, cx, mut a, _g) = in_full("media_campick", "--static");
    let proc = fake_proc("campick");
    a.mic_proc = proc.clone();
    a.cam_sys = fake_cams(&proc);
    a.mic_dir = proc.join("shared");
    std::fs::create_dir_all(&a.mic_dir).unwrap();
    let dir = a.mic_dir.clone();
    let shared = move |f: &str| std::fs::read_to_string(dir.join(f)).unwrap_or_default();
    ask_for(&mut a, &cx, 2);
    let g = frame(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(browser_media::TAB_CAM)), "the chooser is on the camera question");
    assert!(shown(&g).contains("Automatic: FaceTime HD Camera (Built-in)"), "automatic is the first camera: {}", shown(&g));
    // C opens the list: the two cameras once each, plus Automatic; typing filters; Enter makes it the default
    key(&mut a, &cx, Key::Char('c'));
    assert!(a.media_cam_pick.is_some());
    let g = frame(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label.contains("USB Cam")) && g.hits.iter().filter(|h| h.label.contains("FaceTime")).count() == 1);
    for c in "usb".chars() {
        key(&mut a, &cx, Key::Char(c));
    }
    key(&mut a, &cx, Key::Enter);
    assert!(a.media_cam_pick.is_none() && a.media_ask.is_some(), "the question stays open after choosing");
    assert_eq!(shared(iyakki_agent::browserdev::CAM_CHOICE), "USB Cam: usb-0000\n", "the plugin is told");
    assert!(shown(&frame(&mut a, &cx, 1280, 720)).contains("USB Cam"));
    // Delete marks the highlighted camera "don't use": it leaves the list, the plugin gets the hidden file, the question stays open
    key(&mut a, &cx, Key::Char('c'));
    for c in "face".chars() {
        key(&mut a, &cx, Key::Char(c));
    }
    key(&mut a, &cx, Key::Delete);
    assert!(a.media_cam_pick.is_some(), "the list stays open");
    assert_eq!(shared(iyakki_agent::browserdev::CAM_HIDDEN), "FaceTime HD Camera (Built-in):\n");
    assert!(!frame(&mut a, &cx, 1280, 720).hits.iter().any(|h| h.label.contains("FaceTime")), "a hidden camera is not listed again");
    key(&mut a, &cx, Key::Esc);
    // Delete on Automatic hides nothing
    key(&mut a, &cx, Key::Char('c'));
    for c in "automatic".chars() {
        key(&mut a, &cx, Key::Char(c));
    }
    key(&mut a, &cx, Key::Delete);
    assert_eq!(shared(iyakki_agent::browserdev::CAM_HIDDEN), "FaceTime HD Camera (Built-in):\n");
    key(&mut a, &cx, Key::Esc);
    // allowing once tells the plugins the hidden list and the choice before the engine is answered
    key(&mut a, &cx, Key::Char('1'));
    pump(&mut a, &cx, "the answer", |a| answers_logged(a, true) == 1);
    assert_eq!(shared(iyakki_agent::browserdev::CAM_CHOICE), "USB Cam: usb-0000\n");
    let _ = std::fs::remove_dir_all(&proc);
}

#[test]
fn a_defective_microphone_can_be_hidden_and_always_allow_remembers_the_device_for_the_site() {
    let (_fake, cx, mut a, _g) = in_full("media_micsite", "--static");
    let proc = fake_proc("micsite");
    a.mic_proc = proc.clone();
    a.cam_sys = fake_cams(&proc);
    a.mic_dir = proc.join("shared");
    std::fs::create_dir_all(&a.mic_dir).unwrap();
    let dir = a.mic_dir.clone();
    let shared = move |f: &str| std::fs::read_to_string(dir.join(f)).unwrap_or_default();
    ask_for(&mut a, &cx, 1);
    // the analog input is defective: hide it; automatic becomes the USB microphone and the plugin gets the hidden id
    key(&mut a, &cx, Key::Char('m'));
    for c in "alc".chars() {
        key(&mut a, &cx, Key::Char(c));
    }
    key(&mut a, &cx, Key::Delete);
    assert_eq!(shared(iyakki_agent::browserdev::MIC_HIDDEN), "plughw:0,0\n");
    assert!(!frame(&mut a, &cx, 1280, 720).hits.iter().any(|h| h.label.contains("ALC3246")), "a hidden microphone is not listed");
    key(&mut a, &cx, Key::Esc);
    assert!(shown(&frame(&mut a, &cx, 1280, 720)).contains("Automatic: USB Audio (USB)"));
    // "Always allow this site" keeps this microphone for the site
    key(&mut a, &cx, Key::Char('3'));
    pump(&mut a, &cx, "the answer", |a| answers_logged(a, true) == 1);
    assert_eq!(a.media.as_ref().unwrap().site_device("meet.example", iyakki_webview::proto::media_kind::MICROPHONE), "plughw:1,0");
    assert_eq!(shared(iyakki_agent::voicedesk::BROWSER_MIC_CHOICE), "plughw:1,0\n", "the plugin is told before the engine is answered");
    // the remembered device survives a restart of the browser's memory
    let reloaded = browser_media::load(&cx);
    assert_eq!(reloaded.site_device("meet.example", iyakki_webview::proto::media_kind::MICROPHONE), "plughw:1,0");
    let _ = std::fs::remove_dir_all(&proc);
}

#[test]
fn the_camera_question_fits_at_four_screen_sizes_without_overlap() {
    let (_fake, cx, mut a, _g) = in_full("media_camfit", "--static");
    let proc = fake_proc("camfit");
    a.mic_proc = proc.clone();
    a.cam_sys = fake_cams(&proc);
    ask_for(&mut a, &cx, 2);
    for (w, h) in [(2560u32, 1600u32), (1920, 1080), (1280, 720), (800, 480)] {
        let g = frame(&mut a, &cx, w, h);
        let screen = R::new(0.0, 0.0, w as f32, h as f32);
        let ids = [browser_media::TAB_CAM, browser_media::TAB_ONCE, browser_media::TAB_WHILE_OPEN, browser_media::TAB_ALWAYS, browser_media::TAB_BLOCK, browser_media::TAB_NOT_NOW];
        let rects: Vec<R> = ids.iter().filter_map(|id| g.hits.iter().find(|x| x.ev == Event::Tab(*id)).map(|x| x.rect)).collect();
        assert_eq!(rects.len(), ids.len(), "{w}x{h}: every control is there");
        for (i, r) in rects.iter().enumerate() {
            assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= screen.w + 0.5 && r.y + r.h <= screen.h + 0.5, "{w}x{h}: control {i} on screen: {r:?}");
            for (j, o) in rects.iter().enumerate().skip(i + 1) {
                assert!(!(r.x < o.x + o.w - 0.5 && o.x < r.x + r.w - 0.5 && r.y < o.y + o.h - 0.5 && o.y < r.y + r.h - 0.5), "{w}x{h}: controls {i} and {j} overlap: {r:?} {o:?}");
            }
        }
    }
    let _ = std::fs::remove_dir_all(&proc);
}
