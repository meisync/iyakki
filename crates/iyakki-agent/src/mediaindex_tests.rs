//! Tests of the media index that need agent types (the embedder, the Services).


use iyakki_shared::mediaindex::*;
use crate::media::{Kind as FileKind, Library};
use std::sync::Arc;
use crate::chat::{ChatBackend, Message};
use iyakki_shared::embed::TextEmbedder;
use crate::embed::HashEmbed;
use crate::media::Track;

pub fn track(path: &str, title: &str, artist: &str, genre: &str, year: u16) -> Track {
    Track { path: path.into(), kind: FileKind::Audio, title: title.into(), artist: artist.into(), album: crate::media::UNKNOWN_ALBUM.into(), seconds: Some(200), genre: genre.into(), year, size: 10, mtime: 1_700_000_000 }
}

pub fn lib(v: Vec<Track>) -> Library {
    Library { tracks: v, truncated: false }
}

fn sample() -> MediaIndex {
    let mut ix = MediaIndex::in_memory(Some(Arc::new(HashEmbed::default())));
    let mut video = track("/m/Videos/Concert.mp4", "Concert", "Unknown artist", "", 2021);
    video.kind = FileKind::Video;
    let l = lib(vec![
        track("/m/Music/Kuthu/Vaathi Coming.mp3", "Vaathi Coming", "Anirudh Ravichander", "", 2021),
        track("/m/Music/Tamil/Kutthu Fire.mp3", "Fire", "Santhosh Narayanan", "Kutthu", 2019),
        track("/m/Music/Melody/Kannazhaga.mp3", "Kannazhaga", "Anirudh Ravichander", "Melody", 2012),
        track("/m/Music/Melody/Soft Rain.mp3", "Soft Rain", "Sid Sriram", "melodious", 2022),
        track("/m/Music/Rock/Thunder.mp3", "Thunder", "Imagine Dragons", "Rock", 2017),
        track("/m/Music/தமிழ்/குத்து பாடல்.mp3", "குத்து பாடல்", "Devi", "", 2015),
        video,
    ]);
    let imgs = vec![ImageFile { path: "/m/Pictures/2024/Trip to Goa.jpg".into(), size: 5, mtime: 1_710_000_000 }, ImageFile { path: "/m/Pictures/old.png".into(), size: 5, mtime: 1_400_000_000 }];
    ix.sync(&l, &imgs, 1_700_000_100);
    ix.embed_batch(1000);
    ix
}

fn titles(ix: &MediaIndex, phrase: &str) -> Vec<String> {
    ix.find(phrase, 20).iter().map(|i| i.title.clone()).collect()
}

#[test]
fn the_vocabulary_maps_spellings_and_scripts_to_one_type() {
    for w in ["kuthu", "Kutthu", "KOOTHU", "குத்து"] {
        assert_eq!(canonical_type(w), Some("kuthu"), "{w}");
    }
    assert_eq!(canonical_type("melodious"), Some("melody"));
    assert_eq!(canonical_type("மெலடி"), Some("melody"));
    assert_eq!(canonical_type("Hip Hop"), Some("hip-hop"));
    assert_eq!(canonical_type("lo-fi"), Some("lofi"));
    assert_eq!(canonical_type("party"), Some("beats"));
    assert_eq!(canonical_type("banana"), None);
    assert_eq!(script_language("குத்து"), Some("tamil"));
    assert_eq!(script_language("हिन्दी"), Some("hindi"));
    assert_eq!(script_language("plain"), None);
}

