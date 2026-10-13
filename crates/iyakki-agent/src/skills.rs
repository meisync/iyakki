//! Skills: reusable instructions the assistant follows when they are relevant.
//!
//! A skill is a Markdown file with a small header:
//!
//! ```text
//! ---
//! name: meeting-notes
//! description: Turn raw meeting notes into decisions and action items
//! triggers: meeting, minutes, notes
//! ---
//! Instructions in Markdown…
//! ```
//!
//! Files are the source of truth (`<data>/skills/shared/*.md`, `<data>/skills/user/<name>/*.md`);
//! name, description and triggers are indexed in the knowledge base so the best-matching skills are
//! picked for each request. Shared skills reach every user's prompts, so only roles with
//! `manage_skills` may write them; anyone with `skills` may keep their own.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::kb::KnowledgeBase;

pub const MAX_BODY_BYTES: usize = 16 * 1024;
pub const MAX_PER_SCOPE: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    Shared,
    User(String),
}

impl Scope {
    fn ns(&self) -> String {
        match self {
            Scope::Shared => "skills/shared".into(),
            Scope::User(u) => format!("skills/user/{u}"),
        }
    }
    pub fn label(&self) -> &str {
        match self {
            Scope::Shared => "shared",
            Scope::User(_) => "mine",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skill {
    pub scope: Scope,
    pub name: String,
    pub description: String,
    pub triggers: Vec<String>,
    pub body: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SkillError {
    #[error("{0}")]
    Invalid(String),
    #[error("you may not change {0} skills")]
    Forbidden(&'static str),
    #[error("no such skill")]
    NotFound,
    #[error("storage error: {0}")]
    Storage(String),
}

fn bad<T>(m: impl Into<String>) -> Result<T, SkillError> {
    Err(SkillError::Invalid(m.into()))
}

pub fn valid_skill_name(n: &str) -> bool {
    let mut c = n.chars();
    c.next().is_some_and(|f| f.is_ascii_lowercase() || f.is_ascii_digit()) && n.len() <= 48 && c.all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || x == '-')
}

/// Parse skill Markdown (front matter + body). The scope is supplied by the caller, never by the file.
pub fn parse(markdown: &str, scope: Scope) -> Result<Skill, SkillError> {
    let text = markdown.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let rest = text.strip_prefix("---\n").ok_or_else(|| SkillError::Invalid("a skill starts with a `---` header containing name and description".into()))?;
    let (header, body) = rest.split_once("\n---").ok_or_else(|| SkillError::Invalid("the header must end with a line containing only `---`".into()))?;
    let body = body.trim_start_matches(['-', ' ']).trim_start_matches('\n').trim().to_string();
    let (mut name, mut description, mut triggers) = (String::new(), String::new(), Vec::new());
    for line in header.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let v = v.trim().trim_matches('"').trim();
        match k.trim() {
            "name" => name = v.to_string(),
            "description" => description = v.to_string(),
            "triggers" => triggers = v.split(',').map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty()).take(12).collect(),
            _ => {}
        }
    }
    if !valid_skill_name(&name) {
        return bad("name: lowercase letters, digits and `-`, starting with a letter or digit, at most 48 characters");
    }
    if description.is_empty() || description.chars().count() > 300 {
        return bad("description is required and must be at most 300 characters");
    }
    if body.is_empty() {
        return bad("the skill has no instructions after the header");
    }
    if body.len() > MAX_BODY_BYTES {
        return bad(format!("instructions are larger than {} KiB", MAX_BODY_BYTES / 1024));
    }
    if body.chars().any(|c| c == '\0') {
        return bad("instructions contain invalid characters");
    }
    Ok(Skill { scope, name, description, triggers, body })
}

pub fn render(s: &Skill) -> String {
    let mut h = format!("---\nname: {}\ndescription: {}\n", s.name, s.description);
    if !s.triggers.is_empty() {
        h.push_str(&format!("triggers: {}\n", s.triggers.join(", ")));
    }
    format!("{h}---\n{}\n", s.body)
}

/// The text the knowledge base indexes for matching.
fn index_text(s: &Skill) -> String {
    format!("{}. {}. {}", s.name.replace('-', " "), s.description, s.triggers.join(" "))
}

pub struct SkillStore {
    dir: PathBuf,
    kb: Arc<Mutex<KnowledgeBase>>,
}

impl SkillStore {
    pub fn new(dir: &Path, kb: Arc<Mutex<KnowledgeBase>>) -> SkillStore {
        let s = SkillStore { dir: dir.into(), kb };
        s.sync_index();
        s
    }

    fn scope_dir(&self, scope: &Scope) -> PathBuf {
        match scope {
            Scope::Shared => self.dir.join("shared"),
            Scope::User(u) => self.dir.join("user").join(u),
        }
    }

    fn check_user(scope: &Scope) -> Result<(), SkillError> {
        match scope {
            Scope::User(u) if !iyakki_sec::policy::valid_username(u) => bad("invalid user"),
            _ => Ok(()),
        }
    }

    /// Re-index every skill file (cheap: unchanged text is deduplicated by the knowledge base).
    pub fn sync_index(&self) {
        let mut scopes = vec![Scope::Shared];
        if let Ok(rd) = std::fs::read_dir(self.dir.join("user")) {
            scopes.extend(rd.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).filter(|u| iyakki_sec::policy::valid_username(u)).map(Scope::User));
        }
        for sc in scopes {
            for sk in self.list_scope(&sc) {
                self.index(&sk);
            }
        }
    }

    fn index(&self, s: &Skill) {
        let mut kb = self.kb.lock().unwrap();
        let ns = s.scope.ns();
        // replace any older version of this skill's index entry
        for d in kb.docs(&ns).into_iter().filter(|d| d.title == s.name) {
            let _ = kb.remove_document(&ns, &d.id);
        }
        let _ = kb.add_document(&ns, &s.name, &format!("{}.md", s.name), &index_text(s), Default::default());
    }

    pub fn list_scope(&self, scope: &Scope) -> Vec<Skill> {
        if Self::check_user(scope).is_err() {
            return vec![];
        }
        let mut v: Vec<Skill> = std::fs::read_dir(self.scope_dir(scope))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
            .filter_map(|e| parse(&std::fs::read_to_string(e.path()).ok()?, scope.clone()).ok())
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// Skills visible to `user`: theirs first, then shared ones they haven't overridden.
    pub fn list(&self, user: &str) -> Vec<Skill> {
        let mine = self.list_scope(&Scope::User(user.into()));
        let mut all = mine.clone();
        all.extend(self.list_scope(&Scope::Shared).into_iter().filter(|s| !mine.iter().any(|m| m.name == s.name)));
        all
    }

    pub fn get(&self, scope: &Scope, name: &str) -> Option<Skill> {
        if !valid_skill_name(name) || Self::check_user(scope).is_err() {
            return None;
        }
        parse(&std::fs::read_to_string(self.scope_dir(scope).join(format!("{name}.md"))).ok()?, scope.clone()).ok()
    }

    /// Add or replace a skill. `can_shared` / `can_own` come from the caller's policy.
    pub fn add(&self, scope: Scope, markdown: &str, can_shared: bool, can_own: bool) -> Result<Skill, SkillError> {
        use std::os::unix::fs::OpenOptionsExt;
        match &scope {
            Scope::Shared if !can_shared => return Err(SkillError::Forbidden("shared")),
            Scope::User(_) if !can_own => return Err(SkillError::Forbidden("your own")),
            _ => {}
        }
        Self::check_user(&scope)?;
        let skill = parse(markdown, scope.clone())?;
        let dir = self.scope_dir(&scope);
        if self.list_scope(&scope).len() >= MAX_PER_SCOPE && self.get(&scope, &skill.name).is_none() {
            return bad(format!("too many skills (limit {MAX_PER_SCOPE})"));
        }
        std::fs::create_dir_all(&dir).map_err(|e| SkillError::Storage(e.to_string()))?;
        let path = dir.join(format!("{}.md", skill.name));
        let tmp = dir.join(format!("{}.tmp", skill.name));
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| SkillError::Storage(e.to_string()))?;
            f.write_all(render(&skill).as_bytes()).map_err(|e| SkillError::Storage(e.to_string()))?;
        }
        std::fs::rename(&tmp, &path).map_err(|e| SkillError::Storage(e.to_string()))?;
        self.index(&skill);
        Ok(skill)
    }

