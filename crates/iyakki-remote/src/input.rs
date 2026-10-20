//! What the browser sends (small JSON messages) turned into the shell's own input events.

use iyakki_shell::ui::{Event, Key, Nav};
use serde_json::Value;

pub const MIN_W: u32 = 640;
pub const MIN_H: u32 = 400;
pub const MAX_W: u32 = 3840;
pub const MAX_H: u32 = 2160;

#[derive(Debug, PartialEq)]
pub enum Input {
    Event(Event),
    Size(u32, u32),
    /// Send the whole picture again (the page lost it).
    Full,
    /// Shift is held (or not) at the next click: a shift+click extends a text selection.
    Shift(bool),
    /// The page's speech synthesis speaking (browser voice, `docs/REMOTE-PROTOCOL.md`).
    Speech(iyakki_agent::hostvoice::FromClient),
}

fn int(v: &Value, k: &str) -> Option<i32> {
    v.get(k)?.as_f64().filter(|n| n.is_finite() && n.abs() < 100_000.0).map(|n| n as i32)
}

fn key(name: &str, ctrl: bool, shift: bool) -> Option<Key> {
    let nav = |n: Nav, plain: Key| if ctrl || shift { Key::Nav { nav: n, shift, ctrl } } else { plain };
    Some(match name {
        "Enter" => Key::Enter,
        "Backspace" => Key::Backspace,
        "Delete" => Key::Delete,
        "Tab" if shift => Key::BackTab,
        "Tab" => Key::Tab,
        "Escape" => Key::Esc,
        "ArrowUp" => nav(Nav::Up, Key::Up),
        "ArrowDown" => nav(Nav::Down, Key::Down),
        "ArrowLeft" => nav(Nav::Left, Key::Left),
        "ArrowRight" => nav(Nav::Right, Key::Right),
        "Home" => nav(Nav::Home, Key::Home),
        "End" => nav(Nav::End, Key::End),
        "PageUp" => nav(Nav::PageUp, Key::PageUp),
        "PageDown" => nav(Nav::PageDown, Key::PageDown),
        f if f.len() >= 2 && f.starts_with('F') => match f[1..].parse::<u8>() {
            Ok(n) if (1..=12).contains(&n) => Key::F(n),
            _ => return None,
        },
        s => {
            let mut it = s.chars();
            let (Some(c), None) = (it.next(), it.next()) else { return None };
            if ctrl {
                if !c.is_ascii_alphabetic() {
                    return None;
                }
                // Ctrl+Shift+letter is the uppercase letter
                Key::Ctrl(if shift { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() })
            } else if c.is_control() {
                return None;
            } else {
                Key::Char(c)
            }
        }
    })
}