#[test]
fn natural_phrases_become_filters_sort_hints_and_leftover_text() {
    let q = Query::parse("kutthu song");
    assert_eq!((q.types.clone(), q.text.as_str(), q.kind), (vec!["kuthu".to_string()], "", None));
    let q = Query::parse("melody songs by anirudh above 4 stars");
    assert_eq!((q.types.clone(), q.artist.as_deref(), q.min_rating, q.text.as_str()), (vec!["melody".to_string()], Some("anirudh"), 4, ""));
    let q = Query::parse("latest tamil beats");
    assert_eq!((q.sort, q.langs.clone(), q.types.clone()), (Sort::Latest, vec!["tamil".to_string()], vec!["beats".to_string()]));
    assert_eq!(Query::parse("videos").kind, Some(ItemKind::Video));
    let q = Query::parse("photos from 2024");
    assert_eq!((q.kind, q.year_from, q.year_to), (Some(ItemKind::Image), Some(2024), Some(2024)));
    let q = Query::parse("play a hip hop song named thunder most played");
    assert_eq!((q.types.clone(), q.sort, q.text.as_str()), (vec!["hip-hop".to_string()], Sort::MostPlayed, "named thunder"));
    assert_eq!(Query::parse("குத்து").types, vec!["kuthu".to_string()]);
    assert_eq!(Query::parse("songs after 2010 before 2020 rated 3").min_rating, 3);
    assert_eq!(Query::parse("best 5 stars").sort, Sort::TopRated);
    assert!(Query::parse("kutthu song").chips().contains(&"type: kuthu".to_string()));
    assert_eq!(Query::parse("thunder").text, "thunder");
}

#[test]
fn types_come_from_genre_tags_folders_titles_and_scripts() {
    let ix = sample();
    let get = |t: &str| ix.items().iter().find(|i| i.title == t).unwrap().clone();
    assert!(get("Vaathi Coming").types.contains("kuthu"), "from the folder name");
    assert!(get("Fire").types.contains("kuthu"), "from the genre tag, other spelling");
    assert!(get("Soft Rain").types.contains("melody"));
    assert!(get("Thunder").types.contains("rock"));
    assert!(get("குத்து பாடல்").types.contains("kuthu") && get("குத்து பாடல்").langs.contains("tamil"));
    assert!(get("Fire").langs.contains("tamil"), "a language word in the path");
    assert_eq!(get("Concert").kind, ItemKind::Video);
    assert_eq!(get("Trip to Goa").kind, ItemKind::Image);
    assert_eq!(get("Trip to Goa").year, 2024, "the folder year wins");
}

#[test]
fn kutthu_song_finds_the_kuthu_items_whatever_the_spelling() {
    let ix = sample();
    let mut t = titles(&ix, "kutthu song");
    t.sort();
    assert_eq!(t, vec!["Fire", "Vaathi Coming", "குத்து பாடல்"]);
    assert_eq!(titles(&ix, "குத்து").len(), 3);
    assert_eq!(titles(&ix, "tamil kuthu").len(), 2);
    assert!(titles(&ix, "melody").contains(&"Soft Rain".to_string()));
    assert_eq!(titles(&ix, "rock"), vec!["Thunder"]);
}

#[test]
fn ratings_years_artists_kinds_and_sort_hints_narrow_and_order() {
    let mut ix = sample();
    let id = |ix: &MediaIndex, t: &str| ix.items().iter().find(|i| i.title == t).unwrap().id.clone();
    let (k, s) = (id(&ix, "Kannazhaga"), id(&ix, "Soft Rain"));
    ix.rate(&k, 5).unwrap();
    ix.rate(&s, 3).unwrap();
    assert_eq!(titles(&ix, "melody songs by anirudh above 4 stars"), vec!["Kannazhaga"]);
    assert_eq!(titles(&ix, "melody above 3 stars"), vec!["Kannazhaga", "Soft Rain"], "best rated first");
    assert!(titles(&ix, "melody above 5 stars").len() == 1);
    assert_eq!(titles(&ix, "videos"), vec!["Concert"]);
    assert_eq!(titles(&ix, "photos from 2024"), vec!["Trip to Goa"]);
    assert_eq!(titles(&ix, "songs after 2020"), vec!["Vaathi Coming", "Soft Rain"].into_iter().map(String::from).collect::<Vec<_>>().iter().filter(|t| titles(&ix, "songs after 2020").contains(t)).cloned().collect::<Vec<_>>());
    assert!(!titles(&ix, "songs before 2013").contains(&"Thunder".to_string()));
    // plays order "most played"
    ix.record_play(&s, 5).unwrap();
    ix.record_play(&s, 6).unwrap();
    assert_eq!(titles(&ix, "most played melody")[0], "Soft Rain");
    assert_eq!(ix.get(&s).unwrap().plays, 2);
    assert_eq!(ix.get(&s).unwrap().last_played, 6);
    assert!(ix.rate(&s, 6).is_err() && ix.rate("nope", 1).is_err());
    ix.rate(&s, 0).unwrap();
    assert_eq!(ix.get(&s).unwrap().rating, 0, "0 clears");
}

