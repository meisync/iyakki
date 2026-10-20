//! Things that stand in a scene: trees with trunks, branches and leaf clusters, bushes, flowers, houses, caves, mountains,
//! hills, boats, bridges, castles, rockets, fires, mushrooms, rocks, cacti, barns, fences, tents, lanterns, planets, birds and
//! lakes. Each is drawn at the origin (its foot on the ground, or its centre for things that float) facing the viewer, and the
//! composer scales and places it.

use super::art::*;
use super::Setting;

impl Art {
    /// A broadleaf tree: roots, a curved trunk with bark, branches and layered leaf clusters; `pine` draws a snowy fir.
    pub fn tree(&mut self, leaf: Option<Col>, st: Setting) {
        let jungle = st == Setting::Jungle;
        let pine = matches!(st, Setting::Snow | Setting::Mountains) && leaf.is_none();
        let bark = [118, 82, 54];
        if pine {
            let p = self.pn(bark);
            self.rect(-4.0, -24.0, 8.0, 24.0, 1.0, &p);
            let g = leaf.unwrap_or([46, 120, 82]);
            for (i, (w, y)) in [(34.0, -22.0), (28.0, -46.0), (21.0, -68.0), (13.0, -86.0)].iter().enumerate() {
                let c = self.pn(mix(g, [30, 80, 70], i as f32 * 0.08));
                self.poly(&[(0.0, y - 30.0), (*w, *y), (-*w, *y)], &c);
                if st == Setting::Snow {
                    let sn = self.flat([248, 250, 255]);
                    self.poly(&[(0.0, y - 30.0), (w * 0.55, y - 11.0), (w * 0.2, y - 14.0), (0.0, y - 9.0), (-w * 0.3, y - 14.0), (-w * 0.55, y - 11.0)], &sn);
                }
            }
            return;
        }
        let green = leaf.unwrap_or(if jungle { [38, 130, 60] } else { [72, 150, 66] });
        // roots and trunk
        let b = self.pn(bark);
        self.path("M-9,0 Q-5,-6 -5,-20 Q-4,-40 -6,-58 L6,-58 Q4,-40 5,-20 Q5,-6 10,0 Z", &b);
        let dark = self.flat(darken(bark, 0.3)).alpha(0.45);
        self.path("M1,0 Q3,-20 2,-58 L6,-58 Q4,-40 5,-20 Q5,-6 10,0 Z", &dark);
        for y in [-14.0, -28.0, -42.0] {
            self.curve(&format!("M-3,{} q3,2 6,0", n(y)), darken(bark, 0.35), 1.0, 0.6);
        }
        // branches
        self.limb(0.0, -48.0, -17.0, -66.0, 4.0, bark);
        self.limb(0.0, -52.0, 18.0, -70.0, 4.0, bark);
        // crown: dark back layer, main body, bright front layer
        let back = darken(green, 0.18);
        self.blobs(&[(-20.0, -64.0, 15.0), (20.0, -66.0, 16.0), (0.0, -84.0, 17.0)], back);
        self.blobs(&[(-24.0, -66.0, 14.0), (-8.0, -74.0, 19.0), (14.0, -78.0, 18.0), (26.0, -64.0, 14.0), (0.0, -60.0, 16.0), (-12.0, -90.0, 13.0), (10.0, -96.0, 13.0)], green);
        self.blobs(&[(-14.0, -80.0, 8.0), (6.0, -88.0, 8.0), (18.0, -72.0, 7.0)], lighten(green, 0.2));
        if jungle {
            // hanging vines
            for dx in [-20.0, 4.0, 22.0] {
                self.curve(&format!("M{},-62 q-4,12 2,26 t-2,22", n(dx)), [46, 120, 54], 1.8, 0.9);
            }
        }
    }

    pub fn palm(&mut self) {
        let b = self.pn([150, 112, 72]);
        self.path("M-5,0 Q8,-30 2,-80 L8,-80 Q16,-30 6,0 Z", &b);
        for y in [-14.0, -30.0, -46.0, -62.0] {
            self.curve(&format!("M0,{} q6,-2 11,2", n(y)), [100, 70, 44], 1.1, 0.7);
        }
        let g = [58, 150, 74];
        for (i, a) in [-168.0, -140.0, -112.0, -68.0, -40.0, -12.0].iter().enumerate() {
            self.leaf(5.0, -80.0, *a, 42.0, 9.0, if i % 2 == 0 { g } else { darken(g, 0.15) });
        }
        let c = self.pn([120, 80, 50]);
        self.circ(2.0, -76.0, 3.6, &c);
        self.circ(9.0, -76.0, 3.6, &c);
    }

