//! Pre-flight for `deploy_build`: read the Dockerfile and check, before any build runs, that what it COPYs/ADDs exists in the build
//! context and that a `CMD ["node", "app.js"]` names a file that is there. A mismatch is a plain error with the files that ARE
//! in the folder, the likely app files one folder up and the fix. [`build_failure_summary`] turns a failed build's output into the first
//! failing step and its message, without the legacy-builder noise.

use std::path::{Path, PathBuf};

/// One instruction: upper-case keyword and its arguments (continuation lines joined, comments dropped).
fn instructions(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for raw in text.lines() {
        let l = raw.trim();
        if cur.is_empty() && (l.is_empty() || l.starts_with('#')) {
            continue;
        }
        if !cur.is_empty() && l.starts_with('#') {
            continue;
        }
        if let Some(s) = l.strip_suffix('\\') {
            cur.push_str(s);
            cur.push(' ');
            continue;
        }
        cur.push_str(l);
        let line = std::mem::take(&mut cur);
        let (k, rest) = line.split_once(char::is_whitespace).unwrap_or((line.as_str(), ""));
        out.push((k.to_uppercase(), rest.trim().to_string()));
    }
    if !cur.trim().is_empty() {
        let (k, rest) = cur.trim().split_once(char::is_whitespace).unwrap_or((cur.trim(), ""));
        out.push((k.to_uppercase(), rest.trim().to_string()));
    }
    out
}

/// Words of an instruction: JSON array form or whitespace separated.
fn words(args: &str) -> Vec<String> {
    let a = args.trim();
    if a.starts_with('[') {
        if let Ok(v) = serde_json::from_str::<Vec<String>>(a) {
            return v;
        }
    }
    a.split_whitespace().map(str::to_string).collect()
}

fn glob_seg(pat: &[char], s: &[char]) -> bool {
    match (pat.first(), s.first()) {
        (None, None) => true,
        (Some('*'), _) => glob_seg(&pat[1..], s) || (!s.is_empty() && glob_seg(pat, &s[1..])),
        (Some('?'), Some(_)) => glob_seg(&pat[1..], &s[1..]),
        (Some(p), Some(c)) if p == c => glob_seg(&pat[1..], &s[1..]),
        _ => false,
    }
}

/// Paths (relative to `base`, `/` separated) matching `pattern`; `**` matches any depth.
fn glob(base: &Path, pattern: &str) -> Vec<String> {
    fn walk(base: &Path, rel: &str, segs: &[&str], out: &mut Vec<String>, depth: usize) {
        if depth > 8 {
            return;
        }
        let Some((first, rest)) = segs.split_first() else {
            out.push(rel.to_string());
            return;
        };
        let here = if rel.is_empty() { base.to_path_buf() } else { base.join(rel) };
        let join = |n: &str| if rel.is_empty() { n.to_string() } else { format!("{rel}/{n}") };
        if *first == "**" {
            walk(base, rel, rest, out, depth + 1);
            if let Ok(rd) = std::fs::read_dir(&here) {
                for e in rd.flatten().filter(|e| e.path().is_dir()) {
                    walk(base, &join(&e.file_name().to_string_lossy()), segs, out, depth + 1);
                }
            }
            return;
        }
        if !first.contains(['*', '?']) {
            let next = join(first);
            if base.join(&next).exists() {
                walk(base, &next, rest, out, depth + 1);
            }
            return;
        }
        let pat: Vec<char> = first.chars().collect();
        if let Ok(rd) = std::fs::read_dir(&here) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let chars: Vec<char> = name.chars().collect();
                if glob_seg(&pat, &chars) {
                    walk(base, &join(&name), rest, out, depth + 1);
                }
            }
        }
    }
    let p = pattern.trim_start_matches("./").trim_start_matches('/');
    let segs: Vec<&str> = p.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
    let mut out = Vec::new();
    walk(base, "", &segs, &mut out, 0);
    out
}

