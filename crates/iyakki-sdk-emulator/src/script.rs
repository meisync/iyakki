//! Scripts: a JSON list of input steps, with checks and pictures compared with golden PNGs (`iyakki sdk test`).
//!
//! ```json
//! { "size": [800, 480], "mode": "touch", "dark": true, "tolerance": {"channel": 2, "percent": 0.1},
//!   "steps": [ {"shot": "start"}, {"tap": [30, 80]}, {"expect_text": "1"}, {"shot": "after-tap"} ] }
//! ```
//! Steps: `tap [x,y]`, `move [x,y]`, `text "abc"`, `key code` (with optional `mods`), `tick ms`, `hit id`, `resize [w,h]`, `theme "dark"|"light"`,
//! `shot "name"` (compare with the golden picture), `expect_text "..."`, `expect_no_text "..."`, `expect_call "domain.method"`,
//! `expect_prompt "domain.method"`, `expect_refused "domain.method"` (the tile must have used a capability it did not declare).

use crate::emulator::{Emulator, Input, Mode};
use crate::golden::{compare, picture_hash, Outcome, Tolerance};
use crate::png;
use crate::project::Project;
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Default)]
pub struct ScriptReport {
    pub name: String,
    pub steps: usize,
    /// One line per picture: name, hash, result.
    pub shots: Vec<String>,
    pub failures: Vec<String>,
}

