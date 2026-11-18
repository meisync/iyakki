use iyakki_ui_sdk::toolargs::Args;

/// A chat tool's arguments reach the app as strings, whole numbers and booleans: `Args::str`, `Args::uint`, `Args::bool`. A JSON array or
/// object is kept as raw text and there is no accessor for it. So a list is ONE string argument written as `SKU:QTY` items separated by
/// commas ("A1:2,B7:10"). Declare it in the tool's schema as a string and say the format in its description, which the model reads.
pub const SCHEMA: &str = r#"{"type":"object","properties":{"items":{"type":"string","description":"Comma separated SKU:QTY pairs, for example A1:2,B7:10"}},"required":["items"]}"#;

/// Read `SKU:QTY,SKU:QTY` into pairs; every refusal is a sentence the model can act on.
pub fn parse_items(text: &str) -> Result<Vec<(String, u32)>, String> {
    let mut out = Vec::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (sku, qty) = part.split_once(':').ok_or_else(|| format!("`{part}` is not SKU:QTY"))?;
        let qty: u32 = qty.trim().parse().map_err(|_| format!("the quantity in `{part}` is not a whole number"))?;
        if sku.trim().is_empty() || qty == 0 {
            return Err(format!("`{part}` needs a SKU and a quantity above zero"));
        }
        out.push((sku.trim().to_string(), qty));
    }
    if out.is_empty() { Err("give at least one SKU:QTY".into()) } else { Ok(out) }
}

/// In the tick loop: `let items = tool_items(&inv.args)?;` then act, then `chat.answer(&inv, Ok(..))`.
pub fn tool_items(args_json: &str) -> Result<Vec<(String, u32)>, String> {
    let a = Args::parse(args_json)?;
    parse_items(a.str("items").ok_or("give items as a string like A1:2,B7:10")?)
}
