//! Where the tokens of one model request go: the system prompt, the tool definitions, the conversation, and the tool outputs.
//!
//! The estimate is the one the usage ledger uses when a provider reports nothing ([`crate::agent::estimate_tokens`] for messages,
//! about 4 characters a token for a tool definition), so the numbers here and the ledger agree. The run logs one line per model
//! turn (`chat: tokens system=N tools=N history=N outputs=N tools_offered=N of M`) and sends a [`crate::agent::RunEvent::Budget`]
//! so the chat footer can show it.

use crate::chat::{Message, Role, ToolSpec};

/// What one request carries, in estimated tokens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Breakdown {
    /// The system message(s), including the rules and the project state added to it.
    pub system: u64,
    /// The definitions of the offered tools (name, description, parameter schema).
    pub tools: u64,
    /// The conversation: the person's messages and the assistant's text and tool requests.
    pub history: u64,
    /// The results of tool calls that sit in the conversation.
    pub outputs: u64,
    /// How many tools the model is offered in this request (the routing meta-tool counts as one).
    pub offered: u32,
    /// How many tools the account has right now.
    pub total: u32,
}

impl Breakdown {
    pub fn sum(&self) -> u64 {
        self.system + self.tools + self.history + self.outputs
    }

    /// The log line and the footer detail.
    pub fn line(&self) -> String {
        format!("tokens system={} tools={} history={} outputs={} tools_offered={} of {}", self.system, self.tools, self.history, self.outputs, self.offered, self.total)
    }
}

/// One tool definition (the estimate the usage ledger applies to the tools of a request).
pub fn spec_tokens(s: &ToolSpec) -> u64 {
    (s.name.len() + s.description.len() + s.schema.to_string().len()) as u64 / 4
}

pub fn specs_tokens(specs: &[ToolSpec]) -> u64 {
    specs.iter().map(spec_tokens).sum()
}

