//! The drawing engine of the scene composer: colours, a deterministic random source, and the vector primitives every
//! picture is built from (outlined shapes, blob clusters, gradients, soft shadows). The look of a style (soft watercolour,
//! outlined cartoon, bold comic, pixel, night sky) is applied here, so every object drawn through [`Art`] matches.

use crate::story::PictureStyle;

pub type Col = [u8; 3];

pub const W: f32 = 400.0;
pub const H: f32 = 240.0;
const INK: Col = [0x1b, 0x14, 0x2e];

pub fn hex(c: Col) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

pub fn rgb(h: u32) -> Col {
    [(h >> 16) as u8, (h >> 8) as u8, h as u8]
}

pub fn mix(a: Col, b: Col, t: f32) -> Col {
    let t = t.clamp(0.0, 1.0);
    [0, 1, 2].map(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8)
}

pub fn lighten(c: Col, t: f32) -> Col {
    mix(c, [255, 255, 255], t)
}

pub fn darken(c: Col, t: f32) -> Col {
    mix(c, [14, 10, 36], t)
}

/// A number written short: one decimal at most, no trailing zero.
pub fn n(x: f32) -> String {
    let x = if x.is_finite() { x } else { 0.0 };
    let s = format!("{x:.1}");
    s.strip_suffix(".0").map_or(s.clone(), str::to_string)
}

/// A small deterministic random source: the same seed always draws the same picture.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64) as f32
    }
    pub fn between(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.next()
    }
}

/// Fill and outline of a shape, as attribute text.
#[derive(Clone)]
pub struct Pn {
    pub fill: String,
    pub st: String,
}

impl Pn {
    pub fn alpha(mut self, a: f32) -> Pn {
        self.st.push_str(&format!(" opacity=\"{}\"", n2(a)));
        self
    }
}

fn n2(x: f32) -> String {
    format!("{x:.2}")
}

/// Time of day, which sets the sky and tints everything under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Time {
    Day,
    Dawn,
    Sunset,
    Night,
    Storm,
    Cloudy,
}

impl Time {
    pub fn parse(s: &str) -> Option<Time> {
        let s = s.to_lowercase();
        let has = |w: &[&str]| w.iter().any(|x| s.contains(x));
        Some(if has(&["night", "dark", "midnight", "star", "moonlit"]) {
            Time::Night
        } else if has(&["sunset", "dusk", "evening", "twilight", "golden"]) {
            Time::Sunset
        } else if has(&["dawn", "sunrise", "morning", "early"]) {
            Time::Dawn
        } else if has(&["storm", "rain", "thunder"]) {
            Time::Storm
        } else if has(&["cloud", "overcast", "grey", "gray", "fog", "mist"]) {
            Time::Cloudy
        } else if has(&["day", "noon", "sun", "bright", "clear", "afternoon", "blue"]) {
            Time::Day
        } else {
            return None;
        })
    }

    pub fn sky(self) -> [Col; 3] {
        match self {
            Time::Day => [rgb(0x4a90d9), rgb(0x8cc4ef), rgb(0xd6efff)],
            Time::Dawn => [rgb(0x5b6fc4), rgb(0xf2a6b0), rgb(0xffe0b0)],
            Time::Sunset => [rgb(0x2f3b8f), rgb(0xd9648a), rgb(0xffb061)],
            Time::Night => [rgb(0x070b2a), rgb(0x151f5c), rgb(0x3a3f86)],
            Time::Storm => [rgb(0x3b4452), rgb(0x6b7684), rgb(0xa3acb6)],
            Time::Cloudy => [rgb(0x7fa3c4), rgb(0xb4cadb), rgb(0xe1eaf0)],
        }
    }

    fn tint(self) -> (Col, f32) {
        match self {
            Time::Day => ([255, 255, 255], 0.0),
            Time::Dawn => (rgb(0xffb08a), 0.10),
            Time::Sunset => (rgb(0xff7a50), 0.17),
            Time::Night => (rgb(0x1a2658), 0.42),
            Time::Storm => (rgb(0x4a5568), 0.26),
            Time::Cloudy => (rgb(0x8a96a8), 0.10),
        }
    }
}

/// The drawing surface: an SVG body being written, gradient definitions, the style and the colour tint of the hour.
pub struct Art {
    pub out: String,
    pub defs: String,
    pub style: PictureStyle,
    pub time: Time,
    pub rng: Rng,
    uid: u32,
    ink_w: f32,
    ink_mix: f32,
    ink_op: f32,
    lift: f32,
}

