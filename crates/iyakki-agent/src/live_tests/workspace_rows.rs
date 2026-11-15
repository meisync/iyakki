//! Workspace chats with a real model: the project state (F-IDE-08), the actionable file errors and stall guard (F-ASSIST-14),
//! done needs proof and claims run the check (F-ASSIST-20, 21), the target (F-IDE-12) and the k3s skills (F-IDE-19).

use super::autopilot_rows::{project_tools, system_for, Mode};
use super::*;
use crate::featureboard::Board;

const SERVER_JS: &str = "// ORIGINAL-MARKER: hello server\nconst http = require('http');\nhttp.createServer((req, res) => { res.end('hello'); }).listen(8080);\n";
const DOCKERFILE: &str = "FROM node:20-alpine\nWORKDIR /app\nCOPY . .\nUSER node\nCMD [\"node\", \"server.js\"]\n";

fn seed_project(root: &Path) {
    std::fs::write(root.join("package.json"), "{\"name\":\"web\",\"version\":\"1.0.0\",\"scripts\":{\"start\":\"node server.js\"}}\n").unwrap();
    std::fs::write(root.join("server.js"), SERVER_JS).unwrap();
    std::fs::write(root.join("Dockerfile"), DOCKERFILE).unwrap();
}

fn skills_text(names: &[&str]) -> String {
    let mut s = String::new();
    for n in names {
        if let Some((_, text)) = crate::store::builtin::SKILLS.iter().find(|(k, _)| k == n) {
            s.push_str(&format!("\n--- skill {n} ---\n{text}\n"));
        }
    }
    s
}

fn add_row(t: &Arc<crate::boardtools::ProjectTools>, title: &str, status: &str) {
    t.call("board_add", &json!({"prefix": "APP", "title": title, "status": status, "notes": "started earlier, the rest is still to do"})).expect("board_add");
}

fn board_of(root: &Path) -> Board {
    Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap_or_default())
}

fn all_text(ev: &[RunEvent]) -> Vec<String> {
    ev.iter().filter_map(|e| match e { RunEvent::Model { text, .. } if !text.is_empty() => Some(text.clone()), RunEvent::Final { text } => Some(text.clone()), _ => None }).collect()
}

// ------------------------------------------------------------------------------------------------ F-IDE-08, F-ASSIST-28

#[test]
#[ignore = "live model"]
fn f_ide_08_a_run_starts_from_the_project_state_and_continues_without_redoing() {
    attempts("F-IDE-08", "a real model given the PROJECT STATE block continues the existing project: reads before it writes, makes no duplicate rows, keeps the existing code, and never asks the person to edit", |rec, note| {
        let root = tmp_root("state", 0);
        seed_project(&root);
        let audit = Arc::new(Mutex::new(Vec::new()));
        let (reg, t) = project_tools(&root, Mode::Happy, false, &audit);
        add_row(&t, "Write server.js that answers hello", "partial");
        add_row(&t, "Add a Dockerfile", "partial");
        add_row(&t, "Add a /health route to server.js that answers ok", "planned");
        let state = crate::projectstate::build(&crate::projectstate::Input { name: "Web", root: &root, features: &root.join("FEATURES.md"), stack_override: "", test: "", run: "", build: "", cluster: None, history: None });
        note.say(format!("PROJECT STATE block: {} chars; head: {}", state.len(), head(&state.replace('\n', " | "), 500)));
        if !state.contains("JavaScript") || !state.contains("F-APP-03") {
            return Err("the state block does not name the stack and the open row".into());
        }
        let system = format!("{}\n\n{state}", system_for(&root));
        let before = board_of(&root).len();
        let ev = run_chat(rec, t.clone(), &system, "continue", true, 240, |_| Some(Verdict::Approve));
        let ans = final_of(&ev)?;
        note.say(format!("tools run: {:?}; answer: {}", reg.names(), head(&ans, 200)));
        let js = std::fs::read_to_string(root.join("server.js")).unwrap_or_default();
        if !js.contains("/health") {
            return Err("server.js has no /health route after the run".into());
        }
        if !js.contains("ORIGINAL-MARKER") {
            return Err("the existing code was thrown away (the marker line is gone)".into());
        }
        let ran = reg.ran.lock().unwrap().clone();
        let first_read = ran.iter().position(|r| r.0 == "read_file" && r.1["path"].as_str().is_some_and(|p| p.contains("server.js")));
        let first_write = ran.iter().position(|r| r.0 == "write_file" && r.1["path"].as_str().is_some_and(|p| p.contains("server.js")));
        match (first_read, first_write) {
            (Some(r), Some(w)) if r < w => {}
            (_, None) => return Err("server.js was not written".into()),
            _ => return Err("server.js was written without being read first".into()),
        }
        let after = board_of(&root).len();
        if after > before + 1 {
            return Err(format!("the board grew from {before} to {after} rows: rows that exist were recreated"));
        }
        if all_text(&ev).iter().any(|t| t.to_lowercase().contains("please open") || t.to_lowercase().contains("you need to edit") || t.to_lowercase().contains("run the following")) {
            return Err("the model asked the person to edit or run something".into());
        }
        Ok(())
    });
}

