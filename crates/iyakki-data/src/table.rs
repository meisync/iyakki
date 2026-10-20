//! Showing results as text: one cell, a Markdown table (what the assistant's tools return), a summary.

use serde_json::Value;

use crate::model::{Page, Summary};

/// A cell as plain text (empty for null; whole floats keep one decimal so 3 and 3.0 stay apart).
pub fn cell_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() => crate::expr::fmt_float(f),
            _ => n.to_string(),
        },
        other => other.to_string(),
    }
}

fn md_cell(s: &str, max: usize) -> String {
    let flat: String = s.chars().map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c }).collect::<String>().replace('|', "\\|");
    if flat.chars().count() > max { format!("{}…", flat.chars().take(max.saturating_sub(1)).collect::<String>()) } else { flat }
}

/// A Markdown table of a page, cells cut to `max_cell` characters.
pub fn markdown(page: &Page, max_cell: usize) -> String {
    let mut out = String::new();
    let head: Vec<String> = page.columns.iter().map(|c| md_cell(&c.name, max_cell)).collect();
    out.push_str(&format!("| {} |\n", head.join(" | ")));
    out.push_str(&format!("|{}|\n", head.iter().map(|_| " --- ").collect::<Vec<_>>().join("|")));
    for r in &page.rows {
        let cells: Vec<String> = r.iter().map(|v| md_cell(&cell_text(v), max_cell)).collect();
        out.push_str(&format!("| {} |\n", cells.join(" | ")));
    }
    out
}

/// The statistics as a Markdown table, one row per column.
pub fn summary_markdown(s: &Summary) -> String {
    let num = |v: Option<f64>| v.map_or(String::new(), |f| crate::expr::fmt_float((f * 1e4).round() / 1e4));
    let mut out = String::from("| column | type | count | nulls | unique | min | max | mean | std | median | most common |\n|---|---|---|---|---|---|---|---|---|---|---|\n");
    for c in &s.columns {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            md_cell(&c.name, 30),
            c.dtype.name(),
            c.count,
            c.nulls,
            c.unique.map_or(String::new(), |u| u.to_string()),
            md_cell(&c.min.as_ref().map_or(String::new(), cell_text), 24),
            md_cell(&c.max.as_ref().map_or(String::new(), cell_text), 24),
            num(c.mean),
            num(c.std),
            num(c.median),
            md_cell(&c.top.iter().take(3).map(|(v, n)| format!("{v} ({n})")).collect::<Vec<_>>().join(", "), 60),
        ));
    }
    out
}
