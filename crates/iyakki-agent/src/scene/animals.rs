//! Animals of the scene composer: dog, cat, goat, bear, rabbit, elephant and fish. Each faces right with its feet at the
//! origin, round and friendly, with a highlight in the eye, a lighter belly and a shaded back.

use super::art::*;

impl Art {
    fn eye(&mut self, x: f32, y: f32, r: f32) {
        let w = self.flat([255, 255, 255]);
        self.circ(x, y, r * 1.15, &w);
        let d = self.flat([28, 20, 26]);
        self.circ(x + r * 0.2, y + r * 0.05, r * 0.8, &d);
        let h = self.flat([255, 255, 255]);
        self.circ(x + r * 0.5, y - r * 0.3, r * 0.3, &h);
    }

    fn belly(&mut self, x: f32, y: f32, rx: f32, ry: f32, c: Col) {
        let b = self.flat(lighten(c, 0.5)).alpha(0.75);
        self.ell(x, y, rx, ry, &b);
    }

    pub fn dog(&mut self, col: Option<Col>) {
        let c = col.unwrap_or([184, 128, 80]);
        self.shadow(2.0, 0.5, 30.0);
        self.limb(-14.0, -20.0, -15.0, -3.0, 6.0, darken(c, 0.12));
        self.limb(10.0, -20.0, 11.0, -3.0, 6.0, darken(c, 0.12));
        // tail
        self.curve("M-22,-30 Q-34,-36 -33,-50", darken(c, 0.05), 6.0, 1.0);
        let body = self.pn(c);
        self.ell(0.0, -27.0, 24.0, 14.0, &body);
        self.belly(2.0, -20.0, 17.0, 7.0, c);
        let sh = self.flat(darken(c, 0.3)).alpha(0.35);
        self.ell(-4.0, -35.0, 20.0, 7.0, &sh);
        self.limb(-7.0, -18.0, -8.0, -2.0, 6.5, c);
        self.limb(17.0, -18.0, 18.0, -2.0, 6.5, c);
        let sp = self.flat(darken(c, 0.35)).alpha(0.7);
        self.ell(-10.0, -30.0, 7.0, 5.0, &sp);
        // head
        let head = self.pn(c);
        self.circ(26.0, -42.0, 12.5, &head);
        let ear = self.pn(darken(c, 0.3));
        self.path("M17,-50 Q10,-46 12,-32 Q18,-30 21,-42 Z", &ear);
        let sn = self.pn(lighten(c, 0.35));
        self.ell(36.0, -38.0, 9.0, 6.5, &sn);
        let nose = self.pn([40, 30, 36]);
        self.ell(43.0, -41.0, 3.2, 2.4, &nose);
        self.eye(28.5, -46.0, 2.6);
        let br = self.flat(darken(c, 0.4));
        self.rect(24.5, -51.0, 8.0, 1.5, 0.7, &br);
        self.curve("M35,-34 Q38,-31 41,-34", [90, 40, 40], 1.2, 1.0);
        let tg = self.flat([240, 110, 130]);
        self.ell(38.5, -31.0, 2.2, 3.0, &tg);
        let col = self.pn([220, 70, 80]);
        self.rect(15.0, -35.0, 14.0, 3.6, 1.8, &col);
    }

    pub fn cat(&mut self, col: Option<Col>) {
        let c = col.unwrap_or([232, 150, 74]);
        self.shadow(0.0, 0.5, 24.0);
        // tail curling up
        self.curve("M-17,-14 Q-34,-12 -30,-30 Q-28,-40 -22,-38", darken(c, 0.05), 5.4, 1.0);
        self.limb(-10.0, -12.0, -11.0, -2.0, 5.4, darken(c, 0.1));
        self.limb(9.0, -12.0, 10.0, -2.0, 5.4, darken(c, 0.1));
        let body = self.pn(c);
        self.ell(0.0, -19.0, 19.0, 11.0, &body);
        self.belly(2.0, -14.0, 13.0, 5.0, c);
        for x in [-8.0, -1.0, 6.0] {
            self.curve(&format!("M{},-28 q2,5 0,9", n(x)), darken(c, 0.3), 1.8, 0.8);
        }
        self.limb(-4.0, -12.0, -5.0, -2.0, 5.6, c);
        self.limb(15.0, -12.0, 16.0, -2.0, 5.6, c);
        let head = self.pn(c);
        self.poly(&[(13.0, -41.0), (14.0, -53.0), (21.0, -44.0)], &head);
        self.poly(&[(25.0, -44.0), (31.0, -54.0), (32.0, -40.0)], &head);
        let inner = self.flat([244, 160, 170]);
        self.poly(&[(15.0, -43.0), (15.5, -49.0), (19.0, -44.0)], &inner);
        self.circ(22.0, -34.0, 11.5, &head);
        self.belly(26.0, -31.0, 6.0, 4.5, c);
        for x in [18.0, 25.0] {
            let e = self.flat([255, 255, 240]);
            self.ell(x, -36.0, 2.8, 3.2, &e);
            let g = self.flat([70, 150, 80]);
            self.ell(x + 0.4, -36.0, 1.9, 2.8, &g);
            let p = self.flat([20, 16, 20]);
            self.ell(x + 0.4, -36.0, 0.8, 2.6, &p);
        }
        let nose = self.flat([240, 120, 140]);
        self.poly(&[(22.0, -31.0), (25.0, -31.0), (23.5, -29.0)], &nose);
        for dy in [-1.5, 0.5] {
            self.line(26.0, -30.0 + dy, 34.0, -31.5 + dy * 2.0, [255, 255, 255], 0.7);
            self.line(18.0, -30.0 + dy, 11.0, -31.5 + dy * 2.0, [255, 255, 255], 0.7);
        }
    }

