//! Chat tools for the data science toolkit (F-FLOW-31, `datasci_run.rs`): `datasci_status` (free: where the toolkit stands), `datasci_verify`
//! (asks; the self-test again in the Linux apps sandbox with no network) and `python_run` (asks every time, with the WHOLE script on the card;
//! the script runs in the same sandbox with no network, the toolkit read-only, one writable folder, 1 GB of memory, 64 processes and a time
//! limit of 60 seconds by default and 300 at most; what it printed and the files it made come back). Installing and upgrading stay on the
//! Store page (Setup), where the licence text and the size are shown first.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;

use crate::datasci_io::{self, Input, Kind, RunIo, Source, Want};
use crate::datasci_run::{self, RunReport};
use crate::settingstools::{arg_str, no_args, Entry, Env};

const TAB: &str = "LinuxApps";
pub const TOOLS: [&str; 3] = ["datasci_status", "datasci_verify", "python_run"];

pub use iyakki_shared::settingstools_datasci::PROMPT_RULE;

/// What the tools need of the machine (tests give a fake; the real one is the sandbox).
pub trait Backend: Send + Sync {
    fn state(&self, data: &Path) -> String;
    fn verify(&self, data: &Path, user: &str) -> Result<String, String>;
    fn run(&self, data: &Path, user: &str, script: &str, secs: u64) -> Result<RunReport, String>;
    /// [`Backend::run`] with inputs copied in, a frame cap and named outputs (Data tables, pictures). A backend that cannot says so.
    fn run_io(&self, data: &Path, user: &str, script: &str, secs: u64, io: &RunIo) -> Result<RunReport, String> {
        if io.is_empty() {
            self.run(data, user, script, secs)
        } else {
            Err("this computer cannot run a script with inputs and outputs".into())
        }
    }
}

struct Real;

impl Backend for Real {
    fn state(&self, data: &Path) -> String {
        datasci_run::state_text(data, &iyakki_shared::datasci::PINS).1
    }
    fn verify(&self, data: &Path, user: &str) -> Result<String, String> {
        datasci_run::verify_for(data, user)
    }
    fn run(&self, data: &Path, user: &str, script: &str, secs: u64) -> Result<RunReport, String> {
        datasci_run::run_script_for(data, user, script, secs)
    }
    fn run_io(&self, data: &Path, user: &str, script: &str, secs: u64, io: &RunIo) -> Result<RunReport, String> {
        datasci_run::run_script_for_io(data, user, script, secs, io)
    }
}

