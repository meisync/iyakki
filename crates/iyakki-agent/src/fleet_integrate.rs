//! The planner as integrator (F-FLEET-06; docs/development/FLEET-AND-MODEL-ROUTER-PLAN.md section 3.3 step 6).
//!
//! The bundles the workers handed back are applied, in dependency order, to an **integration branch** that lives in its own worktree under
//! `.iyakki/fleet-integration/` of the workspace: the person's own working folder and branch are not touched until [`finalize`], which the
//! caller asks the person about. Each bundle is checked (`git bundle verify`, the fetched head against the head the worker reported), merged
//! with `--no-ff`, and the workspace's test command runs after each merge. A merge that conflicts or a merge after which the tests fail is
//! undone, and becomes a **follow-up task** (an assignment body the caller may send to a member) instead of half-applied work.
//!
//! Everything is the real `git` binary; hooks of the repository never run here (`core.hooksPath` is empty for every call).

use std::path::{Path, PathBuf};

use crate::cloudkeys::{hex, random};
use crate::fleet_hub::{AssignBody, Returned};
use crate::tasks::{add_exclude, git_ok, git_run};

/// A piece of work that did not go in and goes back to be done again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Followup {
    /// The task whose work did not apply.
    pub task: String,
    pub reason: String,
    /// What to send to a member (a new task id; it starts from the integration branch as it is).
    pub next: AssignBody,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Integration {
    pub branch: String,
    pub head: String,
    /// Task ids applied, in the order they were.
    pub applied: Vec<String>,
    /// Task ids skipped because they had no bundle (failures and questions) or were applied before.
    pub skipped: Vec<String>,
    pub followups: Vec<Followup>,
    /// The test result after the last merge (or `no test command`).
    pub tests: String,
}

const NOHOOKS: [&str; 2] = ["-c", "core.hooksPath=/dev/null"];

fn g(dir: &Path, args: &[&str]) -> (bool, String, String) {
    let mut a: Vec<&str> = NOHOOKS.to_vec();
    a.extend_from_slice(args);
    git_run(dir, &a)
}

fn gok(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut a: Vec<&str> = NOHOOKS.to_vec();
    a.extend_from_slice(args);
    git_ok(dir, &a)
}

/// The order the bundles are applied in: a task after the tasks it names (only those that are in the list), the rest by the time they came
/// back. A cycle is an error.
pub fn order(items: &[Returned]) -> Result<Vec<Returned>, String> {
    let mut left: Vec<Returned> = items.to_vec();
    left.sort_by(|a, b| (a.at, &a.id).cmp(&(b.at, &b.id)));
    let mut done: Vec<Returned> = vec![];
    while !left.is_empty() {
        let pos = left.iter().position(|r| r.after.iter().all(|a| done.iter().any(|d| &d.id == a) || !left.iter().any(|l| &l.id == a)));
        match pos {
            Some(n) => done.push(left.remove(n)),
            None => return Err(format!("the tasks depend on each other in a circle: {}", left.iter().map(|r| &r.id[..8]).collect::<Vec<_>>().join(", "))),
        }
    }
    Ok(done)
}

fn followup(task: &Returned, branch: &str, head: &str, why: &str) -> Result<Followup, String> {
    let title: String = format!("Redo: {}", task.title).chars().filter(|c| !c.is_control()).take(120).collect();
    let goal = format!("The work of the task {} could not be applied to the integration branch {branch}: {why}. Start from that branch (commit {head}) and do the same work again so that it fits, keeping what is already there.", &task.id[..8]);
    let next = AssignBody { id: hex(&random::<16>()?), title, goal, base_branch: branch.to_string(), base_commit: head.to_string(), rows: vec![], context_digest: String::new(), budget_tokens: 0, model_class: String::new(), after: vec![] };
    next.check()?;
    Ok(Followup { task: task.id.clone(), reason: why.to_string(), next })
}

