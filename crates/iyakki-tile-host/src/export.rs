//! Exporters so a browser can show a display list: a flat JSON document (for the later browser-side
//! renderer, which draws it on a canvas) and a standalone SVG (for previews and golden files).
use iyakki_tile_abi::{line_height, measure_text, Color, DisplayList, Op, Rect, ABI_VERSION};
use std::fmt::Write;

fn hex(c: Color) -> String {
    format!("#{:02x}{:02x}{:02x}{:02x}", c.0, c.1, c.2, c.3)
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            c if (c as u32) < 0x20 => {}
            c => o.push(c),
        }
    }
    o
}

fn jstr(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn jrect(r: &Rect) -> String {
    format!("\"x\":{},\"y\":{},\"w\":{},\"h\":{}", r.x, r.y, r.w, r.h)
}

fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut o = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        o.push(T[(n >> 18) as usize & 63] as char);
        o.push(T[(n >> 12) as usize & 63] as char);
        o.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        o.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    o
}

/// Like [`to_json`], and the pictures the list names (and only those) go with it as `"images":{"7":{"w":..,"h":..,"rgba":"base64"}}`,
/// for the browser renderer to draw them.
pub fn to_json_with_images(list: &DisplayList, width: i32, height: i32, images: &crate::render::ImageTable) -> String {
    let mut o = to_json(list, width, height);
    let mut used: Vec<u32> = list.ops.iter().filter_map(|op| if let Op::Image { handle, .. } = op { Some(*handle) } else { None }).collect();
    used.sort_unstable();
    used.dedup();
    let mut parts = Vec::new();
    for h in used {
        if let Some(p) = images.get(h) {
            parts.push(format!("\"{h}\":{{\"w\":{},\"h\":{},\"rgba\":\"{}\"}}", p.w, p.h, base64(&p.rgba)));
        }
    }
    if !parts.is_empty() && o.ends_with('}') {
        o.pop();
        o.push_str(&format!(",\"images\":{{{}}}}}", parts.join(",")));
    }
    o
}

/// `{"abi":1,"width":W,"height":H,"ops":[...]}`; text carries its measured width so a browser lays
/// it out exactly like the device.
pub fn to_json(list: &DisplayList, width: i32, height: i32) -> String {
    let mut o = format!("{{\"abi\":{ABI_VERSION},\"width\":{width},\"height\":{height},\"ops\":[");
    for (i, op) in list.ops.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        let _ = match op {
            Op::Rect { r, color } => write!(o, "{{\"op\":\"rect\",{},\"color\":\"{}\"}}", jrect(r), hex(*color)),
            Op::RoundRect { r, radius, color } => {
                write!(o, "{{\"op\":\"round_rect\",{},\"radius\":{radius},\"color\":\"{}\"}}", jrect(r), hex(*color))
            }
            Op::Outline { r, thickness, color } => {
                write!(o, "{{\"op\":\"outline\",{},\"thickness\":{thickness},\"color\":\"{}\"}}", jrect(r), hex(*color))
            }
            Op::Line { x1, y1, x2, y2, thickness, color } => write!(
                o,
                "{{\"op\":\"line\",\"x1\":{x1},\"y1\":{y1},\"x2\":{x2},\"y2\":{y2},\"thickness\":{thickness},\"color\":\"{}\"}}",
                hex(*color)
            ),
            Op::Text { x, y, face, size, color, text } => write!(
                o,
                "{{\"op\":\"text\",\"x\":{x},\"y\":{y},\"face\":{face},\"size\":{size},\"width\":{},\"height\":{},\"color\":\"{}\",\"text\":{}}}",
                measure_text(*face, *size, text),
                line_height(*size),
                hex(*color),
                jstr(text)
            ),
            Op::Image { handle, r } => write!(o, "{{\"op\":\"image\",\"handle\":{handle},{}}}", jrect(r)),
            Op::PushClip { r } => write!(o, "{{\"op\":\"push_clip\",{}}}", jrect(r)),
            Op::PopClip => write!(o, "{{\"op\":\"pop_clip\"}}"),
            Op::Hit { id, r } => write!(o, "{{\"op\":\"hit\",\"id\":{id},{}}}", jrect(r)),
            Op::Access { id, role, r, name, value, flags } => write!(
                o,
                "{{\"op\":\"access\",\"id\":{id},\"role\":{},\"name\":{},\"value\":{},\"flags\":{flags},{}}}",
                *role as u8,
                jstr(name),
                jstr(value),
                jrect(r)
            ),
        };
    }
    o.push_str("]}");
    o
}

