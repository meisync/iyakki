//! A forgiving HTML parser. It never fails: whatever it is given becomes a tree. Scripts and styles are dropped while
//! parsing, depth and size are capped, and there is no recursion in the parser itself.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Element { tag: String, attrs: Vec<(String, String)> },
    Text(String),
}

#[derive(Clone, Debug)]
pub struct Node {
    pub kind: NodeKind,
    pub children: Vec<usize>,
    /// [`NONE`] for the root and for nodes that are not attached to anything.
    pub parent: usize,
}

/// "No node".
pub const NONE: usize = usize::MAX;

/// `nodes[0]` is the root (an element called `#root`).
#[derive(Clone, Debug)]
pub struct Tree {
    pub nodes: Vec<Node>,
    /// The page was bigger than the limits and has been cut.
    pub truncated: bool,
}

impl Tree {
    pub fn tag(&self, i: usize) -> Option<&str> {
        match &self.nodes[i].kind {
            NodeKind::Element { tag, .. } => Some(tag),
            NodeKind::Text(_) => None,
        }
    }

    pub fn attr(&self, i: usize, name: &str) -> Option<&str> {
        match &self.nodes[i].kind {
            NodeKind::Element { attrs, .. } => attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str()),
            NodeKind::Text(_) => None,
        }
    }

    /// All the text under a node, joined (for titles and attribute-free summaries).
    pub fn text_of(&self, i: usize) -> String {
        let mut out = String::new();
        let mut todo = vec![i];
        while let Some(n) = todo.pop() {
            match &self.nodes[n].kind {
                NodeKind::Text(t) => out.push_str(t),
                NodeKind::Element { .. } => todo.extend(self.nodes[n].children.iter().rev()),
            }
        }
        out
    }

    /// The first element with this tag, in document order.
    pub fn find(&self, tag: &str) -> Option<usize> {
        let mut todo = vec![0usize];
        while let Some(n) = todo.pop() {
            if self.tag(n) == Some(tag) {
                return Some(n);
            }
            todo.extend(self.nodes[n].children.iter().rev());
        }
        None
    }
}

const MAX_NODES: usize = 200_000;
const MAX_DEPTH: usize = 200;

const VOID: [&str; 15] = ["br", "hr", "img", "input", "meta", "link", "area", "base", "col", "embed", "source", "track", "wbr", "param", "basefont"];
const BLOCKS_CLOSING_P: [&str; 27] = [
    "p", "div", "ul", "ol", "dl", "table", "h1", "h2", "h3", "h4", "h5", "h6", "pre", "blockquote", "hr", "form", "section", "article", "header", "footer", "nav", "aside", "main", "figure", "address", "fieldset", "li",
];

pub fn parse(input: &str) -> Tree {
    // a page with no doctype is laid out the old, forgiving way ("quirks"): the root carries the mark
    let head: String = input.chars().take(1024).collect::<String>().to_ascii_lowercase();
    let root_attrs = if head.contains("<!doctype") { vec![] } else { vec![("data-iyakki-quirks".to_string(), "1".to_string())] };
    let mut t = Tree { nodes: vec![Node { kind: NodeKind::Element { tag: "#root".into(), attrs: root_attrs }, children: vec![], parent: NONE }], truncated: false };
    let b = input.as_bytes();
    let mut stack: Vec<usize> = vec![0];
    let mut i = 0;
    let mut text_start = 0;
    while i < b.len() {
        if t.nodes.len() >= MAX_NODES {
            t.truncated = true;
            break;
        }
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let next = b.get(i + 1).copied().unwrap_or(0);
        let is_tag = next.is_ascii_alphabetic() || next == b'/' && b.get(i + 2).is_some_and(|c| c.is_ascii_alphabetic()) || next == b'!' || next == b'?';
        if !is_tag {
            i += 1;
            continue;
        }
        push_text(&mut t, &stack, &input[text_start..i]);
        if input[i..].starts_with("<!--") {
            i = input[i + 4..].find("-->").map_or(b.len(), |e| i + 4 + e + 3);
        } else if next == b'!' || next == b'?' {
            i = input[i..].find('>').map_or(b.len(), |e| i + e + 1);
        } else if next == b'/' {
            let end = input[i..].find('>').map_or(b.len(), |e| i + e + 1);
            let name: String = input[i + 2..end.saturating_sub(1).max(i + 2)].trim().split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("").to_ascii_lowercase();
            close(&t, &mut stack, &name);
            i = end;
        } else {
            let (name, attrs, self_closing, end) = read_start_tag(input, i);
            i = end;
            open(&mut t, &mut stack, &name, attrs, self_closing);
            // elements whose content is not markup
            if matches!(name.as_str(), "script" | "style") {
                // the text is kept as it is written (no entities), so a script can be run later
                let close_tag = format!("</{name}");
                let lower = input[i..].to_ascii_lowercase();
                let stop = lower.find(&close_tag).map_or(b.len(), |e| i + e);
                push_raw_text(&mut t, &stack, &input[i..stop]);
                i = stop;
                close(&t, &mut stack, &name);
            } else if matches!(name.as_str(), "title" | "textarea") {
                let close_tag = format!("</{name}");
                let lower = input[i..].to_ascii_lowercase();
                let stop = lower.find(&close_tag).map_or(b.len(), |e| i + e);
                push_text(&mut t, &stack, &input[i..stop]);
                i = stop;
            }
        }
        text_start = i;
    }
    if text_start < b.len() && !t.truncated {
        push_text(&mut t, &stack, &input[text_start..]);
    }
    t
}