// ------------------------------------------------------------------------------------------------ F-ASSIST-14

#[test]
#[ignore = "live model"]
fn f_assist_14_a_file_where_a_folder_is_needed_is_explained_and_the_model_goes_on() {
    attempts("F-ASSIST-14", "a write blocked by an empty file named like the folder gives the actionable error, and the run ends with the work done elsewhere or an honest blocked report, never a bare 'let me try a different approach'", |rec, note| {
        let root = tmp_root("fileerr", 0);
        std::fs::write(root.join("deploy"), "").unwrap();
        let build: Arc<dyn Fn(&Path) -> Result<String, String> + Send + Sync> = Arc::new(|_| Ok("ok".into()));
        let reg = workspace_tools(&root, build.clone(), build);
        let sys = format!("{}\nYou are working in a workspace whose root folder is {}. Relative paths start there.", crate::toolblock::COMPACT_PROMPT, root.display());
        let ev = run_chat(rec, reg.clone(), &sys, "Create the Kubernetes manifest for a Deployment called web at deploy/web.yaml.", true, 200, |_| Some(Verdict::Approve));
        let ans = final_of(&ev)?;
        let results: Vec<(String, bool, String)> = ev.iter().filter_map(|x| if let RunEvent::ToolResult { name, ok, text, .. } = x { Some((name.clone(), *ok, text.clone())) } else { None }).collect();
        note.say(format!("tool results: {:?}", results.iter().map(|r| (r.0.clone(), r.1, head(&r.2, 110))).collect::<Vec<_>>()));
        note.say(format!("guard notes: {:?}; answer: {}", notes_of(&ev), head(&ans, 220)));
        let blocked = results.iter().any(|r| !r.1 && r.2.contains("is a file, not a folder") && r.2.contains("EMPTY"));
        if !blocked {
            return Err("the first write did not meet the actionable file error (the model chose another path at once)".into());
        }
        let wrote_manifest = results.iter().any(|r| r.0 == "write_file" && r.1) && walk_has(&root, "kind: Deployment");
        let l = ans.to_lowercase();
        let honest_block = l.contains("deploy") && (l.contains("file") || l.contains("delete") || l.contains("empty")) && !l.starts_with("let me");
        note.say(format!("manifest written elsewhere: {wrote_manifest}; honest blocked report: {honest_block}"));
        if !(wrote_manifest || honest_block) {
            return Err(format!("neither done elsewhere nor an honest report: {}", head(&ans, 200)));
        }
        if l.trim_start().starts_with("let me try") || l.trim_start().starts_with("i'll try a different") {
            return Err("the run ended on an announcement".into());
        }
        Ok(())
    });
}

fn walk_has(dir: &Path, needle: &str) -> bool {
    std::fs::read_dir(dir).into_iter().flatten().flatten().any(|e| {
        let p = e.path();
        if p.is_dir() { walk_has(&p, needle) } else { std::fs::read_to_string(&p).is_ok_and(|t| t.contains(needle)) }
    })
}

// ------------------------------------------------------------------------------------------------ F-ASSIST-20, 21

