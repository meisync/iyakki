//! The `browser` domain of a browser tile (F-SDK-64): what the UI SDK's `BrowserPort` asks of a tile host, mapped onto the core's embedded browser
//! sessions (`iyakki_core_api::BrowserSessions`). The page is loaded and drawn by the core; the tile only holds a session number and gets the
//! page back as a display-list text. A tile reaches only the sessions of its own tile id (the app name the core keys them by).
//!
//! The core's questions (a download, a permission a page asks for) are the person's: `browser.prompts` lists them, a tile may answer **no**, and a
//! **yes** is taken from the page by the web service's approval card (`TileBackend::browser_call`), never from the tile.

use iyakki_core_api::browsersession::{BrowserError, EnginePref, FindState, Frame, Input, Navigate, OpenSpec, PageDefaults, PageFont, PageKey, PageState, Palette, Prompt, PromptKind, Scheme, SessionId};
use iyakki_core_api::BrowserSessions;
use serde_json::{json, Value};

/// Largest HTML a tile may hand to `navigate` and the longest address and text.
pub const MAX_HTML: usize = 512 * 1024;
pub const MAX_URL: usize = 4096;
pub const MAX_TEXT: usize = 4096;

#[derive(Debug, PartialEq)]
pub enum Req {
    Open(OpenSpec),
    Close(u64),
    Navigate(u64, Navigate),
    Input(u64, Input),
    Find(u64, String, bool),
    State(u64),
    Frame(u64, u64),
    Prompts(u64),
    /// `allow` true is the person's yes: the web service asks them first.
    Answer(u64, u64, bool),
}

fn num(v: &Value, k: &str, lo: f64, hi: f64) -> Result<f32, String> {
    match v.get(k).and_then(Value::as_f64) {
        Some(n) if (lo..=hi).contains(&n) => Ok(n as f32),
        _ => Err(format!("\"{k}\" is a number from {lo} to {hi}")),
    }
}

fn opt_num(v: &Value, k: &str, lo: f64, hi: f64, default: f32) -> Result<f32, String> {
    if v.get(k).is_none() { Ok(default) } else { num(v, k, lo, hi) }
}

fn id(v: &Value) -> Result<u64, String> {
    v.get("id").and_then(Value::as_u64).ok_or_else(|| "\"id\" is the number open gave".to_string())
}

fn text<'a>(v: &'a Value, k: &str, max: usize) -> Result<&'a str, String> {
    match v.get(k).and_then(Value::as_str) {
        Some(s) if s.len() <= max && !s.contains('\0') => Ok(s),
        _ => Err(format!("\"{k}\" is text of at most {max} bytes")),
    }
}

fn colour(v: &Value, k: &str, default: [u8; 4]) -> Result<[u8; 4], String> {
    match v.get(k) {
        None => Ok(default),
        Some(a) => {
            let a = a.as_array().filter(|a| a.len() == 4).ok_or_else(|| format!("\"{k}\" is four numbers from 0 to 255"))?;
            let mut out = [0u8; 4];
            for (i, n) in a.iter().enumerate() {
                out[i] = n.as_u64().filter(|n| *n <= 255).ok_or_else(|| format!("\"{k}\" is four numbers from 0 to 255"))? as u8;
            }
            Ok(out)
        }
    }
}

