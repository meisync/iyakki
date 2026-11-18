//! Backgrounds of the scene composer: the sky of the hour with its sun, moon, stars, clouds and light rays, distant
//! mountains and hills, and a layered setting for each kind of place (jungle, glowing cave, village, farm, beach, sea, river,
//! desert, snow, mountains, space, garden, classroom, room, night field). Layers run far to near with haze between them, so
//! the picture has depth; the foreground frames the scene from the bottom corners.

use super::art::*;
use super::{Composition, Kind, Setting};
use crate::story::PictureStyle;
use std::f32::consts::PI;

pub const GREEN: Col = [0x55, 0xa8, 0x55];

impl Art {
    pub fn sky(&mut self, bottom: f32) {
        let s = self.time.sky();
        let g = self.lin(&s, 0.0, 1.0);
        self.raw(&format!("<rect x=\"0\" y=\"0\" width=\"400\" height=\"{}\" fill=\"{g}\"/>", n(bottom + 2.0)));
    }

    pub fn sun(&mut self, x: f32, y: f32, r: f32, col: Col) {
        let g = self.glow(lighten(col, 0.3), 0.6);
        self.raw(&format!("<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{g}\"/>", n(x), n(y), n(r * 3.4)));
        for i in 0..12 {
            let a = i as f32 * PI / 6.0 + 0.2;
            let (c, s) = (a.cos(), a.sin());
            let (r0, r1) = (r * 1.25, r * if i % 2 == 0 { 1.85 } else { 1.55 });
            self.line(x + c * r0, y + s * r0, x + c * r1, y + s * r1, lighten(col, 0.25), (r * 0.12).max(1.2));
        }
        let p = self.pn(col);
        self.circ(x, y, r, &p);
        let p = self.flat(lighten(col, 0.55)).alpha(0.6);
        self.circ(x - r * 0.3, y - r * 0.3, r * 0.45, &p);
    }

    pub fn moon(&mut self, x: f32, y: f32, r: f32) {
        let g = self.glow([200, 215, 255], 0.5);
        self.raw(&format!("<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{g}\"/>", n(x), n(y), n(r * 3.6)));
        let p = self.pn([246, 240, 208]);
        self.circ(x, y, r, &p);
        for (dx, dy, k) in [(-0.3, -0.2, 0.28), (0.25, 0.25, 0.2), (0.3, -0.35, 0.14), (-0.2, 0.4, 0.12)] {
            let p = self.flat([220, 212, 176]).alpha(0.7);
            self.circ(x + dx * r, y + dy * r, k * r, &p);
        }
    }

