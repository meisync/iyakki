//! The model chat tools with a fake backend: reads are free, the two others ask every time and cannot be remembered; no key anywhere.

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::{Risk, Tools};
use crate::modelopts::ModelOptions;
use crate::modelopttools::*;
use crate::providers::Kind;

const NIM: &str = "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning";

#[derive(Default)]
struct Fake {
    saved: Mutex<Vec<(String, String, ModelOptions)>>,
    fetched: Mutex<Vec<String>>,
    many: bool,
}
impl Backend for Fake {
    fn providers(&self) -> Vec<ProviderInfo> {
        vec![
            ProviderInfo { id: "nvidia".into(), kind: Kind::Nvidia, host: "integrate.api.nvidia.com".into(), enabled: true, key_held: true, chat: Some(NIM.into()), vision: None, embedding: None },
            ProviderInfo { id: "openai".into(), kind: Kind::OpenAi, host: "api.openai.com".into(), enabled: false, key_held: false, chat: None, vision: None, embedding: None },
        ]
    }
    fn options(&self, p: &str, m: &str) -> ModelOptions {
        self.saved.lock().unwrap().iter().rev().find(|(a, b, _)| a == p && b == m).map(|x| x.2.clone()).unwrap_or(ModelOptions { temperature: Some(0.6), ..Default::default() })
    }
    fn set_options(&self, p: &str, m: &str, o: &ModelOptions) -> Result<(), String> {
        self.saved.lock().unwrap().push((p.into(), m.into(), o.clone()));
        Ok(())
    }
    fn fetch(&self, p: &str) -> Result<Vec<String>, String> {
        self.fetched.lock().unwrap().push(p.into());
        if self.many { Ok((0..500).map(|i| format!("m/model-{i:03}")).collect()) } else { Ok(vec!["a/one".into(), NIM.into()]) }
    }
}

fn rig(many: bool) -> (ModelTools, Arc<Fake>) {
    let f = Arc::new(Fake { many, ..Default::default() });
    (ModelTools::new(Arc::new(crate::assign::tests_support::NoTools), f.clone()), f)
}

#[test]
fn reads_are_free_and_the_two_others_ask_every_time_and_cannot_be_remembered() {
    let (t, _) = rig(false);
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).map(|d| d.risk);
    assert_eq!((risk("models_list"), risk("model_options_get")), (Some(Risk::Safe), Some(Risk::Safe)));
    assert_eq!((risk("model_options_set"), risk("provider_models_fetch")), (Some(Risk::Ask), Some(Risk::Ask)));
    for n in ASK_TOOLS {
        assert!(!t.pre_approved(n) && !t.approved_always(n), "{n}");
        assert!(t.grant_blocker(n, &json!({})).is_some(), "{n}: never covered by a session grant");
        assert!(crate::agent::hard_ask(n, &json!({})).is_some(), "{n}: asks even in Auto mode");
    }
    for n in READ_TOOLS {
        assert!(crate::agent::hard_ask(n, &json!({})).is_none(), "{n}");
    }
}

#[test]
fn models_list_and_get_show_providers_and_options_but_no_key() {
    let (t, _) = rig(false);
    let l = t.call("models_list", &json!({})).unwrap();
    assert!(l.contains("nvidia (NVIDIA NIM") || l.contains("nvidia ("), "{l}");
    assert!(l.contains("integrate.api.nvidia.com") && l.contains("key held in the vault") && l.contains("openai") && l.contains("off") && l.contains("key missing"), "{l}");
    assert!(!l.to_lowercase().contains("nvapi"));
    let g = t.call("model_options_get", &json!({"provider": "nvidia", "model": NIM})).unwrap();
    assert!(g.contains("temperature: 0.6") && g.contains("max_tokens: not set"), "{g}");
    let g = t.call("model_options_get", &json!({"provider": "openai", "model": "gpt-4o"})).unwrap();
    assert!(g.contains("reasoning_budget: not used by this provider"), "{g}");
    assert!(t.call("model_options_get", &json!({"provider": "nope", "model": "x"})).unwrap_err().contains("no provider"));
}