fn push_raw_text(t: &mut Tree, stack: &[usize], text: &str) {
    if text.is_empty() || t.nodes.len() >= MAX_NODES {
        return;
    }
    let parent = *stack.last().unwrap_or(&0);
    let id = t.nodes.len();
    t.nodes.push(Node { kind: NodeKind::Text(text.to_string()), children: vec![], parent });
    t.nodes[parent].children.push(id);
}

fn push_text(t: &mut Tree, stack: &[usize], raw: &str) {
    if raw.is_empty() {
        return;
    }
    let text = decode_entities(raw);
    let parent = *stack.last().unwrap_or(&0);
    if let Some(&last) = t.nodes[parent].children.last() {
        if let NodeKind::Text(prev) = &mut t.nodes[last].kind {
            prev.push_str(&text);
            return;
        }
    }
    let id = t.nodes.len();
    t.nodes.push(Node { kind: NodeKind::Text(text), children: vec![], parent });
    t.nodes[parent].children.push(id);
}

/// Name, attributes, whether it ends in `/>`, and where the tag ends (just after `>`).
fn read_start_tag(s: &str, at: usize) -> (String, Vec<(String, String)>, bool, usize) {
    let b = s.as_bytes();
    let mut i = at + 1;
    let name_start = i;
    while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' && b[i] != b'/' {
        i += 1;
    }
    let name = s[name_start..i].to_ascii_lowercase();
    let mut attrs = Vec::new();
    let mut self_closing = false;
    while i < b.len() {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            if b[i] == b'/' && b.get(i + 1) == Some(&b'>') {
                self_closing = true;
            }
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        if b[i] == b'>' {
            i += 1;
            break;
        }
        let ks = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'=' && b[i] != b'>' && b[i] != b'/' {
            i += 1;
        }
        let key = s[ks..i].to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let q = b[i];
                i += 1;
                let vs = i;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                value = decode_entities(&s[vs..i]);
                i = (i + 1).min(b.len());
            } else {
                let vs = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                    i += 1;
                }
                value = decode_entities(&s[vs..i]);
            }
        }
        if !key.is_empty() && attrs.len() < 64 {
            attrs.push((key, value));
        }
    }
    (name, attrs, self_closing, i)
}

fn nearest(t: &Tree, stack: &[usize], tags: &[&str], stop_at: &[&str]) -> Option<usize> {
    for (pos, &n) in stack.iter().enumerate().rev() {
        let tag = t.tag(n).unwrap_or("");
        if tags.contains(&tag) {
            return Some(pos);
        }
        if stop_at.contains(&tag) {
            return None;
        }
    }
    None
}

