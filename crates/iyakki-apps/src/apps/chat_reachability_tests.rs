//! The gate of docs/development/CHAT-REACHABILITY.md: every Settings page has at least one assistant tool that exists in the source, or a
//! written exemption with a reason. A new page cannot ship without one of the two; an exempt page that gets a tool leaves the list.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::apps::settings::Tab;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn table() -> BTreeMap<String, Vec<String>> {
    let text = std::fs::read_to_string(root().join("docs/development/CHAT-REACHABILITY.md")).expect("the reachability table");
    let mut rows = BTreeMap::new();
    for line in text.lines().filter(|l| l.starts_with("| `")) {
        let cols: Vec<&str> = line.split('|').map(str::trim).collect();
        let tab = cols[1].trim_matches('`').to_string();
        let tools: Vec<String> = if cols[3].contains("MISSING") || cols[3].starts_with("none") { Vec::new() } else { cols[3].split('`').skip(1).step_by(2).map(str::to_string).collect() };
        assert!(rows.insert(tab.clone(), tools).is_none(), "{tab} appears twice");
    }
    rows
}

fn exempt() -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(root().join("docs/development/chat-reachability-exempt.txt")).expect("the exemption list");
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let (tab, why) = l.split_once('|').unwrap_or_else(|| panic!("`{l}` needs `Tab | reason`"));
            (tab.trim().to_string(), why.trim().to_string())
        })
        .collect()
}

fn sources(dir: &Path, out: &mut String) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            sources(&p, out);
        } else if name.ends_with(".rs") && !name.contains("_tests") && name != "tests.rs" {
            out.push_str(&std::fs::read_to_string(&p).unwrap_or_default());
        }
    }
}

#[test]
fn chat_reachability_gate() {
    let rows = table();
    let exempt = exempt();
    let tabs: BTreeSet<String> = Tab::ORDER.iter().map(|t| format!("{t:?}")).collect();
    let mut code = String::new();
    sources(&root().join("crates/iyakki-agent/src"), &mut code);
    sources(&root().join("crates/iyakki-shared/src"), &mut code);
    sources(&root().join("crates/iyakki-shell/src"), &mut code);
    sources(&root().join("crates/iyakki-apps/src"), &mut code);

    for t in &tabs {
        assert!(rows.contains_key(t), "Settings page {t} has no row in docs/development/CHAT-REACHABILITY.md: add its chat tools, or exempt it with a reason in chat-reachability-exempt.txt");
    }
    for t in rows.keys() {
        assert!(tabs.contains(t), "the table names {t}, which is not a Settings page");
    }
    for (tab, tools) in &rows {
        if tools.is_empty() {
            assert!(exempt.contains_key(tab), "{tab} has no chat tool and no written exemption");
        } else {
            assert!(!exempt.contains_key(tab), "{tab} has chat tools now: remove it from chat-reachability-exempt.txt");
            for n in tools {
                assert!(code.contains(&format!("\"{n}\"")), "{tab}: the tool {n} is not in the source");
            }
        }
    }
    for (tab, why) in &exempt {
        assert!(tabs.contains(tab) || tab == "SignOut", "exempt {tab} is not a Settings page");
        assert!(why.starts_with("permanent:") || why.starts_with("todo:"), "{tab}: the reason starts with permanent: or todo:");
    }
}

#[test]
fn every_tool_of_the_settings_frame_is_in_the_table_and_the_prompt() {
    let rows = table();
    let listed: BTreeSet<&String> = rows.values().flatten().collect();
    let groups: [&[&str]; 18] = [&iyakki_agent::settingstools_startup::TOOLS, &iyakki_agent::settingstools_gemini::TOOLS, &iyakki_agent::settingstools_geminivoice::TOOLS, &iyakki_agent::settingstools_wasmtiles::TOOLS, &iyakki_agent::settingstools_sound::TOOLS, &iyakki_agent::settingstools_memory::TOOLS, &iyakki_agent::settingstools_views::TOOLS, &iyakki_agent::settingstools_backup::TOOLS, &iyakki_agent::settingstools_notifs::TOOLS, &iyakki_agent::settingstools_saver::TOOLS, &iyakki_agent::settingstools_access::TOOLS, &iyakki_agent::settingstools_machine::TOOLS, &iyakki_agent::settingstools_podtls::TOOLS, &crate::apps::chat_settings::TOOLS, &crate::apps::chat_browser_data::TOOLS, &iyakki_agent::settingstools_net::TOOLS, &iyakki_agent::settingstools_wifi::TOOLS, &[]];
    let prompt = crate::apps::chat_settings::prompt_rules(true);
    for n in groups.iter().flat_map(|g| g.iter()) {
        assert!(listed.contains(&n.to_string()), "{n} is missing from the reachability table");
        assert!(prompt.contains(n), "{n} is not explained in the system prompt");
    }
}

#[test]
fn the_linux_apps_tools_are_in_the_prompt_the_doc_and_the_chat_wiring() {
    // Linux apps is not a Settings page (it lives in Store, Setup), so its tools are listed in the "Not on a page" part of the doc
    let doc = std::fs::read_to_string(root().join("docs/development/CHAT-REACHABILITY.md")).unwrap();
    let prompt = crate::apps::chat_settings::prompt_rules(true);
    let wiring = std::fs::read_to_string(root().join("crates/iyakki-agent/src/facets_b4.rs")).unwrap();
    assert!(wiring.contains("settingstools_linux::entries"), "the Linux apps tools are not wired into the chat");
    for n in iyakki_agent::settingstools_linux::TOOLS {
        assert!(doc.contains(&format!("`{n}`")), "{n} is not in docs/development/CHAT-REACHABILITY.md");
        assert!(prompt.contains(n), "{n} is not explained in the system prompt");
    }
}
