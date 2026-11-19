//! People of the scene composer: a cartoon figure with a big head, rounded limbs, hair styles, eyes, brows and a mouth that
//! show an expression, outfits, shoes, props (torch, lantern, backpack, hat, balloon, book) and poses (stand, walk, run,
//! sit, wave, point, look up, climb, hold hands). Drawn facing right in a box 100 units tall standing at the origin; the
//! composer flips and scales it. The same [`Person`] always draws the same figure.

use super::art::*;
use super::sheet::{hair_rgb, skin_rgb, CharacterSheet};
use super::{Expr, Pose, Prop};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Age {
    Baby,
    Child,
    Adult,
    Elder,
}

#[derive(Clone, Debug)]
pub struct Person {
    pub age: Age,
    pub skin: Col,
    pub hair: Col,
    pub hair_style: String,
    pub top: Col,
    pub bottom: Col,
    pub shoes: Col,
    pub skirt: bool,
    pub shorts: bool,
    pub hat: Option<Col>,
    pub glasses: bool,
    pub pose: Pose,
    pub expr: Expr,
    pub holds: Option<Prop>,
    /// Where the eyes look: -1 left, 1 right (the way the figure faces).
    pub gaze: f32,
}

impl Person {
    /// The story's hero from the character sheet.
    pub fn hero(sheet: &CharacterSheet, pose: Pose, expr: Expr, holds: Option<Prop>) -> Person {
        let c = |n: &str, d: Col| super::colour(n).unwrap_or(d);
        let age = match sheet.age.as_str() {
            "child" => Age::Child,
            "teen" => Age::Child,
            _ => Age::Adult,
        };
        Person {
            age,
            skin: skin_rgb(&sheet.skin),
            hair: hair_rgb(&sheet.hair),
            hair_style: sheet.hair_style.clone(),
            top: c(&sheet.top, [230, 190, 60]),
            bottom: c(&sheet.bottom, [60, 100, 170]),
            shoes: c(&sheet.shoes, [120, 80, 50]),
            skirt: sheet.bottom_style == "skirt",
            shorts: sheet.bottom_style == "shorts",
            hat: if sheet.hat.is_empty() { None } else { super::colour(&sheet.hat) },
            glasses: false,
            pose,
            expr,
            holds,
            gaze: 1.0,
        }
    }

    /// Another person (not the hero): their look follows from the role and the position in the picture, so it is stable.
    pub fn other(role: super::Kind, seed: u32, color: Option<Col>, pose: Pose, expr: Expr, holds: Option<Prop>) -> Person {
        let skins = [[0xf0, 0xc6, 0xa0], [0xb9, 0x7c, 0x52], [0xdc, 0xa5, 0x78], [0x84, 0x52, 0x36], [0xf6, 0xd9, 0xc4]];
        let skin = skins[seed as usize % skins.len()];
        let tops = [[70, 130, 200], [200, 90, 80], [90, 160, 100], [220, 170, 60], [140, 100, 180]];
        let top = color.unwrap_or(tops[(seed as usize / 3) % tops.len()]);
        let (age, hair, style, skirt) = match role {
            super::Kind::Man => (Age::Adult, [0x30, 0x24, 0x1c], "short", false),
            super::Kind::Woman => (Age::Adult, [0x4a, 0x2c, 0x20], if seed % 2 == 0 { "long" } else { "bun" }, true),
            super::Kind::Elder => (Age::Elder, [0xea, 0xe8, 0xe4], if seed % 2 == 0 { "bun" } else { "bald" }, seed % 2 == 0),
            _ => (Age::Baby, [0x3a, 0x28, 0x1c], "short", false),
        };
        Person { age, skin, hair, hair_style: style.into(), top, bottom: if skirt { [150, 70, 90] } else { [70, 80, 110] }, shoes: [90, 64, 48], skirt, shorts: age == Age::Baby, hat: None, glasses: age == Age::Elder, pose, expr, holds, gaze: 1.0 }
    }
}

