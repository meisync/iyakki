//! Numbers as words, per language, for the Indic text frontend.
//!
//! The Indic-TTS models read characters, and their character sets hold at most the digit `0` (Tamil) or none, so every digit
//! must be spoken as words before the text reaches the model. Whole numbers up to 99,99,99,999 are spelt in Tamil, Telugu,
//! Kannada, Malayalam, Hindi and English (Indian system: thousand, lakh, crore; English: thousand, million, billion). Other
//! languages, and anything longer, are read digit by digit with that language's digit names where there are names for it.
//!
//! These tables were written from published number-name lists, not checked by a native speaker of each language: see
//! docs/VOICE.md ("What was verified"). Compound forms follow the plain reading (`tens + units`), which is understood everywhere
//! even where careful speech joins the words (sandhi).

struct Tbl {
    ones: [&'static str; 10],
    teens: [&'static str; 10],
    tens: [&'static str; 8],
    /// Form of the tens word before a unit (`இருபத்தி ஒன்று`); the plain tens word when the same.
    tens_comb: [&'static str; 8],
    /// Hundred words for 1..=9 when nothing follows, and when something follows.
    hund_alone: [&'static str; 10],
    hund_comb: [&'static str; 10],
    /// thousand, lakh, crore: word standing alone / before more words / the bare word for exactly one (empty: say `one` first).
    scales: [Scale; 3],
    one_prefix: &'static str,
}

struct Scale {
    word: &'static str,
    comb: &'static str,
    bare: &'static str,
}

const fn sc(word: &'static str, comb: &'static str, bare: &'static str) -> Scale {
    Scale { word, comb, bare }
}

const TA: Tbl = Tbl {
    ones: ["பூஜ்யம்", "ஒன்று", "இரண்டு", "மூன்று", "நான்கு", "ஐந்து", "ஆறு", "ஏழு", "எட்டு", "ஒன்பது"],
    teens: ["பத்து", "பதினொன்று", "பன்னிரண்டு", "பதின்மூன்று", "பதினான்கு", "பதினைந்து", "பதினாறு", "பதினேழு", "பதினெட்டு", "பத்தொன்பது"],
    tens: ["இருபது", "முப்பது", "நாற்பது", "ஐம்பது", "அறுபது", "எழுபது", "எண்பது", "தொண்ணூறு"],
    tens_comb: ["இருபத்தி", "முப்பத்தி", "நாற்பத்தி", "ஐம்பத்தி", "அறுபத்தி", "எழுபத்தி", "எண்பத்தி", "தொண்ணூற்று"],
    hund_alone: ["", "நூறு", "இருநூறு", "முந்நூறு", "நானூறு", "ஐந்நூறு", "அறுநூறு", "எழுநூறு", "எண்ணூறு", "தொள்ளாயிரம்"],
    hund_comb: ["", "நூற்று", "இருநூற்று", "முந்நூற்று", "நானூற்று", "ஐந்நூற்று", "அறுநூற்று", "எழுநூற்று", "எண்ணூற்று", "தொள்ளாயிரத்து"],
    scales: [sc("ஆயிரம்", "ஆயிரத்து", "ஆயிரம்"), sc("இலட்சம்", "இலட்சத்து", ""), sc("கோடி", "கோடியே", "")],
    one_prefix: "ஒரு",
};

const TE: Tbl = Tbl {
    ones: ["సున్నా", "ఒకటి", "రెండు", "మూడు", "నాలుగు", "ఐదు", "ఆరు", "ఏడు", "ఎనిమిది", "తొమ్మిది"],
    teens: ["పది", "పదకొండు", "పన్నెండు", "పదమూడు", "పద్నాలుగు", "పదిహేను", "పదహారు", "పదిహేడు", "పద్దెనిమిది", "పంతొమ్మిది"],
    tens: ["ఇరవై", "ముప్పై", "నలభై", "యాభై", "అరవై", "డెబ్బై", "ఎనభై", "తొంభై"],
    tens_comb: ["ఇరవై", "ముప్పై", "నలభై", "యాభై", "అరవై", "డెబ్బై", "ఎనభై", "తొంభై"],
    hund_alone: ["", "వంద", "రెండు వందలు", "మూడు వందలు", "నాలుగు వందలు", "ఐదు వందలు", "ఆరు వందలు", "ఏడు వందలు", "ఎనిమిది వందలు", "తొమ్మిది వందలు"],
    hund_comb: ["", "వంద", "రెండు వందల", "మూడు వందల", "నాలుగు వందల", "ఐదు వందల", "ఆరు వందల", "ఏడు వందల", "ఎనిమిది వందల", "తొమ్మిది వందల"],
    scales: [sc("వేలు", "వేల", "వెయ్యి"), sc("లక్షలు", "లక్షల", ""), sc("కోట్లు", "కోట్ల", "")],
    one_prefix: "ఒక",
};

const KN: Tbl = Tbl {
    ones: ["ಸೊನ್ನೆ", "ಒಂದು", "ಎರಡು", "ಮೂರು", "ನಾಲ್ಕು", "ಐದು", "ಆರು", "ಏಳು", "ಎಂಟು", "ಒಂಬತ್ತು"],
    teens: ["ಹತ್ತು", "ಹನ್ನೊಂದು", "ಹನ್ನೆರಡು", "ಹದಿಮೂರು", "ಹದಿನಾಲ್ಕು", "ಹದಿನೈದು", "ಹದಿನಾರು", "ಹದಿನೇಳು", "ಹದಿನೆಂಟು", "ಹತ್ತೊಂಬತ್ತು"],
    tens: ["ಇಪ್ಪತ್ತು", "ಮೂವತ್ತು", "ನಲವತ್ತು", "ಐವತ್ತು", "ಅರವತ್ತು", "ಎಪ್ಪತ್ತು", "ಎಂಬತ್ತು", "ತೊಂಬತ್ತು"],
    tens_comb: ["ಇಪ್ಪತ್ತು", "ಮೂವತ್ತು", "ನಲವತ್ತು", "ಐವತ್ತು", "ಅರವತ್ತು", "ಎಪ್ಪತ್ತು", "ಎಂಬತ್ತು", "ತೊಂಬತ್ತು"],
    hund_alone: ["", "ನೂರು", "ಇನ್ನೂರು", "ಮುನ್ನೂರು", "ನಾನೂರು", "ಐನೂರು", "ಆರುನೂರು", "ಏಳುನೂರು", "ಎಂಟುನೂರು", "ಒಂಬೈನೂರು"],
    hund_comb: ["", "ನೂರ", "ಇನ್ನೂರ", "ಮುನ್ನೂರ", "ನಾನೂರ", "ಐನೂರ", "ಆರುನೂರ", "ಏಳುನೂರ", "ಎಂಟುನೂರ", "ಒಂಬೈನೂರ"],
    scales: [sc("ಸಾವಿರ", "ಸಾವಿರದ", ""), sc("ಲಕ್ಷ", "ಲಕ್ಷದ", ""), sc("ಕೋಟಿ", "ಕೋಟಿಯ", "")],
    one_prefix: "ಒಂದು",
};

const ML: Tbl = Tbl {
    ones: ["പൂജ്യം", "ഒന്ന്", "രണ്ട്", "മൂന്ന്", "നാല്", "അഞ്ച്", "ആറ്", "ഏഴ്", "എട്ട്", "ഒമ്പത്"],
    teens: ["പത്ത്", "പതിനൊന്ന്", "പന്ത്രണ്ട്", "പതിമൂന്ന്", "പതിനാല്", "പതിനഞ്ച്", "പതിനാറ്", "പതിനേഴ്", "പതിനെട്ട്", "പത്തൊമ്പത്"],
    tens: ["ഇരുപത്", "മുപ്പത്", "നാല്പത്", "അമ്പത്", "അറുപത്", "എഴുപത്", "എൺപത്", "തൊണ്ണൂറ്"],
    tens_comb: ["ഇരുപത്തി", "മുപ്പത്തി", "നാല്പത്തി", "അമ്പത്തി", "അറുപത്തി", "എഴുപത്തി", "എൺപത്തി", "തൊണ്ണൂറ്റി"],
    hund_alone: ["", "നൂറ്", "ഇരുന്നൂറ്", "മുന്നൂറ്", "നാനൂറ്", "അഞ്ഞൂറ്", "അറുനൂറ്", "എഴുനൂറ്", "എണ്ണൂറ്", "തൊള്ളായിരം"],
    hund_comb: ["", "നൂറ്റി", "ഇരുന്നൂറ്റി", "മുന്നൂറ്റി", "നാനൂറ്റി", "അഞ്ഞൂറ്റി", "അറുനൂറ്റി", "എഴുനൂറ്റി", "എണ്ണൂറ്റി", "തൊള്ളായിരത്തി"],
    scales: [sc("ആയിരം", "ആയിരത്തി", "ആയിരം"), sc("ലക്ഷം", "ലക്ഷത്തി", ""), sc("കോടി", "കോടി", "")],
    one_prefix: "ഒരു",
};

/// Hindi: only the scale words are used from the table (the numbers below 100 are irregular and listed in full).
const HI: Tbl = Tbl { scales: [sc("हज़ार", "हज़ार", ""), sc("लाख", "लाख", ""), sc("करोड़", "करोड़", "")], one_prefix: "एक", ..TA };

const HI_BELOW_100: [&str; 100] = [
    "शून्य", "एक", "दो", "तीन", "चार", "पाँच", "छह", "सात", "आठ", "नौ", "दस", "ग्यारह", "बारह", "तेरह", "चौदह", "पंद्रह", "सोलह", "सत्रह", "अठारह", "उन्नीस", "बीस", "इक्कीस", "बाईस", "तेईस", "चौबीस", "पच्चीस", "छब्बीस",
    "सत्ताईस", "अट्ठाईस", "उनतीस", "तीस", "इकतीस", "बत्तीस", "तैंतीस", "चौंतीस", "पैंतीस", "छत्तीस", "सैंतीस", "अड़तीस", "उनतालीस", "चालीस", "इकतालीस", "बयालीस", "तैंतालीस", "चौवालीस", "पैंतालीस", "छियालीस", "सैंतालीस",
    "अड़तालीस", "उनचास", "पचास", "इक्यावन", "बावन", "तिरपन", "चौवन", "पचपन", "छप्पन", "सत्तावन", "अट्ठावन", "उनसठ", "साठ", "इकसठ", "बासठ", "तिरसठ", "चौंसठ", "पैंसठ", "छियासठ", "सड़सठ", "अड़सठ", "उनहत्तर", "सत्तर",
    "इकहत्तर", "बहत्तर", "तिहत्तर", "चौहत्तर", "पचहत्तर", "छिहत्तर", "सतहत्तर", "अठहत्तर", "उनासी", "अस्सी", "इक्यासी", "बयासी", "तिरासी", "चौरासी", "पचासी", "छियासी", "सत्तासी", "अट्ठासी", "नवासी", "नब्बे", "इक्यानवे",
    "बानवे", "तिरानवे", "चौरानवे", "पचानवे", "छियानवे", "सत्तानवे", "अट्ठानवे", "निन्यानवे",
];

const EN_ONES: [&str; 20] = ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen"];
const EN_TENS: [&str; 8] = ["twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];

fn below_100(t: &Tbl, n: u64) -> String {
    let n = n as usize;
    if n < 10 {
        t.ones[n].to_string()
    } else if n < 20 {
        t.teens[n - 10].to_string()
    } else if n % 10 == 0 {
        t.tens[n / 10 - 2].to_string()
    } else {
        format!("{} {}", t.tens_comb[n / 10 - 2], t.ones[n % 10])
    }
}

fn indian(t: &Tbl, n: u64, hindi: bool) -> String {
    if n == 0 {
        return if hindi { HI_BELOW_100[0].into() } else { t.ones[0].into() };
    }
    let b100 = |v: u64| if hindi { HI_BELOW_100[v as usize].to_string() } else { below_100(t, v) };
    let mut parts: Vec<String> = Vec::new();
    // crore, lakh, thousand as (value, scale index, size)
    for (v, si, size) in [(n / 10_000_000, 2usize, 10_000_000u64), ((n / 100_000) % 100, 1, 100_000), ((n / 1000) % 100, 0, 1000)] {
        if v == 0 {
            continue;
        }
        let more = n % size != 0;
        let s = &t.scales[si];
        let word = if more { s.comb } else { s.word };
        let head = if v == 1 {
            if s.bare.is_empty() {
                format!("{} {word}", t.one_prefix)
            } else if more {
                s.comb.to_string()
            } else {
                s.bare.to_string()
            }
        } else if si == 2 && v >= 100 {
            // more than 99 crore: crore of a larger number (kept simple: count in crores)
            format!("{} {word}", indian(t, v, hindi))
        } else {
            format!("{} {word}", b100(v))
        };
        parts.push(head);
    }
    let h = (n / 100) % 10;
    let rest = n % 100;
    if h > 0 {
        if hindi {
            parts.push(format!("{} सौ", HI_BELOW_100[h as usize]));
        } else if rest > 0 {
            parts.push(t.hund_comb[h as usize].to_string());
        } else {
            parts.push(t.hund_alone[h as usize].to_string());
        }
    }
    if rest > 0 {
        parts.push(b100(rest));
    }
    parts.join(" ")
}

fn english(n: u64) -> String {
    fn below_1000(n: u64) -> String {
        let mut s = Vec::new();
        if n >= 100 {
            s.push(format!("{} hundred", EN_ONES[(n / 100) as usize]));
        }
        let r = n % 100;
        if r > 0 {
            s.push(if r < 20 { EN_ONES[r as usize].to_string() } else if r % 10 == 0 { EN_TENS[(r / 10 - 2) as usize].to_string() } else { format!("{} {}", EN_TENS[(r / 10 - 2) as usize], EN_ONES[(r % 10) as usize]) });
        }
        s.join(" ")
    }
    if n == 0 {
        return "zero".into();
    }
    let mut parts = Vec::new();
    for (size, name) in [(1_000_000_000u64, "billion"), (1_000_000, "million"), (1000, "thousand")] {
        let v = (n / size) % 1000;
        if v > 0 {
            parts.push(format!("{} {name}", below_1000(v)));
        }
    }
    if n % 1000 > 0 {
        parts.push(below_1000(n % 1000));
    }
    parts.join(" ")
}

/// Largest number spelt out; longer digit strings are read digit by digit.
pub const MAX_SPELLED: u64 = 99_99_99_999;

/// The digit names of a language (0..=9), for reading a number digit by digit.
pub fn digit_names(lang: &str) -> Option<[&'static str; 10]> {
    Some(match lang {
        "ta" => TA.ones,
        "te" => TE.ones,
        "kn" => KN.ones,
        "ml" => ML.ones,
        "hi" => [HI_BELOW_100[0], HI_BELOW_100[1], HI_BELOW_100[2], HI_BELOW_100[3], HI_BELOW_100[4], HI_BELOW_100[5], HI_BELOW_100[6], HI_BELOW_100[7], HI_BELOW_100[8], HI_BELOW_100[9]],
        "en" => ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine"],
        "bn" | "as" => ["শূন্য", "এক", "দুই", "তিন", "চার", "পাঁচ", "ছয়", "সাত", "আট", "নয়"],
        "mr" => ["शून्य", "एक", "दोन", "तीन", "चार", "पाच", "सहा", "सात", "आठ", "नऊ"],
        "gu" => ["શૂન્ય", "એક", "બે", "ત્રણ", "ચાર", "પાંચ", "છ", "સાત", "આઠ", "નવ"],
        "pa" => ["ਸਿਫ਼ਰ", "ਇੱਕ", "ਦੋ", "ਤਿੰਨ", "ਚਾਰ", "ਪੰਜ", "ਛੇ", "ਸੱਤ", "ਅੱਠ", "ਨੌਂ"],
        "or" => ["ଶୂନ୍ୟ", "ଏକ", "ଦୁଇ", "ତିନି", "ଚାରି", "ପାଞ୍ଚ", "ଛଅ", "ସାତ", "ଆଠ", "ନଅ"],
        "brx" | "raj" => ["शून्य", "एक", "दो", "तीन", "चार", "पाँच", "छह", "सात", "आठ", "नौ"],
        _ => return None,
    })
}

/// Does this language spell whole numbers (not only digit by digit)?
pub fn spells_numbers(lang: &str) -> bool {
    matches!(lang, "ta" | "te" | "kn" | "ml" | "hi" | "en")
}

/// A whole number in words, or None when this language only reads digits.
pub fn number_words(n: u64, lang: &str) -> Option<String> {
    if n > MAX_SPELLED {
        return None;
    }
    Some(match lang {
        "ta" => indian(&TA, n, false),
        "te" => indian(&TE, n, false),
        "kn" => indian(&KN, n, false),
        "ml" => indian(&ML, n, false),
        "hi" => indian(&HI, n, true),
        "en" => english(n),
        _ => return None,
    })
}

/// A string of ASCII digits as words: spelt as a number when the language can (and it has no leading zero), else digit by digit.
pub fn digits_to_words(digits: &str, lang: &str) -> String {
    let digits: String = digits.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return String::new();
    }
    if digits.len() <= 10 && (digits == "0" || !digits.starts_with('0')) {
        if let Ok(n) = digits.parse::<u64>() {
            if let Some(w) = number_words(n, lang) {
                return w;
            }
        }
    }
    match digit_names(lang) {
        Some(names) => digits.chars().map(|c| names[c.to_digit(10).unwrap_or(0) as usize]).collect::<Vec<_>>().join(" "),
        None => String::new(),
    }
}

/// The word for the decimal point.
pub fn point_word(lang: &str) -> &'static str {
    match lang {
        "ta" => "புள்ளி",
        "te" => "పాయింట్",
        "kn" => "ಪಾಯಿಂಟ್",
        "ml" => "പോയിന്റ്",
        "hi" => "दशमलव",
        "bn" => "দশমিক",
        "mr" => "पूर्णांक",
        _ => "point",
    }
}

/// The word for `%`.
pub fn percent_word(lang: &str) -> &'static str {
    match lang {
        "ta" => "சதவீதம்",
        "te" => "శాతం",
        "kn" => "ಶೇಕಡಾ",
        "ml" => "ശതമാനം",
        "hi" => "प्रतिशत",
        "bn" => "শতাংশ",
        "mr" => "टक्के",
        _ => "percent",
    }
}

/// The word for the rupee sign.
pub fn rupee_word(lang: &str) -> &'static str {
    match lang {
        "ta" => "ரூபாய்",
        "te" => "రూపాయలు",
        "kn" => "ರೂಪಾಯಿ",
        "ml" => "രൂപ",
        "hi" | "mr" => "रुपये",
        "bn" => "টাকা",
        _ => "rupees",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tamil_numbers_are_spelt() {
        assert_eq!(number_words(0, "ta").unwrap(), "பூஜ்யம்");
        assert_eq!(number_words(7, "ta").unwrap(), "ஏழு");
        assert_eq!(number_words(25, "ta").unwrap(), "இருபத்தி ஐந்து");
        assert_eq!(number_words(100, "ta").unwrap(), "நூறு");
        assert_eq!(number_words(121, "ta").unwrap(), "நூற்று இருபத்தி ஒன்று");
        assert_eq!(number_words(1000, "ta").unwrap(), "ஆயிரம்");
        assert_eq!(number_words(2024, "ta").unwrap(), "இரண்டு ஆயிரத்து இருபத்தி நான்கு");
        assert_eq!(number_words(100000, "ta").unwrap(), "ஒரு இலட்சம்");
        assert_eq!(number_words(10_000_000, "ta").unwrap(), "ஒரு கோடி");
    }

    #[test]
    fn hindi_and_english_numbers() {
        assert_eq!(number_words(25, "hi").unwrap(), "पच्चीस");
        assert_eq!(number_words(99, "hi").unwrap(), "निन्यानवे");
        assert_eq!(number_words(350, "hi").unwrap(), "तीन सौ पचास");
        assert_eq!(number_words(100000, "hi").unwrap(), "एक लाख");
        assert_eq!(number_words(2024, "en").unwrap(), "two thousand twenty four");
        assert_eq!(number_words(1_000_005, "en").unwrap(), "one million five");
    }

    #[test]
    fn every_table_entry_is_in_its_own_script_and_none_is_empty() {
        for (lang, lo, hi) in [("ta", 0x0B80u32, 0x0BFFu32), ("te", 0x0C00, 0x0C7F), ("kn", 0x0C80, 0x0CFF), ("ml", 0x0D00, 0x0D7F), ("hi", 0x0900, 0x097F)] {
            for n in (0..=120).chain([999, 1000, 1001, 12_345, 99_999, 100_000, 1_234_567, 10_000_000, 12_34_56_789, MAX_SPELLED]) {
                let w = number_words(n, lang).unwrap();
                assert!(!w.trim().is_empty(), "{lang} {n}");
                assert!(w.chars().all(|c| c == ' ' || (lo..=hi).contains(&(c as u32)) || c == '\u{200d}' || c == '\u{200c}'), "{lang} {n}: {w}");
            }
        }
    }

    #[test]
    fn long_or_unsupported_numbers_read_digit_by_digit() {
        assert_eq!(number_words(MAX_SPELLED + 1, "ta"), None);
        assert_eq!(digits_to_words("007", "ta"), "பூஜ்யம் பூஜ்யம் ஏழு");
        assert_eq!(digits_to_words("12345678901234", "en"), "one two three four five six seven eight nine zero one two three four");
        assert_eq!(digits_to_words("35", "bn"), "তিন পাঁচ", "no number table for Bengali: digits");
        assert_eq!(digits_to_words("5", "xx"), "");
    }
}
