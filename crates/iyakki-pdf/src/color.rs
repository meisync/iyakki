//! Colour spaces. Device spaces, CIE-based spaces and ICC profiles are treated as their device equivalents; Separation and DeviceN use
//! a tint function of type 2 (or grey when it cannot be evaluated); Pattern and shading colours are not painted.

use crate::file::File;
use crate::obj::*;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub enum Cs {
    Gray,
    Rgb,
    Cmyk,
    Lab,
    Indexed { base: Box<Cs>, hival: usize, table: Rc<Vec<u8>> },
    /// A named tint: components -> alternate space.
    Tint { n: usize, alt: Box<Cs>, c0: Vec<f64>, c1: Vec<f64>, exp: f64, usable: bool },
    Pattern,
}

pub type Rgb = [f32; 3];

impl Cs {
    pub fn n(&self) -> usize {
        match self {
            Cs::Gray | Cs::Indexed { .. } => 1,
            Cs::Rgb | Cs::Lab => 3,
            Cs::Cmyk => 4,
            Cs::Tint { n, .. } => *n,
            Cs::Pattern => 0,
        }
    }

    pub fn initial(&self) -> Vec<f64> {
        match self {
            Cs::Cmyk => vec![0.0, 0.0, 0.0, 1.0],
            Cs::Tint { n, .. } => vec![1.0; *n],
            c => vec![0.0; c.n()],
        }
    }