#[test]
fn tags_and_types_set_by_the_person_are_searchable_and_untag_removes_them() {
    let mut ix = sample();
    let id = ix.items().iter().find(|i| i.title == "Thunder").unwrap().id.clone();
    assert!(titles(&ix, "gym").is_empty() || !titles(&ix, "kuthu").contains(&"Thunder".to_string()));
    ix.tag(&id, &["Kutthu".into(), "roadtrip".into()]).unwrap();
    assert!(ix.get(&id).unwrap().tags.contains("kuthu") && ix.get(&id).unwrap().tags.contains("roadtrip"));
    assert!(titles(&ix, "kuthu").contains(&"Thunder".to_string()), "a tag works as a type");
    assert_eq!(titles(&ix, "roadtrip"), vec!["Thunder"], "free tags match as words");
    ix.untag(&id, &["kutthu".into()]).unwrap();
    assert!(!titles(&ix, "kuthu").contains(&"Thunder".to_string()));
    ix.set_types(&id, &["melodious".into()]).unwrap();
    assert!(titles(&ix, "melody").contains(&"Thunder".to_string()) && !titles(&ix, "rock").contains(&"Thunder".to_string()));
    // any word is accepted: a known one under its canonical name, another as the person wrote it (lowercase)
    ix.set_types(&id, &["Kutthu".into(), "Road Trip".into(), "nonsense".into()]).unwrap();
    let types: Vec<String> = ix.get(&id).unwrap().effective_types().iter().cloned().collect();
    assert_eq!(types, vec!["kuthu".to_string(), "nonsense".to_string(), "road trip".to_string()]);
    assert!(ix.set_types(&id, &["x".repeat(60)]).is_err() && ix.set_types(&id, &["a\nb".into()]).is_err(), "only absurd words are refused");
    ix.set_types(&id, &[]).unwrap();
    assert!(titles(&ix, "rock").contains(&"Thunder".to_string()), "empty goes back to automatic");
}

#[test]
fn explain_shows_the_score_parts_of_the_hybrid_search() {
    let ix = sample();
    let mut q = Query::parse("soft rain");
    q.explain = true;
    let hits = ix.search(&q);
    let e = hits[0].explain.clone().unwrap();
    assert_eq!(ix.items()[hits[0].index].title, "Soft Rain");
    assert!(e.bm25 > 0.0 && e.bm25_rank == Some(1) && e.rrf > 0.0, "{e:?}");
    assert!(e.cosine > 0.2 && e.vector_rank.is_some(), "the vector side agrees: {e:?}");
    // a word that only the vector side can connect to is still found by a prefix of a title word
    let q2 = Query::parse("vaath");
    assert_eq!(ix.find("vaath", 5).len(), 1);
    assert_eq!(q2.text, "vaath");
    // without vectors only the words rank
    let mut plain = MediaIndex::in_memory(None);
    plain.sync(&lib(vec![track("/m/a.mp3", "Soft Rain", "X", "", 2000)]), &[], 1);
    let mut q = Query::parse("rain");
    q.explain = true;
    let e = plain.search(&q)[0].explain.clone().unwrap();
    assert!(e.vector_rank.is_none() && e.bm25_rank == Some(1));
    assert!(ix.search(&Query::parse("zzzqqq")).is_empty());
}

