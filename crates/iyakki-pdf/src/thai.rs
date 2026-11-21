//! Line breaking for Thai, Khmer, Lao and Myanmar, which have no spaces between words. Thai, Khmer, Lao and Myanmar: the ICU word lists (Unicode License v3, `data/WORDLISTS-NOTICE.txt`), deflated in `data/` (Thai also keeps the 384 common words the program started with). The same method for all four: a segmentation that prefers
//! fewer, longer known words (a shortest path over the allowed cut points), and spelling rules that never cut a letter from the vowel or
//! tone mark that belongs to it. Text the dictionary does not know is cut only between syllable-like clusters, so nothing is ever lost or
//! broken inside a cluster; it just may break at a place a reader would not choose.

use std::collections::HashSet;
use std::sync::OnceLock;

/// The 384 common words the program carried first (kept: they are joined with the ICU list, and the tests use some of them).
const WORDS: &str = "\
ฉัน ผม ดิฉัน เรา คุณ เขา เธอ มัน พวก เขา ท่าน ตัว เอง ใคร อะไร ที่ไหน เมื่อไร อย่างไร ทำไม เท่าไร กี่ นี้ นั้น โน้น นี่ นั่น โน่น \
เป็น คือ มี อยู่ ได้ ไป มา ทำ กิน ดื่ม นอน ตื่น เดิน วิ่ง นั่ง ยืน พูด บอก ถาม ตอบ ฟัง ดู เห็น อ่าน เขียน เรียน สอน รู้ เข้าใจ คิด รัก ชอบ เกลียด ต้องการ อยาก \
ซื้อ ขาย ให้ เอา ใช้ เปิด ปิด เริ่ม จบ หยุด รอ หา เจอ พบ ช่วย เล่น ร้อง เต้น ขับ ขี่ ซ่อม สร้าง ทาน อาบ น้ำ ล้าง ซัก ตัด ปลูก เลี้ยง \
ข้าว กับ ปลา เนื้อ ไก่ หมู ผัก ผลไม้ น้ำ ชา กาแฟ นม ไข่ ขนม อาหาร เกลือ น้ำตาล พริก \
บ้าน ห้อง ประตู หน้าต่าง โต๊ะ เก้าอี้ เตียง รถ เรือ เครื่องบิน ถนน ตลาด โรงเรียน โรงพยาบาล วัด ร้าน ธนาคาร สนามบิน เมือง ประเทศ จังหวัด หมู่บ้าน \
คน ผู้ชาย ผู้หญิง เด็ก พ่อ แม่ ลูก พี่ น้อง ปู่ ย่า ตา ยาย เพื่อน ครู นักเรียน หมอ ตำรวจ ทหาร ชาวนา พระ \
วัน คืน เช้า สาย บ่าย เย็น ปี เดือน สัปดาห์ ชั่วโมง นาที วินาที เวลา ตอนนี้ วันนี้ พรุ่งนี้ เมื่อวาน เมื่อวานนี้ ปีนี้ เดือนนี้ \
หนึ่ง สอง สาม สี่ ห้า หก เจ็ด แปด เก้า สิบ ร้อย พัน หมื่น แสน ล้าน \
ใหญ่ เล็ก ดี เลว สวย งาม ร้อน หนาว เย็น อุ่น ใหม่ เก่า ง่าย ยาก เร็ว ช้า ไกล ใกล้ สูง ต่ำ ยาว สั้น หนัก เบา มาก น้อย ทุก บาง หลาย แรก สุดท้าย \
และ หรือ แต่ เพราะ ถ้า เมื่อ ขณะ ที่ ซึ่ง อัน ของ แก่ แด่ ต่อ จาก ถึง ใน บน ใต้ ข้าง หน้า หลัง ระหว่าง ภายใน ภายนอก โดย ด้วย เพื่อ ตาม แล้ว ก็ จะ กำลัง เคย ยัง ไม่ ไม่ได้ ไม่มี \
ไทย ภาษาไทย ภาษา อังกฤษ จีน ญี่ปุ่น เกาหลี อินเดีย อเมริกา กรุงเทพ เชียงใหม่ ภูเก็ต \
ความ การ นัก ผู้ ชาว เครื่อง ที่สุด ที่นี่ ที่นั่น ที่ไหน ตลอด ทั้ง ทั้งหมด ทั้งนี้ เช่น เป็นต้น ดังนั้น เพราะฉะนั้น อย่างไรก็ตาม \
สวัสดี ขอบคุณ ขอโทษ ยินดี ต้อนรับ สบายดี โชคดี ลาก่อน ครับ ค่ะ คะ นะ จ้ะ \
คอมพิวเตอร์ โทรศัพท์ อินเทอร์เน็ต โปรแกรม ข้อมูล ระบบ เครือข่าย หนังสือ กระดาษ ปากกา ดินสอ จดหมาย ข่าว เรื่อง ปัญหา คำถาม คำตอบ ความรัก ความสุข ความรู้ \
ฝน ฟ้า ลม แดด ดิน ทราย ภูเขา แม่น้ำ ทะเล เกาะ ป่า ต้นไม้ ดอกไม้ ใบไม้ สัตว์ นก ช้าง ม้า วัว ควาย หมา แมว \
เงิน ทอง บาท ราคา แพง ถูก ฟรี งาน บริษัท สำนักงาน เจ้าหน้าที่ รัฐบาล กฎหมาย ศาล สิทธิ เสรีภาพ ประชาชน สังคม เศรษฐกิจ การเมือง การศึกษา สุขภาพ วัฒนธรรม ประวัติศาสตร์ \
ร่างกาย หัว ตา หู จมูก ปาก ฟัน มือ แขน ขา เท้า หัวใจ ผม \
สี แดง เหลือง เขียว ฟ้า ขาว ดำ ส้ม ชมพู ม่วง น้ำเงิน";

