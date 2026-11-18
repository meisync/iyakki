//! The compact prompt of `iyakki_shared::toolblock` stays much shorter than the full system prompt (the full prompt lives in the agent).

#[test]
fn the_compact_prompt_is_much_shorter_than_the_full_one() {
    use iyakki_shared::toolblock::COMPACT_PROMPT;
    assert!(COMPACT_PROMPT.len() * 2 < crate::agent::AGENT_SYSTEM_PROMPT.len(), "{} vs {}", COMPACT_PROMPT.len(), crate::agent::AGENT_SYSTEM_PROMPT.len());
}
