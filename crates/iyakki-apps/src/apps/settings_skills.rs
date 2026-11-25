//! Settings -> Skills: the person's skills and the shared ones: add from the clipboard, a form or a file, and delete (with a question first).
//! The page reaches the skills only through `SkillsPort` (docs/development/SETTINGS-EXTRACTION-PLAN.md, lane ST-B).

use super::*;
use super::apps_port::SkillsPort;

impl SettingsApp {
    pub(super) fn add_skill(&mut self, p: &dyn SkillsPort, markdown: &str, shared: bool) -> Result<String, String> {
        use iyakki_shared::skills::Scope;
        if shared && !p.may_manage_skills() {
            p.audit("settings.denied", "skills.shared");
            return Err("You may not edit the shared skills.".into());
        }
        let scope = if shared { Scope::Shared } else { Scope::User(p.user()) };
        let sk = p.skills_add(scope, markdown)?;
        p.audit("skills.add", &format!("{}:{}", sk.scope.label(), sk.name));
        Ok(format!("Saved {} skill `{}`.", sk.scope.label(), sk.name))
    }

    /// Read a skill file, but only from where this user may read (symlinks resolved first).
    pub(super) fn read_skill_file(p: &dyn SkillsPort, path: &str) -> Result<String, String> {
        p.skill_file_read(path)
    }

    pub(super) fn handle_skills(&mut self, k: &Key, p: &dyn SkillsPort) {
        let list = p.skills_list();
        let cur = list.get(self.sel()).cloned();
        match k {
            Key::Up => self.move_sel(-1, list.len()),
            Key::Down => self.move_sel(1, list.len()),
            Key::Char(c @ ('c' | 'C')) => {
                let shared = *c == 'C';
                match crate::ui::clipboard::latest() {
                    None => self.fail("The clipboard is empty. Copy a skill (Markdown with name/description at the top) first."),
                    Some(t) => match self.add_skill(p, &t, shared) {
                        Ok(m) => self.ok(m),
                        Err(e) => self.fail(e),
                    },
                }
            }
            Key::Char('a') => {
                let mut f = Form::new("New skill").text("Name", "", "lowercase-with-dashes").text("Description", "", "when should the assistant use it?").text("Triggers", "", "words that suggest it, comma separated").text("Instructions", "", "what the assistant should do");
                if self.share_skills {
                    f = f.choice("Scope", &["Mine", "Shared with everyone"], 0);
                } else {
                    f = f.choice("Scope", &["Mine"], 0);
                }
                self.open_form(FormKind::NewSkill, f);
            }
            Key::Char('i') => {
                let mut f = Form::new("Import skill file").path("File path", "", "/path/to/skill.md", crate::filepicker::PathKind::File, &["md"]);
                f = if self.share_skills { f.choice("Scope", &["Mine", "Shared with everyone"], 0) } else { f.choice("Scope", &["Mine"], 0) };
                self.open_form(FormKind::ImportSkill, f);
            }
            Key::Char('d') => {
                if let Some(sk) = cur {
                    let can = match sk.scope {
                        iyakki_shared::skills::Scope::Shared => p.may_manage_skills(),
                        _ => true,
                    };
                    if can {
                        let text = format!("Delete the {} skill “{}”?", sk.scope.label(), sk.name);
                        self.modal = Some(Modal::Confirm(ConfirmKind::Skill(sk.scope, sk.name), text));
                    } else {
                        self.fail("Only operators and administrators can delete shared skills.");
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn render_skills(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn SkillsPort) {
        let list = p.skills_list();
        if list.is_empty() {
            g.text(a.x, a.y + 4.0 * s, 15.0 * s, DIM, "No skills yet. A skill is a short Markdown file with instructions the assistant follows when a request matches it.", Face::Sans);
            g.text(a.x, a.y + 28.0 * s, 15.0 * s, DIM, "Copy one and press c, or press a to write one here.", Face::Sans);
            return;
        }
        let preview_h = 96.0 * s;
        let mut y = a.y;
        for (i, sk) in list.iter().enumerate() {
            if y + 58.0 * s > a.y + a.h - preview_h {
                break;
            }
            let chips: Vec<(&str, [u8; 4])> = vec![(sk.scope.label(), if sk.scope == iyakki_shared::skills::Scope::Shared { crate::fx::AMBER } else { ACCENT })];
            let on = i == self.sel();
            row(g, &mut self.tip, i, a.x, y, a.w, 52.0 * s, s, on, &sk.name, &sk.description, &chips);
            y += 58.0 * s;
        }
        if let Some(sk) = list.get(self.sel()) {
            let py = a.y + a.h - preview_h + 6.0 * s;
            let trig = g.fit(13.0 * s, &format!("triggers: {}", if sk.triggers.is_empty() { "(none)".to_string() } else { sk.triggers.join(", ") }), Face::Bold, a.w);
            g.text(a.x, py, 13.0 * s, MUTED, &trig, Face::Bold);
            for (n, l) in g.wrap(13.0 * s, &sk.body, Face::Mono, a.w).iter().take(4).enumerate() {
                g.text(a.x, py + 22.0 * s + n as f32 * 17.0 * s, 13.0 * s, TEXT, l, Face::Mono);
            }
        }
    }
}