    pub fn stars(&mut self, count: usize, ymax: f32) {
        for i in 0..count {
            let (x, y) = (self.rng.between(4.0, 396.0), self.rng.between(4.0, ymax));
            let r = self.rng.between(0.6, 1.7);
            if i % 7 == 0 {
                let k = r * 3.4;
                let p = self.flat([255, 250, 220]);
                self.poly(&[(x, y - k), (x + k * 0.22, y - k * 0.22), (x + k, y), (x + k * 0.22, y + k * 0.22), (x, y + k), (x - k * 0.22, y + k * 0.22), (x - k, y), (x - k * 0.22, y - k * 0.22)], &p);
            } else {
                self.raw(&format!("<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"#fffbe6\" opacity=\"0.9\"/>", n(x), n(y), n(r)));
            }
        }
    }

    pub fn cloud(&mut self, x: f32, y: f32, s: f32) {
        let base = match self.time {
            Time::Storm => [150, 156, 168],
            _ => [255, 255, 255],
        };
        self.open(x, y, s, false);
        self.blobs(&[(0.0, 0.0, 12.0), (15.0, -8.0, 16.0), (32.0, -3.0, 13.0), (-14.0, 3.0, 9.0), (44.0, 3.0, 9.0)], base);
        self.raw("</g>");
    }

    /// Soft shafts of light falling from a point above the frame.
    pub fn rays(&mut self, x: f32, strength: f32) {
        let g = self.lin(&[[255, 250, 200], [255, 250, 200]], 0.0, 1.0);
        for i in 0..5 {
            let o = i as f32 * 26.0 - 40.0;
            let a = 0.05 + strength * 0.07 * (1.0 - (i as f32 - 2.0).abs() / 3.0);
            self.raw(&format!("<polygon points=\"{},0 {},0 {},240 {},240\" fill=\"{g}\" opacity=\"{}\"/>", n(x + o), n(x + o + 14.0), n(x + o + 70.0 + i as f32 * 14.0), n(x + o + 10.0), n2f(a)));
        }
    }

    pub fn fog(&mut self, y: f32, h: f32, op: f32) {
        let c = self.time.sky()[2];
        let g = self.glow(c, op);
        self.raw(&format!("<ellipse cx=\"200\" cy=\"{}\" rx=\"260\" ry=\"{}\" fill=\"{g}\"/>", n(y), n(h)));
    }

    /// A jagged mountain range with shaded sides and, optionally, snow on the peaks.
    pub fn range(&mut self, base: f32, height: f32, col: Col, snow: bool, count: usize) {
        let mut peaks = Vec::new();
        for i in 0..count {
            let x = (i as f32 + 0.5 + self.rng.between(-0.25, 0.25)) * W / count as f32;
            peaks.push((x, base - height * self.rng.between(0.6, 1.0)));
        }
        let mut pts = vec![(-10.0, base)];
        for (i, p) in peaks.iter().enumerate() {
            let prev = if i == 0 { -10.0 } else { peaks[i - 1].0 };
            pts.push(((prev + p.0) / 2.0 + self.rng.between(-6.0, 6.0), base - height * self.rng.between(0.1, 0.3)));
            pts.push(*p);
        }
        pts.push((410.0, base));
        pts.push((410.0, base + 30.0));
        pts.push((-10.0, base + 30.0));
        let p = self.pn(col);
        self.poly(&pts, &p);
        for pk in &peaks {
            let sh = self.flat(darken(col, 0.25)).alpha(0.55);
            self.poly(&[*pk, (pk.0 + height * 0.55, base), (pk.0 + height * 0.1, base)], &sh);
            if snow {
                let k = height * 0.28;
                let cap = self.flat([248, 250, 255]);
                self.poly(&[*pk, (pk.0 + k * 0.9, pk.1 + k), (pk.0 + k * 0.45, pk.1 + k * 0.78), (pk.0 + k * 0.1, pk.1 + k * 1.1), (pk.0 - k * 0.4, pk.1 + k * 0.8), (pk.0 - k * 0.9, pk.1 + k)], &cap);
            }
        }
    }

    /// A rolling hill layer: its top edge waves, the colour fades to the haze with distance.
    pub fn hills(&mut self, y: f32, amp: f32, waves: f32, col: Col, bottom: f32) {
        let ph = self.rng.between(0.0, 6.0);
        let d = self.hill_path(y, amp, waves, ph, bottom);
        let p = self.pn(col);
        self.path(&d, &p);
        // a lit crest line: the washes of a painted hill
        let d2 = self.hill_path(y + amp * 0.9 + 6.0, amp * 0.8, waves, ph + 1.0, bottom);
        let p = self.flat(darken(col, 0.12)).alpha(0.35);
        self.path(&d2, &p);
    }

    /// The ground: a wavy top edge at `y`, gradient to the bottom of the frame.
    pub fn ground(&mut self, y: f32, top: Col, bottom: Col, amp: f32) {
        let d = self.hill_path(y, amp, 1.3, 0.8, H + 4.0);
        let g = self.lin(&[top, bottom], 0.0, 1.0);
        let st = self.pn(top).st;
        self.raw(&format!("<path d=\"{d}\" fill=\"{g}\"{st}/>"));
    }

    pub fn trail(&mut self, y0: f32, x0: f32, x1: f32, w1: f32, col: Col) {
        let (a, b) = (x0, x1);
        let d = format!("M{},{} C{},{} {},{} {},{} L{},{} C{},{} {},{} {},{} Z", n(a - 2.0), n(y0), n(a - 10.0), n(y0 + 30.0), n(b - w1 * 0.6), n(y0 + 55.0), n(b - w1), n(H + 4.0), n(b + w1), n(H + 4.0), n(b + w1 * 0.5), n(y0 + 55.0), n(a + 14.0), n(y0 + 30.0), n(a + 2.0), n(y0));
        let p = self.flat(col);
        self.path(&d, &p);
        let p = self.flat(lighten(col, 0.3)).alpha(0.45);
        let d2 = format!("M{},{} C{},{} {},{} {},{} L{},{} C{},{} {},{} {},{} Z", n(a), n(y0 + 2.0), n(a - 6.0), n(y0 + 30.0), n(b - w1 * 0.4), n(y0 + 60.0), n(b - w1 * 0.6), n(H + 4.0), n(b + w1 * 0.2), n(H + 4.0), n(b + w1 * 0.2), n(y0 + 60.0), n(a + 8.0), n(y0 + 30.0), n(a + 1.0), n(y0 + 2.0));
        self.path(&d2, &p);
    }

    pub fn tufts(&mut self, count: usize, y0: f32, y1: f32, col: Col) {
        for _ in 0..count {
            let (x, y) = (self.rng.between(6.0, 394.0), self.rng.between(y0, y1));
            let k = 3.0 + (y - y0) / (y1 - y0 + 1.0) * 5.0;
            let c = hex(self.tone(col));
            self.raw(&format!("<path d=\"M{},{} l-{},-{} M{},{} l0,-{} M{},{} l{},-{}\" fill=\"none\" stroke=\"{c}\" stroke-width=\"1.2\" stroke-linecap=\"round\" opacity=\"0.7\"/>", n(x), n(y), n(k * 0.4), n(k * 0.8), n(x), n(y), n(k), n(x), n(y), n(k * 0.4), n(k * 0.8)));
        }
    }

    pub fn fireflies(&mut self, count: usize, ymin: f32, ymax: f32) {
        let g = self.glow([255, 240, 140], 0.9);
        for _ in 0..count {
            let (x, y) = (self.rng.between(8.0, 392.0), self.rng.between(ymin, ymax));
            self.raw(&format!("<circle cx=\"{}\" cy=\"{}\" r=\"7\" fill=\"{g}\"/><circle cx=\"{}\" cy=\"{}\" r=\"1.3\" fill=\"#fff8c0\"/>", n(x), n(y), n(x), n(y)));
        }
    }

    /// A big curved leaf, rotated, with its vein; foreground frames and ferns.
    pub fn leaf(&mut self, x: f32, y: f32, ang: f32, len: f32, wid: f32, col: Col) {
        self.open_raw(&format!("transform=\"translate({} {}) rotate({})\"", n(x), n(y), n(ang)));
        let d = format!("M0,0 C{},{} {},{} {},0 C{},{} {},{} 0,0 Z", n(len * 0.25), n(-wid), n(len * 0.7), n(-wid * 1.1), n(len), n(len * 0.7), n(wid * 0.8), n(len * 0.25), n(wid * 0.7));
        let p = self.pn(col);
        self.path(&d, &p);
        let p = self.flat(darken(col, 0.25)).alpha(0.4);
        self.path(&format!("M0,0 C{},{} {},{} {},0 C{},{} {},{} 0,0 Z", n(len * 0.25), n(wid * 0.1), n(len * 0.7), n(wid * 0.1), n(len), n(len * 0.7), n(wid * 0.8), n(len * 0.25), n(wid * 0.7)), &p);
        self.curve(&format!("M0,0 Q{},{} {},0", n(len * 0.5), n(-wid * 0.15), n(len * 0.95)), lighten(col, 0.4), 1.0, 0.7);
        self.close();
    }

    pub fn fern(&mut self, x: f32, y: f32, s: f32, col: Col) {
        for i in 0..7 {
            let a = -150.0 + i as f32 * 20.0;
            self.leaf(x, y, a, 26.0 * s, 5.0 * s, if i % 2 == 0 { col } else { darken(col, 0.15) });
        }
    }

    fn windows_glow(&self) -> bool {
        matches!(self.time, Time::Night | Time::Sunset | Time::Storm | Time::Dawn)
    }

    /// A small distant house with a roof and a window (lit when it is dark).
    pub fn far_house(&mut self, x: f32, y: f32, s: f32, wall: Col, roof: Col) {
        self.open(x, y, s, false);
        let p = self.pn(wall);
        self.rect(-22.0, -30.0, 44.0, 30.0, 1.0, &p);
        let p = self.pn(roof);
        self.poly(&[(-27.0, -29.0), (0.0, -52.0), (27.0, -29.0)], &p);
        let sh = self.flat(darken(wall, 0.2)).alpha(0.4);
        self.rect(6.0, -30.0, 16.0, 30.0, 0.0, &sh);
        let p = self.pn([110, 70, 40]);
        self.rect(-17.0, -17.0, 9.0, 17.0, 3.0, &p);
        let lit = self.windows_glow();
        let win = if lit { [255, 220, 120] } else { [170, 210, 235] };
        let p = self.pn(win);
        self.rect(2.0, -24.0, 9.0, 9.0, 1.0, &p);
        if lit {
            let g = self.glow([255, 210, 110], 0.7);
            self.raw(&format!("<circle cx=\"6.5\" cy=\"-19\" r=\"14\" fill=\"{g}\"/>"));
        }
        let p = self.pn(darken(roof, 0.2));
        self.rect(10.0, -50.0, 6.0, 14.0, 0.0, &p);
        self.close();
    }

    /// A tree seen from far: trunk and one round crown.
    pub fn far_tree(&mut self, x: f32, y: f32, s: f32, col: Col, pine: bool) {
        self.open(x, y, s, false);
        let p = self.pn([110, 80, 55]);
        self.rect(-2.5, -14.0, 5.0, 14.0, 0.0, &p);
        if pine {
            let p = self.pn(col);
            self.poly(&[(0.0, -52.0), (13.0, -26.0), (-13.0, -26.0)], &p);
            self.poly(&[(0.0, -40.0), (17.0, -12.0), (-17.0, -12.0)], &p);
        } else {
            self.blobs(&[(0.0, -30.0, 15.0), (-11.0, -22.0, 11.0), (11.0, -22.0, 11.0), (0.0, -42.0, 10.0)], col);
        }
        self.close();
    }

    /// The setting of a picture: returns the height of the horizon (the top of the ground).
    pub fn backdrop(&mut self, c: &Composition) -> f32 {
        let st = c.setting;
        let night = matches!(self.time, Time::Night);
        let has = |k: Kind| c.objects.iter().find(|o| o.kind == k);
        match st {
            Setting::Cave => return self.cave(c),
            Setting::Classroom => return self.classroom(),
            Setting::Room => return self.room(c),
            Setting::Space => return self.space(),
            _ => {}
        }
        let h = match st {
            Setting::Jungle => 128.0,
            Setting::Forest => 134.0,
            Setting::Beach => 118.0,
            Setting::Sea => 120.0,
            Setting::Mountains => 148.0,
            Setting::Village => 142.0,
            Setting::Snow => 138.0,
            _ => 138.0,
        };
        self.sky(h + 10.0);
        let hazec = self.time.sky()[2];
        // heavenly bodies
        let low = matches!(self.time, Time::Dawn | Time::Sunset);
        let covered = matches!(st, Setting::Jungle | Setting::Forest);
        if night {
            self.stars(if covered { 30 } else { 70 }, h - 20.0);
        }
        let sun = has(Kind::Sun);
        let moon = has(Kind::Moon);
        // the sun or moon goes where nothing tall stands in front of it
        let tall: Vec<f32> = c.objects.iter().filter(|o| matches!(o.kind, Kind::Hero | Kind::Man | Kind::Woman | Kind::Elder | Kind::Tree | Kind::Palm | Kind::House | Kind::Castle | Kind::Barn | Kind::Cave | Kind::Rocket | Kind::Mountain)).map(|o| o.x * 4.0).collect();
        let free_x = [312.0f32, 96.0, 200.0, 250.0, 150.0].into_iter().max_by(|a, b| {
            let d = |x: f32| tall.iter().map(|t| (t - x).abs()).fold(1000.0f32, f32::min);
            (d(*a).min(110.0) - (a - 312.0).abs() * 0.001).total_cmp(&(d(*b).min(110.0) - (b - 312.0).abs() * 0.001))
        }).unwrap_or(312.0);
        match self.time {
            Time::Night => {
                let (x, y, r) = moon.map_or((free_x, 46.0, 17.0), |m| (m.x * 4.0, m.y.map_or(46.0, |v| v * 2.4), 14.0 * m.size.mul()));
                self.moon(x.clamp(40.0, 360.0), y.clamp(34.0, 110.0), r);
            }
            Time::Storm | Time::Cloudy => {}
            _ => {
                let (x, y, r) = sun.map_or((free_x, if low { h - 26.0 } else { 44.0 }, if low { 24.0 } else { 18.0 }), |s| (s.x * 4.0, s.y.map_or(44.0, |v| v * 2.4), 17.0 * s.size.mul()));
                let col = if low { [255, 170, 90] } else { [255, 214, 74] };
                self.sun(x.clamp(40.0, 360.0), if low { y.max(h - 40.0) } else { y.clamp(36.0, 110.0) }, r, col);
            }
        }
        // distant landscape
        let (far, mid, near) = (mix(hazec, [90, 140, 90], 0.45), mix(hazec, [80, 150, 80], 0.65), [96, 170, 90]);
        match st {
            Setting::Mountains => {
                self.range(h - 8.0, 95.0, mix(hazec, [120, 130, 170], 0.55), true, 4);
                self.range(h + 4.0, 70.0, mix(hazec, [90, 110, 150], 0.7), true, 5);
                self.fog(h - 4.0, 16.0, 0.5);
                self.hills(h + 6.0, 6.0, 1.2, mix(hazec, [70, 130, 80], 0.75), h + 40.0);
            }
            Setting::Snow => {
                self.range(h - 6.0, 60.0, mix(hazec, [150, 170, 210], 0.6), true, 4);
                self.hills(h - 2.0, 8.0, 1.1, [226, 236, 248], h + 40.0);
                self.hills(h + 6.0, 6.0, 1.6, [240, 246, 255], h + 40.0);
            }
            Setting::Desert => {
                self.range(h - 8.0, 36.0, mix(hazec, [200, 140, 100], 0.5), false, 3);
                self.hills(h - 6.0, 9.0, 0.9, mix(hazec, [226, 190, 120], 0.7), h + 40.0);
                self.hills(h + 2.0, 7.0, 1.3, [232, 200, 130], h + 40.0);
            }
            Setting::Beach | Setting::Sea => {
                self.hills(h - 4.0, 3.0, 0.6, mix(hazec, [90, 120, 120], 0.45), h);
            }
            Setting::Jungle | Setting::Forest => {
                self.hills(h - 4.0, 8.0, 1.2, mix(hazec, [60, 130, 90], 0.45), h + 20.0);
                let pine = st == Setting::Forest;
                for i in 0..9 {
                    let x = i as f32 * 48.0 + self.rng.between(-10.0, 10.0);
                    let (dy, sc) = (self.rng.between(-2.0, 4.0), 1.2 + self.rng.between(0.0, 0.5));
                    self.far_tree(x, h + dy, sc, mix(hazec, [50, 120, 80], 0.5), pine && i % 2 == 0);
                }
                self.fog(h - 6.0, 22.0, 0.55);
            }
            Setting::Village => {
                self.range(h - 6.0, 38.0, mix(hazec, [120, 140, 170], 0.5), false, 3);
                self.hills(h - 6.0, 8.0, 1.0, far, h + 30.0);
                self.hills(h + 2.0, 6.0, 1.4, mid, h + 30.0);
                let roofs = [[200, 90, 70], [180, 110, 70], [170, 80, 90]];
                let walls = [[238, 218, 180], [225, 200, 170], [240, 230, 200], [210, 190, 160]];
                for i in 0..5 {
                    let x = 40.0 + i as f32 * 78.0 + self.rng.between(-14.0, 14.0);
                    self.far_house(x, h + 4.0 + (i % 2) as f32 * 4.0, 0.75 + (i % 3) as f32 * 0.12, walls[i % 4], roofs[i % 3]);
                }
            }
            _ => {
                self.hills(h - 8.0, 12.0, 0.9, far, h + 30.0);
                self.hills(h - 2.0, 9.0, 1.3, mid, h + 30.0);
                if matches!(st, Setting::Farm | Setting::Garden | Setting::River) {
                    self.hills(h + 2.0, 6.0, 1.9, near, h + 30.0);
                }
            }
        }
        if !matches!(st, Setting::Mountains | Setting::Snow | Setting::Desert | Setting::Beach | Setting::Sea | Setting::Jungle | Setting::Forest) {
            self.fog(h + 2.0, 12.0, 0.35);
        }
        // sunlight through the leaves
        if covered && !night {
            self.rays(250.0, 1.0);
        } else if low {
            self.rays(120.0, 0.5);
        }
        if matches!(st, Setting::Jungle | Setting::Forest) {
            self.canopy(st == Setting::Jungle);
        }
        // clouds
        if !night && !matches!(st, Setting::Jungle) || matches!(st, Setting::Jungle) && !night && has(Kind::Cloud).is_some() {
            let cl = c.objects.iter().filter(|o| o.kind == Kind::Cloud).count();
            if cl == 0 {
                let k = if matches!(self.time, Time::Storm | Time::Cloudy) { 4 } else { 2 };
                for i in 0..k {
                    let (x, y, s) = (60.0 + i as f32 * 130.0 + self.rng.between(-20.0, 20.0), 36.0 + (i % 2) as f32 * 26.0 + self.rng.between(0.0, 10.0), 0.9 + self.rng.between(0.0, 0.5));
                    self.cloud(x, y, s);
                }
            }
        }
        // the ground
        match st {
            Setting::Beach => {
                // sea, then sand with a wet edge and foam
                let sea = self.lin(&[[70, 170, 210], [40, 120, 180]], 0.0, 1.0);
                self.raw(&format!("<rect x=\"0\" y=\"{}\" width=\"400\" height=\"60\" fill=\"{sea}\"/>", n(h - 1.0)));
                self.waves(h + 4.0, h + 50.0, 9);
                self.glint(sun.map_or(free_x, |s| s.x * 4.0), h + 2.0, h + 44.0);
                let sand = self.lin(&[[246, 226, 170], [232, 200, 135]], 0.0, 1.0);
                let wet = self.hill_path(h + 52.0, 6.0, 1.1, 2.0, H + 4.0);
                let p = self.flat([200, 170, 120]);
                self.path(&wet, &p);
                let d = self.hill_path(h + 58.0, 6.0, 1.1, 2.0, H + 4.0);
                self.raw(&format!("<path d=\"{d}\" fill=\"{sand}\"/>"));
                let foam = self.hill_path(h + 49.0, 5.5, 1.1, 2.0, h + 53.0);
                let p = self.flat([255, 255, 255]).alpha(0.7);
                self.path(&foam, &p);
                self.tufts(10, h + 90.0, H - 10.0, [200, 170, 110]);
            }
            Setting::Sea => {
                let sea = self.lin(&[[74, 172, 214], [24, 90, 160]], 0.0, 1.0);
                self.raw(&format!("<rect x=\"0\" y=\"{}\" width=\"400\" height=\"{}\" fill=\"{sea}\"/>", n(h - 1.0), n(H - h + 2.0)));
                self.waves(h + 4.0, H, 16);
                self.glint(sun.map_or(free_x, |s| s.x * 4.0), h + 2.0, H - 20.0);
            }
            Setting::Snow => {
                self.ground(h + 6.0, [250, 252, 255], [205, 222, 242], 4.0);
                // blue shadows and sparkle
                for i in 0..6 {
                    let (x, y) = (self.rng.between(10.0, 390.0), self.rng.between(h + 20.0, H - 8.0));
                    let p = self.flat([150, 180, 225]).alpha(0.3);
                    self.ell(x, y, 30.0 + i as f32 * 4.0, 4.0, &p);
                }
            }
            Setting::Desert => {
                self.ground(h + 6.0, [238, 206, 138], [214, 172, 100], 5.0);
                for _ in 0..8 {
                    let (x, y) = (self.rng.between(10.0, 390.0), self.rng.between(h + 16.0, H - 6.0));
                    self.curve(&format!("M{},{} q14,-4 28,0 t28,0", n(x), n(y)), [196, 150, 90], 1.2, 0.6);
                }
            }
            Setting::Jungle | Setting::Forest => {
                self.ground(h + 4.0, [76, 150, 70], [38, 98, 54], 4.0);
                self.trail(h + 8.0, 200.0, 188.0, 62.0, [186, 150, 100]);
                self.tufts(30, h + 10.0, H - 6.0, [40, 110, 50]);
            }
            Setting::Farm => {
                self.ground(h + 4.0, [150, 190, 80], [100, 150, 60], 3.0);
                // crop rows
                for i in 0..7 {
                    let t = i as f32 / 6.0;
                    let x = 20.0 + t * 130.0;
                    let p = self.flat([88, 140, 58]).alpha(0.8);
                    self.poly(&[(30.0 + t * 40.0, h + 12.0), (40.0 + t * 40.0, h + 12.0), (x - 14.0 + i as f32 * 2.0, H), (x + 10.0 + i as f32 * 6.0, H)], &p);
                }
                self.trail(h + 8.0, 300.0, 290.0, 50.0, [200, 170, 110]);
            }
            Setting::Village => {
                self.ground(h + 6.0, [122, 188, 90], [78, 140, 66], 3.0);
                self.trail(h + 12.0, 190.0, 210.0, 66.0, [214, 184, 132]);
                self.tufts(24, h + 12.0, H - 6.0, [60, 130, 60]);
            }
            Setting::Mountains => {
                self.ground(h + 8.0, [100, 164, 84], [60, 118, 66], 4.0);
                self.tufts(24, h + 14.0, H - 6.0, [50, 110, 56]);
            }
            Setting::River => {
                self.ground(h + 4.0, [124, 192, 92], [78, 146, 70], 3.0);
                if !c.objects.iter().any(|o| o.kind == Kind::River) {
                    self.river(200.0, h + 4.0, 1.0);
                }
                self.tufts(24, h + 10.0, H - 6.0, [56, 124, 58]);
            }
            _ => {
                self.ground(h + 4.0, [124, 192, 92], [70, 138, 70], 4.0);
                if matches!(st, Setting::Garden) {
                    self.trail(h + 10.0, 200.0, 180.0, 58.0, [214, 190, 140]);
                } else if !night {
                    self.trail(h + 10.0, 210.0, 230.0, 54.0, [208, 180, 128]);
                }
                self.tufts(30, h + 10.0, H - 6.0, [56, 124, 58]);
            }
        }
        if night {
            self.fireflies(9, h - 10.0, H - 20.0);
        }
        h
    }

    /// Leaves overhead and big dark trunks at the sides: the roof of a jungle or a forest, with vines hanging down.
    pub fn canopy(&mut self, jungle: bool) {
        let green = if jungle { [30, 112, 58] } else { [52, 128, 70] };
        // trunks at both sides
        for (x, w) in [(18.0f32, 30.0f32), (388.0, 34.0)] {
            let p = self.pn([96, 66, 46]);
            self.path(&format!("M{},250 Q{},170 {},110 L{},-4 L{},-4 L{},110 Q{},170 {},250 Z", n(x - w), n(x - w * 0.5), n(x - w * 0.45), n(x - w * 0.4), n(x + w * 0.4), n(x + w * 0.45), n(x + w * 0.5), n(x + w)), &p);
            let sh = self.flat([40, 24, 20]).alpha(0.3);
            self.rect(x + w * 0.05, -4.0, w * 0.4, 260.0, 0.0, &sh);
            for i in 0..6 {
                self.curve(&format!("M{},{} q4,6 0,12", n(x - w * 0.2 + (i % 2) as f32 * w * 0.3), n(20.0 + i as f32 * 38.0)), [50, 30, 24], 1.2, 0.5);
            }
        }
        // the roof of leaves
        let mut cs = Vec::new();
        let mut x = -10.0;
        while x < 420.0 {
            let r = self.rng.between(18.0, 30.0);
            let y = self.rng.between(-6.0, 16.0);
            cs.push((x, y, r));
            x += r * 1.1;
        }
        self.blobs(&cs, darken(green, 0.2));
        let mut cs = Vec::new();
        let mut x = 10.0;
        while x < 420.0 {
            let r = self.rng.between(12.0, 20.0);
            let y = self.rng.between(-4.0, 12.0);
            cs.push((x, y, r));
            x += r * 1.8;
        }
        self.blobs(&cs, green);
        // vines and hanging leaves
        for i in 0..6 {
            let x = 60.0 + i as f32 * 58.0 + self.rng.between(-12.0, 12.0);
            let len = self.rng.between(24.0, 58.0);
            self.curve(&format!("M{},8 q-5,{} 2,{} t-2,{}", n(x), n(len * 0.3), n(len * 0.5), n(len * 0.5)), [44, 112, 52], 2.0, 0.95);
            self.leaf(x, 8.0 + len, 90.0, 10.0, 3.0, [60, 140, 64]);
        }
    }

    pub fn waves(&mut self, y0: f32, y1: f32, rows: usize) {
        for i in 0..rows {
            let t = i as f32 / rows as f32;
            let y = y0 + (y1 - y0) * t;
            let k = 0.5 + t;
            let x = self.rng.between(0.0, 400.0);
            let w = 14.0 * k;
            self.curve(&format!("M{},{} q{},-{} {},0 t{},0", n(x), n(y), n(w / 2.0), n(2.2 * k), n(w), n(w)), [255, 255, 255], 1.1 * k, 0.45);
            let x = self.rng.between(0.0, 400.0);
            self.curve(&format!("M{},{} q{},-{} {},0 t{},0", n(x), n(y + 3.0), n(w / 2.0), n(2.0 * k), n(w), n(w)), [210, 235, 250], 1.0 * k, 0.4);
        }
    }

    /// The bright streak of the sun on water.
    pub fn glint(&mut self, x: f32, y0: f32, y1: f32) {
        let n_ = 7;
        for i in 0..n_ {
            let t = i as f32 / n_ as f32;
            let w = 6.0 + t * 24.0;
            let y = y0 + (y1 - y0) * t;
            let p = self.flat([255, 244, 200]).alpha(0.55 - t * 0.2);
            let j = self.rng.between(-4.0, 4.0);
            self.rect(x - w / 2.0 + j, y, w, 1.8, 0.9, &p);
        }
    }

    /// A winding river that widens towards the viewer; `x` is its centre at the bottom of the frame.
    pub fn river(&mut self, x: f32, h: f32, s: f32) {
        let w = 42.0 * s;
        let d = format!("M{},{} C{},{} {},{} {},{} L{},{} C{},{} {},{} {},{} Z", n(x - 70.0 * s * 0.0 + 18.0), n(h), n(x + 40.0), n(h + 26.0), n(x - 70.0), n(h + 56.0), n(x - w), n(H + 4.0), n(x + w), n(H + 4.0), n(x - 40.0), n(h + 60.0), n(x + 96.0), n(h + 30.0), n(x + 24.0), n(h));
        let g = self.lin(&[[98, 190, 226], [52, 130, 190]], 0.0, 1.0);
        let bank = self.flat([150, 120, 80]);
        self.path(&d, &bank);
        self.open_raw("transform=\"translate(0 0)\"");
        self.raw(&format!("<path d=\"{d}\" fill=\"{g}\" transform=\"translate({} {}) scale(0.94 0.99) translate({} {})\"/>", n(x * 0.06), n(2.4), n(0.0), n(0.0)));
        self.close();
        for i in 0..5 {
            let y = h + 30.0 + i as f32 * 20.0;
            let cx = x - 10.0 * i as f32 + 24.0 - i as f32 * 3.0;
            self.curve(&format!("M{},{} q10,-3 20,0", n(cx - 10.0), n(y)), [255, 255, 255], 1.2, 0.5);
        }
    }

    fn cave(&mut self, c: &Composition) -> f32 {
        let h = 168.0;
        let wall = self.lin(&[[22, 16, 44], [44, 32, 78], [30, 22, 58]], 0.0, 1.0);
        self.raw(&format!("<rect width=\"400\" height=\"240\" fill=\"{wall}\"/>"));
        // back wall rock masses
        for i in 0..5 {
            let x = 40.0 + i as f32 * 80.0;
            let p = self.flat([54, 42, 92]).alpha(0.7);
            let (jx, jy) = (self.rng.between(-14.0, 14.0), self.rng.between(-10.0, 20.0));
            self.ell(x + jx, 110.0 + jy, 56.0, 46.0, &p);
        }
        // the far glow of the cave
        let g = self.glow([120, 220, 255], 0.55);
        self.raw(&format!("<circle cx=\"310\" cy=\"128\" r=\"90\" fill=\"{g}\"/>"));
        // stalactites
        let mut x = -10.0;
        let mut pts = vec![(-10.0, -4.0)];
        while x < 410.0 {
            let w = self.rng.between(18.0, 36.0);
            let d = self.rng.between(18.0, 56.0);
            pts.push((x, 0.0));
            pts.push((x + w * 0.5, d));
            x += w;
        }
        pts.push((410.0, -4.0));
        let p = self.pn([38, 28, 66]);
        self.poly(&pts, &p);
        // floor
        let d = self.hill_path(h, 5.0, 1.2, 0.5, H + 4.0);
        let fl = self.lin(&[[58, 46, 92], [24, 18, 46]], 0.0, 1.0);
        let st = self.pn([58, 46, 92]).st;
        self.raw(&format!("<path d=\"{d}\" fill=\"{fl}\"{st}/>"));
        // crystals that glow
        let tint = c.objects.iter().find(|o| o.kind == Kind::Crystal).and_then(|o| o.color).unwrap_or([90, 220, 255]);
        let have = c.objects.iter().any(|o| o.kind == Kind::Crystal);
        if !have {
            for (x, y, s) in [(34.0, h + 4.0, 1.1), (370.0, h + 2.0, 1.0), (330.0, h - 18.0, 0.8), (150.0, h - 10.0, 0.6)] {
                self.crystals(x, y, s, tint);
            }
        }
        self.fireflies(7, 70.0, 150.0);
        h
    }

    /// A cluster of glowing crystals standing at (x, y).
    pub fn crystals(&mut self, x: f32, y: f32, s: f32, col: Col) {
        let g = self.glow(col, 0.75);
        self.raw(&format!("<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{g}\"/>", n(x), n(y - 22.0 * s), n(58.0 * s)));
        self.open(x, y, s, false);
        for (dx, h, w, a) in [(-12.0, 34.0, 8.0, -14.0), (0.0, 50.0, 10.0, 2.0), (13.0, 30.0, 8.0, 16.0), (24.0, 18.0, 6.0, 26.0)] {
            self.open_raw(&format!("transform=\"translate({} 0) rotate({})\"", n(dx), n(a)));
            let p = self.pn(mix(col, [255, 255, 255], 0.15));
            self.poly(&[(-w, 0.0), (-w * 0.8, -h * 0.8), (0.0, -h), (w * 0.8, -h * 0.8), (w, 0.0)], &p);
            let l = self.flat([255, 255, 255]).alpha(0.55);
            self.poly(&[(-w * 0.5, -2.0), (-w * 0.4, -h * 0.75), (0.0, -h * 0.95), (-w * 0.1, -h * 0.5)], &l);
            let dk = self.flat(darken(col, 0.35)).alpha(0.4);
            self.poly(&[(0.0, -h), (w * 0.8, -h * 0.8), (w, 0.0), (w * 0.3, 0.0)], &dk);
            self.close();
        }
        self.close();
    }

    fn space(&mut self) -> f32 {
        let h = 170.0;
        let g = self.lin(&[[6, 4, 24], [22, 12, 58], [52, 22, 84]], 0.0, 1.0);
        self.raw(&format!("<rect width=\"400\" height=\"240\" fill=\"{g}\"/>"));
        for (x, y, r, c) in [(90.0, 60.0, 80.0, [140, 60, 200]), (300.0, 40.0, 70.0, [60, 90, 200]), (220.0, 110.0, 60.0, [200, 60, 140])] {
            let g = self.glow(c, 0.35);
            self.raw(&format!("<circle cx=\"{x}\" cy=\"{y}\" r=\"{r}\" fill=\"{g}\"/>"));
        }
        self.stars(110, h - 10.0);
        // a ringed planet and a blue world
        self.open(320.0, 64.0, 1.0, false);
        let p = self.pn([232, 160, 90]);
        self.circ(0.0, 0.0, 26.0, &p);
        let b = self.flat([200, 110, 60]).alpha(0.55);
        self.ell(0.0, 6.0, 25.0, 6.0, &b);
        self.ell(-3.0, -10.0, 22.0, 3.0, &b);
        let sh = self.flat([60, 20, 80]).alpha(0.4);
        self.circ(8.0, 8.0, 22.0, &sh);
        let ring = hex(self.tone([250, 220, 160]));
        self.raw(&format!("<ellipse cx=\"0\" cy=\"0\" rx=\"46\" ry=\"9\" fill=\"none\" stroke=\"{ring}\" stroke-width=\"3\" transform=\"rotate(-18)\" opacity=\"0.85\"/>"));
        self.close();
        let p = self.pn([70, 130, 220]);
        self.circ(70.0, 52.0, 16.0, &p);
        let gcol = self.flat([90, 190, 110]);
        self.path("M62,48 q6,-6 12,-2 q-2,6 4,8 q-6,6 -14,2 z", &gcol);
        // the moon's surface
        let d = self.hill_path(h, 5.0, 1.0, 1.0, H + 4.0);
        let gr = self.lin(&[[176, 172, 190], [100, 96, 122]], 0.0, 1.0);
        let st = self.pn([176, 172, 190]).st;
        self.raw(&format!("<path d=\"{d}\" fill=\"{gr}\"{st}/>"));
        for (x, y, rx) in [(60.0, 196.0, 30.0), (190.0, 214.0, 38.0), (310.0, 190.0, 24.0), (370.0, 220.0, 26.0), (120.0, 180.0, 14.0)] {
            let a = self.flat([70, 66, 92]).alpha(0.55);
            self.ell(x, y, rx, rx * 0.28, &a);
            let b = self.flat([210, 206, 224]).alpha(0.5);
            self.ell(x - 1.0, y - 1.5, rx * 0.82, rx * 0.2, &b);
        }
        h
    }

    fn classroom(&mut self) -> f32 {
        let h = 160.0;
        let wall = self.lin(&[[246, 232, 196], [232, 214, 172]], 0.0, 1.0);
        self.raw(&format!("<rect width=\"400\" height=\"{}\" fill=\"{wall}\"/>", n(h + 2.0)));
        // the board with chalk drawings (no letters)
        let p = self.pn([130, 90, 55]);
        self.rect(80.0, 26.0, 190.0, 92.0, 4.0, &p);
        let p = self.pn([44, 92, 74]);
        self.rect(86.0, 32.0, 178.0, 80.0, 2.0, &p);
        let chalk = [240, 240, 232];
        self.curve("M104,92 q12,-26 24,0", chalk, 1.6, 0.85);
        self.curve("M104,92 h24", chalk, 1.6, 0.85);
        self.curve("M150,50 l20,0 l0,16 l-20,0 z", chalk, 1.6, 0.8);
        self.curve("M190,90 l16,-28 l16,28 z", chalk, 1.6, 0.8);
        self.curve("M236,56 a9,9 0 1 0 0.1,0", [255, 230, 120], 1.6, 0.85);
        self.curve("M150,100 h80", chalk, 1.4, 0.5);
        // window with sky
        let sky = self.lin(&[[120, 180, 235], [200, 232, 250]], 0.0, 1.0);
        let fr = self.pn([140, 100, 64]);
        self.rect(296.0, 30.0, 76.0, 84.0, 3.0, &fr);
        self.raw(&format!("<rect x=\"301\" y=\"35\" width=\"66\" height=\"74\" fill=\"{sky}\"/>"));
        self.cloud(318.0, 62.0, 0.5);
        let gr = self.flat([110, 180, 90]);
        self.rect(301.0, 94.0, 66.0, 15.0, 0.0, &gr);
        self.line(334.0, 35.0, 334.0, 109.0, [140, 100, 64], 3.0);
        self.line(301.0, 72.0, 367.0, 72.0, [140, 100, 64], 3.0);
        // clock
        let p = self.pn([250, 250, 244]);
        self.circ(36.0, 48.0, 15.0, &p);
        self.line(36.0, 48.0, 36.0, 38.0, [60, 50, 50], 1.8);
        self.line(36.0, 48.0, 43.0, 51.0, [60, 50, 50], 1.8);
        // the floor
        let f = self.lin(&[[196, 148, 98], [150, 104, 66]], 0.0, 1.0);
        self.raw(&format!("<rect y=\"{}\" width=\"400\" height=\"{}\" fill=\"{f}\"/>", n(h), n(H - h + 2.0)));
        for i in 0..6 {
            let y = h + 6.0 + i as f32 * (80.0 / 6.0) * (1.0 + i as f32 * 0.2);
            self.curve(&format!("M0,{} L400,{}", n(y), n(y)), [120, 80, 50], 1.0, 0.35);
        }
        let p = self.flat([60, 40, 30]).alpha(0.25);
        self.rect(0.0, h - 4.0, 400.0, 6.0, 0.0, &p);
        // desks seen from behind, in the distance
        for (x, y, s) in [(40.0, h + 20.0, 0.8), (330.0, h + 24.0, 0.85)] {
            self.desk(x, y, s);
        }
        h
    }

    pub fn desk(&mut self, x: f32, y: f32, s: f32) {
        self.open(x, y, s, false);
        self.shadow(0.0, 2.0, 34.0);
        let top = self.pn([208, 162, 104]);
        self.poly(&[(-34.0, -26.0), (34.0, -26.0), (40.0, -18.0), (-40.0, -18.0)], &top);
        let side = self.pn([160, 116, 72]);
        self.rect(-40.0, -18.0, 80.0, 5.0, 0.0, &side);
        for dx in [-34.0, 30.0] {
            self.rect(dx, -13.0, 4.0, 14.0, 0.0, &side);
        }
        let book = self.pn([200, 70, 70]);
        self.poly(&[(-12.0, -26.0), (4.0, -26.0), (7.0, -22.0), (-15.0, -22.0)], &book);
        self.close();
    }

    fn room(&mut self, c: &Composition) -> f32 {
        let h = 166.0;
        let night = matches!(self.time, Time::Night | Time::Sunset);
        let wall = self.lin(&[[250, 226, 190], [236, 200, 160]], 0.0, 1.0);
        self.raw(&format!("<rect width=\"400\" height=\"{}\" fill=\"{wall}\"/>", n(h + 2.0)));
        for i in 0..14 {
            let p = self.flat([255, 255, 255]).alpha(0.18);
            self.rect(i as f32 * 30.0, 0.0, 14.0, h, 0.0, &p);
        }
        // window with curtains
        let (s0, s1) = if night { ([14, 20, 70], [60, 70, 140]) } else { ([120, 185, 240], [210, 235, 252]) };
        let sky = self.lin(&[s0, s1], 0.0, 1.0);
        let fr = self.pn([250, 244, 232]);
        self.rect(246.0, 24.0, 88.0, 96.0, 3.0, &fr);
        self.raw(&format!("<rect x=\"252\" y=\"30\" width=\"76\" height=\"84\" fill=\"{sky}\"/>"));
        if night {
            self.raw("<circle cx=\"300\" cy=\"54\" r=\"9\" fill=\"#f6f0d0\"/><circle cx=\"268\" cy=\"46\" r=\"1.2\" fill=\"#fff\"/><circle cx=\"282\" cy=\"88\" r=\"1.2\" fill=\"#fff\"/><circle cx=\"316\" cy=\"96\" r=\"1.2\" fill=\"#fff\"/>");
        } else {
            self.cloud(272.0, 60.0, 0.5);
        }
        self.line(290.0, 30.0, 290.0, 114.0, [250, 244, 232], 3.0);
        self.line(252.0, 72.0, 328.0, 72.0, [250, 244, 232], 3.0);
        let cur = self.pn([210, 90, 100]);
        self.path("M238,20 L262,20 Q268,60 258,124 L238,124 Z", &cur);
        self.path("M342,20 L318,20 Q312,60 322,124 L342,124 Z", &cur);
        // a picture frame on the wall
        let fr = self.pn([160, 110, 70]);
        self.rect(60.0, 40.0, 62.0, 46.0, 2.0, &fr);
        let pic = self.pn([150, 205, 170]);
        self.rect(65.0, 45.0, 52.0, 36.0, 1.0, &pic);
        let sun = self.flat([255, 220, 110]);
        self.circ(104.0, 55.0, 6.0, &sun);
        let hill = self.flat([90, 160, 100]);
        self.path("M65,81 Q80,58 96,70 T117,66 L117,81 Z", &hill);
        // floor and a rug
        let f = self.lin(&[[206, 156, 104], [160, 112, 72]], 0.0, 1.0);
        self.raw(&format!("<rect y=\"{}\" width=\"400\" height=\"{}\" fill=\"{f}\"/>", n(h), n(H - h + 2.0)));
        for i in 0..5 {
            let y = h + 8.0 + i as f32 * 16.0;
            self.curve(&format!("M0,{} L400,{}", n(y), n(y)), [128, 88, 56], 1.0, 0.35);
        }
        let p = self.flat([70, 44, 30]).alpha(0.2);
        self.rect(0.0, h - 5.0, 400.0, 7.0, 0.0, &p);
        let rug = self.pn([196, 84, 90]);
        self.ell(180.0, 206.0, 120.0, 22.0, &rug);
        let ring = self.flat([250, 210, 150]);
        self.ell(180.0, 206.0, 96.0, 15.0, &ring);
        let mid = self.flat([196, 84, 90]);
        self.ell(180.0, 206.0, 70.0, 10.0, &mid);
        // a lamp that glows in the evening, a plant
        let lx = 366.0;
        if night || c.mood == super::Mood::Calm {
            let g = self.glow([255, 224, 150], 0.8);
            self.raw(&format!("<circle cx=\"{lx}\" cy=\"120\" r=\"70\" fill=\"{g}\"/>"));
        }
        let pole = self.pn([120, 90, 60]);
        self.rect(lx - 2.0, 126.0, 4.0, 64.0, 0.0, &pole);
        self.ell(lx, 192.0, 14.0, 4.0, &pole);
        let sh = self.pn([252, 230, 170]);
        self.poly(&[(lx - 16.0, 126.0), (lx + 16.0, 126.0), (lx + 9.0, 100.0), (lx - 9.0, 100.0)], &sh);
        let pot = self.pn([200, 110, 80]);
        self.poly(&[(30.0, 178.0), (60.0, 178.0), (56.0, 204.0), (34.0, 204.0)], &pot);
        for i in 0..6 {
            self.leaf(45.0, 178.0, -100.0 + i as f32 * 28.0, 34.0, 7.0, [70, 150, 80]);
        }
        h
    }

    /// The near edge of the picture: dark leaves and grass blades at the bottom corners that frame the scene.
    pub fn foreground(&mut self, st: Setting) {
        let grass = match st {
            Setting::Jungle => [34, 100, 56],
            Setting::Forest => [48, 112, 62],
            Setting::Desert => [190, 150, 90],
            Setting::Snow => [200, 218, 240],
            Setting::Beach => [214, 184, 120],
            Setting::Mountains | Setting::Meadow | Setting::Garden | Setting::Farm | Setting::Village | Setting::River | Setting::Night => [60, 130, 66],
            _ => return,
        };
        match st {
            Setting::Jungle | Setting::Forest => {
                self.fern(14.0, 242.0, 1.5, grass);
                self.fern(388.0, 244.0, 1.4, darken(grass, 0.1));
                if st == Setting::Jungle {
                    self.leaf(-6.0, 230.0, -50.0, 64.0, 14.0, grass);
                    self.leaf(406.0, 232.0, -130.0, 60.0, 13.0, grass);
                }
            }
            Setting::Desert | Setting::Snow | Setting::Beach => {
                let p = self.flat(grass).alpha(0.7);
                self.ell(20.0, 246.0, 70.0, 12.0, &p);
                self.ell(390.0, 248.0, 80.0, 12.0, &p);
            }
            _ => {
                for i in 0..9 {
                    let x = 4.0 + i as f32 * 6.0;
                    let k = 14.0 + (i % 3) as f32 * 8.0;
                    let c = darken(grass, 0.1 + (i % 2) as f32 * 0.12);
                    self.leaf(x, 242.0, -80.0 - (i % 4) as f32 * 8.0, k, 3.0, c);
                    self.leaf(396.0 - i as f32 * 6.0, 242.0, -100.0 + (i % 4) as f32 * 8.0, k * 1.1, 3.0, c);
                }
            }
        }
    }

    /// Falling rain over everything.
    pub fn rain(&mut self) {
        let p = self.flat([40, 50, 90]).alpha(0.16);
        self.rect(0.0, 0.0, 400.0, 240.0, 0.0, &p);
        let c = hex(self.tone([215, 232, 255]));
        let mut s = String::new();
        for _ in 0..60 {
            let (x, y) = (self.rng.between(0.0, 410.0), self.rng.between(-10.0, 235.0));
            s.push_str(&format!("M{},{} l-3,9 ", n(x), n(y)));
        }
        self.raw(&format!("<path d=\"{s}\" fill=\"none\" stroke=\"{c}\" stroke-width=\"1.2\" stroke-linecap=\"round\" opacity=\"0.75\"/>"));
    }

    pub fn snowfall(&mut self) {
        for _ in 0..55 {
            let (x, y) = (self.rng.between(2.0, 398.0), self.rng.between(2.0, 236.0));
            let r = self.rng.between(1.0, 2.8);
            self.raw(&format!("<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"#ffffff\" opacity=\"0.92\"/>", n(x), n(y), n(r)));
        }
    }

    /// Edge shading that pulls the eye to the middle; washes and paper grain for watercolour, dots for comic.
    pub fn finishing(&mut self, mood: super::Mood) {
        let tint = match mood {
            super::Mood::Mysterious => Some(([30, 20, 70], 0.18)),
            super::Mood::Sad => Some(([70, 80, 110], 0.16)),
            super::Mood::Exciting => Some(([255, 140, 60], 0.08)),
            super::Mood::Happy => Some(([255, 220, 120], 0.06)),
            super::Mood::Calm => None,
        };
        if let Some((c, a)) = tint {
            let p = self.flat(c).alpha(a);
            self.rect(0.0, 0.0, 400.0, 240.0, 0.0, &p);
        }
        let id = self.id();
        self.defs.push_str(&format!("<radialGradient id=\"{id}\" cx=\"0.5\" cy=\"0.5\" r=\"0.75\"><stop offset=\"0.55\" stop-color=\"#140e30\" stop-opacity=\"0\"/><stop offset=\"1\" stop-color=\"#140e30\" stop-opacity=\"0.32\"/></radialGradient>"));
        self.raw(&format!("<rect width=\"400\" height=\"240\" fill=\"url(#{id})\"/>"));
        match self.style {
            PictureStyle::Watercolor => {
                // paper grain: faint speckles and two big translucent washes
                for _ in 0..46 {
                    let (x, y) = (self.rng.between(0.0, 400.0), self.rng.between(0.0, 240.0));
                    let r = self.rng.between(0.8, 3.0);
                    self.raw(&format!("<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"#7a5a30\" opacity=\"0.05\"/>", n(x), n(y), n(r)));
                }
                self.raw("<ellipse cx=\"60\" cy=\"40\" rx=\"90\" ry=\"46\" fill=\"#ffffff\" opacity=\"0.07\"/><ellipse cx=\"340\" cy=\"200\" rx=\"110\" ry=\"40\" fill=\"#ffe9b8\" opacity=\"0.07\"/>");
            }
            PictureStyle::Comic => {
                // halftone dots in the lower corners
                let mut s = String::new();
                for row in 0..12 {
                    for col in 0..30 {
                        let (x, y) = (col as f32 * 7.0 + (row % 2) as f32 * 3.5, 240.0 - row as f32 * 6.0);
                        let r = 1.7 * (1.0 - row as f32 / 12.0) * (1.0 - (x - 40.0).abs() / 220.0).max(0.0);
                        if r > 0.3 {
                            s.push_str(&format!("<circle cx=\"{}\" cy=\"{}\" r=\"{}\"/>", n(x), n(y), n2f(r)));
                        }
                    }
                }
                self.raw(&format!("<g fill=\"#1b142e\" opacity=\"0.22\">{s}</g>"));
                self.raw("<rect x=\"1.5\" y=\"1.5\" width=\"397\" height=\"237\" fill=\"none\" stroke=\"#1b142e\" stroke-width=\"3\"/>");
            }
            PictureStyle::NightSky => {
                self.raw("<ellipse cx=\"200\" cy=\"90\" rx=\"230\" ry=\"90\" fill=\"#5a4bd0\" opacity=\"0.07\"/>");
            }
            PictureStyle::Pixel => {}
        }
    }
}

fn n2f(x: f32) -> String {
    format!("{x:.2}")
}