fn open(t: &mut Tree, stack: &mut Vec<usize>, name: &str, attrs: Vec<(String, String)>, self_closing: bool) {
    // implied end tags
    if BLOCKS_CLOSING_P.contains(&name) {
        if let Some(p) = nearest(t, stack, &["p"], &["td", "th", "li", "div", "body", "blockquote", "table"]) {
            stack.truncate(p);
        }
    }
    match name {
        "li" => {
            if let Some(p) = nearest(t, stack, &["li"], &["ul", "ol", "dl", "table"]) {
                stack.truncate(p);
            }
        }
        "dt" | "dd" => {
            if let Some(p) = nearest(t, stack, &["dt", "dd"], &["dl", "table"]) {
                stack.truncate(p);
            }
        }
        "option" => {
            if let Some(p) = nearest(t, stack, &["option"], &["select", "optgroup", "datalist"]) {
                stack.truncate(p);
            }
        }
        "optgroup" => {
            if let Some(p) = nearest(t, stack, &["option", "optgroup"], &["select"]) {
                stack.truncate(p);
            }
        }
        "tr" => {
            if let Some(p) = nearest(t, stack, &["tr"], &["table"]) {
                stack.truncate(p);
            }
        }
        "td" | "th" => {
            if let Some(p) = nearest(t, stack, &["td", "th"], &["tr", "table"]) {
                stack.truncate(p);
            }
        }
        "thead" | "tbody" | "tfoot" => {
            if let Some(p) = nearest(t, stack, &["thead", "tbody", "tfoot"], &["table"]) {
                stack.truncate(p);
            }
        }
        _ => {}
    }
    let parent = *stack.last().unwrap_or(&0);
    let id = t.nodes.len();
    t.nodes.push(Node { kind: NodeKind::Element { tag: name.to_string(), attrs }, children: vec![], parent });
    t.nodes[parent].children.push(id);
    if !VOID.contains(&name) && !self_closing && stack.len() < MAX_DEPTH {
        stack.push(id);
    }
}

fn close(t: &Tree, stack: &mut Vec<usize>, name: &str) {
    if let Some(p) = stack.iter().rposition(|&n| t.tag(n) == Some(name)) {
        if p > 0 {
            stack.truncate(p);
        }
    }
}