impl ScriptReport {
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

fn plain_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 60 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn pair(v: &Value, what: &str) -> Result<(i32, i32), String> {
    let a = v.as_array().filter(|a| a.len() == 2).ok_or_else(|| format!("{what} takes [x, y]"))?;
    match (a[0].as_i64(), a[1].as_i64()) {
        (Some(x), Some(y)) if x.abs() < 32768 && y.abs() < 32768 => Ok((x as i32, y as i32)),
        _ => Err(format!("{what} takes two whole numbers")),
    }
}

/// Run `script` (JSON text) against the tile in `project`. `golden_dir` holds `<name>--<shot>.png`; with `update` the pictures are
/// written instead of compared. The fake services use `scratch` for files.
pub fn run(project: &Project, name: &str, script: &str, golden_dir: &Path, update: bool, scratch: &Path) -> ScriptReport {
    let mut rep = ScriptReport { name: name.into(), ..Default::default() };
    let v: Value = match serde_json::from_str(script) {
        Ok(v) => v,
        Err(e) => {
            rep.failures.push(format!("the script is not valid JSON: {e}"));
            return rep;
        }
    };
    if !plain_name(name) {
        rep.failures.push(format!("script name {name:?}: letters, digits, - and _ only"));
        return rep;
    }
    let size = match v.get("size") {
        Some(s) => match pair(s, "size") {
            Ok((w, h)) if (100..=4096).contains(&w) && (100..=4096).contains(&h) => (w, h),
            Ok(_) => {
                rep.failures.push("size: between 100 and 4096 in each direction".into());
                return rep;
            }
            Err(e) => {
                rep.failures.push(e);
                return rep;
            }
        },
        None => (800, 480),
    };
    let mode = match v.get("mode").and_then(Value::as_str) {
        None | Some("touch") => Mode::Touch,
        Some("mouse") => Mode::Mouse,
        Some(o) => {
            rep.failures.push(format!("mode {o:?}: touch or mouse"));
            return rep;
        }
    };
    let dark = v.get("dark").and_then(Value::as_bool).unwrap_or(false);
    let tol = Tolerance {
        channel: v.pointer("/tolerance/channel").and_then(Value::as_u64).unwrap_or(0).min(255) as u8,
        percent: v.pointer("/tolerance/percent").and_then(Value::as_f64).unwrap_or(0.0).clamp(0.0, 100.0),
    };
    let Some(steps) = v.get("steps").and_then(Value::as_array) else {
        rep.failures.push("the script has no \"steps\" list".into());
        return rep;
    };
    // read every step first so a typo fails before anything runs
    let mut parsed: Vec<(String, Value)> = Vec::new();
    for (i, s) in steps.iter().enumerate() {
        match s.as_object().filter(|o| o.keys().filter(|k| *k != "mods").count() == 1) {
            Some(o) => {
                let key = o.keys().find(|k| *k != "mods").cloned().unwrap_or_default();
                if !["tap", "move", "text", "key", "tick", "hit", "resize", "theme", "shot", "expect_text", "expect_no_text", "expect_call", "expect_prompt", "expect_refused"].contains(&key.as_str()) {
                    rep.failures.push(format!("step {}: unknown step {key:?}", i + 1));
                }
                parsed.push((key, s.clone()));
            }
            None => rep.failures.push(format!("step {}: one action per step", i + 1)),
        }
    }
    if !rep.failures.is_empty() {
        return rep;
    }
    let mut emu = match Emulator::new(project.clone(), scratch.to_path_buf(), size, mode, dark) {
        Ok(e) => e,
        Err(e) => {
            rep.failures.push(format!("the tile did not start: {e}"));
            return rep;
        }
    };
    let expects_refusal = parsed.iter().any(|(k, _)| k == "expect_refused");
    for (i, (key, step)) in parsed.iter().enumerate() {
        let n = i + 1;
        rep.steps = n;
        let arg = &step[key.as_str()];
        let input: Result<Option<Input>, String> = match key.as_str() {
            "tap" => pair(arg, "tap").map(|(x, y)| Some(Input::Tap(x, y))),
            "move" => pair(arg, "move").map(|(x, y)| Some(Input::Move(x, y))),
            "text" => arg.as_str().map(|t| Some(Input::Text(t.to_string()))).ok_or_else(|| "text takes a string".into()),
            "key" => arg.as_u64().map(|c| Some(Input::Key { code: c as u32, mods: step.get("mods").and_then(Value::as_u64).unwrap_or(0) as u8 })).ok_or_else(|| "key takes a key code".into()),
            "tick" => arg.as_u64().filter(|m| *m <= 60_000).map(|m| Some(Input::Tick(m as u32))).ok_or_else(|| "tick takes milliseconds up to 60000".into()),
            "hit" => arg.as_u64().map(|id| Some(Input::Hit(id as u32))).ok_or_else(|| "hit takes a region id".into()),
            "resize" => pair(arg, "resize").map(|(w, h)| Some(Input::Resize(w, h))),
            "theme" => match arg.as_str() {
                Some("dark") => Ok(Some(Input::Theme(true))),
                Some("light") => Ok(Some(Input::Theme(false))),
                _ => Err("theme takes \"dark\" or \"light\"".into()),
            },
            _ => Ok(None),
        };
        match input {
            Err(e) => {
                rep.failures.push(format!("step {n}: {e}"));
                return rep;
            }
            Ok(Some(inp)) => {
                if let Err(e) = emu.apply(inp) {
                    // a script that checks for the refusal (expect_refused) expects this error
                    if !(expects_refusal && e.starts_with("REFUSED")) {
                        rep.failures.push(format!("step {n}: {e}"));
                        return rep;
                    }
                }
                continue;
            }
            Ok(None) => {}
        }
        let want = arg.as_str().unwrap_or("");
        match key.as_str() {
            "expect_text" => {
                if !emu.texts().iter().any(|t| t.contains(want)) {
                    rep.failures.push(format!("step {n}: expected the picture to show {want:?}; it shows {:?}", emu.texts()));
                }
            }
            "expect_no_text" => {
                if emu.texts().iter().any(|t| t.contains(want)) {
                    rep.failures.push(format!("step {n}: the picture shows {want:?} but should not"));
                }
            }
            "expect_call" => {
                if !emu.services.calls().iter().any(|c| format!("{}.{}", c.domain, c.method) == want && !c.refused) {
                    rep.failures.push(format!("step {n}: expected a call to {want}; the calls so far: {:?}", emu.services.calls().iter().map(|c| format!("{}.{}", c.domain, c.method)).collect::<Vec<_>>()));
                }
            }
            "expect_prompt" => {
                if !emu.services.prompts().iter().any(|p| format!("{}.{}", p.domain, p.method) == want) {
                    rep.failures.push(format!("step {n}: expected the person to be asked about {want}"));
                }
            }
            "expect_refused" => {
                if !emu.services.violations().iter().any(|v| v == want) {
                    rep.failures.push(format!("step {n}: expected the tile to be refused for {want}"));
                }
            }
            "shot" => {
                if !plain_name(want) {
                    rep.failures.push(format!("step {n}: picture name {want:?}: letters, digits, - and _ only"));
                    return rep;
                }
                let img = emu.image();
                let hash = picture_hash(&img.rgba);
                let path = golden_dir.join(format!("{name}--{want}.png"));
                let bytes = png::encode(img.w as u32, img.h as u32, &img.rgba);
                if update {
                    let wrote = std::fs::create_dir_all(golden_dir).and_then(|_| std::fs::write(&path, &bytes));
                    match wrote {
                        Ok(()) => rep.shots.push(format!("{want}: written {} (sha256 {})", path.display(), &hash[..12])),
                        Err(e) => rep.failures.push(format!("step {n}: cannot write {}: {e}", path.display())),
                    }
                    continue;
                }
                let outcome = match std::fs::read(&path) {
                    Ok(g) => compare(img.w as u32, img.h as u32, &img.rgba, &g, tol),
                    Err(_) => Outcome::Unreadable(format!("no golden picture at {} (run `iyakki sdk test --update` to create it)", path.display())),
                };
                let line = format!("{want}: sha256 {}", &hash[..12]);
                if outcome.passed() {
                    rep.shots.push(format!("{line}, {}", if let Outcome::Within { pixels, max } = outcome { format!("within tolerance ({pixels} pixels differ, largest step {max})") } else { "same".into() }));
                } else {
                    // keep the actual picture beside the golden so the developer can look at both
                    let actual = golden_dir.join("_actual").join(format!("{name}--{want}.png"));
                    let _ = std::fs::create_dir_all(golden_dir.join("_actual")).and_then(|_| std::fs::write(&actual, &bytes));
                    rep.shots.push(format!("{line}, FAILED"));
                    rep.failures.push(format!("step {n}: picture {want:?} differs: {outcome:?}; the actual picture is {}", actual.display()));
                }
            }
            _ => {}
        }
    }
    rep
}
