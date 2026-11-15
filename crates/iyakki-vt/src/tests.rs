use crate::*;

fn run(cols: usize, rows: usize, input: &str) -> Screen {
    let mut s = Screen::new(cols, rows);
    Parser::new().feed(&mut s, input.as_bytes());
    s
}

fn lines(s: &Screen) -> Vec<String> {
    (0..s.rows()).map(|y| s.row_text(y)).collect()
}

#[test]
fn text_wraps_and_scrolls_into_the_scrollback() {
    let s = run(5, 3, "abcdefgh\r\nline2\r\nline3\r\nline4");
    assert_eq!(lines(&s), vec!["line2", "line3", "line4"]);
    assert_eq!(s.scrollback_len(), 2);
    assert_eq!(s.scrollback_text(0).unwrap(), "abcde");
    assert_eq!(s.scrollback_text(1).unwrap(), "fgh");
    // the view that is one line back shows the line that scrolled off
    assert_eq!(s.view_row(1, 0).iter().map(|c| c.ch).collect::<String>().trim(), "fgh");
}

#[test]
fn the_last_column_waits_before_wrapping() {
    let s = run(4, 2, "abcd\r\nef");
    assert_eq!(lines(&s), vec!["abcd", "ef"]);
    let s = run(4, 2, "abcd");
    assert_eq!((s.x, s.y), (3, 0), "the cursor waits on the last column");
    let s = run(4, 2, "abcdX");
    assert_eq!(lines(&s), vec!["abcd", "X"]);
    let mut s = Screen::new(4, 2);
    Parser::new().feed(&mut s, b"\x1b[?7labcdef");
    assert_eq!(lines(&s), vec!["abcf", ""], "with wrapping off the last column is overwritten");
}

#[test]
fn cursor_movement_and_erasing() {
    let s = run(10, 4, "hello world\x1b[1;7H\x1b[K");
    assert_eq!(s.row_text(0), "hello");
    let s = run(10, 4, "aaaa\r\nbbbb\r\ncccc\x1b[2;3H\x1b[1J");
    assert_eq!(lines(&s), vec!["", "   b", "cccc", ""]);
    let s = run(10, 4, "abcdef\x1b[3D\x1b[2P");
    assert_eq!(s.row_text(0), "abcf");
    let s = run(10, 4, "abcdef\x1b[3D\x1b[2@Z");
    assert_eq!(s.row_text(0), "abcZ def");
    let s = run(10, 4, "x\x1b[5;5H*");
    assert_eq!((s.x, s.y), (5, 3), "positions are limited to the screen");
    assert_eq!(s.cell(4, 3).ch, '*');
}

#[test]
fn colours_and_attributes() {
    let s = run(10, 2, "\x1b[1;31mR\x1b[0mn\x1b[38;5;200mI\x1b[48;2;1;2;3mT\x1b[39;49m\x1b[7mV");
    assert_eq!(s.cell(0, 0).fg, Color::Indexed(1));
    assert_eq!(s.cell(0, 0).attrs & BOLD, BOLD);
    assert_eq!(s.cell(1, 0).fg, Color::Default);
    assert_eq!(s.cell(1, 0).attrs, 0);
    assert_eq!(s.cell(2, 0).fg, Color::Indexed(200));
    assert_eq!(s.cell(3, 0).bg, Color::Rgb(1, 2, 3));
    assert_eq!(s.cell(3, 0).fg, Color::Indexed(200));
    assert_eq!(s.cell(4, 0).bg, Color::Default);
    assert_eq!(s.cell(4, 0).attrs & REVERSE, REVERSE);
    let s = run(10, 2, "\x1b[91;102mB");
    assert_eq!((s.cell(0, 0).fg, s.cell(0, 0).bg), (Color::Indexed(9), Color::Indexed(10)));
}