    pub fn goat(&mut self, col: Option<Col>) {
        let c = col.unwrap_or([244, 240, 230]);
        self.shadow(0.0, 0.5, 30.0);
        let hoof = [70, 56, 52];
        self.limb(-15.0, -24.0, -16.0, -4.0, 4.6, darken(c, 0.15));
        self.limb(9.0, -24.0, 10.0, -4.0, 4.6, darken(c, 0.15));
        self.curve("M-22,-38 Q-28,-40 -27,-46", c, 5.0, 1.0);
        let body = self.pn(c);
        self.ell(0.0, -36.0, 24.0, 14.0, &body);
        let sh = self.flat(darken(c, 0.3)).alpha(0.3);
        self.ell(-3.0, -43.0, 21.0, 6.0, &sh);
        self.limb(-8.0, -24.0, -9.0, -4.0, 4.8, c);
        self.limb(17.0, -24.0, 18.0, -4.0, 4.8, c);
        for x in [-16.0, -9.0, 10.0, 18.0] {
            let h = self.flat(hoof);
            self.rect(x - 2.6, -5.0, 5.2, 5.0, 1.2, &h);
        }
        // neck and head
        let neck = self.pn(c);
        self.path("M14,-42 Q22,-52 24,-62 L34,-60 Q32,-44 26,-30 Z", &neck);
        self.ell(30.0, -62.0, 9.0, 8.0, &neck);
        let sn = self.pn(lighten(c, 0.1));
        self.ell(38.0, -58.0, 7.0, 5.4, &sn);
        let nose = self.flat([220, 150, 150]);
        self.ell(43.0, -58.0, 2.0, 1.8, &nose);
        self.eye(31.0, -64.0, 2.2);
        // horns, ear, beard
        let hn = self.pn([200, 180, 140]);
        self.path("M26,-68 Q18,-82 10,-80 Q18,-76 22,-66 Z", &hn);
        let ear = self.pn(darken(c, 0.12));
        self.path("M24,-62 Q14,-64 12,-58 Q20,-58 25,-58 Z", &ear);
        let bd = self.pn(c);
        self.path("M38,-54 Q41,-46 37,-42 Q36,-48 34,-53 Z", &bd);
    }

    pub fn bear(&mut self, col: Option<Col>) {
        let c = col.unwrap_or([128, 86, 54]);
        self.shadow(0.0, 0.5, 34.0);
        self.limb(-16.0, -22.0, -17.0, -4.0, 10.0, darken(c, 0.15));
        self.limb(12.0, -22.0, 13.0, -4.0, 10.0, darken(c, 0.15));
        let body = self.pn(c);
        self.ell(0.0, -32.0, 28.0, 19.0, &body);
        let sh = self.flat(darken(c, 0.3)).alpha(0.3);
        self.ell(-4.0, -42.0, 24.0, 8.0, &sh);
        self.limb(-5.0, -22.0, -6.0, -4.0, 10.5, c);
        self.limb(23.0, -22.0, 24.0, -4.0, 10.5, c);
        self.belly(2.0, -26.0, 16.0, 8.0, c);
        let head = self.pn(c);
        self.circ(25.0, -52.0, 3.0, &head);
        self.circ(40.0, -52.0, 3.0, &head);
        self.circ(32.0, -42.0, 14.0, &head);
        let ear = self.flat(lighten(c, 0.35));
        self.circ(25.0, -52.0, 1.6, &ear);
        self.circ(40.0, -52.0, 1.6, &ear);
        let sn = self.pn(lighten(c, 0.45));
        self.ell(41.0, -38.0, 8.0, 6.0, &sn);
        let nose = self.flat([34, 26, 30]);
        self.ell(46.0, -41.0, 3.2, 2.4, &nose);
        self.eye(35.0, -46.0, 2.3);
        self.curve("M40,-34 Q43,-31 46,-34", [60, 30, 30], 1.2, 1.0);
    }