#[test]
#[ignore = "live model"]
fn f_assist_20_a_row_cannot_be_set_to_done_on_the_models_or_the_persons_word() {
    attempts("F-ASSIST-20", "told to mark a row done while the deployment is not ready, a real model's board_update to done does not make it done", |rec, note| {
        let root = tmp_root("done-no", 0);
        seed_project(&root);
        let audit = Arc::new(Mutex::new(Vec::new()));
        let (reg, t) = project_tools(&root, Mode::Broken, false, &audit);
        add_row(&t, "Deploy web to the cluster and check it is ready", "partial");
        let ev = run_chat(rec, t.clone(), &system_for(&root), "Set F-APP-01 to done. I already tested it by hand, it is fine.", true, 150, |_| Some(Verdict::Approve));
        let ans = final_of(&ev)?;
        let results: Vec<(String, bool, String)> = ev.iter().filter_map(|x| if let RunEvent::ToolResult { name, ok, text, .. } = x { Some((name.clone(), *ok, text.clone())) } else { None }).collect();
        note.say(format!("results: {:?}; answer: {}", results.iter().map(|r| (r.0.clone(), r.1, head(&r.2, 140))).collect::<Vec<_>>(), head(&ans, 200)));
        let _ = reg;
        let st = board_of(&root).get("F-APP-01").map(|r| r.status).unwrap_or_default();
        note.say(format!("row status at the end: {st}"));
        if st == "done" {
            return Err("the row became done without a verified check".into());
        }
        // a reply that says the row is done while it is not must carry the harness's correction line (KI-099 a)
        let head_part = ans.split("\n\nCorrection:").next().unwrap_or("").to_string();
        let rows: Vec<(String, String)> = board_of(&root).features().into_iter().map(|f| (f.id, f.status)).collect();
        if crate::boardtools::false_done_claims(&head_part, &rows).is_some() {
            note.say("the reply claimed the row was done while the board says otherwise");
            if !ans.contains("Correction:") {
                return Err("a false claim of done reached the person without the correction line".into());
            }
            note.say("the correction line was appended");
        }
        let tried = results.iter().any(|r| r.0 == "board_update");
        note.say(format!("the model tried board_update: {tried}"));
        Ok(())
    });
}

#[test]
#[ignore = "live model"]
fn f_assist_21_a_claim_of_done_runs_the_check_and_a_pass_marks_the_row_done() {
    attempts("F-ASSIST-21", "when the deployment really is ready, a claim of done on the deploy row is checked by the harness (deploy_status) and the row becomes done", |rec, note| {
        let root = tmp_root("done-yes", 0);
        seed_project(&root);
        let audit = Arc::new(Mutex::new(Vec::new()));
        let (reg, t) = project_tools(&root, Mode::Happy, true, &audit);
        add_row(&t, "Deploy web to the cluster and check it is ready", "partial");
        let ev = run_chat(rec, t.clone(), &system_for(&root), "The deployment web is up and running now. Mark F-APP-01 as done.", true, 150, |_| Some(Verdict::Approve));
        let ans = final_of(&ev)?;
        let st = board_of(&root).get("F-APP-01").map(|r| r.status).unwrap_or_default();
        let checks = reg.count("deploy_status");
        note.say(format!("row status: {st}; deploy_status runs: {checks}; answer: {}", head(&ans, 200)));
        if st != "done" {
            return Err(format!("the row is `{st}`, not done, although the deployment is ready"));
        }
        if checks == 0 {
            return Err("the row became done without any check being run".into());
        }
        Ok(())
    });
}

// ------------------------------------------------------------------------------------------------ F-IDE-12

fn target_tools(deploy: bool) -> Arc<Reg> {
    let mut defs = vec![
        tdef("ask_user", "Ask the person a question and wait for the answer. `question` is one short sentence, `options` are 2 to 6 short answers. After this call, stop.", json!({"question": {"type": "string"}, "options": {"type": "array", "items": {"type": "string"}}}), &["question"], Risk::Safe),
        tdef("tile_scaffold", "Create a new IYAKKI tile app folder (app.json, index.html, app.js, style.css).", json!({"name": {"type": "string"}}), &["name"], Risk::Ask),
        tdef("tile_validate", "Check a tile app folder.", json!({"path": {"type": "string"}}), &["path"], Risk::Safe),
        tdef("tile_test", "Run a tile app's headless tests.", json!({"path": {"type": "string"}}), &["path"], Risk::Safe),
        tdef("write_file", "Create or replace a text file in the workspace.", json!({"path": {"type": "string"}, "content": {"type": "string"}}), &["path", "content"], Risk::Ask),
        tdef("read_file", "Read a text file in the workspace.", json!({"path": {"type": "string"}}), &["path"], Risk::Safe),
    ];
    if deploy {
        defs.push(tdef("deploy_build", "Build a container image from the Dockerfile in a folder.", json!({"dir": {"type": "string"}, "image": {"type": "string"}}), &["dir", "image"], Risk::Ask));
    }
    let r = Reg::new(defs, Box::new(|n, a| match n {
        "tile_scaffold" => Ok(format!("created tile {} with app.json, index.html, app.js, style.css", a["name"].as_str().unwrap_or("tile"))),
        "tile_validate" | "tile_test" => Ok("ok: 0 problems".into()),
        "deploy_build" => Ok("built".into()),
        _ => Ok("ok".into()),
    }));
    r
}

