//! The thirteen tools of the Developer tools tile: a name, a few words about it, what the boxes are called and the buttons, each
//! button a plain function from the boxes to a result (the work itself lives in the sibling modules).

use super::{b64, convert, diff, hashes, jwt, json, query, regex, url, uuid_time};

/// What a button gets.
pub struct Ctx<'a> {
    pub input: &'a str,
    pub aux: &'a str,
    /// The tool's on/off choice (ignore case for the regex tester).
    pub flag: bool,
    /// How many matches the JSON query writes out.
    pub limit: usize,
}

pub type Act = fn(&Ctx) -> Result<String, String>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Text,
    Ping,
    Ports,
    /// The QR code maker: its own page (kinds of code, level, picture, save buttons).
    Qr,
}

pub struct Tool {
    pub name: &'static str,
    pub blurb: &'static str,
    pub kind: Kind,
    pub input_label: &'static str,
    pub input_hint: &'static str,
    /// A second box: (label, rows, hint).
    pub aux: Option<(&'static str, usize, &'static str)>,
    pub toggle: Option<&'static str>,
    pub actions: &'static [(&'static str, Act)],
}

/// Placeholder for the buttons of the network tools (the app starts those itself).
fn net(_: &Ctx) -> Result<String, String> {
    Err("Press the button on the page.".into())
}

pub const QUERY: usize = 1;
pub const DIFF: usize = 8;
pub const PING: usize = 10;
pub const PORTS: usize = 11;
pub const QR: usize = 12;

