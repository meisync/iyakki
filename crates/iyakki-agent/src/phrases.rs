//! Phrase tables for the assistant's robustness checks in German, French, Spanish and Hindi (F-ASSIST-10). The checks (a refusal that a
//! tool could answer, an answer that promises to report back) were English only: a model answering in the person's language slipped past
//! them. Plain lower-case text, matched as a start or as a part of a sentence the same way the English tables are. Not a translator and
//! not complete: more languages and more phrasings are added here, in one place, and every table has a test.

/// Starts of an answer that is a refusal or an apology ("I cannot", "sorry", "unfortunately").
pub const REFUSAL_STARTS: &[&str] = &[
    // German
    "ich kann nicht", "ich kann das nicht", "ich bin nicht in der lage", "leider", "entschuldigung", "es tut mir leid", "als ki", "ich habe keinen zugriff", "möchten sie, dass ich", "soll ich",
    // French
    "je ne peux pas", "je ne suis pas en mesure", "désolé", "desole", "malheureusement", "je m'excuse", "en tant qu'ia", "je n'ai pas accès", "voulez-vous que je", "souhaitez-vous que je",
    // Spanish
    "no puedo", "no soy capaz", "lo siento", "disculpa", "lamentablemente", "desafortunadamente", "como ia", "no tengo acceso", "¿quieres que", "¿quiere que", "¿desea que",
    // Hindi
    "मैं नहीं कर सकता", "मैं नहीं कर सकती", "मुझे खेद है", "क्षमा करें", "माफ़ करें", "माफ करें", "दुर्भाग्य से", "मेरे पास पहुँच नहीं", "मेरे पास पहुंच नहीं", "क्या आप चाहते हैं कि मैं",
];

/// Parts of a sentence that say "I cannot reach / open / check it" (the refusals the guard looks for when a tool could do it).
pub const REFUSAL_PARTS: &[&str] = &[
    "kann nicht auf", "habe keinen zugriff", "kann keine webseiten", "kann das internet nicht", "kann nicht im internet", "kann keine screenshots", "kann nicht abrufen",
    "ne peux pas accéder", "n'ai pas accès", "ne peux pas ouvrir", "ne peux pas consulter", "ne peux pas naviguer", "ne peux pas récupérer", "pas d'accès à internet", "ne peux pas prendre de capture",
    "no puedo acceder", "no tengo acceso", "no puedo abrir", "no puedo consultar", "no puedo navegar", "no puedo obtener", "sin acceso a internet", "no puedo tomar capturas",
    "तक पहुँच नहीं", "तक पहुंच नहीं", "इंटरनेट तक", "खोल नहीं सकता", "खोल नहीं सकती", "देख नहीं सकता", "देख नहीं सकती", "जाँच नहीं कर सकता", "जांच नहीं कर सकता",
];

/// A sentence that promises to wait, check back or report later.
pub const PROMISES: &[&str] = &[
    "ich melde mich", "ich gebe ihnen bescheid", "ich gebe dir bescheid", "ich informiere sie", "ich informiere dich", "ich halte sie auf dem laufenden", "ich halte dich auf dem laufenden", "ich sage bescheid", "ich berichte",
    "je vous tiendrai au courant", "je te tiendrai au courant", "je reviendrai vers vous", "je reviens vers vous", "je vous préviendrai", "je te préviendrai", "je vous informerai", "je vous recontacte", "je vous dirai",
    "te avisaré", "le avisaré", "te informaré", "le informaré", "te mantendré al tanto", "le mantendré al tanto", "volveré a informarte", "volveré con", "te diré cuando", "le diré cuando", "te escribiré cuando",
    "आपको बताऊंगा", "आपको बताऊँगा", "आपको बताऊंगी", "आपको बताऊँगी", "आपको सूचित करूंगा", "आपको सूचित करूँगा", "आपको सूचित करूंगी", "आपको सूचित करूँगी", "बाद में बताऊं", "खत्म होने पर बताऊं", "पूरा होने पर बताऊं",
];

/// Parts that turn a promise into something else: "I cannot", "if you want", a question.
pub const DENIALS: &[&str] = &[
    "kann nicht", "kann ich nicht", "nicht möglich", "wenn sie möchten", "wenn du möchtest", "wenn du willst", "soll ich", "möchten sie",
    "ne peux pas", "je ne peux", "impossible", "si vous voulez", "si tu veux", "voulez-vous", "souhaitez-vous",
    "no puedo", "no es posible", "si quieres", "si desea", "si lo desea", "¿quieres", "¿desea", "¿quiere",
    "नहीं कर सकता", "नहीं कर सकती", "अगर आप चाहें", "यदि आप चाहें", "क्या आप चाहते",
];

