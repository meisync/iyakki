//! Browser nodes for QA: open a page, take a picture, read the DOM, check things, click, type, fill forms, wait, extract, compare to
//! a baseline. All of them act on ONE browser session that belongs to the run (made by the host on the first node, closed when the
//! run ends); the engine never touches a browser, it asks [`Host::browser_op`]. See `docs/FLOWS.md` ("Browser nodes") and
//! `docs/FLOWS-QA.md`.
//!
//! * Values that are secrets are written as `vault:NAME` (a vault entry's name). They are read at the moment the node runs, are
//!   registered with the redactor (so no log, output, history or report keeps them) and never stored in the flow file.
//! * A failed check is a failed node with a plain message, unless `soft` is on: then the node goes on and the failure is only
//!   recorded (the run report lists it).

use std::sync::Arc;

use serde_json::{json, Map, Value};

use super::*;
use crate::host::BrowserOp;

pub const KINDS: [&str; 20] = [
    "br_open", "br_screenshot", "br_dom", "br_assert", "br_click", "br_type", "br_fill", "br_submit", "br_key", "br_hover", "br_scroll", "br_select", "br_wait", "br_extract", "br_js", "br_frame", "br_viewport", "br_storage", "br_download", "br_compare",
];

pub fn is_browser_qa(kind: &str) -> bool {
    KINDS.contains(&kind)
}

struct Spec {
    id: &'static str,
    op: &'static str,
    label: &'static str,
    blurb: &'static str,
    fields: Vec<Field>,
    /// Config keys that may carry `vault:NAME` values or `{{secret.NAME}}`.
    secret_fields: &'static [&'static str],
    /// Config keys the engine must leave as typed (expressions are kept as text for the page's own use).
    raw: &'static [&'static str],
}

struct QaKind(Spec);

pub fn register(r: &mut Registry) {
    for spec in specs() {
        r.register(Arc::new(QaKind(spec)));
    }
}

fn sel() -> Field {
    Field::new("selector", "Selector", FieldKind::Text, "a CSS selector, e.g. #email, form.login button[type=submit], [data-test=save]")
}

fn idx() -> Field {
    Field::new("index", "Which one", FieldKind::Text, "when several elements match: 0 is the first (default), -1 the last")
}

fn nth_default() -> Field {
    idx()
}

