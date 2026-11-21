//! Which SDK version of an app the person chose (F-SDK-34): EDIT and CLI each have an old screen and a version written on the UI SDK
//! (`iyakki-app-edit`, `iyakki-app-terminal`). The old screens stay; this is the person's own choice between them, kept as one line of
//! words in the core's app data (`app` [`APP`], key [`KEY`], for the person), so the shell, the Settings page and the chat tool read
//! and write the same text. Nothing is chosen until the person says so: the old screen is the default.

/// The app data namespace and key the choice is kept under.
pub const APP: &str = "shell";
pub const KEY: &str = "sdk-apps";

/// The apps that have an SDK version, as the app ids the shell and the policy know.
pub const IDS: [&str; 2] = ["edit", "cli"];

/// The ids named in the kept text (only known ids, each once, in the order of [`IDS`]).
pub fn chosen(text: Option<&str>) -> Vec<&'static str> {
    let words: Vec<&str> = text.unwrap_or("").split_whitespace().collect();
    IDS.iter().copied().filter(|id| words.contains(id)).collect()
}

/// Whether `id` is chosen in the kept text.
pub fn is_chosen(text: Option<&str>, id: &str) -> bool {
    chosen(text).contains(&id)
}

/// The text to keep after `id` is switched `on` or off; an unknown id is refused.
pub fn with(text: Option<&str>, id: &str, on: bool) -> Result<String, String> {
    if !IDS.contains(&id) {
        return Err(format!("only {} have a version on the UI SDK", IDS.join(" and ")));
    }
    let mut now = chosen(text);
    now.retain(|i| *i != id);
    if on {
        now.push(IDS.iter().copied().find(|i| *i == id).unwrap_or(IDS[0]));
    }
    now.sort_by_key(|i| IDS.iter().position(|x| x == i));
    Ok(now.join(" "))
}

/// One sentence for the person: what is chosen.
pub fn describe(text: Option<&str>) -> String {
    let c = chosen(text);
    let one = |id: &str| format!("{}: {}", id.to_uppercase(), if c.contains(&id) { "the SDK version" } else { "the usual screen" });
    format!("{}. {}.", one("edit"), one("cli"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_chosen_until_the_person_says_so_and_only_known_apps_count() {
        assert!(chosen(None).is_empty() && chosen(Some("")).is_empty());
        assert_eq!(chosen(Some("cli edit edit nonsense markdown")), vec!["edit", "cli"]);
        assert!(is_chosen(Some("cli"), "cli") && !is_chosen(Some("cli"), "edit") && !is_chosen(None, "edit"));
    }

    #[test]
    fn switching_one_app_leaves_the_other_and_keeps_a_fixed_order() {
        let a = with(None, "cli", true).unwrap();
        assert_eq!(a, "cli");
        let b = with(Some(&a), "edit", true).unwrap();
        assert_eq!(b, "edit cli");
        assert_eq!(with(Some(&b), "edit", false).unwrap(), "cli");
        assert_eq!(with(Some(&b), "cli", true).unwrap(), "edit cli", "on twice is still on once");
        assert_eq!(with(Some(&b), "cli", false).unwrap(), "edit");
        assert!(with(None, "chat", true).unwrap_err().contains("edit and cli"));
    }

    #[test]
    fn the_sentence_names_both_apps() {
        assert_eq!(describe(None), "EDIT: the usual screen. CLI: the usual screen.");
        assert_eq!(describe(Some("cli")), "EDIT: the usual screen. CLI: the SDK version.");
    }
}