#[test]
fn model_options_set_shows_old_and_new_values_validates_and_saves_only_when_called() {
    let (t, f) = rig(false);
    let a = json!({"provider": "nvidia", "model": NIM, "temperature": 0.2, "max_tokens": 65536, "reasoning_budget": 16384});
    let card = t.preview("model_options_set", &a).unwrap();
    assert!(card.contains("temperature: 0.6 -> 0.2") && card.contains("max_tokens: not set -> 65536") && card.contains("reasoning_budget: not set -> 16384"), "{card}");
    assert!(f.saved.lock().unwrap().is_empty(), "the card changes nothing");
    assert!(t.describe("model_options_set", &a).starts_with("Change the request options of nvidia/"));
    // invalid values are refused before anyone is asked
    assert!(t.refusal("model_options_set", &json!({"provider": "nvidia", "model": NIM, "top_p": 3})).unwrap().contains("top_p"));
    assert!(t.refusal("model_options_set", &json!({"provider": "nvidia", "model": NIM})).unwrap().contains("say which option"));
    assert!(t.refusal("model_options_set", &json!({"provider": "nvidia", "model": NIM, "temperature": 0.6})).unwrap().contains("nothing would change"));
    assert!(t.refusal("model_options_set", &a).is_none());
    let r = t.call("model_options_set", &a).unwrap();
    assert!(r.contains("saved for nvidia/") && r.contains("reasoning_budget: 16384"), "{r}");
    let saved = f.saved.lock().unwrap();
    assert_eq!(saved[0].2, ModelOptions { max_tokens: Some(65536), temperature: Some(0.2), top_p: None, reasoning_budget: Some(16384) });
    drop(saved);
    // a provider that does not take the budget is told so
    let r = t.call("model_options_set", &json!({"provider": "openai", "model": "gpt-4o", "reasoning_budget": 100})).unwrap();
    assert!(r.contains("reasoning_budget: 100 (not used by this provider)"), "{r}");
    // null clears
    t.call("model_options_set", &json!({"provider": "nvidia", "model": NIM, "temperature": null})).unwrap();
    assert_eq!(f.saved.lock().unwrap().last().unwrap().2.temperature, None);
}

#[test]
fn a_key_in_a_call_is_refused_without_echoing_it() {
    let (t, f) = rig(false);
    let key = "nvapi-FAKEFAKEFAKEFAKE0123456789made-up";
    let e = t.refusal("provider_models_fetch", &json!({"provider": key})).unwrap();
    assert!(!e.contains("FAKEFAKE"), "{e}");
    let e = t.call("model_options_set", &json!({"provider": "nvidia", "model": key, "top_p": 0.5})).unwrap_err();
    assert!(e.contains("never send a key") && !e.contains("FAKEFAKE"), "{e}");
    assert!(f.saved.lock().unwrap().is_empty());
}

#[test]
fn provider_models_fetch_names_the_host_and_returns_capped_ids() {
    let (t, f) = rig(false);
    let a = json!({"provider": "nvidia"});
    let card = t.preview("provider_models_fetch", &a).unwrap();
    assert!(card.contains("integrate.api.nvidia.com") && card.contains("not shown"), "{card}");
    assert!(f.fetched.lock().unwrap().is_empty(), "nothing is contacted before the person approves");
    let r = t.call("provider_models_fetch", &a).unwrap();
    assert!(r.contains("offers 2 model(s)") && r.contains(NIM), "{r}");
    let (t, _) = rig(true);
    let r = t.call("provider_models_fetch", &a).unwrap();
    assert!(r.contains("offers 500 model(s)") && r.contains("and 300 more") && !r.contains("model-499"), "capped: {}", r.len());
    assert!(t.refusal("provider_models_fetch", &json!({"provider": "ghost"})).unwrap().contains("no provider"));
}
