//! The coding-agent status cache as a trait object (F-SPLIT-03): a screen holds `Arc<dyn ExtStatusApi>`, the model list and the entries are computed
//! from plain answers, and a lookup that forgets what was found works through the trait.

use std::sync::Arc;

use iyakki_agent_api::ExtStatusApi;
use iyakki_shared::extagent::{entries_with, models_with, Auth, ModelInfo, ModelSource, Status, StatusCache, CLAUDE_CODE};
use iyakki_shared::extagentcfg::AgentsCfg;

fn ready() -> Status {
    Status { installed: true, version: "1".into(), auth: Auth::SignedIn("a".into()), mode: iyakki_shared::extagent::Mode::Bridged, chip: "ready".into(), fix: String::new() }
}

#[test]
fn a_cache_behind_the_trait_remembers_forgets_and_feeds_the_entries_and_the_model_list() {
    let cache: Arc<dyn ExtStatusApi> = Arc::new(Arc::new(StatusCache::default()));
    assert!(cache.get(CLAUDE_CODE).is_none() && !cache.busy());
    let cfg = AgentsCfg::default();
    // nothing found yet: the entry says "checking"
    let e = entries_with(&cfg, &|id| cache.get(id));
    let first = e.iter().find(|e| e.id == CLAUDE_CODE).expect("the built-in agent is listed");
    assert_eq!(first.chip, "checking…");
    cache.set(CLAUDE_CODE, ready());
    let e = entries_with(&cfg, &|id| cache.get(id));
    assert_eq!(e.iter().find(|e| e.id == CLAUDE_CODE).unwrap().chip, "ready");
    // a model list the CLI printed wins over the documented one; the person's chosen model is added when it is in neither
    cache.set_models(CLAUDE_CODE, vec![ModelInfo::new("m-one", "One", "", ModelSource::Cli)]);
    let agent = cfg.agent(CLAUDE_CODE).unwrap();
    let list = models_with(cache.cli_models(CLAUDE_CODE), agent.as_ref());
    assert_eq!(list.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["m-one"]);
    assert_eq!(models_with(None, agent.as_ref()).len(), agent.builtin_models().len());
    // signing in or out forgets what was found
    cache.invalidate();
    assert!(cache.get(CLAUDE_CODE).is_none() && cache.cli_models(CLAUDE_CODE).is_some(), "the status goes, the printed model list stays for its day");
}
