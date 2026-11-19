//! Graph RAG (F-RAG-15): entities from plain rules, links by shared sentences, expansion by link, the switch.

use super::*;
use crate::kb::default_embedder;

fn kb() -> KnowledgeBase {
    let mut kb = KnowledgeBase::in_memory(default_embedder());
    let add = |kb: &mut KnowledgeBase, ns: &str, title: &str, text: &str| {
        kb.add_document(ns, title, title, text, Default::default()).unwrap();
    };
    add(&mut kb, "user/alice", "team.md", "Alice Wong leads Project Orion at Acme Corp. She reports to Daniel Okafor. The budget of Project Orion is tight.");
    add(&mut kb, "user/alice", "launch.md", "Project Orion ships in June from the Leeds office. Maria Gomez tests the release of Project Orion every night.");
    add(&mut kb, "user/alice", "lunch.md", "The canteen serves soup. Nobody wrote anything about plans. Tomatoes need water.");
    add(&mut kb, "user/alice", "gomez.md", "Maria Gomez moved to the University of Leeds in 2019 and teaches data science.");
    kb
}

#[test]
fn entities_are_runs_of_capitalised_words_acronyms_and_names_with_digits_but_not_a_sentences_first_word() {
    let e = extract("Alice Wong leads Project Orion at Acme Corp. The team met the NASA people in the University of Leeds. Tuesday was cold. R2D2 and GPT4 appeared.");
    for want in ["Alice Wong", "Project Orion", "Acme Corp", "NASA", "University of Leeds", "R2D2"] {
        assert!(e.iter().any(|x| x == want), "{want} in {e:?}");
    }
    assert!(!e.iter().any(|x| x == "The" || x == "Tuesday" || x == "Nobody"), "{e:?}");
    assert_eq!(extract("Alice met Alice and Alice."), vec!["Alice"], "a repeat is listed once; mid-sentence names count");
    assert!(extract("it rained. nothing here is named.").is_empty());
}

#[test]
fn the_graph_links_entities_that_share_a_sentence_and_answers_what_is_around_one() {
    let k = kb();
    let g = Graph::build(&k, "user/alice");
    assert!(g.entities() >= 8 && g.link_count() >= 6, "{} entities, {} links", g.entities(), g.link_count());
    let (name, links, docs) = g.related("project orion").expect("the entity is known in any letter case");
    assert_eq!(name, "Project Orion");
    assert!(links.iter().any(|(n, _)| n == "Alice Wong") && links.iter().any(|(n, _)| n == "Maria Gomez"), "{links:?}");
    assert!(docs.len() == 2, "{docs:?}");
    assert!(g.related("nobody known").is_none());
    assert!(g.top(2).iter().any(|(n, c)| n == "Project Orion" && *c == 2), "{:?}", g.top(3));
}

#[test]
fn a_question_that_names_one_entity_reaches_passages_about_its_linked_ones() {
    let k = kb();
    let g = Graph::build(&k, "user/alice");
    assert_eq!(g.named_in("who tests the release of project orion?"), vec!["project orion"]);
    assert_eq!(g.named_in("what is on the menu"), Vec::<String>::new());
    let text_of = |d: &str, i: u32| k.doc_chunks("user/alice", d).into_iter().find(|c| c.0 == i).map(|c| c.1);
    // the plain search found only team.md's passage; the link to Maria Gomez leads to her own document
    let team = k.docs("user/alice").into_iter().find(|d| d.title == "team.md").unwrap().id;
    let have = vec![(team, 0u32)];
    let more = g.expand("who works on project orion", &have, 5, &text_of);
    assert!(more.iter().any(|l| l.title == "gomez.md" && l.via.contains("Project Orion <-> Maria Gomez")), "{more:#?}");
    assert!(more.iter().all(|l| !have.contains(&(l.doc_id.clone(), l.chunk_idx))), "what the search found is not offered again");
    assert!(more.iter().all(|l| !l.text.is_empty() && l.via.contains("<->")));
    // a passage that names both entities ranks above one that names only the linked one
    let both = more.iter().position(|l| l.title == "launch.md").unwrap();
    let only = more.iter().position(|l| l.title == "gomez.md").unwrap();
    assert!(both < only, "{more:#?}");
    // nothing named: nothing added; a limit of zero: nothing
    assert!(g.expand("what is for lunch", &have, 5, &text_of).is_empty());
    assert!(g.expand("project orion", &have, 0, &text_of).is_empty());
}

