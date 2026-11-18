//! Character widths for the page-size estimate, so a line of text breaks near the place a real font breaks it.
//!
//! No font files are read. The table holds the advance widths (thousandths of an em) of the printable ASCII characters of a
//! common sans-serif face (the same proportions as Helvetica, Arial and the metric-compatible Liberation Sans and Arimo); a
//! fixed-pitch face uses 0.6 em a character; characters of wide scripts (CJK, full-width forms) take one em and other
//! characters 0.56 em. The reader draws with its own font, so this stays an estimate of what a browser with a common face would
//! show, not a measurement of the face in use.

/// The same for a Times-like serif (Times New Roman, Liberation Serif, Nimbus Roman).
const SERIF: [u16; 95] = [
    250, 333, 408, 500, 500, 833, 778, 180, 333, 333, 500, 564, 250, 333, 250, 278, // space ! " # $ % & ' ( ) * + , - . /
    500, 500, 500, 500, 500, 500, 500, 500, 500, 500, 278, 278, 564, 564, 564, 444, // 0-9 : ; < = > ?
    921, 722, 667, 667, 722, 611, 556, 722, 722, 333, 389, 722, 611, 889, 722, 722, // @ A-O
    556, 722, 667, 556, 611, 722, 722, 944, 722, 722, 611, 333, 278, 333, 469, 500, // P-Z [ \\ ] ^ _
    333, 444, 500, 444, 500, 444, 333, 500, 500, 278, 278, 500, 278, 778, 500, 500, // ` a-o
    500, 500, 333, 389, 278, 500, 500, 722, 500, 500, 444, 480, 200, 480, 541, // p-z { | } ~
];

/// Advance of the ASCII characters 32 (space) to 126 (`~`), in thousandths of an em.
const ASCII: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, // space ! " # $ % & ' ( ) * + , - . /
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, // 0-9 : ; < = > ?
    1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, // @ A-O
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, // P-Z [ \ ] ^ _
    333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, // ` a-o
    556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584, // p-z { | } ~
];

/// Width of one character in ems.
pub fn advance_em(c: char, mono: bool) -> f32 {
    advance_face(c, mono, false)
}

/// Width of one character in ems in a fixed-pitch, a sans-serif (`serif` false) or a Times-like face.
pub fn advance_face(c: char, mono: bool, serif: bool) -> f32 {
    if c.is_control() {
        return 0.0;
    }
    if mono {
        return if is_wide(c) { 1.0 } else { 0.6 };
    }
    match c as u32 {
        32..=126 => f32::from(if serif { SERIF[c as usize - 32] } else { ASCII[c as usize - 32] }) / 1000.0,
        _ if is_wide(c) => 1.0,
        // combining marks add no width
        0x300..=0x36f | 0x200b..=0x200f => 0.0,
        _ => 0.56,
    }
}

/// Characters drawn one em wide: CJK, Hangul, kana, full-width forms.
fn is_wide(c: char) -> bool {
    matches!(c as u32, 0x1100..=0x115f | 0x2e80..=0xa4cf | 0xac00..=0xd7a3 | 0xf900..=0xfaff | 0xfe30..=0xfe6f | 0xff00..=0xff60 | 0xffe0..=0xffe6 | 0x1f300..=0x1faff | 0x20000..=0x3fffd)
}

/// Width in ems of `text` after the collapsing of white space (a run of white space is one space).
pub fn text_em(text: &str, mono: bool) -> f32 {
    text_face(text, mono, false)
}

/// [`text_em`] in a chosen face.
pub fn text_face(text: &str, mono: bool, serif: bool) -> f32 {
    let advance_em = |c: char, m: bool| advance_face(c, m, serif);
    let mut w = 0.0;
    let mut prev_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !prev_space {
                w += advance_em(' ', mono);
            }
            prev_space = true;
        } else {
            w += advance_em(c, mono);
            prev_space = false;
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_have_their_own_widths_and_a_fixed_pitch_face_has_one() {
        assert_eq!(advance_em('i', false), 0.222);
        assert_eq!(advance_em('m', false), 0.833);
        assert_eq!(advance_em('W', false), 0.944);
        assert_eq!(advance_em('~', false), 0.584);
        assert_eq!(advance_em('i', true), advance_em('m', true));
        assert_eq!(advance_em('a', true), 0.6);
    }

    #[test]
    fn white_space_collapses_and_wide_scripts_take_an_em() {
        assert_eq!(text_em("a  \n b", false), text_em("a b", false));
        assert_eq!(text_em("日本", false), 2.0);
        assert_eq!(text_em("", false), 0.0);
        assert_eq!(advance_em('\u{301}', false), 0.0);
    }

    #[test]
    fn a_text_of_narrow_letters_is_narrower_than_one_of_wide_letters() {
        assert!(text_em("illicit", false) < text_em("mmmmmmm", false) / 2.0);
    }
}