impl Art {
    pub fn new(style: PictureStyle, time: Time, seed: u64) -> Art {
        let (ink_w, ink_mix, ink_op, lift) = match style {
            PictureStyle::Watercolor => (1.1, 0.42, 0.55, 0.10),
            PictureStyle::Comic => (2.4, 0.86, 1.0, 0.0),
            PictureStyle::Pixel => (0.0, 0.0, 0.0, 0.0),
            PictureStyle::NightSky => (1.0, 0.5, 0.45, 0.0),
        };
        Art { out: String::new(), defs: String::new(), style, time, rng: Rng(seed | 1), uid: 0, ink_w, ink_mix, ink_op, lift }
    }

    /// A colour as the hour and the style show it.
    pub fn tone(&self, c: Col) -> Col {
        let (t, a) = self.time.tint();
        let c = if self.lift > 0.0 { lighten(c, self.lift) } else { c };
        mix(c, t, a)
    }

    pub fn pn(&self, c: Col) -> Pn {
        let c = self.tone(c);
        Pn { fill: hex(c), st: self.stroke(c) }
    }

    /// No outline (washes, glows, shading).
    pub fn flat(&self, c: Col) -> Pn {
        Pn { fill: hex(self.tone(c)), st: String::new() }
    }

    fn stroke(&self, c: Col) -> String {
        if self.ink_w <= 0.0 {
            return String::new();
        }
        let op = if self.ink_op < 1.0 { format!(" stroke-opacity=\"{}\"", n2(self.ink_op)) } else { String::new() };
        format!(" stroke=\"{}\" stroke-width=\"{}\" stroke-linejoin=\"round\" stroke-linecap=\"round\"{op}", hex(mix(c, INK, self.ink_mix)), n(self.ink_w))
    }

    pub fn ink_width(&self) -> f32 {
        self.ink_w
    }

    pub fn id(&mut self) -> String {
        self.uid += 1;
        format!("g{}", self.uid)
    }

    /// A vertical (or sloped) gradient between colours; returns the `url(#id)` to fill with.
    pub fn lin(&mut self, stops: &[Col], x2: f32, y2: f32) -> String {
        let id = self.id();
        let mut s = format!("<linearGradient id=\"{id}\" x1=\"0\" y1=\"0\" x2=\"{}\" y2=\"{}\">", n2(x2), n2(y2));
        let k = stops.len().max(2) - 1;
        for (i, c) in stops.iter().enumerate() {
            s.push_str(&format!("<stop offset=\"{}\" stop-color=\"{}\"/>", n2(i as f32 / k as f32), hex(self.tone(*c))));
        }
        s.push_str("</linearGradient>");
        self.defs.push_str(&s);
        format!("url(#{id})")
    }

    /// A glow: colour fading to nothing from the centre.
    pub fn glow(&mut self, c: Col, a0: f32) -> String {
        let id = self.id();
        let c = hex(self.tone(c));
        self.defs.push_str(&format!("<radialGradient id=\"{id}\" cx=\"0.5\" cy=\"0.5\" r=\"0.5\"><stop offset=\"0\" stop-color=\"{c}\" stop-opacity=\"{}\"/><stop offset=\"0.5\" stop-color=\"{c}\" stop-opacity=\"{}\"/><stop offset=\"1\" stop-color=\"{c}\" stop-opacity=\"0\"/></radialGradient>", n2(a0), n2(a0 * 0.35)));
        format!("url(#{id})")
    }

    // ---- primitives

    pub fn raw(&mut self, s: &str) {
        self.out.push_str(s);
    }

    pub fn path(&mut self, d: &str, p: &Pn) {
        self.out.push_str(&format!("<path d=\"{d}\" fill=\"{}\"{}/>", p.fill, p.st));
    }

    pub fn circ(&mut self, cx: f32, cy: f32, r: f32, p: &Pn) {
        self.out.push_str(&format!("<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\"{}/>", n(cx), n(cy), n(r), p.fill, p.st));
    }

    pub fn ell(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, p: &Pn) {
        self.out.push_str(&format!("<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" fill=\"{}\"{}/>", n(cx), n(cy), n(rx), n(ry), p.fill, p.st));
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, rx: f32, p: &Pn) {
        self.out.push_str(&format!("<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\" fill=\"{}\"{}/>", n(x), n(y), n(w), n(h), n(rx), p.fill, p.st));
    }

    pub fn poly(&mut self, pts: &[(f32, f32)], p: &Pn) {
        let s: Vec<String> = pts.iter().map(|(x, y)| format!("{},{}", n(*x), n(*y))).collect();
        self.out.push_str(&format!("<polygon points=\"{}\" fill=\"{}\"{}/>", s.join(" "), p.fill, p.st));
    }