fn polar(x: f32, y: f32, len: f32, deg: f32) -> (f32, f32) {
    let a = deg.to_radians();
    (x + a.sin() * len, y + a.cos() * len)
}

impl Art {
    /// Draw a person with the feet at the origin, facing right.
    pub fn person(&mut self, p: &Person) {
        let (hr, hy, sw, hipw, torso_top, torso_bot, leg_len, arm_len, limb_w) = match p.age {
            Age::Baby => (21.0, -66.0, 11.0, 10.0, -45.0, -22.0, 20.0, 18.0, 8.0),
            Age::Child => (17.0, -80.0, 12.0, 10.5, -62.0, -35.0, 34.0, 26.0, 8.4),
            Age::Adult => (14.0, -84.0, 14.5, 12.0, -68.0, -38.0, 38.0, 30.0, 8.6),
            Age::Elder => (14.0, -80.0, 13.0, 12.0, -65.0, -37.0, 36.0, 28.0, 8.4),
        };
        let sit = p.pose == Pose::Sit;
        let (la, ra, fl, nl, lean): (f32, f32, f32, f32, f32) = match p.pose {
            Pose::Stand => (-9.0, 9.0, 3.0, -3.0, 0.0),
            Pose::Walk => (-32.0, 32.0, -18.0, 20.0, 2.0),
            Pose::Run => (-70.0, 78.0, -42.0, 44.0, 9.0),
            Pose::Sit => (24.0, 12.0, 88.0, 84.0, 0.0),
            Pose::Wave => (-9.0, 155.0, 3.0, -3.0, 0.0),
            Pose::Point => (-9.0, 98.0, 3.0, -4.0, 0.0),
            Pose::LookUp => (-10.0, 10.0, 4.0, -4.0, -3.0),
            Pose::Climb => (168.0, -165.0, 22.0, -30.0, 0.0),
            Pose::HoldHands => (-9.0, 104.0, 3.0, -4.0, 0.0),
        };
        // props change the near arm
        let (mut ra, mut la) = (ra, la);
        match p.holds {
            Some(Prop::Torch) | Some(Prop::Lantern) if matches!(p.pose, Pose::Stand | Pose::Walk | Pose::LookUp | Pose::Sit) => ra = if p.holds == Some(Prop::Torch) { 128.0 } else { 78.0 },
            Some(Prop::Balloon) => ra = 150.0,
            Some(Prop::Book) => {
                ra = 62.0;
                la = 52.0;
            }
            _ => {}
        }
        let ground = 0.0;
        let hipy = if sit { -14.0 } else { torso_bot };
        let dy = if sit { 24.0 } else { 0.0 };
        let _ = ground;
        // shadow
        self.shadow(if sit { 12.0 } else { 0.0 }, 1.5, if sit { 34.0 } else { 20.0 });
        let skin_sh = darken(p.skin, 0.2);
        let (hx, hip_l, hip_r) = (0.0f32, -hipw * 0.5, hipw * 0.5);
        // ---- the back: backpack, far arm, hair behind the head
        if p.holds == Some(Prop::Backpack) {
            self.open_raw(&format!("transform=\"translate(0 {}) rotate({} 0 {})\"", n(dy), n(lean), n(hipy)));
            let c = [210, 90, 70];
            let pn = self.pn(c);
            self.rect(-sw - 10.0, torso_top + 3.0, 15.0, (torso_bot - torso_top) * 0.95, 4.0, &pn);
            let pn = self.pn(darken(c, 0.2));
            self.rect(-sw - 8.0, torso_top + 14.0, 11.0, 8.0, 2.0, &pn);
            self.close();
        }
        // ---- legs
        let leg = |s: &mut Art, hxp: f32, ang: f32, far: bool| {
            let (kx, ky, fx, fy);
            if sit {
                let hy0 = -9.0 - if far { 2.0 } else { 0.0 };
                fx = hxp + leg_len * 0.95;
                fy = hy0 + if far { -3.0 } else { 1.0 };
                kx = hxp + leg_len * 0.5;
                ky = hy0 - 5.0;
                let c = if p.skirt { p.skin } else { p.bottom };
                s.limb(hxp, hy0, kx, ky, limb_w, c);
                s.limb(kx, ky, fx, fy, limb_w * 0.9, if p.shorts || p.skirt { p.skin } else { p.bottom });
                shoe(s, fx, fy, p.shoes, 0.0);
                return;
            }
            let hy0 = torso_bot + 1.0;
            let (thigh, shin) = if p.pose == Pose::Run && !far { (ang, ang - 62.0) } else if p.pose == Pose::Run { (ang, ang + 40.0) } else if p.pose == Pose::Walk && far { (ang, ang * 0.6) } else { (ang, ang * 0.7) };
            let (k0, k1) = polar(hxp, hy0, leg_len * 0.5, thigh);
            kx = k0;
            ky = k1;
            let (a0, a1) = polar(kx, ky, leg_len * 0.5, shin);
            fx = a0;
            fy = a1;
            // clamp so the foot rests on the ground line
            let lift = if p.pose == Pose::Run && !far { 0.0 } else { (-fy - 4.0).max(0.0).min(0.0) };
            let _ = lift;
            s.limb(hxp, hy0, kx, ky, limb_w, if p.shorts || p.skirt { p.skin } else { p.bottom });
            s.limb(kx, ky, fx, fy, limb_w * 0.92, if p.shorts || p.skirt { p.skin } else { p.bottom });
            if p.shorts {
                s.limb(hxp, hy0, hxp + (kx - hxp) * 0.8, hy0 + (ky - hy0) * 0.8, limb_w + 1.2, p.bottom);
            }
            shoe(s, fx, fy, p.shoes, if p.pose == Pose::Run && !far { -14.0 } else { 0.0 });
        };
        if p.pose == Pose::Climb {
            leg(self, hip_l, -30.0, true);
            leg(self, hip_r, 22.0, false);
        } else {
            leg(self, hip_l, fl, true);
            leg(self, hip_r, nl, false);
        }
        // ---- the upper body, leaning and (when sitting) lowered
        self.open_raw(&format!("transform=\"translate(0 {}) rotate({} 0 {})\"", n(dy), n(lean), n(hipy)));
        // far arm
        let shoulder_y = torso_top + 7.0;
        self.arm(-sw + 2.0, shoulder_y, la * -1.0 * 0.0 + la, arm_len, limb_w, p, true);
        // torso
        let shirt = self.pn(p.top);
        let t = format!("M{},{} Q{},{} {},{} L{},{} Q{},{} {},{} Z", n(-sw), n(torso_top + 4.0), n(-sw), n(torso_top - 1.0), n(-sw + 5.0), n(torso_top - 1.0), n(sw - 5.0), n(torso_top - 1.0), n(sw), n(torso_top - 1.0), n(sw), n(torso_top + 4.0));
        let _ = t;
        let hw = hipw + 1.5;
        let body = format!("M{},{} Q{},{} {},{} L{},{} L{},{} L{},{} L{},{} L{},{} L{},{} Z", n(-sw + 3.0), n(torso_top), n(0.0), n(torso_top - 4.0), n(sw - 3.0), n(torso_top), n(sw), n(torso_top + 6.0), n(sw + 1.0), n(torso_bot - 4.0), n(hw), n(torso_bot + 1.0), n(-hw), n(torso_bot + 1.0), n(-sw - 1.0), n(torso_bot - 4.0), n(-sw), n(torso_top + 6.0));
        if p.skirt {
            let sk = self.pn(p.bottom);
            let (w0, w1) = (hw + 1.0, hw + 11.0 + if p.age == Age::Adult { 3.0 } else { 0.0 });
            let sy = torso_bot - 9.0;
            self.path(&format!("M{},{} L{},{} L{},{} L{},{} Z", n(-w0), n(sy), n(w0), n(sy), n(w1), n(torso_bot + 15.0), n(-w1), n(torso_bot + 15.0)), &sk);
            let shade = self.flat(darken(p.bottom, 0.3)).alpha(0.4);
            self.poly(&[(w0 * 0.2, sy), (w0, sy), (w1, torso_bot + 15.0), (w1 * 0.3, torso_bot + 15.0)], &shade);
            self.curve(&format!("M{},{} Q{},{} {},{}", n(-w1 + 3.0), n(torso_bot + 11.0), n(0.0), n(torso_bot + 15.0), n(w1 - 3.0), n(torso_bot + 11.0)), lighten(p.bottom, 0.4), 1.0, 0.6);
        } else {
            // the waist of shorts or trousers
            let b = self.pn(p.bottom);
            self.rect(-hw, torso_bot - 7.0, hw * 2.0, 9.0, 3.0, &b);
        }
        self.path(&body, &shirt);
        // shade on the side away from the light, a collar and a stripe or two
        let sh = self.flat(darken(p.top, 0.3)).alpha(0.38);
        self.path(&format!("M{},{} L{},{} Q{},{} {},{} L{},{} Z", n(sw * 0.35), n(torso_top), n(sw - 3.0), n(torso_top), n(sw + 1.0), n(torso_top + 6.0), n(sw + 1.0), n(torso_bot - 4.0), n(hw * 0.5), n(torso_bot)), &sh);
        let st = self.flat(lighten(p.top, 0.45)).alpha(0.7);
        let mid = (torso_top + torso_bot) / 2.0;
        if p.age != Age::Elder && !p.skirt {
            self.rect(-sw + 1.0, mid - 6.0, sw * 2.0 - 2.0, 3.2, 0.0, &st);
        }
        let col = self.flat(p.skin);
        self.ell(0.0, torso_top + 0.5, 5.2, 3.0, &col);
        // head and neck
        let tilt = if p.pose == Pose::LookUp { -10.0 } else { 0.0 };
        let neck = self.pn(p.skin);
        self.rect(-3.8, hy + hr - 6.0, 7.6, torso_top - (hy + hr) + 9.0, 3.0, &neck);
        let ns = self.flat(skin_sh).alpha(0.5);
        self.rect(-3.8, hy + hr - 3.0, 7.6, 5.0, 2.0, &ns);
        self.open_raw(&format!("transform=\"rotate({} 0 {})\"", n(tilt), n(hy + hr - 3.0)));
        self.head(p, hx, hy, hr);
        self.close();
        // near arm (in front of the body)
        self.arm(sw - 2.0, shoulder_y, ra, arm_len, limb_w, p, false);
        self.close();
    }