/// The openers of "Would you like me to ...?" (asking leave for something the assistant may just do).
pub const ASK_OPENERS: &[&str] = &[
    "möchten sie, dass ich ", "möchtest du, dass ich ", "willst du, dass ich ", "soll ich ", "wollen sie, dass ich ",
    "voulez-vous que je ", "veux-tu que je ", "souhaitez-vous que je ", "souhaites-tu que je ", "dois-je ",
    "¿quieres que ", "¿quiere que ", "¿desea que ", "¿te gustaría que ", "quieres que ", "¿debo ", "¿quieres que yo ",
    "क्या आप चाहते हैं कि मैं ", "क्या आप चाहेंगे कि मैं ", "क्या मैं ",
];

/// Verbs of a read-only lookup (searching, opening, checking, reading), as stems that follow the opener.
pub const READ_VERBS: &[&str] = &[
    "suchen", "nachsehen", "nachschlagen", "abrufen", "öffnen", "prüfen", "überprüfen", "lesen", "finden", "nachschauen",
    "cherch", "consult", "ouvr", "vérifi", "lire", "lise", "trouv", "regard",
    "busc", "busqu", "consult", "abrir", "abra ", "comprob", "compru", "verific", "verifiqu", "leer", "lea ", "encontr", "encuentr", "mire", "revis",
    "खोज", "देख", "खोल", "जाँच", "जांच", "पढ़", "ढूंढ", "ढूँढ",
];

/// Verbs of something that changes or spends (sending, deleting, buying, writing): leave to ask first.
pub const WRITE_VERBS: &[&str] = &[
    "senden", "schicken", "löschen", "entfernen", "kaufen", "bezahlen", "installieren", "schreiben", "speichern", "überschreiben", "erstellen", "anmelden", "hochladen",
    "envoi", "supprim", "effac", "achet", "payer", "paie", "install", "écri", "enregistr", "écras", "créer", "crée", "téléver",
    "envi", "borr", "elimin", "compr ", "compra", "pagar", "pague", "instal", "escrib", "guard", "sobrescrib", "crear", "cree ", "subir", "suba ",
    "भेज", "हटा", "मिटा", "खरीद", "भुगतान", "इंस्टॉल", "लिख", "सहेज", "बना", "अपलोड",
];

/// Words of an answer that claims work was done or is under way ("done", "created", "works", "deployed").
pub const PROGRESS: &[&str] = &[
    "fertig", "erledigt", "erstellt", "gebaut", "bereitgestellt", "läuft", "erfolgreich", "abgeschlossen", "installiert", "implementiert", "funktioniert",
    "terminé", "fini ", "créé", "construit", "déployé", "fonctionne", "réussi", "installé", "implémenté", "est prêt",
    "listo", "terminado", "completado", "creado", "construido", "desplegado", "funciona", "correctamente", "instalado", "implementado",
    "पूरा", "हो गया", "बन गया", "सफल", "चल रहा", "इंस्टॉल हो", "तैयार है",
];

/// The opener of an ask-first question in one of these languages, found in `tail` (lower case): (opener, where it starts).
pub fn ask_opener(tail: &str) -> Option<(&'static str, usize)> {
    ASK_OPENERS.iter().find_map(|o| tail.find(o).map(|i| (*o, i)))
}

pub fn reads(clause: &str) -> bool {
    READ_VERBS.iter().any(|v| clause.contains(v))
}

pub fn writes(clause: &str) -> bool {
    WRITE_VERBS.iter().any(|v| clause.contains(v))
}

/// Does a (lower case) answer claim progress in one of these languages?
pub fn claims_progress(lower: &str) -> bool {
    PROGRESS.iter().any(|w| lower.contains(w))
}

/// Does the text (any case) start like a refusal in one of these languages?
pub fn starts_like_refusal(lower: &str) -> bool {
    let t = lower.trim_start();
    REFUSAL_STARTS.iter().any(|p| t.starts_with(p) || (p.starts_with(t) && t.chars().count() >= 4))
}

/// Does the text mention a refusal to reach something in one of these languages?
pub fn mentions_refusal(lower: &str) -> bool {
    REFUSAL_PARTS.iter().any(|p| lower.contains(p))
}