    pub fn bush(&mut self, col: Option<Col>) {
        let g = col.unwrap_or([70, 148, 70]);
        self.shadow(0.0, 0.0, 22.0);
        self.blobs(&[(-13.0, -8.0, 9.0), (0.0, -12.0, 12.0), (13.0, -8.0, 9.0), (-4.0, -5.0, 8.0), (7.0, -4.0, 8.0)], g);
        for (x, y) in [(-8.0, -12.0), (6.0, -16.0), (12.0, -6.0)] {
            let b = self.pn([220, 70, 80]);
            self.circ(x, y, 1.9, &b);
        }
    }

    /// A little clump of three flowers with stems and leaves.
    pub fn flower(&mut self, col: Option<Col>) {
        let petal = col.unwrap_or([240, 120, 160]);
        for (dx, h, k) in [(-9.0, 20.0, 0.9), (0.0, 28.0, 1.1), (10.0, 22.0, 0.95)] {
            let st = [64, 140, 66];
            self.curve(&format!("M{},0 Q{},{} {},{}", n(dx), n(dx + 2.0), n(-h * 0.5), n(dx), n(-h)), st, 1.9, 1.0);
            self.leaf(dx, -h * 0.35, -30.0, 8.0, 2.4, st);
            let r = 3.7 * k;
            for i in 0..5 {
                let a = i as f32 * std::f32::consts::TAU / 5.0 + 0.3;
                let p = self.pn(if (dx as i32) % 2 == 0 { petal } else { lighten(petal, 0.15) });
                self.circ(dx + a.cos() * r, -h + a.sin() * r, r * 0.95, &p);
            }
            let c = self.pn([255, 214, 70]);
            self.circ(dx, -h, r * 0.7, &c);
        }
    }

    pub fn house(&mut self, wall: Option<Col>) {
        let w = wall.unwrap_or([240, 214, 170]);
        let lit = matches!(self.time, Time::Night | Time::Sunset | Time::Storm | Time::Dawn);
        self.shadow(0.0, 0.0, 40.0);
        let p = self.pn(w);
        self.rect(-31.0, -38.0, 62.0, 38.0, 1.5, &p);
        // wall shade, timber
        let sh = self.flat(darken(w, 0.25)).alpha(0.35);
        self.rect(12.0, -38.0, 19.0, 38.0, 0.0, &sh);
        let brick = self.flat(darken(w, 0.18)).alpha(0.4);
        for i in 0..3 {
            self.rect(-31.0, -8.0 - i as f32 * 12.0, 62.0, 1.2, 0.0, &brick);
        }
        // roof with tiles
        let roof = [196, 84, 66];
        let r = self.pn(roof);
        self.poly(&[(-38.0, -36.0), (0.0, -68.0), (38.0, -36.0)], &r);
        let rs = self.flat(darken(roof, 0.28)).alpha(0.45);
        self.poly(&[(0.0, -68.0), (38.0, -36.0), (10.0, -36.0)], &rs);
        for i in 0..3 {
            self.curve(&format!("M{},{} L{},{}", n(-30.0 + i as f32 * 6.0), n(-39.0 - i as f32 * 4.0), n(30.0 - i as f32 * 6.0), n(-39.0 - i as f32 * 4.0)), lighten(roof, 0.3), 1.0, 0.6);
        }
        let ch = self.pn([150, 96, 80]);
        self.rect(14.0, -66.0, 8.0, 18.0, 1.0, &ch);
        // door and windows
        let d = self.pn([116, 74, 44]);
        self.path("M-18,0 L-18,-20 Q-11,-27 -4,-20 L-4,0 Z", &d);
        let kn = self.pn([255, 214, 90]);
        self.circ(-7.5, -10.0, 1.1, &kn);
        let win = if lit { [255, 220, 120] } else { [172, 214, 238] };
        if lit {
            let g = self.glow([255, 210, 110], 0.75);
            self.raw(&format!("<circle cx=\"12\" cy=\"-22\" r=\"26\" fill=\"{g}\"/>"));
        }
        let fr = self.pn([250, 240, 224]);
        self.rect(5.0, -30.0, 15.0, 15.0, 1.0, &fr);
        let gl = self.pn(win);
        self.rect(6.5, -28.5, 12.0, 12.0, 0.5, &gl);
        self.line(12.5, -28.5, 12.5, -16.5, [250, 240, 224], 1.0);
        self.line(6.5, -22.5, 18.5, -22.5, [250, 240, 224], 1.0);
        let fb = self.pn([226, 100, 110]);
        self.rect(4.0, -14.5, 17.0, 3.0, 1.0, &fb);
    }