    fn arm(&mut self, sx: f32, sy: f32, ang: f32, len: f32, w: f32, p: &Person, far: bool) {
        // the elbow bends a little towards the front, more when the arm is raised
        let bend = if ang.abs() > 100.0 { 14.0 } else if ang.abs() > 40.0 { 16.0 } else { 6.0 };
        let sign = if ang >= 0.0 { 1.0 } else { -1.0 };
        let (ex, ey) = polar(sx, sy, len * 0.52, ang - sign * bend * 0.3);
        let (hx, hy) = polar(ex, ey, len * 0.52, ang + sign * bend);
        let sleeve = if p.age == Age::Elder { 0.95 } else { 0.55 };
        self.limb(sx, sy, ex, ey, w, p.skin);
        self.limb(ex, ey, hx, hy, w * 0.9, p.skin);
        // sleeve over the upper part
        let (sx2, sy2) = (sx + (ex - sx) * sleeve * 1.0, sy + (ey - sy) * sleeve);
        self.limb(sx, sy, sx2, sy2, w + 1.2, p.top);
        let hand = self.pn(p.skin);
        self.circ(hx, hy, w * 0.62, &hand);
        if !far {
            match p.holds {
                Some(Prop::Torch) => self.torch(hx, hy, ang),
                Some(Prop::Lantern) => self.lantern(hx, hy),
                Some(Prop::Balloon) => self.balloon(hx, hy),
                Some(Prop::Book) => {
                    let b = self.pn([70, 110, 190]);
                    self.rect(hx - 2.0, hy - 8.0, 16.0, 12.0, 1.5, &b);
                    let pg = self.flat([250, 246, 232]);
                    self.rect(hx + 0.5, hy - 6.5, 12.0, 9.0, 0.5, &pg);
                    self.line(hx + 6.5, hy - 6.5, hx + 6.5, hy + 2.5, [160, 150, 130], 0.8);
                }
                _ => {}
            }
        }
    }