fn ignored(patterns: &[String], rel: &str) -> bool {
    let mut ig = false;
    for p in patterns {
        let (neg, pat) = match p.strip_prefix('!') {
            Some(x) => (true, x),
            None => (false, p.as_str()),
        };
        let pat = pat.trim_start_matches("./").trim_start_matches('/').trim_end_matches('/');
        if pat.is_empty() {
            continue;
        }
        let hit = rel == pat || rel.starts_with(&format!("{pat}/")) || {
            let pc: Vec<char> = pat.chars().collect();
            let rc: Vec<char> = rel.chars().collect();
            glob_seg(&pc, &rc) || rel.split('/').next().is_some_and(|f| glob_seg(&pc, &f.chars().collect::<Vec<_>>()))
        };
        if hit {
            ig = !neg;
        }
    }
    ig
}

fn list(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned() + if e.path().is_dir() { "/" } else { "" }).collect()).unwrap_or_default();
    v.sort();
    v
}

fn app_like(name: &str) -> bool {
    let n = name.to_lowercase();
    ["package.json", "package-lock.json", "requirements.txt", "pyproject.toml", "go.mod", "pom.xml", "cargo.toml"].contains(&n.as_str()) || [".js", ".mjs", ".cjs", ".ts", ".py", ".go", ".html"].iter().any(|e| n.ends_with(e))
}

fn shown(v: &[String]) -> String {
    if v.is_empty() {
        return "(nothing)".into();
    }
    let mut s = v.iter().take(15).cloned().collect::<Vec<_>>().join(", ");
    if v.len() > 15 {
        s.push_str(&format!(", … {} more", v.len() - 15));
    }
    s
}

/// The script a `node`/`python` CMD or ENTRYPOINT runs, when it is obvious.
fn script_of(args: &str) -> Option<String> {
    let w = words(args);
    let w: Vec<&String> = if w.first().is_some_and(|f| f == "sh" || f == "/bin/sh") && w.get(1).is_some_and(|x| x == "-c") { return None } else { w.iter().collect() };
    let prog = w.first()?.rsplit('/').next()?.to_string();
    if !["node", "nodejs", "python", "python3", "bun", "deno"].contains(&prog.as_str()) {
        return None;
    }
    let s = w.iter().skip(1).find(|x| !x.starts_with('-'))?;
    ([".js", ".mjs", ".cjs", ".ts", ".py"].iter().any(|e| s.ends_with(e)) && !s.contains('$')).then(|| s.to_string())
}

/// Check `dockerfile` against the build context `dir` (`root`: the workspace, so the parent is only looked at inside it).
/// Err: the plain message, nothing should be built. Ok: warnings to show with the result.
pub fn check(dir: &Path, dockerfile: &Path, root: &Path) -> Result<Vec<String>, String> {
    check_target(dir, dockerfile, root, "")
}

/// The stage names of a multi-stage Dockerfile (`FROM image AS name`), in order.
pub fn stages(text: &str) -> Vec<String> {
    instructions(text).into_iter().filter(|(k, _)| k == "FROM").filter_map(|(_, a)| {
        let w: Vec<&str> = a.split_whitespace().collect();
        w.iter().position(|x| x.eq_ignore_ascii_case("as")).and_then(|i| w.get(i + 1)).map(|n| n.to_string())
    }).collect()
}