pub fn plan(method: &str, args_json: &str) -> Result<Req, String> {
    let v: Value = serde_json::from_str(if args_json.trim().is_empty() { "{}" } else { args_json }).map_err(|_| "the arguments are not JSON".to_string())?;
    if !v.is_object() {
        return Err("the arguments are a JSON object".into());
    }
    Ok(match method {
        "open" => {
            let d = Palette::default();
            let pal = v.get("palette").cloned().unwrap_or(Value::Null);
            let palette = Palette { bg: colour(&pal, "bg", d.bg)?, text: colour(&pal, "text", d.text)?, muted: colour(&pal, "muted", d.muted)?, link: colour(&pal, "link", d.link)?, rule: colour(&pal, "rule", d.rule)?, code_bg: colour(&pal, "code_bg", d.code_bg)?, mark: colour(&pal, "mark", d.mark)? };
            let scheme = match v.get("scheme").and_then(Value::as_str).unwrap_or("dark") {
                "dark" => Scheme::Dark,
                "light" => Scheme::Light,
                "high-contrast" => Scheme::HighContrast,
                _ => return Err("\"scheme\" is dark, light or high-contrast".into()),
            };
            let font = match v.get("family").and_then(Value::as_str).unwrap_or("sans") {
                "sans" => PageFont::Sans,
                "mono" => PageFont::Mono,
                _ => return Err("\"family\" is sans or mono".into()),
            };
            let engine = match v.get("engine").and_then(Value::as_str) {
                None => EnginePref::InHouse,
                Some(k) => EnginePref::from_key(k).ok_or("\"engine\" is in-house, full or auto")?,
            };
            Req::Open(OpenSpec {
                width: num(&v, "width", 1.0, 8192.0)?,
                height: num(&v, "height", 1.0, 8192.0)?,
                text_scale: opt_num(&v, "text_scale", 0.5, 4.0, 1.0)?,
                engine,
                palette,
                defaults: PageDefaults { scheme, font_size: opt_num(&v, "font_size", 8.0, 48.0, 15.0)?, font, reduced_motion: v.get("reduced_motion").and_then(Value::as_bool).unwrap_or(false) },
            })
        }
        "close" => Req::Close(id(&v)?),
        "navigate" => {
            let nav = match v.get("kind").and_then(Value::as_str) {
                Some("url") => Navigate::Url(text(&v, "url", MAX_URL)?.to_string()),
                Some("html") => Navigate::Html { base: text(&v, "base", MAX_URL)?.to_string(), html: text(&v, "html", MAX_HTML)?.to_string() },
                Some("back") => Navigate::Back,
                Some("forward") => Navigate::Forward,
                Some("reload") => Navigate::Reload,
                Some("stop") => Navigate::Stop,
                _ => return Err("\"kind\" is url, html, back, forward, reload or stop".into()),
            };
            Req::Navigate(id(&v)?, nav)
        }
        "input" => {
            let input = match v.get("kind").and_then(Value::as_str) {
                Some("activate") => Input::Activate(v.get("n").and_then(Value::as_u64).filter(|n| *n <= u32::MAX as u64).ok_or("\"n\" is the number of the link")? as u32),
                Some("wheel") => Input::Wheel(v.get("n").and_then(Value::as_i64).filter(|n| n.abs() <= 1000).ok_or("\"n\" is a whole number from -1000 to 1000")? as i32),
                Some("key") => Input::Key(match v.get("key").and_then(Value::as_str) {
                    Some("up") => PageKey::Up,
                    Some("down") => PageKey::Down,
                    Some("page_up") => PageKey::PageUp,
                    Some("page_down") => PageKey::PageDown,
                    Some("home") => PageKey::Home,
                    Some("end") => PageKey::End,
                    Some("enter") => PageKey::Enter,
                    Some("esc") => PageKey::Esc,
                    _ => return Err("\"key\" is up, down, page_up, page_down, home, end, enter or esc".into()),
                }),
                Some("text") => Input::Text(text(&v, "text", MAX_TEXT)?.chars().filter(|c| !c.is_control() || *c == '\n').collect()),
                Some("resize") => Input::Resize { width: num(&v, "width", 1.0, 8192.0)?, height: num(&v, "height", 1.0, 8192.0)? },
                _ => return Err("\"kind\" is activate, wheel, key, text or resize".into()),
            };
            Req::Input(id(&v)?, input)
        }
        "find" => Req::Find(id(&v)?, text(&v, "query", 200)?.to_string(), v.get("forward").and_then(Value::as_bool).unwrap_or(true)),
        "state" => Req::State(id(&v)?),
        "frame" => Req::Frame(id(&v)?, v.get("since").and_then(Value::as_u64).unwrap_or(0)),
        "prompts" => Req::Prompts(id(&v)?),
        "answer" => Req::Answer(id(&v)?, v.get("prompt").and_then(Value::as_u64).ok_or("\"prompt\" is the number prompts gave")?, v.get("allow").and_then(Value::as_bool).ok_or("\"allow\" is true or false")?),
        _ => return Err("that is not a call this tile may make".into()),
    })
}