fn specs() -> Vec<Spec> {
    let t = FieldKind::Text;
    vec![
        Spec {
            id: "br_open",
            op: "open",
            label: "Open URL",
            blurb: "Opens a page in the run's browser and waits until it has loaded (or the network is quiet).",
            fields: vec![
                Field::new("url", "Address", t.clone(), "http or https; never with a user name or password in it").required(),
                Field::new("wait", "Wait for", FieldKind::Choice(&["load", "idle", "none"]), "load: the page finished loading; idle: and nothing changes for a moment; none: do not wait").default(json!("idle")),
            ],
            secret_fields: &[],
            raw: &[],
        },
        Spec {
            id: "br_screenshot",
            op: "screenshot",
            label: "Screenshot",
            blurb: "A picture of the page, the whole page, or one element, saved as a PNG file and shown in the run.",
            fields: vec![
                Field::new("name", "Name", t.clone(), "part of the file name, e.g. login-page").default(json!("screenshot")),
                sel(),
                nth_default(),
                Field::new("full_page", "Whole page", FieldKind::Bool, "scroll and join pictures to cover the whole page (up to 12 screens)").default(json!(false)),
            ],
            secret_fields: &[],
            raw: &[],
        },
        Spec {
            id: "br_dom",
            op: "dom",
            label: "Get DOM",
            blurb: "Reads the page: the whole DOM, an element's HTML, text or attributes, title, address, console messages, links. Passwords and tokens are removed.",
            fields: vec![
                Field::new("mode", "Read", FieldKind::Choice(&["page", "outer_html", "text", "attributes", "summary"]), "page: the whole serialized DOM; outer_html/text/attributes: of the selector; summary: title, address, console only").default(json!("page")),
                sel(),
                nth_default(),
                Field::new("max_bytes", "Size cap", FieldKind::Number, "longest text kept, in bytes (default 200000, at most 2000000)").default(json!(200_000)),
                Field::new("console", "Console", FieldKind::Bool, "include console messages and script errors seen since the page was opened").default(json!(true)),
                Field::new("links", "Links", FieldKind::Bool, "include every link's address and text").default(json!(false)),
                Field::new("failures", "Failed loads", FieldKind::Bool, "include images, scripts and styles that failed to load (what the engine can tell)").default(json!(false)),
            ],
            secret_fields: &[],
            raw: &[],
        },
        Spec {
            id: "br_assert",
            op: "assert",
            label: "Check",
            blurb: "Checks the page: a selector exists or has a count, its text equals, contains or matches, an attribute, visible, enabled; the address or title. Fails the node with a clear message, or only records it (soft).",
            fields: vec![
                Field::new(
                    "check",
                    "Check",
                    FieldKind::Choice(&[
                        "exists", "not_exists", "count", "text_equals", "text_contains", "text_matches", "attr_equals", "attr_contains", "value_equals", "visible", "hidden", "enabled", "disabled", "checked", "url_equals", "url_contains", "title_equals", "title_contains", "no_console_errors", "page_contains", "page_not_contains",
                    ]),
                    "what must be true",
                )
                .required(),
                sel(),
                nth_default(),
                Field::new("expected", "Expected", t.clone(), "the text, number, pattern or address to compare with"),
                Field::new("attribute", "Attribute", t.clone(), "for the attribute checks: the attribute's name, e.g. href"),
                Field::new("soft", "Soft", FieldKind::Bool, "record a failure but keep the run going").default(json!(false)),
                Field::new("wait_ms", "Keep trying for", FieldKind::Number, "milliseconds to wait for the check to become true (default 2000, 0: look once)").default(json!(2000)),
            ],
            secret_fields: &[],
            raw: &[],
        },
        Spec {
            id: "br_click",
            op: "click",
            label: "Click",
            blurb: "Clicks an element (scrolled into view first).",
            fields: vec![sel().required(), nth_default(), Field::new("double", "Double click", FieldKind::Bool, "").default(json!(false)), Field::new("native", "Real mouse", FieldKind::Bool, "send a real mouse press at the element's centre instead of the page's own click").default(json!(false))],
            secret_fields: &[],
            raw: &[],
        },
        Spec {
            id: "br_type",
            op: "type",
            label: "Type text",
            blurb: "Types text into a field. Use vault:NAME for a password; it is read when the node runs and never stored.",
            fields: vec![
                sel().required(),
                nth_default(),
                Field::new("text", "Text", t.clone(), "what to type; vault:NAME takes it from your vault").required(),
                Field::new("clear", "Clear first", FieldKind::Bool, "").default(json!(true)),
                Field::new("native", "Real keys", FieldKind::Bool, "type with real key input instead of setting the value").default(json!(false)),
                Field::new("press_enter", "Press Enter after", FieldKind::Bool, "").default(json!(false)),
            ],
            secret_fields: &["text"],
            raw: &[],
        },
        Spec {
            id: "br_fill",
            op: "fill",
            label: "Fill form",
            blurb: "Fills several fields at once: a map of selector to value (inputs, text areas, selects, checkboxes, radios). File inputs are refused unless the file is in a folder you may read.",
            fields: vec![
                Field::new("fields", "Fields", FieldKind::Json, "{\"#email\": \"qa@example.com\", \"#password\": \"vault:qa-password\", \"#terms\": true, \"#country\": \"India\"}").required(),
                Field::new("submit", "Submit after", FieldKind::Text, "optional: selector of the button or form to submit when done"),
            ],
            secret_fields: &["fields"],
            raw: &[],
        },
        Spec {
            id: "br_submit",
            op: "submit",
            label: "Submit form",
            blurb: "Submits a form (the page's own submit handlers run).",
            fields: vec![sel(), nth_default()],
            secret_fields: &[],
            raw: &[],
        },
        Spec {
            id: "br_key",
            op: "key",
            label: "Press key",
            blurb: "Presses a key (Enter, Escape, Tab, ArrowDown, one character...), in a field if you name one.",
            fields: vec![Field::new("key", "Key", t.clone(), "Enter, Escape, Tab, Backspace, Delete, ArrowUp/Down/Left/Right, Home, End, PageUp, PageDown, Space or one character").required(), sel(), nth_default()],
            secret_fields: &[],
            raw: &[],
        },
        Spec { id: "br_hover", op: "hover", label: "Hover", blurb: "Moves the pointer over an element (menus and tooltips).", fields: vec![sel().required(), nth_default()], secret_fields: &[], raw: &[] },
        Spec {
            id: "br_scroll",
            op: "scroll",
            label: "Scroll",
            blurb: "Scrolls to an element, to the top or bottom, or by an amount.",
            fields: vec![
                Field::new("to", "Scroll to", FieldKind::Choice(&["selector", "top", "bottom", "down", "up"]), "").default(json!("selector")),
                sel(),
                nth_default(),
                Field::new("pixels", "Pixels", FieldKind::Text, "for down and up (default one screen)"),
            ],
            secret_fields: &[],
            raw: &[],
        },
        Spec {
            id: "br_select",
            op: "select",
            label: "Select option",
            blurb: "Chooses an option of a drop-down list by its text, value or position.",
            fields: vec![sel().required(), nth_default(), Field::new("option", "Option", t.clone(), "the option's text, value or position").required(), Field::new("by", "Match by", FieldKind::Choice(&["auto", "label", "value", "index"]), "").default(json!("auto"))],
            secret_fields: &[],
            raw: &[],
        },
        Spec {
            id: "br_wait",
            op: "wait",
            label: "Wait for",
            blurb: "Waits for a selector to appear or go away, some text, an address, or just for time.",
            fields: vec![
                Field::new("for", "Wait for", FieldKind::Choice(&["selector", "selector_gone", "text", "text_gone", "url", "time"]), "").default(json!("selector")),
                Field::new("value", "Selector / text / address", t.clone(), "what to wait for (a CSS selector for the selector choices)"),
                Field::new("ms", "Time (ms)", FieldKind::Number, "for `time`; otherwise it is the node's time limit that ends the wait"),
            ],
            secret_fields: &[],
            raw: &[],
        },
        Spec {
            id: "br_extract",
            op: "extract",
            label: "Extract",
            blurb: "Reads parts of the page into JSON: fields by selector, or the rows of a table.",
            fields: vec![
                Field::new("fields", "Fields", FieldKind::Json, "{\"price\": \".price\", \"link\": {\"selector\": \"a.buy\", \"attr\": \"href\"}, \"tags\": {\"selector\": \".tag\", \"all\": true}}"),
                Field::new("rows", "Rows", FieldKind::Json, "{\"selector\": \"table tr\", \"columns\": {\"name\": \"td:nth-child(1)\", \"qty\": \"td:nth-child(2)\"}}"),
            ],
            secret_fields: &[],
            raw: &[],
        },
        Spec {
            id: "br_js",
            op: "js",
            label: "Run JavaScript",
            blurb: "Runs a JavaScript expression in the page and returns its JSON value. Administrators only; time limited.",
            fields: vec![
                Field::new("expression", "Expression", FieldKind::Multiline, "document.title, or: (function(a){ return a.n * 2 })(args) ").required(),
                Field::new("args", "Arguments", FieldKind::Json, "available to the expression as `args`"),
            ],
            secret_fields: &["args"],
            raw: &["expression"],
        },
        Spec {
            id: "br_frame",
            op: "frame",
            label: "Switch tab/frame",
            blurb: "Aims later nodes at an iframe of the page (same site), its parent, or back at the main page. The engine has one tab.",
            fields: vec![Field::new("action", "Action", FieldKind::Choice(&["frame", "parent", "main", "tabs"]), "frame: go into the iframe named by the selector; parent: one level up; main: top page; tabs: list the tabs").default(json!("frame")), sel()],
            secret_fields: &[],
            raw: &[],
        },
        Spec {
            id: "br_viewport",
            op: "viewport",
            label: "Set viewport",
            blurb: "Sets the window size, to test phone, tablet and desktop layouts.",
            fields: vec![
                Field::new("preset", "Size", FieldKind::Choice(&["custom", "phone 375x667", "tablet 768x1024", "laptop 1280x800", "desktop 1920x1080"]), "").default(json!("custom")),
                Field::new("width", "Width", FieldKind::Number, "pixels, 200 to 1920").default(json!(1280)),
                Field::new("height", "Height", FieldKind::Number, "pixels, 200 to 1200").default(json!(800)),
            ],
            secret_fields: &[],
            raw: &[],
        },
        Spec {
            id: "br_storage",
            op: "storage",
            label: "Set cookie / storage",
            blurb: "Sets a cookie, a localStorage or a sessionStorage value of the current site. Secret values are vault:NAME references only.",
            fields: vec![
                Field::new("kind", "Where", FieldKind::Choice(&["cookie", "local_storage", "session_storage"]), "").default(json!("cookie")),
                Field::new("name", "Name", t.clone(), "").required(),
                Field::new("value", "Value", t.clone(), "text, or vault:NAME to take it from your vault").required(),
                Field::new("path", "Cookie path", t.clone(), "default /"),
            ],
            secret_fields: &["value"],
            raw: &[],
        },
        Spec {
            id: "br_download",
            op: "download",
            label: "Wait for download",
            blurb: "Waits for the page to start a download (click the link first, in an earlier node) and saves the file in your write folders.",
            fields: vec![
                Field::new("name_contains", "File name has", t.clone(), "optional: wait for a download whose name contains this"),
                Field::new("save_in", "Save in", t.clone(), "a folder you may write to (default: the flow's work folder)"),
            ],
            secret_fields: &[],
            raw: &[],
        },
        Spec {
            id: "br_compare",
            op: "compare",
            label: "Compare to baseline",
            blurb: "Takes a picture and compares it with the saved baseline; the first run saves the baseline. Fails above the threshold and makes a diff picture.",
            fields: vec![
                Field::new("name", "Baseline name", t.clone(), "e.g. home-desktop").required(),
                sel(),
                nth_default(),
                Field::new("threshold", "Allowed change (%)", FieldKind::Text, "percent of pixels that may differ, e.g. 0.5 (default 0.5)").default(json!("0.5")),
                Field::new("tolerance", "Colour tolerance", FieldKind::Number, "how far one colour channel may move before a pixel counts as changed, 0 to 255 (default 8)").default(json!(8)),
                Field::new("update", "Replace baseline", FieldKind::Bool, "save this picture as the new baseline").default(json!(false)),
                Field::new("soft", "Soft", FieldKind::Bool, "record a difference but keep the run going").default(json!(false)),
            ],
            secret_fields: &[],
            raw: &[],
        },
    ]
}