    /// A stroked line (no fill).
    pub fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, c: Col, w: f32) {
        self.out.push_str(&format!("<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"{}\" stroke-linecap=\"round\"/>", n(x1), n(y1), n(x2), n(y2), hex(self.tone(c)), n(w)));
    }

    /// A rounded limb (a thick line with round ends) with the outline of the style around it.
    pub fn limb(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, w: f32, c: Col) {
        let c = self.tone(c);
        if self.ink_w > 0.0 {
            let ink = hex(mix(c, INK, self.ink_mix));
            let op = if self.ink_op < 1.0 { format!(" opacity=\"{}\"", n2(self.ink_op)) } else { String::new() };
            self.out.push_str(&format!("<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{ink}\" stroke-width=\"{}\" stroke-linecap=\"round\"{op}/>", n(x1), n(y1), n(x2), n(y2), n(w + self.ink_w * 1.8)));
        }
        self.out.push_str(&format!("<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"{}\" stroke-linecap=\"round\"/>", n(x1), n(y1), n(x2), n(y2), hex(c), n(w)));
    }

    /// A stroked curve (no fill).
    pub fn curve(&mut self, d: &str, c: Col, w: f32, op: f32) {
        self.out.push_str(&format!("<path d=\"{d}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{}\" stroke-linecap=\"round\" stroke-linejoin=\"round\" opacity=\"{}\"/>", hex(self.tone(c)), n(w), n2(op)));
    }

    pub fn open(&mut self, x: f32, y: f32, s: f32, flip: bool) {
        let sx = if flip { -s } else { s };
        self.out.push_str(&format!("<g transform=\"translate({} {}) scale({} {})\">", n(x), n(y), n2(sx), n2(s)));
    }

    pub fn open_raw(&mut self, attrs: &str) {
        self.out.push_str(&format!("<g {attrs}>"));
    }

    pub fn close(&mut self) {
        self.out.push_str("</g>");
    }

    /// A cluster of round blobs drawn as one outlined shape with a lit side and a shaded side (foliage, bushes, clouds).
    pub fn blobs(&mut self, circles: &[(f32, f32, f32)], c: Col) {
        let base = self.tone(c);
        if self.ink_w > 0.0 {
            let ink = hex(mix(base, INK, self.ink_mix));
            let w = self.ink_w;
            let op = self.ink_op;
            let mut s = String::new();
            for (x, y, r) in circles {
                s.push_str(&format!("<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{ink}\"/>", n(*x), n(*y), n(r + w * 0.9)));
            }
            if op < 1.0 {
                s = format!("<g opacity=\"{}\">{s}</g>", n2(op));
            }
            self.out.push_str(&s);
        }
        let shade = hex(mix(base, [10, 20, 60], 0.28));
        let lit = hex(mix(base, [255, 255, 240], 0.30));
        let b = hex(base);
        let mut s = String::new();
        for (x, y, r) in circles {
            s.push_str(&format!("<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{shade}\"/>", n(*x + r * 0.10), n(*y + r * 0.12), n(*r)));
        }
        for (x, y, r) in circles {
            s.push_str(&format!("<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{b}\"/>", n(*x), n(*y), n(r * 0.92)));
        }
        for (x, y, r) in circles {
            s.push_str(&format!("<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{lit}\" opacity=\"0.55\"/>", n(*x - r * 0.25), n(*y - r * 0.28), n(r * 0.5)));
        }
        self.out.push_str(&s);
    }

    /// A soft shadow on the ground under something `hw` wide.
    pub fn shadow(&mut self, x: f32, y: f32, hw: f32) {
        let p = self.flat([20, 25, 50]).alpha(0.13);
        self.ell(x, y, hw * 1.15, hw * 0.26, &p);
        let p = self.flat([20, 25, 50]).alpha(0.16);
        self.ell(x, y, hw * 0.8, hw * 0.17, &p);
    }

    /// A wavy line across the frame as a path `M x0,y ... L x1,bottom L x0,bottom Z` (the area under it).
    pub fn hill_path(&mut self, y: f32, amp: f32, waves: f32, phase: f32, bottom: f32) -> String {
        let steps = 16;
        let mut pts = Vec::new();
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let yy = y + amp * ((t * waves * std::f32::consts::TAU + phase).sin() * 0.6 + (t * waves * 2.3 * std::f32::consts::TAU + phase * 1.7).sin() * 0.4);
            pts.push((t * (W + 20.0) - 10.0, yy));
        }
        let mut d = format!("M{},{}", n(pts[0].0), n(pts[0].1));
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let mx = (a.0 + b.0) / 2.0;
            d.push_str(&format!(" Q{},{} {},{}", n(a.0), n(a.1), n(mx), n((a.1 + b.1) / 2.0)));
        }
        let last = pts[pts.len() - 1];
        d.push_str(&format!(" L{},{} L{},{} L{},{} Z", n(last.0), n(last.1), n(last.0), n(bottom), n(pts[0].0), n(bottom)));
        d
    }

    /// The whole picture as an SVG document.
    pub fn finish(self) -> String {
        format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 400 240\"><defs>{}</defs>{}</svg>", self.defs, self.out)
    }
}