    /// The mouth of a cave: a rock mass with a dark entrance; a colour gives the glow inside.
    pub fn cave_mouth(&mut self, glow: Option<Col>) {
        let rock = [128, 122, 136];
        self.shadow(0.0, 0.0, 56.0);
        let p = self.pn(rock);
        self.path("M-56,0 C-58,-30 -42,-56 -18,-64 C0,-72 24,-66 42,-46 C54,-30 58,-12 56,0 Z", &p);
        for (x, y, rx, ry) in [(-34.0, -30.0, 14.0, 9.0), (30.0, -36.0, 12.0, 8.0), (4.0, -58.0, 16.0, 7.0)] {
            let l = self.flat(lighten(rock, 0.25)).alpha(0.5);
            self.ell(x, y, rx, ry, &l);
        }
        let dk = self.flat(darken(rock, 0.3)).alpha(0.4);
        self.path("M20,-62 C42,-50 56,-30 56,0 L30,0 C34,-26 30,-46 20,-62 Z", &dk);
        let inner = self.pn([22, 16, 34]);
        self.path("M-24,0 C-24,-30 -14,-44 0,-44 C14,-44 24,-30 24,0 Z", &inner);
        let g = glow.unwrap_or([255, 214, 120]);
        let gl = self.glow(g, if glow.is_some() { 0.9 } else { 0.35 });
        self.raw(&format!("<ellipse cx=\"0\" cy=\"-14\" rx=\"34\" ry=\"36\" fill=\"{gl}\"/>"));
        // stalactite teeth at the opening
        let t = self.flat(darken(rock, 0.25));
        for x in [-16.0, -6.0, 6.0, 15.0] {
            self.poly(&[(x - 3.0, -44.0), (x + 3.0, -44.0), (x, -36.0)], &t);
        }
    }

    pub fn mountain(&mut self, col: Option<Col>) {
        let c = col.unwrap_or([128, 134, 160]);
        let p = self.pn(c);
        self.path("M-92,0 L-28,-86 L-8,-62 L26,-104 L92,0 Z", &p);
        let sh = self.flat(darken(c, 0.3)).alpha(0.5);
        self.poly(&[(26.0, -104.0), (92.0, 0.0), (30.0, 0.0), (18.0, -50.0)], &sh);
        let sn = self.flat([248, 250, 255]);
        self.poly(&[(26.0, -104.0), (40.0, -82.0), (30.0, -86.0), (24.0, -74.0), (16.0, -86.0), (8.0, -78.0)], &sn);
        self.poly(&[(-28.0, -86.0), (-18.0, -70.0), (-24.0, -72.0), (-30.0, -64.0), (-38.0, -72.0)], &sn);
    }

    pub fn hill_mound(&mut self, col: Option<Col>) {
        let c = col.unwrap_or([108, 180, 90]);
        let p = self.pn(c);
        self.path("M-112,2 C-92,-38 -44,-48 0,-46 C48,-46 92,-32 112,2 Z", &p);
        let hi = self.flat(lighten(c, 0.3)).alpha(0.5);
        self.path("M-80,-8 C-60,-34 -24,-42 6,-42 C-30,-30 -56,-18 -80,-8 Z", &hi);
        let sh = self.flat(darken(c, 0.2)).alpha(0.35);
        self.path("M112,2 C100,-20 80,-30 60,-34 C84,-18 96,-8 98,2 Z", &sh);
    }