/// The host a URL names, lower case; empty when it names none.
pub fn url_host(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or("");
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    let auth = auth.rsplit_once('@').map_or(auth, |x| x.1);
    let h = auth.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map_or(auth, |x| x.0);
    h.trim_matches(['[', ']']).to_ascii_lowercase()
}

/// The hosts the flow's Open URL nodes name. Addresses that are templates are filled in with what is known when the browser
/// session starts (the run's inputs and variables); one that needs an earlier node's output adds nothing.
pub fn flow_hosts(flow: &Flow, ctx: &Value) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for n in flow.nodes.iter().filter(|n| n.kind == "br_open") {
        let Some(raw) = n.config.get("url") else { continue };
        let url = match raw {
            Value::String(u) if u.contains("{{") => match crate::expr::resolve(raw, ctx, None) {
                Ok(Value::String(r)) => r,
                _ => continue,
            },
            Value::String(u) => u.clone(),
            _ => continue,
        };
        let h = url_host(&url);
        if !h.is_empty() && !v.contains(&h) {
            v.push(h);
        }
    }
    v
}

fn has_userinfo(url: &str) -> bool {
    let rest = url.split("://").nth(1).unwrap_or("");
    rest.split(['/', '?', '#']).next().unwrap_or("").contains('@')
}