#[test]
fn scroll_regions_keep_the_status_line_still() {
    // a 5-row screen with the top and bottom rows fixed: only rows 2-4 scroll
    let s = run(6, 5, "TOP\x1b[5;1HBOT\x1b[2;4r\x1b[4;1Ha\r\nb\r\nc\r\nd");
    assert_eq!(s.row_text(0), "TOP");
    assert_eq!(s.row_text(4), "BOT");
    assert_eq!(s.row_text(1), "b");
    assert_eq!(s.row_text(2), "c");
    assert_eq!(s.row_text(3), "d");
    assert_eq!(s.scrollback_len(), 0, "a partial region never fills the scrollback");
    assert_eq!(s.region(), (1, 3));
    // reverse index at the top of the region scrolls it down
    let s = run(6, 4, "a\r\nb\r\nc\r\nd\x1b[2;3r\x1b[2;1H\x1bM");
    assert_eq!(lines(&s), vec!["a", "", "b", "d"]);
}

#[test]
fn insert_and_delete_lines() {
    let s = run(5, 4, "1\r\n2\r\n3\r\n4\x1b[2;1H\x1b[L");
    assert_eq!(lines(&s), vec!["1", "", "2", "3"]);
    let s = run(5, 4, "1\r\n2\r\n3\r\n4\x1b[2;1H\x1b[M");
    assert_eq!(lines(&s), vec!["1", "3", "4", ""]);
}

#[test]
fn the_alternate_screen_leaves_the_main_one_alone() {
    let mut s = Screen::new(10, 3);
    let mut p = Parser::new();
    p.feed(&mut s, b"shell$ ls\r\nfile\r\n");
    p.feed(&mut s, b"\x1b[?1049h\x1b[2J\x1b[HVIM");
    assert!(s.alt_screen);
    assert_eq!(s.row_text(0), "VIM");
    p.feed(&mut s, b"\x1b[?1049l");
    assert!(!s.alt_screen);
    assert_eq!(lines(&s), vec!["shell$ ls", "file", ""]);
    assert_eq!((s.x, s.y), (0, 2), "the cursor is where it was");
    // the alternate screen never adds to the scrollback
    p.feed(&mut s, b"\x1b[?1049h1\r\n2\r\n3\r\n4\r\n5\x1b[?1049l");
    assert_eq!(s.scrollback_len(), 0);
}

#[test]
fn line_drawing_characters_and_utf8() {
    let s = run(10, 2, "\x1b(0lqqk\x1b(Bx");
    assert_eq!(s.row_text(0), "┌──┐x");
    let s = run(10, 2, "é€😀日本");
    assert_eq!(s.row_text(0), "é€😀日本");
    assert!(s.cell(3, 0).wide_tail, "a wide character takes two cells");
    assert_eq!(s.x, 8);
    // a combining mark has no cell of its own
    let s = run(5, 1, "e\u{301}x");
    assert_eq!(s.row_text(0), "ex");
}

#[test]
fn broken_utf8_and_junk_are_shown_as_replacement_marks_not_lost() {
    let mut s = Screen::new(10, 1);
    Parser::new().feed(&mut s, &[b'a', 0xe2, 0x82, b'b', 0xff, b'c']);
    assert_eq!(s.row_text(0), "a\u{fffd}b\u{fffd}c");
    // a character split between two reads is joined
    let mut s = Screen::new(10, 1);
    let mut p = Parser::new();
    p.feed(&mut s, &[0xe2, 0x82]);
    p.feed(&mut s, &[0xac]);
    assert_eq!(s.row_text(0), "€");
}

#[test]
fn titles_replies_and_modes() {
    let mut s = Screen::new(10, 3);
    let mut p = Parser::new();
    p.feed(&mut s, b"\x1b]0;my\x07host\x1b[?25l\x1b[?2004h\x1b[?1h");
    assert_eq!(s.title, "my");
    assert!(!s.cursor_visible && s.bracketed_paste && s.app_cursor_keys);
    p.feed(&mut s, b"\x1b]2;ev\x7fil\x1b\\ok");
    assert_eq!(s.title, "evil", "control characters are removed from a title");
    p.feed(&mut s, b"\x1b[2;3H\x1b[6n\x1b[c\x1b[5n");
    assert_eq!(String::from_utf8(s.take_replies()).unwrap(), "\x1b[2;3R\x1b[?62;22c\x1b[0n");
    assert!(s.take_replies().is_empty());
}