    pub fn rgb(&self, c: &[f64]) -> Rgb {
        let g = |i: usize| c.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0) as f32;
        match self {
            Cs::Gray => [g(0); 3],
            Cs::Rgb => [g(0), g(1), g(2)],
            Cs::Cmyk => {
                let (c_, m, y, k) = (g(0), g(1), g(2), g(3));
                [(1.0 - c_) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)]
            }
            Cs::Lab => {
                // L* a* b* to sRGB, D50 white point, approximate
                let l = c.first().copied().unwrap_or(0.0).clamp(0.0, 100.0);
                let a = c.get(1).copied().unwrap_or(0.0).clamp(-128.0, 127.0);
                let b = c.get(2).copied().unwrap_or(0.0).clamp(-128.0, 127.0);
                let fy = (l + 16.0) / 116.0;
                let (fx, fz) = (fy + a / 500.0, fy - b / 200.0);
                let f = |t: f64| if t > 6.0 / 29.0 { t * t * t } else { 3.0 * (6.0f64 / 29.0).powi(2) * (t - 4.0 / 29.0) };
                let (x, y, z) = (0.9642 * f(fx), f(fy), 0.8249 * f(fz));
                let lin = [3.1339 * x - 1.6169 * y - 0.4906 * z, -0.9788 * x + 1.9161 * y + 0.0335 * z, 0.0719 * x - 0.2290 * y + 1.4052 * z];
                let gm = |v: f64| {
                    let v = v.clamp(0.0, 1.0);
                    (if v <= 0.0031308 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }) as f32
                };
                [gm(lin[0]), gm(lin[1]), gm(lin[2])]
            }
            Cs::Indexed { base, hival, table } => {
                let i = (c.first().copied().unwrap_or(0.0).max(0.0) as usize).min(*hival);
                let n = base.n().max(1);
                let comps: Vec<f64> = (0..n).map(|k| table.get(i * n + k).copied().unwrap_or(0) as f64 / 255.0).collect();
                if matches!(**base, Cs::Lab) {
                    let l = comps[0] * 100.0;
                    return base.rgb(&[l, comps[1] * 255.0 - 128.0, comps[2] * 255.0 - 128.0]);
                }
                base.rgb(&comps)
            }
            Cs::Tint { alt, c0, c1, exp, usable, .. } => {
                let t = c.iter().cloned().fold(0.0f64, f64::max).clamp(0.0, 1.0);
                if !*usable {
                    let v = (1.0 - t) as f32;
                    return [v; 3];
                }
                let out: Vec<f64> = c0.iter().zip(c1.iter()).map(|(a, b)| a + t.powf(*exp) * (b - a)).collect();
                alt.rgb(&out)
            }
            Cs::Pattern => [0.5; 3],
        }
    }

    /// Parse a colour space object. `res`: the resource ColorSpace dictionary for names.
    pub fn parse(f: &File, o: &Obj, res: Option<&Dict>, depth: usize) -> Cs {
        if depth > 4 {
            return Cs::Gray;
        }
        let o = f.resolve(o);
        match &o {
            Obj::Name(n) => match &**n {
                "DeviceGray" | "G" | "CalGray" => Cs::Gray,
                "DeviceRGB" | "RGB" | "CalRGB" => Cs::Rgb,
                "DeviceCMYK" | "CMYK" => Cs::Cmyk,
                "Pattern" => Cs::Pattern,
                other => match res.and_then(|r| r.get(other)) {
                    Some(x) => Cs::parse(f, x, None, depth + 1),
                    None => Cs::Gray,
                },
            },
            Obj::Arr(a) if !a.is_empty() => {
                let name = f.resolve(&a[0]);
                match name.name().unwrap_or("") {
                    "DeviceGray" | "G" | "CalGray" => Cs::Gray,
                    "DeviceRGB" | "RGB" | "CalRGB" => Cs::Rgb,
                    "DeviceCMYK" | "CMYK" => Cs::Cmyk,
                    "Lab" => Cs::Lab,
                    "Pattern" => Cs::Pattern,
                    "ICCBased" => {
                        let n = a.get(1).map(|s| f.resolve(s)).and_then(|s| s.dict().cloned()).and_then(|d| f.get(&d, "N").int()).unwrap_or(3);
                        match n {
                            1 => Cs::Gray,
                            4 => Cs::Cmyk,
                            _ => Cs::Rgb,
                        }
                    }
                    "Indexed" | "I" if a.len() >= 4 => {
                        let base = Cs::parse(f, &a[1], res, depth + 1);
                        let hival = f.resolve(&a[2]).int().unwrap_or(0).clamp(0, 255) as usize;
                        let lookup = f.resolve(&a[3]);
                        let table = match &lookup {
                            Obj::Str(s) => (**s).clone(),
                            s @ Obj::Stream(..) => f.stream_data(s).map(|d| d.data).unwrap_or_default(),
                            _ => vec![],
                        };
                        Cs::Indexed { base: Box::new(base), hival, table: Rc::new(table) }
                    }
                    "Separation" | "DeviceN" if a.len() >= 4 => {
                        let n = if name.name() == Some("Separation") { 1 } else { f.resolve(&a[1]).arr().map(|x| x.len()).unwrap_or(1).clamp(1, 32) };
                        let alt = Cs::parse(f, &a[2], res, depth + 1);
                        let func = f.resolve(&a[3]);
                        let (mut c0, mut c1, mut exp, mut usable) = (vec![0.0; alt.n()], vec![1.0; alt.n()], 1.0, false);
                        if let Some(d) = func.dict() {
                            if f.get(d, "FunctionType").int() == Some(2) && n == 1 {
                                let nums = |k: &str, dflt: Vec<f64>| f.get(d, k).arr().map(|a| a.iter().filter_map(|x| f.resolve(x).num()).collect::<Vec<_>>()).filter(|v| v.len() == alt.n()).unwrap_or(dflt);
                                c0 = nums("C0", c0);
                                c1 = nums("C1", c1);
                                exp = f.get(d, "N").num().unwrap_or(1.0).clamp(0.01, 20.0);
                                usable = true;
                            }
                        }
                        Cs::Tint { n, alt: Box::new(alt), c0, c1, exp, usable }
                    }
                    _ => Cs::Gray,
                }
            }
            _ => Cs::Gray,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_spaces() {
        assert_eq!(Cs::Gray.rgb(&[0.5]), [0.5; 3]);
        assert_eq!(Cs::Cmyk.rgb(&[0.0, 0.0, 0.0, 1.0]), [0.0; 3]);
        assert_eq!(Cs::Cmyk.rgb(&[0.0; 4]), [1.0; 3]);
        assert_eq!(Cs::Rgb.rgb(&[2.0, -1.0, 0.5]), [1.0, 0.0, 0.5]);
    }

    #[test]
    fn indexed_looks_up_and_clamps() {
        let cs = Cs::Indexed { base: Box::new(Cs::Rgb), hival: 1, table: Rc::new(vec![255, 0, 0, 0, 0, 255]) };
        assert_eq!(cs.rgb(&[0.0]), [1.0, 0.0, 0.0]);
        assert_eq!(cs.rgb(&[7.0]), [0.0, 0.0, 1.0]);
    }
}