/// The other way round (F-CLOUD-76): a frame a tenant's web service sent (`to_json`'s document) as a display list, checked by the ABI's
/// own limits (the list is encoded and decoded again, so a list the codec would refuse is refused here). Returns the list and the
/// surface size it was drawn for. Unknown operations, a missing field and a colour that is not `#rrggbb[aa]` are errors in words.
pub fn from_json(v: &serde_json::Value) -> Result<(DisplayList, i32, i32), String> {
    use iyakki_tile_abi::Role;
    let num = |o: &serde_json::Value, k: &str| -> Result<i32, String> { o.get(k).and_then(|x| x.as_i64()).and_then(|x| i32::try_from(x).ok()).ok_or_else(|| format!("\"{k}\" is a whole number")) };
    let rect = |o: &serde_json::Value| -> Result<Rect, String> { Ok(Rect::new(num(o, "x")?, num(o, "y")?, num(o, "w")?.max(0), num(o, "h")?.max(0))) };
    let colour = |o: &serde_json::Value| -> Result<Color, String> {
        let t = o.get("color").and_then(|c| c.as_str()).and_then(|c| c.strip_prefix('#')).ok_or("\"color\" is a colour like #rrggbbaa")?;
        if !(t.len() == 6 || t.len() == 8) || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("\"color\" is a colour like #rrggbbaa".into());
        }
        let b = |i: usize| u8::from_str_radix(&t[i..i + 2], 16).unwrap_or(0);
        Ok(Color(b(0), b(2), b(4), if t.len() == 8 { b(6) } else { 255 }))
    };
    let (w, h) = (num(v, "width")?, num(v, "height")?);
    let ops = v.get("ops").and_then(|o| o.as_array()).ok_or("the frame has no \"ops\"")?;
    let mut list = DisplayList::new();
    for o in ops {
        let name = o.get("op").and_then(|n| n.as_str()).ok_or("an operation has no name")?;
        let op = match name {
            "rect" => Op::Rect { r: rect(o)?, color: colour(o)? },
            "round_rect" => Op::RoundRect { r: rect(o)?, radius: num(o, "radius")?, color: colour(o)? },
            "outline" => Op::Outline { r: rect(o)?, thickness: num(o, "thickness")?, color: colour(o)? },
            "line" => Op::Line { x1: num(o, "x1")?, y1: num(o, "y1")?, x2: num(o, "x2")?, y2: num(o, "y2")?, thickness: num(o, "thickness")?, color: colour(o)? },
            "text" => Op::Text { x: num(o, "x")?, y: num(o, "y")?, face: u8::try_from(num(o, "face")?).map_err(|_| "\"face\" is a small number")?, size: u32::try_from(num(o, "size")?).map_err(|_| "\"size\" is positive")?, color: colour(o)?, text: o.get("text").and_then(|t| t.as_str()).ok_or("text has no \"text\"")?.to_string() },
            "image" => Op::Image { handle: u32::try_from(num(o, "handle")?).map_err(|_| "\"handle\" is positive")?, r: rect(o)? },
            "push_clip" => Op::PushClip { r: rect(o)? },
            "pop_clip" => Op::PopClip,
            "hit" => Op::Hit { id: u32::try_from(num(o, "id")?).map_err(|_| "\"id\" is positive")?, r: rect(o)? },
            "access" => {
                let role = match num(o, "role")? {
                    1 => Role::Button,
                    2 => Role::Field,
                    3 => Role::ListItem,
                    4 => Role::Toggle,
                    5 => Role::Tab,
                    6 => Role::Heading,
                    7 => Role::Slider,
                    8 => Role::Link,
                    9 => Role::Text,
                    _ => return Err("an access node has an unknown role".into()),
                };
                Op::Access { id: u32::try_from(num(o, "id")?).map_err(|_| "\"id\" is positive")?, role, r: rect(o)?, name: o.get("name").and_then(|t| t.as_str()).unwrap_or("").to_string(), value: o.get("value").and_then(|t| t.as_str()).unwrap_or("").to_string(), flags: u8::try_from(num(o, "flags").unwrap_or(0)).map_err(|_| "\"flags\" is a small number")? }
            }
            other => return Err(format!("the frame has an operation this version does not know: {other}")),
        };
        list.push(op);
    }
    // the ABI's own limits decide: what the codec would not carry is not drawn
    let bytes = list.encode().map_err(|e| format!("the frame is over the tile limits: {e:?}"))?;
    let list = DisplayList::decode(&bytes).map_err(|e| format!("the frame is over the tile limits: {e:?}"))?;
    Ok((list, w, h))
}