/// One message from the page, or `None` when it is not something we understand (it is ignored, never trusted).
pub fn parse(text: &str) -> Option<Input> {
    let v: Value = serde_json::from_str(text).ok()?;
    let (x, y) = (int(&v, "x"), int(&v, "y"));
    Some(match v.get("t")?.as_str()? {
        "size" => {
            let (w, h) = (int(&v, "w")?, int(&v, "h")?);
            Input::Size((w.max(0) as u32).clamp(MIN_W, MAX_W), (h.max(0) as u32).clamp(MIN_H, MAX_H))
        }
        "move" => Input::Event(Event::MouseMove(x?, y?)),
        "down" => Input::Event(Event::MouseDown(x?, y?)),
        "right" => Input::Event(Event::MouseRight(x?, y?)),
        "up" => Input::Event(Event::MouseUp(x?, y?)),
        "wheel" => Input::Event(Event::Scroll(int(&v, "d")?.clamp(-20, 20))),
        "key" => {
            let k = v.get("key")?.as_str()?;
            let flag = |n: &str| v.get(n).and_then(Value::as_bool).unwrap_or(false);
            Input::Event(Event::Key(key(k, flag("ctrl"), flag("shift"))?))
        }
        "paste" => {
            let t: String = v.get("s")?.as_str()?.chars().take(200_000).collect();
            Input::Event(Event::Paste(t))
        }
        "full" => Input::Full,
        "shift" => Input::Shift(v.get("on").and_then(Value::as_bool).unwrap_or(false)),
        t if t.starts_with("sp_") => Input::Speech(iyakki_agent::hostvoice::FromClient::decode(text)?),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> Option<Event> {
        match parse(s)? {
            Input::Event(e) => Some(e),
            _ => None,
        }
    }

    #[test]
    fn pointer_messages_become_mouse_events() {
        assert_eq!(ev(r#"{"t":"move","x":10,"y":20}"#), Some(Event::MouseMove(10, 20)));
        assert_eq!(ev(r#"{"t":"down","x":5.7,"y":6}"#), Some(Event::MouseDown(5, 6)));
        assert_eq!(ev(r#"{"t":"right","x":1,"y":2}"#), Some(Event::MouseRight(1, 2)), "the right button opens the context menu");
        assert_eq!(parse(r#"{"t":"shift","on":true}"#), Some(Input::Shift(true)));
        assert_eq!(parse(r#"{"t":"shift"}"#), Some(Input::Shift(false)));
        assert_eq!(ev(r#"{"t":"wheel","d":-3}"#), Some(Event::Scroll(-3)));
        assert_eq!(ev(r#"{"t":"wheel","d":9999}"#), Some(Event::Scroll(20)), "a huge scroll is capped");
        assert_eq!(ev(r#"{"t":"move","x":1}"#), None, "a missing coordinate is no event");
        assert_eq!(ev(r#"{"t":"move","x":1e300,"y":1}"#), None, "an absurd coordinate is no event");
    }

    #[test]
    fn keys_map_the_way_the_shell_expects() {
        assert_eq!(ev(r#"{"t":"key","key":"a"}"#), Some(Event::Key(Key::Char('a'))));
        assert_eq!(ev(r#"{"t":"key","key":"é"}"#), Some(Event::Key(Key::Char('é'))));
        assert_eq!(ev(r#"{"t":"key","key":" "}"#), Some(Event::Key(Key::Char(' '))));
        assert_eq!(ev(r#"{"t":"key","key":"Enter"}"#), Some(Event::Key(Key::Enter)));
        assert_eq!(ev(r#"{"t":"key","key":"Escape"}"#), Some(Event::Key(Key::Esc)));
        assert_eq!(ev(r#"{"t":"key","key":"Tab","shift":true}"#), Some(Event::Key(Key::BackTab)));
        assert_eq!(ev(r#"{"t":"key","key":"ArrowLeft"}"#), Some(Event::Key(Key::Left)));
        assert_eq!(ev(r#"{"t":"key","key":"ArrowLeft","shift":true,"ctrl":true}"#), Some(Event::Key(Key::Nav { nav: Nav::Left, shift: true, ctrl: true })));
        assert_eq!(ev(r#"{"t":"key","key":"c","ctrl":true}"#), Some(Event::Key(Key::Ctrl('c'))));
        assert_eq!(ev(r#"{"t":"key","key":"V","ctrl":true,"shift":true}"#), Some(Event::Key(Key::Ctrl('V'))));
        assert_eq!(ev(r#"{"t":"key","key":"F12"}"#), Some(Event::Key(Key::F(12))));
        assert_eq!(ev(r#"{"t":"key","key":"F13"}"#), None);
        assert_eq!(ev(r#"{"t":"key","key":"Shift"}"#), None, "modifier keys alone do nothing");
        assert_eq!(ev(r#"{"t":"key","key":"\u0007"}"#), None, "control characters are not typed");
        assert_eq!(ev(r#"{"t":"key","key":"1","ctrl":true}"#), None);
    }

    #[test]
    fn size_is_kept_within_sane_bounds_and_other_messages_work() {
        assert_eq!(parse(r#"{"t":"size","w":1,"h":1}"#), Some(Input::Size(MIN_W, MIN_H)));
        assert_eq!(parse(r#"{"t":"size","w":99999,"h":99999}"#), Some(Input::Size(MAX_W, MAX_H)));
        assert_eq!(parse(r#"{"t":"size","w":1280,"h":720}"#), Some(Input::Size(1280, 720)));
        assert_eq!(parse(r#"{"t":"full"}"#), Some(Input::Full));
        assert_eq!(ev(r#"{"t":"paste","s":"héllo"}"#), Some(Event::Paste("héllo".into())));
        assert_eq!(parse(r#"{"t":"sp_end","id":4}"#), Some(Input::Speech(iyakki_agent::hostvoice::FromClient::Ended { id: 4 })));
        assert_eq!(parse(r#"{"t":"sp_end"}"#), None, "a speech event without an id is ignored");
        assert_eq!(parse(r#"{"t":"sp_bogus","id":1}"#), None);
        assert_eq!(parse("not json"), None);
        assert_eq!(parse(r#"{"t":"reboot"}"#), None);
        assert_eq!(parse(r#"{"x":1}"#), None);
    }
}
