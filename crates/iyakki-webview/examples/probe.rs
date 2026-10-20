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

fn hello_center(v: &WebView) -> (i32, i32) {
    let (w, h) = v.frame_size();
    (w as i32 / 2, h as i32 / 2)
}
