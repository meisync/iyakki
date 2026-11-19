use super::*;

fn dir(tag: &str) -> PathBuf {
    let d = crate::testdir::outside_project("datamigrate", tag);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_steps_change_the_files_of_the_data_folder_and_nothing_else() {
    let d = dir("ok");
    std::fs::write(d.join("settings.json"), r#"{"colour":"red","old":1}"#).unwrap();
    std::fs::write(d.join("a.txt"), "x").unwrap();
    std::fs::write(d.join("cache.tmp"), "y").unwrap();
    let ops = parse(r#"{"ops":[{"op":"json_rename_key","file":"settings.json","from":"colour","to":"color"},{"op":"json_set_default","file":"settings.json","key":"theme","value":"dark"},{"op":"json_set_default","file":"settings.json","key":"color","value":"blue"},{"op":"json_remove_key","file":"settings.json","key":"old"},{"op":"rename","from":"a.txt","to":"sub/b.txt"},{"op":"delete","file":"cache.tmp"},{"op":"delete","file":"never-there.tmp"}]}"#).unwrap();
    assert_eq!(apply(&d, &ops).unwrap(), "7 migration steps done");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(d.join("settings.json")).unwrap()).unwrap();
    assert_eq!(v, serde_json::json!({"color": "red", "theme": "dark"}), "renamed, defaulted without overwriting, removed");
    assert!(!d.join("a.txt").exists() && d.join("sub/b.txt").exists() && !d.join("cache.tmp").exists());
}

#[test]
fn a_file_that_asks_for_anything_else_is_refused_before_a_single_step_runs() {
    for bad in [
        r#"{"ops":[]}"#,
        r#"{"ops":[{"op":"run","cmd":"rm -rf /"}]}"#,
        r#"{"ops":[{"op":"delete","file":"../x"}]}"#,
        r#"{"ops":[{"op":"delete","file":"/etc/passwd"}]}"#,
        r#"{"ops":[{"op":"rename","from":"a","to":"b/../../c"}]}"#,
        r#"{"ops":[{"op":"delete","file":"a","extra":1}]}"#,
        "not json",
    ] {
        assert!(parse(bad).is_err(), "{bad}");
    }
    let many = format!(r#"{{"ops":[{}]}}"#, vec![r#"{"op":"delete","file":"a"}"#; MAX_OPS + 1].join(","));
    assert!(parse(&many).is_err());
}

#[test]
fn a_link_in_the_data_folder_is_never_followed_and_a_step_that_cannot_run_says_why() {
    let d = dir("links");
    let outside = dir("links-outside");
    std::fs::write(outside.join("secret.json"), r#"{"k":1}"#).unwrap();
    std::os::unix::fs::symlink(&outside, d.join("out")).unwrap();
    let e = apply(&d, &parse(r#"{"ops":[{"op":"json_remove_key","file":"out/secret.json","key":"k"}]}"#).unwrap()).unwrap_err();
    assert!(e.contains("link"), "{e}");
    assert_eq!(std::fs::read_to_string(outside.join("secret.json")).unwrap(), r#"{"k":1}"#, "the file outside is untouched");
    std::fs::write(d.join("a.txt"), "x").unwrap();
    std::fs::write(d.join("b.txt"), "y").unwrap();
    assert!(apply(&d, &parse(r#"{"ops":[{"op":"rename","from":"a.txt","to":"b.txt"}]}"#).unwrap()).unwrap_err().contains("already there"));
    assert!(apply(&d, &parse(r#"{"ops":[{"op":"rename","from":"missing","to":"c"}]}"#).unwrap()).unwrap_err().contains("not there"));
    std::fs::write(d.join("t.json"), "[1,2]").unwrap();
    assert!(apply(&d, &parse(r#"{"ops":[{"op":"json_remove_key","file":"t.json","key":"k"}]}"#).unwrap()).unwrap_err().contains("not a JSON object"));
}
