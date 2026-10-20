//! Putting a [`Composition`] on the page: backdrop first, then the things of the scene far to near, each scaled for its depth
//! and kept inside the frame, the characters facing the action, then the near edge, weather and finishing touches.

use super::art::*;
use super::people::Person;
use super::{CharacterSheet, Composition, Expr, Kind, Mood, Obj, Pose, Prop, Setting};
use crate::story::PictureStyle;

/// (half width, height) of the thing as drawn at scale 1; the height it should have when standing at the bottom of the
/// frame; how far down the ground it stands (0 at the horizon, 1 at the bottom).
fn spec(k: Kind) -> (f32, f32, f32, f32) {
    match k {
        Kind::Tree => (36.0, 105.0, 175.0, 0.2),
        Kind::Palm => (46.0, 100.0, 170.0, 0.24),
        Kind::Bush => (26.0, 24.0, 44.0, 0.42),
        Kind::Flower => (18.0, 30.0, 40.0, 0.82),
        Kind::House => (40.0, 70.0, 100.0, 0.14),
        Kind::Cave => (58.0, 70.0, 130.0, 0.18),
        Kind::Mountain => (92.0, 105.0, 130.0, 0.0),
        Kind::Hill => (112.0, 48.0, 70.0, 0.02),
        Kind::Castle => (48.0, 112.0, 120.0, 0.1),
        Kind::Barn => (50.0, 70.0, 96.0, 0.14),
        Kind::Rock => (24.0, 24.0, 40.0, 0.6),
        Kind::Mushroom => (14.0, 28.0, 34.0, 0.7),
        Kind::Crystal => (26.0, 52.0, 70.0, 0.5),
        Kind::Cactus => (20.0, 60.0, 80.0, 0.34),
        Kind::Fence => (60.0, 28.0, 36.0, 0.5),
        Kind::Tent => (36.0, 60.0, 70.0, 0.4),
        Kind::Fire => (18.0, 46.0, 50.0, 0.66),
        Kind::Lantern => (8.0, 58.0, 70.0, 0.55),
        Kind::Bridge => (70.0, 60.0, 80.0, 0.52),
        Kind::Boat => (34.0, 58.0, 80.0, 0.0),
        Kind::Rocket => (24.0, 100.0, 130.0, 0.32),
        Kind::Cat => (36.0, 54.0, 52.0, 0.74),
        Kind::Dog => (46.0, 56.0, 62.0, 0.74),
        Kind::Goat => (46.0, 82.0, 78.0, 0.72),
        Kind::Bear => (50.0, 60.0, 84.0, 0.72),
        Kind::Rabbit => (30.0, 44.0, 46.0, 0.76),
        Kind::Elephant => (66.0, 76.0, 112.0, 0.68),
        Kind::Hero => (26.0, 100.0, 124.0, 0.74),
        Kind::Man => (26.0, 100.0, 134.0, 0.64),
        Kind::Woman => (28.0, 100.0, 128.0, 0.64),
        Kind::Elder => (26.0, 100.0, 118.0, 0.64),
        Kind::Baby => (24.0, 90.0, 64.0, 0.8),
        _ => (20.0, 40.0, 40.0, 0.5),
    }
}