    fn torch(&mut self, hx: f32, hy: f32, ang: f32) {
        // the stick points along the arm and a little up, the fire sits on its end
        let (tx, ty) = (hx + 6.0, hy - 22.0);
        let _ = ang;
        let g = self.glow([255, 190, 80], 0.85);
        self.raw(&format!("<circle cx=\"{}\" cy=\"{}\" r=\"58\" fill=\"{g}\"/>", n(tx), n(ty - 6.0)));
        self.limb(hx, hy + 2.0, tx, ty + 4.0, 3.6, [120, 80, 48]);
        let o = self.pn([255, 140, 40]);
        self.path(&format!("M{},{} C{},{} {},{} {},{} C{},{} {},{} {},{} Z", n(tx), n(ty - 20.0), n(tx + 9.0), n(ty - 9.0), n(tx + 8.0), n(ty + 1.0), n(tx), n(ty + 2.0), n(tx - 8.0), n(ty + 1.0), n(tx - 9.0), n(ty - 9.0), n(tx), n(ty - 20.0)), &o);
        let y = self.flat([255, 226, 110]);
        self.path(&format!("M{},{} C{},{} {},{} {},{} C{},{} {},{} {},{} Z", n(tx), n(ty - 13.0), n(tx + 5.0), n(ty - 6.0), n(tx + 4.0), n(ty), n(tx), n(ty), n(tx - 4.0), n(ty), n(tx - 5.0), n(ty - 6.0), n(tx), n(ty - 13.0)), &y);
    }

