//! Re-ranking of retrieved passages (F-RAG-14): the lexical scorer, the model scorer behind a fake server, the order rules and the
//! stored mode.

use super::*;
use iyakki_proxy::UpstreamResponse;
use std::sync::Mutex;

struct Fake {
    answer: Result<(u16, String), String>,
    seen: Mutex<Vec<(String, String)>>,
}

impl Upstream for Fake {
    fn request(&self, method: &str, url: &str, _h: &[(String, String)], body: &[u8]) -> Result<UpstreamResponse, String> {
        self.seen.lock().unwrap().push((format!("{method} {url}"), String::from_utf8_lossy(body).to_string()));
        self.answer.clone().map(|(status, b)| UpstreamResponse { status, body: b.into_bytes() })
    }
}

fn fake(answer: Result<(u16, &str), &str>) -> Arc<Fake> {
    Arc::new(Fake { answer: answer.map(|(s, b)| (s, b.to_string())).map_err(String::from), seen: Mutex::new(vec![]) })
}

const PASSAGES: [&str; 4] = [
    "The garden needs water twice a week in summer.",
    "To renew the passport, bring two photos to the town hall and pay the fee at the desk.",
    "Passport photos must be recent. Renew a passport early.",
    "Bring your own bag to the market.",
];

#[test]
fn the_lexical_scorer_prefers_the_passage_that_has_the_words_together_and_in_order() {
    let q = "how do I renew my passport";
    let s = LexicalReranker.scores(q, &PASSAGES).unwrap();
    assert_eq!(s.len(), 4);
    let best = s.iter().cloned().enumerate().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).unwrap().0;
    assert!([1, 2].contains(&best), "{s:?}");
    assert!(s[1] > s[0] && s[1] > s[3] && s[2] > s[3], "{s:?}");
    // the same words in the question's order beat the words scattered
    let t = LexicalReranker.scores("renew passport photos", &["photos are taken at the market on tuesdays while the passport office on the other side of town is open and to renew anything at all you wait", "renew passport photos at the desk"]).unwrap();
    assert!(t[1] > t[0], "{t:?}");
    assert_eq!(LexicalReranker.scores("zzz qqq", &PASSAGES).unwrap(), vec![0.0; 4]);
    assert!(LexicalReranker.scores("   ", &PASSAGES).is_err());
    assert!(LexicalReranker.scores("passport", &[""]).unwrap()[0] == 0.0, "an empty passage scores nothing");
}

#[test]
fn passages_are_reordered_by_score_keep_the_search_order_on_ties_and_the_tail_stays_behind() {
    struct By(Vec<f32>);
    impl Reranker for By {
        fn name(&self) -> String {
            "by".into()
        }
        fn scores(&self, _q: &str, p: &[&str]) -> Result<Vec<f32>, String> {
            assert_eq!(p.len(), self.0.len());
            Ok(self.0.clone())
        }
    }
    let items = vec!["a", "b", "c", "d"];
    let (o, w) = rerank(&By(vec![0.1, 0.9, 0.9, 0.5]), "q", items.clone(), |s| s.to_string());
    assert_eq!(o, ["b", "c", "d", "a"], "ties keep the search order");
    assert!(w.is_none());
    // one passage or none: nothing to order
    assert_eq!(rerank(&By(vec![]), "q", vec!["a"], |s| s.to_string()).0, ["a"]);
    // more than the cap: the tail keeps its place behind the reordered head
    let many: Vec<String> = (0..MAX_CANDIDATES + 3).map(|i| format!("p{i}")).collect();
    let mut sc: Vec<f32> = (0..MAX_CANDIDATES).map(|i| i as f32).collect();
    sc.reverse();
    sc.swap(0, MAX_CANDIDATES - 1);
    let (o, _) = rerank(&By(sc), "q", many.clone(), |s| s.clone());
    assert_eq!(o.len(), many.len());
    assert_eq!(&o[MAX_CANDIDATES..], &many[MAX_CANDIDATES..]);
    // an error leaves the order and says why
    let (o, w) = rerank(&LexicalReranker, "   ", items, |s| s.to_string());
    assert_eq!(o, ["a", "b", "c", "d"]);
    assert!(w.unwrap().contains("skipped"));
}

#[test]
fn a_reranking_model_is_asked_in_the_servers_shape_and_a_bad_answer_is_refused() {
    let f = fake(Ok((200, r#"{"results":[{"index":2,"relevance_score":0.9},{"index":0,"relevance_score":0.1},{"index":1,"relevance_score":0.4}]}"#)));
    let rr = HttpReranker { base: "http://127.0.0.1:13305/api/v1/".into(), model: "bge-reranker-v2-m3-GGUF".into(), up: f.clone() };
    let s = rr.scores("q", &["a", "b", "c"]).unwrap();
    assert_eq!(s, vec![0.1, 0.4, 0.9]);
    let seen = f.seen.lock().unwrap().clone();
    assert_eq!(seen[0].0, "POST http://127.0.0.1:13305/api/v1/reranking");
    let b: Value = serde_json::from_str(&seen[0].1).unwrap();
    assert_eq!((b["model"].as_str(), b["query"].as_str(), b["documents"].as_array().map(Vec::len)), (Some("bge-reranker-v2-m3-GGUF"), Some("q"), Some(3)));
    for bad in [r#"{"results":[{"index":0,"relevance_score":1.0}]}"#, r#"{"results":[{"index":9,"relevance_score":1.0}]}"#, "not json", r#"{"x":1}"#, r#"{"results":[{"index":0}]}"#] {
        assert!(parse_scores(bad, 3).is_err(), "{bad}");
    }
    assert!(HttpReranker { base: "http://x".into(), model: "m".into(), up: fake(Ok((500, ""))) }.scores("q", &["a"]).unwrap_err().contains("500"));
    assert!(HttpReranker { base: "http://x".into(), model: "m".into(), up: fake(Err("refused")) }.scores("q", &["a"]).unwrap_err().contains("did not answer"));
}

#[test]
fn the_mode_is_off_until_chosen_is_stored_and_checked_and_builds_the_right_scorer() {
    let d = std::env::temp_dir().join(format!("iyakki-rerank-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    assert_eq!(Mode::load(&d), Mode::Off);
    Mode::Lexical.save(&d).unwrap();
    assert_eq!(Mode::load(&d), Mode::Lexical);
    let m = Mode::parse("model", "bge-reranker-v2-m3-GGUF").unwrap();
    m.save(&d).unwrap();
    assert_eq!(Mode::load(&d).text(), "model bge-reranker-v2-m3-GGUF");
    assert!(Mode::parse("model", "").is_err() && Mode::parse("model", "a b").is_err() && Mode::parse("fast", "").is_err());
    assert!(Mode::Model { model: "bad name".into() }.save(&d).is_err());
    std::fs::write(d.join("rag-rerank.json"), "{ broken").unwrap();
    assert_eq!(Mode::load(&d), Mode::Off, "a damaged file reads as off");
    let up: Arc<dyn Upstream> = fake(Err("x"));
    assert!(build(&Mode::Off, None, up.clone()).unwrap().is_none());
    assert_eq!(build(&Mode::Lexical, None, up.clone()).unwrap().unwrap().name(), "lexical");
    assert!(build(&m, None, up.clone()).err().unwrap().contains("no Lemonade"));
    assert!(build(&m, Some("http://127.0.0.1:1/api/v1".into()), up).unwrap().unwrap().name().starts_with("model bge"));
}
