//! The DEV TOOLS tile over a fake port: no session, no file system, no sockets, no real clipboard. What the tile asks of the rest of the system is
//! exactly the `DevToolsPort` calls.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::port::{DevToolsPort, NetJob};
use super::*;

struct FakeJob {
    text: Mutex<String>,
    done: AtomicBool,
    stopped: AtomicBool,
}

impl NetJob for FakeJob {
    fn finished(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }
    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }
    fn text(&self) -> String {
        self.text.lock().unwrap().clone()
    }
}

#[derive(Default)]
struct FakePort {
    files: Vec<(String, String)>,
    clip: RefCell<Option<String>>,
    audit: RefCell<Vec<(String, String)>>,
    saved: RefCell<Vec<(PathBuf, String, usize)>>,
    job: Mutex<Option<Arc<FakeJob>>>,
}

impl DevToolsPort for FakePort {
    fn read_text_file(&self, path: &str, max: u64) -> Result<String, String> {
        match self.files.iter().find(|(p, _)| p == path) {
            Some((_, t)) if t.len() as u64 <= max => Ok(t.clone()),
            Some(_) => Err(format!("{path} is too big")),
            None => Err(format!("This account may not read {path}.")),
        }
    }
    fn home_dir(&self) -> PathBuf {
        PathBuf::from("/home/ada")
    }
    fn save_picture(&self, dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
        self.saved.borrow_mut().push((dir.to_path_buf(), name.to_string(), bytes.len()));
        Ok(dir.join(name))
    }
    fn png_with_dpi(&self, w: u32, h: u32, _rgba: &[u8], _dpi: f32) -> Result<Vec<u8>, String> {
        Ok(vec![0x89; (w * h) as usize % 7 + 8])
    }
    fn stamp(&self) -> String {
        "20261105-143012".into()
    }
    fn audit(&self, action: &str, detail: &str) {
        self.audit.borrow_mut().push((action.into(), detail.into()));
    }
    fn start_ping(&self, host: &str, _count: usize) -> Result<Arc<dyn NetJob>, String> {
        let j = Arc::new(FakeJob { text: Mutex::new(format!("PING {host}")), done: AtomicBool::new(false), stopped: AtomicBool::new(false) });
        *self.job.lock().unwrap() = Some(j.clone());
        Ok(j)
    }
    fn start_ports(&self, host: &str, spec: &str) -> Result<Arc<dyn NetJob>, String> {
        if spec == "bad" {
            return Err("'bad' is not a port".into());
        }
        self.start_ping(host, 0)
    }
    fn clipboard_paste(&self) -> Option<String> {
        self.clip.borrow().clone()
    }
    fn clipboard_copy(&self, text: &str) {
        *self.clip.borrow_mut() = Some(text.to_string());
    }
}

fn run(app: &mut DevToolsApp, port: &FakePort, tool: usize, input: &str, act: usize) {
    app.select(tool);
    app.input.set(input);
    app.press(T_ACT + act, port);
}

#[test]
fn a_text_tool_runs_without_touching_the_port() {
    let (mut a, p) = (DevToolsApp::new(), FakePort::default());
    run(&mut a, &p, 0, "{\"b\":1,\"a\":2}", 3);
    assert!(!a.out.err && a.out.text.find("\"a\"") < a.out.text.find("\"b\""), "{}", a.out.text);
    assert!(p.audit.borrow().is_empty() && p.saved.borrow().is_empty());
}

#[test]
fn at_path_reads_the_file_through_the_port_and_a_refusal_is_the_output() {
    let (mut a, mut p) = (DevToolsApp::new(), FakePort::default());
    p.files.push(("/data/x.json".into(), "{\"k\":[1,2]}".into()));
    run(&mut a, &p, 0, "@/data/x.json", 1);
    assert!(!a.out.err && a.out.text.contains("[1,2]"), "{}", a.out.text);
    run(&mut a, &p, 0, "@/secret/y.json", 0);
    assert!(a.out.err && a.out.text.contains("may not read /secret/y.json"));
}