    pub fn boat(&mut self, hull: Option<Col>) {
        let hc = hull.unwrap_or([176, 98, 60]);
        let w = self.flat([20, 50, 90]).alpha(0.2);
        self.ell(0.0, 5.0, 36.0, 4.0, &w);
        self.limb(0.0, -8.0, 0.0, -56.0, 2.4, [110, 76, 50]);
        let s1 = self.pn([252, 244, 226]);
        self.path("M3,-54 Q26,-34 24,-12 L3,-12 Z", &s1);
        let sh = self.flat([200, 190, 170]).alpha(0.5);
        self.path("M3,-54 Q14,-40 13,-12 L3,-12 Z", &sh);
        let s2 = self.pn([240, 200, 200]);
        self.path("M-3,-48 Q-20,-30 -20,-12 L-3,-12 Z", &s2);
        let h = self.pn(hc);
        self.path("M-32,-12 L32,-12 Q28,6 16,6 L-16,6 Q-28,6 -32,-12 Z", &h);
        let st = self.flat(lighten(hc, 0.35)).alpha(0.8);
        self.rect(-29.0, -9.0, 58.0, 2.2, 0.0, &st);
        let fl = self.pn([230, 70, 80]);
        self.poly(&[(0.0, -56.0), (12.0, -52.0), (0.0, -48.0)], &fl);
        self.curve("M-34,6 q6,-3 12,0 t12,0 t12,0 t12,0 t12,0", [255, 255, 255], 1.4, 0.8);
    }

    pub fn bridge(&mut self) {
        let st = [170, 160, 150];
        self.shadow(0.0, 0.0, 66.0);
        let p = self.pn(st);
        self.path("M-70,0 L-70,-16 Q0,-52 70,-16 L70,0 Q36,-2 30,-14 Q0,-30 -30,-14 Q-36,-2 -70,0 Z", &p);
        let top = self.pn([196, 186, 172]);
        self.path("M-70,-16 Q0,-52 70,-16 L70,-21 Q0,-58 -70,-21 Z", &top);
        for i in 0..6 {
            let x = -52.0 + i as f32 * 21.0;
            let y = -21.0 - (1.0 - (x / 70.0).powi(2)) * 24.0;
            let b = self.pn(mix(st, [120, 110, 100], 0.5));
            self.rect(x - 1.6, y - 9.0, 3.2, 10.0, 0.5, &b);
        }
        let sh = self.flat(darken(st, 0.3)).alpha(0.4);
        self.path("M-30,-14 Q0,-30 30,-14 Q0,-22 -30,-14 Z", &sh);
    }

    pub fn castle(&mut self, col: Option<Col>) {
        let c = col.unwrap_or([200, 194, 214]);
        self.shadow(0.0, 0.0, 52.0);
        let body = self.pn(c);
        self.rect(-26.0, -58.0, 52.0, 58.0, 0.5, &body);
        for dx in [-46.0f32, 28.0] {
            let t = self.pn(darken(c, 0.06));
            self.rect(dx, -76.0, 20.0, 76.0, 0.5, &t);
            let r = self.pn([196, 70, 84]);
            self.poly(&[(dx - 3.0, -76.0), (dx + 10.0, -100.0), (dx + 23.0, -76.0)], &r);
            let w = self.pn([60, 50, 80]);
            self.rect(dx + 7.0, -62.0, 6.0, 12.0, 3.0, &w);
        }
        for i in 0..5 {
            let m = self.pn(c);
            self.rect(-26.0 + i as f32 * 11.0, -64.0, 7.0, 7.0, 0.0, &m);
        }
        let sh = self.flat(darken(c, 0.3)).alpha(0.35);
        self.rect(8.0, -58.0, 18.0, 58.0, 0.0, &sh);
        let g = self.pn([76, 56, 80]);
        self.path("M-9,0 L-9,-20 Q0,-32 9,-20 L9,0 Z", &g);
        let w2 = self.pn([250, 214, 120]);
        self.rect(-4.0, -46.0, 8.0, 10.0, 4.0, &w2);
        self.limb(-36.0, -100.0, -36.0, -112.0, 1.4, [90, 70, 70]);
        let f = self.pn([240, 200, 70]);
        self.poly(&[(-36.0, -112.0), (-24.0, -108.0), (-36.0, -104.0)], &f);
    }

