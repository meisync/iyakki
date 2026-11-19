//! Passport and visa photo rules by country: the one table the CAMERA app, its passport mode and the assistant's
//! `camera_passport` tool read. Each entry names its official source and the day it was checked; an entry whose numbers
//! could not be confirmed on an official page is marked unverified and the app says so instead of pretending.
//!
//! Sizes are in millimetres (1 in = 25.4 mm). Head = chin to crown (top of the hair). Eye line = from the bottom edge.

/// Rules for a photo uploaded to an online form.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Digital {
    /// Smallest and largest picture, in pixels (width, height).
    pub min_px: (u32, u32),
    pub max_px: (u32, u32),
    /// File size limits in KB (1 KB = 1024 bytes); 0 = no lower limit.
    pub min_kb: u32,
    pub max_kb: u32,
    pub note: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spec {
    pub id: &'static str,
    pub name: &'static str,
    /// Words the country picker matches besides the name (country names, other spellings).
    pub keywords: &'static str,
    pub w_mm: f32,
    pub h_mm: f32,
    pub head_min_mm: f32,
    pub head_max_mm: f32,
    /// Eye line from the bottom edge, when the rules give it.
    pub eye_min_mm: Option<f32>,
    pub eye_max_mm: Option<f32>,
    /// The crown's distance from the top edge when the rules give it.
    pub crown_rule_mm: Option<f32>,
    pub background: &'static str,
    pub digital: Option<Digital>,
    pub source: &'static str,
    /// The day the source was read (`""` with `verified: false`).
    pub checked: &'static str,
    pub verified: bool,
    pub note: &'static str,
}

pub const CUSTOM_ID: &str = "custom";

#[allow(clippy::too_many_arguments)]
const fn sp(id: &'static str, name: &'static str, keywords: &'static str, w_mm: f32, h_mm: f32, head: (f32, f32), eye: Option<(f32, f32)>, crown_rule_mm: Option<f32>, background: &'static str, digital: Option<Digital>, source: &'static str, checked: &'static str, verified: bool, note: &'static str) -> Spec {
    let (eye_min_mm, eye_max_mm) = match eye {
        Some((a, b)) => (Some(a), Some(b)),
        None => (None, None),
    };
    Spec { id, name, keywords, w_mm, h_mm, head_min_mm: head.0, head_max_mm: head.1, eye_min_mm, eye_max_mm, crown_rule_mm, background, digital, source, checked, verified, note }
}

/// The day the sources below were read.
pub const CHECKED: &str = "2026-10-18";

const fn dig(min_px: (u32, u32), max_px: (u32, u32), min_kb: u32, max_kb: u32, note: &'static str) -> Option<Digital> {
    Some(Digital { min_px, max_px, min_kb, max_kb, note })
}

