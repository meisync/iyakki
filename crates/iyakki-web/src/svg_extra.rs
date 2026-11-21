//! The parts of the SVG renderer that draw pictures and light (F-BROX-05, KI-714): `image` and `feImage` from `data:` addresses (PNG,
//! JPEG, GIF and WebP; an SVG inside an SVG and a file or web address are left out, the renderer has no network and no files),
//! `feTile`, the sub-region of a filter primitive, and the lighting filters `feDiffuseLighting` and `feSpecularLighting` with
//! `feDistantLight`, `fePointLight` and `feSpotLight`. A child module of `svg` so it can use the renderer's private types.

use super::*;

/// A decoded picture: premultiplied RGBA as floats from 0 to 1.
pub(super) struct Picture {
    w: usize,
    h: usize,
    px: Vec<f32>,
}

/// Pictures larger than this are not drawn (the decoders cap their own sizes too).
const MAX_PICTURE_PIXELS: usize = 16_000_000;
/// Pictures decoded while drawing one SVG.
pub(super) const MAX_PICTURES: usize = 32;

/// The picture an `href` names, when it is a `data:` address of a raster picture.
pub(super) fn load_picture(href: &str) -> Option<Picture> {
    let href = href.trim();
    if !href.starts_with("data:") || href.len() > MAX_INPUT * 2 {
        return None;
    }
    let (_, bytes) = crate::image::parse_data_url(href)?;
    let bmp = match crate::image::sniff(&bytes) {
        crate::image::Kind::Png => crate::png::decode(&bytes).ok()?,
        crate::image::Kind::Svg | crate::image::Kind::Unknown => return None,
        _ => crate::image::decode(&bytes).ok()?,
    };
    let (w, h) = (bmp.width as usize, bmp.height as usize);
    if w == 0 || h == 0 || w * h > MAX_PICTURE_PIXELS || bmp.rgba.len() < w * h * 4 {
        return None;
    }
    let mut px = Vec::with_capacity(w * h * 4);
    for p in bmp.rgba.chunks_exact(4).take(w * h) {
        let a = p[3] as f32 / 255.0;
        px.extend_from_slice(&[p[0] as f32 / 255.0 * a, p[1] as f32 / 255.0 * a, p[2] as f32 / 255.0 * a, a]);
    }
    Some(Picture { w, h, px })
}

impl Picture {
    fn texel(&self, x: i64, y: i64) -> [f32; 4] {
        let (x, y) = (x.clamp(0, self.w as i64 - 1) as usize, y.clamp(0, self.h as i64 - 1) as usize);
        let o = (y * self.w + x) * 4;
        [self.px[o], self.px[o + 1], self.px[o + 2], self.px[o + 3]]
    }

    /// Bilinear sample at picture coordinates (texel centres at .5), averaged over a few taps when the picture is drawn much smaller.
    fn sample(&self, u: f64, v: f64, taps: usize, su: f64, sv: f64) -> [f32; 4] {
        let mut acc = [0f32; 4];
        for j in 0..taps {
            for i in 0..taps {
                let (du, dv) = if taps == 1 { (0.0, 0.0) } else { (((i as f64 + 0.5) / taps as f64 - 0.5) * su, ((j as f64 + 0.5) / taps as f64 - 0.5) * sv) };
                let (fu, fv) = (u + du - 0.5, v + dv - 0.5);
                let (x0, y0) = (fu.floor(), fv.floor());
                let (tx, ty) = ((fu - x0) as f32, (fv - y0) as f32);
                let (a, b, c, d) = (self.texel(x0 as i64, y0 as i64), self.texel(x0 as i64 + 1, y0 as i64), self.texel(x0 as i64, y0 as i64 + 1), self.texel(x0 as i64 + 1, y0 as i64 + 1));
                for ch in 0..4 {
                    let top = a[ch] + (b[ch] - a[ch]) * tx;
                    let bot = c[ch] + (d[ch] - c[ch]) * tx;
                    acc[ch] += top + (bot - top) * ty;
                }
            }
        }
        let n = (taps * taps) as f32;
        [acc[0] / n, acc[1] / n, acc[2] / n, acc[3] / n]
    }
}