pub static TOOLS: [Tool; 13] = [
    Tool { name: "JSON", blurb: "Format, minify, validate (an error names its line and column) or sort the keys.", kind: Kind::Text, input_label: "JSON", input_hint: "Paste JSON here", aux: None, toggle: None,
        actions: &[("Format", |c| json::format(c.input)), ("Minify", |c| json::minify(c.input)), ("Validate", |c| json::validate(c.input)), ("Sort keys", |c| json::sorted(c.input))] },
    Tool { name: "JSON query", blurb: "Query a large JSON document: .items[2].name  .users[*].email  ..id  .items[?(@.age>30)]  .a[-1]  .a[1:3]. To use a big file, type @/path/to/file.json in the box.", kind: Kind::Text, input_label: "JSON", input_hint: "Paste JSON, or @/path/to/file.json", aux: Some(("Path", 1, ".items[*].name")), toggle: None,
        actions: &[("Run query", |c| query::query(c.input, c.aux, c.limit))] },
    Tool { name: "Base64", blurb: "Encode text, or decode standard and URL-safe Base64 (padding and line breaks are optional).", kind: Kind::Text, input_label: "Text or Base64", input_hint: "Type text to encode or Base64 to decode", aux: None, toggle: None,
        actions: &[("Encode", |c| b64::encode_text(c.input, false)), ("Encode URL-safe", |c| b64::encode_text(c.input, true)), ("Decode", |c| b64::decode_text(c.input))] },
    Tool { name: "JWT", blurb: "Read a JSON Web Token: header, payload, and exp / iat / nbf as dates. The signature is NOT verified unless you type a secret and press Verify HS256.", kind: Kind::Text, input_label: "Token", input_hint: "Paste a token (eyJ...)", aux: Some(("Secret (only for Verify HS256)", 1, "your-256-bit-secret")), toggle: None,
        actions: &[("Decode", |c| jwt::decode_now(c.input, "")), ("Verify HS256", |c| if c.aux.is_empty() { Err("Type the secret in the Secret box first.".into()) } else { jwt::decode_now(c.input, c.aux) })] },
    Tool { name: "URL", blurb: "Percent-encode or decode text, or take a URL apart and list its query parameters.", kind: Kind::Text, input_label: "Text or URL", input_hint: "Type text, or paste a URL or a query string", aux: None, toggle: None,
        actions: &[("Encode", |c| url::encode_action(c.input)), ("Decode", |c| url::decode_action(c.input)), ("Parse URL", |c| url::parse(c.input))] },
    Tool { name: "Hash", blurb: "SHA-256, SHA-1, MD5 and CRC-32 of the text (UTF-8). MD5 and SHA-1 are for checksums, not for security.", kind: Kind::Text, input_label: "Text", input_hint: "Type or paste the text", aux: None, toggle: None,
        actions: &[("Hash it", |c| hashes::all(c.input))] },
    Tool { name: "UUID / time", blurb: "New UUIDs (v4 random, v7 time-ordered), and the time converter: type epoch seconds or milliseconds, an ISO 8601 date, or now.", kind: Kind::Text, input_label: "Timestamp", input_hint: "1700000000, 2024-05-06T07:08:09Z or now", aux: None, toggle: None,
        actions: &[("UUID v4", |_| uuid_time::gen(false, 1)), ("5 x UUID v4", |_| uuid_time::gen(false, 5)), ("UUID v7", |_| uuid_time::gen(true, 1)), ("Convert time", |c| uuid_time::convert(c.input, uuid_time::now_ms())), ("Now", |_| uuid_time::convert("now", uuid_time::now_ms()))] },
    Tool { name: "Regex", blurb: "Test a pattern: . * + ? {m,n} [] [^] \\d \\w \\s \\b ^ $ | ( ) (?: ) and lazy *? +?. A pattern that backtracks too much is stopped and the page says so.", kind: Kind::Text, input_label: "Text to search", input_hint: "The text to search in", aux: Some(("Pattern", 1, "\\w+@\\w+\\.com")), toggle: Some("Ignore case"),
        actions: &[("Find matches", |c| regex::test(c.aux, c.input, c.flag))] },
    Tool { name: "Diff", blurb: "Compare two texts line by line. Added lines start with +, removed lines with -, so it reads without colour.", kind: Kind::Text, input_label: "First text", input_hint: "The original text", aux: Some(("Second text", 5, "The changed text")), toggle: None,
        actions: &[("Compare", |c| diff::diff(c.input, c.aux))] },
    Tool { name: "Converters", blurb: "JSON and YAML (a simple subset), CSV and JSON, number bases (255, 0xff, 0b1111, 0o377) and string cases.", kind: Kind::Text, input_label: "Input", input_hint: "Paste what you want to convert", aux: None, toggle: None,
        actions: &[("JSON to YAML", |c| convert::json_to_yaml(c.input)), ("YAML to JSON", |c| convert::yaml_to_json(c.input)), ("CSV to JSON", |c| convert::csv_to_json(c.input)), ("JSON to CSV", |c| convert::json_to_csv(c.input)), ("Number bases", |c| convert::number_bases(c.input)), ("String cases", |c| convert::cases(c.input))] },
    Tool { name: "Ping", blurb: "Send echo requests to the host you type and see the time of each reply, the loss and min / avg / max. Uses ICMP where the system allows it, else times a TCP connect and says so.", kind: Kind::Ping, input_label: "Host or IP", input_hint: "example.com or 192.168.1.1", aux: None, toggle: None,
        actions: &[("Ping 5 times", net), ("Ping 10 times", net)] },
    Tool { name: "Port checker", blurb: "Is a TCP port open, closed or filtered on the host you type? Ports like 22,80,443 or a range 8000-8010 (at most 256), checked in the background with a 1.5 second wait.", kind: Kind::Ports, input_label: "Host or IP", input_hint: "example.com or 192.168.1.10", aux: Some(("Ports", 1, "22,80,443 or 8000-8010")), toggle: None,
        actions: &[("Check ports", net)] },
    Tool { name: "QR code", blurb: "Make a QR code from text, a web address, a Wi-Fi network, a phone number, an e-mail, a text message, a map position or a contact. Black on white, so a phone can scan it. Save it as PNG or SVG.", kind: Kind::Qr, input_label: "Text", input_hint: "Any text: a note, a code, a link", aux: None, toggle: None,
        actions: &[("Copy text", net)] },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_name_and_a_button_and_the_indexes_match() {
        assert_eq!(TOOLS.len(), 13);
        for t in &TOOLS {
            assert!(!t.name.is_empty() && !t.actions.is_empty() && !t.blurb.is_empty());
        }
        assert_eq!(TOOLS[QUERY].name, "JSON query");
        assert_eq!(TOOLS[DIFF].name, "Diff");
        assert_eq!((TOOLS[PING].kind, TOOLS[PORTS].kind, TOOLS[QR].kind), (Kind::Ping, Kind::Ports, Kind::Qr));
        assert_eq!(TOOLS[QR].name, "QR code");
    }

    #[test]
    fn text_buttons_run_on_sample_input() {
        let c = Ctx { input: "{\"a\":[1,2]}", aux: ".a[1]", flag: false, limit: 10 };
        assert!(TOOLS[0].actions[0].1(&c).unwrap().contains("\"a\": ["));
        assert!(TOOLS[1].actions[0].1(&c).unwrap().starts_with("1 match"));
        let c = Ctx { input: "abc", aux: "", flag: false, limit: 10 };
        assert!(TOOLS[3].actions[1].1(&c).is_err());
        assert!(TOOLS[5].actions[0].1(&c).unwrap().contains("ba7816bf"));
    }
}
