use super::*;

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-rec-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn a_recording_is_saved_as_a_16_bit_wav_that_reads_back_and_is_never_overwritten() {
    let d = dir("save");
    let pcm = crate::voice::tone(300.0, 1000);
    let e = save(&d, "Note", &pcm, 16_000).unwrap();
    assert_eq!(e.name, "Note");
    assert!((e.secs - 1.0).abs() < 0.01, "{}", e.secs);
    let bytes = read(&d, "Note").unwrap();
    let w = crate::voice::parse_wav(&bytes).unwrap();
    assert_eq!((w.rate, w.channels, w.samples.len()), (16_000, 1, pcm.len()));
    assert_eq!(&bytes[34..36], &16u16.to_le_bytes(), "16 bits per sample");
    let again = save(&d, "Note", &pcm, 16_000).unwrap();
    assert_eq!(again.name, "Note-2");
    assert!(save(&d, "x", &[], 16_000).is_err());
    assert!(std::fs::read_dir(&d).unwrap().all(|f| !f.unwrap().file_name().to_string_lossy().ends_with(".part")));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn names_are_checked_listed_renamed_and_deleted_inside_the_folder_only() {
    let d = dir("names");
    let pcm = crate::voice::tone(300.0, 200);
    save(&d, "one", &pcm, 16_000).unwrap();
    save(&d, "two", &pcm, 16_000).unwrap();
    std::fs::write(d.join("notes.txt"), "x").unwrap();
    let mut names: Vec<String> = list(&d).into_iter().map(|e| e.name).collect();
    names.sort();
    assert_eq!(names, ["one", "two"], "only wav files are listed");
    for bad in ["", "  ", "../x", "a/b", ".hidden", "a\\b", "a\nb"] {
        assert!(clean_name(bad).is_err(), "{bad:?}");
        assert!(rename(&d, "one", bad).is_err());
    }
    assert!(delete(&d, "../one").is_err());
    assert_eq!(rename(&d, "one", " First.wav ").unwrap(), "First");
    assert!(rename(&d, "First", "two").unwrap_err().contains("already exists"));
    assert!(rename(&d, "gone", "x").is_err());
    delete(&d, "First").unwrap();
    assert_eq!(delete(&d, "First").unwrap_err(), "That recording is gone.");
    assert_eq!(list(&d).len(), 1);
    assert!(list(&d.join("missing")).is_empty());
    let _ = std::fs::remove_dir_all(&d);
}