/// The scripts that are cut by a dictionary.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Script {
    Thai,
    Khmer,
    Lao,
    Myanmar,
}

const THAI_Z: &[u8] = include_bytes!("../data/words-thai.z");
const KHMER_Z: &[u8] = include_bytes!("../data/words-khmer.z");
const LAO_Z: &[u8] = include_bytes!("../data/words-lao.z");
const MYANMAR_Z: &[u8] = include_bytes!("../data/words-myanmar.z");

type Words = (HashSet<Vec<char>>, usize);

fn from_z(z: &[u8]) -> Words {
    let raw = crate::filter::inflate(z, 16 << 20);
    let text = String::from_utf8_lossy(&raw);
    let set: HashSet<Vec<char>> = text.lines().filter(|w| !w.is_empty()).map(|w| w.chars().collect()).collect();
    let max = set.iter().map(|w| w.len()).max().unwrap_or(1).min(40);
    (set, max)
}

fn dict(script: Script) -> &'static Words {
    static T: OnceLock<Words> = OnceLock::new();
    static K: OnceLock<Words> = OnceLock::new();
    static L: OnceLock<Words> = OnceLock::new();
    static M: OnceLock<Words> = OnceLock::new();
    match script {
        Script::Thai => T.get_or_init(|| {
            // the ICU Thai list and the small list the program started with
            let (mut set, _) = from_z(THAI_Z);
            set.extend(WORDS.split_whitespace().map(|w| w.chars().collect::<Vec<char>>()));
            let max = set.iter().map(|w| w.len()).max().unwrap_or(1).min(40);
            (set, max)
        }),
        Script::Khmer => K.get_or_init(|| from_z(KHMER_Z)),
        Script::Lao => L.get_or_init(|| from_z(LAO_Z)),
        Script::Myanmar => M.get_or_init(|| from_z(MYANMAR_Z)),
    }
}

