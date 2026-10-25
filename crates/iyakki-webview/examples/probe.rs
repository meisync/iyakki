//! Drive a helper by hand and look at what it draws: `cargo run -p iyakki-webview --example probe -- HELPER URL [script...]`.
//! Script steps (each its own argument): `wait:SECS`, `shot:FILE.png`, `move:X,Y`, `click:X,Y`, `type:TEXT`, `key:ret|tab|esc|bs`,
//! `wheel:DX,DY` (1/120 notch units), `nav:URL`, `resize:W,H`, `eval:JS`, `focus`, `fps:SECS` (measure frames per second for that long).
//! Options before the script: `--size WxH`, `--scale PCT`, `--profile DIR`, `--ua TEXT`, `--allow HOST` (repeatable), `--env K=V` (repeatable; exactly these variables).

use std::io::Write;
use std::time::{Duration, Instant};

use iyakki_webview::keys;
use iyakki_webview::proto::{Hello, MouseKind, ToHelper};
use iyakki_webview::{Config, Event, WebView};

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, t) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *t = c;
    }
    let mut c = 0xffff_ffffu32;
    for b in data {
        c = table[((c ^ *b as u32) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

/// A PNG (stored deflate blocks, no compression) from BGRA pixels.
fn write_png(path: &str, w: u32, h: u32, bgra: &[u8]) -> std::io::Result<()> {
    let mut raw = Vec::with_capacity((w as usize * 3 + 1) * h as usize);
    for y in 0..h as usize {
        raw.push(0);
        for x in 0..w as usize {
            let o = (y * w as usize + x) * 4;
            raw.extend_from_slice(&[bgra[o + 2], bgra[o + 1], bgra[o]]);
        }
    }
    let mut z = vec![0x78, 0x01];
    let mut chunks = raw.chunks(65535).peekable();
    while let Some(c) = chunks.next() {
        z.push(if chunks.peek().is_none() { 1 } else { 0 });
        z.extend_from_slice(&(c.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        z.extend_from_slice(c);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for x in &raw {
        a = (a + *x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut chunk = |ty: &[u8; 4], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = ty.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    std::fs::File::create(path)?.write_all(&out)
}

fn pump(v: &mut WebView, d: Duration, frames: &mut u64) {
    let t = Instant::now();
    while t.elapsed() < d {
        for e in v.poll() {
            match e {
                Event::Frame => *frames += 1,
                Event::Title(s) => println!("  title: {s}"),
                Event::Url(s) => println!("  url: {s}"),
                Event::Loading { loading, progress } => println!("  loading: {loading} {progress}%"),
                Event::History { can_back, can_forward } => println!("  history: back={can_back} forward={can_forward}"),
                Event::Download { url, name } => println!("  download: {name} <- {url}"),
                Event::Ready { engine, version } => println!("  ready: {engine} {version}"),
                Event::Crashed(r) | Event::Exited(r) => println!("  GONE: {r}"),
                Event::Log(l) => println!("  log: {l}"),
                Event::Clipboard(s) => println!("  clipboard: {s:?}"),
                Event::Fullscreen(on) => println!("  fullscreen: {on}"),
                Event::AudioPlaying(on) => println!("  audio playing: {on}"),
                Event::MediaUnsupported(w) => println!("  media without a decoder: {w}"),
                Event::ScriptResult { id, ok, json } => println!("  script {id} ok={ok}: {json}"),
                Event::Cursor(_) => {}
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: probe HELPER URL [--size WxH] [--scale PCT] [--profile DIR] [steps...]");
        std::process::exit(2);
    }
    let helper = args.remove(0);
    if helper == "--bench" {
        // `probe --bench W,H`: only the pixel work benchmark, no engine
        let (w, h) = args[0].split_once(',').map_or((2560, 1400), |(a, b)| (a.parse().unwrap_or(2560), b.parse().unwrap_or(1400)));
        bench(w, h);
        return;
    }
    let url = args.remove(0);
    let mut hello = Hello { w: 1280, h: 720, max_w: 3840, max_h: 2160, user_agent: "IYAKKI/0.1".into(), ..Hello::default() };
    let mut steps = Vec::new();
    // `--env K=V` (repeatable): the helper then gets exactly these variables, as the shell gives it (webengine::helper_env)
    let mut env: Vec<(String, String)> = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--size" => {
                let s = it.next().unwrap();
                let (w, h) = s.split_once('x').unwrap();
                hello.w = w.parse().unwrap();
                hello.h = h.parse().unwrap();
            }
            "--scale" => hello.scale_pct = it.next().unwrap().parse().unwrap(),
            "--profile" => hello.profile = it.next().unwrap(),
            "--ua" => hello.user_agent = it.next().unwrap(),
            "--allow" => hello.allow_hosts.push(it.next().unwrap()),
            "--env" => {
                let kv = it.next().unwrap();
                let (k, v) = kv.split_once('=').unwrap_or((kv.as_str(), ""));
                env.push((k.to_string(), v.to_string()));
            }
            _ => steps.push(a),
        }
    }
    let mut cfg = Config::new(helper, hello);
    if !env.is_empty() {
        cfg.env = Some(env);
    }
    let mut v = WebView::start(cfg).expect("start");
    let mut frames = 0u64;
    v.send(ToHelper::Navigate(url));
    v.send(ToHelper::Focus(true));
    for s in steps {
        let (cmd, arg) = s.split_once(':').unwrap_or((s.as_str(), ""));
        println!("> {s}");
        let xy = |a: &str| {
            let (x, y) = a.split_once(',').unwrap_or(("0", "0"));
            (x.parse::<i32>().unwrap(), y.parse::<i32>().unwrap())
        };
        match cmd {
            "wait" => pump(&mut v, Duration::from_secs_f32(arg.parse().unwrap()), &mut frames),
            "fps" => {
                let before = v.frames_received();
                let t = Instant::now();
                pump(&mut v, Duration::from_secs_f32(arg.parse().unwrap()), &mut frames);
                println!("  fps: {:.1} ({} frames in {:.1}s)", (v.frames_received() - before) as f64 / t.elapsed().as_secs_f64(), v.frames_received() - before, t.elapsed().as_secs_f64());
            }
            "shot" => {
                pump(&mut v, Duration::from_millis(200), &mut frames);
                let (w, h) = v.frame_size();
                match write_png(arg, w, h, v.pixels()) {
                    Ok(()) => println!("  saved {arg} ({w}x{h})"),
                    Err(e) => println!("  could not save: {e}"),
                }
            }
            "move" => {
                let (x, y) = xy(arg);
                v.send(ToHelper::Mouse { kind: MouseKind::Move, x, y, button: 0, buttons: 0, mods: 0, clicks: 0 });
            }
            "click" => {
                let (x, y) = xy(arg);
                v.send(ToHelper::Mouse { kind: MouseKind::Move, x, y, button: 0, buttons: 0, mods: 0, clicks: 0 });
                pump(&mut v, Duration::from_millis(100), &mut frames);
                v.send(ToHelper::Mouse { kind: MouseKind::Down, x, y, button: 1, buttons: 1, mods: 0, clicks: 1 });
                pump(&mut v, Duration::from_millis(60), &mut frames);
                v.send(ToHelper::Mouse { kind: MouseKind::Up, x, y, button: 1, buttons: 0, mods: 0, clicks: 1 });
            }
            "type" => {
                for c in arg.chars() {
                    let sym = keys::from_char(c);
                    v.send(ToHelper::Key { down: true, keysym: sym, mods: 0, text: c.to_string() });
                    v.send(ToHelper::Key { down: false, keysym: sym, mods: 0, text: String::new() });
                    pump(&mut v, Duration::from_millis(40), &mut frames);
                }
            }
            "key" => {
                let sym = match arg {
                    "ret" => keys::RETURN,
                    "tab" => keys::TAB,
                    "esc" => keys::ESCAPE,
                    "bs" => keys::BACKSPACE,
                    "down" => keys::DOWN,
                    _ => keys::SPACE,
                };
                v.send(ToHelper::Key { down: true, keysym: sym, mods: 0, text: String::new() });
                v.send(ToHelper::Key { down: false, keysym: sym, mods: 0, text: String::new() });
            }
            "wheel" => {
                let (dx, dy) = xy(arg);
                v.send(ToHelper::Wheel { x: hello_center(&v).0, y: hello_center(&v).1, dx, dy, mods: 0 });
            }
            "nav" => {
                v.send(ToHelper::Navigate(arg.to_string()));
            }
            "resize" => {
                let (w, h) = xy(arg);
                v.send(ToHelper::Resize { w: w as u32, h: h as u32 });
            }
            "bench" => {
                // bench:W,H  the per-frame pixel work of the shell's page area at that size (nothing is drawn on the screen)
                let (w, h) = xy(if arg.is_empty() { "2560,1400" } else { arg });
                bench(w, h);
            }
            "measure" => {
                // measure:SECS  or  measure:SECS,WHEEL_DY,STEP_MS (a wheel notch of DY every STEP_MS, turning round every 2 s)
                let p: Vec<&str> = arg.split(',').collect();
                let secs: f32 = p.first().and_then(|s| s.parse().ok()).unwrap_or(10.0);
                let wheel = match (p.get(1).and_then(|s| s.parse::<i32>().ok()), p.get(2).and_then(|s| s.parse::<u64>().ok())) {
                    (Some(dy), Some(ms)) => Some((dy, ms.max(1))),
                    _ => None,
                };
                measure(&mut v, secs, wheel, &mut frames);
            }
            "eval" => {
                // the answer is printed by `pump` as `script ID ok=...: JSON`
                v.send(ToHelper::Script { id: 1, js: arg.to_string(), timeout_ms: 8000 });
                pump(&mut v, Duration::from_millis(1500), &mut frames);
            }
            "focus" => {
                v.send(ToHelper::Focus(true));
            }
            _ => println!("  (unknown step)"),
        }
    }
    println!("frames: {}", v.frames_received());
}

/// Time the shell's per-frame work on a page area of `pw` x `ph` pixels (the screen is 2560x1600): channel swap, white fill, 1:1 copy
/// of the whole picture and of a 1 % rectangle, and the 2x smooth stretch of a quarter-size picture. Milliseconds per call.
fn bench(pw: i32, ph: i32) {
    use iyakki_webview::blit::{bgra_to_rgba_opaque, blit_region, blit_region_x2};
    let mut seed = 1u64;
    let mut next = move || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) as u8
    };
    let src: Vec<u8> = (0..pw * ph * 4).map(|i| if i % 4 == 3 { 255 } else { next() }).collect();
    let mut pm = vec![0u8; src.len()];
    let (gw, gh) = (2560i32.max(pw), 1600i32.max(ph + 100));
    let mut screen = vec![0u8; (gw * gh * 4) as usize];
    let mut time = |name: &str, f: &mut dyn FnMut()| {
        f();
        let t = Instant::now();
        for _ in 0..20 {
            f();
        }
        println!("  bench {name:<52} {:8.3} ms", t.elapsed().as_secs_f64() * 50.0);
    };
    time("channel swap, loop as it was (before)", &mut || {
        for (d, s) in pm.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            if s[3] == 255 {
                d.copy_from_slice(&[s[2], s[1], s[0], 255]);
            } else {
                let rest = 255 - s[3];
                d.copy_from_slice(&[s[2].saturating_add(rest), s[1].saturating_add(rest), s[0].saturating_add(rest), 255]);
            }
        }
    });
    time("channel swap, the shipped function, row by row", &mut || {
        for (d, s) in pm.chunks_mut(pw as usize * 4).zip(src.chunks(pw as usize * 4)) {
            bgra_to_rgba_opaque(s, d);
        }
    });
    time("channel swap, the shipped function, one call (after)", &mut || {
        bgra_to_rgba_opaque(&src, &mut pm);
    });
    time("channel swap, loop as it was (before, again)", &mut || {
        for (d, s) in pm.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            if s[3] == 255 {
                d.copy_from_slice(&[s[2], s[1], s[0], 255]);
            } else {
                let rest = 255 - s[3];
                d.copy_from_slice(&[s[2].saturating_add(rest), s[1].saturating_add(rest), s[0].saturating_add(rest), 255]);
            }
        }
    });
    time("channel swap, the shipped function, row by row", &mut || {
        for (d, s) in pm.chunks_mut(pw as usize * 4).zip(src.chunks(pw as usize * 4)) {
            bgra_to_rgba_opaque(s, d);
        }
    });
    time("channel swap, the shipped function, one call (after, again)", &mut || {
        bgra_to_rgba_opaque(&src, &mut pm);
    });
    time("white fill of the whole area (before, every frame)", &mut || {
        for p in screen[..(pw * ph * 4) as usize].chunks_exact_mut(4) {
            p.copy_from_slice(&[255, 255, 255, 255]);
        }
        std::hint::black_box(&mut screen);
    });
    time("copy the whole picture 1:1 into the scene", &mut || {
        blit_region(&pm, pw, ph, &mut screen, gw, gh, (0, 100, pw, ph), (0, 0, pw, ph));
    });
    time("copy a 1 % rectangle 1:1 (a tile-only update)", &mut || {
        blit_region(&pm, pw, ph, &mut screen, gw, gh, (0, 100, pw, ph), (pw / 10, ph / 10, pw / 10, ph / 10));
    });
    let (hw, hh) = (pw / 2, ph / 2);
    time("smooth: stretch a half-size picture to 2x (whole)", &mut || {
        blit_region_x2(&pm, hw, hh, &mut screen, gw, gh, (0, 100, pw, ph), (0, 0, hw, hh));
    });
}

/// (pid, name, parent, cpu ticks, rss kB) of every process, from /proc (empty where there is none).
fn proc_table() -> Vec<(u32, String, u32, u64, u64)> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/proc") else { return out };
    for e in rd.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
        let (Some(open), Some(close)) = (stat.find('('), stat.rfind(')')) else { continue };
        let name = stat[open + 1..close].to_string();
        let f: Vec<&str> = stat[close + 2..].split(' ').collect();
        let get = |i: usize| f.get(i).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        let rss_pages = get(21);
        out.push((pid, name, get(1) as u32, get(11) + get(12), rss_pages * 4));
    }
    out
}

/// (thread id, name, cpu ticks) of every thread of a process.
fn thread_table(pid: u32) -> Vec<(u32, String, u64)> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(format!("/proc/{pid}/task")) else { return out };
    for e in rd.flatten() {
        let Some(tid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/task/{tid}/stat")) else { continue };
        let (Some(open), Some(close)) = (stat.find('('), stat.rfind(')')) else { continue };
        let f: Vec<&str> = stat[close + 2..].split(' ').collect();
        let get = |i: usize| f.get(i).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        out.push((tid, stat[open + 1..close].to_string(), get(11) + get(12)));
    }
    out
}

/// Run for `secs`, optionally sending wheel steps, and print the frame rate, the pixels the frames carried and the CPU of the engine's processes.
fn measure(v: &mut WebView, secs: f32, wheel: Option<(i32, u64)>, frames: &mut u64) {
    let hz = 100.0; // USER_HZ
    let before = proc_table();
    // the busiest engine process is also looked at thread by thread (what a video or a scroll spends its cores on)
    let busiest: Option<u32> = before.iter().filter(|p| p.1.contains("WPEWebProcess")).max_by_key(|p| p.3).map(|p| p.0);
    let threads_before = busiest.map(thread_table).unwrap_or_default();
    let (f0, t0) = (v.frames_received(), Instant::now());
    let (mut px, mut gaps, mut last, mut worst) = (0u64, 0u32, Instant::now(), Duration::ZERO);
    let mut next_wheel = Instant::now();
    let (cx, cy) = hello_center(v);
    while t0.elapsed().as_secs_f32() < secs {
        if let Some((dy, ms)) = wheel {
            if Instant::now() >= next_wheel {
                let dir = if (t0.elapsed().as_millis() / 2000) % 2 == 1 { -1 } else { 1 };
                v.send(ToHelper::Wheel { x: cx, y: cy, dx: 0, dy: dy * dir, mods: 0 });
                next_wheel += Duration::from_millis(ms);
            }
        }
        let n0 = v.frames_received();
        for e in v.poll() {
            if let Event::Frame = e {
                *frames += 1;
            }
        }
        if v.frames_received() != n0 {
            let gap = last.elapsed();
            worst = worst.max(gap);
            if gap > Duration::from_millis(100) {
                gaps += 1;
            }
            last = Instant::now();
            if let Some(r) = v.take_dirty() {
                px += r.w as u64 * r.h as u64;
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let el = t0.elapsed().as_secs_f64();
    let n = v.frames_received() - f0;
    let (fw, fh) = v.frame_size();
    println!(
        "  measure: {:.1} fps ({n} frames in {el:.1}s), worst gap {} ms, {gaps} gaps over 100 ms, dirty area per frame {:.0}% of {fw}x{fh} (union of each poll)",
        n as f64 / el,
        worst.as_millis(),
        if n > 0 && fw > 0 && fh > 0 { px as f64 * 100.0 / (n as f64 * fw as f64 * fh as f64) } else { 0.0 }
    );
    let after = proc_table();
    let mut total = 0.0;
    for (pid, name, _ppid, ticks, rss) in &after {
        let b = before.iter().find(|x| x.0 == *pid).map_or(0, |x| x.3);
        let cpu = (ticks.saturating_sub(b)) as f64 / hz / el * 100.0;
        if cpu >= 1.0 || name.contains("WPE") || name.contains("webview") {
            println!("  cpu {cpu:6.1}%  rss {:6} MB  pid {pid} {name}", rss / 1024);
            total += cpu;
        }
    }
    if let Some(pid) = busiest {
        let mut rows: Vec<(f64, String)> = thread_table(pid)
            .into_iter()
            .map(|(tid, name, t)| (t.saturating_sub(threads_before.iter().find(|x| x.0 == tid).map_or(0, |x| x.2)) as f64 / hz / el * 100.0, name))
            .filter(|r| r.0 >= 3.0)
            .collect();
        rows.sort_by(|a, b| b.0.total_cmp(&a.0));
        let top: Vec<String> = rows.iter().take(14).map(|(c, n)| format!("{n} {c:.0}%")).collect();
        println!("  threads of the busiest web process (>= 3 %): {}", top.join(", "));
    }
    println!("  cpu total (processes shown): {total:.0}% of {} cores", std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1));
}

fn hello_center(v: &WebView) -> (i32, i32) {
    let (w, h) = v.frame_size();
    (w as i32 / 2, h as i32 / 2)
}