/// Replace every `vault:NAME` text in `v` by the vault entry, remembering each in `found` (and with the redactor).
fn unvault(cx: &NodeCx, v: &Value, found: &mut Vec<String>) -> Result<Value, NodeError> {
    Ok(match v {
        Value::String(t) => match t.strip_prefix("vault:") {
            Some(name) if !name.is_empty() && !name.contains(char::is_whitespace) => {
                let secret = cx.host.secret(name).map_err(NodeError::consent)?;
                cx.redactor.add(&secret);
                if !found.contains(&secret) {
                    found.push(secret.clone());
                }
                Value::String(secret)
            }
            _ => v.clone(),
        },
        Value::Array(a) => Value::Array(a.iter().map(|x| unvault(cx, x, found)).collect::<Result<_, _>>()?),
        Value::Object(o) => {
            let mut m = Map::new();
            for (k, x) in o {
                m.insert(k.clone(), unvault(cx, x, found)?);
            }
            Value::Object(m)
        }
        other => other.clone(),
    })
}

fn check_selector(node: &Node, key: &str, v: &mut Vec<Issue>) {
    let sel = s(&node.config, key);
    if sel.trim().is_empty() {
        v.push(Issue::error(&node.id, "give a CSS selector"));
    } else if sel.chars().any(|c| c == '\0' || c == '\n') || sel.len() > 500 {
        v.push(Issue::error(&node.id, "that selector is not usable (a line break, or longer than 500 characters)"));
    }
}