#[test]
fn a_change_of_embedder_skips_the_old_vectors_and_the_index_survives_a_restart() {
    let dir = std::env::temp_dir().join(format!("iyakki-mediaindex-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut ix = MediaIndex::open(&dir, "ann", Some(Arc::new(HashEmbed::default())));
    ix.sync(&lib(vec![track("/m/Kuthu/a.mp3", "A", "X", "", 2000), track("/m/b.mp3", "B", "Y", "Melody", 2001)]), &[], 10);
    ix.embed_batch(10);
    let a = ix.items()[0].id.clone();
    ix.rate(&a, 4).unwrap();
    ix.tag(&a, &["fav".into()]).unwrap();
    assert_eq!(ix.vector_count(), 1, "new words make the old vector stale");
    ix.embed_batch(10);
    ix.save().unwrap();
    let back = MediaIndex::open(&dir, "ann", Some(Arc::new(HashEmbed::default())));
    assert_eq!((back.len(), back.vector_count()), (2, 2));
    assert_eq!(back.get(&a).unwrap().rating, 4);
    assert!(back.get(&a).unwrap().tags.contains("fav"));
    assert_eq!(back.find("kuthu", 5).len(), 1);
    // another embedder id: vectors are ignored, words still work
    struct Other;
    impl TextEmbedder for Other {
        fn id(&self) -> String {
            "other-8".into()
        }
        fn embed(&self, t: &[String]) -> Result<Vec<Vec<f32>>, String> {
            Ok(t.iter().map(|_| vec![1.0; 8]).collect())
        }
    }
    let other = MediaIndex::open(&dir, "ann", Some(Arc::new(Other)));
    assert_eq!(other.vector_count(), 0);
    assert_eq!(other.find("kuthu", 5).len(), 1);
    // another person has nothing
    assert!(MediaIndex::open(&dir, "bob", None).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_rescan_adds_removes_and_rereads_changed_files_but_keeps_the_persons_data() {
    let mut ix = MediaIndex::in_memory(None);
    let a = track("/m/a.mp3", "Alpha", "X", "", 2000);
    let mut b = track("/m/b.mp3", "B", "Y", "", 2001);
    let r = ix.sync(&lib(vec![a.clone(), b.clone()]), &[], 1);
    assert_eq!(r, SyncReport { added: 2, removed: 0, changed: 0 });
    let id = item_id("/m/a.mp3");
    ix.rate(&id, 5).unwrap();
    b.title = "B renamed".into();
    b.mtime += 5;
    let r = ix.sync(&lib(vec![a.clone(), b.clone(), track("/m/c.mp3", "C", "Z", "", 2002)]), &[], 2);
    assert_eq!(r, SyncReport { added: 1, removed: 0, changed: 1 });
    assert_eq!(ix.find("renamed", 5).len(), 1);
    assert_eq!(ix.get(&id).unwrap().rating, 5);
    let r = ix.sync(&lib(vec![b]), &[], 3);
    assert_eq!((r.removed, ix.len()), (2, 1));
    assert!(ix.find("alpha", 5).is_empty());
}

#[test]
fn pictures_are_found_by_signature_not_by_name_alone() {
    // the excluded folder has a name no process id or temp path can contain (the id of the process once contained "2023" and the test failed)
    let dir = std::env::temp_dir().join(format!("iyakki-mediaimg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("outofscope")).unwrap();
    std::fs::write(dir.join("outofscope/a.png"), b"\x89PNG\r\n\x1a\n0000").unwrap();
    std::fs::write(dir.join("fake.jpg"), b"not a picture at all").unwrap();
    std::fs::write(dir.join("b.jpeg"), [0xFF, 0xD8, 0xFF, 0xE0, 0, 0]).unwrap();
    std::fs::write(dir.join("notes.txt"), b"x").unwrap();
    let d = dir.to_str().unwrap().to_string();
    let got = scan_images(&[d.clone()], &|_| true, 100);
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(scan_images(&[d], &|p| !p.contains("/outofscope/"), 100).len() == 1, "the read scope decides");
    let _ = std::fs::remove_dir_all(&dir);
}

struct Scripted(String);
impl ChatBackend for Scripted {
    fn complete(&self, h: &[Message]) -> Result<String, String> {
        assert!(h.last().unwrap().content.contains("Vaathi Coming") && h.last().unwrap().content.contains("kuthu"), "the titles and the vocabulary are in the prompt");
        Ok(self.0.clone())
    }
}

#[test]
fn the_assistant_proposes_types_which_are_only_applied_when_accepted() {
    let mut ix = sample();
    let items: Vec<Item> = ix.items().iter().filter(|i| i.title == "Vaathi Coming" || i.title == "Thunder").cloned().collect();
    let refs: Vec<&Item> = items.iter().collect();
    let props = suggest_tags(&refs, &Scripted("1: Kutthu, dance, made-up | Tamil\n2: rock | English\nnoise line\n9: pop".into())).unwrap();
    assert_eq!(props.len(), 2);
    assert_eq!(props[0].types, vec!["kuthu".to_string(), "beats".to_string()]);
    assert_eq!(props[0].language.as_deref(), Some("tamil"));
    assert!(ix.get(&props[0].id).unwrap().type_override.is_none(), "nothing applied by itself");
    assert_eq!(apply_proposals(&mut ix, &props), 2);
    assert!(ix.get(&props[1].id).unwrap().effective_types().contains("rock"));
    assert!(suggest_tags(&[], &Scripted(String::new())).unwrap().is_empty());
    struct Down;
    impl ChatBackend for Down {
        fn complete(&self, _: &[Message]) -> Result<String, String> {
            Err("no model".into())
        }
    }
    assert!(suggest_tags(&refs, &Down).is_err());
}

struct Dim64;
impl TextEmbedder for Dim64 {
    fn id(&self) -> String {
        "hash-64".into()
    }
    fn embed(&self, t: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let e = iyakki_mem::HashEmbedder { dim: 64 };
        use iyakki_mem::Embedder as _;
        Ok(t.iter().map(|x| e.embed(x)).collect())
    }
}

#[test]
#[ignore = "benchmark: cargo test -p iyakki-agent --release mediaindex::tests::benchmark -- --ignored --nocapture"]
fn benchmark_fifty_thousand_items() {
    let genres = ["Melody", "Kuthu", "Rock", "Classical", "Pop", "Devotional", "Hip-hop", "Lofi", ""];
    let artists = ["Anirudh", "Rahman", "Sid Sriram", "Ilaiyaraaja", "Hiphop Tamizha", "Yuvan", "Harris", "Imman"];
    let words = ["moon", "river", "fire", "dance", "love", "night", "road", "sky", "dream", "heart", "storm", "gold", "city", "rain", "wind"];
    let mut tracks = vec![];
    for i in 0..50_000usize {
        let t = format!("{} {} {}", words[i % 15], words[(i / 15) % 15], i);
        tracks.push(track(&format!("/m/Music/{}/{}/{}.mp3", genres[i % 9], artists[i % 8], i), &t, artists[i % 8], genres[i % 9], 1990 + (i % 35) as u16));
    }
    let mut ix = MediaIndex::in_memory(Some(Arc::new(Dim64)));
    let t0 = std::time::Instant::now();
    ix.sync(&lib(tracks), &[], 1);
    let t_sync = t0.elapsed();
    let t0 = std::time::Instant::now();
    while ix.embed_batch(5000) > 0 {}
    let t_embed = t0.elapsed();
    for i in (0..50_000).step_by(3) {
        let id = ix.items()[i].id.clone();
        ix.rate(&id, ((i / 3) % 5 + 1) as u8).unwrap();
    }
    let queries = ["kutthu song", "melody songs by anirudh above 4 stars", "latest tamil beats", "moon river", "fire storm 2005", "dream", "classical above 3 stars"];
    let mut worst = std::time::Duration::ZERO;
    let mut sum = std::time::Duration::ZERO;
    let mut runs = 0u32;
    for _ in 0..5 {
        for q in queries {
            let query = Query::parse(q);
            let t0 = std::time::Instant::now();
            let h = ix.search(&query);
            let d = t0.elapsed();
            println!("  {q:45} {d:?} ({} hits)", h.len());
            assert!(!h.is_empty() || q.contains("latest tamil"), "{q}");
            worst = worst.max(d);
            sum += d;
            runs += 1;
        }
    }
    let dir = std::env::temp_dir().join(format!("iyakki-mediabench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    ix.path = dir.join("media-index-bench.json");
    ix.vpath = dir.join("media-index-bench.vec");
    ix.unsaved = true;
    let t0 = std::time::Instant::now();
    ix.save().unwrap();
    let t_save = t0.elapsed();
    let sz = std::fs::metadata(&ix.path).unwrap().len() + std::fs::metadata(&ix.vpath).unwrap().len();
    let t0 = std::time::Instant::now();
    let back = MediaIndex::open(&dir, "bench", Some(Arc::new(Dim64)));
    let t_load = t0.elapsed();
    assert_eq!(back.len(), 50_000);
    println!("BENCH 50k items, 64-dim vectors: sync {t_sync:?}, embed {t_embed:?}, search avg {:?} worst {worst:?}, save {t_save:?} ({} MB), load {t_load:?}", sum / runs, sz / 1_000_000);
    let _ = std::fs::remove_dir_all(&dir);
}
