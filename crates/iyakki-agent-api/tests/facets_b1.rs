//! The facets of bucket b1 answer "unavailable" by default and the fakes need no agent (F-SPLIT-03).

use std::path::Path;
use std::sync::Arc;

use iyakki_agent_api::{AgentApi, AgentFacetsB1, AiMachine, FakeAgentApi, FakeAiMachine, ServicesB1};
use iyakki_shared::aiprofile::Vis;

#[test]
fn the_fake_agent_has_every_b1_facet_and_they_answer_empty() {
    fn takes(_: &dyn AgentApi) {}
    let f = FakeAgentApi::default();
    takes(&f);
    let m = f.ai_profile().machine(Path::new("/x"));
    assert!(m.unknown() && !m.capable() && m.visibility("ollama", false) == Vis::Show);
    assert!(!f.ai_profile().local_capable());
    assert!(f.av_build().consent("any map").is_err());
    assert!(f.hw_firmware().plan_is_empty(Path::new("/x")));
    assert_eq!(f.hw_firmware().view_now(Path::new("/x"), Path::new("/h")), Default::default());
    assert!(f.hw_firmware().accept(Path::new("/x"), &[], "ann", 1).is_err());
    assert!(f.context_intent().detect("remember to use tabs").is_none());
    assert!(f.matrix_check_homeserver("https://example.org").is_err());
    assert_eq!(f.mic_glyph(), iyakki_shared::voiceassist::MicGlyph::Off);
}

#[test]
fn a_made_up_machine_hides_what_it_is_told_to_and_show_all_brings_it_back() {
    let m = FakeAiMachine { summary: "A test box".into(), vis: vec![("ollama".into(), Vis::Hidden("no GPU".into())), ("elasticsearch".into(), Vis::Recommended("fine".into()))], ..Default::default() };
    assert_eq!(m.visibility("ollama", false), Vis::Hidden("no GPU".into()));
    assert_eq!(m.visibility("ollama", true), Vis::NotRecommended("no GPU".into()));
    assert_eq!(m.visibility("python", false), Vis::Show);
    assert_eq!(m.hidden_items(), vec![("ollama".to_string(), "no GPU".to_string())]);
    let a: Arc<dyn AiMachine> = Arc::new(m.clone());
    assert!(a == (Arc::new(m) as Arc<dyn AiMachine>), "same facts, same machine");
}

#[test]
fn a_core_without_header_rules_keeps_none_and_refuses_changes() {
    let f = FakeAgentApi::default();
    struct V;
    impl iyakki_agent_api::HeaderValues for V {
        fn get(&self, _: &str) -> Result<Option<String>, String> {
            Ok(None)
        }
        fn set(&self, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn delete(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
    }
    let s = f.browser_headers().open(Path::new("/x"), Arc::new(V));
    assert!(s.list().is_empty() && s.activate() == 0);
    assert!(s.delete("a").is_err() && s.set_enabled("a", true).is_err());
    assert!(f.browser_headers().active_here(&iyakki_web::Url::parse("https://example.org/").unwrap()).is_empty());
}

#[test]
fn the_scene_facet_reads_a_host_and_a_core_without_scenes_fetches_nothing() {
    let f = FakeAgentApi::default();
    assert_eq!(f.scene().host_of("https://pics.example.org/a/b.png?x=1"), "pics.example.org");
    assert!(f.scene().asks_for_link("hello", 1).is_none());
}

#[test]
fn a_kube_feed_of_a_core_without_a_cluster_is_empty() {
    let f = FakeAgentApi::default();
    let k = Arc::new(iyakki_shared::k8s::FakeKube::new());
    let feed = f.kube_feed().start(k, Default::default(), std::time::Duration::from_secs(1), std::time::Duration::from_secs(1));
    feed.set_visible(true);
    assert!(feed.latest().is_none() && feed.logs().is_none() && feed.detail().is_none() && feed.fetches() == 0);
}

#[test]
fn the_context_undo_compares_by_identity() {
    let a = iyakki_agent_api::ContextUndo(Arc::new(5u8));
    assert_eq!(a, a.clone());
    assert_ne!(a, iyakki_agent_api::ContextUndo(Arc::new(5u8)));
}

#[test]
fn services_facet_defaults_are_unavailable() {
    struct S;
    impl ServicesB1 for S {}
    let s = Arc::new(S);
    assert!(!s.msg_mirror_tick("ann", Arc::new(|_, _| {})));
    assert!(s.matrix("ann", Arc::new(|_, _| {})).accounts().is_empty());
    assert!(s.matrix("ann", Arc::new(|_, _| {})).add("a", "b", iyakki_agent_api::MatrixAuth::Token("t".into()), "").is_err());
    assert!(s.system_speaker().say("hi").is_err());
    assert!(s.msg_prompt_rules("ann").is_empty());
}

#[test]
fn the_second_batch_of_fakes_answers_empty() {
    use iyakki_agent_api::{BrowserMediaHandle, HostEnginesFacet, SshHost};
    let f = FakeAgentApi::default();
    assert!(f.host_engines().direct("espeak-ng", None, None).is_none());
    assert!(f.host_engines().test_speak("espeak-ng", None, None, "en", 1.0, "aplay").is_err());
    let r = f.story().new_reader();
    assert!(!r.is_active() && r.status() == iyakki_shared::story::Status::Idle && r.sounds().0.is_empty());
    let h = f.sound_hold_registry();
    assert!(h.holds("radio").is_none() && h.conflict_for_new_player("radio", false).is_none());
    assert!(f.browser_media_for_user("ann").is_none());
    struct M;
    impl BrowserMediaHandle for M {}
    assert!(!M.stop_capture() && M.indicator_text().is_none() && M.request(iyakki_shared::browsermedia::Kind::Camera) == iyakki_shared::browsermedia::Requested::Denied);
    struct S;
    impl SshHost for S {}
    assert!(S.connections().is_empty() && S.probe("h", 22).is_err() && !S.has_client());
    struct Sv;
    impl ServicesB1 for Sv {}
    let sv = Arc::new(Sv);
    assert!(sv.wake_spawn(None).is_err() && sv.wake_set_opt_in(true).is_err());
    assert!(sv.ssh_host("ann").trusted_keys("h", 22).is_empty());
    f.wake_request_session();
}