    pub fn rocket(&mut self, col: Option<Col>) {
        let c = col.unwrap_or([236, 238, 246]);
        let g = self.glow([255, 180, 90], 0.8);
        self.raw(&format!("<circle cx=\"0\" cy=\"6\" r=\"30\" fill=\"{g}\"/>"));
        let fl = self.pn([255, 150, 40]);
        self.path("M-7,-6 Q0,34 7,-6 Z", &fl);
        let fy = self.flat([255, 230, 120]);
        self.path("M-4,-6 Q0,20 4,-6 Z", &fy);
        let fin = self.pn([220, 70, 80]);
        self.path("M-12,-20 L-24,-2 L-10,-8 Z", &fin);
        self.path("M12,-20 L24,-2 L10,-8 Z", &fin);
        let b = self.pn(c);
        self.path("M0,-96 C16,-76 16,-44 11,-8 L-11,-8 C-16,-44 -16,-76 0,-96 Z", &b);
        let sh = self.flat(darken(c, 0.25)).alpha(0.45);
        self.path("M0,-96 C16,-76 16,-44 11,-8 L4,-8 C8,-44 8,-76 0,-96 Z", &sh);
        let nose = self.pn([220, 70, 80]);
        self.path("M0,-96 C8,-86 11,-78 12,-72 L-12,-72 C-11,-78 -8,-86 0,-96 Z", &nose);
        let wnd = self.pn([110, 190, 235]);
        self.circ(0.0, -52.0, 6.2, &wnd);
        let hi = self.flat([255, 255, 255]).alpha(0.7);
        self.circ(-1.8, -54.0, 2.0, &hi);
        let band = self.flat([220, 70, 80]);
        self.rect(-11.4, -28.0, 22.8, 4.0, 0.0, &band);
    }

    /// A campfire: logs, flames, sparks and a warm glow.
    pub fn fire(&mut self) {
        let g = self.glow([255, 170, 70], 0.8);
        self.raw(&format!("<circle cx=\"0\" cy=\"-14\" r=\"52\" fill=\"{g}\"/>"));
        self.shadow(0.0, 1.0, 20.0);
        let st = self.pn([120, 120, 128]);
        for i in 0..6 {
            let a = i as f32 * 1.05;
            self.circ(a.cos() * 15.0, 2.0 + a.sin() * 3.0, 3.6, &st);
        }
        self.limb(-13.0, 1.0, 9.0, -6.0, 5.0, [112, 76, 48]);
        self.limb(13.0, 1.0, -9.0, -6.0, 5.0, [96, 64, 40]);
        let o = self.pn([255, 120, 32]);
        self.path("M0,-42 C12,-28 14,-18 10,-10 C6,-4 -6,-4 -10,-10 C-14,-18 -10,-26 -4,-30 C-4,-22 0,-20 2,-24 C4,-30 -1,-36 0,-42 Z", &o);
        let y = self.flat([255, 214, 80]);
        self.path("M0,-28 C7,-20 8,-14 5,-9 C2,-6 -3,-6 -5,-9 C-7,-14 -3,-20 0,-28 Z", &y);
        let w = self.flat([255, 250, 210]);
        self.path("M0,-17 C3,-13 3,-10 1.5,-8 C-0.5,-8 -2,-10 0,-17 Z", &w);
        for (x, y) in [(10.0, -36.0), (-9.0, -40.0), (4.0, -50.0)] {
            self.raw(&format!("<circle cx=\"{x}\" cy=\"{y}\" r=\"1.1\" fill=\"#ffd070\"/>"));
        }
    }

    pub fn mushroom(&mut self, col: Option<Col>) {
        let c = col.unwrap_or([214, 70, 70]);
        self.shadow(0.0, 0.0, 10.0);
        for (dx, s) in [(0.0, 1.0), (9.0, 0.6)] {
            self.open(dx, 0.0, s, false);
            let st = self.pn([246, 238, 220]);
            self.path("M-4,0 Q-3,-10 -3.4,-14 L3.4,-14 Q3,-10 4,0 Z", &st);
            let cap = self.pn(c);
            self.path("M-12,-13 Q-11,-27 0,-28 Q11,-27 12,-13 Q0,-10 -12,-13 Z", &cap);
            let sp = self.flat([255, 255, 255]).alpha(0.9);
            self.circ(-4.0, -21.0, 2.2, &sp);
            self.circ(4.0, -19.0, 1.8, &sp);
            self.circ(0.0, -25.0, 1.4, &sp);
            self.close();
        }
    }

