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
        };
    }
    o.push_str("]}");
    o
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
