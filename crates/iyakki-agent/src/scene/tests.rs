use super::art::Time;
use super::compose::{compose, compose_svg};
use super::*;
use crate::chat::{ChatBackend, Message};
use crate::story::{DrawingMode, PictureStyle, Scene, StoryOptions};
use std::sync::Mutex;

struct Fake {
    f: Box<dyn Fn(&[Message]) -> Result<String, String> + Send + Sync>,
    seen: Mutex<Vec<String>>,
}

impl Fake {
    fn new(f: impl Fn(&[Message]) -> Result<String, String> + Send + Sync + 'static) -> Fake {
        Fake { f: Box::new(f), seen: Mutex::new(vec![]) }
    }
    fn calls(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

impl ChatBackend for Fake {
    fn complete(&self, h: &[Message]) -> Result<String, String> {
        self.seen.lock().unwrap().push(format!("{}\n{}", h[0].content, h.get(1).map_or("", |m| m.content.as_str())));
        (self.f)(h)
    }
}

fn px(b: &iyakki_web::image::Bitmap, x: u32, y: u32) -> [u8; 4] {
    let o = ((y * b.width + x) * 4) as usize;
    [b.rgba[o], b.rgba[o + 1], b.rgba[o + 2], b.rgba[o + 3]]
}

fn render(svg: &str, w: u32, h: u32) -> iyakki_web::image::Bitmap {
    iyakki_web::svg::render(svg.as_bytes(), Some((w, h))).unwrap()
}

fn scene(text: &str) -> Scene {
    Scene { place_before: 1, title: "Part 1".into(), text: text.into() }
}

fn sheet() -> CharacterSheet {
    CharacterSheet { name: "Arun".into(), skin: "brown".into(), hair: "black".into(), top: "yellow".into(), bottom: "blue".into(), companion: "goat".into(), ..CharacterSheet::default() }
}

// ---------------------------------------------------------------------------------------------- the sanitiser and the renderer

const BAKERY: &str = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 800 480'>\
<defs><linearGradient id='sky' x1='0' y1='0' x2='0' y2='1'><stop offset='0' stop-color='#ff0000'/><stop offset='1' stop-color='#0000ff'/></linearGradient>\
<clipPath id='win'><rect x='520' y='100' width='100' height='100'/></clipPath></defs>\
<rect width='800' height='480' fill=\"url('#sky')\"/>\
<rect x='0' y='300' width='800' height='180' style='fill:#00aa00'/>\
<g transform='rotate(0 400 240)'><path d='M100 300 L100 150 L260 150 L260 300 Z' fill='#cc8844' stroke='#552200' stroke-width='4'/></g>\
<circle cx='570' cy='150' r='80' fill='#ffff00' clip-path='url(#win)'/>\
<path d='M300 400 A60 60 0 0 1 420 400 Z' fill='#ffffff'/>\
<style>rect{fill:#123456}</style><text x='5' y='5'>words</text>\
</svg>";

#[test]
fn a_good_hand_written_drawing_survives_the_sanitiser_and_renders_as_written() {
    let out = crate::story::sanitize_svg(BAKERY).unwrap();
    // the parts that used to be lost: the gradient stops, the quoted url, the style attribute, the clip path
    for kept in ["<stop offset=\"0\" stop-color=\"#ff0000\"/>", "fill=\"url('#sky')\"", "fill=\"#00aa00\"", "<clipPath id=\"win\">", "clip-path=\"url(#win)\""] {
        assert!(out.contains(kept), "{kept} was lost: {out}");
    }
    assert!(!out.contains("<text") && !out.contains("<style") && !out.contains("words"));
    let b = render(&out, 400, 240);
    let top = px(&b, 200, 2);
    let bot = px(&b, 200, 140);
    assert!(top[0] > 200 && top[2] < 60, "the sky starts red at the top: {top:?}");
    assert!(bot[2] > bot[0], "and ends blue lower down: {bot:?}");
    assert_eq!(px(&b, 200, 225)[..3], [0, 170, 0], "the ground is the green of its style attribute");
    assert_eq!(px(&b, 90, 125)[..3], [204, 136, 68], "the wall");
    // the sun is clipped to the window square
    assert_eq!(px(&b, 285, 75)[..3], [255, 255, 0], "inside the clip");
    assert_ne!(px(&b, 285, 100)[..3], [255, 255, 0], "outside the clip");
    assert_eq!(px(&b, 180, 190)[..3], [255, 255, 255], "the arc closes into a filled half disc");
    // thinking before the answer is ignored even if it mentions a drawing
    let thought = format!("<think>I will write <svg then...</think>{BAKERY}");
    assert_eq!(crate::story::sanitize_svg(&thought).unwrap(), out);
    // the caps leave room for a rich drawing, and still stop a flood
    assert!(crate::story::MAX_SVG_BYTES >= 200_000 && crate::story::MAX_SVG_ELEMENTS >= 3000);
    let big = format!("<svg viewBox='0 0 800 480'>{}</svg>", "<circle cx='5' cy='5' r='3' fill='#abc'/>".repeat(2500));
    assert!(crate::story::sanitize_svg(&big).is_ok());
    // external references still cannot get through
    let evil = "<svg viewBox='0 0 8 8'><use href='http://evil/x.svg#a'/><rect width='8' height='8' filter='url(http://evil/f)' fill='#fff'/><rect width='8' height='8' fill='#000'/></svg>";
    let out = crate::story::sanitize_svg(evil).unwrap();
    assert!(!out.contains("evil") && !out.contains("href"), "{out}");
}

#[test]
fn the_picture_checks_reject_blank_flat_and_tiny_drawings_and_accept_a_scene() {
    let flat = format!("<svg viewBox='0 0 800 480'><rect width='800' height='480' fill='#336699'/>{}</svg>", "<circle cx='5' cy='5' r='2' fill='#336699'/>".repeat(14));
    assert!(rich::check_picture(&flat).unwrap_err().contains("flat colour"));
    let empty = format!("<svg viewBox='0 0 800 480'>{}</svg>", "<circle cx='400' cy='240' r='10' fill='#f00'/>".repeat(14));
    assert!(rich::check_picture(&empty).unwrap_err().contains("empty"));
    assert!(rich::check_picture("<svg viewBox='0 0 8 8'><rect width='8' height='8' fill='#f00'/></svg>").unwrap_err().contains("only"));
    // a composed scene passes its own checks
    let c = parse_composition(r#"{"setting":"village","time":"sunset","objects":["hero","dog","house"]}"#).unwrap();
    assert!(rich::check_picture(&compose_svg(&c, PictureStyle::Watercolor, &sheet())).is_ok());
}

// ---------------------------------------------------------------------------------------------- the composer

#[test]
fn the_scene_json_is_validated_and_clamped() {
    let c = parse_composition("<think>hm</think>Sure!\n```json\n{\"setting\":\"Dark Jungle\",\"time\":\"night\",\"mood\":\"spooky\",\"objects\":[{\"kind\":\"tree\",\"x\":250,\"size\":\"huge\"},{\"kind\":\"Unicorn\",\"x\":10},{\"object\":\"bunny\",\"color\":\"light blue\",\"x\":-30},\"boy\",{\"kind\":\"hero\",\"x\":\"far left\",\"pose\":\"running fast\",\"expression\":\"scared\",\"holds\":\"a torch\"}]}\n```").unwrap();
    assert_eq!((c.setting, c.time, c.mood), (Setting::Jungle, Time::Night, Mood::Mysterious));
    let kinds: Vec<Kind> = c.objects.iter().map(|o| o.kind).collect();
    assert_eq!(kinds, vec![Kind::Tree, Kind::Rabbit, Kind::Hero, Kind::Hero], "the unicorn is dropped, synonyms understood: {kinds:?}");
    assert_eq!(c.objects[0].x, 100.0, "positions are clamped");
    assert_eq!(c.objects[0].size, Size::Large);
    assert_eq!(c.objects[1].x, 0.0);
    assert!(c.objects[1].color.is_some());
    let h = &c.objects[3];
    assert_eq!((h.x, h.pose, h.expr, h.holds), (8.0, Some(Pose::Run), Some(Expr::Scared), Some(Prop::Torch)));
    let many = format!("{{\"objects\":[{}]}}", (0..30).map(|i| format!("{{\"kind\":\"tree\",\"x\":{}}}", i * 3)).collect::<Vec<_>>().join(","));
    assert_eq!(parse_composition(&many).unwrap().objects.len(), MAX_OBJECTS);
    for bad in ["no json at all", "{\"objects\":[]}", "{\"objects\":[\"dragon\"]}", "{\"setting\":\"cave\"}", "{broken"] {
        assert!(parse_composition(bad).is_err(), "{bad}");
    }
}

#[test]
fn every_kind_of_thing_is_drawn_inside_the_frame_with_sky_above_ground() {
    let sh = sheet();
    for (kind, name) in Kind::ALL {
        for (setting, time) in [(Setting::Meadow, "day"), (Setting::River, "sunset"), (Setting::Snow, "day"), (Setting::Beach, "night")] {
            for (x, size) in [(0, "large"), (50, "medium"), (100, "large"), (100, "small")] {
                let json = format!("{{\"setting\":\"{}\",\"time\":\"{time}\",\"objects\":[{{\"kind\":\"{name}\",\"x\":{x},\"size\":\"{size}\"}}]}}", setting.name());
                let c = parse_composition(&json).unwrap();
                let (svg, placed) = compose(&c, PictureStyle::Watercolor, &sh);
                for p in &placed {
                    assert!(p.x - p.half_width >= 3.0 && p.x + p.half_width <= 397.0, "{name} {x} {size} leaves the sides: {p:?}");
                    assert!(p.foot - p.height >= 5.0 && p.foot <= 240.0, "{name} {x} {size} leaves top or bottom: {p:?}");
                }
                if !matches!(kind, Kind::Sun | Kind::Moon | Kind::Stars | Kind::Cloud | Kind::Bird | Kind::Planet | Kind::Lake | Kind::River | Kind::Rain | Kind::Snow | Kind::Fireflies) {
                    assert!(placed.iter().any(|p| p.kind == kind), "{name} was not placed in {json}");
                }
                if x == 50 && size == "medium" && setting == Setting::Meadow {
                    // a picture at every size the tile draws it, never blank, the sky above the ground
                    for (w, h) in [(260u32, 156u32), (520, 312)] {
                        let b = render(&svg, w, h);
                        assert_eq!((b.width, b.height), (w, h));
                        let (top, bot) = (px(&b, w / 8, 3), px(&b, w / 8, h - 4));
                        assert!(top[3] == 255 && bot[3] == 255, "{name}: the frame is covered");
                        let lum = |p: [u8; 4]| p[0] as u32 * 3 + p[1] as u32 * 6 + p[2] as u32;
                        if setting == Setting::Meadow && time == "day" {
                            assert!(top[2] > top[0] && bot[1] > bot[2] && lum(top) > lum(bot) / 2, "{name}: blue sky above green ground: {top:?} {bot:?}");
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn every_setting_in_every_style_is_a_rich_drawing_that_passes_the_sanitiser() {
    let sh = sheet();
    for (setting, name) in Setting::ALL {
        for style in PictureStyle::ALL {
            let json = format!("{{\"setting\":\"{name}\",\"time\":\"day\",\"mood\":\"calm\",\"objects\":[{{\"kind\":\"hero\",\"x\":35}},{{\"kind\":\"goat\",\"x\":60}},{{\"kind\":\"tree\",\"x\":88}}]}}");
            let c = parse_composition(&json).unwrap();
            let svg = compose_svg(&c, *style, &sh);
            let kept = crate::story::sanitize_svg(&svg).unwrap_or_else(|e| panic!("{name} {style:?}: {e}"));
            assert!(kept.len() > 4000, "{name} {style:?} is a real picture");
            rich::check_picture(&svg).unwrap_or_else(|e| panic!("{name} {style:?}: {e}"));
            assert_eq!(svg, compose_svg(&c, *style, &sh), "{name} {style:?} is deterministic");
            let _ = setting;
        }
    }
    // pixel art is made of square rectangles only
    let c = parse_composition(r#"{"setting":"forest","objects":["hero"]}"#).unwrap();
    let px_svg = compose_svg(&c, PictureStyle::Pixel, &sh);
    assert!(px_svg.matches("<rect").count() > 300 && !px_svg.contains("<circle") && px_svg.len() < 150_000);
}

#[test]
fn the_hero_looks_the_same_on_every_page_and_follows_the_sheet() {
    let a = parse_composition(r#"{"setting":"meadow","time":"day","objects":[{"kind":"hero","x":30},{"kind":"goat","x":70}]}"#).unwrap();
    let b = parse_composition(r#"{"setting":"cave","time":"night","objects":[{"kind":"hero","x":60,"pose":"run"}]}"#).unwrap();
    let mut s1 = sheet();
    s1.top = "red".into();
    let (pa, pb) = (compose_svg(&a, PictureStyle::Comic, &s1), compose_svg(&b, PictureStyle::Comic, &s1));
    for (name, svg) in [("meadow", &pa), ("cave", &pb)] {
        assert!(svg.contains("#d8433b"), "{name}: the red shirt of the sheet");
        assert!(svg.contains("#b97c52"), "{name}: the skin of the sheet");
    }
    let mut s2 = s1.clone();
    s2.top = "green".into();
    s2.skin = "pale".into();
    let pc = compose_svg(&a, PictureStyle::Comic, &s2);
    assert!(pc.contains("#4f9f52") && !pc.contains("#d8433b") && pc.contains("#f6d9c4"), "another sheet, another boy");
    // the companion animal comes in the colour the sheet gave it
    let mut s3 = sheet();
    s3.companion_color = "#ff00ff".into();
    assert!(compose_svg(&a, PictureStyle::Comic, &s3).contains("#ff00ff"));
}

#[test]
fn the_character_sheet_comes_from_the_model_or_from_the_words() {
    let story = "Arun was a brave boy. He wore a red shirt and a blue cap. One day Arun met a goat. The goat followed Arun home. Arun smiled at the goat.";
    let d = CharacterSheet::derive("a boy and a goat", "Arun, 8", story);
    assert_eq!((d.name.as_str(), d.gender.as_str(), d.companion.as_str(), d.top.as_str(), d.hat.as_str(), d.human), ("Arun", "boy", "goat", "red", "blue", true), "{d:?}");
    assert_eq!(d, CharacterSheet::derive("a boy and a goat", "Arun, 8", story), "the same words give the same sheet");
    let girl = CharacterSheet::derive("", "", "Mira loved her bike. She rode it every day. Mira said hello to her friend.");
    assert_eq!((girl.name.as_str(), girl.gender.as_str(), girl.bottom_style.as_str()), ("Mira", "girl", "skirt"));
    assert!(!CharacterSheet::derive("a fox in the woods", "", "The fox ran. The fox hid. The fox slept.").human, "a story about an animal has no human hero");
    // the model's JSON is merged over the guess; nonsense in it is ignored
    let fake = Fake::new(|_| Ok("{\"name\":\"Arun\",\"gender\":\"male\",\"age\":9,\"skin\":\"dark brown\",\"hair\":\"black\",\"hair_style\":\"curly\",\"top\":\"yellow\",\"bottom\":\"navy shorts\",\"shoes\":\"white\",\"hat\":\"none\",\"companion\":\"goat\",\"foo\":1}".into()));
    let o = StoryOptions { topic: "a boy and a goat".into(), ..Default::default() };
    let m = sheet::character_sheet(&fake, &o, story);
    assert_eq!((m.gender.as_str(), m.age.as_str(), m.skin.as_str(), m.hair_style.as_str(), m.top.as_str(), m.bottom.as_str(), m.hat.as_str()), ("boy", "child", "deep", "curly", "yellow", "navy", ""), "{m:?}");
    assert_eq!(CharacterSheet::from_json(&m.to_json()).unwrap(), m, "it is saved as JSON and read back");
    // a model that cannot help leaves the guess; one that cannot be reached too
    assert_eq!(sheet::character_sheet(&Fake::new(|_| Ok("sorry".into())), &o, story).name, "Arun");
    assert_eq!(sheet::character_sheet(&Fake::new(|_| Err("down".into())), &o, story).gender, "boy");
    assert!(m.describe().contains("Arun, a boy of about 8") && m.describe().contains("goat"));
    // the same words go to an image model
    let ip = crate::story::image_prompt_with("T", &o, &scene("Arun met a goat."), &m, "make it night");
    assert!(ip.contains("Arun, a boy") && ip.contains("make it night") && ip.contains("No text") && ip.contains("5:3"), "{ip}");
}

// ---------------------------------------------------------------------------------------------- the pipeline

const SENTENCES: [(&str, &str); 10] = [
    ("Arun walked through the green jungle with his goat.", r#"{"setting":"jungle","time":"day","objects":[{"kind":"hero","x":30,"pose":"walk"},{"kind":"goat","x":60},{"kind":"tree","x":90,"size":"large"}]}"#),
    ("In the glowing cave Arun held a torch and saw blue crystals.", r#"{"setting":"cave","time":"night","objects":[{"kind":"hero","x":35,"holds":"torch"},{"kind":"crystal","color":"blue","x":75}]}"#),
    ("Mira sat by the river and watched the birds at sunrise.", r#"{"setting":"river","time":"dawn","objects":[{"kind":"hero","x":25,"pose":"sit"},{"kind":"bird","x":60,"y":12},{"kind":"bridge","x":70}]}"#),
    ("The village lanterns glowed at dusk.", r#"{"setting":"village","time":"sunset","objects":[{"kind":"lantern","x":30},{"kind":"house","x":70},{"kind":"hero","x":50}]}"#),
    ("A little boat rocked on the sea under the sun.", r#"{"setting":"sea","time":"day","objects":[{"kind":"boat","x":50},{"kind":"sun","x":80,"y":15}]}"#),
    ("Snow fell on the pine tree and a rabbit hopped by.", r#"{"setting":"snow","time":"day","objects":[{"kind":"tree","x":30},{"kind":"rabbit","x":60},{"kind":"snow","x":50}]}"#),
    ("The rocket flew past the ringed planet in space.", r#"{"setting":"space","time":"night","objects":[{"kind":"rocket","x":60},{"kind":"planet","x":30,"y":20}]}"#),
    ("The farmer fed the goat near the red barn.", r#"{"setting":"farm","time":"day","objects":[{"kind":"man","x":30},{"kind":"goat","x":50},{"kind":"barn","color":"red","x":80}]}"#),
    ("They sat by the campfire under the stars.", r#"{"setting":"night","time":"night","objects":[{"kind":"fire","x":50},{"kind":"hero","x":30,"pose":"sit"},{"kind":"stars","x":50}]}"#),
    ("Grandma read a book in the warm room while the cat slept.", r#"{"setting":"room","time":"night","objects":[{"kind":"elder","x":35,"holds":"book"},{"kind":"cat","x":60}]}"#),
];

#[test]
fn a_corpus_of_sentences_gives_pictures_that_show_what_the_model_chose() {
    let o = StoryOptions { drawing: DrawingMode::Cartoon, ..Default::default() };
    let mut sums = std::collections::HashSet::new();
    for (text, json) in SENTENCES {
        let fake = Fake::new(move |h| if h[0].content.contains("scene composer") { Ok(json.to_string()) } else { Err("unexpected".into()) });
        let d = draw_picture(&fake, "Tale", &o, &scene(text), &sheet(), &DrawCfg { strong: false, story_text: text, feedback: "", ..Default::default() }).unwrap();
        assert_eq!((d.how, d.note.is_none()), ("composed", true), "{text}");
        assert_eq!(fake.calls().len(), 1, "one request is enough when the JSON is good");
        assert!(fake.calls()[0].contains(text), "the scene sentence is in the request");
        let svg = String::from_utf8(d.bytes).unwrap();
        rich::check_picture(&svg).unwrap_or_else(|e| panic!("{text}: {e}"));
        // the things the model chose are the things drawn
        let c = parse_composition(json).unwrap();
        let (_, placed) = compose(&c, o.style, &sheet());
        for obj in &c.objects {
            if !matches!(obj.kind, Kind::Sun | Kind::Moon | Kind::Stars | Kind::Cloud | Kind::Bird | Kind::Planet | Kind::Rain | Kind::Snow) {
                assert!(placed.iter().any(|p| p.kind == obj.kind), "{text}: {:?} is in the picture", obj.kind);
            }
        }
        assert!(d.log.contains("scene, try 1") && d.log.contains("REPLY"), "the exchange is logged");
        sums.insert(svg);
    }
    assert_eq!(sums.len(), SENTENCES.len(), "different sentences, different pictures");
}

#[test]
fn bad_json_is_asked_again_then_falls_back_to_the_models_svg_then_to_the_words() {
    let o = StoryOptions { drawing: DrawingMode::Cartoon, ..Default::default() };
    // invalid first, valid second: one retry
    let n = std::sync::atomic::AtomicUsize::new(0);
    let fake = Fake::new(move |_| if n.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 { Ok("I would draw a boy".into()) } else { Ok(SENTENCES[0].1.into()) });
    let d = draw_picture(&fake, "T", &o, &scene("Arun in the jungle."), &sheet(), &DrawCfg { strong: false, story_text: "", feedback: "", ..Default::default() }).unwrap();
    assert_eq!((d.how, fake.calls().len()), ("composed", 2));
    // never valid: JSON twice, then the plain SVG request, which the model answers well
    let fake = Fake::new(|h| if h[0].content.contains("scene composer") { Ok("no".into()) } else { Ok(BAKERY.into()) });
    let d = draw_picture(&fake, "T", &o, &scene("Arun in the jungle."), &sheet(), &DrawCfg { strong: false, story_text: "", feedback: "", ..Default::default() }).unwrap();
    assert_eq!((d.how, fake.calls().len()), ("model svg", 3));
    // nothing usable at all: a gentle scene from the words, said honestly
    let fake = Fake::new(|_| Ok("I cannot do that".into()));
    let d = draw_picture(&fake, "T", &o, &scene("Arun walked into the glowing cave at night with a torch."), &sheet(), &DrawCfg { strong: false, story_text: "", feedback: "", ..Default::default() }).unwrap();
    assert_eq!((d.how, fake.calls().len()), ("keywords", 3));
    assert!(d.note.as_deref().unwrap().starts_with("Built-in sketch: the model's drawing could not be used"), "{:?}", d.note);
    assert!(d.log.contains("built-in sketch"));
    rich::check_picture(&String::from_utf8(d.bytes).unwrap()).unwrap();
    // a model that cannot be reached is an error to show, not a made-up picture
    let err = draw_picture(&Fake::new(|_| Err("connection refused".into())), "T", &o, &scene("x"), &sheet(), &DrawCfg { strong: false, story_text: "", feedback: "", ..Default::default() }).unwrap_err();
    assert!(err.contains("connection refused"));
}

#[test]
fn the_words_of_a_sentence_choose_a_scene() {
    let c = compose_from_keywords("Arun walked into the glowing cave at night with a torch and a goat.", true);
    assert_eq!((c.setting, c.time), (Setting::Cave, Time::Night));
    let kinds: Vec<Kind> = c.objects.iter().map(|o| o.kind).collect();
    assert!(kinds.contains(&Kind::Hero) && kinds.contains(&Kind::Goat) && kinds.contains(&Kind::Fire), "{kinds:?}");
    let c = compose_from_keywords("The ship sailed across the sea past a whale at sunset.", false);
    assert_eq!((c.setting, c.time), (Setting::Sea, Time::Sunset));
    assert!(!c.objects.iter().any(|o| o.kind == Kind::Hero) && c.objects.iter().any(|o| o.kind == Kind::Boat));
    let c = compose_from_keywords("Nothing in particular happened.", true);
    assert!(c.objects.iter().any(|o| o.kind == Kind::Hero) && c.objects.len() >= 2 && c.setting == Setting::Meadow);
}

#[test]
fn strong_models_draw_rich_svg_and_weak_ones_use_the_cartoon() {
    for (m, strong) in [("claude-sonnet-4", true), ("gpt-4o", true), ("gemini-2.0-flash", true), ("qwen3:32b", true), ("llama3.3:70b", true), ("o3-mini", true), ("qwen3:8b", false), ("llama3.2:3b", false), ("phi3:mini", false), ("gemma2:9b", false), ("qwen2.5:7b-instruct-q4_0", false)] {
        assert_eq!(rich::is_strong(m, false), strong, "{m}");
    }
    assert!(rich::is_strong("anything", true), "an external agent is strong");
    let o = StoryOptions::default();
    let good = format!("<svg viewBox='0 0 800 480'><defs><linearGradient id='s' x1='0' y1='0' x2='0' y2='1'><stop offset='0' stop-color='#ff9966'/><stop offset='1' stop-color='#3366cc'/></linearGradient></defs><rect width='800' height='480' fill='url(#s)'/>{}</svg>", (0..20).map(|i| format!("<circle cx='{}' cy='{}' r='{}' fill='#{:02x}{:02x}40'/>", 30 + i * 37, 100 + (i * 53) % 300, 10 + i, i * 12, 255 - i * 10)).collect::<String>());
    // strong: one rich request carrying the story, the moment and the character sheet
    let g = good.clone();
    let strong = Fake::new(move |_| Ok(g.clone()));
    let d = draw_picture(&strong, "The Shortcut", &o, &scene("Ravi squeezed through the gap behind the bakery."), &sheet(), &DrawCfg { strong: true, story_text: "FULLSTORY Ravi found a gap.", feedback: "make the wall blue", ..Default::default() }).unwrap();
    assert_eq!((d.how, strong.calls().len()), ("rich svg", 1));
    let ask = &strong.calls()[0];
    for need in ["viewBox=\"0 0 800 480\"", "DRAW THIS MOMENT", "Ravi squeezed through the gap", "FULLSTORY", "CHARACTER SHEET", "Arun, a boy", "make the wall blue", "No text", "layers"] {
        assert!(ask.contains(need), "the rich request lacks {need:?}");
    }
    // a blank first answer is sent back with the reason; the second is used
    let n = std::sync::atomic::AtomicUsize::new(0);
    let g = good.clone();
    let retry = Fake::new(move |_| if n.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 { Ok("<svg viewBox='0 0 800 480'><rect width='10' height='10' fill='#fff'/><rect width='5' height='5' fill='#000'/></svg>".into()) } else { Ok(g.clone()) });
    let d = draw_picture(&retry, "T", &o, &scene("x y z."), &sheet(), &DrawCfg { strong: true, story_text: "", feedback: "", ..Default::default() }).unwrap();
    assert_eq!(d.how, "rich svg");
    assert!(retry.calls()[1].contains("COULD NOT BE USED") && retry.calls()[1].contains("only 2 shapes"), "{}", retry.calls()[1]);
    // a strong model whose drawings keep failing still gets a picture: the cartoon, with the reason
    let bad = Fake::new(|h| if h[0].content.contains("scene composer") { Ok(SENTENCES[0].1.into()) } else { Ok("sorry".into()) });
    let d = draw_picture(&bad, "T", &o, &scene("Arun in the jungle."), &sheet(), &DrawCfg { strong: true, story_text: "", feedback: "", ..Default::default() }).unwrap();
    assert_eq!(d.how, "composed");
    assert!(d.note.unwrap().starts_with("The model's own drawing could not be used"));
    // weak: the composer straight away; and the person can force either way
    let weak = Fake::new(|_| Ok(SENTENCES[0].1.into()));
    assert_eq!(draw_picture(&weak, "T", &o, &scene("x"), &sheet(), &DrawCfg { strong: false, story_text: "", feedback: "", ..Default::default() }).unwrap().how, "composed");
    let forced = Fake::new(move |_| Ok(good.clone()));
    let rich_o = StoryOptions { drawing: DrawingMode::Rich, ..Default::default() };
    assert_eq!(draw_picture(&forced, "T", &rich_o, &scene("x"), &sheet(), &DrawCfg { strong: false, story_text: "", feedback: "", ..Default::default() }).unwrap().how, "rich svg");
}

#[test]
fn composer_requests_carry_the_vocabulary_the_sheet_and_the_safety_words() {
    let o = StoryOptions::default();
    let m = composer_messages("T", &o, &scene("Arun met a goat."), &sheet(), "make it night");
    let sys = &m[0].content;
    for k in Kind::ALL {
        assert!(sys.contains(k.1), "{} is in the vocabulary of the request", k.1);
    }
    for s in Setting::ALL {
        assert!(sys.contains(s.1));
    }
    assert!(sys.contains("Arun, a boy") && sys.contains("small children") && sys.contains("YOU HAVE NO TOOLS") && sys.contains("scene composer"));
    assert!(m[1].content.contains("Arun met a goat.") && m[1].content.contains("make it night"));
}

/// Tiny real pictures made with an image library (8x8 pixels); the PNG has a text chunk and the JPEG a comment, which must not
/// survive. For the tests of what a model's reply may hold.
pub mod samples {
    pub const PNG_HEX: &str = "89504e470d0a1a0a0000000d49484452000000080000000808020000004b6d29dc0000001c74455874436f6d6d656e7400736563726574206c6f636174696f6e206461746154e84f540000002a49444154789c63d4883ab1e0510203034382dc026406e3091b0d4c5106060626aca209720b1807d22800fc713f4fb3c21d830000000049454e44ae426082";
    pub const JPG_HEX: &str = "ffd8ffe000104a46494600010100000100010000fffe001073656372657420636f6d6d656e74ffdb00430006040506050406060506070706080a100a0a09090a140e0f0c1017141818171416161a1d251f1a1b231c1616202c20232627292a29191f2d302d283025282928ffdb0043010707070a080a130a0a13281a161a2828282828282828282828282828282828282828282828282828282828282828282828282828282828282828282828282828ffc00011080008000803012200021101031101ffc4001f0000010501010101010100000000000000000102030405060708090a0bffc400b5100002010303020403050504040000017d01020300041105122131410613516107227114328191a1082342b1c11552d1f02433627282090a161718191a25262728292a3435363738393a434445464748494a535455565758595a636465666768696a737475767778797a838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7e8e9eaf1f2f3f4f5f6f7f8f9faffc4001f0100030101010101010101010000000000000102030405060708090a0bffc400b51100020102040403040705040400010277000102031104052131061241510761711322328108144291a1b1c109233352f0156272d10a162434e125f11718191a262728292a35363738393a434445464748494a535455565758595a636465666768696a737475767778797a82838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae2e3e4e5e6e7e8e9eaf2f3f4f5f6f7f8f9faffda000c03010002110311003f00e53fcf4a28a2b87155fd935eea77eeafd11f6f84c3fb64df338dbb3b7567ffd9";
    pub const GIF_HEX: &str = "47494638376108000800810000c83c28285ac80000000000002c000000000800080000081a000300002090e040830811160cb070604185071b1294f8506240003b";
    pub const WEBP_HEX: &str = "524946465e000000574542505650382052000000f001009d012a0800080001402625b00274304001692ae4a800feec0013369e704de633f485e8044a83dfe03a308beb79b52cce1dbff26c03ffc6f2166329bf637fff9631b7662a3feff626efc1bfaa800000";

    pub fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect()
    }
    pub fn b64(b: &[u8]) -> String {
        crate::story::base64_encode(b)
    }
}

use samples::{b64, bytes};

#[test]
fn replies_are_judged_by_their_first_bytes_in_every_form_a_picture_can_come() {
    use reply::{accept, sniff, PicKind};
    let (png, jpg, gif, webp) = (bytes(samples::PNG_HEX), bytes(samples::JPG_HEX), bytes(samples::GIF_HEX), bytes(samples::WEBP_HEX));
    assert_eq!(sniff(&png), Some((PicKind::Raster, "png")));
    assert_eq!(sniff(&jpg), Some((PicKind::Raster, "jpg")));
    assert_eq!(sniff(&gif), Some((PicKind::Raster, "gif")));
    assert_eq!(sniff(&webp), Some((PicKind::Raster, "webp")));
    assert_eq!(sniff(b"<svg viewBox='0 0 1 1'/>"), Some((PicKind::Svg, "svg")));
    assert_eq!(sniff(b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom"), Some((PicKind::Video, "mp4")));
    assert_eq!(sniff(&[0x1A, 0x45, 0xDF, 0xA3, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]), Some((PicKind::Video, "webm")));
    assert_eq!(sniff(b"hello, this is text"), None);
    assert_eq!(reply::dimensions(&png, "png"), Some((8, 8)));
    assert_eq!(reply::dimensions(&jpg, "jpg"), Some((8, 8)));
    assert_eq!(reply::dimensions(&gif, "gif"), Some((8, 8)));
    assert_eq!(reply::dimensions(&webp, "webp"), Some((8, 8)));
    // a data address: the claimed type is ignored, the bytes decide (a JPEG that says it is a PNG is a JPEG)
    let f = accept(&format!("Here you go! ![scene](data:image/png;base64,{})", b64(&jpg)), None, false).unwrap();
    assert_eq!((f.ext, f.kind, f.origin), ("jpg", PicKind::Raster, "a data address"));
    // a base64 block in a code fence, and a bare base64 line (the picture is made bigger than 200 characters of text)
    let fenced = format!("Sure.\n```base64\n{}\n```\nDone.", b64(&webp).as_bytes().chunks(60).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join("\n"));
    let big = bytes(samples::JPG_HEX);
    assert!(b64(&big).len() > 400);
    assert_eq!(accept(&fenced, None, false).unwrap().ext, "webp");
    assert_eq!(accept(&b64(&big), None, false).unwrap().origin, "a base64 line");
    // an svg block still works, and thinking before the answer is ignored
    assert_eq!(accept("<think>data:image/png;base64,AAAA</think>Here: <svg viewBox='0 0 8 8'><rect width='8' height='8' fill='#f00'/><circle r='3' cx='4' cy='4' fill='#00f'/></svg>", None, false).unwrap().kind, PicKind::Svg);
    // nothing in the reply
    assert!(accept("I cannot draw today.", None, false).unwrap_err().contains("no picture"));
}

#[test]
fn metadata_is_stripped_and_hostile_corrupt_or_oversized_files_are_refused() {
    use reply::{accept, validate};
    let (png, jpg) = (bytes(samples::PNG_HEX), bytes(samples::JPG_HEX));
    assert!(png.windows(4).any(|w| w == b"tEXt") && jpg.windows(14).any(|w| w == b"secret comment"));
    let p = validate(png.clone(), false, "t").unwrap();
    assert!(!p.bytes.windows(4).any(|w| w == b"tEXt") && !p.bytes.windows(6).any(|w| w == b"secret") && p.bytes.len() < png.len(), "the PNG text chunk is gone");
    let j = validate(jpg.clone(), false, "t").unwrap();
    assert!(!j.bytes.windows(6).any(|w| w == b"secret") && j.bytes.len() < jpg.len(), "the JPEG comment is gone");
    assert!(iyakki_web::image::decode(&j.bytes).is_ok(), "and it still decodes");
    // corrupt: a JPEG cut in half, a PNG without its end, random bytes behind a good signature
    assert!(validate(jpg[..jpg.len() / 2].to_vec(), false, "t").unwrap_err().contains("cannot be decoded"));
    assert!(validate(png[..png.len() - 12].to_vec(), false, "t").unwrap_err().contains("cut off"));
    let mut junk = png[..33].to_vec();
    junk.extend(std::iter::repeat(7u8).take(40));
    assert!(validate(junk, false, "t").is_err());
    // a decompression bomb: a header that claims 60000 x 60000 pixels is refused before anything is decoded
    let mut bomb = png.clone();
    bomb[16..20].copy_from_slice(&60000u32.to_be_bytes());
    bomb[20..24].copy_from_slice(&60000u32.to_be_bytes());
    assert!(validate(bomb, false, "t").unwrap_err().contains("more than"));
    // too large in bytes
    let mut huge = png.clone();
    huge.extend(std::iter::repeat(0u8).take(reply::MAX_IMAGE_BYTES));
    assert!(validate(huge, false, "t").unwrap_err().contains("too large"));
    // a bad svg is refused by the sanitiser, a mislabelled executable is not a picture at all
    assert!(validate(b"<svg><script>1</script></svg>".to_vec(), false, "t").is_err());
    assert!(validate(b"MZ\x90\0 not a picture".to_vec(), false, "t").unwrap_err().contains("not a picture"));
    // video: off unless the person allowed clips, then size-capped
    let mp4 = b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom-and-some-more-bytes".to_vec();
    assert!(validate(mp4.clone(), false, "t").unwrap_err().contains("switched off"));
    assert_eq!(validate(mp4.clone(), true, "t").unwrap().ext, "mp4");
    let mut big = mp4;
    big.extend(std::iter::repeat(0u8).take(reply::MAX_VIDEO_BYTES));
    assert!(validate(big, true, "t").unwrap_err().contains("too large"));
    // a corrupt picture in a reply names why, so the caller can fall back
    assert!(accept(&format!("![x](data:image/jpeg;base64,{})", b64(&jpg[..100])), None, false).unwrap_err().contains("cannot be"));
}

#[test]
fn files_in_the_agents_folder_are_used_only_from_that_folder_and_links_are_never_fetched() {
    use reply::accept;
    let dir = std::env::temp_dir().join(format!("iyakki-reply-{}", std::process::id()));
    let work = dir.join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("scene.png"), bytes(samples::PNG_HEX)).unwrap();
    std::fs::write(dir.join("outside.png"), bytes(samples::PNG_HEX)).unwrap();
    let f = accept("I saved it: ![the scene](scene.png)", Some(&work), false).unwrap();
    assert_eq!((f.ext, f.origin), ("png", "a file in the agent's folder"));
    // the file name is looked up in that folder only: a path that climbs out, or an absolute path elsewhere, is not read
    assert!(accept("![x](../outside.png)", Some(&work), false).is_err());
    assert!(accept(&format!("![x]({})", dir.join("outside.png").display()), Some(&work), false).is_err());
    assert!(accept("![x](scene.png)", None, false).is_err(), "without a folder nothing is read");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.join("outside.png"), work.join("link.png")).unwrap();
        assert!(accept("![x](link.png)", Some(&work), false).is_err(), "a link that points out of the folder is not followed");
    }
    // an address on the web is not fetched: the error names the host so the person can decide
    let e = accept("![picture](https://images.example.org/cdn/abc.png)", Some(&work), false).unwrap_err();
    assert!(e.contains("images.example.org") && e.contains("not fetched without your approval"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_image_a_strong_writer_returns_is_used_as_it_is_and_a_bad_one_falls_back_with_the_reason() {
    let png = bytes(samples::PNG_HEX);
    let o = StoryOptions::default();
    let p = png.clone();
    let writer = Fake::new(move |_| Ok(format!("Here is the picture: ![scene](data:image/png;base64,{})", b64(&p))));
    let d = draw_picture(&writer, "T", &o, &scene("Ravi found a gap."), &sheet(), &DrawCfg { strong: true, ..Default::default() }).unwrap();
    assert_eq!((d.how, d.ext, d.bytes.starts_with(&[0x89, b'P'])), ("image reply", "png", true));
    assert!(d.log.contains("used: png from a data address") && d.prompt.starts_with("Image from the writer"), "{}", d.log);
    // a corrupt image: the second try carries the reason; then the built-in cartoon says why
    let bad = Fake::new(|h| if h[0].content.contains("scene composer") { Ok(SENTENCES[0].1.into()) } else { Ok(format!("![x](data:image/png;base64,{})", b64(&bytes(samples::PNG_HEX)[..60]))) });
    let d = draw_picture(&bad, "T", &o, &scene("x"), &sheet(), &DrawCfg { strong: true, ..Default::default() }).unwrap();
    assert_eq!(d.how, "composed");
    assert!(d.note.unwrap().contains("cut off or damaged"), "the reason reaches the label");
    assert!(bad.calls().iter().any(|c| c.contains("COULD NOT BE USED") && c.contains("cut off or damaged")));
    // a video the person did not allow is refused with that reason
    let mp4 = b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom-and-some-more-bytes".to_vec();
    let m = mp4.clone();
    let vid = Fake::new(move |h| if h[0].content.contains("scene composer") { Ok(SENTENCES[0].1.into()) } else { Ok(format!("data:video/mp4;base64,{}", b64(&m))) });
    let d = draw_picture(&vid, "T", &o, &scene("x"), &sheet(), &DrawCfg { strong: true, ..Default::default() }).unwrap();
    assert!(d.how == "composed" && d.note.unwrap().contains("switched off"));
    let d = draw_picture(&vid, "T", &o, &scene("x"), &sheet(), &DrawCfg { strong: true, allow_video: true, ..Default::default() }).unwrap();
    assert_eq!((d.how, d.ext), ("video reply", "mp4"));
    assert!(!StoryOptions::default().video, "video clips are off by default");
}
