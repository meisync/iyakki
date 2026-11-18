//! The switch's chat tools with a fake backend, the rule that reads the switch, and the real backend over a settings file.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::pdfnativetools::*;

struct Empty;
impl Tools for Empty {
    fn defs(&self) -> Vec<ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _: &Value) -> Result<String, String> {
        Err(format!("unknown {name}"))
    }
    fn describe(&self, name: &str, _: &Value) -> String {
        name.into()
    }
}

struct Fake(Mutex<Vec<Entry>>);
impl Backend for Fake {
    fn entries(&self) -> Vec<Entry> {
        self.0.lock().unwrap().clone()
    }
    fn set(&self, target: &str, native: bool) -> Result<(), String> {
        self.0.lock().unwrap().iter_mut().find(|e| e.target == target).ok_or("no such target")?.native = native;
        Ok(())
    }
}

fn rig() -> (PdfOutputTools, Arc<Fake>) {
    let f = Arc::new(Fake(Mutex::new(vec![
        Entry { target: "openai".into(), label: "OpenAI".into(), agent: false, native: false },
        Entry { target: "agent:claude".into(), label: "Claude Code".into(), agent: true, native: false },
    ])));
    (PdfOutputTools::new(Arc::new(Empty), f.clone()), f)
}

#[test]
fn the_status_is_free_and_the_change_asks_every_time_with_a_card_and_cannot_be_remembered() {
    let (t, f) = rig();
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap().risk;
    assert_eq!((risk("pdf_output_status"), risk("pdf_output_set")), (Risk::Safe, Risk::Ask));
    assert!(defs.iter().all(|d| d.spec.description.len() < 300));
    let s = t.call("pdf_output_status", &json!({})).unwrap();
    assert!(s.contains("openai (OpenAI): pdf_create offered") && s.contains("agent:claude (Claude Code): pdf_create offered"), "{s}");
    let args = json!({"target": "agent:claude", "native": true});
    let card = t.preview("pdf_output_set", &args).unwrap();
    assert!(card.contains("agent:claude") && card.contains("is offered pdf_create now") && card.contains("writes PDF files itself"), "{card}");
    assert!(t.describe("pdf_output_set", &args).contains("agent:claude"));
    assert!(!f.entries()[1].native, "asking changes nothing");
    assert!(t.call("pdf_output_set", &args).unwrap().contains("not offered"));
    assert!(f.entries()[1].native && !f.entries()[0].native);
    assert!(t.call("pdf_output_status", &json!({})).unwrap().contains("agent:claude (Claude Code): writes PDF files itself"));
    t.call("pdf_output_set", &json!({"target": "agent:claude", "native": false})).unwrap();
    assert!(!f.entries()[1].native);
    for bad in [json!({"native": true}), json!({"target": "openai"}), json!({"target": "nope", "native": true}), json!({"target": "openai", "native": "yes"})] {
        assert!(t.refusal("pdf_output_set", &bad).is_some() && t.call("pdf_output_set", &bad).is_err(), "{bad}");
    }
    assert!(is_pdf_output_tool("pdf_output_set") && !is_pdf_output_tool("pdf_create"));
    assert_eq!(crate::toolblock::group_of("pdf_output_set"), crate::toolblock::group_of("pdf_create"), "same group as the PDF tools");
}

#[test]
fn the_rule_reads_the_provider_of_the_chat_model_or_the_chosen_agent() {
    use crate::extagent::AgentsCfg;
    use crate::providers::{Kind, ProviderCfg};
    use crate::settings::{ModelChoice, Settings};
    let mut s = Settings::default();
    let mut p = ProviderCfg::new("local", Kind::Ollama);
    p.models = vec!["m".into()];
    s.providers.push(p);
    s.chat = Some(ModelChoice { provider: "local".into(), model: "m".into() });
    let mut a = AgentsCfg::default();
    assert!(!is_native(&s, &a, None) && !is_native(&s, &a, Some(crate::extagent::CLAUDE_CODE)), "offered by default");
    s.providers[0].native_pdf_output = true;
    assert!(is_native(&s, &a, None), "the chat model's provider says so");
    assert!(!is_native(&s, &a, Some(crate::extagent::CLAUDE_CODE)), "an agent is judged by its own switch, not the provider's");
    a.claude.native_pdf_output = true;
    assert!(is_native(&s, &a, Some(crate::extagent::CLAUDE_CODE)));
    s.chat = None;
    s.providers[0].native_pdf_output = true;
    assert!(!is_native(&s, &a, None), "no chat model, nothing to judge: offered");
}