#[test]
fn unknown_and_hostile_sequences_are_skipped_whole() {
    let s = run(20, 2, "a\x1b[?9999;1;2zb\x1b[99999999999999zc\x1bPsixel data\x1b\\d\x1b_apc\x07e\x1b]52;c;AAAA\x07f");
    assert_eq!(s.row_text(0).replace(' ', ""), "abcdef", "{:?}", s.row_text(0));
    // a very long parameter list does not run the parser out of bounds
    let long = format!("\x1b[{}m!", "1;".repeat(5000));
    let s = run(5, 1, &long);
    assert_eq!(s.row_text(0), "!");
    // CAN ends a sequence
    let s = run(10, 1, "a\x1b[3\x18b");
    assert_eq!(s.row_text(0), "ab");
}

#[test]
fn any_bytes_leave_a_valid_screen() {
    let mut x: u64 = 0x1234_5678_9abc_def1;
    for round in 0..300 {
        let mut s = Screen::new(1 + round % 40, 1 + round % 17);
        let mut p = Parser::new();
        let mut chunk = Vec::new();
        for _ in 0..600 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let b = (x >> 33) as u8;
            // bias toward escape-sequence bytes
            chunk.push(match b % 7 {
                0 => 0x1b,
                1 => b'[',
                2 => b"0123456789;?"[(b as usize / 7) % 12],
                3 => b"HJKmhlrABCDLMPX@"[(b as usize / 7) % 16],
                _ => b,
            });
        }
        p.feed(&mut s, &chunk);
        assert!(s.x < s.cols() && s.y < s.rows());
        let (t, b) = s.region();
        assert!(t < b || (t == 0 && b == s.rows() - 1) || s.rows() == 1);
        s.resize(1 + (round * 7) % 60, 1 + (round * 3) % 30);
        assert!(s.x < s.cols() && s.y < s.rows());
        let _ = s.text();
    }
}

#[test]
fn resizing_keeps_the_cursor_on_the_screen_and_moves_lost_rows_to_the_scrollback() {
    let mut s = run(8, 4, "1\r\n2\r\n3\r\n4");
    s.resize(6, 2);
    assert_eq!((s.cols(), s.rows()), (6, 2));
    assert_eq!(lines(&s), vec!["3", "4"]);
    assert_eq!(s.scrollback_text(s.scrollback_len() - 1).unwrap(), "2");
    s.resize(20, 5);
    assert_eq!(s.cols(), 20);
    assert!(s.y < 5);
    s.resize(0, 0);
    assert_eq!((s.cols(), s.rows()), (1, 1));
}

#[test]
fn keys_become_the_bytes_programs_expect() {
    let n = Mods::default();
    let ctrl = Mods { ctrl: true, ..n };
    let alt = Mods { alt: true, ..n };
    assert_eq!(keys::encode(Key::Char('a'), n, false), b"a");
    assert_eq!(keys::encode(Key::Char('c'), ctrl, false), vec![3]);
    assert_eq!(keys::encode(Key::Char('x'), alt, false), b"\x1bx");
    assert_eq!(keys::encode(Key::Char('é'), n, false), "é".as_bytes());
    assert_eq!(keys::encode(Key::Enter, n, false), b"\r");
    assert_eq!(keys::encode(Key::Backspace, n, false), vec![0x7f]);
    assert_eq!(keys::encode(Key::Up, n, false), b"\x1b[A");
    assert_eq!(keys::encode(Key::Up, n, true), b"\x1bOA", "application cursor keys");
    assert_eq!(keys::encode(Key::Left, ctrl, false), b"\x1b[1;5D");
    assert_eq!(keys::encode(Key::Delete, n, false), b"\x1b[3~");
    assert_eq!(keys::encode(Key::PageDown, alt, false), b"\x1b[6;3~");
    assert_eq!(keys::encode(Key::F(1), n, false), b"\x1bOP");
    assert_eq!(keys::encode(Key::F(5), n, false), b"\x1b[15~");
    assert_eq!(keys::encode(Key::F(12), n, false), b"\x1b[24~");
    assert!(keys::encode(Key::F(40), n, false).is_empty());
    assert_eq!(keys::encode(Key::BackTab, n, false), b"\x1b[Z");
}

