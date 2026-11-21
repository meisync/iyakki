// SPDX-License-Identifier: Apache-2.0
use super::*;
use std::sync::Mutex;

fn tmp(tag: &str) -> PathBuf {
    iyakki_archive::archive::tests::tmp(&format!("datasci-io-{tag}"))
}

#[derive(Default)]
struct Rec(Mutex<Vec<String>>);

impl Sink for Rec {
    fn table(&self, name: &str, file: &Path, rows: usize, columns: usize) -> Result<String, String> {
        assert!(file.is_file());
        self.0.lock().unwrap().push(format!("table {name} {rows}x{columns}"));
        Ok("It shows in the Data tab.".into())
    }
    fn image(&self, name: &str, file: &Path) -> Result<String, String> {
        assert!(file.is_file());
        self.0.lock().unwrap().push(format!("image {name}"));
        Ok("Kept in your pictures.".into())
    }
}

fn file_input(name: &str, p: &Path) -> Input {
    Input { name: name.into(), source: Source::File(p.to_path_buf()) }
}

#[test]
fn inputs_are_copied_in_read_only_and_bad_names_links_and_sizes_are_refused() {
    let d = tmp("inputs");
    let src = d.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.csv"), "x,y\n1,2\n").unwrap();
    let work = d.join("work");
    let io = RunIo { inputs: vec![file_input("data.csv", &src.join("a.csv"))], ..Default::default() };
    let notes = prepare(&work, &io).unwrap();
    assert_eq!(notes, ["in/data.csv (8 bytes)"]);
    assert_eq!(std::fs::read_to_string(work.join("in/data.csv")).unwrap(), "x,y\n1,2\n");
    assert!(work.join("out").is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(work.join("in/data.csv")).unwrap().permissions().mode() & 0o222, 0, "read-only for the script");
    }
    let _ = std::fs::set_permissions(work.join("in"), std::os::unix::fs::PermissionsExt::from_mode(0o755));
    // refusals leave no `in` folder behind
    for (name, why) in [("../x", "plain name"), (".hid", "plain name"), ("", "plain name"), ("a b", "plain name")] {
        let w = d.join("w2");
        let r = prepare(&w, &RunIo { inputs: vec![file_input(name, &src.join("a.csv"))], ..Default::default() });
        assert!(r.unwrap_err().contains(why), "{name}");
    }
    let w = d.join("w3");
    let twice = RunIo { inputs: vec![file_input("a", &src.join("a.csv")), file_input("a", &src.join("a.csv"))], ..Default::default() };
    assert!(prepare(&w, &twice).unwrap_err().contains("twice"));
    let many = RunIo { inputs: (0..9).map(|i| file_input(&format!("f{i}"), &src.join("a.csv"))).collect(), ..Default::default() };
    assert!(prepare(&w, &many).unwrap_err().contains("at most 8"));
    assert!(prepare(&w, &RunIo { inputs: vec![file_input("m", &src.join("missing.csv"))], ..Default::default() }).is_err());
    assert!(!w.join("in").exists(), "a refusal leaves nothing half done");
    assert!(prepare(&w, &RunIo { frame_cap: Some(0), ..Default::default() }).is_err());
    assert!(prepare(&w, &RunIo { frame_cap: Some(FRAME_CAP_MAX + 1), ..Default::default() }).is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(src.join("a.csv"), src.join("link.csv")).unwrap();
        assert!(prepare(&d.join("w4"), &RunIo { inputs: vec![file_input("l", &src.join("link.csv"))], ..Default::default() }).unwrap_err().contains("link"));
    }
    let big = src.join("big.bin");
    std::fs::File::create(&big).unwrap().set_len(INPUT_BYTES + 1).unwrap();
    assert!(prepare(&d.join("w5"), &RunIo { inputs: vec![file_input("b", &big)], ..Default::default() }).unwrap_err().contains("larger than"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_frame_folder_is_numbered_capped_and_must_be_one_kind_of_picture() {
    let d = tmp("frames");
    let pics = d.join("pics");
    std::fs::create_dir_all(&pics).unwrap();
    for n in ["c.jpg", "a.JPG", "b.jpeg", "notes.txt"] {
        std::fs::write(pics.join(n), n).unwrap();
    }
    let frames = |name: &str| Input { name: name.into(), source: Source::Frames(pics.clone()) };
    let w = d.join("w");
    let notes = prepare(&w, &RunIo { inputs: vec![frames("cam")], frame_cap: Some(2), ..Default::default() }).unwrap();
    assert_eq!(notes, ["in/cam/%06d.jpg (2 frames, cap 2)"]);
    assert_eq!(std::fs::read_to_string(w.join("in/cam/000001.jpg")).unwrap(), "a.JPG", "sorted by name, numbered from 1");
    assert_eq!(std::fs::read_to_string(w.join("in/cam/000002.jpg")).unwrap(), "b.jpeg");
    assert!(!w.join("in/cam/000003.jpg").exists(), "the cap holds");
    assert_eq!(RunIo::default().cap(), FRAME_CAP_DEFAULT);
    std::fs::write(pics.join("d.png"), "d").unwrap();
    assert!(prepare(&d.join("w2"), &RunIo { inputs: vec![frames("cam")], ..Default::default() }).unwrap_err().contains("mixes"));
    let empty = d.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert!(prepare(&d.join("w3"), &RunIo { inputs: vec![Input { name: "e".into(), source: Source::Frames(empty) }], ..Default::default() }).unwrap_err().contains("no jpg or png"));
    let _ = std::fs::set_permissions(w.join("in"), std::os::unix::fs::PermissionsExt::from_mode(0o755));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_csv_is_a_table_only_with_a_header_and_the_same_columns_in_every_row() {
    assert_eq!(check_csv(b"a,b\n1,2\n3,4\n").unwrap(), (2, 2));
    assert_eq!(check_csv(b"a,b\r\n1,2").unwrap(), (1, 2), "no final new line, CRLF");
    assert_eq!(check_csv(b"a\n\"x,\"\"y\"\"\nz\"\n").unwrap(), (1, 1), "quotes hold commas, doubled quotes and new lines");
    assert_eq!(check_csv("\u{feff}n\n1\n".as_bytes()).unwrap(), (1, 1), "a byte order mark is not a column");
    assert!(check_csv(b"").unwrap_err().contains("empty"));
    assert!(check_csv(b"a,b\n1\n").unwrap_err().contains("row 2 has 1 columns but the header has 2"));
    assert!(check_csv(b"a,b\n\"1,2\n").unwrap_err().contains("never closed"));
    assert!(check_csv(&[0xff, 0xfe, b'a']).unwrap_err().contains("UTF-8"));
}

#[test]
fn only_the_wanted_files_are_collected_checked_by_kind_and_handed_to_the_data_sink() {
    let d = tmp("collect");
    let (work, keep) = (d.join("work"), d.join("keep"));
    std::fs::create_dir_all(work.join("out")).unwrap();
    let out = work.join("out");
    std::fs::write(out.join("set.csv"), "f1,label\n0.1,0\n0.9,1\n").unwrap();
    std::fs::write(out.join("broken.csv"), "a,b\n1\n").unwrap();
    std::fs::write(out.join("plot.png"), [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1]).unwrap();
    std::fs::write(out.join("fake.png"), "not a picture").unwrap();
    std::fs::write(out.join("notes.txt"), "kept").unwrap();
    std::fs::write(out.join("unlisted.txt"), "never collected").unwrap();
    let io = RunIo {
        outputs: vec![
            Want { file: "set.csv".into(), kind: Kind::Table },
            Want { file: "broken.csv".into(), kind: Kind::Table },
            Want { file: "plot.png".into(), kind: Kind::Image },
            Want { file: "fake.png".into(), kind: Kind::Image },
            Want { file: "notes.txt".into(), kind: Kind::File },
            Want { file: "missing.csv".into(), kind: Kind::Table },
        ],
        ..Default::default()
    };
    let sink = Rec::default();
    let notes = collect(&work, &io, &keep, &sink).unwrap();
    assert!(notes[0].contains("table, 2 rows, 2 columns") && notes[0].contains("Data tab"), "{notes:?}");
    assert!(notes[1].contains("not a table") && notes[1].contains("row 2"), "{notes:?}");
    assert!(notes[2].contains("picture") && notes[3].contains("not a JPEG or PNG"), "{notes:?}");
    assert_eq!(notes[4], "notes.txt (4 bytes)");
    assert!(notes[5].contains("did not make it"));
    assert_eq!(*sink.0.lock().unwrap(), ["table set.csv 2x2", "image plot.png"], "the broken table and the fake picture never reach the sink");
    assert!(keep.join("set.csv").is_file() && keep.join("notes.txt").is_file() && !keep.join("unlisted.txt").exists() && !keep.join("broken.csv").exists());
    // a wanted name must be plain, so a script cannot name a path outside out/
    let bad = RunIo { outputs: vec![Want { file: "../in/secret".into(), kind: Kind::File }], ..Default::default() };
    assert!(collect(&work, &bad, &keep, &sink).is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc/hostname", out.join("link.txt")).unwrap();
        let l = RunIo { outputs: vec![Want { file: "link.txt".into(), kind: Kind::File }], ..Default::default() };
        assert!(collect(&work, &l, &keep, &sink).unwrap()[0].contains("not a plain file"));
        assert!(!keep.join("link.txt").exists());
    }
    let _ = std::fs::remove_dir_all(&d);
}