/// Read `inputs`, `outputs` and `frame_cap` of a `python_run` call. Every input is a file or folder inside the person's own home
/// (a Files path, or the file an open Data table came from); the script only ever gets copies.
pub fn io_of(env: &Env, a: &serde_json::Value) -> Result<RunIo, String> {
    let home = std::fs::canonicalize(crate::scope::home_root_for_writes(&env.user, &env.dir)).map_err(|e| e.to_string())?;
    let list = |k: &str| -> Result<Vec<serde_json::Value>, String> {
        match a.get(k) {
            None | Some(serde_json::Value::Null) => Ok(vec![]),
            Some(serde_json::Value::Array(v)) => Ok(v.clone()),
            _ => Err(format!("{k} is a list")),
        }
    };
    let mut io = RunIo::default();
    for i in list("inputs")? {
        let name = i["name"].as_str().ok_or("every input has a name")?.to_string();
        let from = match (i["path"].as_str(), i["handle"].as_str()) {
            (Some(p), None) => crate::scope::resolve(&home, p)?,
            (None, Some(h)) => crate::datatools::handle_source(&env.user, h).ok_or_else(|| format!("no Data table {h} is open"))?,
            _ => return Err(format!("the input {name} names a `path` or a Data `handle`, one of them")),
        };
        let real = std::fs::canonicalize(&from).map_err(|_| format!("the input {name} does not exist"))?;
        if !real.starts_with(&home) {
            return Err(format!("the input {name} is outside your home folder; the script only gets copies of what is there"));
        }
        let source = if i["frames"].as_bool() == Some(true) { Source::Frames(real) } else { Source::File(real) };
        io.inputs.push(Input { name, source });
    }
    for o in list("outputs")? {
        let file = o["file"].as_str().ok_or("every output has a file name")?.to_string();
        let kind = match o["kind"].as_str().unwrap_or("file") {
            "table" => Kind::Table,
            "image" => Kind::Image,
            "file" => Kind::File,
            other => return Err(format!("the output kind {other:?} is table, image or file")),
        };
        io.outputs.push(Want { file, kind });
    }
    io.frame_cap = match a.get("frame_cap") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => Some(v.as_u64().filter(|c| (1..=datasci_io::FRAME_CAP_MAX as u64).contains(c)).ok_or_else(|| format!("frame_cap is a whole number from 1 to {}", datasci_io::FRAME_CAP_MAX))? as usize),
    };
    if io.inputs.len() > datasci_io::INPUTS_MAX || io.outputs.len() > datasci_io::OUTPUTS_MAX {
        return Err(format!("at most {} inputs and {} outputs", datasci_io::INPUTS_MAX, datasci_io::OUTPUTS_MAX));
    }
    let mut names = std::collections::BTreeSet::new();
    for i in &io.inputs {
        datasci_io::plain_name(&i.name)?;
        if !names.insert(&i.name) {
            return Err(format!("the input name {} is used twice", i.name));
        }
    }
    for o in &io.outputs {
        datasci_io::plain_name(&o.file)?;
    }
    Ok(io)
}

pub fn entries(env: &Env) -> Vec<Entry> {
    entries_with(env, Arc::new(Real))
}

fn seconds(a: &serde_json::Value) -> Result<u64, String> {
    match a.get("seconds") {
        None | Some(serde_json::Value::Null) => Ok(datasci_run::RUN_SECS_DEFAULT),
        Some(v) => v.as_u64().filter(|s| (1..=datasci_run::RUN_SECS_MAX).contains(s)).ok_or_else(|| format!("seconds is a whole number from 1 to {}", datasci_run::RUN_SECS_MAX)),
    }
}