#[test]
fn a_paste_cannot_escape_its_bracket() {
    assert_eq!(keys::paste("a\r\nb", false), b"a\rb");
    assert_eq!(keys::paste("x\x1b[201~rm -rf", true), b"\x1b[200~x[201~rm -rf\x1b[201~");
    assert_eq!(keys::paste("\x03\x04", false), b"");
}

#[cfg(unix)]
mod unix {
    use crate::session::Session;
    use crate::*;

    fn env() -> Vec<(String, String)> {
        vec![("TERM".into(), "xterm".into()), ("PATH".into(), "/usr/bin:/bin".into())]
    }

    fn wait_for(s: &mut Session, what: &str) -> bool {
        for _ in 0..200 {
            s.poll();
            if s.screen.text().contains(what) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        false
    }

    #[test]
    fn a_program_sees_a_real_terminal_of_the_asked_size_and_can_be_resized() {
        let mut s = Session::start("/bin/sh", &["-c".into(), "stty size; read x; stty size; echo done".into()], &env(), 60, 20).unwrap();
        assert!(wait_for(&mut s, "20 60"), "{}", s.screen.text());
        s.resize(50, 12);
        s.key(Key::Enter, Mods::default());
        assert!(wait_for(&mut s, "12 50"), "{}", s.screen.text());
        assert!(wait_for(&mut s, "done"));
        for _ in 0..100 {
            s.poll();
            if s.finished() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(s.finished());
        assert_eq!(s.exit_code(), Some(0));
    }

    #[test]
    fn typed_keys_are_echoed_and_ctrl_c_stops_a_running_program() {
        let mut s = Session::start("/bin/sh", &["-c".into(), "echo ready; exec sleep 30".into()], &env(), 40, 6).unwrap();
        assert!(wait_for(&mut s, "ready"));
        s.key(Key::Char('c'), Mods { ctrl: true, ..Mods::default() });
        // generous: on a busy computer the shell may take seconds to hand the terminal to `sleep`
        for _ in 0..1000 {
            s.poll();
            if s.finished() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(s.finished(), "Ctrl+C reaches the program through the terminal");
        assert_eq!(s.exit_code(), Some(128 + 2));
        let mut e = Session::start("/bin/cat", &[], &env(), 40, 6).unwrap();
        e.paste("hello pty\n");
        assert!(wait_for(&mut e, "hello pty"));
        e.close();
    }

    #[test]
    fn the_program_gets_only_the_environment_it_was_given_and_a_missing_program_is_an_error() {
        let mut s = Session::start("/bin/sh", &["-c".into(), "echo [$HOME][$TERM]".into()], &env(), 40, 4).unwrap();
        assert!(wait_for(&mut s, "[][xterm]"), "{}", s.screen.text());
        assert!(Session::start("/no/such/program", &[], &env(), 40, 4).is_err());
    }

    #[test]
    fn a_full_screen_program_uses_the_alternate_screen_and_cursor_addressing() {
        let script = "printf '\\033[?1049h\\033[2J\\033[3;5Hhello'; read x; printf '\\033[?1049l'";
        let mut s = Session::start("/bin/sh", &["-c".into(), script.into()], &env(), 30, 6).unwrap();
        assert!(wait_for(&mut s, "hello"));
        assert!(s.screen.alt_screen);
        assert_eq!(s.screen.row_text(2), "    hello");
        s.key(Key::Enter, Mods::default());
        for _ in 0..100 {
            s.poll();
            if !s.screen.alt_screen {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!s.screen.alt_screen);
    }
}
