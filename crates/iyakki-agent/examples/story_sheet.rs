//! Draws the sample scenes of the built-in cartoon composer to PPM files, for the contact sheet in docs/media:
//! `cargo run -q --release -p iyakki-agent --example story_sheet [outdir] [style] [scene-number]`
//! (no model is involved: the scenes are fixed JSON, as a model would give it).

use iyakki_agent::scene::{self, CharacterSheet};
use iyakki_agent::story::PictureStyle;

pub const SCENES: [(&str, &str); 12] = [
    ("jungle", r#"{"setting":"jungle","time":"day","mood":"happy","objects":[{"kind":"hero","x":36,"pose":"walk","expression":"surprised"},{"kind":"flower","color":"red","x":14},{"kind":"tree","x":88,"size":"large"},{"kind":"mushroom","x":66}]}"#),
    ("cave", r#"{"setting":"cave","time":"night","mood":"mysterious","objects":[{"kind":"hero","x":40,"holds":"torch","expression":"surprised","pose":"stand"},{"kind":"crystal","color":"cyan","x":74,"size":"large"},{"kind":"crystal","color":"violet","x":14}]}"#),
    ("goat", r#"{"setting":"meadow","time":"day","mood":"happy","objects":[{"kind":"sun","x":78,"y":14},{"kind":"hero","x":34,"pose":"hold hands"},{"kind":"goat","x":62},{"kind":"tree","x":90,"size":"large"},{"kind":"flower","color":"pink","x":16},{"kind":"cloud","x":30,"y":12}]}"#),
    ("river", r#"{"setting":"river","time":"dawn","mood":"calm","objects":[{"kind":"hero","x":22,"pose":"sit","expression":"happy"},{"kind":"bridge","x":60},{"kind":"tree","x":88},{"kind":"bird","x":50,"y":10}]}"#),
    ("village", r#"{"setting":"village","time":"sunset","mood":"calm","objects":[{"kind":"hero","x":30,"pose":"walk"},{"kind":"dog","x":52},{"kind":"lantern","x":78},{"kind":"house","x":88}]}"#),
    ("sea", r#"{"setting":"sea","time":"day","mood":"exciting","objects":[{"kind":"boat","x":50,"size":"large"},{"kind":"bird","x":24,"y":14},{"kind":"sun","x":80,"y":16}]}"#),
    ("snow", r#"{"setting":"snow","time":"day","mood":"happy","objects":[{"kind":"hero","x":40,"pose":"wave","expression":"happy"},{"kind":"tree","x":84,"size":"large"},{"kind":"rabbit","x":64},{"kind":"tree","x":10}]}"#),
    ("space", r#"{"setting":"space","time":"night","mood":"exciting","objects":[{"kind":"rocket","x":70},{"kind":"hero","x":30,"pose":"look up","expression":"surprised"}]}"#),
    ("farm", r#"{"setting":"farm","time":"day","mood":"happy","objects":[{"kind":"barn","x":78},{"kind":"hero","x":36,"pose":"run"},{"kind":"goat","x":18},{"kind":"fence","x":50}]}"#),
    ("night", r#"{"setting":"night","time":"night","mood":"calm","objects":[{"kind":"fire","x":50},{"kind":"hero","x":30,"pose":"sit","expression":"sleepy"},{"kind":"tent","x":82},{"kind":"fireflies","x":50}]}"#),
    ("room", r#"{"setting":"room","time":"night","mood":"calm","objects":[{"kind":"hero","x":34,"pose":"stand","holds":"book"},{"kind":"cat","x":60}]}"#),
    ("beach", r#"{"setting":"beach","time":"day","mood":"happy","objects":[{"kind":"palm","x":88},{"kind":"hero","x":30,"pose":"point","expression":"happy"},{"kind":"boat","x":56}]}"#),
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = args.get(1).cloned().unwrap_or_else(|| "/tmp/story-sheet".into());
    let style = PictureStyle::from_id(args.get(2).map_or("watercolor", String::as_str)).unwrap_or(PictureStyle::Watercolor);
    let only: Option<usize> = args.get(3).and_then(|s| s.parse().ok());
    std::fs::create_dir_all(&dir).unwrap();
    let sheet = CharacterSheet { name: "Arun".into(), skin: "brown".into(), hair: "black".into(), hair_style: "short".into(), top: "yellow".into(), bottom: "blue".into(), companion: "goat".into(), ..CharacterSheet::default() };
    for (i, (name, json)) in SCENES.iter().enumerate() {
        if only.is_some_and(|o| o != i) {
            continue;
        }
        let comp = scene::parse_composition(json).unwrap();
        let t = std::time::Instant::now();
        let svg = scene::draw(&comp, style, &sheet);
        let bm = iyakki_web::svg::render(svg.as_bytes(), Some((800, 480))).unwrap();
        let mut ppm = format!("P6\n{} {}\n255\n", bm.width, bm.height).into_bytes();
        for p in bm.rgba.chunks(4) {
            ppm.extend_from_slice(&p[..3]);
        }
        std::fs::write(format!("{dir}/{i:02}-{name}.ppm"), ppm).unwrap();
        std::fs::write(format!("{dir}/{i:02}-{name}.svg"), &svg).unwrap();
        eprintln!("{i:02} {name}: {} bytes svg, {} ms", svg.len(), t.elapsed().as_millis());
    }
}