#[test]
fn paste_and_copy_go_through_the_port() {
    let (mut a, p) = (DevToolsApp::new(), FakePort::default());
    a.press(T_PASTE, &p);
    assert!(a.out.err && a.out.text.contains("no text to paste"));
    *p.clip.borrow_mut() = Some("hello".into());
    a.press(T_PASTE, &p);
    assert_eq!(a.input.text, "hello");
    run(&mut a, &p, 2, "hi", 0);
    a.press(T_COPY, &p);
    assert_eq!(p.clip.borrow().as_deref(), Some(a.out.text.as_str()));
}

#[test]
fn ping_follows_the_job_the_port_starts_and_stop_stops_it() {
    let (mut a, p) = (DevToolsApp::new(), FakePort::default());
    run(&mut a, &p, tools::PING, "example.org", 0);
    assert_eq!(a.out.text, "PING example.org");
    let job = p.job.lock().unwrap().clone().unwrap();
    *job.text.lock().unwrap() = "PING example.org\nreply time=3 ms".into();
    assert!(a.poll_job() && a.out.text.contains("time=3"));
    a.press(T_STOP, &p);
    assert!(job.stopped.load(Ordering::SeqCst));
    job.done.store(true, Ordering::SeqCst);
    assert!(a.poll_job() && a.job.is_none());
}

#[test]
fn a_port_check_the_port_refuses_is_an_error_line() {
    let (mut a, p) = (DevToolsApp::new(), FakePort::default());
    a.select(tools::PORTS);
    a.input.set("example.org");
    a.aux.set("bad");
    a.press(T_ACT, &p);
    assert!(a.out.err && a.out.text.contains("not a port") && a.job.is_none());
}

#[test]
fn saving_a_qr_picture_names_the_stamp_the_pictures_folder_and_audits_the_file_only() {
    let (mut a, p) = (DevToolsApp::new(), FakePort::default());
    a.select(tools::QR);
    a.input.set("https://example.org");
    a.press(T_SAVE_SVG, &p);
    a.press(T_SAVE_PNG, &p);
    let saved = p.saved.borrow();
    assert_eq!(saved.len(), 2, "{:?}", a.qr.note);
    assert_eq!(saved[0].0, PathBuf::from("/home/ada/Pictures"));
    assert_eq!(saved[0].1, "qr-20261105-143012.svg");
    assert_eq!(saved[1].1, "qr-20261105-143012.png");
    let audit = p.audit.borrow();
    assert_eq!(audit[0].0, "devtools.qr.save");
    assert!(!audit[0].1.contains("example.org"), "what the code holds is never logged: {}", audit[0].1);
    assert!(a.qr.note.as_ref().is_some_and(|(t, err)| t.starts_with("Saved qr-") && !err));
}

#[test]
fn saving_with_nothing_typed_says_so_and_writes_nothing() {
    let (mut a, p) = (DevToolsApp::new(), FakePort::default());
    a.select(tools::QR);
    a.press(T_SAVE_PNG, &p);
    assert!(p.saved.borrow().is_empty());
    assert!(a.qr.note.as_ref().is_some_and(|(_, err)| *err));
}

#[test]
fn the_tile_draws_every_tool_without_a_port() {
    let mut a = DevToolsApp::new();
    let mut g = iyakki_uikit::gfx::Gfx::new(1000, 600);
    for t in 0..TOOLS.len() {
        a.select(t);
        g.clear_hits();
        a.render(&mut g, iyakki_uikit::ui::R::new(0.0, 0.0, 1000.0, 600.0), 1.0, 1);
        assert!(!g.hits.is_empty(), "tool {t} drew nothing clickable");
    }
}

#[test]
fn escape_from_the_list_goes_back_and_elsewhere_steps_to_the_list() {
    let (mut a, p) = (DevToolsApp::new(), FakePort::default());
    assert_eq!(a.handle(&Event::Key(Key::Esc), &p), Flow::Stay);
    assert_eq!(a.focus, Focus::List);
    assert_eq!(a.handle(&Event::Key(Key::Esc), &p), Flow::Back);
}