#[test]
fn the_switch_is_off_until_set_and_the_built_graph_is_kept_until_the_collection_changes() {
    let d = std::env::temp_dir().join(format!("iyakki-ragraph-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    assert!(!is_on(&d));
    set_on(&d, true).unwrap();
    assert!(is_on(&d));
    set_on(&d, false).unwrap();
    assert!(!is_on(&d));
    std::fs::write(d.join("rag-graph.json"), "{broken").unwrap();
    assert!(!is_on(&d), "a damaged file reads as off");
    let mut k = kb();
    let a = graph_of(&k, "user/alice");
    assert!(Arc::ptr_eq(&a, &graph_of(&k, "user/alice")), "the same graph while nothing changed");
    k.add_document("user/alice", "new.md", "new.md", "Priya Nair joined Project Orion.", Default::default()).unwrap();
    let b = graph_of(&k, "user/alice");
    assert!(!Arc::ptr_eq(&a, &b) && b.related("priya nair").is_some(), "a changed collection is read again");
    assert_eq!(graph_of(&k, "user/none").entities(), 0);
}

#[test]
fn a_name_that_starts_a_sentence_counts_when_the_texts_show_it_standing_alone_elsewhere() {
    // alone, a sentence's first capitalised word is just a capital letter
    assert!(!extract("Nadia built the Pylon gauge.").iter().any(|e| e == "Nadia"));
    // the same text shows the name standing alone inside another sentence
    let e = extract("Nadia built the Pylon gauge. The inspector praised Nadia.");
    assert!(e.iter().any(|x| x == "Nadia") && e.iter().any(|x| x == "Pylon"), "{e:?}");
    // a word that is only ever part of a longer name does not turn a lone initial into a name
    let e = extract("Project plans are due. We joined Project Orion today.");
    assert!(!e.iter().any(|x| x == "Project") && e.iter().any(|x| x == "Project Orion"), "{e:?}");
    // across a whole collection: the document that only starts with the name still links it
    let mut k = KnowledgeBase::in_memory(default_embedder());
    k.add_document("user/n", "a.md", "a.md", "Nadia built the Pylon gauge in the workshop.", Default::default()).unwrap();
    k.add_document("user/n", "b.md", "b.md", "The inspector praised Nadia on Friday.", Default::default()).unwrap();
    k.add_document("user/n", "c.md", "c.md", "Tomatoes need water. Nobody wrote anything.", Default::default()).unwrap();
    let g = Graph::build(&k, "user/n");
    let (name, links, docs) = g.related("nadia").expect("the name is known");
    assert_eq!(name, "Nadia");
    assert!(links.iter().any(|(n, _)| n == "Pylon"), "{links:?}");
    assert_eq!(docs.len(), 2, "{docs:?}");
    assert!(g.related("tomatoes").is_none() && g.related("nobody").is_none(), "capital letters alone are still not names");
}

#[test]
fn words_that_come_back_are_entities_too_in_lower_case_and_in_a_script_without_capitals() {
    let mut k = KnowledgeBase::in_memory(default_embedder());
    let mut add = |t: &str, text: &str| {
        k.add_document("user/bob", t, t, text, Default::default()).unwrap();
    };
    // "kubernetes" is never written with a capital; the Tamil words have no capitals at all
    add("a.md", "we deploy kubernetes every friday. the cluster needs patching.");
    add("b.md", "the kubernetes upgrade broke the ingress. patching the cluster fixed it.");
    add("c.md", "ingress rules for kubernetes live in the cluster repo.");
    add("d.md", "சென்னை அலுவலகம் புதிய திட்டம். சென்னை குழு வேலை செய்கிறது.");
    add("e.md", "சென்னை நகரில் திட்டம் தொடங்கியது.");
    add("f.md", "சென்னை மழை பெய்தது; திட்டம் தாமதம்.");
    for i in 0..8 {
        add(&format!("n{i}.md"), &format!("lunch menu number {i}: soup and bread only."));
    }
    let g = Graph::build(&k, "user/bob");
    assert!(g.named_in("how do we patch kubernetes?").contains(&"kubernetes".to_string()), "a lower-case term the documents keep coming back to is an entity");
    let (_, links, _) = g.related("kubernetes").expect("known");
    assert!(links.iter().any(|(n, _)| n == "ingress" || n == "cluster"), "{links:?}");
    assert!(g.named_in("சென்னை திட்டம்").iter().any(|n| n == "சென்னை"), "{:?}", g.named_in("சென்னை திட்டம்"));
    assert!(g.related("lunch").is_none(), "a word in a third of the passages, or in the filler alone, is not picked");
}

#[test]
fn the_graph_is_kept_on_disk_with_the_size_it_was_made_from_and_read_back_only_while_that_still_holds() {
    let k = kb();
    let g = Graph::build(&k, "user/alice");
    let d = crate::testdir::outside_project("ragraph", "saved");
    g.save(&d, (4, 9)).unwrap();
    let back = Graph::load(&d, "user/alice", (4, 9)).expect("same size: read back");
    assert_eq!((back.entities(), back.link_count()), (g.entities(), g.link_count()));
    assert!(Graph::load(&d, "user/alice", (5, 9)).is_none(), "the collection changed: built again");
    assert!(Graph::load(&d, "user/bob", (4, 9)).is_none());
    use std::os::unix::fs::PermissionsExt;
    let f = std::fs::read_dir(&d).unwrap().flatten().find(|e| e.file_name().to_string_lossy().contains("rag-graph")).unwrap();
    assert_eq!(f.metadata().unwrap().permissions().mode() & 0o077, 0, "private to the person");
}

// ------------------------------------------------------------------ model-based extraction (F-RAG-18)

struct FakeModel {
    answers: Vec<(&'static str, Vec<&'static str>)>,
    asked: std::sync::Mutex<Vec<String>>,
    fail: bool,
}

impl crate::ragentities::EntityModel for FakeModel {
    fn name(&self) -> String {
        "fake-model".into()
    }
    fn entities(&self, passage: &str) -> Result<Vec<String>, String> {
        self.asked.lock().unwrap().push(passage.to_string());
        if self.fail {
            return Err("the model server did not answer".into());
        }
        Ok(self.answers.iter().filter(|(k, _)| passage.contains(k)).flat_map(|(_, v)| v.iter().map(|s| s.to_string())).collect())
    }
}

#[test]
fn the_model_names_become_entities_and_links_and_nothing_is_sent_until_asked() {
    use crate::ragentities as re;
    let k = kb();
    let plan = re::plan(&k, "user/alice");
    assert_eq!(plan.passages, 4);
    assert!(plan.chars > 100 && !plan.cut);
    let m = FakeModel { answers: vec![("canteen", vec!["canteen soup", "Tomato"]), ("Tomatoes", vec!["Tomato"])], asked: Default::default(), fail: false };
    assert!(m.asked.lock().unwrap().is_empty(), "planning sends nothing");
    let found = re::extract_all(&k, "user/alice", &m).unwrap();
    assert_eq!(m.asked.lock().unwrap().len(), 4, "each passage once");
    assert_eq!(found.names(), 3, "one passage names two things, the other one");
    let mut g = Graph::build(&k, "user/alice");
    assert!(g.related("canteen soup").is_none(), "the plain rules do not see a lower-case phrase");
    g.add_model_entities(&found);
    let (name, links, _) = g.related("canteen soup").expect("now known");
    assert_eq!(name, "canteen soup");
    assert!(links.iter().any(|(n, _)| n == "Tomato"));
}

#[test]
fn a_model_that_fails_stops_the_run_early_and_the_answers_are_cleaned() {
    use crate::ragentities as re;
    let k = kb();
    let m = FakeModel { answers: vec![], asked: Default::default(), fail: true };
    // four passages: the run reports every failure but only stops after five and a majority, so here it ends with a count
    let found = re::extract_all(&k, "user/alice", &m).unwrap();
    assert_eq!(found.failed, 4);
    assert_eq!(found.names(), 0);
    let list = re::parse_entities("Sure! Here you go:\n```json\n[\"Alice Wong\", \"alice wong\", \"\", \"x\", 7, \"Bad\\u0007Name\", \"Acme Corp\"]\n```").unwrap();
    assert_eq!(list, vec!["Alice Wong", "Acme Corp"]);
    assert!(re::parse_entities("no list here").is_err() && re::parse_entities("] [").is_err() && re::parse_entities("[1, 2").is_err());
    let many = format!("[{}]", (0..100).map(|i| format!("\"name {i}\"")).collect::<Vec<_>>().join(","));
    assert_eq!(re::parse_entities(&many).unwrap().len(), re::MAX_PER_PASSAGE);
}

#[test]
fn the_http_model_asks_the_local_server_for_a_list_and_the_setting_and_the_saved_result_follow_the_rules() {
    use crate::ragentities as re;
    use iyakki_proxy::{Upstream, UpstreamResponse};
    struct Up(std::sync::Mutex<Vec<(String, String)>>);
    impl Upstream for Up {
        fn request(&self, method: &str, url: &str, _h: &[(String, String)], body: &[u8]) -> Result<UpstreamResponse, String> {
            self.0.lock().unwrap().push((format!("{method} {url}"), String::from_utf8_lossy(body).into_owned()));
            Ok(UpstreamResponse { status: 200, body: br#"{"choices":[{"message":{"content":"[\"Project Orion\", \"Leeds\"]"}}]}"#.to_vec() })
        }
    }
    let up = Arc::new(Up(Default::default()));
    let m = re::HttpEntityModel { base: "http://127.0.0.1:9380/api/v1/".into(), model: "Qwen3-0.6B".into(), up: up.clone() };
    use re::EntityModel;
    assert_eq!(m.entities(&"x".repeat(5000)).unwrap(), vec!["Project Orion", "Leeds"]);
    let sent = up.0.lock().unwrap();
    assert_eq!(sent[0].0, "POST http://127.0.0.1:9380/api/v1/chat/completions");
    assert!(sent[0].1.len() < 3000, "the passage is cut to the limit before it is sent");
    drop(sent);
    // the setting is off until the person names a model, and a damaged name is refused
    let d = crate::testdir::outside_project("ragent", "model");
    std::fs::create_dir_all(&d).unwrap();
    assert!(re::model(&d).is_none());
    assert!(re::set_model(&d, Some("bad name; rm")).is_err());
    re::set_model(&d, Some("Qwen3-0.6B")).unwrap();
    assert_eq!(re::model(&d).as_deref(), Some("Qwen3-0.6B"));
    re::set_model(&d, None).unwrap();
    assert!(re::model(&d).is_none());
    // the saved result is read back only for the same size of collection
    let e = re::Extracted { ns: "user/alice".into(), docs: 4, chunks: 4, model: "m".into(), per: vec![("d".into(), 0, vec!["Leeds".into()])], failed: 0 };
    e.save(&d).unwrap();
    assert!(re::Extracted::load(&d, "user/alice", (4, 4)).is_some() && re::Extracted::load(&d, "user/alice", (4, 5)).is_none());
    use std::os::unix::fs::PermissionsExt;
    let f = std::fs::read_dir(&d).unwrap().filter_map(Result::ok).find(|f| f.file_name().to_string_lossy().starts_with("rag-entities-user")).unwrap();
    assert_eq!(f.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    std::fs::remove_dir_all(&d).unwrap();
}
