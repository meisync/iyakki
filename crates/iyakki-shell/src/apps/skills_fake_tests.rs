//! The SKILLS tile against a fake port: no skill store, no package store, no file, no `AppCx` (the fourth tile moved onto the app standard,
//! docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12). The same behaviours are proved through the real port in `mod tests`.

use super::*;
use iyakki_agent::pkg::{Installed, PkgError};
use iyakki_agent::skills::{Skill, SkillError};
use std::sync::Mutex;

struct FakePort {
    log: Mutex<Vec<String>>,
    skills: Mutex<Vec<Skill>>,
    keys: Mutex<Vec<(String, String)>>,
    own: bool,
    shared: bool,
    add_error: Mutex<Option<String>>,
    file: Mutex<Result<String, String>>,
}

impl FakePort {
    fn new(own: bool, shared: bool) -> FakePort {
        let sk = |scope: Scope, n: &str| Skill { scope, name: n.into(), description: format!("about {n}"), triggers: vec!["tea".into()], body: "Do the thing.".into() };
        FakePort { log: Mutex::default(), skills: Mutex::new(vec![sk(Scope::User("ann".into()), "mine-one"), sk(Scope::Shared, "team-one")]), keys: Mutex::default(), own, shared, add_error: Mutex::new(None), file: Mutex::new(Err("no such file".into())) }
    }
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

impl SkillsPort for FakePort {
    fn user(&self) -> String {
        "ann".into()
    }
    fn can_own(&self) -> bool {
        self.own
    }
    fn can_shared(&self) -> bool {
        self.shared
    }
    fn list(&self) -> Vec<Skill> {
        let mut v = self.skills.lock().unwrap().clone();
        v.sort_by(|a, b| (matches!(a.scope, Scope::Shared), &a.name).cmp(&(matches!(b.scope, Scope::Shared), &b.name)));
        v
    }
    fn add(&self, scope: Scope, markdown: &str) -> Result<Skill, SkillError> {
        if let Some(e) = self.add_error.lock().unwrap().clone() {
            return Err(SkillError::Invalid(e));
        }
        if scope == Scope::Shared && !self.shared {
            return Err(SkillError::Forbidden("shared"));
        }
        let name = markdown.lines().find_map(|l| l.strip_prefix("name: ")).unwrap_or("x").to_string();
        let s = Skill { scope, name, description: "d".into(), triggers: vec![], body: markdown.to_string() };
        self.log.lock().unwrap().push(format!("add {}", s.name));
        self.skills.lock().unwrap().push(s.clone());
        Ok(s)
    }
    fn review(&self, text: &str, _: &Scope) -> Result<iyakki_agent::pkg::Review, PkgError> {
        self.log.lock().unwrap().push(format!("review {}", text.len()));
        Err(PkgError::Invalid("not a package this test knows".into()))
    }
    fn install(&self, _: &iyakki_agent::pkg::Review) -> Result<Skill, PkgError> {
        Err(PkgError::NotApproved)
    }
    fn remove(&self, scope: &Scope, name: &str) -> Result<(), PkgError> {
        self.log.lock().unwrap().push(format!("remove {name}"));
        self.skills.lock().unwrap().retain(|s| !(s.name == name && &s.scope == scope));
        Ok(())
    }
    fn trust_key(&self, key: &str, name: &str) -> Result<(), PkgError> {
        if key.len() != 64 {
            return Err(PkgError::Invalid("a key is 64 hex characters".into()));
        }
        self.log.lock().unwrap().push(format!("trust {name}"));
        self.keys.lock().unwrap().push((key.into(), name.into()));
        Ok(())
    }
    fn trusted_keys(&self) -> Vec<(String, String)> {
        self.keys.lock().unwrap().clone()
    }
    fn installed(&self, _: &Scope, name: &str) -> Option<(String, Installed)> {
        (name == "team-one").then(|| (name.to_string(), Installed { version: "1.2".into(), sha256: "ab".repeat(32), signer: Some("cd".repeat(32)) }))
    }
    fn read_text_file(&self, _path: &str) -> Result<String, String> {
        self.file.lock().unwrap().clone()
    }
    fn audit(&self, action: &str, subject: &str) {
        self.log.lock().unwrap().push(format!("audit {action} {subject}"));
    }
}

fn key(a: &mut SkillsApp, p: &FakePort, k: Key) {
    a.handle_with(&Event::Key(k), p);
}

fn typ(a: &mut SkillsApp, p: &FakePort, text: &str) {
    for c in text.chars() {
        key(a, p, Key::Char(c));
    }
}

#[test]
fn a_new_skill_is_made_from_the_form_through_the_port_with_one_audit_line_and_a_refusal_stays_in_the_form() {
    let p = FakePort::new(true, false);
    let mut a = SkillsApp::new(&p);
    key(&mut a, &p, Key::Char('n'));
    typ(&mut a, &p, "kettle");
    key(&mut a, &p, Key::Tab);
    typ(&mut a, &p, "boil water first");
    key(&mut a, &p, Key::Tab);
    typ(&mut a, &p, "tea");
    key(&mut a, &p, Key::Tab);
    typ(&mut a, &p, "Fill, boil, pour.");
    *p.add_error.lock().unwrap() = Some("a skill needs a longer description".into());
    key(&mut a, &p, Key::Enter);
    assert!(matches!(a.mode, Mode::Form(..)), "the form stays open with the reason");
    assert!(!p.log().iter().any(|l| l.starts_with("add ")));
    *p.add_error.lock().unwrap() = None;
    key(&mut a, &p, Key::Enter);
    assert!(matches!(a.mode, Mode::Browse));
    assert!(p.log().contains(&"add kettle".to_string()) && p.log().contains(&"audit skill.add kettle".to_string()), "{:?}", p.log());
    assert!(a.status_text().unwrap().contains("Added the skill"), "{:?}", a.status_text());
}

#[test]
fn deleting_asks_first_only_y_deletes_and_a_shared_skill_is_not_deleted_by_one_who_may_not() {
    let p = FakePort::new(true, false);
    let mut a = SkillsApp::new(&p);
    key(&mut a, &p, Key::Char('d'));
    assert!(matches!(a.mode, Mode::Confirm(..)));
    key(&mut a, &p, Key::Char('x'));
    assert!(!p.log().iter().any(|l| l.starts_with("remove ")), "any other key is a no");
    key(&mut a, &p, Key::Char('d'));
    key(&mut a, &p, Key::Char('y'));
    assert!(p.log().contains(&"remove mine-one".to_string()) && p.log().contains(&"audit skill.delete mine-one".to_string()), "{:?}", p.log());
    // what is left is the shared one: this account may not delete it
    key(&mut a, &p, Key::Char('d'));
    assert!(matches!(a.mode, Mode::Browse));
    assert!(a.status_text().unwrap().contains("Only operators and administrators"), "{:?}", a.status_text());
    assert!(!p.log().iter().any(|l| l == "remove team-one"));
    // an account that manages the shared skills is asked and may
    let p = FakePort::new(true, true);
    let mut a = SkillsApp::new(&p);
    key(&mut a, &p, Key::Down);
    key(&mut a, &p, Key::Char('d'));
    assert!(matches!(a.mode, Mode::Confirm(Scope::Shared, _)));
}

#[test]
fn a_file_is_read_through_the_port_only_and_a_package_text_goes_to_review() {
    let p = FakePort::new(true, false);
    let mut a = SkillsApp::new(&p);
    key(&mut a, &p, Key::Char('l'));
    typ(&mut a, &p, "/home/ann/skill.md");
    key(&mut a, &p, Key::Ctrl('s'));
    assert!(matches!(a.mode, Mode::Form(..)), "the port refused the file: the form stays with the reason");
    *p.file.lock().unwrap() = Ok("IYAKKI-PKG {\"not\":\"real\"}".into());
    key(&mut a, &p, Key::Ctrl('s'));
    assert!(p.log().iter().any(|l| l.starts_with("review ")), "{:?}", p.log());
    assert!(a.status_text().unwrap().contains("cannot be installed"), "{:?}", a.status_text());
    // plain skill text from a file is added
    let mut a = SkillsApp::new(&p);
    *p.file.lock().unwrap() = Ok("---\nname: fromfile\ndescription: d\n---\nbody\n".into());
    key(&mut a, &p, Key::Char('l'));
    typ(&mut a, &p, "/home/ann/skill.md");
    key(&mut a, &p, Key::Ctrl('s'));
    assert!(p.log().contains(&"add fromfile".to_string()), "{:?} {:?} in a form: {}", p.log(), a.status_text(), matches!(a.mode, Mode::Form(..)));
}

#[test]
fn only_an_account_that_manages_shared_skills_trusts_a_publisher_and_the_trust_is_audited() {
    let p = FakePort::new(true, false);
    let mut a = SkillsApp::new(&p);
    key(&mut a, &p, Key::Char('k'));
    assert!(matches!(a.mode, Mode::Keys));
    key(&mut a, &p, Key::Char('t'));
    assert!(a.status_text().unwrap().contains("Only people who manage shared skills"), "{:?}", a.status_text());
    let p = FakePort::new(true, true);
    let mut a = SkillsApp::new(&p);
    key(&mut a, &p, Key::Char('k'));
    key(&mut a, &p, Key::Char('t'));
    typ(&mut a, &p, &"ab".repeat(32));
    key(&mut a, &p, Key::Tab);
    typ(&mut a, &p, "Acme");
    key(&mut a, &p, Key::Enter);
    let form_err = if let Mode::Form(f, _) = &a.mode { f.error.clone() } else { None };
    assert!(p.log().contains(&"trust Acme".to_string()) && p.log().iter().any(|l| l.starts_with("audit skill.publisher.trust ")), "{:?} {:?} {:?}", p.log(), a.status_text(), form_err);
    assert_eq!(p.trusted_keys().len(), 1);
}

#[test]
fn the_tile_draws_the_list_a_package_and_the_publishers_at_the_usual_sizes_from_the_port() {
    use crate::gfx::Gfx;
    let p = FakePort::new(true, true);
    p.keys.lock().unwrap().push(("ef".repeat(32), "Acme".into()));
    let mut a = SkillsApp::new(&p);
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720)] {
        let mut g = Gfx::new(w, h);
        a.render_with(&mut g, R::new(0.0, 0.0, w as f32, h as f32), 1.0, &p, 2);
    }
    key(&mut a, &p, Key::Down);
    let mut g = Gfx::new(1280, 720);
    a.render_with(&mut g, R::new(0.0, 0.0, 1280.0, 720.0), 1.0, &p, 2);
    let seen: String = g.layout_log.texts.iter().map(|t| t.3.clone()).collect::<Vec<_>>().join(" ");
    assert!(seen.contains("package 1.2"), "the package the port knows is shown: {seen}");
    key(&mut a, &p, Key::Char('k'));
    let mut g = Gfx::new(900, 600);
    a.render_with(&mut g, R::new(0.0, 0.0, 900.0, 600.0), 1.0, &p, 2);
    assert!(g.layout_log.texts.iter().any(|t| t.3.contains("Acme")));
}
