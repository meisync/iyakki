//! Types moved out of `iyakki_agent::gitworktools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nGIT AND WORKSPACES: git_clone_repo copies a repository into the person's Repositories folder (`submodules`: true also clones its submodules; leave it out unless asked). It asks every time and takes an https:// address only; for a private repository the person's saved Git connection for that host is used by itself, so never ask for or send a token or password (if there is none, tell the person to add a connection in the Git tile). git_clones_list shows the clones on this computer. ide_workspace_add registers a folder as an AI IDE workspace (once, it asks); ide_workspace_list shows them. After a clone, read the deployment documents with read_file and use cluster_status and cluster_start for the cluster.";

use std::path::{Path, PathBuf};

use iyakki_sec::policy::{App, Policy};

/// Whether this account is offered the tools at all (the Git tile and the AI IDE are apps of the role).
pub fn offered(p: &Policy) -> bool {
    p.allows_app(App::Git) || p.allows_app(App::Ide)
}

/// `/home/<user>/Repositories`, or the account's folder under the data folder when it has no home to write in; the second value is the
/// folder the jobs' write check must also allow. The Git tile and these tools clone into the same place.
pub fn default_repos_root(data_dir: &Path, user: &str, policy: &Policy) -> (PathBuf, Option<PathBuf>) {
    let home = PathBuf::from(format!("/home/{user}/Repositories"));
    let fb = data_dir.join("repositories").join(user);
    let usable = policy.allows_write(user, &home.to_string_lossy()) && home.parent().is_some_and(|p| p.is_dir());
    if usable { (home, None) } else { (fb.clone(), Some(fb)) }
}