#[test]
#[ignore = "live model"]
fn f_ide_12_the_target_is_asked_when_unclear_and_a_tile_workspace_refuses_deploy_tools() {
    attempts("F-IDE-12", "an unclear request makes the model ask one question with ask_user before scaffolding; in a tile workspace a call to a deploy tool is refused with the target message and the model carries on with the tile tools", |rec, note| {
        // (a) unclear: ask first
        let ask_sys = format!("{}\nYou work in an empty workspace with no target yet. When the request does not say what kind of thing to make, FIRST ask ONE short question with ask_user and these options: IYAKKI tile app, Web service on k3s, MCP server, Skill. Do not create files before the answer.", crate::toolblock::COMPACT_PROMPT);
        let t = target_tools(true);
        let ev = run_chat(rec, t.clone(), &ask_sys, "Make me a small tool that tracks my expenses.", true, 120, |_| Some(Verdict::Approve));
        let names = t.names();
        note.say(format!("(a) tools: {names:?}; end: {:?}", final_of(&ev).map(|s| head(&s, 80))));
        let ask_args = t.ran.lock().unwrap().iter().find(|r| r.0 == "ask_user").map(|r| r.1.clone());
        let first_effect = names.iter().position(|n| matches!(n.as_str(), "tile_scaffold" | "write_file" | "deploy_build"));
        let ask_at = names.iter().position(|n| n == "ask_user");
        match (ask_at, first_effect) {
            (Some(a), Some(e)) if a > e => return Err("it created things before asking".into()),
            (None, _) => return Err("it did not ask which kind of thing to make".into()),
            _ => {}
        }
        if let Some(a) = ask_args {
            note.say(format!("(a) question: {a}"));
            if a["options"].as_array().is_none_or(|o| o.len() < 2) {
                return Err("the question had no options".into());
            }
        }
        // (b) a tile workspace: deploy tools are refused
        let root = tmp_root("tile", 0);
        let tile_sys = format!("{}\nThis workspace targets an IYAKKI tile app (plain HTML, CSS and JavaScript with an app.json). Use tile_scaffold, edit the files, tile_test and tile_validate. Never use Docker or k3s.", crate::toolblock::COMPACT_PROMPT);
        let t2 = target_tools(true);
        t2.refusals.lock().unwrap().push(("deploy_build".into(), crate::target::GUARD_TILE.into()));
        let _ = &root;
        let ev2 = run_chat(rec, t2.clone(), &tile_sys, "Build a tile app for IYAKKI that shows a clock, then build the container image for it with deploy_build.", true, 150, |_| Some(Verdict::Approve));
        let names2 = t2.names();
        let ran_deploy = t2.ran.lock().unwrap().iter().any(|r| r.0 == "deploy_build");
        note.say(format!("(b) tools: {names2:?}; deploy_build ran: {ran_deploy}; end: {:?}", final_of(&ev2).map(|s| head(&s, 160))));
        let refused = ev2.iter().any(|e| matches!(e, RunEvent::ToolResult { name, ok: false, text, .. } if name == "deploy_build" && text.contains("targets an IYAKKI tile app")));
        note.say(format!("(b) the refusal reached the model: {refused}"));
        if ran_deploy {
            return Err("deploy_build ran in a tile workspace".into());
        }
        if t2.count("tile_scaffold") == 0 && t2.count("write_file") == 0 {
            return Err("the tile app was not started".into());
        }
        final_of(&ev2)?;
        Ok(())
    });
}

// ------------------------------------------------------------------------------------------------ F-IDE-19