fn sentence(e: BrowserError) -> String {
    e.to_string()
}

fn state_json(s: PageState) -> Value {
    json!({"url": s.url, "title": s.title, "loading": s.loading, "can_back": s.can_back, "can_forward": s.can_forward, "error": s.error, "engine": s.engine.key(), "find": find_json(&s.find), "generation": s.generation, "scroll_permille": s.scroll_permille})
}

fn find_json(f: &FindState) -> Value {
    json!({"query": f.query, "matches": f.matches, "current": f.current})
}

fn prompt_json(p: &Prompt) -> Value {
    json!({"id": p.id, "kind": match p.kind { PromptKind::Download => "download", PromptKind::Permission => "permission" }, "origin": p.origin, "text": p.text})
}

/// Run a planned request for `user` and the app name `app` (the tile's id). `Req::Answer` with `allow` true must have been approved by the
/// person before this is called; this function does not ask.
pub fn run(b: &dyn BrowserSessions, user: &str, app: &str, req: &Req) -> Result<String, String> {
    let sid = |n: &u64| SessionId(*n);
    Ok(match req {
        Req::Open(spec) => json!({"id": b.open(user, app, spec).map_err(sentence)?.0}).to_string(),
        Req::Close(n) => {
            b.close(user, app, sid(n));
            json!({"ok": true}).to_string()
        }
        Req::Navigate(n, nav) => {
            b.navigate(user, app, sid(n), nav).map_err(sentence)?;
            json!({"ok": true}).to_string()
        }
        Req::Input(n, input) => {
            b.input(user, app, sid(n), input).map_err(sentence)?;
            json!({"ok": true}).to_string()
        }
        Req::Find(n, q, fwd) => find_json(&b.find(user, app, sid(n), q, *fwd).map_err(sentence)?).to_string(),
        Req::State(n) => state_json(b.state(user, app, sid(n)).map_err(sentence)?).to_string(),
        Req::Frame(n, since) => match b.frame(user, app, sid(n), *since).map_err(sentence)? {
            None => Value::Null.to_string(),
            Some(Frame { generation, list }) => json!({"generation": generation, "list": list}).to_string(),
        },
        Req::Prompts(n) => Value::Array(b.prompts(user, app, sid(n)).iter().map(prompt_json).collect()).to_string(),
        Req::Answer(n, p, allow) => {
            b.answer_prompt(user, app, sid(n), *p, *allow).map_err(sentence)?;
            json!({"ok": true}).to_string()
        }
    })
}