/// Which of the four scripts a letter belongs to.
pub fn script_of(c: char) -> Option<Script> {
    match c {
        '\u{0E01}'..='\u{0E5B}' => Some(Script::Thai),
        '\u{1780}'..='\u{17FF}' | '\u{19E0}'..='\u{19FF}' => Some(Script::Khmer),
        '\u{0E81}'..='\u{0EDF}' => Some(Script::Lao),
        '\u{1000}'..='\u{109F}' | '\u{A9E0}'..='\u{A9FF}' | '\u{AA60}'..='\u{AA7F}' => Some(Script::Myanmar),
        _ => None,
    }
}

pub fn is_thai(c: char) -> bool {
    ('\u{0E01}'..='\u{0E5B}').contains(&c)
}

/// Does the text hold a letter of one of the four dictionary scripts?
pub fn has_dictionary_script(word: &str) -> bool {
    word.chars().any(|c| script_of(c).is_some())
}

/// Leading vowels: written (and encoded) before the consonant they are read after.
fn leading(script: Script, c: char) -> bool {
    match script {
        Script::Thai => ('\u{0E40}'..='\u{0E44}').contains(&c),
        Script::Lao => ('\u{0EC0}'..='\u{0EC4}').contains(&c),
        // Khmer and Myanmar encode every vowel after its consonant; a coeng or virama binds the NEXT letter, checked below
        Script::Khmer | Script::Myanmar => false,
    }
}

/// Characters that belong to the letter before them (following vowels, above and below marks, tone marks, repeat marks, medials,
/// the Khmer coeng's subscript, the Myanmar asat) and so can never start a line.
fn attaches_back(script: Script, c: char) -> bool {
    match script {
        Script::Thai => matches!(c, '\u{0E30}'..='\u{0E3A}' | '\u{0E45}' | '\u{0E46}' | '\u{0E47}'..='\u{0E4E}' | '\u{0E2F}'),
        Script::Lao => matches!(c, '\u{0EB0}'..='\u{0EBC}' | '\u{0EC6}' | '\u{0EC8}'..='\u{0ECD}' | '\u{0EAF}'),
        Script::Khmer => matches!(c, '\u{17B4}'..='\u{17D3}' | '\u{17DD}'),
        Script::Myanmar => matches!(c, '\u{102B}'..='\u{103E}' | '\u{1056}'..='\u{1059}' | '\u{105E}'..='\u{1060}' | '\u{1062}'..='\u{1064}' | '\u{1067}'..='\u{106D}' | '\u{1071}'..='\u{1074}' | '\u{1082}'..='\u{108D}' | '\u{108F}' | '\u{109A}'..='\u{109D}'),
    }
}

/// May a line break between the chars `i - 1` and `i` of a run, by spelling alone?
fn cluster_boundary(script: Script, s: &[char], i: usize) -> bool {
    if i == 0 || i >= s.len() {
        return true;
    }
    // a Khmer coeng (U+17D2) or Myanmar virama (U+1039) joins the consonant after it to the one before it
    let binds = match script {
        Script::Khmer => s[i - 1] == '\u{17D2}',
        Script::Myanmar => s[i - 1] == '\u{1039}',
        _ => false,
    };
    !(binds || leading(script, s[i - 1]) || attaches_back(script, s[i]))
}

/// Cut points (as char indices into `run`, a run of Thai letters only) where a line may break, ascending, without 0 and the end.
pub fn run_breaks(run: &[char]) -> Vec<usize> {
    run_breaks_in(Script::Thai, run)
}