/// Replace `&name;`, `&#123;` and `&#x1F;` with the characters they stand for. Unknown names are left as written.
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest.char_indices().take(12).find(|(_, c)| *c == ';').map(|(e, _)| e);
        let decoded = end.and_then(|e| {
            let body = &rest[1..e];
            let ch = if let Some(num) = body.strip_prefix('#') {
                let n = if let Some(h) = num.strip_prefix(['x', 'X']) { u32::from_str_radix(h, 16).ok() } else { num.parse::<u32>().ok() };
                n.and_then(char::from_u32).filter(|c| *c != '\0')
            } else {
                named(body)
            };
            ch.map(|c| (c, e))
        });
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn named(n: &str) -> Option<char> {
    Some(match n {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "laquo" => '«',
        "raquo" => '»',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "bull" => '•',
        "middot" => '·',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        "sect" => '§',
        "deg" => '°',
        "plusmn" => '±',
        "times" => '×',
        "divide" => '÷',
        "larr" => '←',
        "rarr" => '→',
        "uarr" => '↑',
        "darr" => '↓',
        "para" => '¶',
        "micro" => 'µ',
        "frac12" => '½',
        "frac14" => '¼',
        "iexcl" => '¡',
        "iquest" => '¿',
        "eacute" => 'é',
        "egrave" => 'è',
        "aacute" => 'á',
        "agrave" => 'à',
        "uuml" => 'ü',
        "ouml" => 'ö',
        "auml" => 'ä',
        "szlig" => 'ß',
        "ntilde" => 'ñ',
        "ccedil" => 'ç',
        "check" => '✓',
        "hearts" => '♥',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(t: &Tree, n: usize, out: &mut String) {
        match &t.nodes[n].kind {
            NodeKind::Text(s) => out.push_str(&format!("{s:?}")),
            NodeKind::Element { tag, .. } => {
                out.push_str(tag);
                if !t.nodes[n].children.is_empty() {
                    out.push('(');
                    for (i, c) in t.nodes[n].children.iter().enumerate() {
                        if i > 0 {
                            out.push(' ');
                        }
                        shape(t, *c, out);
                    }
                    out.push(')');
                }
            }
        }
    }

    fn s(html: &str) -> String {
        let t = parse(html);
        let mut o = String::new();
        shape(&t, 0, &mut o);
        o
    }

    #[test]
    fn nesting_attributes_and_void_elements() {
        assert_eq!(s("<p>Hello <b>big</b> world<br>next<img src=x></p>"), "#root(p(\"Hello \" b(\"big\") \" world\" br \"next\" img))");
        let t = parse(r#"<a HREF="/x?a=1&amp;b=2" class=big data-x='y z' hidden>t</a>"#);
        let a = t.find("a").unwrap();
        assert_eq!(t.attr(a, "href"), Some("/x?a=1&b=2"));
        assert_eq!(t.attr(a, "class"), Some("big"));
        assert_eq!(t.attr(a, "data-x"), Some("y z"));
        assert_eq!(t.attr(a, "hidden"), Some(""));
    }

    #[test]
    fn implied_end_tags_for_paragraphs_lists_and_tables() {
        assert_eq!(s("<p>one<p>two<div>three</div>"), "#root(p(\"one\") p(\"two\") div(\"three\"))");
        assert_eq!(s("<ul><li>a<li>b<ul><li>c</ul><li>d</ul>"), "#root(ul(li(\"a\") li(\"b\" ul(li(\"c\"))) li(\"d\")))");
        assert_eq!(s("<table><tr><td>1<td>2<tr><td>3</table>"), "#root(table(tr(td(\"1\") td(\"2\")) tr(td(\"3\"))))");
        assert_eq!(s("<dl><dt>a<dd>b<dt>c<dd>d</dl>"), "#root(dl(dt(\"a\") dd(\"b\") dt(\"c\") dd(\"d\")))");
    }

    #[test]
    fn scripts_styles_and_comments_are_dropped_and_title_text_is_kept() {
        assert_eq!(s("a<script>if (1 < 2) { x('</p>') }</script>b<style>p { }</style>c<!-- <b>no</b> -->d"), "#root(\"a\" script(\"if (1 < 2) { x('</p>') }\") \"b\" style(\"p { }\") \"cd\")");
        let t = parse("<title>A &amp; B <b>not markup</b></title><p>x");
        assert_eq!(t.text_of(t.find("title").unwrap()), "A & B <b>not markup</b>");
    }

    #[test]
    fn stray_and_unclosed_markup_never_fails() {
        assert_eq!(s("a < b and c > d"), "#root(\"a < b and c > d\")");
        assert_eq!(s("</div>x</p></span>y"), "#root(\"xy\")");
        assert_eq!(s("<b><i>x</b>y</i>"), "#root(b(i(\"x\")) \"y\")");
        let _ = parse("<div class=\"unterminated");
        let _ = parse("<");
        let _ = parse("<a href=\"x");
        let _ = parse("<!doctype html><?xml version='1.0'?><![CDATA[x]]>");
        let _ = parse("<p/><br/><br /><div/>text");
        assert_eq!(parse("").nodes.len(), 1);
    }

    #[test]
    fn options_close_each_other() {
        assert_eq!(s("<select><option>a<option>b<optgroup label=g><option>c<option>d</select>"), "#root(select(option(\"a\") option(\"b\") optgroup(option(\"c\") option(\"d\"))))");
    }

    #[test]
    fn depth_and_size_are_capped() {
        let deep = "<div>".repeat(5000) + "x";
        let t = parse(&deep);
        assert!(t.nodes.len() <= 5002);
        // the text is still there, attached somewhere
        assert!((0..t.nodes.len()).any(|i| matches!(&t.nodes[i].kind, NodeKind::Text(x) if x == "x")));
        let wide = "<i>a</i>".repeat(300_000);
        let t = parse(&wide);
        assert!(t.truncated && t.nodes.len() <= MAX_NODES + 2, "{}", t.nodes.len());
    }

    #[test]
    fn entities() {
        assert_eq!(decode_entities("a &amp; b &lt;c&gt; &quot;d&quot; &#65;&#x42; &copy; &nbsp;|"), "a & b <c> \"d\" AB © \u{a0}|");
        assert_eq!(decode_entities("&unknown; &amp &#xZZ; &#0; &#1114112; AT&T"), "&unknown; &amp &#xZZ; &#0; &#1114112; AT&T");
        assert_eq!(decode_entities("no entities"), "no entities");
        assert_eq!(decode_entities("&"), "&");
    }

    #[test]
    fn multibyte_text_survives_tag_scanning() {
        assert_eq!(s("<p>héllo — 日本語 <b>ü</b></p>"), "#root(p(\"héllo — 日本語 \" b(\"ü\")))");
        let _ = parse("<p title=\"é\">é<");
    }
}