/// `verified`: the size and head rules were confirmed on the official page (read directly, or as quoted from that
/// government domain by a search when the site refused automated reading; the note says which).
pub static SPECS: &[Spec] = &[
    sp("us-passport", "USA passport 2×2 in", "united states america usa us passport", 50.8, 50.8, (25.4, 34.9), None, None, "white or off-white", dig((600, 600), (600, 600), 54, 10240, "online renewal: JPEG, 54 KB to 10 MB; no pixel size stated, 600×600 used"), "https://travel.state.gov/content/travel/en/passports/how-apply/photos.html", CHECKED, true, "Head 1 to 1⅜ in. The eye-line rule is no longer on the current page. Read through a search of travel.state.gov (the site refused direct reading)."),
    sp("us-visa", "US visa (DS-160) 2×2 in", "united states america usa us visa ds-160 ds160", 50.8, 50.8, (25.4, 34.9), None, None, "white or off-white", dig((600, 600), (1200, 1200), 0, 240, "JPEG, square, 600×600 to 1200×1200 px, at most 240 KB"), "https://travel.state.gov/content/travel/en/us-visas/visa-information-resources/photos.html", "", false, "The 600–1200 px, 240 KB upload rule is the widely used DS-160 rule but was confirmed on the official page only for the Diversity Visa."),
    sp("in-passport", "India passport 35×45 mm", "india indian bharat passport seva", 35.0, 45.0, (36.0, 38.0), None, None, "white", None, "https://www.passportindia.gov.in/", CHECKED, true, "Booklet: 4.5 × 3.5 cm, white background; face 80–85% of the photo (minors and ICAO guides). Read through a search of passportindia.gov.in."),
    sp("in-evisa", "India e-Visa (square)", "india indian evisa e-visa visa", 50.8, 50.8, (25.4, 34.9), None, None, "plain, light or white", dig((350, 350), (1000, 1000), 10, 300, "JPEG, square; e-Visa allows 10 KB–1 MB, the visa guide 350–1000 px and 10–300 KB (both met)"), "https://indianvisaonline.gov.in/evisa/tvoa.html", CHECKED, true, "Full head from the top of the hair to the chin, centred; no head ratio is given (a 2×2 in guide is drawn)."),
    sp("uk-passport", "UK passport 35×45 mm", "united kingdom uk britain british gb england passport", 35.0, 45.0, (29.0, 34.0), None, None, "plain cream or light grey", dig((600, 750), (0, 0), 50, 10240, "online: at least 600×750 px, 50 KB to 10 MB, do not crop tighter"), "https://www.gov.uk/photos-for-passports/photo-requirements", CHECKED, true, "Head 29–34 mm chin to crown."),
    sp("eu-schengen", "Schengen visa / EU 35×45 mm", "europe european union eu schengen visa netherlands italy spain", 35.0, 45.0, (31.5, 36.0), None, None, "plain, light", None, "https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32009R0810", CHECKED, true, "The EU sets no single size; 35×45 mm is the member states' size (e.g. government.nl). Face 70–80% of the height (ICAO)."),
    sp("de-passport", "Germany passport 35×45 mm", "germany german deutschland reisepass passbild", 35.0, 45.0, (32.0, 36.0), None, None, "plain, light, contrasting", None, "https://www.bundesdruckerei.de/system/files/dokumente/pdf/Passbildschablone-Erwachsene.pdf", CHECKED, true, "Face 32–36 mm from age 10. Read through a search of the official sites."),
    sp("fr-passport", "France passport 35×45 mm", "france french passeport", 35.0, 45.0, (32.0, 36.0), None, None, "plain light blue or light grey — white is not allowed", None, "https://www.service-public.gouv.fr/particuliers/vosdroits/F10619", CHECKED, true, "32–36 mm from the chin to the top of the skull (hair not counted). Read through a search of service-public.gouv.fr."),
    sp("ca-passport", "Canada passport 50×70 mm", "canada canadian passport", 50.0, 70.0, (31.0, 36.0), None, None, "plain white or light", dig((1200, 1800), (3000, 4500), 200, 5120, "online renewal: JPEG 2:3, 1200×1800 to 3000×4500 px, 200 KB to 5 MB, from a commercial photographer"), "https://www.canada.ca/en/immigration-refugees-citizenship/services/canadian-passports/photos.html", CHECKED, true, "Head 31–36 mm chin to crown."),
    sp("au-passport", "Australia passport 35×45 mm", "australia australian passport", 35.0, 45.0, (32.0, 36.0), None, None, "plain white or light grey", None, "https://www.passports.gov.au/help/passport-photos", CHECKED, true, "35–40 × 45–50 mm, head 32–36 mm; glossy prints. Read through a search of passports.gov.au."),
    sp("cn-passport", "China passport 33×48 mm", "china chinese prc passport travel document", 33.0, 48.0, (28.0, 33.0), None, Some(4.0), "white", None, "https://us.china-embassy.gov.cn/lsfw/zj/hzlxz/201903/t20190309_5098803.htm", CHECKED, true, "Head 28–33 mm high, 15–22 mm wide; crown 3–5 mm from the top. Read through a search of the embassy site."),
    sp("cn-visa", "China visa 33×48 mm", "china chinese prc visa", 33.0, 48.0, (28.0, 33.0), None, Some(4.0), "white or near white", dig((354, 472), (354, 472), 40, 120, "online: JPEG, at most 354×472 px, 40–120 KB"), "https://www.visaforchina.cn/", CHECKED, true, "Online rules read on visaforchina.cn; print rules as for the passport."),
    sp("jp-passport", "Japan passport 35×45 mm", "japan japanese passport", 35.0, 45.0, (32.0, 36.0), None, None, "plain, white recommended", dig((413, 531), (413, 531), 0, 600, "online (Mynaportal): JPEG, at most 600 KB; no pixel size stated, 300 dpi size used"), "https://www.mofa.go.jp/mofaj/toko/passport/ic_photo.html", CHECKED, true, "Face 34 ± 2 mm. Read through a search of mofa.go.jp."),
    sp("sg-passport", "Singapore passport 35×45 mm", "singapore passport ica", 35.0, 45.0, (25.0, 35.0), None, None, "white (light grey if hair or headwear is white)", dig((400, 514), (400, 514), 0, 8192, "online: 400×514 px, at most 8 MB"), "https://www.ica.gov.sg/photo-guidelines", CHECKED, true, "Online size read on ica.gov.sg; head 25–35 mm from the older ICA guide."),
    sp("my-passport", "Malaysia passport 35×50 mm", "malaysia malaysian passport", 35.0, 50.0, (25.0, 30.0), None, None, "white, no shadow", None, "https://www.imi.gov.my/index.php/en/main-services/passport/malaysian-international-passport/", CHECKED, true, "Head 25–30 mm. Read through a search of the official sites."),
    sp("ae-passport", "UAE (ICP) 35×45 mm", "uae united arab emirates dubai abu dhabi icp emirates id visa", 35.0, 45.0, (31.5, 36.0), None, None, "white", None, "https://icp.gov.ae/wp-content/uploads/2021/11/icao_english.pdf", CHECKED, true, "35–40 × 45 mm, face 70–80% of the photo."),
    sp("sa-evisa", "Saudi Arabia eVisa", "saudi arabia ksa evisa visa", 50.8, 50.8, (35.6, 40.6), None, None, "white, no pattern or shadow", dig((200, 200), (200, 200), 5, 100, "online: 200×200 px, 5–100 KB"), "https://visa.visitsaudi.com/Home/PhotoSpecifications", CHECKED, true, "Face 70–80% of the frame; embassies take 2×2 in prints. Read through a search of visitsaudi.com."),
    sp("lk-passport", "Sri Lanka passport", "sri lanka lankan passport", 35.0, 45.0, (32.0, 36.0), None, None, "as the studio requires", None, "https://www.immigration.gov.lk/pages_e.php?id=7", "", false, "No size is stated: passport photos are taken digitally by an authorised studio and prints are not accepted. 35×45 mm is used on visa forms."),
    sp("nz-passport", "New Zealand passport (online)", "new zealand nz kiwi passport", 36.0, 48.0, (28.8, 38.4), None, None, "plain, light", dig((900, 1200), (4500, 6000), 250, 5120, "online: JPEG 3:4, 900×1200 to 4500×6000 px, 250 KB to 5 MB"), "https://www.passports.govt.nz/passport-photos", "", false, "The online size is confirmed; the head size is not stated, so the guide shows 60–80% of the height."),
    sp("za-passport", "South Africa passport 35×45 mm", "south africa african rsa passport", 35.0, 45.0, (32.0, 36.0), None, None, "white", None, "https://www.dirco.gov.za/canberra/south-african-passports/", "", false, "Size only from a mission page; photos are usually taken live at Home Affairs offices. Head size not stated."),
    sp("br-passport", "Brazil passport 5×7 cm", "brazil brasil brazilian passaporte", 50.0, 70.0, (34.0, 42.0), None, None, "white", None, "https://www.gov.br/pf/pt-br/assuntos/passaporte/legislacao/in_173_de_2020.pdf", "", false, "5×7 cm prints are only for children under 5; others are photographed at the appointment. Head size not stated."),
    sp("mx-passport", "Mexico passport 35×45 mm", "mexico mexican pasaporte sre", 35.0, 45.0, (32.0, 36.0), None, None, "white", None, "https://consulmex.sre.gob.mx/sandiego/images/Formas/sol_pasaporte.pdf", "", false, "Size from one consulate form only; photos are usually taken on site. Head size not stated."),
    sp("ru-passport", "Russia passport 35×45 mm", "russia russian federation passport zagran", 35.0, 45.0, (32.0, 36.0), None, None, "white, even", None, "", "", false, "Numbers from legal texts not on a government page; the MVD page could not be read."),
    sp(CUSTOM_ID, "Custom size", "custom other own", 35.0, 45.0, (27.0, 34.0), None, None, "as required", None, "", "", false, "Your own width and height; check the head size yourself"),
];