/// [`check`] for a build with `--target <target>` (empty: the last stage): only the stages up to the target are read, the target must
/// be a stage of the Dockerfile, and a COPY from `tests/` that is missing says so.
pub fn check_target(dir: &Path, dockerfile: &Path, root: &Path, target: &str) -> Result<Vec<String>, String> {
    let Ok(text) = std::fs::read_to_string(dockerfile) else { return Ok(vec![]) };
    let names = stages(&text);
    if !target.is_empty() && !names.iter().any(|n| n == target) {
        return Err(format!("the Dockerfile has no stage named `{target}` (target={target}); its stages: {}. Add `FROM <base> AS {target}` with the test commands, or build without a target.", if names.is_empty() { "none (it is a single-stage Dockerfile)".to_string() } else { names.join(", ") }));
    }
    // only the stages that are built: everything up to the end of the target stage
    let all = instructions(&text);
    let mut ins: Vec<(String, String)> = Vec::new();
    let mut in_target_done = false;
    for (k, a) in all {
        if k == "FROM" {
            if in_target_done {
                break;
            }
            let w: Vec<&str> = a.split_whitespace().collect();
            if !target.is_empty() && w.iter().position(|x| x.eq_ignore_ascii_case("as")).and_then(|i| w.get(i + 1)).is_some_and(|n| *n == target) {
                in_target_done = true;
            }
        }
        ins.push((k, a));
    }
    let ignore: Vec<String> = std::fs::read_to_string(dir.join(".dockerignore")).map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(str::to_string).collect()).unwrap_or_default();
    let here = list(dir);
    let dir_name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let parent: Option<PathBuf> = dir.parent().filter(|p| p.starts_with(root) && *p != dir).map(Path::to_path_buf);
    let hints = |missing_pkg: bool| {
        let likely: Vec<String> = parent.as_ref().map(|p| list(p).into_iter().filter(|n| !n.ends_with('/') && app_like(n)).collect()).unwrap_or_default();
        let mut s = format!("Files in that folder: {}.", shown(&here));
        if let Some(p) = &parent {
            if !likely.is_empty() {
                s.push_str(&format!(" Found these likely app files in the parent {}: {}.", p.display(), shown(&likely)));
            }
        }
        s.push_str(" Fix: keep the Dockerfile, the app files and package.json in ONE folder");
        if !likely.is_empty() {
            s.push_str(&format!(": move the Dockerfile next to the app files (or pass dir={})", parent.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| ".".into())));
        } else {
            s.push_str(&format!(" ({dir_name}) and add the missing files"));
        }
        if missing_pkg && !likely.iter().any(|n| n == "package.json") {
            s.push_str(", and create package.json");
        }
        s.push('.');
        s
    };
    let mut warnings = Vec::new();
    let mut workdir = String::new();
    let mut copied: Vec<String> = Vec::new();
    for (k, a) in ins {
        match k.as_str() {
            "WORKDIR" => workdir = a.trim().to_string(),
            "COPY" | "ADD" => {
                let w = words(&a);
                if w.iter().any(|x| x.starts_with("--from")) || a.contains("<<") {
                    continue;
                }
                let args: Vec<&String> = w.iter().filter(|x| !x.starts_with("--")).collect();
                if args.len() < 2 {
                    continue;
                }
                for src in &args[..args.len() - 1] {
                    if src.contains("://") || src.contains('$') {
                        continue;
                    }
                    let found = glob(dir, src);
                    if found.is_empty() {
                        let tests = if src.trim_start_matches("./").starts_with("test") { " (the tests folder the test stage copies is missing: create it with the tests, or fix the path)" } else { "" };
                        return Err(format!("{k} {src} matches no file in {}{tests}. {}", dir.display(), hints(src.contains("package"))));
                    }
                    if found.iter().all(|f| ignored(&ignore, f)) {
                        return Err(format!("{k} {src} only matches files that .dockerignore excludes in {}; remove the pattern from .dockerignore or copy something else.", dir.display()));
                    }
                    copied.extend(found);
                }
            }
            "CMD" | "ENTRYPOINT" if target.is_empty() => {
                if let Some(script) = script_of(&a) {
                    let base = script.trim_start_matches("./").to_string();
                    let in_ctx = dir.join(&base).exists() || (!workdir.is_empty() && copied.iter().any(|c| c.rsplit('/').next() == base.rsplit('/').next()));
                    let in_copied = copied.iter().any(|c| c == &base || c.ends_with(&format!("/{base}")));
                    if !in_ctx && !in_copied {
                        let alt: Vec<String> = here.iter().filter(|n| !n.ends_with('/') && (n.ends_with(".js") || n.ends_with(".py") || n.ends_with(".mjs"))).cloned().collect();
                        warnings.push(format!("{k} runs `{script}` but no such file is in {}{}", dir.display(), if alt.is_empty() { String::new() } else { format!(" (found {} instead: change the {k} or rename the file)", shown(&alt)) }));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(warnings)
}

/// The `USER` of the last stage of a Dockerfile (None: no USER, so the image runs as root).
pub fn final_user(text: &str) -> Option<String> {
    let mut user = None;
    for (k, a) in instructions(text) {
        match k.as_str() {
            "FROM" => user = None,
            "USER" => user = a.split_whitespace().next().map(str::to_string),
            _ => {}
        }
    }
    user
}

/// Is that `USER` value a numeric non-root id (`10001`, `10001:10001`)? Kubernetes can only verify `runAsNonRoot` for a number.
pub fn numeric_nonroot(user: &str) -> bool {
    let uid = user.split(':').next().unwrap_or("");
    uid.parse::<u64>().is_ok_and(|n| n != 0)
}

/// The Dockerfile of the workspace that builds image `base` (a folder of that name, the root, or the only Dockerfile in a
/// folder below the root).
pub fn find_dockerfile(root: &Path, base: &str) -> Option<PathBuf> {
    let has = |d: &Path| ["Dockerfile", "Containerfile"].into_iter().map(|f| d.join(f)).find(|p| p.is_file());
    if !base.is_empty() {
        if let Some(p) = has(&root.join(base)) {
            return Some(p);
        }
    }
    if let Some(p) = has(root) {
        return Some(p);
    }
    let found: Vec<PathBuf> = std::fs::read_dir(root).ok()?.flatten().filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.')).filter_map(|e| has(&e.path())).collect();
    (found.len() == 1).then(|| found[0].clone())
}

/// A failed build's output as (summary line, the output without noise, a note about the legacy builder or None).
pub fn build_failure_summary(out: &str) -> (String, String, Option<String>) {
    let noise = |l: &str| {
        let t = l.to_lowercase();
        t.contains("legacy builder is deprecated") || t.contains("install the buildx component") || t.contains("docs.docker.com/go/buildx") || t.contains("docs.docker.com/go/deprecated") || t.contains("buildkit is currently disabled")
    };
    let had_noise = out.lines().any(noise);
    let clean: Vec<&str> = out.lines().filter(|l| !noise(l)).collect();
    let step = clean.iter().rev().find(|l| l.starts_with("Step ") || l.starts_with("STEP ") || l.contains("ERROR [")).map(|l| l.trim().to_string());
    let msg = clean.iter().find(|l| {
        let t = l.trim_start();
        t.starts_with("COPY failed") || t.starts_with("ADD failed") || t.starts_with("ERROR") || t.starts_with("Error") || t.starts_with("error:") || t.contains("failed to compute cache key") || t.contains("returned a non-zero code")
    });
    let summary = match (step, msg) {
        (Some(s), Some(m)) if !s.contains(m.trim()) => format!("failed at `{s}`: {}", m.trim()),
        (Some(s), _) => format!("failed at `{s}`"),
        (None, Some(m)) => m.trim().to_string(),
        (None, None) => clean.iter().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string()).unwrap_or_default(),
    };
    let note = had_noise.then(|| "Note: docker's legacy builder printed a deprecation banner (installing docker buildx is optional and is not what failed).".to_string());
    (summary, clean.join("\n"), note)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-dockerlint-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const DF: &str = "FROM node:20-alpine\nWORKDIR /app\nCOPY package*.json .\nRUN npm install\nCOPY . .\nEXPOSE 3000\nCMD [\"node\",\"app.js\"]\n";

    /// The reported layout: the Dockerfile alone in nodejs-webserver/, index.js one folder up.
    #[test]
    fn a_dockerfile_alone_in_its_folder_with_the_app_one_level_up_is_explained_before_any_build() {
        let ws = tmp("layout");
        let dir = ws.join("nodejs-webserver");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Dockerfile"), DF).unwrap();
        std::fs::write(ws.join("index.js"), "console.log(1)").unwrap();
        let e = check(&dir, &dir.join("Dockerfile"), &ws).unwrap_err();
        assert!(e.contains("COPY package*.json matches no file in"), "{e}");
        assert!(e.contains("Files in that folder: Dockerfile."), "{e}");
        assert!(e.contains("Found these likely app files in the parent") && e.contains("index.js"), "{e}");
        assert!(e.contains("move the Dockerfile next to the app files (or pass dir=") && e.contains("create package.json"), "{e}");
    }

    #[test]
    fn globs_dockerignore_stages_and_a_matching_context_pass() {
        let d = tmp("ok");
        std::fs::write(d.join("Dockerfile"), DF).unwrap();
        std::fs::write(d.join("package.json"), "{}").unwrap();
        std::fs::write(d.join("app.js"), "").unwrap();
        assert_eq!(check(&d, &d.join("Dockerfile"), &d).unwrap(), Vec::<String>::new());
        // a COPY --from stage and a URL are not checked
        std::fs::write(d.join("Dockerfile"), "FROM a AS b\nFROM c\nCOPY --from=b /out /out\nADD https://x/y.tgz /y\nCOPY app.js /app.js\n").unwrap();
        assert!(check(&d, &d.join("Dockerfile"), &d).is_ok());
        // .dockerignore excluding the only match
        std::fs::write(d.join("Dockerfile"), "FROM c\nCOPY app.js /app.js\n").unwrap();
        std::fs::write(d.join(".dockerignore"), "app.js\n").unwrap();
        assert!(check(&d, &d.join("Dockerfile"), &d).unwrap_err().contains(".dockerignore"));
        // a directory source and a deeper glob
        std::fs::remove_file(d.join(".dockerignore")).unwrap();
        std::fs::create_dir_all(d.join("src/a")).unwrap();
        std::fs::write(d.join("src/a/x.py"), "").unwrap();
        std::fs::write(d.join("Dockerfile"), "FROM c\nCOPY src /src\nCOPY src/*/x.py /x\nCOPY nothing.txt /n\n").unwrap();
        let e = check(&d, &d.join("Dockerfile"), &d).unwrap_err();
        assert!(e.contains("COPY nothing.txt matches no file"), "{e}");
    }

    #[test]
    fn a_cmd_naming_a_missing_script_warns_and_names_the_file_that_is_there() {
        let d = tmp("cmd");
        std::fs::write(d.join("Dockerfile"), "FROM node\nWORKDIR /app\nCOPY . .\nCMD [\"node\", \"app.js\"]\n").unwrap();
        std::fs::write(d.join("index.js"), "").unwrap();
        let w = check(&d, &d.join("Dockerfile"), &d).unwrap();
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("runs `app.js`") && w[0].contains("index.js instead"), "{w:?}");
        std::fs::write(d.join("app.js"), "").unwrap();
        assert!(check(&d, &d.join("Dockerfile"), &d).unwrap().is_empty());
        // shell-form and python, not obvious: no warning
        std::fs::write(d.join("Dockerfile"), "FROM p\nCOPY . .\nCMD python3 -u main.py\n").unwrap();
        assert_eq!(check(&d, &d.join("Dockerfile"), &d).unwrap().len(), 1);
        std::fs::write(d.join("Dockerfile"), "FROM p\nCOPY . .\nCMD sh -c \"npm start\"\n").unwrap();
        assert!(check(&d, &d.join("Dockerfile"), &d).unwrap().is_empty());
    }

    #[test]
    fn the_real_error_is_shown_without_the_legacy_builder_banner() {
        let out = "DEPRECATED: The legacy builder is deprecated and will be removed in a future release.\n            Install the buildx component to build images with BuildKit:\n            https://docs.docker.com/go/buildx/\n\nSending build context to Docker daemon  2.048kB\nStep 1/7 : FROM node:20-alpine\n ---> abc\nStep 3/7 : COPY package*.json .\nCOPY failed: no source files were specified\n";
        let (s, clean, note) = build_failure_summary(out);
        assert_eq!(s, "failed at `Step 3/7 : COPY package*.json .`: COPY failed: no source files were specified");
        assert!(!clean.contains("legacy builder") && !clean.contains("buildx"), "{clean}");
        assert!(note.unwrap().contains("buildx is optional"));
        let (s2, _, n2) = build_failure_summary("Error: crun: executable not found\n");
        assert!(s2.contains("executable not found") && n2.is_none());
    }
}
