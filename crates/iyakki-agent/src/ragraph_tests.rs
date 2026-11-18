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
