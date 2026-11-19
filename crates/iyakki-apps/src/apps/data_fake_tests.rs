//! The DATA tile against a fake port: the environment is a temp folder and the person is "ann"; no `AppCx`, no services (the fifth tile moved onto
//! the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12). The view itself is tested against the same kind of environment in `mod tests`.

use super::*;
use std::path::PathBuf;

struct FakePort {
    dir: PathBuf,
    read_only: bool,
}

impl DataPort for FakePort {
    fn env(&self) -> DataEnv {
        let (d1, d2, d3) = (self.dir.to_string_lossy().into_owned(), self.dir.to_string_lossy().into_owned(), self.dir.clone());
        let ro = self.read_only;
        let r = iyakki_filescope::FileRoots { home: d3.clone(), roots: vec![d3], mounts: vec![] };
        DataEnv { svc: iyakki_data::builtin_service(), can_read: Arc::new(move |p| p.starts_with(&d1)), can_write: Arc::new(move |p| !ro && p.starts_with(&d2)), roots: Arc::new(move || r.clone()), recent: Some(self.dir.join(".recent.json")), host: None }
    }
    fn user(&self) -> String {
        "ann".into()
    }
}

fn setup(tag: &str, read_only: bool) -> (FakePort, String) {
    let dir = std::fs::canonicalize(crate::apps::tests::temp_dir(tag)).unwrap();
    let mut t = String::from("id,name,score\n");
    for i in 0..40 {
        t.push_str(&format!("{i},\"user {i}\",{}\n", i * 3));
    }
    let f = dir.join("t.csv");
    std::fs::write(&f, t).unwrap();
    (FakePort { dir, read_only }, f.to_string_lossy().into_owned())
}

#[test]
fn the_tile_opens_the_file_it_is_asked_to_from_the_environment_the_port_gives_and_tells_the_assistant_the_columns_of_that_person() {
    let (p, file) = setup("data-fake-open", false);
    request_open(&(file.clone()), None);
    let mut a = DataApp::new(&p);
    for _ in 0..200 {
        a.view().settle();
        a.tell_assistant();
        if a.view().total() == 40 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(a.view().total(), 40);
    assert_eq!(a.view().column_names(), ["id", "name", "score"]);
    assert_eq!(a.told.as_ref().map(|t| t.0.as_str()), Some(file.as_str()), "the assistant was told the file of this person");
    assert_eq!(a.told.as_ref().map(|t| t.1.clone()), Some(vec!["id".to_string(), "name".to_string(), "score".to_string()]));
}

#[test]
fn a_file_outside_what_the_port_allows_is_not_opened() {
    let (p, _) = setup("data-fake-refuse", false);
    let outside = std::env::temp_dir().join(format!("iyakki-data-outside-{}.csv", std::process::id()));
    std::fs::write(&outside, "a,b\n1,2\n").unwrap();
    request_open(&(outside.to_string_lossy().into_owned()), None);
    let mut a = DataApp::new(&p);
    a.view().settle();
    assert_eq!(a.view().total(), 0, "nothing was opened");
    let _ = std::fs::remove_file(&outside);
}

#[test]
fn the_tile_draws_from_what_the_port_gave_it_at_the_usual_sizes() {
    let (p, file) = setup("data-fake-draw", true);
    request_open(&(file), None);
    let mut a = DataApp::new(&p);
    a.view().settle();
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720)] {
        let mut g = Gfx::new(w, h);
        a.view().draw(&mut g, R::new(0.0, 0.0, w as f32, h as f32), 1.0, 1);
    }
}
