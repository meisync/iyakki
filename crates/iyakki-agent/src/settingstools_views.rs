//! Chat tools for Settings -> Desktop views: `views_list` (free), `views_make`, `views_rename`, `views_delete`, `views_lock`, `views_keys`
//! and `views_switch` (each asks). They use `views::Views`, the same file and rules the page uses. Locking a view only adds the password
//! question; `views_switch` to a locked view still shows the lock screen, the password is typed by the person.

use serde_json::{json, Value};

use crate::settingstools::{arg_bool, arg_str, no_args, Entry, Env};
use crate::views::{KeySet, UserTemplates, Views, MAX_NAME, TEMPLATES};
pub use iyakki_shared::settingstools_views::{PROMPT_RULE};

const TAB: &str = "Views";

pub const TOOLS: [&str; 11] = ["views_list", "views_templates_list", "views_make", "views_rename", "views_delete", "views_lock", "views_keys", "views_switch", "views_share", "views_template_save", "views_template_delete"];


fn describe_list(v: &Views, own: &UserTemplates) -> String {
    let mut out = format!("{} views (at most 9). Step keys: {}. After the lock or the screen saver: {}.", v.views.len(), v.key_set.label(), if v.reset_on_lock { "return to the first view" } else { "stay where you were" });
    out.push_str(&format!(" Window layouts are {}. One window per app: {}.", if v.remember_layout { "remembered for each view" } else { "not remembered" }, if v.single_apps.is_empty() { "off".to_string() } else { v.single_apps.join(", ") }));
    for (i, w) in v.views.iter().enumerate() {
        out.push_str(&format!("\n{}. {}{}{}{}", i + 1, w.name, if w.locked { " (locked)" } else { "" }, if w.id == v.active { " (showing)" } else { "" }, w.layout.as_ref().map_or(String::new(), |l| format!(" [{}: {} pane(s)]", l.layout, l.apps.len()))));
    }
    out.push_str(&format!("\nTemplates: {}.", TEMPLATES.iter().map(|t| t.id).collect::<Vec<_>>().join(", ")));
    if !own.list.is_empty() {
        out.push_str(&format!("\nThe person's own templates: {}.", own.list.iter().map(|t| format!("{} ({})", t.name, t.tiles.join(", "))).collect::<Vec<_>>().join("; ")));
    }
    out
}

fn which(a: &Value) -> String {
    arg_str(a, "view").unwrap_or_else(|_| "a view".into())
}