    fn lantern(&mut self, hx: f32, hy: f32) {
        let g = self.glow([255, 214, 120], 0.9);
        self.raw(&format!("<circle cx=\"{}\" cy=\"{}\" r=\"44\" fill=\"{g}\"/>", n(hx + 3.0), n(hy + 6.0)));
        self.curve(&format!("M{},{} q4,-9 8,0", n(hx - 1.0), n(hy + 1.0)), [60, 50, 50], 1.4, 1.0);
        let fr = self.pn([70, 56, 56]);
        self.rect(hx - 4.0, hy + 1.0, 14.0, 3.0, 1.0, &fr);
        let gl = self.pn([255, 222, 130]);
        self.rect(hx - 3.0, hy + 4.0, 12.0, 12.0, 2.0, &gl);
        self.rect(hx - 4.0, hy + 16.0, 14.0, 3.0, 1.0, &fr);
        let hi = self.flat([255, 250, 210]).alpha(0.8);
        self.rect(hx - 1.0, hy + 6.0, 4.0, 8.0, 1.0, &hi);
    }

    fn balloon(&mut self, hx: f32, hy: f32) {
        let (bx, by) = (hx + 4.0, hy - 38.0);
        self.curve(&format!("M{},{} Q{},{} {},{}", n(hx), n(hy), n(hx + 6.0), n(hy - 20.0), n(bx), n(by + 14.0)), [90, 80, 90], 0.9, 0.9);
        let b = self.pn([232, 70, 90]);
        self.ell(bx, by, 11.0, 14.0, &b);
        self.poly(&[(bx - 2.0, by + 13.0), (bx + 2.0, by + 13.0), (bx, by + 17.0)], &b);
        let hi = self.flat([255, 255, 255]).alpha(0.55);
        self.ell(bx - 4.0, by - 5.0, 2.8, 4.5, &hi);
    }