impl NodeKind for QaKind {
    fn id(&self) -> &'static str {
        self.0.id
    }
    fn label(&self) -> &'static str {
        self.0.label
    }
    fn blurb(&self) -> &'static str {
        self.0.blurb
    }
    fn category(&self) -> &'static str {
        "Browser"
    }
    fn schema(&self) -> Vec<Field> {
        self.0.fields.clone()
    }
    fn secret_fields(&self) -> &'static [&'static str] {
        self.0.secret_fields
    }
    fn raw_fields(&self) -> &'static [&'static str] {
        self.0.raw
    }
    fn validate(&self, node: &Node, _f: &Flow) -> Vec<Issue> {
        let c = &node.config;
        let mut v = Vec::new();
        let tmpl = |k: &str| s(c, k).contains("{{");
        match self.0.id {
            "br_open" => {
                let u = s(c, "url");
                if u.is_empty() {
                    v.push(Issue::error(&node.id, "give the address to open"));
                } else if !tmpl("url") {
                    if !(u.starts_with("http://") || u.starts_with("https://")) {
                        v.push(Issue::error(&node.id, "only http and https addresses can be opened"));
                    } else if has_userinfo(u) {
                        v.push(Issue::error(&node.id, "no user name or password in an address: use Type text with vault:NAME on the sign-in form"));
                    } else if url_host(u).is_empty() {
                        v.push(Issue::error(&node.id, "the address has no host"));
                    }
                }
            }
            "br_click" | "br_hover" => check_selector(node, "selector", &mut v),
            "br_type" => {
                check_selector(node, "selector", &mut v);
                if c.get("text").is_none_or(Value::is_null) {
                    v.push(Issue::error(&node.id, "give the text to type (vault:NAME takes it from your vault)"));
                }
            }
            "br_select" => {
                check_selector(node, "selector", &mut v);
                if s(c, "option").is_empty() {
                    v.push(Issue::error(&node.id, "name the option to choose"));
                }
            }
            "br_scroll" if matches!(s(c, "to"), "" | "selector") => check_selector(node, "selector", &mut v),
            "br_fill" => match c.get("fields") {
                Some(Value::Object(m)) if !m.is_empty() => {
                    if m.keys().any(|k| k.trim().is_empty()) {
                        v.push(Issue::error(&node.id, "a field needs a selector as its key"));
                    }
                }
                _ => v.push(Issue::error(&node.id, "give the fields as a map of selector to value")),
            },
            "br_key" if s(c, "key").is_empty() => v.push(Issue::error(&node.id, "name the key to press")),
            "br_assert" => {
                let check = s(c, "check");
                let needs_sel = !matches!(check, "url_equals" | "url_contains" | "title_equals" | "title_contains" | "no_console_errors" | "page_contains" | "page_not_contains");
                if check.is_empty() {
                    v.push(Issue::error(&node.id, "choose what to check"));
                }
                if needs_sel {
                    check_selector(node, "selector", &mut v);
                }
                if matches!(check, "attr_equals" | "attr_contains") && s(c, "attribute").is_empty() {
                    v.push(Issue::error(&node.id, "name the attribute to check"));
                }
                if matches!(check, "text_equals" | "text_contains" | "text_matches" | "attr_equals" | "attr_contains" | "value_equals" | "count" | "url_equals" | "url_contains" | "title_equals" | "title_contains" | "page_contains" | "page_not_contains") && c.get("expected").is_none_or(|e| e.is_null() || e.as_str() == Some("")) {
                    v.push(Issue::error(&node.id, "say what is expected"));
                }
                if check == "text_matches" && !tmpl("expected") && crate::minire::Re::new(s(c, "expected")).is_err() {
                    v.push(Issue::error(&node.id, "the pattern is not valid"));
                }
            }
            "br_wait" => {
                let w = if s(c, "for").is_empty() { "selector" } else { s(c, "for") };
                if w == "time" {
                    if u(c, "ms").is_none() {
                        v.push(Issue::error(&node.id, "give the time to wait in milliseconds"));
                    }
                } else if s(c, "value").is_empty() {
                    v.push(Issue::error(&node.id, "say what to wait for"));
                }
            }
            "br_extract" => {
                let has_f = matches!(c.get("fields"), Some(Value::Object(m)) if !m.is_empty());
                let has_r = matches!(c.get("rows"), Some(Value::Object(m)) if m.contains_key("selector"));
                if !has_f && !has_r {
                    v.push(Issue::error(&node.id, "give fields (name to selector) or rows (a row selector and columns)"));
                }
            }
            "br_js" => {
                if s(c, "expression").trim().is_empty() {
                    v.push(Issue::error(&node.id, "give the expression"));
                }
                v.push(Issue::warn(&node.id, "this step runs script in the page: only administrators can run it"));
            }
            "br_frame" if matches!(s(c, "action"), "" | "frame") => check_selector(node, "selector", &mut v),
            "br_viewport" if s(c, "preset").is_empty() || s(c, "preset") == "custom" => {
                for (k, max) in [("width", 1920), ("height", 1200)] {
                    match u(c, k) {
                        Some(n) if (200..=max).contains(&n) => {}
                        _ if tmpl(k) => {}
                        _ => v.push(Issue::error(&node.id, format!("{k} must be 200 to {max} pixels"))),
                    }
                }
            }
            "br_storage" => {
                if s(c, "name").is_empty() {
                    v.push(Issue::error(&node.id, "give the name"));
                }
                let val = s(c, "value");
                if !val.starts_with("vault:") && !val.contains("{{secret.") {
                    let l = s(c, "name").to_ascii_lowercase();
                    if ["token", "secret", "session", "password", "auth", "key", "jwt"].iter().any(|w| l.contains(w)) {
                        v.push(Issue::error(&node.id, "a value that looks like a credential must be vault:NAME, never written into the flow"));
                    }
                }
            }
            "br_compare" => {
                if s(c, "name").is_empty() {
                    v.push(Issue::error(&node.id, "name the baseline"));
                }
            }
            _ => {}
        }
        v
    }
    fn run(&self, cx: &NodeCx, node: &Node, cfg: &Value) -> NodeResult {
        let mut args = cfg.clone();
        let mut secrets: Vec<String> = Vec::new();
        if matches!(self.0.op, "type" | "fill" | "storage" | "js") {
            args = unvault(cx, &args, &mut secrets)?;
        }
        // a secret may also arrive through `{{secret.NAME}}` (the engine already registered it with the redactor)
        if self.0.id == "br_open" && has_userinfo(s(&args, "url")) {
            return Err(NodeError::config("no user name or password in an address"));
        }
        let soft = b(cfg, "soft", false);
        let hosts = flow_hosts(cx.flow, cx.ctx);
        let profile = if cx.flow.defaults.browser.profile.is_empty() { "isolated".to_string() } else { cx.flow.defaults.browser.profile.clone() };
        let req = BrowserOp {
            op: self.0.op.to_string(),
            args,
            timeout_ms: cx.timeout_ms,
            flow_id: cx.flow.id.clone(),
            run_id: cx.run_id.to_string(),
            node_id: cx.node_id.to_string(),
            node_title: node.title().to_string(),
            hosts,
            trusted: cx.flow.defaults.browser.trusted,
            profile,
            secrets,
            max_actions: cx.flow.defaults.browser.max_actions,
            max_minutes: cx.flow.defaults.browser.max_minutes,
        };
        cx.log(format!("browser: {}", self.describe(node)));
        let r = cx.host.browser_op(&req, cx.cancel)?;
        for l in r.log.iter().take(20) {
            cx.log(l);
        }
        let mut value = r.value;
        if matches!(self.0.op, "assert" | "compare") {
            let passed = value.get("passed").and_then(Value::as_bool).unwrap_or(false);
            let message = value.get("message").and_then(Value::as_str).unwrap_or("").to_string();
            if let Value::Object(m) = &mut value {
                m.insert("soft".into(), json!(soft));
            }
            if !passed {
                if !soft {
                    let mut e = NodeError::new(message);
                    e.transient = Some(true);
                    e.output = value.to_string();
                    return Err(e);
                }
                cx.log(format!("soft check failed, the flow goes on: {message}"));
            }
        }
        Ok(NodeOutput { value, port: "out".into(), wait: None, artifacts: r.artifacts })
    }
    fn classify_error(&self, e: &NodeError, _n: &Node) -> ErrClass {
        if e.navigation || e.timed_out {
            ErrClass::Retryable
        } else {
            default_class(e)
        }
    }
    fn describe(&self, node: &Node) -> String {
        let c = &node.config;
        let sel = s(c, "selector");
        match self.0.id {
            "br_open" => format!("open {}", crate::kinds::preview(&json!(s(c, "url")), 60)),
            "br_assert" => format!("check {} {}", s(c, "check"), sel).trim().to_string(),
            "br_key" => format!("press {}", s(c, "key")),
            "br_wait" => format!("wait for {} {}", s(c, "for"), s(c, "value")).trim().to_string(),
            "br_viewport" => format!("viewport {}", if s(c, "preset").is_empty() || s(c, "preset") == "custom" { format!("{}x{}", u(c, "width").unwrap_or(0), u(c, "height").unwrap_or(0)) } else { s(c, "preset").to_string() }),
            "br_screenshot" | "br_compare" => format!("{} {}", self.0.label.to_lowercase(), s(c, "name")),
            _ => format!("{} {}", self.0.label.to_lowercase(), sel).trim().to_string(),
        }
    }
}
