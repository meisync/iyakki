//! The defaults of the alpha.6 split-b facets: a core without them answers "nothing / unavailable".

use std::path::Path;

use iyakki_agent_api::{AgentApi, AgentFacetsS6B, FakeAgentApi, HostProbeFacet, NoS6B, SoundProbeFacet};

#[test]
fn the_fake_answers_nothing() {
    let f = FakeAgentApi::default();
    let s: &dyn SoundProbeFacet = f.sound_probe();
    assert_eq!(s.voice_peak(&[1, -5, 3]), 0);
    assert!(s.audio_outputs_at(Path::new("/proc")).is_empty());
    assert!(!s.audio_mixing());
    let h: &dyn HostProbeFacet = &NoS6B;
    assert!(!h.webengine_offered());
    assert!(h.webengine_embedded_helper().is_none());
    assert!(!h.sharing_notice_is_on(Path::new("/nonexistent")));
    let _api: &dyn AgentApi = &f;
}