    pub fn rabbit(&mut self, col: Option<Col>) {
        let c = col.unwrap_or([226, 220, 210]);
        self.shadow(0.0, 0.5, 18.0);
        let tail = self.pn([255, 255, 255]);
        self.circ(-13.0, -14.0, 4.5, &tail);
        let body = self.pn(c);
        self.ell(0.0, -13.0, 14.0, 11.0, &body);
        self.belly(3.0, -9.0, 8.0, 5.0, c);
        let foot = self.pn(c);
        self.ell(-5.0, -1.8, 7.0, 2.8, &foot);
        self.ell(9.0, -1.8, 5.5, 2.6, &foot);
        // ears
        for (dx, a) in [(-1.0, -10.0), (6.0, 6.0)] {
            self.open_raw(&format!("transform=\"translate({} -30) rotate({})\"", n(12.0 + dx), n(a)));
            let e = self.pn(c);
            self.ell(0.0, -9.0, 3.8, 11.0, &e);
            let i = self.flat([244, 170, 180]);
            self.ell(0.0, -9.0, 1.9, 8.0, &i);
            self.close();
        }
        let head = self.pn(c);
        self.circ(14.0, -26.0, 9.0, &head);
        self.eye(17.0, -28.0, 1.9);
        let nose = self.flat([240, 120, 140]);
        self.ell(22.5, -25.0, 1.6, 1.2, &nose);
        let ch = self.flat([244, 130, 140]).alpha(0.5);
        self.circ(17.0, -23.0, 2.2, &ch);
        self.line(22.0, -24.0, 28.0, -25.0, [255, 255, 255], 0.7);
        self.line(22.0, -23.0, 28.0, -22.0, [255, 255, 255], 0.7);
    }

    pub fn elephant(&mut self, col: Option<Col>) {
        let c = col.unwrap_or([150, 160, 178]);
        self.shadow(2.0, 0.5, 44.0);
        self.limb(-22.0, -30.0, -22.0, -5.0, 14.0, darken(c, 0.15));
        self.limb(10.0, -30.0, 10.0, -5.0, 14.0, darken(c, 0.15));
        self.curve("M-34,-46 Q-42,-40 -40,-28", darken(c, 0.1), 3.0, 1.0);
        let body = self.pn(c);
        self.ell(0.0, -44.0, 36.0, 26.0, &body);
        let sh = self.flat(darken(c, 0.3)).alpha(0.3);
        self.ell(-4.0, -56.0, 32.0, 10.0, &sh);
        self.limb(-6.0, -30.0, -6.0, -5.0, 14.0, c);
        self.limb(26.0, -30.0, 26.0, -5.0, 14.0, c);
        for x in [-22.0, -6.0, 10.0, 26.0] {
            let t = self.flat([240, 236, 226]);
            self.ell(x, -3.0, 5.0, 2.4, &t);
        }
        // trunk
        self.limb(48.0, -48.0, 56.0, -28.0, 11.0, c);
        self.limb(56.0, -28.0, 54.0, -6.0, 8.5, c);
        self.limb(54.0, -6.0, 60.0, -3.0, 6.0, c);
        let head = self.pn(c);
        self.circ(40.0, -54.0, 20.0, &head);
        let ear = self.pn(darken(c, 0.12));
        self.ell(30.0, -52.0, 14.0, 20.0, &ear);
        let ei = self.flat([232, 160, 170]).alpha(0.6);
        self.ell(30.0, -50.0, 8.0, 14.0, &ei);
        self.eye(46.0, -58.0, 2.8);
        let tk = self.pn([250, 246, 236]);
        self.path("M50,-42 Q60,-40 64,-30 Q56,-34 50,-38 Z", &tk);
    }

    pub fn fish_body(&mut self, col: Option<Col>) {
        let c = col.unwrap_or([255, 150, 60]);
        let tail = self.pn(darken(c, 0.1));
        self.poly(&[(10.0, 0.0), (22.0, -9.0), (19.0, 0.0), (22.0, 9.0)], &tail);
        let fin = self.pn(darken(c, 0.15));
        self.path("M-2,-6 Q4,-16 10,-6 Z", &fin);
        let body = self.pn(c);
        self.ell(0.0, 0.0, 15.0, 9.0, &body);
        self.belly(0.0, 4.0, 11.0, 4.0, c);
        let st = self.flat(lighten(c, 0.4)).alpha(0.6);
        self.path("M3,-8 Q9,0 3,8 Q7,0 3,-8 Z", &st);
        self.eye(-8.0, -2.0, 1.8);
        self.curve("M-14,2 q2,1.5 4,0.5", [120, 50, 40], 1.0, 1.0);
    }
}