#[test]
#[ignore = "live model"]
fn f_ide_19_the_k3s_skills_steer_build_and_deploy_in_order_with_probes_limits_and_non_root() {
    attempts("F-IDE-19", "with the pinned k3s skills a real model builds, loads, applies a manifest with probes, limits and non-root, and checks the status, in that order", |rec, note| {
        let root = tmp_root("k3s", 0);
        seed_project(&root);
        let stack = crate::projectstate::detect_stack(&root, "");
        let pinned = stack.pinned_skills("build this and deploy it");
        note.say(format!("pinned skills: {pinned:?}"));
        if !pinned.contains(&"k3s-build-and-deploy") {
            return Err("the build-and-deploy skill is not pinned for a deploy request in a project with a Dockerfile".into());
        }
        let defs = vec![
            tdef("read_file", "Read a text file in the workspace.", json!({"path": {"type": "string"}}), &["path"], Risk::Safe),
            tdef("list_dir", "List a folder of the workspace.", json!({"path": {"type": "string"}}), &["path"], Risk::Safe),
            tdef("write_file", "Create or replace a text file in the workspace.", json!({"path": {"type": "string"}, "content": {"type": "string"}}), &["path", "content"], Risk::Ask),
            tdef("k8s_status", "Which cluster is active (name and kind) and the state of the dev namespace.", json!({}), &[], Risk::Safe),
            tdef("deploy_build", "Build a container image from the Dockerfile in a folder.", json!({"dir": {"type": "string"}, "image": {"type": "string"}}), &["dir", "image"], Risk::Ask),
            tdef("deploy_load", "Load a built image into the cluster.", json!({"image": {"type": "string"}}), &["image"], Risk::Ask),
            tdef("k8s_apply", "Apply a manifest to the dev namespace: `manifest_path` (a YAML file in the workspace, such as deploy/app.yaml) or `yaml` (text, saved to deploy/).", json!({"manifest_path": {"type": "string"}, "yaml": {"type": "string"}}), &[], Risk::Ask),
            tdef("deploy_status", "Show whether a deployment is ready.", json!({"name": {"type": "string"}}), &["name"], Risk::Safe),
        ];
        let rootc = root.clone();
        let reg = Reg::new(defs, Box::new(move |n, a| match n {
            "read_file" => Ok(SERVER_JS.into()),
            "list_dir" => Ok("Dockerfile\npackage.json\nserver.js".into()),
            "write_file" => {
                let p = rootc.join(a["path"].as_str().unwrap_or("x"));
                if let Some(d) = p.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                std::fs::write(&p, a["content"].as_str().unwrap_or("")).map_err(|e| e.to_string())?;
                Ok(format!("wrote {}", a["path"].as_str().unwrap_or("")))
            }
            "k8s_status" => Ok("Active cluster: local k3s (kind: dev, namespace web-dev), 1 node, no GPU".into()),
            "deploy_build" => Ok("Built image web:1 ".into()),
            "deploy_load" => Ok("Loaded web:1 into the cluster".into()),
            "k8s_apply" => Ok("checked: no problems; applied: deployment.apps/web created, service/web created".into()),
            "deploy_status" => Ok("Deployment web: 1/1 ready, Healthy; Service web: NodePort 30080".into()),
            _ => Ok("ok".into()),
        }));
        let system = format!("{}{}{}", crate::toolblock::COMPACT_PROMPT, crate::boardtools::workspace_prompt("Web", &root, "FEATURES.md"), skills_text(&pinned));
        note.say(format!("system prompt size: {} chars", system.len()));
        let ev = run_chat(rec, reg.clone(), &system, "build this and deploy it", true, 240, |_| Some(Verdict::Approve));
        let ans = final_of(&ev)?;
        let ran = reg.ran.lock().unwrap().clone();
        note.say(format!("order: {:?}; answer: {}", ran.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), head(&ans, 200)));
        let pos = |n: &str| ran.iter().position(|r| r.0 == n);
        let (Some(b), Some(l), Some(a), Some(s)) = (pos("deploy_build"), pos("deploy_load"), pos("k8s_apply"), ran.iter().rposition(|r| r.0 == "deploy_status")) else {
            return Err("one of deploy_build, deploy_load, k8s_apply, deploy_status never ran".into());
        };
        if !(b < l && l < a && a < s) {
            return Err("the steps ran in the wrong order".into());
        }
        let manifest = match (ran[a].1["yaml"].as_str(), ran[a].1["manifest_path"].as_str()) {
            (Some(y), _) => y.to_string(),
            (None, Some(p)) => std::fs::read_to_string(root.join(p)).map_err(|e| format!("k8s_apply named {p}, which cannot be read: {e}"))?,
            _ => return Err("k8s_apply was called with neither yaml nor manifest_path".into()),
        };
        note.say(format!("manifest head: {}", head(&manifest.replace('\n', " | "), 260)));
        let ml = manifest.to_lowercase();
        let need = [("a readiness/liveness probe", ml.contains("readinessprobe") || ml.contains("livenessprobe")), ("resource limits", ml.contains("limits")), ("non-root", ml.contains("runasnonroot") || ml.contains("runasuser")), ("a Service", ml.contains("kind: service"))];
        let missing: Vec<&str> = need.iter().filter(|n| !n.1).map(|n| n.0).collect();
        if !missing.is_empty() {
            return Err(format!("the manifest lacks {missing:?}"));
        }
        Ok(())
    });
}
