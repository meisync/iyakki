//! Chat tools for the desktop theme, including the theme files a person wrote (F-SHELL-62): the list is free (files that cannot be read
//! are listed with the reason), changing the choice asks a card first. The choice is saved with the person's display settings and takes
//! effect when they sign in again (the open screens keep the colours they were drawn with).

use iyakki_agent::settingstools::{no_args, Entry, Env};
use serde_json::json;

use super::AppCx;

pub const TOOLS: [&str; 2] = ["display_themes", "display_theme_set"];

pub const PROMPT_RULE: &str = "\nDESKTOP THEMES: display_themes lists the built-in themes (dark, light, high-contrast), the theme files the person or an administrator put in the themes folder (a file that cannot be read is listed with the reason) and which one is chosen; display_theme_set chooses one and asks the person first. The choice applies the next time they sign in.";

pub fn entries(cx: &AppCx, env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let (data, user) = (cx.services.dir().to_path_buf(), env.user.clone());
        v.push(Entry::read("Display", "display_themes", "List the desktop themes: the built-in ones, the person's own theme files and the shared ones (each with its name or, when the file cannot be read, why), and which one is chosen.", no_args(), move |_| {
            let chosen = iyakki_uikit::theme::load_choice(&data, &user).unwrap_or_else(|| "dark".into());
            let mut out = vec![format!("Chosen: {chosen}"), "Built in: dark, light, high-contrast".to_string()];
            let files = iyakki_uikit::usertheme::available(&data, &user);
            if files.is_empty() {
                out.push("Theme files: none (a file `name.theme` in the themes folder adds one).".into());
            }
            for (k, r) in files {
                out.push(match r {
                    Ok(u) => format!("Theme file {k}: {}", u.name),
                    Err(e) => format!("Theme file {k}: cannot be used: {e}"),
                });
            }
            Ok(out.join("\n"))
        }));
    }
    {
        let (data, user, e1) = (cx.services.dir().to_path_buf(), env.user.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"theme": {"type": "string", "description": "dark, light, high-contrast, or the key of a theme file from display_themes (for example midnight-blue)."}}, "required": ["theme"]});
        v.push(Entry::change("Display", "display_theme_set", "Choose the desktop theme: a built-in one or a theme file. A file that cannot be read is refused. It applies the next time the person signs in. The person approves a card first.", schema, |a| format!("Desktop theme: {}", a["theme"].as_str().unwrap_or("?")), move |a| {
            let t = a.get("theme").and_then(|x| x.as_str()).unwrap_or("").trim().to_lowercase();
            match t.as_str() {
                "dark" | "light" | "high-contrast" => {
                    iyakki_uikit::theme::save(&data, &user, iyakki_uikit::theme::ThemeId::from_key(&t));
                    e1.audit("settings.display.theme", &t);
                    Ok(format!("The theme {t} is chosen; it applies the next time you sign in."))
                }
                key => {
                    let (_, r) = iyakki_uikit::usertheme::available(&data, &user).into_iter().find(|(k, _)| k == key).ok_or_else(|| format!("there is no theme `{key}` (display_themes lists them)"))?;
                    let u = r.map_err(|e| format!("the theme file {key} cannot be used: {e}"))?;
                    iyakki_uikit::theme::save_choice(&data, &user, &format!("{}{key}", iyakki_uikit::usertheme::CHOICE_PREFIX));
                    e1.audit("settings.display.theme", &format!("file {key}"));
                    Ok(format!("The theme {} is chosen; it applies the next time you sign in.", u.name))
                }
            }
        }));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_agent::agent::{Risk, ToolDef, Tools};
    use iyakki_agent::settingstools::SettingsTools;
    use serde_json::Value;
    use std::sync::{Arc, Mutex};

    struct NoTools;
    impl Tools for NoTools {
        fn defs(&self) -> Vec<ToolDef> {
            vec![]
        }
        fn call(&self, name: &str, _a: &Value) -> Result<String, String> {
            Err(format!("no tool {name}"))
        }
        fn describe(&self, name: &str, _a: &Value) -> String {
            name.to_string()
        }
    }

    #[test]
    fn the_list_is_free_the_choice_asks_a_file_that_cannot_be_read_is_refused_and_the_choice_is_saved() {
        let cx = crate::apps::tests::cx_with_admin_store("chatthemes");
        cx.services.on_login("root", "rootpass12", true);
        let audits = Arc::new(Mutex::new(Vec::new()));
        let a2 = audits.clone();
        let settings = Arc::new(Mutex::new(iyakki_agent::settings::SettingsStore::open(&cx.services.dir().join("settings.json")).unwrap()));
        let env = Env { dir: cx.services.dir().to_path_buf(), user: "root".into(), admin: true, settings, audit: Arc::new(move |a, d| a2.lock().unwrap().push(format!("{a} {d}"))) };
        let dir = cx.services.dir().to_path_buf();
        std::fs::create_dir_all(dir.join("themes")).unwrap();
        std::fs::write(dir.join("themes/midnight-blue.theme"), "name = Midnight blue\nbase = dark\nbg_top = #0b1020\n").unwrap();
        std::fs::write(dir.join("themes/dull.theme"), "name = Dull\ntext = #111111\n").unwrap();
        let t = SettingsTools::new(Arc::new(NoTools), true, entries(&cx, &env));
        let d = t.defs();
        assert_eq!(d.iter().find(|x| x.spec.name == "display_themes").unwrap().risk, Risk::Safe);
        assert_eq!(d.iter().find(|x| x.spec.name == "display_theme_set").unwrap().risk, Risk::Ask);
        let l = t.call("display_themes", &json!({})).unwrap();
        assert!(l.contains("Chosen: dark") && l.contains("Theme file midnight-blue: Midnight blue") && l.contains("Theme file dull: cannot be used") && l.contains("`text` on"), "{l}");
        assert!(t.call("display_theme_set", &json!({"theme": "dull"})).unwrap_err().contains("cannot be used"));
        assert!(t.call("display_theme_set", &json!({"theme": "nope"})).unwrap_err().contains("no theme"));
        assert_eq!(iyakki_uikit::theme::load_choice(&dir, "root"), None, "nothing was saved by the refusals");
        assert!(t.call("display_theme_set", &json!({"theme": "midnight-blue"})).unwrap().contains("next time you sign in"));
        assert_eq!(iyakki_uikit::theme::load_choice(&dir, "root").as_deref(), Some("user:midnight-blue"));
        assert!(t.call("display_themes", &json!({})).unwrap().contains("Chosen: user:midnight-blue"));
        assert!(t.call("display_theme_set", &json!({"theme": "light"})).is_ok());
        assert_eq!(iyakki_uikit::theme::load(&dir, "root"), iyakki_uikit::theme::ThemeId::Light);
        assert!(audits.lock().unwrap().iter().any(|a| a == "settings.display.theme file midnight-blue"));
    }
}