    pub fn remove(&self, scope: &Scope, name: &str, can_shared: bool, can_own: bool) -> Result<(), SkillError> {
        match scope {
            Scope::Shared if !can_shared => return Err(SkillError::Forbidden("shared")),
            Scope::User(_) if !can_own => return Err(SkillError::Forbidden("your own")),
            _ => {}
        }
        Self::check_user(scope)?;
        if !valid_skill_name(name) {
            return Err(SkillError::NotFound);
        }
        std::fs::remove_file(self.scope_dir(scope).join(format!("{name}.md"))).map_err(|_| SkillError::NotFound)?;
        let mut kb = self.kb.lock().unwrap();
        let ns = scope.ns();
        for d in kb.docs(&ns).into_iter().filter(|d| d.title == name) {
            let _ = kb.remove_document(&ns, &d.id);
        }
        Ok(())
    }

    /// The best-matching skills for `query` (at most `k`), personal ones winning name clashes.
    pub fn select(&self, user: &str, query: &str, k: usize) -> Vec<Skill> {
        if query.trim().is_empty() || k == 0 || !iyakki_sec::policy::valid_username(user) {
            return vec![];
        }
        let mine = format!("skills/user/{user}");
        let hits = self.kb.lock().unwrap().search_weighted(&[("skills/shared", 1.0), (mine.as_str(), 1.2)], query, k * 3);
        let mut out: Vec<Skill> = Vec::new();
        for h in hits {
            let scope = if h.ns == "skills/shared" { Scope::Shared } else { Scope::User(user.into()) };
            if out.iter().any(|s| s.name == h.title) {
                continue;
            }
            if let Some(s) = self.get(&scope, &h.title) {
                out.push(s);
            }
            if out.len() == k {
                break;
            }
        }
        out
    }
}