    pub fn rock(&mut self, col: Option<Col>) {
        let c = col.unwrap_or([138, 138, 148]);
        self.shadow(0.0, 0.0, 22.0);
        let p = self.pn(c);
        self.path("M-22,0 Q-24,-12 -12,-18 Q0,-24 12,-18 Q24,-12 22,0 Z", &p);
        let l = self.flat(lighten(c, 0.3)).alpha(0.55);
        self.path("M-16,-10 Q-10,-18 0,-19 Q-8,-14 -16,-10 Z", &l);
        let d = self.flat(darken(c, 0.3)).alpha(0.4);
        self.path("M22,0 Q24,-12 12,-18 Q16,-8 14,0 Z", &d);
    }

    pub fn cactus(&mut self, col: Option<Col>) {
        let c = col.unwrap_or([84, 156, 90]);
        self.shadow(0.0, 0.0, 16.0);
        let p = self.pn(c);
        self.rect(-6.0, -58.0, 12.0, 58.0, 6.0, &p);
        self.path("M-6,-30 L-16,-30 Q-20,-30 -20,-36 L-20,-46 Q-20,-50 -16,-50 Q-12,-50 -12,-46 L-12,-36 L-6,-36 Z", &p);
        self.path("M6,-24 L14,-24 Q18,-24 18,-30 L18,-40 Q18,-44 14,-44 Q10,-44 10,-40 L10,-30 L6,-30 Z", &p);
        let sh = self.flat(darken(c, 0.25)).alpha(0.4);
        self.rect(1.0, -56.0, 5.0, 56.0, 3.0, &sh);
        for y in [-48.0, -38.0, -28.0, -18.0, -8.0] {
            self.curve(&format!("M-1,{} l0,-3 M3,{} l0,-3", n(y), n(y + 4.0)), [236, 232, 190], 0.9, 0.9);
        }
        let f = self.pn([240, 100, 140]);
        self.circ(0.0, -60.0, 3.4, &f);
    }

    pub fn barn(&mut self, col: Option<Col>) {
        let c = col.unwrap_or([190, 62, 58]);
        self.shadow(0.0, 0.0, 52.0);
        let p = self.pn(c);
        self.path("M-46,0 L-46,-34 L-38,-52 Q0,-70 38,-52 L46,-34 L46,0 Z", &p);
        let sh = self.flat(darken(c, 0.3)).alpha(0.35);
        self.rect(24.0, -52.0, 22.0, 52.0, 0.0, &sh);
        for i in 0..6 {
            self.line(-40.0 + i as f32 * 16.0, -48.0, -40.0 + i as f32 * 16.0, -1.0, darken(c, 0.25), 1.0);
        }
        let w = self.pn([248, 240, 224]);
        self.rect(-18.0, -34.0, 36.0, 34.0, 1.0, &w);
        self.line(-18.0, -34.0, 18.0, 0.0, c, 3.0);
        self.line(18.0, -34.0, -18.0, 0.0, c, 3.0);
        let lo = self.pn([248, 240, 224]);
        self.circ(0.0, -48.0, 5.0, &lo);
        let roof = self.pn([120, 80, 70]);
        self.path("M-50,-34 L-40,-54 Q0,-74 40,-54 L50,-34 L46,-34 L38,-50 Q0,-66 -38,-50 L-46,-34 Z", &roof);
    }

    pub fn fence(&mut self) {
        let wood = [196, 150, 100];
        let rail = self.pn(wood);
        self.rect(-60.0, -20.0, 120.0, 4.0, 1.0, &rail);
        self.rect(-60.0, -9.0, 120.0, 4.0, 1.0, &rail);
        for i in 0..7 {
            let x = -57.0 + i as f32 * 19.0;
            let p = self.pn(wood);
            self.path(&format!("M{},0 L{},-23 L{},-27 L{},-23 L{},0 Z", n(x - 3.5), n(x - 3.5), n(x), n(x + 3.5), n(x + 3.5)), &p);
        }
        self.shadow(0.0, 0.0, 56.0);
    }