impl Spec {
    /// Where the crown sits (mm from the top): from the eye-line rule when there is one, else the head centred a little
    /// above the middle.
    pub fn crown_mm(&self) -> f32 {
        let mid = (self.head_min_mm + self.head_max_mm) / 2.0;
        if let Some(c) = self.crown_rule_mm {
            return c;
        }
        match (self.eye_min_mm, self.eye_max_mm) {
            (Some(a), Some(b)) => ((self.h_mm - (a + b) / 2.0) - 0.47 * mid).max(1.0),
            _ => ((self.h_mm - mid) * 0.45).max(1.0),
        }
    }

    /// The rules in one line, for the app and the assistant.
    pub fn describe(&self) -> String {
        let mut s = format!("{}: {}×{} mm, head {}–{} mm chin to crown", self.name, fmt(self.w_mm), fmt(self.h_mm), fmt(self.head_min_mm), fmt(self.head_max_mm));
        if let (Some(a), Some(b)) = (self.eye_min_mm, self.eye_max_mm) {
            s.push_str(&format!(", eyes {}–{} mm from the bottom", fmt(a), fmt(b)));
        }
        s.push_str(&format!(", {} background", self.background));
        if let Some(d) = self.digital {
            if d.max_px == (0, 0) {
                s.push_str(&format!("; online: at least {}×{} px, ", d.min_px.0, d.min_px.1));
            } else if d.min_px == d.max_px {
                s.push_str(&format!("; online: {}×{} px, ", d.min_px.0, d.min_px.1));
            } else {
                s.push_str(&format!("; online: {}×{} to {}×{} px, ", d.min_px.0, d.min_px.1, d.max_px.0, d.max_px.1));
            }
            if d.min_kb > 0 {
                s.push_str(&format!("{}–{} KB", d.min_kb, d.max_kb));
            } else {
                s.push_str(&format!("at most {} KB", d.max_kb));
            }
        }
        if self.verified {
            s.push_str(&format!(" (source: {}, checked {})", self.source, self.checked));
        } else if self.id != CUSTOM_ID {
            s.push_str(" (UNVERIFIED: no official page confirmed these numbers; check before use");
            if !self.source.is_empty() {
                s.push_str(&format!("; see {}", self.source));
            }
            s.push(')');
        }
        s
    }
}