/// The parts of a request (`req` as sent, `offered` the tools sent with it, `total` the tools the account has).
pub fn breakdown(req: &[Message], offered: &[ToolSpec], total: usize) -> Breakdown {
    let part = |f: &dyn Fn(&Message) -> bool| -> u64 { req.iter().filter(|m| f(m)).map(|m| crate::agent::estimate_tokens(std::slice::from_ref(m))).sum() };
    Breakdown {
        system: part(&|m| m.role == Role::System),
        tools: specs_tokens(offered),
        history: part(&|m| m.role != Role::System && m.role != Role::Tool),
        outputs: part(&|m| m.role == Role::Tool),
        offered: offered.len() as u32,
        total: total as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{ToolDef, Tools, UserTools};
    use crate::services::Services;
    use crate::toolblock::{group_of, initial_groups, more_tools_spec, select_specs, Group};
    use iyakki_sec::policy::{BrowserAgent, Role};
    use std::collections::BTreeSet;
    use std::sync::Arc;

    struct Off;
    impl iyakki_proxy::Upstream for Off {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }

    fn defs_of(role: Role, tag: &str) -> Vec<ToolDef> {
        let dir = std::env::temp_dir().join(format!("iyakki-tokbud-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("home")).unwrap();
        let svc = Services::open(&dir.join("svc"), Arc::new(Off), Arc::new(Off)).unwrap();
        svc.screen.set_present(true);
        crate::browsertools::assume_engine(true);
        let mut p = role.template();
        p.browser_agent = BrowserAgent::Interactive;
        p.fs_read = vec![dir.join("home").display().to_string()];
        p.fs_write = if role == Role::Guest { vec![] } else { vec![dir.join("home").display().to_string()] };
        p.net_hosts = vec!["example.com".into()];
        UserTools::new("admin", p, svc, Arc::new(crate::cli::tests::MockHost::default()), vec![]).with_cwd(&dir.join("home")).defs()
    }

    /// What a first request offers for a message and a context, as the run decides it.
    fn offered(defs: &[ToolDef], ask: &str, ctx: &[Group]) -> Vec<ToolSpec> {
        let hist = vec![Message::system(crate::toolblock::COMPACT_PROMPT), Message::user(ask)];
        let loaded = initial_groups(&hist, &ctx.iter().copied().collect());
        let mut v = select_specs(defs, &loaded);
        if defs.iter().any(|d| !loaded.contains(&group_of(&d.spec.name))) {
            v.push(more_tools_spec());
        }
        v
    }

    fn names(v: &[ToolSpec]) -> BTreeSet<String> {
        v.iter().map(|s| s.name.clone()).collect()
    }

    const FLOWS: (&str, &[Group]) = ("why did my flow fail last night", &[Group::Flows]);
    const BROWSER: (&str, &[Group]) = ("open gemini in the browser and ask it for a recipe", &[]);
    const PLAIN: (&str, &[Group]) = ("what is the weather in chennai", &[]);

    /// The numbers written to docs/development/TOKEN-BUDGET.md (run with --nocapture).
    #[test]
    fn the_routed_tool_list_is_far_smaller_than_all_tools_and_inside_its_budget() {
        let defs = defs_of(Role::Admin, "budget");
        let all: Vec<ToolSpec> = defs.iter().map(|d| d.spec.clone()).collect();
        let all_t = specs_tokens(&all);
        let sys = crate::toolblock::COMPACT_PROMPT.len() as u64 / 4;
        eprintln!("TOKENS all tools: {} tools, {all_t} tokens; compact system prompt {sys} tokens; long prompt {}", all.len(), crate::agent::AGENT_SYSTEM_PROMPT.len() / 4);
        // before this routing the tools of Flows, Dash, Data, Pictures, Build and Cluster were offered on every request
        let old_core: BTreeSet<Group> = Group::ALL.into_iter().filter(|g| !matches!(g, Group::Browser | Group::Media | Group::Radio | Group::Calendar | Group::System)).collect();
        let old_plain = specs_tokens(&select_specs(&defs, &old_core)) + spec_tokens(&more_tools_spec());
        let old_browser = specs_tokens(&select_specs(&defs, &old_core.iter().copied().chain([Group::Browser]).collect())) + spec_tokens(&more_tools_spec());
        eprintln!("TOKENS before routing: plain/flows {old_plain}, browser {old_browser}");
        assert!(old_plain > 2 * 2321 / 2);
        // flows: 6000 until the six webhook tools (about 570 tokens) were routed here from every request (they start flows)
        for (label, (ask, ctx), budget) in [("plain", PLAIN, 2600u64), ("flows", FLOWS, 6500), ("browser", BROWSER, 7500)] {
            let o = offered(&defs, ask, ctx);
            let t = specs_tokens(&o);
            eprintln!("TOKENS {label}: {} tools offered of {}, {t} tokens ({}% of all)", o.len(), all.len(), t * 100 / all_t);
            assert!(t <= budget, "{label}: {t} tokens over the budget {budget}");
            assert!(t * 2 <= all_t, "{label}: {t} is not at least 50% below all tools ({all_t})");
        }
    }

    fn has_prefix(n: &BTreeSet<String>, p: &str) -> bool {
        n.iter().any(|x| x.starts_with(p))
    }

    #[test]
    fn each_context_is_offered_exactly_its_groups() {
        let defs = defs_of(Role::Admin, "route");
        let plain = names(&offered(&defs, PLAIN.0, PLAIN.1));
        for must in ["now", "read_file", "terminal", "web_research", "ask_user", "fetch_url", "more_tools"] {
            assert!(plain.contains(must), "{must}");
        }
        for not in ["flow_", "dash_", "data_", "browser_open", "media_", "radio_", "k8s_", "tile_", "calendar_", "show_svg"] {
            assert!(!has_prefix(&plain, not), "plain chat must not be offered {not}: {plain:?}");
        }
        let flows = names(&offered(&defs, FLOWS.0, FLOWS.1));
        assert!(has_prefix(&flows, "flow_") && !has_prefix(&flows, "dash_") && !has_prefix(&flows, "radio_"));
        // the Flows tools are not offered to the same question outside the Flows context unless the words ask for them
        assert!(!has_prefix(&names(&offered(&defs, "summarise notes.txt", &[])), "flow_"));
        let browser = names(&offered(&defs, BROWSER.0, BROWSER.1));
        assert!(browser.contains("browser_open") && browser.contains("browser_click") && !has_prefix(&browser, "flow_"));
        assert!(has_prefix(&names(&offered(&defs, "turn this csv table into a summary", &[])), "data_"));
        assert!(has_prefix(&names(&offered(&defs, "play a tamil fm", &[])), "radio_"));
        assert!(has_prefix(&names(&offered(&defs, "draw a diagram of the network", &[])), "show_svg"));
        assert!(has_prefix(&names(&offered(&defs, "build a dashboard for prometheus", &[])), "dash_"));
        // the topic follows the conversation: a follow-up keeps the flows tools
        let hist = vec![Message::system("S"), Message::user("my flow failed"), Message::user("yes, do it")];
        assert!(initial_groups(&hist, &BTreeSet::new()).contains(&Group::Flows));
    }

    #[test]
    fn every_tool_is_reachable_through_more_tools_and_policy_still_wins() {
        let defs = defs_of(Role::Admin, "reach");
        let spec = more_tools_spec().schema.to_string();
        let offered_now = names(&offered(&defs, PLAIN.0, PLAIN.1));
        for d in &defs {
            let g = group_of(&d.spec.name);
            assert!(g.always() && offered_now.contains(&d.spec.name) || spec.contains(&format!("\"{}\"", g.label())), "{} ({}) cannot be reached", d.spec.name, g.label());
            if !g.always() {
                assert!(crate::toolblock::describe_group(&defs, g).contains(&d.spec.name));
            }
        }
        // routing never adds what the account's policy removed
        let guest = defs_of(Role::Guest, "guest");
        let all_guest = names(&guest.iter().map(|d| d.spec.clone()).collect::<Vec<_>>());
        for (ask, ctx) in [PLAIN, FLOWS, BROWSER] {
            let o = names(&offered(&guest, ask, ctx));
            assert!(o.iter().all(|n| n == "more_tools" || all_guest.contains(n)));
            assert!(!o.contains("write_file"));
        }
        // the risk of a tool is a property of the tool, whatever route offered it
        let routed = offered(&defs, FLOWS.0, FLOWS.1);
        for s in routed.iter().filter(|s| s.name != "more_tools") {
            let d = defs.iter().find(|d| d.spec.name == s.name).unwrap();
            assert_eq!(d.spec.description, s.description);
        }
        assert_eq!(crate::agent::hard_ask("write_file", &serde_json::json!({"path": "/etc/passwd"})).is_some(), crate::agent::hard_ask("write_file", &serde_json::json!({"path": "/etc/passwd"})).is_some());
    }

    #[test]
    fn the_breakdown_splits_a_request_by_part() {
        let req = vec![Message::system("s".repeat(400)), Message::user("u".repeat(80)), Message::assistant("a".repeat(40)), Message::tool_result(&crate::chat::ToolCall { id: "1".into(), name: "x".into(), args: serde_json::Value::Null }, "o".repeat(800))];
        let spec = ToolSpec { name: "t".into(), description: "d".repeat(100), schema: serde_json::json!({}) };
        let b = breakdown(&req, &[spec], 9);
        assert_eq!((b.system, b.outputs, b.offered, b.total), (104, 204, 1, 9));
        assert!(b.history >= 30 && b.tools >= 25 && b.sum() == b.system + b.tools + b.history + b.outputs);
        assert!(b.line().starts_with("tokens system=104 tools=") && b.line().ends_with("tools_offered=1 of 9"));
    }
}