    pub fn tent(&mut self, col: Option<Col>) {
        let c = col.unwrap_or([226, 130, 70]);
        self.shadow(0.0, 0.0, 36.0);
        let p = self.pn(c);
        self.poly(&[(-36.0, 0.0), (0.0, -50.0), (36.0, 0.0)], &p);
        let sh = self.flat(darken(c, 0.3)).alpha(0.4);
        self.poly(&[(0.0, -50.0), (36.0, 0.0), (8.0, 0.0)], &sh);
        let d = self.pn(darken(c, 0.55));
        self.poly(&[(-10.0, 0.0), (0.0, -26.0), (10.0, 0.0)], &d);
        self.limb(0.0, -50.0, 0.0, -60.0, 1.4, [100, 80, 60]);
        let f = self.pn([240, 220, 90]);
        self.poly(&[(0.0, -60.0), (10.0, -57.0), (0.0, -54.0)], &f);
    }

    /// A lantern on a post, glowing.
    pub fn lantern_post(&mut self) {
        let g = self.glow([255, 214, 120], 0.9);
        self.raw(&format!("<circle cx=\"0\" cy=\"-46\" r=\"40\" fill=\"{g}\"/>"));
        let p = self.pn([70, 60, 66]);
        self.rect(-1.8, -40.0, 3.6, 40.0, 0.5, &p);
        self.ell(0.0, -1.0, 6.0, 2.0, &p);
        self.rect(-6.0, -52.0, 12.0, 3.0, 1.0, &p);
        let gl = self.pn([255, 224, 130]);
        self.rect(-4.6, -49.0, 9.2, 9.0, 1.5, &gl);
        self.poly(&[(-7.0, -52.0), (0.0, -58.0), (7.0, -52.0)], &p);
    }

    pub fn planet(&mut self, col: Option<Col>) {
        let c = col.unwrap_or([226, 150, 90]);
        let p = self.pn(c);
        self.circ(0.0, 0.0, 24.0, &p);
        let b = self.flat(darken(c, 0.2)).alpha(0.5);
        self.ell(0.0, 6.0, 23.0, 5.0, &b);
        self.ell(-2.0, -9.0, 20.0, 3.0, &b);
        let l = self.flat(lighten(c, 0.35)).alpha(0.5);
        self.circ(-8.0, -9.0, 8.0, &l);
        let ring = hex(self.tone(lighten(c, 0.3)));
        self.raw(&format!("<ellipse cx=\"0\" cy=\"0\" rx=\"40\" ry=\"8\" fill=\"none\" stroke=\"{ring}\" stroke-width=\"2.6\" transform=\"rotate(-16)\" opacity=\"0.85\"/>"));
    }

    /// A small flock of birds in the sky.
    pub fn birds(&mut self, col: Option<Col>) {
        let c = col.unwrap_or([56, 54, 74]);
        for (x, y, s) in [(0.0, 0.0, 1.0), (-16.0, 8.0, 0.8), (14.0, 9.0, 0.85)] {
            self.curve(&format!("M{},{} q{},{} {},0 q{},-{} {},0", n(x - 8.0 * s), n(y), n(4.0 * s), n(-7.0 * s), n(8.0 * s), n(4.0 * s), n(7.0 * s), n(8.0 * s)), c, 2.0, 1.0);
        }
    }

    pub fn lake(&mut self, rx: f32, ry: f32, col: Option<Col>) {
        let c = col.unwrap_or([84, 164, 214]);
        let bank = self.pn(mix(c, [150, 120, 80], 0.6));
        self.ell(0.0, 1.5, rx + 3.0, ry + 2.0, &bank);
        let g = self.lin(&[lighten(c, 0.2), darken(c, 0.15)], 0.0, 1.0);
        let st = self.pn(c).st;
        self.raw(&format!("<ellipse cx=\"0\" cy=\"0\" rx=\"{}\" ry=\"{}\" fill=\"{g}\"{st}/>", n(rx), n(ry)));
        let hi = self.flat([255, 255, 255]).alpha(0.4);
        self.ell(-rx * 0.3, -ry * 0.3, rx * 0.35, ry * 0.12, &hi);
        self.ell(rx * 0.2, ry * 0.25, rx * 0.25, ry * 0.1, &hi);
        for i in 0..3 {
            let x = rx * (-0.4 + i as f32 * 0.4);
            self.curve(&format!("M{},{} q6,-2 12,0", n(x), n(ry * 0.5)), [255, 255, 255], 1.0, 0.6);
        }
    }
}