/// Draw `pic` (picture pixels to device pixels by `to_dev`) over `cv`, inside the polygon `clip` (device pixels), with `opacity`.
fn paint_picture(cv: &mut Canvas, pic: &Picture, to_dev: &M, clip: Vec<P>, opacity: f32, work: &mut u64) {
    let Some(inv) = to_dev.invert() else { return };
    let Some(mask) = fill_mask(&[orient(clip)], false, cv.w, cv.h, work) else { return };
    // how many picture pixels one device pixel covers along each axis
    let (su, sv) = ((inv.0[0] * inv.0[0] + inv.0[1] * inv.0[1]).sqrt(), (inv.0[2] * inv.0[2] + inv.0[3] * inv.0[3]).sqrt());
    let taps = (su.max(sv).ceil() as usize).clamp(1, 6);
    for my in 0..mask.h {
        for mx in 0..mask.w {
            let cov = mask.data[my * mask.w + mx];
            if cov == 0 {
                continue;
            }
            let (x, y) = (mask.x0 + mx, mask.y0 + my);
            let (u, v) = inv.apply((x as f64 + 0.5, y as f64 + 0.5));
            let s = pic.sample(u, v, taps, su, sv);
            let k = cov as f32 / 255.0 * opacity;
            let a = s[3] * k;
            if a <= 0.0 {
                continue;
            }
            let o = (y * cv.w + x) * 4;
            let d = &mut cv.px[o..o + 4];
            let inv_a = 1.0 - a;
            for c in 0..3 {
                d[c] = (s[c] * k * 255.0 + d[c] as f32 * inv_a + 0.5).min(255.0) as u8;
            }
            d[3] = (a * 255.0 + d[3] as f32 * inv_a + 0.5).min(255.0) as u8;
        }
    }
}

fn rect_poly(m: &M, x: f64, y: f64, w: f64, h: f64) -> Vec<P> {
    vec![m.apply((x, y)), m.apply((x + w, y)), m.apply((x + w, y + h)), m.apply((x, y + h))]
}

/// Put a picture into the box (x, y, w, h) of the space `m` with `preserveAspectRatio`, as `image` and `feImage` do.
fn draw_into_box(cv: &mut Canvas, pic: &Picture, m: &M, bx: [f64; 4], aspect: &str, opacity: f32, work: &mut u64) {
    let [x, y, w, h] = bx;
    if !(w > 0.0 && h > 0.0 && bx.iter().all(|v| v.is_finite())) {
        return;
    }
    let fit = fit_view_box(&[0.0, 0.0, pic.w as f64, pic.h as f64], w, h, aspect);
    let to_dev = m.mul(&M([1.0, 0.0, 0.0, 1.0, x, y])).mul(&fit);
    let (_, _, slice) = parse_aspect(aspect);
    // a picture that is cut (`slice`) shows only inside the box; otherwise its own rectangle is inside the box anyway
    let poly = if slice { rect_poly(m, x, y, w, h) } else { rect_poly(&to_dev, 0.0, 0.0, pic.w as f64, pic.h as f64) };
    paint_picture(cv, pic, &to_dev, poly, opacity, work);
}

impl<'a> Renderer<'a> {
    /// The `image` element: a picture from a `data:` address in the box given by `x`, `y`, `width` and `height`.
    pub(super) fn draw_image(&mut self, cv: &mut Canvas, node: &Node, st: &Style, m: &M) {
        if !st.visible || self.pictures >= MAX_PICTURES {
            return;
        }
        let Some(href) = node.attr("href") else { return };
        self.pictures += 1;
        let Some(pic) = load_picture(href) else { return };
        let len = |name: &str, refv: f64| node.attr(name).and_then(|v| length(v, refv)).filter(|v| *v > 0.0);
        let (x, y) = (node.attr("x").and_then(|v| length(v, self.vw)).unwrap_or(0.0), node.attr("y").and_then(|v| length(v, self.vh)).unwrap_or(0.0));
        let (pw, ph) = (pic.w as f64, pic.h as f64);
        let (w, h) = match (len("width", self.vw), len("height", self.vh)) {
            (Some(w), Some(h)) => (w, h),
            (Some(w), None) => (w, w * ph / pw),
            (None, Some(h)) => (h * pw / ph, h),
            (None, None) => (pw, ph),
        };
        self.work += (pic.w * pic.h) as u64 + (cv.w * cv.h) as u64;
        draw_into_box(cv, &pic, m, [x, y, w, h], node.attr("preserveAspectRatio").unwrap_or(""), 1.0, &mut self.work);
    }