fn allowed(k: Kind, st: Setting) -> bool {
    let indoor = matches!(st, Setting::Classroom | Setting::Room);
    let cave = st == Setting::Cave;
    let space = st == Setting::Space;
    match k {
        Kind::Sun | Kind::Moon | Kind::Cloud | Kind::Stars => !(indoor || cave || space) || (k == Kind::Stars && space),
        Kind::Tree | Kind::Palm | Kind::Hill | Kind::Mountain | Kind::Cactus | Kind::Barn | Kind::River | Kind::Lake | Kind::Bridge | Kind::Bush | Kind::Castle | Kind::Boat | Kind::Fish | Kind::Rain | Kind::Fence | Kind::Tent | Kind::Bird | Kind::Flower => !(indoor || cave || space) || matches!(k, Kind::Rain if cave) || (k == Kind::Flower && indoor) || (k == Kind::Fence && false),
        Kind::House => !(indoor || cave || space),
        Kind::Cave => !(indoor || space),
        Kind::Planet => space,
        _ => true,
    }
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

/// Where something was put: its kind, the middle of its feet, its scale and the half width and height it covers, in the 400x240
/// frame.
#[derive(Clone, Debug)]
pub struct Placed {
    pub kind: Kind,
    pub x: f32,
    pub foot: f32,
    pub scale: f32,
    pub half_width: f32,
    pub height: f32,
}

/// A picture as an SVG document (viewBox 400x240).
pub fn compose_svg(c: &Composition, style: PictureStyle, sheet: &CharacterSheet) -> String {
    compose(c, style, sheet).0
}

/// The picture and where each thing of the scene stands.
pub fn compose(c: &Composition, style: PictureStyle, sheet: &CharacterSheet) -> (String, Vec<Placed>) {
    let mut c = c.clone();
    if style == PictureStyle::NightSky {
        c.time = Time::Night;
    }
    if c.setting == Setting::Night {
        c.time = Time::Night;
    }
    if c.setting == Setting::Cave {
        // a cave is lit from inside: the hour outside does not darken it
        c.time = Time::Day;
    }
    c.objects.retain(|o| allowed(o.kind, c.setting));
    c.objects.truncate(super::MAX_OBJECTS);
    // the story's hero is in the picture when asked for; a companion animal keeps its look from the sheet
    for o in &mut c.objects {
        if sheet.companion_kind() == Some(o.kind) && o.color.is_none() && !sheet.companion_color.is_empty() {
            o.color = super::colour(&sheet.companion_color);
        }
    }
    let seed = fnv(&format!("{:?}{:?}{}", c, style, sheet.name));
    let mut a = Art::new(style, c.time, seed);
    let h = a.backdrop(&c);
    let setting = c.setting;
    let objs = &c.objects;
    // things in the sky
    for (i, o) in objs.iter().enumerate() {
        match o.kind {
            Kind::Cloud => {
                let (x, y) = (o.x * 4.0, o.y.map_or(26.0 + (i % 3) as f32 * 14.0, |v| v * 2.4));
                a.cloud(x.clamp(40.0, 360.0), y.clamp(20.0, h - 30.0), 0.8 * o.size.mul());
            }
            Kind::Bird => {
                let (x, y) = (o.x * 4.0, o.y.map_or(48.0 + (i % 3) as f32 * 12.0, |v| v * 2.4));
                a.open(x.clamp(30.0, 370.0), y.clamp(18.0, h - 20.0), o.size.mul(), false);
                a.birds(o.color);
                a.close();
            }
            Kind::Planet => {
                let (x, y) = (o.x * 4.0, o.y.map_or(60.0, |v| v * 2.4));
                a.open(x.clamp(50.0, 350.0), y.clamp(34.0, 130.0), 0.8 * o.size.mul(), false);
                a.planet(o.color);
                a.close();
            }
            _ => {}
        }
    }
    // water: lakes and rivers first (things stand on and by them)
    let mut waters: Vec<(f32, f32, f32)> = Vec::new();
    for o in objs.iter().filter(|o| matches!(o.kind, Kind::Lake | Kind::River)) {
        if o.kind == Kind::River {
            let x = (o.x * 4.0).clamp(70.0, 330.0);
            a.river(x, h + 4.0, 0.8 + 0.2 * o.size.mul());
            waters.push((x - 10.0, h + 0.55 * (H - h), 40.0));
        } else {
            let (rx, ry) = (60.0 * o.size.mul(), 17.0 * o.size.mul());
            let x = (o.x * 4.0).clamp(rx + 6.0, 394.0 - rx);
            let y = h + 0.5 * (H - h) + (waters.len() as f32) * 10.0;
            a.open(x, y.min(H - ry - 8.0), 1.0, false);
            a.lake(rx, ry, o.color);
            a.close();
            waters.push((x, y, rx));
        }
    }
    let sea = matches!(setting, Setting::Sea | Setting::Beach);
    // boats and fish with no water get a small lake of their own
    let needs_water = objs.iter().any(|o| matches!(o.kind, Kind::Boat | Kind::Fish));
    if needs_water && waters.is_empty() && !sea && !matches!(setting, Setting::River) {
        let o = objs.iter().find(|o| matches!(o.kind, Kind::Boat | Kind::Fish)).map(|o| o.x).unwrap_or(50.0);
        let x = (o * 4.0).clamp(80.0, 320.0);
        let y = h + 0.55 * (H - h);
        a.open(x, y, 1.0, false);
        a.lake(78.0, 20.0, None);
        a.close();
        waters.push((x, y, 78.0));
    }
    if matches!(setting, Setting::River) && waters.is_empty() {
        waters.push((190.0, h + 0.55 * (H - h), 40.0));
    }
    // the others, far to near
    struct Item<'a> {
        o: &'a Obj,
        foot: f32,
        x: f32,
        scale: f32,
        idx: usize,
    }
    let mut items: Vec<Item> = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        if matches!(o.kind, Kind::Sun | Kind::Moon | Kind::Stars | Kind::Cloud | Kind::Bird | Kind::Planet | Kind::Lake | Kind::River | Kind::Rain | Kind::Snow | Kind::Fireflies) {
            continue;
        }
        let (hw, lh, rf, d) = spec(o.kind);
        let jitter = ((i * 37 % 5) as f32 - 2.0) * 0.015;
        let mut x = o.x * 4.0;
        let mut foot = h + (d + jitter).clamp(0.0, 1.0) * (H - 8.0 - h);
        let mut persp = 0.62 + 0.38 * ((foot - h) / (H - 8.0 - h)).clamp(0.0, 1.0);
        match o.kind {
            Kind::Boat | Kind::Fish => {
                let w = waters.iter().min_by(|p, q| (p.0 - x).abs().total_cmp(&(q.0 - x).abs())).copied();
                if let Some((wx, wy, wr)) = w.filter(|_| !sea) {
                    x = x.clamp(wx - wr * 0.55, wx + wr * 0.55);
                    foot = wy + if o.kind == Kind::Boat { 3.0 } else { 2.0 };
                } else {
                    foot = h + if o.kind == Kind::Boat { 22.0 } else { 40.0 } + (i % 3) as f32 * 8.0;
                }
                persp = 0.8;
            }
            Kind::Bridge => {
                if let Some((wx, wy, _)) = waters.first().copied() {
                    x = wx;
                    foot = wy + 14.0;
                }
            }
            Kind::Rocket if o.y.is_some_and(|y| y < 70.0) => {
                foot = o.y.unwrap_or(40.0) * 2.4 + 50.0;
                persp = 0.8;
            }
            Kind::Mountain => foot = h + 8.0,
            _ => {}
        }
        let mut scale = rf / (lh.max(1.0)) * o.size.mul() * persp;
        let maxh = foot - 6.0;
        if lh * scale > maxh {
            scale = maxh / lh;
        }
        let half = hw * scale;
        if half * 2.0 > W - 8.0 {
            scale = (W - 8.0) / (2.0 * hw);
        }
        let half = hw * scale;
        x = x.clamp(half + 4.0, W - half - 4.0);
        foot = foot.clamp(10.0, H - 3.0);
        items.push(Item { o, foot, x, scale, idx: i });
    }
    // far objects (mountains, hills) first, then by distance
    items.sort_by(|p, q| (p.o.kind != Kind::Mountain && p.o.kind != Kind::Hill).cmp(&(q.o.kind != Kind::Mountain && q.o.kind != Kind::Hill)).then(p.foot.total_cmp(&q.foot)).then(p.x.total_cmp(&q.x)));
    let placed: Vec<Placed> = items.iter().filter(|i| !(i.o.kind == Kind::Hero && !sheet.human)).map(|i| { let (hw, lh, _, _) = spec(i.o.kind); Placed { kind: i.o.kind, x: i.x, foot: i.foot, scale: i.scale, half_width: hw * i.scale, height: lh * i.scale } }).collect();
    let hero_x = items.iter().find(|i| i.o.kind == Kind::Hero).map(|i| i.x);
    let focus_for = |it: &Item| -> f32 {
        // look towards the nearest interesting thing (or, for animals, towards the hero), else towards the middle
        if it.o.kind.is_animal() {
            if let Some(hx) = hero_x {
                return hx;
            }
        }
        let mut best: Option<(f32, f32)> = None;
        for j in &items {
            if j.idx == it.idx || matches!(j.o.kind, Kind::Hill | Kind::Mountain | Kind::Flower | Kind::Bush | Kind::Fence | Kind::Tree | Kind::Palm | Kind::Rock | Kind::Mushroom) {
                continue;
            }
            let d = (j.x - it.x).abs();
            if d > 12.0 && best.is_none_or(|b| d < b.0) {
                best = Some((d, j.x));
            }
        }
        best.map_or(200.0, |b| b.1)
    };
    let mood_expr = match c.mood {
        Mood::Happy | Mood::Calm => Expr::Happy,
        Mood::Mysterious | Mood::Exciting => Expr::Surprised,
        Mood::Sad => Expr::Scared,
    };
    for it in &items {
        let (o, x, f, s) = (it.o, it.x, it.foot, it.scale);
        let face_right = focus_for(it) >= x;
        let flip = !face_right;
        let col = o.color;
        match o.kind {
            k if k.is_person() => {
                let focus = focus_for(it);
                let far = (focus - x).abs();
                let pose = o.pose.unwrap_or(if far > 40.0 && it.o.kind == Kind::Hero { Pose::Walk } else { Pose::Stand });
                let expr = o.expr.unwrap_or(mood_expr);
                let holds = o.holds.or(if k == Kind::Hero && setting == Setting::Cave { Some(Prop::Torch) } else if k == Kind::Hero && c.time == Time::Night && matches!(setting, Setting::Forest | Setting::Jungle | Setting::Village | Setting::Night) { Some(Prop::Lantern) } else { None });
                let mut p = if k == Kind::Hero { Person::hero(sheet, pose, expr, holds) } else { Person::other(k, (x as u32) / 7 + it.idx as u32, col, pose, expr, holds) };
                if !sheet.human && k == Kind::Hero {
                    continue;
                }
                p.gaze = 1.0;
                a.open(x, f, s, flip);
                a.person(&p);
                a.close();
            }
            _ => {
                let rot = if o.kind == Kind::Rocket && o.y.is_some_and(|y| y < 70.0) { " rotate(18)" } else { "" };
                a.open_raw(&format!("transform=\"translate({} {}) scale({} {}){rot}\"", n(x), n(f), n2s(if flip && (o.kind.is_animal()) { -s } else { s }), n2s(s)));
                match o.kind {
                    Kind::Tree => a.tree(col, setting),
                    Kind::Palm => a.palm(),
                    Kind::Bush => a.bush(col),
                    Kind::Flower => a.flower(col),
                    Kind::House => a.house(col),
                    Kind::Cave => a.cave_mouth(col),
                    Kind::Mountain => a.mountain(col),
                    Kind::Hill => a.hill_mound(col),
                    Kind::Boat => a.boat(col),
                    Kind::Bridge => a.bridge(),
                    Kind::Castle => a.castle(col),
                    Kind::Rocket => a.rocket(col),
                    Kind::Fire => a.fire(),
                    Kind::Crystal => a.crystals(0.0, 0.0, 1.0, col.unwrap_or([90, 220, 255])),
                    Kind::Mushroom => a.mushroom(col),
                    Kind::Rock => a.rock(col),
                    Kind::Cactus => a.cactus(col),
                    Kind::Barn => a.barn(col),
                    Kind::Fence => a.fence(),
                    Kind::Tent => a.tent(col),
                    Kind::Lantern => a.lantern_post(),
                    Kind::Cat => a.cat(col),
                    Kind::Dog => a.dog(col),
                    Kind::Goat => a.goat(col),
                    Kind::Bear => a.bear(col),
                    Kind::Rabbit => a.rabbit(col),
                    Kind::Elephant => a.elephant(col),
                    Kind::Fish => {
                        a.fish_body(col);
                        for (bx, by, r) in [(-18.0, -12.0, 1.8), (-24.0, -20.0, 1.3)] {
                            a.raw(&format!("<circle cx=\"{bx}\" cy=\"{by}\" r=\"{r}\" fill=\"none\" stroke=\"#ffffff\" stroke-width=\"0.8\" opacity=\"0.8\"/>"));
                        }
                    }
                    _ => {}
                }
                a.close();
            }
        }
    }
    if objs.iter().any(|o| o.kind == Kind::Fireflies) {
        a.fireflies(10, h - 40.0, h + 50.0);
    }
    a.foreground(setting);
    if objs.iter().any(|o| o.kind == Kind::Rain) || matches!(c.time, Time::Storm) {
        a.rain();
    }
    if objs.iter().any(|o| o.kind == Kind::Snow) || setting == Setting::Snow {
        a.snowfall();
    }
    a.finishing(c.mood);
    let svg = a.finish();
    let svg = if style == PictureStyle::Pixel { pixelate(&svg).unwrap_or(svg) } else { svg };
    (svg, placed)
}