/// The words of the person's card for a yes to a prompt (the page's own text is data: cut and cleaned).
pub fn card_words(p: &Prompt) -> String {
    let what = match p.kind {
        PromptKind::Download => "a download",
        PromptKind::Permission => "a permission",
    };
    let clean = |s: &str, n: usize| s.chars().filter(|c| !c.is_control()).take(n).collect::<String>();
    format!("Allow {what} for {}: {}", clean(&p.origin, 80), clean(&p.text, 120))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_checks_numbers_words_and_colours_and_fills_the_defaults() {
        let Req::Open(s) = plan("open", r#"{"width":320,"height":240}"#).unwrap() else { panic!() };
        assert_eq!((s.width, s.height, s.text_scale, s.engine), (320.0, 240.0, 1.0, EnginePref::InHouse));
        let Req::Open(s) = plan("open", r#"{"width":10,"height":20,"engine":"full","scheme":"light","family":"mono","font_size":20,"reduced_motion":true,"text_scale":2,"palette":{"bg":[1,2,3,255]}}"#).unwrap() else { panic!() };
        assert_eq!((s.engine, s.defaults.scheme, s.defaults.font, s.defaults.font_size, s.defaults.reduced_motion, s.palette.bg), (EnginePref::Full, Scheme::Light, PageFont::Mono, 20.0, true, [1, 2, 3, 255]));
        for bad in [r#"{}"#, r#"{"width":0,"height":5}"#, r#"{"width":99999,"height":5}"#, r#"{"width":5,"height":5,"engine":"x"}"#, r#"{"width":5,"height":5,"scheme":"x"}"#, r#"{"width":5,"height":5,"palette":{"bg":[1,2,3]}}"#, r#"{"width":5,"height":5,"palette":{"bg":[1,2,3,999]}}"#, r#"{"width":5,"height":5,"text_scale":50}"#, "[1]", "nope"] {
            assert!(plan("open", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn navigation_input_find_and_answers_are_checked_before_the_core_is_asked() {
        assert_eq!(plan("navigate", r#"{"id":3,"kind":"url","url":"https://example.org/"}"#).unwrap(), Req::Navigate(3, Navigate::Url("https://example.org/".into())));
        assert_eq!(plan("navigate", r#"{"id":3,"kind":"back"}"#).unwrap(), Req::Navigate(3, Navigate::Back));
        assert!(matches!(plan("navigate", r#"{"id":3,"kind":"html","base":"about:blank","html":"<p>x"}"#).unwrap(), Req::Navigate(3, Navigate::Html { .. })));
        assert_eq!(plan("input", r#"{"id":1,"kind":"key","key":"page_down"}"#).unwrap(), Req::Input(1, Input::Key(PageKey::PageDown)));
        assert_eq!(plan("input", "{\"id\":1,\"kind\":\"text\",\"text\":\"a\\u0007b\\nc\"}").unwrap(), Req::Input(1, Input::Text("ab\nc".into())));
        assert_eq!(plan("input", r#"{"id":1,"kind":"wheel","n":-3}"#).unwrap(), Req::Input(1, Input::Wheel(-3)));
        assert_eq!(plan("find", r#"{"id":1,"query":"cat"}"#).unwrap(), Req::Find(1, "cat".into(), true));
        assert_eq!(plan("answer", r#"{"id":1,"prompt":7,"allow":false}"#).unwrap(), Req::Answer(1, 7, false));
        let long = "x".repeat(MAX_URL + 1);
        for (m, a) in [("navigate", r#"{"kind":"url","url":"a"}"#.to_string()), ("navigate", r#"{"id":1,"kind":"go"}"#.into()), ("navigate", format!(r#"{{"id":1,"kind":"url","url":"{long}"}}"#)), ("input", r#"{"id":1,"kind":"wheel","n":999999}"#.into()), ("input", r#"{"id":1,"kind":"key","key":"f12"}"#.into()), ("find", format!(r#"{{"id":1,"query":"{}"}}"#, "q".repeat(300))), ("answer", r#"{"id":1,"prompt":7}"#.into()), ("state", "{}".into()), ("settings", "{}".into()), ("set_enabled", r#"{"id":1}"#.into())] {
            assert!(plan(m, &a).is_err(), "{m} {a}");
        }
    }

    #[test]
    fn a_core_with_no_browser_answers_with_its_sentence_and_the_card_words_are_cleaned() {
        let e = run(&iyakki_core_api::NoBrowser, "u", "tile", &Req::State(1)).unwrap_err();
        assert!(e.contains("no embedded browser"), "{e}");
        let p = Prompt { id: 1, kind: PromptKind::Download, origin: "evil.example\u{7}".into(), text: format!("save {} now", "x".repeat(300)) };
        let w = card_words(&p);
        assert!(w.starts_with("Allow a download for evil.example: save xxx") && w.len() < 260 && !w.contains('\u{7}'), "{w}");
    }
}