    /// `feImage`: a `data:` picture put into the primitive's box (the given `x`, `y`, `width`, `height` in user space, else the filter
    /// region), or an element of the file drawn like `use` does.
    pub(super) fn fe_image(&mut self, node: &Node, w: usize, h: usize, region: (usize, usize, usize, usize), m: &M) -> Canvas {
        let mut out = Canvas::new(w, h);
        let Some(href) = node.attr("href") else { return out };
        if let Some(id) = href.trim().strip_prefix('#') {
            if let Some(&t) = self.ids.get(id) {
                self.render_node(&mut out, t, &Style::root(), m, 1);
            }
            return out;
        }
        if self.pictures >= MAX_PICTURES {
            return out;
        }
        self.pictures += 1;
        let Some(pic) = load_picture(href) else { return out };
        let (rx, ry, rw, rh) = (region.0 as f64, region.1 as f64, (region.2 - region.0) as f64, (region.3 - region.1) as f64);
        let a = m.0;
        let axis_aligned = a[1].abs() < 1e-9 && a[2].abs() < 1e-9;
        let given = |name: &str| node.attr(name).and_then(|v| length(v, if name == "x" || name == "width" { self.vw } else { self.vh }));
        let (mut bx, mut by, mut bw, mut bh) = (rx, ry, rw, rh);
        if axis_aligned {
            if let Some(x) = given("x") {
                bx = a[0] * x + a[4];
            }
            if let Some(y) = given("y") {
                by = a[3] * y + a[5];
            }
            if let Some(v) = given("width") {
                bw = (a[0] * v).abs();
            }
            if let Some(v) = given("height") {
                bh = (a[3] * v).abs();
            }
        }
        self.work += (pic.w * pic.h) as u64 + (w * h) as u64;
        draw_into_box(&mut out, &pic, &M::ID, [bx, by, bw, bh], node.attr("preserveAspectRatio").unwrap_or(""), 1.0, &mut self.work);
        out
    }
}

/// A pixel rectangle: left, top, right, bottom.
pub(super) type Rect = (usize, usize, usize, usize);