/// Does one sentence promise to report back in one of these languages, with no denial in it?
pub fn promises(sentence: &str) -> bool {
    PROMISES.iter().any(|p| sentence.contains(p)) && !DENIALS.iter().any(|d| sentence.contains(d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusals_in_four_languages_are_recognised_and_ordinary_answers_are_not() {
        for t in ["Leider kann ich das nicht öffnen.", "Désolé, je ne peux pas faire cela.", "Lo siento, no puedo abrir esa página.", "क्षमा करें, मैं यह नहीं कर सकता।", "¿Quieres que lo compruebe?", "Voulez-vous que je vérifie ?"] {
            assert!(starts_like_refusal(&t.to_lowercase()), "{t}");
        }
        for t in ["Der Dienst läuft.", "Le service fonctionne.", "El servicio funciona.", "सेवा चल रही है।"] {
            assert!(!starts_like_refusal(&t.to_lowercase()), "{t}");
        }
        assert!(mentions_refusal("ich habe keinen zugriff auf das internet"));
        assert!(mentions_refusal("je ne peux pas accéder à ce site"));
        assert!(mentions_refusal("no puedo acceder a esa página"));
        assert!(mentions_refusal("मेरे पास इंटरनेट तक पहुँच नहीं है"));
        assert!(!mentions_refusal("hier ist das ergebnis"));
    }

    #[test]
    fn a_promise_to_report_back_is_found_in_each_language_and_a_denial_or_a_question_is_not_one() {
        for s in ["ich melde mich, sobald der build fertig ist", "je vous tiendrai au courant dès que c'est fini", "te avisaré cuando termine", "काम खत्म होने पर बताऊं"] {
            assert!(promises(s), "{s}");
        }
        for s in ["ich kann mich nicht melden", "je ne peux pas vous tenir au courant, mais je vous dirai si vous voulez", "no puedo avisarte cuando termine", "अगर आप चाहें तो आपको बताऊंगा"] {
            assert!(!promises(s), "{s}");
        }
    }

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
        assert!(crate::assistant_guard::detect_denial("Ich habe keinen Zugriff auf das Internet.", "suche im internet nach dem wetter", &names).is_some());
        assert!(crate::assistant_guard::detect_denial("No tengo acceso a internet, no puedo acceder a ese sitio.", "busca en internet el tiempo", &names).is_some());
        assert!(crate::assistant_guard::detect_denial("Le service fonctionne bien.", "cherche sur internet", &names).is_none());
    }

    #[test]
    fn a_question_that_asks_leave_for_a_lookup_is_found_in_each_language_and_a_question_about_a_change_is_not() {
        let names = ["web_search", "web_fetch", "web_research"];
        for (reply, request) in [
            ("Das Wetter in Köln kenne ich nicht. Soll ich im Internet danach suchen?", "wie ist das wetter, schau im internet"),
            ("Je n'ai pas cette information. Voulez-vous que je cherche sur internet ?", "cherche sur internet le temps"),
            ("No tengo ese dato. ¿Quieres que lo busque en internet?", "busca en internet el tiempo"),
        ] {
            let f = crate::assistant_guard::detect_ask_first(reply, request, &names);
            assert!(f.is_some(), "{reply}");
        }
        // asking leave for something that changes or spends stays a question
        for (reply, request) in [
            ("Die Datei ist fertig. Soll ich sie jetzt an Anna senden?", "schicke das im internet"),
            ("Voulez-vous que je supprime ce fichier sur internet ?", "internet"),
            ("¿Quieres que borre la carpeta de internet?", "internet"),
        ] {
            assert!(crate::assistant_guard::detect_ask_first(reply, request, &names).is_none(), "{reply}");
        }
    }

    #[test]
    fn a_claim_of_work_done_is_found_in_each_language_and_a_question_is_not_a_claim() {
        for t in ["Das Deployment ist fertig und läuft.", "Le service est déployé et fonctionne.", "El servicio está desplegado y funciona.", "काम पूरा हो गया है।"] {
            assert!(crate::assistant_guard::claims_progress(t), "{t}");
        }
        for t in ["Soll ich das Deployment bauen?", "Voulez-vous que je lance le déploiement ?", "¿Quieres que lo despliegue?"] {
            assert!(!crate::assistant_guard::claims_progress(t), "{t}");
        }
        assert!(!crate::assistant_guard::claims_progress("Hallo, wie kann ich helfen"));
    }
}