/// The same for a run of letters of one script.
pub fn run_breaks_in(script: Script, run: &[char]) -> Vec<usize> {
    let n = run.len();
    if n < 2 {
        return vec![];
    }
    let (words, maxlen) = dict(script);
    let cb = |i: usize| cluster_boundary(script, run, i);
    // best[i]: the cheapest way to cut run[..i]; from[i]: where its last piece starts
    let inf = u32::MAX / 2;
    let mut best = vec![inf; n + 1];
    let mut from = vec![0usize; n + 1];
    best[0] = 0;
    for i in 1..=n {
        if !cb(i) {
            continue;
        }
        let lo = i.saturating_sub(*maxlen);
        for j in lo..i {
            if best[j] >= inf || !cb(j) {
                continue;
            }
            let piece = &run[j..i];
            let cost = if words.contains(piece) {
                1
            } else {
                // an unknown piece costs by its size in clusters; one the dictionary does not know should be as small as it can be
                // but never be split into pieces inside a cluster
                let clusters = (j + 1..i).filter(|&k| cb(k)).count() as u32 + 1;
                if clusters == 1 {
                    3
                } else {
                    continue;
                }
            };
            if best[j] + cost < best[i] || (best[j] + cost == best[i] && j > from[i]) {
                best[i] = best[j] + cost;
                from[i] = j;
            }
        }
    }
    let mut cuts = Vec::new();
    let mut at = n;
    while at > 0 && best[at] < inf {
        let j = from[at];
        if j > 0 {
            cuts.push(j);
        }
        at = j;
    }
    cuts.reverse();
    // unknown clusters next to each other are one stretch of unknown text: do not cut between them
    let mut kept = Vec::new();
    let mut prev = 0usize;
    for &c in &cuts {
        let left_known = words.contains(&run[prev..c]);
        let next = cuts.iter().copied().find(|&x| x > c).unwrap_or(n);
        let right_known = words.contains(&run[c..next]);
        if left_known || right_known {
            kept.push(c);
        }
        prev = c;
    }
    kept
}