pub fn entries_with(env: &Env, b: Arc<dyn Backend>) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let (e, b) = (env.clone(), b.clone());
        v.push(Entry::read(TAB, "datasci_status", "Say whether the data science toolkit (Python with scikit-learn, NumPy, SciPy and OpenCV) is installed, up to date or behind this IYAKKI's pinned files.", no_args(), move |_| Ok(b.state(&e.dir))));
    }
    {
        let (e, b) = (env.clone(), b.clone());
        v.push(Entry::change(TAB, "datasci_verify", "Run the toolkit's self-test again in the Linux apps sandbox (no network). The person approves a card first.", no_args(), |_| "Run the data science toolkit self-test".to_string(), move |_| {
            let said = b.verify(&e.dir, &e.user)?;
            e.audit("datasci.verify", "ok");
            Ok(format!("The toolkit self-test passed: {said}"))
        }).admin());
    }
    {
        let (e, b) = (env.clone(), b);
        let schema = json!({"type": "object", "properties": {"script": {"type": "string", "description": "Python 3 source (up to 20000 characters). It runs with NumPy, SciPy, scikit-learn and OpenCV, no network, in an empty folder it may write files to; print results."}, "seconds": {"type": "integer", "description": "Time limit, 1 to 300 (default 60)."}, "inputs": {"type": "array", "description": "Up to 8 things copied to in/<name> before the run, read-only: {name, path} a file in the person's home, {name, handle} the file an open Data table came from, or {name, path, frames: true} a folder of jpg or png pictures copied as in/<name>/000001.jpg ... (at most frame_cap of them).", "items": {"type": "object"}}, "outputs": {"type": "array", "description": "Files the script writes to out/ that come back: {file, kind} with kind table (a CSV with a header, added to the Data tab), image (a JPEG or PNG, kept in Pictures/ai-output) or file. With no outputs listed, the files left in the work folder are kept.", "items": {"type": "object"}}, "frame_cap": {"type": "integer", "description": "Most frames of a frame folder copied in, 1 to 18000 (default 1800); the script sees it as IYAKKI_FRAME_CAP."}}, "required": ["script"]});
        v.push(Entry::change(TAB, "python_run", "Run a Python script with the data science toolkit in a sandbox with no network and give back what it printed and the files it made. The person reads the whole script on a card first.", schema, |a| format!("Run a Python script ({} characters, up to {} s)", a["script"].as_str().map_or(0, str::len), a["seconds"].as_u64().unwrap_or(datasci_run::RUN_SECS_DEFAULT)), move |a| {
            let script = arg_str(a, "script")?;
            let secs = seconds(a)?;
            let io = io_of(&e, a)?;
            let r = b.run_io(&e.dir, &e.user, &script, secs, &io)?;
            e.audit("datasci.python_run", &format!("{} characters, {} input(s), {} output(s) asked, {} file(s)", script.len(), io.inputs.len(), io.outputs.len(), r.files.len()));
            Ok(format!("{}{}", if r.stdout.trim().is_empty() { "The script printed nothing.".to_string() } else { format!("Output:\n{}", r.stdout.trim_end()) }, if r.files.is_empty() { String::new() } else { format!("\nFiles it made ({}): {}{}", r.files.len(), r.files.join(", "), r.keep.as_ref().map(|k: &PathBuf| format!(" (kept in {})", k.display())).unwrap_or_default()) }))
        }).with_preview(|a| {
            let names = |k: &str, f: &str| -> Vec<String> { a[k].as_array().map(|v| v.iter().map(|i| format!("{}{}", i[f].as_str().unwrap_or("?"), if k == "inputs" { if i["frames"].as_bool() == Some(true) { " (frames)" } else { "" } } else { "" })).collect()).unwrap_or_default() };
            let (ins, outs) = (names("inputs", "name"), names("outputs", "file"));
            format!(
                "This script will run with no network access, with no access to your files except {}:\n{}\n{}\n{}",
                if ins.is_empty() { "its own empty work folder".to_string() } else { format!("read-only copies of {} and its own work folder", ins.join(", ")) },
                if outs.is_empty() { "Files it leaves in the work folder are kept.".to_string() } else { format!("Only {} are taken back from out/ (tables are added to Data, pictures to Pictures/ai-output).", outs.join(", ")) },
                if a["frame_cap"].is_null() { String::new() } else { format!("Frame cap {}.", a["frame_cap"]) },
                a["script"].as_str().unwrap_or("")
            )
        })
        .with_refusal(|a| {
            let s = a["script"].as_str().unwrap_or("");
            if s.trim().is_empty() {
                Some("the script is empty".into())
            } else if s.len() > datasci_run::SCRIPT_MAX {
                Some(format!("the script is longer than {} characters", datasci_run::SCRIPT_MAX))
            } else {
                seconds(a).err().or_else(|| {
                    let bad_name = |k: &str, f: &str| a[k].as_array().into_iter().flatten().find_map(|i| datasci_io::plain_name(i[f].as_str().unwrap_or("")).err());
                    bad_name("inputs", "name").or_else(|| bad_name("outputs", "file")).or_else(|| a["frame_cap"].as_u64().filter(|c| !(1..=datasci_io::FRAME_CAP_MAX as u64).contains(c)).map(|_| format!("frame_cap is 1 to {}", datasci_io::FRAME_CAP_MAX)))
                })
            }
        }));
    }
    v
}

#[cfg(test)]
#[path = "settingstools_datasci_tests.rs"]
mod tests;