    fn head(&mut self, p: &Person, hx: f32, hy: f32, hr: f32) {
        let back_hair = |s: &mut Art| {
            let hc = p.hair;
            match p.hair_style.as_str() {
                "long" => {
                    let c = s.pn(hc);
                    s.path(&format!("M{},{} Q{},{} {},{} L{},{} Q{},{} {},{} Z", n(hx - hr - 3.0), n(hy), n(hx - hr - 5.0), n(hy + hr * 1.7), n(hx - hr * 0.2), n(hy + hr * 2.0), n(hx + hr * 0.6), n(hy + hr * 1.9), n(hx + hr + 3.0), n(hy + hr * 1.5), n(hx + hr + 3.0), n(hy)), &c);
                }
                "ponytail" => {
                    let c = s.pn(hc);
                    s.path(&format!("M{},{} Q{},{} {},{} Q{},{} {},{} Z", n(hx - hr * 0.8), n(hy - hr * 0.5), n(hx - hr * 2.2), n(hy - hr * 0.4), n(hx - hr * 1.7), n(hy + hr * 1.5), n(hx - hr * 1.2), n(hy + hr * 0.8), n(hx - hr * 0.8), n(hy + hr * 0.2)), &c);
                }
                "braids" => {
                    for sgn in [-1.0f32, 1.0] {
                        for i in 0..4 {
                            let c = s.pn(hc);
                            s.ell(hx + sgn * (hr + 1.5), hy + hr * 0.5 + i as f32 * 5.0, 3.4, 3.0, &c);
                        }
                        let b = s.pn([220, 80, 100]);
                        s.circ(hx + sgn * (hr + 1.5), hy + hr * 0.5 + 19.0, 2.0, &b);
                    }
                }
                _ => {}
            }
        };
        back_hair(self);
        // ears
        let ear = self.pn(p.skin);
        self.circ(hx - hr + 0.5, hy + 1.5, hr * 0.2, &ear);
        self.circ(hx + hr - 0.5, hy + 1.5, hr * 0.2, &ear);
        // the face
        let face = self.pn(p.skin);
        self.ell(hx, hy, hr * 0.98, hr, &face);
        let sh = self.flat(darken(p.skin, 0.18)).alpha(0.32);
        self.path(&format!("M{},{} Q{},{} {},{} Q{},{} {},{} Z", n(hx + hr * 0.5), n(hy - hr * 0.8), n(hx + hr * 1.1), n(hy), n(hx + hr * 0.5), n(hy + hr * 0.9), n(hx + hr * 0.95), n(hy + hr * 0.1), n(hx + hr * 0.5), n(hy - hr * 0.8)), &sh);
        // hair
        let hc = p.hair;
        match p.hair_style.as_str() {
            "bald" => {}
            "curly" => {
                let mut cs = Vec::new();
                for i in 0..7 {
                    let a = std::f32::consts::PI * (1.05 + i as f32 * 0.15);
                    cs.push((hx + a.cos() * hr * 0.92, hy - 1.0 + a.sin() * hr * 0.95, hr * 0.34));
                }
                cs.push((hx, hy - hr * 0.7, hr * 0.5));
                self.blobs(&cs, hc);
            }
            _ => {
                let c = self.pn(hc);
                let (l, r, top) = (hx - hr - 1.2, hx + hr + 1.2, hy - hr - 1.6);
                self.path(&format!("M{},{} Q{},{} {},{} Q{},{} {},{} Q{},{} {},{} Q{},{} {},{} Q{},{} {},{} Z", n(l), n(hy + 2.0), n(l - 1.0), n(top), n(hx), n(top), n(r + 1.0), n(top), n(r), n(hy + 2.0), n(r - 2.0), n(hy - hr * 0.5), n(hx + hr * 0.35), n(hy - hr * 0.72), n(hx - hr * 0.1), n(hy - hr * 0.35), n(hx - hr * 0.55), n(hy - hr * 0.45), n(l + 2.0), n(hy - hr * 0.2), n(l), n(hy + 2.0)), &c);
                let hi = self.flat(lighten(hc, 0.4)).alpha(0.5);
                self.path(&format!("M{},{} Q{},{} {},{}", n(hx - hr * 0.7), n(hy - hr * 0.8), n(hx - hr * 0.1), n(hy - hr * 1.05), n(hx + hr * 0.4), n(hy - hr * 0.85)), &hi);
                if p.hair_style == "bun" {
                    let b = self.pn(hc);
                    self.circ(hx - hr * 0.15, hy - hr - 5.0, hr * 0.42, &b);
                }
            }
        }
        // hat
        if let Some(h) = p.hat.or(if p.holds == Some(Prop::Hat) { Some([200, 70, 70]) } else { None }) {
            let c = self.pn(h);
            self.path(&format!("M{},{} Q{},{} {},{} Q{},{} {},{} Z", n(hx - hr - 1.0), n(hy - hr * 0.35), n(hx - hr), n(hy - hr * 1.5), n(hx), n(hy - hr * 1.5), n(hx + hr), n(hy - hr * 1.5), n(hx + hr + 1.0), n(hy - hr * 0.35)), &c);
            let br = self.pn(darken(h, 0.15));
            self.ell(hx + 3.0, hy - hr * 0.38, hr * 1.3, 3.6, &br);
        }
        // face features, the eyes looking where the figure looks
        let g = p.gaze * 2.2;
        let look_up = if p.pose == Pose::LookUp { -1.8 } else { 0.0 };
        let ey = hy + hr * 0.08;
        for (i, ex) in [hx - hr * 0.36 + g, hx + hr * 0.36 + g].iter().enumerate() {
            let _ = i;
            if p.expr == Expr::Sleepy {
                self.curve(&format!("M{},{} Q{},{} {},{}", n(ex - 3.4), n(ey), n(*ex), n(ey + 3.0), n(ex + 3.4), n(ey)), INKC, 1.5, 0.95);
            } else {
                let big = if p.expr == Expr::Surprised { 1.15 } else { 1.0 };
                let w = self.flat([255, 255, 255]);
                self.ell(*ex, ey, 3.1 * big, 3.7 * big, &w);
                self.raw(&format!("<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" fill=\"none\" stroke=\"#3a2a30\" stroke-width=\"0.6\" opacity=\"0.5\"/>", n(*ex), n(ey), n(3.1 * big), n(3.7 * big)));
                let iris = self.flat([70, 44, 36]);
                self.circ(ex + g * 0.25, ey + look_up + 0.2, 2.3 * big, &iris);
                let pu = self.flat([20, 14, 20]);
                self.circ(ex + g * 0.28, ey + look_up + 0.2, 1.3 * big, &pu);
                let hi = self.flat([255, 255, 255]);
                self.circ(ex + g * 0.25 + 0.9, ey + look_up - 0.9, 0.8, &hi);
            }
            // eyebrows
            let (b0, b1) = match p.expr {
                Expr::Surprised => (-8.2, -8.2),
                Expr::Scared => (-6.2, -8.6),
                Expr::Sleepy => (-4.6, -4.6),
                Expr::Happy => (-6.6, -7.2),
                Expr::Neutral => (-6.4, -6.4),
            };
            let (a, b) = if i == 0 { (b1, b0) } else { (b0, b1) };
            self.curve(&format!("M{},{} Q{},{} {},{}", n(ex - 3.4), n(ey + a), n(*ex), n(ey + (a + b) / 2.0 - 1.4), n(ex + 3.4), n(ey + b)), darken(hc, 0.1), 1.3, 0.95);
        }
        // nose and mouth
        let nose = self.flat(darken(p.skin, 0.3)).alpha(0.8);
        self.path(&format!("M{},{} q1.6,2.8 -0.4,3.2", n(hx + g + hr * 0.1), n(hy + hr * 0.2)), &nose);
        let my = hy + hr * 0.52;
        let mx = hx + g * 0.9;
        match p.expr {
            Expr::Happy => {
                let m = self.flat([150, 40, 50]);
                self.path(&format!("M{},{} Q{},{} {},{} Q{},{} {},{} Z", n(mx - 4.4), n(my), n(mx), n(my + 1.0), n(mx + 4.4), n(my), n(mx), n(my + 7.4), n(mx - 4.4), n(my)), &m);
                let t = self.flat([240, 130, 140]);
                self.ell(mx, my + 4.6, 2.2, 1.3, &t);
                let tt = self.flat([255, 255, 255]);
                self.rect(mx - 3.2, my + 0.4, 6.4, 1.5, 0.6, &tt);
            }
            Expr::Surprised => {
                let m = self.flat([130, 36, 48]);
                self.ell(mx, my + 2.2, 2.6, 3.5, &m);
            }
            Expr::Scared => self.curve(&format!("M{},{} Q{},{} {},{} T{},{}", n(mx - 4.2), n(my + 2.4), n(mx - 2.1), n(my - 0.4), n(mx), n(my + 2.2), n(mx + 4.2), n(my + 2.2)), [130, 36, 48], 1.4, 1.0),
            Expr::Sleepy => {
                let m = self.flat([160, 70, 80]);
                self.ell(mx, my + 2.0, 1.8, 2.2, &m);
            }
            Expr::Neutral => self.curve(&format!("M{},{} Q{},{} {},{}", n(mx - 3.4), n(my + 1.8), n(mx), n(my + 3.2), n(mx + 3.4), n(my + 1.8)), [130, 50, 56], 1.4, 1.0),
        }
        // cheeks
        let ch = self.flat([240, 110, 120]).alpha(0.4);
        self.ell(hx - hr * 0.58 + g, hy + hr * 0.5, 3.2, 2.1, &ch);
        self.ell(hx + hr * 0.62 + g, hy + hr * 0.5, 3.2, 2.1, &ch);
        if p.glasses {
            for ex in [hx - hr * 0.36 + g, hx + hr * 0.36 + g] {
                self.raw(&format!("<circle cx=\"{}\" cy=\"{}\" r=\"4.8\" fill=\"#ffffff\" fill-opacity=\"0.25\" stroke=\"#4a3a30\" stroke-width=\"1\"/>", n(ex), n(ey)));
            }
            self.line(hx - hr * 0.36 + g + 4.8, ey, hx + hr * 0.36 + g - 4.8, ey, [74, 58, 48], 1.0);
        }
    }
}

const INKC: Col = [50, 36, 44];

fn shoe(s: &mut Art, x: f32, y: f32, c: Col, rot: f32) {
    s.open_raw(&format!("transform=\"translate({} {}) rotate({})\"", n(x), n(y), n(rot)));
    let p = s.pn(c);
    s.path("M-5.5,-2.5 Q-5.5,-5 -1,-5 L2,-5 Q8,-4 10.5,0.5 Q11,3.5 8,3.5 L-5,3.5 Q-6,3.5 -5.5,-2.5 Z", &p);
    let sole = s.pn([245, 240, 230]);
    s.rect(-5.8, 2.2, 16.6, 2.2, 1.0, &sole);
    s.close();
}