fn fmt(v: f32) -> String {
    if (v - v.round()).abs() < 0.05 { format!("{}", v.round() as i64) } else { format!("{v:.1}") }
}

pub fn spec(id: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.id == id)
}

/// The entries matching what was typed in the picker (every word must appear in the name, id or keywords), in table order.
pub fn search(q: &str) -> Vec<usize> {
    let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
    SPECS.iter().enumerate().filter(|(_, s)| {
        let hay = format!("{} {} {}", s.name, s.id, s.keywords).to_lowercase();
        words.iter().all(|w| hay.contains(w.as_str()))
    }).map(|(i, _)| i).collect()
}

/// The custom entry with the person's own size.
pub fn custom(w_mm: f32, h_mm: f32) -> Spec {
    let base = *spec(CUSTOM_ID).expect("the custom entry");
    let h = h_mm.clamp(20.0, 120.0);
    Spec { w_mm: w_mm.clamp(20.0, 120.0), h_mm: h, head_min_mm: h * 0.6, head_max_mm: h * 0.75, ..base }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_names_a_source_and_date_for_every_verified_entry_and_marks_the_rest() {
        let must = ["us-passport", "us-visa", "in-passport", "in-evisa", "uk-passport", "eu-schengen", "de-passport", "fr-passport", "ca-passport", "au-passport", "cn-passport", "cn-visa", "jp-passport", "sg-passport", "my-passport", "ae-passport", "sa-evisa", "lk-passport", "nz-passport", "za-passport", "br-passport", "mx-passport", "ru-passport", CUSTOM_ID];
        for id in must {
            assert!(spec(id).is_some(), "{id} is in the table");
        }
        let mut ids: Vec<&str> = SPECS.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), SPECS.len(), "ids are unique");
        for s in SPECS {
            assert!(s.w_mm >= 20.0 && s.h_mm >= 20.0 && s.head_min_mm < s.head_max_mm && s.head_max_mm < s.h_mm, "{}", s.id);
            if s.verified {
                assert!(s.source.starts_with("https://") && s.checked == CHECKED, "{}: a verified entry names its official page and the day", s.id);
                assert!(s.describe().contains(s.source) && !s.describe().contains("UNVERIFIED"));
            } else if s.id != CUSTOM_ID {
                assert!(s.checked.is_empty() && s.describe().contains("UNVERIFIED"), "{}", s.id);
            }
            if let Some(d) = s.digital {
                assert!(d.max_kb > d.min_kb && d.min_px.0 > 0 && (d.max_px == (0, 0) || (d.max_px.0 >= d.min_px.0 && d.max_px.1 >= d.min_px.1)), "{}", s.id);
            }
            // nothing in the table is fetched: sources are plain text
            assert!(!s.source.contains(' '));
        }
        // France forbids white; the US visa rule is shown with its pixel and size limits
        assert!(spec("fr-passport").unwrap().background.contains("not allowed"));
        let us = spec("us-visa").unwrap().describe();
        assert!(us.contains("600×600 to 1200×1200 px") && us.contains("at most 240 KB") && us.contains("UNVERIFIED"), "{us}");
        assert!(spec("uk-passport").unwrap().describe().contains("at least 600×750 px"));
        assert!(spec("sa-evisa").unwrap().describe().contains("200×200 px, 5–100 KB"));
    }

    #[test]
    fn the_picker_finds_countries_by_any_word_and_the_custom_size_is_kept_sane() {
        let names = |q: &str| search(q).into_iter().map(|i| SPECS[i].id).collect::<Vec<_>>();
        assert_eq!(names("india"), vec!["in-passport", "in-evisa"]);
        assert_eq!(names("usa visa"), vec!["us-visa"]);
        assert_eq!(names("UK"), vec!["uk-passport"]);
        assert!(names("deutschland").contains(&"de-passport"));
        assert_eq!(names("").len(), SPECS.len());
        assert!(names("atlantis").is_empty());
        let c = custom(40.0, 500.0);
        assert_eq!((c.w_mm, c.h_mm), (40.0, 120.0));
        assert!(c.head_min_mm < c.head_max_mm && c.head_max_mm < c.h_mm && c.id == CUSTOM_ID);
    }
}
