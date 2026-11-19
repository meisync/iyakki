//! The tables of `iyakki_shared::phrases` against the agent's wait gate (`pending`).

#[test]
fn the_gate_and_the_denial_guard_use_the_tables() {
    // the wait gate: a promise in German, French, Spanish and Hindi is a promise; a question or a denial is not
    assert!(crate::pending::promises_to_report("Der Build läuft. Ich melde mich, sobald er fertig ist."));
    assert!(crate::pending::promises_to_report("Le déploiement est lancé. Je vous tiendrai au courant."));
    assert!(crate::pending::promises_to_report("Lo estoy ejecutando. Te avisaré cuando termine."));
    assert!(crate::pending::promises_to_report("काम शुरू हो गया है। पूरा होने पर बताऊं"));
    assert!(!crate::pending::promises_to_report("Soll ich mich melden? Ich melde mich nicht von selbst, das kann ich nicht."));
    assert!(!crate::pending::promises_to_report("Le service fonctionne."));
    // the denial guard: "I have no access to the internet" in another language is still found when a tool does it
    let names = ["web_search", "web_fetch", "web_research"];
    assert!(iyakki_shared::assistant_guard::detect_denial("Ich habe keinen Zugriff auf das Internet.", "suche im internet nach dem wetter", &names).is_some());
    assert!(iyakki_shared::assistant_guard::detect_denial("No tengo acceso a internet, no puedo acceder a ese sitio.", "busca en internet el tiempo", &names).is_some());
    assert!(iyakki_shared::assistant_guard::detect_denial("Le service fonctionne bien.", "cherche sur internet", &names).is_none());
}