pub(super) fn union(a: Rect, b: Rect) -> Rect {
    if a.0 >= a.2 || a.1 >= a.3 {
        return b;
    }
    if b.0 >= b.2 || b.1 >= b.3 {
        return a;
    }
    (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
}

pub(super) fn intersect(a: Rect, b: Rect) -> Rect {
    let (l, t, r, bo) = (a.0.max(b.0), a.1.max(b.1), a.2.min(b.2), a.3.min(b.3));
    (l, t, r.max(l), bo.max(t))
}

/// The sub-region of a primitive: `x`, `y`, `width` and `height` (user space) replace what the default (`def`: the union of what its inputs
/// covered, or the filter region) says. A transform with rotation or skew, or `primitiveUnits="objectBoundingBox"`, leaves the default.
pub(super) fn subregion(node: &Node, def: Rect, m: &M, w: usize, h: usize, bbox_units: bool, bbox: Option<Rect>) -> Rect {
    let a = m.0;
    if a[1].abs() > 1e-9 || a[2].abs() > 1e-9 {
        return def;
    }
    if bbox_units {
        // fractions (or percentages) of the element's box, which is known in picture pixels
        let Some((bx0, by0, bx1, by1)) = bbox else { return def };
        let frac = |name: &str| node.attr(name).map(str::trim).and_then(|v| take_number(v).map(|(n, rest)| if rest.trim() == "%" { n / 100.0 } else { n })).filter(|v| v.is_finite());
        let (fx, fy, fw, fh) = (frac("x"), frac("y"), frac("width"), frac("height"));
        if fx.is_none() && fy.is_none() && fw.is_none() && fh.is_none() {
            return def;
        }
        let (bw, bh) = ((bx1 - bx0) as f64, (by1 - by0) as f64);
        let (mut l, mut t, mut r, mut b) = (def.0 as f64, def.1 as f64, def.2 as f64, def.3 as f64);
        if let Some(x) = fx {
            l = bx0 as f64 + x * bw;
        }
        if let Some(y) = fy {
            t = by0 as f64 + y * bh;
        }
        if let Some(v) = fw {
            r = l + (v * bw).abs();
        }
        if let Some(v) = fh {
            b = t + (v * bh).abs();
        }
        return region_px(l, t, r, b, w, h);
    }
    let get = |name: &str| node.attr(name).and_then(|v| take_number(v.trim()).map(|(n, _)| n)).filter(|v| v.is_finite());
    let (gx, gy, gw, gh) = (get("x"), get("y"), get("width"), get("height"));
    if gx.is_none() && gy.is_none() && gw.is_none() && gh.is_none() {
        return def;
    }
    let (mut l, mut t, mut r, mut b) = (def.0 as f64, def.1 as f64, def.2 as f64, def.3 as f64);
    if let Some(x) = gx {
        l = a[0] * x + a[4];
    }
    if let Some(y) = gy {
        t = a[3] * y + a[5];
    }
    if let Some(v) = gw {
        r = l + (a[0] * v).abs();
    }
    if let Some(v) = gh {
        b = t + (a[3] * v).abs();
    }
    region_px(l, t, r, b, w, h)
}

/// `feTile`: the input's sub-region repeated over the whole picture, starting from its top-left corner.
pub(super) fn tile(a: &Canvas, r: Rect) -> Canvas {
    let mut o = Canvas::new(a.w, a.h);
    let (tw, th) = (r.2.saturating_sub(r.0), r.3.saturating_sub(r.1));
    if tw == 0 || th == 0 || r.2 > a.w || r.3 > a.h {
        return o;
    }
    for y in 0..a.h {
        let sy = r.1 + (y as i64 - r.1 as i64).rem_euclid(th as i64) as usize;
        for x in 0..a.w {
            let sx = r.0 + (x as i64 - r.0 as i64).rem_euclid(tw as i64) as usize;
            let (s, d) = ((sy * a.w + sx) * 4, (y * a.w + x) * 4);
            o.px[d..d + 4].copy_from_slice(&a.px[s..s + 4]);
        }
    }
    o
}

enum Light {
    Distant { az: f64, el: f64 },
    Point { x: f64, y: f64, z: f64 },
    Spot { x: f64, y: f64, z: f64, tx: f64, ty: f64, tz: f64, exp: f64, cone: Option<f64> },
}

fn norm3(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l < 1e-12 { [0.0, 0.0, 1.0] } else { [v[0] / l, v[1] / l, v[2] / l] }
}

/// The surface normal at a pixel from the alpha channel, with the edge kernels of the specification (the available neighbours only).
fn surface_normal(alpha: &[f64], w: usize, h: usize, x: usize, y: usize, scale: f64) -> (f64, f64) {
    let at = |xx: usize, yy: usize| alpha[yy * w + xx];
    let (xl, xr) = (x.saturating_sub(1), (x + 1).min(w - 1));
    let (yt, yb) = (y.saturating_sub(1), (y + 1).min(h - 1));
    // along one axis: the difference between the far and near column, summed over the rows that exist with weights 1 2 1
    let mut nx = 0.0;
    let mut wsum = 0.0;
    for yy in yt..=yb {
        let wt = if yy == y { 2.0 } else { 1.0 };
        nx += wt * (at(xr, yy) - at(xl, yy));
        wsum += wt;
    }
    let span_x = (xr - xl).max(1) as f64;
    let nx = -scale * nx * 2.0 / (wsum * span_x);
    let mut ny = 0.0;
    let mut wsum = 0.0;
    for xx in xl..=xr {
        let wt = if xx == x { 2.0 } else { 1.0 };
        ny += wt * (at(xx, yb) - at(xx, yt));
        wsum += wt;
    }
    let span_y = (yb - yt).max(1) as f64;
    (nx, -scale * ny * 2.0 / (wsum * span_y))
}

/// `feDiffuseLighting` and `feSpecularLighting`: the alpha of the input is a height map lit by the light source child. `m` takes the light's
/// user-space position to pixels and `k` scales its height.
pub(super) fn lighting(a: &Canvas, node: &Node, kids: &[&Node], m: &M, k: f64, color: Rgba, specular: bool) -> Canvas {
    let num = |n: &Node, name: &str, def: f64| n.attr(name).and_then(|v| numbers(v).first().copied()).filter(|v| v.is_finite()).unwrap_or(def);
    let Some(ln) = kids.iter().find(|n| matches!(n.tag.as_str(), "feDistantLight" | "fePointLight" | "feSpotLight")) else {
        return Canvas::new(a.w, a.h);
    };
    let light = match ln.tag.as_str() {
        "feDistantLight" => Light::Distant { az: num(ln, "azimuth", 0.0).to_radians(), el: num(ln, "elevation", 0.0).to_radians() },
        "fePointLight" => {
            let p = m.apply((num(ln, "x", 0.0), num(ln, "y", 0.0)));
            Light::Point { x: p.0, y: p.1, z: num(ln, "z", 0.0) * k }
        }
        _ => {
            let p = m.apply((num(ln, "x", 0.0), num(ln, "y", 0.0)));
            let t = m.apply((num(ln, "pointsAtX", 0.0), num(ln, "pointsAtY", 0.0)));
            let exp = num(ln, "specularExponent", 1.0);
            let cone = ln.attr("limitingConeAngle").and_then(|v| numbers(v).first().copied()).filter(|v| v.is_finite()).map(|v| v.abs().to_radians().cos());
            Light::Spot { x: p.0, y: p.1, z: num(ln, "z", 0.0) * k, tx: t.0, ty: t.1, tz: num(ln, "pointsAtZ", 0.0) * k, exp: if exp > 0.0 { exp } else { 1.0 }, cone }
        }
    };
    let scale = num(node, "surfaceScale", 1.0);
    let (kc, exp) = if specular { (num(node, "specularConstant", 1.0), num(node, "specularExponent", 1.0).clamp(1.0, 128.0)) } else { (num(node, "diffuseConstant", 1.0), 1.0) };
    if kc < 0.0 || (specular && !(1.0..=128.0).contains(&num(node, "specularExponent", 1.0))) {
        // a negative constant, or a specular exponent outside 1 to 128, is an error: the primitive shows nothing
        return Canvas::new(a.w, a.h);
    }
    let alpha: Vec<f64> = a.px.chunks_exact(4).map(|p| p[3] as f64 / 255.0).collect();
    let mut o = Canvas::new(a.w, a.h);
    // the colour of a spot light at a point is the light colour times the spot's falloff
    let lc = [color[0] as f64, color[1] as f64, color[2] as f64];
    for y in 0..a.h {
        for x in 0..a.w {
            let (nx, ny) = surface_normal(&alpha, a.w, a.h, x, y, scale);
            let n = norm3([nx, ny, 1.0]);
            let z = scale * alpha[y * a.w + x];
            let (l, lcol) = match &light {
                Light::Distant { az, el } => ([az.cos() * el.cos(), az.sin() * el.cos(), el.sin()], lc),
                Light::Point { x: lx, y: ly, z: lz } => (norm3([lx - x as f64, ly - y as f64, lz - z]), lc),
                Light::Spot { x: lx, y: ly, z: lz, tx, ty, tz, exp: se, cone } => {
                    let l = norm3([lx - x as f64, ly - y as f64, lz - z]);
                    let s = norm3([tx - lx, ty - ly, tz - lz]);
                    let minus_l_dot_s = -(l[0] * s[0] + l[1] * s[1] + l[2] * s[2]);
                    let lit = minus_l_dot_s > 0.0 && cone.is_none_or(|c| minus_l_dot_s >= c);
                    let f = if lit { minus_l_dot_s.powf(*se) } else { 0.0 };
                    (l, [lc[0] * f, lc[1] * f, lc[2] * f])
                }
            };
            let d = (y * a.w + x) * 4;
            if specular {
                let h = norm3([l[0], l[1], l[2] + 1.0]);
                let nh = (n[0] * h[0] + n[1] * h[1] + n[2] * h[2]).max(0.0);
                let f = kc * nh.powf(exp);
                let v = [(f * lcol[0]).clamp(0.0, 1.0), (f * lcol[1]).clamp(0.0, 1.0), (f * lcol[2]).clamp(0.0, 1.0)];
                let al = v[0].max(v[1]).max(v[2]);
                put4(&mut o.px[d..d + 4], [v[0] as f32, v[1] as f32, v[2] as f32, al as f32]);
            } else {
                let nl = (n[0] * l[0] + n[1] * l[1] + n[2] * l[2]).max(0.0);
                let f = kc * nl;
                put4(&mut o.px[d..d + 4], [(f * lcol[0]).clamp(0.0, 1.0) as f32, (f * lcol[1]).clamp(0.0, 1.0) as f32, (f * lcol[2]).clamp(0.0, 1.0) as f32, 1.0]);
            }
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 16 x 12 PNG: red and blue squares of 4 pixels, green with a fading alpha (the same picture as the Chrome comparison).
    const PNG16: &str = "iVBORw0KGgoAAAANSUhEUgAAABAAAAAMCAYAAABr5z2BAAAAT0lEQVR4nGN4pqHxHxlr2NxBwYTkGYaBARonAs4jY1INBBlwGBmTY8B2ZEyOAauRMckGYGg4ETAfGZMeCycCJiNjcgxoR8bkGFCNjAkZAADXzKBQzEQKdAAAAABJRU5ErkJggg==";

    fn draw(inner: &str) -> Bitmap {
        render(format!(r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="48" viewBox="0 0 64 48">{inner}</svg>"##).as_bytes(), None).unwrap()
    }

    fn px(b: &Bitmap, x: u32, y: u32) -> [u8; 4] {
        let o = ((y * b.width + x) * 4) as usize;
        [b.rgba[o], b.rgba[o + 1], b.rgba[o + 2], b.rgba[o + 3]]
    }

    #[test]
    fn an_image_from_a_data_address_is_drawn_in_its_box_with_its_colours() {
        let b = draw(&format!(r##"<image href="data:image/png;base64,{PNG16}" x="0" y="0" width="64" height="48"/>"##));
        // the first square is red, the next blue (4 picture pixels are 16 device pixels here)
        let red = px(&b, 6, 6);
        assert!(red[0] > 200 && red[1] < 80 && red[2] < 80 && red[3] == 255, "{red:?}");
        let blue = px(&b, 22, 6);
        assert!(blue[2] > 180 && blue[0] < 90, "{blue:?}");
        // the fading green square is see-through in places
        assert!(px(&b, 6, 22)[3] < 255 || px(&b, 6, 38)[3] < 255 || px(&b, 6, 22)[1] > 150);
        // moved by x and y and kept inside a smaller box with its aspect ratio (4:3 into 32 x 32: 32 x 24, centred)
        let c = draw(&format!(r##"<image xlink:href="data:image/png;base64,{PNG16}" x="16" y="8" width="32" height="32"/>"##));
        assert_eq!(px(&c, 2, 2)[3], 0, "nothing outside the box");
        assert!(px(&c, 20, 16)[3] > 0 && px(&c, 20, 10)[3] == 0, "centred vertically: the picture is 24 high inside 32");
    }

    #[test]
    fn what_cannot_be_read_draws_nothing_and_never_panics() {
        for href in ["http://example.org/a.png", "file:///etc/passwd", "data:image/png;base64,AAAA", "data:image/png;base64,", "data:text/plain,hello", "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='5' height='5'><rect width='5' height='5'/></svg>", "", "data:"] {
            let b = draw(&format!(r##"<image href="{href}" x="0" y="0" width="64" height="48"/>"##));
            assert!(b.rgba.chunks(4).all(|p| p[3] == 0), "{href}");
        }
        // a box with no size, a negative size, NaN-like text
        for attrs in ["", r#"width="0" height="10""#, r#"width="-5" height="10""#, r#"width="x" height="y""#] {
            let _ = draw(&format!(r##"<image href="data:image/png;base64,{PNG16}" {attrs}/>"##));
        }
        // many images: the count is capped, the drawing still ends
        let many: String = (0..80).map(|i| format!(r##"<image href="data:image/png;base64,{PNG16}" x="{}" y="0" width="8" height="6"/>"##, i % 50)).collect();
        let b = draw(&many);
        assert!(b.rgba.chunks(4).any(|p| p[3] > 0));
    }

    #[test]
    fn feimage_fills_the_filter_region_and_an_element_reference_draws_the_element() {
        let b = draw(&format!(r##"<defs><filter id="f" x="0" y="0" width="100%" height="100%"><feImage href="data:image/png;base64,{PNG16}" preserveAspectRatio="none"/></filter></defs><rect x="8" y="8" width="48" height="32" fill="#000" filter="url(#f)"/>"##));
        let p = px(&b, 14, 14);
        assert!(p[0] > 180 && p[1] < 90, "red picture, not the black rectangle: {p:?}");
        assert_eq!(px(&b, 2, 2)[3], 0);
        let e = draw(r##"<defs><circle id="c" cx="32" cy="24" r="10" fill="#08f"/><filter id="f" x="0" y="0" width="100%" height="100%"><feImage href="#c"/></filter></defs><rect x="8" y="8" width="48" height="32" fill="#000" filter="url(#f)"/>"##);
        let p = px(&e, 32, 24);
        assert!(p[2] > 200 && p[0] < 60, "{p:?}");
    }

    #[test]
    fn tile_repeats_a_rectangle_from_its_corner() {
        let mut a = Canvas::new(10, 6);
        for (i, p) in a.px.chunks_exact_mut(4).enumerate() {
            let (x, y) = (i % 10, i / 10);
            if (2..4).contains(&x) && (1..3).contains(&y) {
                p.copy_from_slice(&[255, 0, 0, 255]);
            }
        }
        let t = tile(&a, (2, 1, 4, 3));
        let at = |x: usize, y: usize| t.px[(y * 10 + x) * 4];
        // the tile is 2 x 2 with its red pixels in the first column of the tile's own first row and everything repeats
        for y in 0..6 {
            for x in 0..10 {
                assert_eq!(at(x, y), 255, "({x}, {y})");
            }
        }
        assert!(tile(&a, (3, 3, 3, 3)).px.iter().all(|v| *v == 0), "an empty tile gives nothing");
        assert!(tile(&a, (0, 0, 11, 6)).px.iter().all(|v| *v == 0), "a tile outside the picture gives nothing");
    }

    #[test]
    fn subregions_follow_the_given_numbers_and_fall_back_to_the_default() {
        let n = |attrs: &[(&str, &str)]| Node { tag: "feFlood".into(), attrs: attrs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), kids: vec![], text: String::new() };
        let def = (0, 0, 100, 100);
        let m = M([2.0, 0.0, 0.0, 2.0, 10.0, 0.0]);
        assert_eq!(subregion(&n(&[]), def, &m, 200, 200, false, None), def);
        assert_eq!(subregion(&n(&[("x", "5"), ("y", "5"), ("width", "10"), ("height", "20")]), def, &m, 200, 200, false, None), (20, 10, 40, 50));
        assert_eq!(subregion(&n(&[("width", "10")]), def, &m, 200, 200, false, None), (0, 0, 20, 100), "only the width given: the left stays");
        assert_eq!(subregion(&n(&[("x", "5")]), def, &m, 200, 200, true, None), def, "bounding-box units without a box leave the default");
        assert_eq!(subregion(&n(&[("x", "0.5"), ("width", "0.25")]), def, &m, 200, 200, true, Some((10, 20, 50, 60))), (30, 0, 40, 100), "bounding-box units are fractions of the box");
        assert_eq!(subregion(&n(&[("x", "5")]), def, &M([0.7, 0.7, -0.7, 0.7, 0.0, 0.0]), 200, 200, false, None), def, "a rotated space leaves the default");
        assert_eq!(union((0, 0, 0, 0), (1, 2, 3, 4)), (1, 2, 3, 4));
        assert_eq!(intersect((0, 0, 10, 10), (5, 5, 20, 20)), (5, 5, 10, 10));
        assert_eq!(intersect((0, 0, 4, 4), (6, 6, 9, 9)).2 - intersect((0, 0, 4, 4), (6, 6, 9, 9)).0, 0);
    }

    #[test]
    fn a_distant_light_straight_above_a_flat_surface_gives_the_constant_times_the_colour() {
        let a = Canvas::new(8, 8);
        let nodes = |light: Node| (Node { tag: "feDiffuseLighting".into(), attrs: vec![("diffuseConstant".into(), "0.5".into())], kids: vec![], text: String::new() }, light);
        let light = Node { tag: "feDistantLight".into(), attrs: vec![("azimuth".into(), "0".into()), ("elevation".into(), "90".into())], kids: vec![], text: String::new() };
        let (n, l) = nodes(light);
        let o = lighting(&a, &n, &[&l], &M::ID, 1.0, [1.0, 0.5, 0.0, 1.0], false);
        assert_eq!(&o.px[..4], &[128, 64, 0, 255]);
        // a negative constant, and no light source, show nothing
        let bad = Node { tag: "feDiffuseLighting".into(), attrs: vec![("diffuseConstant".into(), "-1".into())], kids: vec![], text: String::new() };
        assert!(lighting(&a, &bad, &[&l], &M::ID, 1.0, [1.0; 4], false).px.iter().all(|v| *v == 0));
        assert!(lighting(&a, &n, &[], &M::ID, 1.0, [1.0; 4], false).px.iter().all(|v| *v == 0));
        // specular: exponent out of range shows nothing; in range, straight above gives ks * colour with alpha the largest channel
        let sp = Node { tag: "feSpecularLighting".into(), attrs: vec![("specularExponent".into(), "0.5".into())], kids: vec![], text: String::new() };
        assert!(lighting(&a, &sp, &[&l], &M::ID, 1.0, [1.0; 4], true).px.iter().all(|v| *v == 0));
        let sp = Node { tag: "feSpecularLighting".into(), attrs: vec![("specularExponent".into(), "1".into()), ("specularConstant".into(), "1".into())], kids: vec![], text: String::new() };
        let o = lighting(&a, &sp, &[&l], &M::ID, 1.0, [1.0, 0.5, 0.0, 1.0], true);
        assert_eq!(o.px[3], 255);
        assert!(o.px[0] > 250 && (120..136).contains(&o.px[1]));
    }

    #[test]
    fn the_surface_normal_leans_against_a_ramp_at_the_edges_too() {
        // alpha rising to the right: the normal leans left (negative x) by the same amount everywhere, edges included, and none is NaN
        let (w, h) = (5usize, 4usize);
        let alpha: Vec<f64> = (0..w * h).map(|i| (i % w) as f64 / 4.0).collect();
        for y in 0..h {
            for x in 0..w {
                let (nx, ny) = surface_normal(&alpha, w, h, x, y, 2.0);
                assert!(nx.is_finite() && ny.is_finite());
                assert!((nx + 1.0).abs() < 1e-9, "({x}, {y}): {nx}");
                assert!(ny.abs() < 1e-9);
            }
        }
        // a 1 x 1 picture has no neighbours and no slope
        assert_eq!(surface_normal(&[1.0], 1, 1, 0, 0, 1.0), (0.0, 0.0));
    }

    #[test]
    fn lit_filters_and_hostile_attributes_never_panic() {
        for light in ["", r#"<feDistantLight azimuth="x" elevation="NaN"/>"#, r#"<fePointLight x="1e30" y="-1e30" z="inf"/>"#, r#"<feSpotLight limitingConeAngle="0" specularExponent="-3" pointsAtX="1e300"/>"#] {
            let _ = draw(&format!(r##"<defs><filter id="f"><feDiffuseLighting surfaceScale="1e9" diffuseConstant="1e9">{light}</feDiffuseLighting></filter></defs><circle cx="30" cy="20" r="10" filter="url(#f)"/>"##));
            let _ = draw(&format!(r##"<defs><filter id="f"><feSpecularLighting specularExponent="128" specularConstant="1e9">{light}</feSpecularLighting><feTile/></filter></defs><circle cx="30" cy="20" r="10" filter="url(#f)"/>"##));
        }
    }
}