fn n2s(x: f32) -> String {
    format!("{x:.3}")
}

/// Pixel art from the picture itself: it is drawn small, its colours are cut down to a small palette, and every run of
/// equal pixels becomes one rectangle (merged downwards too), so the file stays small and the pixels stay square.
pub fn pixelate(svg: &str) -> Option<String> {
    let (gw, gh) = (100usize, 60usize);
    let bm = iyakki_web::svg::render(svg.as_bytes(), Some((gw as u32, gh as u32))).ok()?;
    // colours cut to 7 levels per channel, then lone speckles (a pixel unlike all its neighbours) take the colour around them
    let q = |v: u8| -> u8 { (((v as u32 + 21) / 43) * 43).min(255) as u8 };
    let mut raw: Vec<[u8; 3]> = Vec::with_capacity(gw * gh);
    for i in 0..gw * gh {
        let p = &bm.rgba[i * 4..i * 4 + 4];
        raw.push([q(p[0]), q(p[1]), q(p[2])]);
    }
    let mut grid = raw.clone();
    for y in 1..gh - 1 {
        for x in 1..gw - 1 {
            let c = raw[y * gw + x];
            let mut same = 0;
            let mut seen: Vec<([u8; 3], u32)> = Vec::new();
            for dy in 0..3 {
                for dx in 0..3 {
                    let n = raw[(y + dy - 1) * gw + x + dx - 1];
                    if (dx, dy) != (1, 1) {
                        if n == c {
                            same += 1;
                        }
                        match seen.iter_mut().find(|s| s.0 == n) {
                            Some(s) => s.1 += 1,
                            None => seen.push((n, 1)),
                        }
                    }
                }
            }
            if same == 0 {
                if let Some(best) = seen.iter().max_by_key(|s| s.1) {
                    grid[y * gw + x] = best.0;
                }
            }
        }
    }
    // runs per row, then merged with the identical run of the row above
    struct R {
        x: usize,
        w: usize,
        y: usize,
        h: usize,
        c: [u8; 3],
    }
    let mut done: Vec<R> = Vec::new();
    let mut open: Vec<R> = Vec::new();
    for y in 0..gh {
        let mut row: Vec<R> = Vec::new();
        let mut x = 0;
        while x < gw {
            let c = grid[y * gw + x];
            let mut e = x + 1;
            while e < gw && grid[y * gw + e] == c {
                e += 1;
            }
            row.push(R { x, w: e - x, y, h: 1, c });
            x = e;
        }
        let mut next: Vec<R> = Vec::new();
        for r in row {
            if let Some(pos) = open.iter().position(|o| o.x == r.x && o.w == r.w && o.c == r.c && o.y + o.h == y) {
                let mut o = open.swap_remove(pos);
                o.h += 1;
                next.push(o);
            } else {
                next.push(r);
            }
        }
        done.append(&mut open);
        open = next;
    }
    done.append(&mut open);
    done.sort_by_key(|r| (r.y, r.x));
    // the most common colour is laid under everything, and each rectangle reaches 3 units into its neighbours, so no seam shows
    // when the picture is drawn at a size that does not fall on whole pixels
    let mut count: std::collections::HashMap<[u8; 3], usize> = std::collections::HashMap::new();
    for c in &grid {
        *count.entry(*c).or_insert(0) += 1;
    }
    let under = count.iter().max_by_key(|(c, n)| (**n, **c)).map_or([0, 0, 0], |(c, _)| *c);
    let mut out = String::from("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 400 240\">");
    out.push_str(&format!("<rect x=\"0\" y=\"0\" width=\"400\" height=\"240\" fill=\"{}\"/>", hex(under)));
    for r in done {
        out.push_str(&format!("<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>", r.x * 4, r.y * 4, r.w * 4 + 3, r.h * 4 + 3, hex(r.c)));
    }
    out.push_str("</svg>");
    Some(out)
}