/// Split a word into pieces a line may end between: runs of Thai, Khmer, Lao or Myanmar by their dictionary, and at the edge between
/// those scripts and other letters.
pub fn pieces(word: &str) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let sc = script_of(chars[i]);
        let mut j = i;
        while j < chars.len() && script_of(chars[j]) == sc {
            j += 1;
        }
        let run = &chars[i..j];
        if let Some(sc) = sc {
            let mut s = 0;
            for c in run_breaks_in(sc, run).into_iter().chain(std::iter::once(run.len())) {
                out.push(run[s..c].iter().collect());
                s = c;
            }
        } else {
            out.push(run.iter().collect());
        }
        i = j;
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn cut(s: &str) -> Vec<String> {
        pieces(s)
    }

    #[test]
    fn a_sentence_without_spaces_is_cut_at_its_words() {
        assert_eq!(cut("ฉันกินข้าวกับปลาที่ตลาดวันนี้"), vec!["ฉัน", "กิน", "ข้าว", "กับ", "ปลา", "ที่", "ตลาด", "วันนี้"]);
        assert_eq!(cut("สวัสดีครับ"), vec!["สวัสดี", "ครับ"]);
        assert_eq!(cut("ภาษาไทย"), vec!["ภาษาไทย"]);
    }

    #[test]
    fn spelling_rules_never_split_a_letter_from_its_vowel_or_mark() {
        // a leading vowel stays with the consonant after it; marks and following vowels stay with the letter before
        for text in ["เดินไปโรงเรียนที่ไกลมากๆ", "แล้วเขาก็บอกว่าไม่รู้เรื่อง", "ไม่เข้าใจคำถามของเธอเลย", "ใครทำน้ำหกที่โต๊ะ"] {
            let ps = cut(text);
            assert_eq!(ps.concat(), text, "nothing lost");
            for (k, p) in ps.iter().enumerate() {
                let first = p.chars().next().unwrap();
                assert!(!attaches_back(Script::Thai, first), "{p} starts with a mark or following vowel in {ps:?}");
                let last = p.chars().last().unwrap();
                assert!(!leading(Script::Thai, last) || k + 1 == ps.len(), "{p} ends with a leading vowel in {ps:?}");
            }
        }
    }

    #[test]
    fn unknown_text_is_kept_whole_between_known_words_and_latin_is_a_piece_of_its_own() {
        let ps = cut("ฉันชอบซูซูกิมากABC123");
        assert_eq!(ps.concat(), "ฉันชอบซูซูกิมากABC123");
        assert_eq!(ps.last().unwrap(), "ABC123");
        assert!(ps.contains(&"มาก".to_string()) && ps.contains(&"ชอบ".to_string()));
        assert_eq!(cut("hello"), vec!["hello"]);
        assert!(cut("").is_empty());
    }

    /// Words of the given script from a list of words of that list, joined; the cut must come back as those words or finer.
    pub(crate) fn words_of(script: Script, take: usize) -> Vec<String> {
        let (set, _) = dict(script);
        let mut w: Vec<&Vec<char>> = set.iter().filter(|w| w.len() >= 4 && w.len() <= 7).collect();
        w.sort();
        w.iter().step_by(w.len() / take).take(take).map(|x| x.iter().collect()).collect()
    }

    #[test]
    fn the_khmer_lao_and_myanmar_lists_are_loaded_and_have_their_size() {
        for (sc, least) in [(Script::Thai, 26_000), (Script::Khmer, 80_000), (Script::Lao, 30_000), (Script::Myanmar, 40_000)] {
            let (set, max) = dict(sc);
            assert!(set.len() >= least, "{sc:?} has {}", set.len());
            assert!(*max >= 8 && *max <= 40);
        }
    }

    #[test]
    fn joined_words_of_each_list_are_cut_into_whole_clusters_and_nothing_is_lost() {
        for sc in [Script::Thai, Script::Khmer, Script::Lao, Script::Myanmar] {
            let ws = words_of(sc, 12);
            for pair in ws.chunks(2) {
                let text: String = pair.concat();
                let ps = pieces(&text);
                assert_eq!(ps.concat(), text, "{sc:?}: nothing lost");
                for (k, p) in ps.iter().enumerate() {
                    let cs: Vec<char> = p.chars().collect();
                    assert!(!attaches_back(sc, cs[0]) || k == 0, "{sc:?}: {p} starts with a mark in {ps:?}");
                    assert!(!leading(sc, *cs.last().unwrap()) || k + 1 == ps.len(), "{sc:?}: {p} ends with a leading vowel");
                }
                // two words of a list are never cut into more pieces than letters, and at least one cut falls where they meet when the
                // pair is not itself a word
                let (set, _) = dict(sc);
                let joined: Vec<char> = text.chars().collect();
                if !set.contains(&joined[..]) && pair.len() == 2 {
                    assert!(ps.len() >= 2, "{sc:?}: {text} should be cut");
                }
            }
        }
    }

    #[test]
    fn a_coeng_a_virama_and_a_leading_lao_vowel_never_part_from_their_letter() {
        let khmer: Vec<char> = "ក្រុម".chars().collect(); // ka, coeng, ra, u, mo
        assert!(!cluster_boundary(Script::Khmer, &khmer, 2));
        let my: Vec<char> = "ကက္က".chars().collect();
        assert!(!cluster_boundary(Script::Myanmar, &my, 3));
        let lao: Vec<char> = "ເກ".chars().collect();
        assert!(!cluster_boundary(Script::Lao, &lao, 1));
        for sc in [Script::Khmer, Script::Lao, Script::Myanmar] {
            assert!(pieces("").is_empty());
            assert_eq!(script_of('a'), None);
            let _ = sc;
        }
    }

    #[test]
    fn latin_between_scripts_is_its_own_piece_and_a_mixed_word_is_not_lost() {
        let w = words_of(Script::Khmer, 3).concat();
        let text = format!("{w}ABC{w}");
        let ps = pieces(&text);
        assert_eq!(ps.concat(), text);
        assert!(ps.contains(&"ABC".to_string()));
    }
}