fn fill(c: Color) -> String {
    format!("fill=\"#{:02x}{:02x}{:02x}\" fill-opacity=\"{:.3}\"", c.0, c.1, c.2, c.3 as f32 / 255.0)
}
fn stroke(c: Color, t: i32) -> String {
    format!("stroke=\"#{:02x}{:02x}{:02x}\" stroke-opacity=\"{:.3}\" stroke-width=\"{t}\"", c.0, c.1, c.2, c.3 as f32 / 255.0)
}

/// A standalone SVG of the list. Hit regions become transparent rectangles with `data-hit-id`.
pub fn to_svg(list: &DisplayList, width: i32, height: i32) -> String {
    let mut o = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\">"
    );
    let (mut clip_n, mut open) = (0u32, 0u32);
    for op in &list.ops {
        let _ = match op {
            Op::Rect { r, color } => {
                write!(o, "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" {}/>", r.x, r.y, r.w, r.h, fill(*color))
            }
            Op::RoundRect { r, radius, color } => write!(
                o,
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{radius}\" {}/>",
                r.x, r.y, r.w, r.h, fill(*color)
            ),
            Op::Outline { r, thickness, color } => {
                // the border lies inside the rectangle, so the stroke is centred half a width in
                let t = *thickness;
                write!(
                    o,
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\" {}/>",
                    r.x as f32 + t as f32 / 2.0,
                    r.y as f32 + t as f32 / 2.0,
                    (r.w - t).max(0),
                    (r.h - t).max(0),
                    stroke(*color, t)
                )
            }
            Op::Line { x1, y1, x2, y2, thickness, color } => {
                write!(o, "<line x1=\"{x1}\" y1=\"{y1}\" x2=\"{x2}\" y2=\"{y2}\" {}/>", stroke(*color, *thickness))
            }
            Op::Text { x, y, face, size, color, text } => {
                let fam = if *face == 1 { "monospace" } else { "sans-serif" };
                let weight = if *face == 2 { " font-weight=\"bold\"" } else { "" };
                write!(
                    o,
                    "<text x=\"{x}\" y=\"{}\" font-family=\"{fam}\"{weight} font-size=\"{size}\" textLength=\"{}\" lengthAdjust=\"spacingAndGlyphs\" {}>{}</text>",
                    y + (*size as i32) * 4 / 5,
                    measure_text(*face, *size, text),
                    fill(*color),
                    esc(text)
                )
            }
            Op::Image { handle, r } => write!(
                o,
                "<image href=\"iyakki-image:{handle}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/>",
                r.x, r.y, r.w, r.h
            ),
            Op::PushClip { r } => {
                clip_n += 1;
                open += 1;
                write!(
                    o,
                    "<clipPath id=\"c{clip_n}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/></clipPath><g clip-path=\"url(#c{clip_n})\">",
                    r.x, r.y, r.w, r.h
                )
            }
            Op::PopClip => {
                open = open.saturating_sub(1);
                write!(o, "</g>")
            }
            Op::Access { .. } => Ok(()),
            Op::Hit { id, r } => write!(
                o,
                "<rect data-hit-id=\"{id}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\" pointer-events=\"all\"/>",
                r.x, r.y, r.w, r.h
            ),
        };
    }
    for _ in 0..open {
        o.push_str("</g>");
    }
    o.push_str("</svg>");
    o
}
