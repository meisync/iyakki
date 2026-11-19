//! What an external coding agent is told about IYAKKI itself: an "IYAKKI app", "tile" or "app for IYAKKI" is a tile app (plain HTML,
//! CSS and JavaScript with an `app.json`) built and published with IYAKKI's own tools, NOT a Python or Docker web service on the
//! cluster. The person's message is looked at before the run (the same words the built-in assistant routes on,
//! [`crate::target::resolve_intent`]) and the matching authoring skills are put in front of the agent.

use crate::store::builtin::SKILLS;
use crate::target::{resolve_intent, Intent, Target};

/// At most this much of one skill is added.
const SKILL_CAP: usize = 6000;

/// The skills to put in front of the agent for this request (names from [`SKILLS`]); empty for anything else.
pub fn skills_for(prompt: &str) -> Vec<&'static str> {
    let p = prompt.to_lowercase();
    let store_words = ["private store", "publish to the store", "publish it to the store", "to the store", "store app", "publish to store"].iter().any(|w| p.contains(w));
    match resolve_intent(prompt) {
        Intent::Is(Target::IyakkiTile) => vec!["iyakki-app-author", "iyakki-tile-author"],
        Intent::Is(Target::McpServer) if p.contains("iyakki") || store_words => vec!["iyakki-app-author", "iyakki-mcp-author"],
        Intent::Is(Target::Skill) if p.contains("iyakki") || store_words => vec!["iyakki-app-author", "iyakki-skill-author"],
        _ if store_words => vec!["iyakki-app-author", "iyakki-tile-author"],
        _ => vec![],
    }
}

fn body(markdown: &str) -> &str {
    let t = markdown.trim_start();
    match t.strip_prefix("---").and_then(|r| r.find("\n---").map(|i| &r[i + 4..])) {
        Some(b) => b.trim_start(),
        None => t,
    }
}

/// The text that goes in front of the agent's rules for this request, or nothing.
pub fn for_prompt(prompt: &str) -> String {
    let names = skills_for(prompt);
    if names.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n\nTHE PERSON ASKED FOR AN IYAKKI APP. Here \"an IYAKKI app\", \"a tile\" or \"an app for IYAKKI\" means a tile app for IYAKKI's launcher: a web bundle (app.json plus index.html, app.js and style.css) that runs in IYAKKI's own page engine. Tiles ARE web pages: use plain HTML, CSS and JavaScript only. Do NOT use Rust, Python, Node servers, Docker, a Dockerfile or k3s for a tile (Rust is for IYAKKI itself and for wasm tools), and do not make deploy rows, unless the person explicitly asks for a cluster deployment. Do exactly this: 1. tile_scaffold {name} and edit the files it made; 2. tile_test and tile_validate until clean; 3. tile_ship {path}: ONE call that checks, packs, signs with the person's own key (made automatically) and publishes to their private store, showing the person a single approval card. Never ask the person for keys, accounts, tokens or sign-in: none are needed, IYAKKI keeps them. If a tool returns a real blocker (for example no private store deployed yet), say it in one sentence with the person's next step, using the tool's own words, and stop. The authoring skills follow.\n");
    for n in names {
        if let Some((_, text)) = SKILLS.iter().find(|(k, _)| *k == n) {
            let b = body(text);
            let cut = b.char_indices().nth(SKILL_CAP).map_or(b.len(), |(i, _)| i);
            s.push_str(&format!("\n--- skill {n} ---\n{}\n", &b[..cut]));
        }
    }
    s
}