/// System-prompt block for the chosen skills ("" if none).
pub fn prompt_block(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut s = String::from("The user has these skills enabled. Follow them when they apply to the request:\n");
    for k in skills {
        let body: String = k.body.chars().take(4000).collect();
        s.push_str(&format!("\n## Skill: {}\n{}\n", k.name, body));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::HashEmbed;

    fn store(tag: &str) -> (SkillStore, PathBuf) {
        let d = std::env::temp_dir().join(format!("iyakki-skills-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        let kb = Arc::new(Mutex::new(KnowledgeBase::in_memory(Arc::new(HashEmbed::default()))));
        (SkillStore::new(&d, kb), d)
    }

    const MEETING: &str = "---\nname: meeting-notes\ndescription: Turn raw meeting notes into decisions and action items\ntriggers: meeting, minutes\n---\nList decisions first, then action items with owners.\n";
    const RECIPE: &str = "---\nname: recipe-format\ndescription: Format cooking recipes with ingredients and numbered steps\n---\nUse an Ingredients list then numbered Steps.\n";

    #[test]
    fn parsing_and_validation() {
        let s = parse(MEETING, Scope::Shared).unwrap();
        assert_eq!((s.name.as_str(), s.triggers.clone()), ("meeting-notes", vec!["meeting".to_string(), "minutes".to_string()]));
        assert_eq!(s.body, "List decisions first, then action items with owners.");
        assert_eq!(parse(&render(&s), Scope::Shared).unwrap(), s, "render/parse round-trips");
        // CRLF and quoted values are accepted
        assert!(parse("---\r\nname: \"x-y\"\r\ndescription: \"d\"\r\n---\r\nbody\r\n", Scope::Shared).is_ok());
        for (bad, why) in [
            ("no header", "starts with"),
            ("---\nname: a\ndescription: d\nno end", "end with"),
            ("---\nname: Bad Name\ndescription: d\n---\nb", "name:"),
            ("---\nname: ../x\ndescription: d\n---\nb", "name:"),
            ("---\nname: ok\n---\nb", "description"),
            ("---\nname: ok\ndescription: d\n---\n   \n", "no instructions"),
        ] {
            let e = parse(bad, Scope::Shared).unwrap_err().to_string();
            assert!(e.contains(why), "{bad:?} -> {e}");
        }
        let huge = format!("---\nname: big\ndescription: d\n---\n{}", "x".repeat(MAX_BODY_BYTES + 1));
        assert!(parse(&huge, Scope::Shared).unwrap_err().to_string().contains("larger"));
        let long_desc = format!("---\nname: big\ndescription: {}\n---\nb", "d".repeat(301));
        assert!(parse(&long_desc, Scope::Shared).is_err());
        // a file cannot claim a scope: the caller decides
        assert_eq!(parse("---\nname: a\ndescription: d\nscope: shared\n---\nb", Scope::User("amy".into())).unwrap().scope, Scope::User("amy".into()));
    }

    #[test]
    fn add_list_get_remove_with_permissions() {
        let (st, d) = store("perm");
        let amy = Scope::User("amy".into());
        assert_eq!(st.add(Scope::Shared, MEETING, false, true).unwrap_err(), SkillError::Forbidden("shared"));
        assert_eq!(st.add(amy.clone(), RECIPE, true, false).unwrap_err(), SkillError::Forbidden("your own"));
        st.add(Scope::Shared, MEETING, true, true).unwrap();
        st.add(amy.clone(), RECIPE, false, true).unwrap();
        let names: Vec<String> = st.list("amy").into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["recipe-format", "meeting-notes"], "personal first, then shared");
        assert_eq!(st.list("bob").iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["meeting-notes"], "bob does not see amy's skill");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(d.join("user/amy/recipe-format.md")).unwrap().permissions().mode() & 0o777, 0o600);
        // removing needs the matching permission; unknown / malicious names are just "not found"
        assert_eq!(st.remove(&Scope::Shared, "meeting-notes", false, true).unwrap_err(), SkillError::Forbidden("shared"));
        assert_eq!(st.remove(&amy, "../../etc/passwd", false, true).unwrap_err(), SkillError::NotFound);
        st.remove(&amy, "recipe-format", false, true).unwrap();
        assert!(st.get(&amy, "recipe-format").is_none());
        assert_eq!(st.remove(&amy, "recipe-format", false, true).unwrap_err(), SkillError::NotFound);
        // invalid users cannot be used to escape the skills directory
        assert!(st.add(Scope::User("../x".into()), RECIPE, true, true).is_err());
        assert!(st.list_scope(&Scope::User("../x".into())).is_empty());
    }

    #[test]
    fn replacing_a_skill_updates_its_index_entry() {
        let (st, _) = store("replace");
        st.add(Scope::Shared, MEETING, true, true).unwrap();
        let v2 = "---\nname: meeting-notes\ndescription: Summarise standups\ntriggers: standup\n---\nBe terse.\n";
        st.add(Scope::Shared, v2, true, true).unwrap();
        assert_eq!(st.list("x").len(), 1);
        assert_eq!(st.select("x", "standup", 1)[0].body, "Be terse.");
        assert!(st.select("x", "action items decisions", 1).is_empty(), "the old description no longer matches");
        let kb = st.kb.lock().unwrap();
        assert_eq!(kb.docs("skills/shared").len(), 1, "one index entry per skill");
    }

    #[test]
    fn the_best_skills_are_selected_for_a_request() {
        let (st, _) = store("select");
        st.add(Scope::Shared, MEETING, true, true).unwrap();
        st.add(Scope::Shared, RECIPE, true, true).unwrap();
        st.add(Scope::User("amy".into()), "---\nname: pirate-voice\ndescription: Answer like a pirate\ntriggers: pirate, arr\n---\nTalk like a pirate.\n", false, true).unwrap();
        let pick = |q: &str, u: &str| st.select(u, q, 2).into_iter().map(|s| s.name).collect::<Vec<_>>();
        assert_eq!(pick("please summarise my meeting minutes", "amy")[0], "meeting-notes");
        assert_eq!(pick("how do I format a cooking recipe", "amy")[0], "recipe-format");
        assert_eq!(pick("answer like a pirate", "amy")[0], "pirate-voice");
        assert!(!pick("answer like a pirate", "bob").contains(&"pirate-voice".to_string()), "bob never gets amy's personal skill");
        assert!(pick("zzzz qqqq", "amy").is_empty());
        assert!(st.select("amy", "", 2).is_empty() && st.select("amy", "meeting", 0).is_empty());
        assert!(st.select("../x", "meeting", 2).is_empty());
        // a personal skill with the same name overrides the shared one
        st.add(Scope::User("amy".into()), "---\nname: meeting-notes\ndescription: My own meeting style\n---\nWrite haikus.\n", false, true).unwrap();
        let s = st.select("amy", "meeting", 3);
        assert_eq!(s.iter().filter(|k| k.name == "meeting-notes").count(), 1);
        assert_eq!(s.iter().find(|k| k.name == "meeting-notes").unwrap().body, "Write haikus.");
        assert_eq!(st.list("amy").iter().filter(|k| k.name == "meeting-notes").count(), 1);
    }

    #[test]
    fn skills_survive_a_restart_and_prompt_block_is_capped() {
        let (st, d) = store("restart");
        st.add(Scope::Shared, MEETING, true, true).unwrap();
        drop(st);
        let kb = Arc::new(Mutex::new(KnowledgeBase::in_memory(Arc::new(HashEmbed::default()))));
        let st2 = SkillStore::new(&d, kb);
        assert_eq!(st2.select("amy", "meeting minutes", 1)[0].name, "meeting-notes", "the index is rebuilt from the files at startup");
        assert_eq!(prompt_block(&[]), "");
        let blk = prompt_block(&[parse(&format!("---\nname: long\ndescription: d\n---\n{}", "word ".repeat(2000)), Scope::Shared).unwrap()]);
        assert!(blk.contains("## Skill: long") && blk.chars().count() < 4300);
    }
}