/// Apply the bundles to the integration branch of `repo`. `bundles` pairs what came back with the file that holds it; `test` is the
/// workspace's test command (run in the integration worktree), if it has one. Applying again later adds to the same branch.
pub fn integrate(repo: &Path, name: &str, bundles: &[(Returned, PathBuf)], test: Option<(&str, &[String])>) -> Result<Integration, String> {
    if !crate::tasks::is_git_root(repo) {
        return Err("the workspace is not a git repository with a commit".into());
    }
    let name: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(24).collect();
    let branch = format!("fleet/integration-{name}");
    let tree = repo.join(".iyakki/fleet-integration").join(&name);
    add_exclude(repo, ".iyakki/fleet-integration/");
    if !tree.exists() {
        let head = gok(repo, &["rev-parse", "HEAD"])?.trim().to_string();
        std::fs::create_dir_all(tree.parent().unwrap_or(repo)).map_err(|e| e.to_string())?;
        let t = tree.to_string_lossy().into_owned();
        // the branch may exist from an earlier pass whose worktree was removed
        if g(repo, &["rev-parse", "--verify", "-q", &format!("refs/heads/{branch}")]).0 {
            gok(repo, &["worktree", "add", "-q", &t, &branch])?;
        } else {
            gok(repo, &["worktree", "add", "-q", "-b", &branch, &t, &head])?;
        }
    }
    let ordered = order(&bundles.iter().map(|b| b.0.clone()).collect::<Vec<_>>())?;
    let mut out = Integration { branch: branch.clone(), tests: "no test command".into(), ..Default::default() };
    for r in ordered {
        let file = bundles.iter().find(|b| b.0.id == r.id).map(|b| b.1.clone());
        let Some(file) = file.filter(|f| r.bundle && r.status == "done" && f.exists()) else {
            out.skipped.push(r.id.clone());
            continue;
        };
        let reference = format!("refs/fleet/{}", &r.id[..16]);
        if g(&tree, &["merge-base", "--is-ancestor", &reference, "HEAD"]).0 {
            out.skipped.push(r.id.clone());
            continue;
        }
        let here = gok(&tree, &["rev-parse", "HEAD"])?.trim().to_string();
        let f = file.to_string_lossy().into_owned();
        let (ok, _, err) = g(repo, &["bundle", "verify", &f]);
        if !ok {
            out.followups.push(followup(&r, &branch, &here, &format!("the bundle cannot be applied here ({})", err.lines().last().unwrap_or("verify failed").trim()))?);
            continue;
        }
        let spec = format!("refs/heads/{}:{reference}", r.branch);
        let (ok, _, err) = g(repo, &["fetch", "-q", "--no-tags", "--force", &f, &spec]);
        if !ok {
            out.followups.push(followup(&r, &branch, &here, &format!("the bundle holds no branch {} ({})", r.branch, err.lines().last().unwrap_or("").trim()))?);
            continue;
        }
        let tip = gok(repo, &["rev-parse", &reference])?.trim().to_string();
        if !r.head.is_empty() && tip != r.head {
            let _ = g(repo, &["update-ref", "-d", &reference]);
            out.followups.push(followup(&r, &branch, &here, "the bundle's head is not the head the worker reported")?);
            continue;
        }
        let msg = format!("Integrate task {}: {}", &r.id[..8], r.title);
        let (merged, mout, merr) = g(&tree, &["merge", "--no-ff", "-q", "-m", &msg, &reference]);
        if !merged {
            let files: Vec<String> = g(&tree, &["diff", "--name-only", "--diff-filter=U"]).1.lines().map(str::to_string).collect();
            let _ = g(&tree, &["merge", "--abort"]);
            let why = if files.is_empty() { format!("the merge failed ({})", mout.lines().next().unwrap_or(merr.lines().next().unwrap_or("")).trim()) } else { format!("it conflicts in {}", files.join(", ")) };
            out.followups.push(followup(&r, &branch, &here, &why)?);
            continue;
        }
        if let Some((prog, args)) = test {
            let (passed, text) = crate::tasks::run_command_in(&tree, prog, args);
            out.tests = text.clone();
            if !passed {
                gok(&tree, &["reset", "-q", "--hard", &here])?;
                out.followups.push(followup(&r, &branch, &here, &format!("the tests failed after it was merged: {}", text.lines().last().unwrap_or("").trim()))?);
                continue;
            }
        }
        out.applied.push(r.id.clone());
    }
    out.head = gok(&tree, &["rev-parse", "HEAD"])?.trim().to_string();
    Ok(out)
}

/// Put the integration branch into the person's branch (the one checked out in `repo`). The caller has asked the person. Refused when the
/// working folder has uncommitted changes, so nothing of the person's is mixed in; a merge that conflicts is undone.
pub fn finalize(repo: &Path, name: &str) -> Result<String, String> {
    let name: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(24).collect();
    let branch = format!("fleet/integration-{name}");
    if !g(repo, &["rev-parse", "--verify", "-q", &format!("refs/heads/{branch}")]).0 {
        return Err("there is no integration branch yet".into());
    }
    let current = gok(repo, &["symbolic-ref", "-q", "--short", "HEAD"]).map_err(|_| "the workspace is on a detached HEAD: check out a branch first".to_string())?.trim().to_string();
    if !gok(repo, &["status", "--porcelain", "--untracked-files=no"])?.trim().is_empty() {
        return Err("the working folder has uncommitted changes: commit or stash them first".into());
    }
    if g(repo, &["merge-base", "--is-ancestor", &branch, "HEAD"]).0 {
        return Err(format!("{current} already has everything on {branch}"));
    }
    let (ok, _, err) = g(repo, &["merge", "--no-ff", "-q", "-m", &format!("Finalize the fleet integration {name}"), &branch]);
    if !ok {
        let _ = g(repo, &["merge", "--abort"]);
        return Err(format!("it does not merge into {current}: {}", err.lines().next().unwrap_or("conflict").trim()));
    }
    let tree = repo.join(".iyakki/fleet-integration").join(&name);
    let _ = g(repo, &["worktree", "remove", "--force", &tree.to_string_lossy()]);
    let _ = g(repo, &["worktree", "prune"]);
    let _ = g(repo, &["branch", "-q", "-d", &branch]);
    Ok(format!("merged {branch} into {current}"))
}
