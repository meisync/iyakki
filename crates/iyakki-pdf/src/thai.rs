//! Line breaking for Thai, which has no spaces between words: a small in-house dictionary of common words, a segmentation that prefers
//! fewer, longer known words (a shortest path over the allowed cut points), and spelling rules that never cut a letter from the vowel or
//! tone mark that belongs to it. Text the dictionary does not know is cut only between syllable-like clusters, so nothing is ever lost or
//! broken inside a cluster; it just may break at a place a reader would not choose.

use std::collections::HashSet;
use std::sync::OnceLock;

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

fn dict() -> &'static (HashSet<Vec<char>>, usize) {
    static D: OnceLock<(HashSet<Vec<char>>, usize)> = OnceLock::new();
    D.get_or_init(|| {
        let set: HashSet<Vec<char>> = WORDS.split_whitespace().map(|w| w.chars().collect()).collect();
        let max = set.iter().map(|w| w.len()).max().unwrap_or(1);
        (set, max)
    })
}

pub fn is_thai(c: char) -> bool {
    ('\u{0E01}'..='\u{0E5B}').contains(&c)
}

/// Leading vowels: written before the consonant they are read after.
fn leading(c: char) -> bool {
    ('\u{0E40}'..='\u{0E44}').contains(&c)
}

/// Characters that belong to the letter before them (following vowels, above and below marks, tone marks, the repeat mark, the
/// thanthakhat) and so can never start a line.
fn attaches_back(c: char) -> bool {
    matches!(c, '\u{0E30}'..='\u{0E3A}' | '\u{0E45}' | '\u{0E46}' | '\u{0E47}'..='\u{0E4E}' | '\u{0E2F}')
}

/// May a line break between the chars `i - 1` and `i` of a Thai run, by spelling alone?
fn cluster_boundary(s: &[char], i: usize) -> bool {
    if i == 0 || i >= s.len() {
        return true;
    }
    !(leading(s[i - 1]) || attaches_back(s[i]))
}

/// Cut points (as char indices into `run`, a run of Thai letters only) where a line may break, ascending, without 0 and the end.
pub fn run_breaks(run: &[char]) -> Vec<usize> {
    let n = run.len();
    if n < 2 {
        return vec![];
    }
    let (words, maxlen) = dict();
    // best[i]: the cheapest way to cut run[..i]; from[i]: where its last piece starts
    let inf = u32::MAX / 2;
    let mut best = vec![inf; n + 1];
    let mut from = vec![0usize; n + 1];
    best[0] = 0;
    for i in 1..=n {
        if !cluster_boundary(run, i) {
            continue;
        }
        let lo = i.saturating_sub(*maxlen);
        for j in lo..i {
            if best[j] >= inf || !cluster_boundary(run, j) {
                continue;
            }
            let piece = &run[j..i];
            let cost = if words.contains(piece) {
                1
            } else {
                // an unknown piece costs by its size in clusters; one the dictionary does not know should be as small as it can be
                // but never be split into pieces inside a cluster
                let clusters = (j + 1..i).filter(|&k| cluster_boundary(run, k)).count() as u32 + 1;
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
        if best[i] >= inf {
            // no known word and no single cluster ends here only when `i` is inside a cluster; nothing to do
            continue;
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

/// Split a word into pieces a line may end between: Thai runs by the dictionary, and at the edge between Thai and other letters.
pub fn pieces(word: &str) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let thai = is_thai(chars[i]);
        let mut j = i;
        while j < chars.len() && is_thai(chars[j]) == thai {
            j += 1;
        }
        let run = &chars[i..j];
        if thai {
            let mut s = 0;
            for c in run_breaks(run).into_iter().chain(std::iter::once(run.len())) {
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
mod tests {
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
                assert!(!attaches_back(first), "{p} starts with a mark or following vowel in {ps:?}");
                let last = p.chars().last().unwrap();
                assert!(!leading(last) || k + 1 == ps.len(), "{p} ends with a leading vowel in {ps:?}");
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
}
