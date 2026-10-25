//! Developer mode's `iyakki dev input`: the shell turns what a paired computer queued (`iyakki_agent::devmode::input`) into its own
//! pointer and key events, so a tap is a tap and a key is a key for whatever is on the screen; nothing is injected below the shell.

use iyakki_agent::devmode::input::InputEv;

use crate::ui::{Event, Key};

/// The shell key for a name the device accepted (`Enter`, `Esc`, `Up`, `F5`, `Ctrl+c`, `a`...).
pub fn key_of(name: &str) -> Option<Key> {
    Some(match name {
        "Enter" => Key::Enter,
        "Esc" => Key::Esc,
        "Tab" => Key::Tab,
        "BackTab" => Key::BackTab,
        "Backspace" => Key::Backspace,
        "Delete" => Key::Delete,
        "Up" => Key::Up,
        "Down" => Key::Down,
        "Left" => Key::Left,
        "Right" => Key::Right,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        "Space" => Key::Char(' '),
        n => {
            if let Some(f) = n.strip_prefix('F').and_then(|f| f.parse::<u8>().ok()).filter(|f| (1..=12).contains(f)) {
                Key::F(f)
            } else if let Some(c) = n.strip_prefix("Ctrl+").and_then(|c| c.chars().next()) {
                Key::Ctrl(c)
            } else {
                let mut it = n.chars();
                match (it.next(), it.next()) {
                    (Some(c), None) if !c.is_control() => Key::Char(c),
                    _ => return None,
                }
            }
        }
    })
}

/// The events one queued input stands for, in order.
pub fn events(e: &InputEv) -> Vec<Event> {
    match e {
        InputEv::Tap(x, y) => vec![Event::MouseMove(*x, *y), Event::MouseDown(*x, *y), Event::MouseUp(*x, *y)],
        InputEv::Swipe(x1, y1, x2, y2, _ms) => {
            // press, move in ten steps, release; a vertical swipe also scrolls (a finger drags the content with it)
            let mut v = vec![Event::MouseMove(*x1, *y1), Event::MouseDown(*x1, *y1)];
            for i in 1..=10 {
                v.push(Event::MouseMove(x1 + (x2 - x1) * i / 10, y1 + (y2 - y1) * i / 10));
            }
            v.push(Event::MouseUp(*x2, *y2));
            let dy = y2 - y1;
            if dy.abs() > 20 && (x2 - x1).abs() < dy.abs() {
                v.push(Event::Scroll(if dy < 0 { -3 } else { 3 }));
            }
            v
        }
        InputEv::Key(n) => key_of(n).map(Event::Key).into_iter().collect(),
        InputEv::Text(t) => t.chars().map(|c| if c == '\n' { Event::Key(Key::Enter) } else { Event::Key(Key::Char(c)) }).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_accepted_key_name_has_a_shell_key_and_events_come_out_in_order() {
        for n in ["Enter", "Esc", "Tab", "BackTab", "Backspace", "Delete", "Up", "Down", "Left", "Right", "Home", "End", "PageUp", "PageDown", "Space", "F1", "F12", "Ctrl+c", "a", "é"] {
            assert!(iyakki_agent::devmode::input::valid_key(n) && key_of(n).is_some(), "{n}");
        }
        assert_eq!(key_of("F5"), Some(Key::F(5)));
        assert_eq!(key_of("Ctrl+c"), Some(Key::Ctrl('c')));
        assert_eq!(key_of("F13"), None);
        assert_eq!(key_of("ab"), None);
        assert_eq!(events(&InputEv::Tap(5, 6)), vec![Event::MouseMove(5, 6), Event::MouseDown(5, 6), Event::MouseUp(5, 6)]);
        assert_eq!(events(&InputEv::Text("a\nb".into())), vec![Event::Key(Key::Char('a')), Event::Key(Key::Enter), Event::Key(Key::Char('b'))]);
        let sw = events(&InputEv::Swipe(100, 500, 100, 100, 300));
        assert_eq!(sw.first(), Some(&Event::MouseMove(100, 500)));
        assert_eq!(sw[2], Event::MouseMove(100, 460));
        assert!(sw.contains(&Event::Scroll(-3)) && sw.contains(&Event::MouseUp(100, 100)));
    }
}