pub fn entries(env: &Env) -> Vec<Entry> {
    let sel = json!({"type": "object", "properties": {"view": {"type": "string", "description": "The view's number (1 to 9) or its name."}}, "required": ["view"]});
    let mut v = Vec::new();
    {
        let e = env.clone();
        v.push(Entry::read(TAB, "views_list", "Show the person's desktop views: number, name, locked or not, which one is showing, the step keys and the templates.", no_args(), move |_| Ok(describe_list(&Views::load(&e.dir, &e.user), &UserTemplates::load(&e.dir, &e.user)))));
    }
    {
        let e = env.clone();
        v.push(Entry::read(TAB, "views_templates_list", "List the templates for desktop views: the built-in ones and the person's own, each with its apps.", no_args(), move |_| {
            let own = UserTemplates::load(&e.dir, &e.user);
            let mut out = String::from("Built-in templates:");
            for t in TEMPLATES {
                out.push_str(&format!("\n{}: {}", t.id, t.tiles.join(", ")));
            }
            out.push_str("\nThe person's own templates:");
            if own.list.is_empty() {
                out.push_str(" none yet (views_template_save keeps one).");
            }
            for t in &own.list {
                out.push_str(&format!("\n{}: {}", t.name, t.tiles.join(", ")));
            }
            Ok(out)
        }));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"name": {"type": "string", "description": "Up to 16 characters (optional with a template)."}, "template": {"type": "string", "description": "work, media, monitoring, or the name of one of the person's own templates."}}});
        v.push(Entry::change(
            TAB,
            "views_make",
            "Make a new desktop view, empty or from a template (work, media, monitoring). The person approves a card first.",
            schema,
            |a| match (arg_str(a, "name"), arg_str(a, "template")) {
                (Ok(n), _) => format!("Make the desktop view {n}"),
                (_, Ok(t)) => format!("Make a desktop view from the {t} template"),
                _ => "Make a desktop view".into(),
            },
            move |a| {
                let mut views = Views::load(&e.dir, &e.user);
                let msg = match (arg_str(a, "name"), arg_str(a, "template")) {
                    (Ok(n), t) => views.add(&n, t.ok().as_deref()).map(|_| format!("Made the view {n}."))?,
                    (Err(_), Ok(t)) if crate::views::template(&t).is_none() && UserTemplates::load(&e.dir, &e.user).get(&t).is_some() => {
                        let own = UserTemplates::load(&e.dir, &e.user);
                        views.add_from_user_template(own.get(&t).ok_or("no such template")?).map(|(_, n)| format!("Made the view {n}. Its apps open the first time the person switches to it."))?
                    }
                    (Err(_), Ok(t)) => views.add_from_template(&t).map(|(_, n)| format!("Made the view {n}. Its apps open the first time the person switches to it."))?,
                    _ => return Err("say a `name`, a `template`, or both".into()),
                };
                views.save(&e.dir, &e.user);
                e.audit("view.edit", &msg);
                Ok(msg)
            },
        ));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"view": {"type": "string"}, "name": {"type": "string", "description": format!("Up to {MAX_NAME} characters.")}}, "required": ["view", "name"]});
        v.push(Entry::change(TAB, "views_rename", "Rename a desktop view. The person approves a card first.", schema, |a| format!("Rename the view {} to {}", which(a), arg_str(a, "name").unwrap_or_default()), move |a| {
            let mut views = Views::load(&e.dir, &e.user);
            let id = views.find(&arg_str(a, "view")?)?;
            let name = arg_str(a, "name")?;
            views.rename(id, &name)?;
            views.save(&e.dir, &e.user);
            e.audit("view.edit", &name);
            Ok(format!("Renamed to {name}."))
        }));
    }
    {
        let e = env.clone();
        v.push(Entry::change(TAB, "views_delete", "Delete a desktop view; the apps open in it are closed, nothing is uninstalled. The last view stays. The person approves a card first.", sel.clone(), |a| format!("Delete the desktop view {}", which(a)), move |a| {
            let mut views = Views::load(&e.dir, &e.user);
            let id = views.find(&arg_str(a, "view")?)?;
            views.delete(id)?;
            views.save(&e.dir, &e.user);
            e.audit("view.delete", &id.to_string());
            Ok("View deleted.".into())
        }));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"view": {"type": "string"}, "locked": {"type": "boolean"}}, "required": ["view", "locked"]});
        v.push(Entry::change(TAB, "views_lock", "Lock or unlock a desktop view: a locked view asks for the person's password each time they switch to it. The person approves a card first.", schema, |a| format!("{} the desktop view {}", if a["locked"].as_bool() == Some(false) { "Unlock" } else { "Lock" }, which(a)), move |a| {
            let mut views = Views::load(&e.dir, &e.user);
            let id = views.find(&arg_str(a, "view")?)?;
            let locked = arg_bool(a, "locked")?;
            views.set_locked(id, locked);
            views.save(&e.dir, &e.user);
            let name = views.get(id).map(|w| w.name.clone()).unwrap_or_default();
            e.audit("view.lock", &format!("{name}:{locked}"));
            Ok(if locked { format!("{name} asks for the password each time the person switches to it.") } else { format!("{name} opens without the password again.") })
        }));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"key_set": {"type": "string", "enum": ["browser", "mac", "both"]}, "reset_on_lock": {"type": "boolean"}, "remember_layout": {"type": "boolean", "description": "Keep each view's window arrangement across switching and sign-out (off forgets the saved ones)."}, "custom_prev": {"type": "string", "description": "A key of your own for the previous view, used with Ctrl+Alt: a letter other than M, Q, L and S, or [ ] , . (empty text with custom_next empty turns the own keys off)."}, "custom_next": {"type": "string", "description": "The key for the next view, used with Ctrl+Alt; it must differ from custom_prev."}, "single_apps": {"type": "array", "items": {"type": "string"}, "description": "App ids that keep one window: opening one that is already open goes to it. [\"*\"] is every app, [] turns the rule off."}}});
        v.push(Entry::change(TAB, "views_keys", "Choose the step keys (browser-style Ctrl+Alt+PageUp/PageDown, Mac-style Control+Command+Left/Right, or both) and whether the lock returns to the first view. The person approves a card first.", schema, |_| "Change the desktop view keys".into(), move |a| {
            let mut views = Views::load(&e.dir, &e.user);
            if let Some(k) = a.get("key_set").and_then(Value::as_str) {
                views.key_set = match k {
                    "browser" => KeySet::Browser,
                    "mac" => KeySet::Mac,
                    "both" => KeySet::Both,
                    _ => return Err("key_set is browser, mac or both".into()),
                };
            }
            if let Some(b) = a.get("reset_on_lock").and_then(Value::as_bool) {
                views.reset_on_lock = b;
            }
            if let Some(b) = a.get("remember_layout").and_then(Value::as_bool) {
                views.set_remember_layout(b);
            }
            if let Some(list) = a.get("single_apps") {
                let apps: Vec<String> = list.as_array().ok_or("`single_apps` is a list of app ids")?.iter().map(|x| x.as_str().map(str::to_string).ok_or("`single_apps` holds app ids as text")).collect::<Result<_, _>>()?;
                views.set_single(&apps)?;
            }
            if a.get("custom_prev").is_some() || a.get("custom_next").is_some() {
                let given = |k: &str| a.get(k).and_then(Value::as_str).map(str::trim);
                match (given("custom_prev"), given("custom_next")) {
                    (Some(p), Some(n)) => views.custom_keys = crate::views::CustomKeys::from_typed(p, n).map_err(|e| format!("`custom_prev` and `custom_next`: {e}"))?,
                    _ => return Err("give both `custom_prev` and `custom_next` (or both empty to turn the own keys off)".into()),
                }
            }
            if ["key_set", "reset_on_lock", "remember_layout", "single_apps", "custom_prev", "custom_next"].iter().all(|k| a.get(*k).is_none()) {
                return Err("say `key_set`, `reset_on_lock`, `remember_layout`, `single_apps` or `custom_prev` with `custom_next`".into());
            }
            views.save(&e.dir, &e.user);
            e.audit("view.keys", views.key_set.label());
            Ok(format!("Step keys: {}{}. After the lock or the screen saver: {}. Window layouts: {}. One window per app: {}.", views.key_set.label(), views.custom_keys.as_ref().map(|c| format!(", and your own {}", c.label())).unwrap_or_default(), if views.reset_on_lock { "return to the first view" } else { "stay where you were" }, if views.remember_layout { "remembered" } else { "not remembered" }, if views.single_apps.is_empty() { "off".to_string() } else { views.single_apps.join(", ") }))
        }));
    }
    {
        let e = env.clone();
        v.push(Entry::change(TAB, "views_switch", "Go to a desktop view. A locked view shows the lock screen and the person types the password. The person approves a card first.", sel, |a| format!("Switch to the desktop view {}", which(a)), move |a| {
            let views = Views::load(&e.dir, &e.user);
            let id = views.find(&arg_str(a, "view")?)?;
            crate::views::request_switch(&e.dir, &e.user, id);
            let w = views.get(id).map(|w| (w.name.clone(), w.locked)).unwrap_or_default();
            e.audit("view.switch.request", &w.0);
            Ok(if w.1 { format!("{} is locked: the lock screen asks the person for the password first.", w.0) } else { format!("Switching to {}.", w.0) })
        }));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"view": {"type": ["string", "integer"], "description": "The view that should show the open app as well: its number or name."}}, "required": ["view"]});
        v.push(Entry::change(TAB, "views_share", "Show the app that is open on screen now in another desktop view as well. It is the same instance (not a copy): what is typed in one view is there in the other. The other view must have no app open. The person approves a card first.", schema, |a| format!("Show the open app in the desktop view {} as well", which(a)), move |a| {
            let views = Views::load(&e.dir, &e.user);
            let id = views.find(&arg_str(a, "view")?)?;
            let name = views.get(id).map(|w| w.name.clone()).unwrap_or_default();
            crate::views::request_share(&e.dir, &e.user, id);
            e.audit("view.share.request", &name);
            Ok(format!("The app that is open will be shown in {name} as well (if it has no app open and one is showing now)."))
        }));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"name": {"type": "string", "description": format!("Up to {MAX_NAME} characters; the same name replaces the template.")}, "tiles": {"type": "array", "items": {"type": "string"}, "description": "App ids in pane order, 1 to 6."}}, "required": ["name", "tiles"]});
        v.push(Entry::change(TAB, "views_template_save", "Keep a template of the person's own for desktop views: a name and the apps that open in it. The person approves a card first.", schema, |a| format!("Keep the view template {}", arg_str(a, "name").unwrap_or_default()), move |a| {
            let name = arg_str(a, "name")?;
            let tiles: Vec<String> = a.get("tiles").and_then(Value::as_array).ok_or("`tiles` is a list of app ids")?.iter().filter_map(|t| t.as_str().map(String::from)).collect();
            let mut own = UserTemplates::load(&e.dir, &e.user);
            own.save_template(&name, &tiles)?;
            if !own.save(&e.dir, &e.user) {
                return Err("the templates file could not be written".into());
            }
            e.audit("view.template", &name);
            Ok(format!("Kept the template {name}: {}.", tiles.join(", ")))
        }));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]});
        v.push(Entry::change(TAB, "views_template_delete", "Delete one of the person's own view templates (views made from it stay). The person approves a card first.", schema, |a| format!("Delete the view template {}", arg_str(a, "name").unwrap_or_default()), move |a| {
            let name = arg_str(a, "name")?;
            let mut own = UserTemplates::load(&e.dir, &e.user);
            own.remove(&name)?;
            if !own.save(&e.dir, &e.user) {
                return Err("the templates file could not be written".into());
            }
            e.audit("view.template.delete", &name);
            Ok(format!("Deleted the template {name}."))
        }));
    }
    v
}
