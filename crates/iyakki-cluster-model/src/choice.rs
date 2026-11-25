//! The person's last cluster choice and the managed cluster's kubeconfig; re-exported by `iyakki_shared::devcluster`.

use std::path::Path;
use std::path::PathBuf;
use serde::Deserialize;
use serde::Serialize;

/// The person's last choice, kept per user next to their other settings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Choice {
    pub kubeconfig: String,
    pub context: String,
}

impl Choice {
    pub fn path(dir: &Path, user: &str) -> PathBuf {
        let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
        dir.join(format!("devcluster-{safe}.json"))
    }
    pub fn load(dir: &Path, user: &str) -> Option<Choice> {
        serde_json::from_str(&std::fs::read_to_string(Self::path(dir, user)).ok()?).ok().filter(|c: &Choice| !c.kubeconfig.is_empty())
    }
    pub fn save(&self, dir: &Path, user: &str) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        std::fs::write(Self::path(dir, user), serde_json::to_string_pretty(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }
}

static MACHINE_KUBECONFIG: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

/// Remember the kubeconfig of the cluster IYAKKI manages on this machine (`None` forgets it).
pub fn set_machine_kubeconfig(p: Option<std::path::PathBuf>) {
    *MACHINE_KUBECONFIG.lock().unwrap_or_else(|e| e.into_inner()) = p;
}

pub fn machine_kubeconfig() -> Option<std::path::PathBuf> {
    MACHINE_KUBECONFIG.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_choice_file_is_named_after_the_user_with_unsafe_characters_removed() {
        assert_eq!(Choice::path(Path::new("/var/state"), "admin-team_1"), PathBuf::from("/var/state/devcluster-admin-team_1.json"));
        assert_eq!(Choice::path(Path::new("/var/state"), "admin team!"), PathBuf::from("/var/state/devcluster-adminteam.json"));
    }

    #[test]
    fn a_choice_with_no_user_characters_left_keeps_the_generic_file_name() {
        assert_eq!(Choice::path(Path::new("/s"), "@@@.."), PathBuf::from("/s/devcluster-.json"));
    }

    #[test]
    fn a_choice_read_from_json_fills_missing_fields_with_empty_strings() {
        let c: Choice = serde_json::from_str(r#"{"context":"lab"}"#).unwrap();
        assert_eq!(c, Choice { kubeconfig: String::new(), context: "lab".into() });
    }

    #[test]
    fn an_empty_json_object_is_the_default_choice() {
        let c: Choice = serde_json::from_str("{}").unwrap();
        assert_eq!(c, Choice::default());
    }
}
