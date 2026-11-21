//! What goes into and comes out of a `python_run` (F-FLOW-31, docs/development/DATA-SCIENCE-ADDON-PLAN.md section 4.1).
//!
//! * Inputs: at most 8 files or frame folders the person named (their own Files or an open Data table), each COPIED to `/work/in/<name>`
//!   before the run (the script never sees the person's folders), refused when a name is not plain, a source is a link or too big.
//!   A frame folder is a folder of numbered pictures (jpg or png, not mixed) copied as `000001.jpg`, `000002.jpg` ... so that
//!   `cv2.VideoCapture("in/<name>/%06d.jpg")` reads them; at most `frame_cap` of them (default 1800, at most 18000: the plan's cap),
//!   and the script is told the cap in `IYAKKI_FRAME_CAP`.
//! * Outputs: only the files the request lists are collected from `/work/out/`, at most 20 of 50 MB each. A `table` is checked (UTF-8,
//!   a header row, the same number of columns in every row) and handed to a sink that registers it with the Data service; an `image` must
//!   be a JPEG or a PNG; a `file` is only kept. Anything else the script wrote is left behind and removed with the work folder.
// SPDX-License-Identifier: Apache-2.0

use std::path::{Path, PathBuf};

pub const INPUTS_MAX: usize = 8;
pub const INPUT_BYTES: u64 = 20 << 20;
pub const INPUTS_TOTAL_BYTES: u64 = 64 << 20;
pub const FRAME_CAP_DEFAULT: usize = 1800;
pub const FRAME_CAP_MAX: usize = 18_000;
pub const OUTPUTS_MAX: usize = 20;
pub const OUTPUT_BYTES: u64 = 50 << 20;
/// The most rows a table output may have.
pub const TABLE_ROWS_MAX: usize = 1_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// One file, already resolved by the caller to somewhere the person may read.
    File(PathBuf),
    /// A folder of pictures taken as frames.
    Frames(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    pub name: String,
    pub source: Source,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Table,
    Image,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Want {
    pub file: String,
    pub kind: Kind,
}

/// What a run is given and asked to hand back. Empty means the plain run of F-FLOW-31 (no inputs, top-level files kept).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunIo {
    pub inputs: Vec<Input>,
    pub outputs: Vec<Want>,
    pub frame_cap: Option<usize>,
}

impl RunIo {
    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty() && self.outputs.is_empty() && self.frame_cap.is_none()
    }
    pub fn cap(&self) -> usize {
        self.frame_cap.unwrap_or(FRAME_CAP_DEFAULT)
    }
}

/// Registers what a run made with the Data service (a table) or the person's pictures (an image); tests give a fake.
pub trait Sink: Send + Sync {
    /// A checked CSV at `file`; the answer is one sentence for the model (where it shows).
    fn table(&self, name: &str, file: &Path, rows: usize, columns: usize) -> Result<String, String>;
    /// A checked JPEG or PNG at `file`.
    fn image(&self, name: &str, file: &Path) -> Result<String, String>;
}

/// A name of one file or folder: letters, digits, `.`, `_`, `-`; no leading dot; at most 64 characters.
pub fn plain_name(n: &str) -> Result<(), String> {
    if n.is_empty() || n.len() > 64 || n.starts_with('.') || !n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
        return Err(format!("{n:?} is not a plain name (letters, digits, . _ - only, at most 64, no leading dot)"));
    }
    Ok(())
}

fn is_link_or_missing(p: &Path) -> Result<std::fs::Metadata, String> {
    let m = std::fs::symlink_metadata(p).map_err(|_| "a named input does not exist".to_string())?;
    if m.file_type().is_symlink() {
        return Err("an input is a link, which is never followed".into());
    }
    Ok(m)
}

fn picture_ext(p: &Path) -> Option<&'static str> {
    match p.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => Some("jpg"),
        "png" => Some("png"),
        _ => None,
    }
}

/// Make `work/in` and `work/out`, copy the inputs in, and say what was put there (one line each, for the card and the audit).
/// Nothing is left half done: any refusal removes `work/in` again.
pub fn prepare(work: &Path, io: &RunIo) -> Result<Vec<String>, String> {
    if io.inputs.len() > INPUTS_MAX {
        return Err(format!("at most {INPUTS_MAX} inputs"));
    }
    if io.outputs.len() > OUTPUTS_MAX {
        return Err(format!("at most {OUTPUTS_MAX} outputs"));
    }
    if let Some(c) = io.frame_cap {
        if !(1..=FRAME_CAP_MAX).contains(&c) {
            return Err(format!("the frame cap is 1 to {FRAME_CAP_MAX}"));
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for i in &io.inputs {
        plain_name(&i.name)?;
        if !seen.insert(i.name.clone()) {
            return Err(format!("the input name {} is used twice", i.name));
        }
    }
    let (inn, outd) = (work.join("in"), work.join("out"));
    std::fs::create_dir_all(&inn).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&outd).map_err(|e| e.to_string())?;
    let r = (|| -> Result<Vec<String>, String> {
        let (mut notes, mut total) = (Vec::new(), 0u64);
        for i in &io.inputs {
            match &i.source {
                Source::File(p) => {
                    let m = is_link_or_missing(p)?;
                    if !m.is_file() {
                        return Err(format!("the input {} is not a file", i.name));
                    }
                    if m.len() > INPUT_BYTES {
                        return Err(format!("the input {} is larger than {} MB", i.name, INPUT_BYTES >> 20));
                    }
                    total += m.len();
                    if total > INPUTS_TOTAL_BYTES {
                        return Err(format!("the inputs are larger than {} MB together", INPUTS_TOTAL_BYTES >> 20));
                    }
                    std::fs::copy(p, inn.join(&i.name)).map_err(|e| e.to_string())?;
                    notes.push(format!("in/{} ({} bytes)", i.name, m.len()));
                }
                Source::Frames(dir) => {
                    if !is_link_or_missing(dir)?.is_dir() {
                        return Err(format!("the input {} is not a folder", i.name));
                    }
                    let mut pics: Vec<PathBuf> = std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).filter(|p| picture_ext(p).is_some() && std::fs::symlink_metadata(p).map(|m| m.is_file()).unwrap_or(false)).collect();
                    pics.sort();
                    let Some(ext) = pics.first().and_then(|p| picture_ext(p)) else { return Err(format!("the folder of {} holds no jpg or png pictures", i.name)) };
                    if pics.iter().any(|p| picture_ext(p) != Some(ext)) {
                        return Err(format!("the folder of {} mixes jpg and png pictures; frames must be one kind", i.name));
                    }
                    pics.truncate(io.cap());
                    let to = inn.join(&i.name);
                    std::fs::create_dir_all(&to).map_err(|e| e.to_string())?;
                    for (n, p) in pics.iter().enumerate() {
                        let len = std::fs::metadata(p).map_err(|e| e.to_string())?.len();
                        if len > INPUT_BYTES {
                            return Err(format!("a picture of {} is larger than {} MB", i.name, INPUT_BYTES >> 20));
                        }
                        total += len;
                        if total > INPUTS_TOTAL_BYTES {
                            return Err(format!("the inputs are larger than {} MB together", INPUTS_TOTAL_BYTES >> 20));
                        }
                        std::fs::copy(p, to.join(format!("{:06}.{ext}", n + 1))).map_err(|e| e.to_string())?;
                    }
                    notes.push(format!("in/{}/%06d.{ext} ({} frames, cap {})", i.name, pics.len(), io.cap()));
                }
            }
        }
        Ok(notes)
    })();
    match r {
        Ok(n) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                // the copies are read-only to the script
                fn lock(p: &Path) {
                    if let Ok(rd) = std::fs::read_dir(p) {
                        for e in rd.flatten() {
                            let q = e.path();
                            if q.is_dir() {
                                lock(&q);
                            } else {
                                let _ = std::fs::set_permissions(&q, std::fs::Permissions::from_mode(0o444));
                            }
                        }
                    }
                    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o555));
                }
                lock(&inn);
            }
            Ok(n)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&inn);
            Err(e)
        }
    }
}

/// Make the read-only copies writable again so the work folder can be removed.
pub fn unlock(work: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fn open_up(p: &Path) {
            let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755));
            if let Ok(rd) = std::fs::read_dir(p) {
                for e in rd.flatten() {
                    if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        open_up(&e.path());
                    }
                }
            }
        }
        open_up(&work.join("in"));
    }
    #[cfg(not(unix))]
    let _ = work;
}

/// Count the rows and columns of a CSV and refuse one that is not a table: not UTF-8, empty, a row with another number of columns than
/// the header, more than [`TABLE_ROWS_MAX`] rows. Quotes (with doubled quotes inside) may hold commas and new lines. `(rows without the header, columns)`.
pub fn check_csv(bytes: &[u8]) -> Result<(usize, usize), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "the table is not UTF-8 text".to_string())?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (mut cols_per_row, mut field_open, mut in_q, mut cols, mut any) = (Vec::<usize>::new(), false, false, 1usize, false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        any = true;
        match c {
            '"' if in_q => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                } else {
                    in_q = false;
                }
            }
            '"' if !field_open => {
                in_q = true;
                field_open = true;
            }
            ',' if !in_q => {
                cols += 1;
                field_open = false;
            }
            '\r' if !in_q => {}
            '\n' if !in_q => {
                cols_per_row.push(cols);
                cols = 1;
                field_open = false;
                any = false;
                if cols_per_row.len() > TABLE_ROWS_MAX + 1 {
                    return Err(format!("the table has more than {TABLE_ROWS_MAX} rows"));
                }
            }
            _ => field_open = true,
        }
    }
    if in_q {
        return Err("a quoted cell is never closed".into());
    }
    if any {
        cols_per_row.push(cols);
    }
    let Some(&header) = cols_per_row.first() else { return Err("the table is empty".into()) };
    if let Some(bad) = cols_per_row.iter().position(|c| *c != header) {
        return Err(format!("row {} has {} columns but the header has {header}", bad + 1, cols_per_row[bad]));
    }
    Ok((cols_per_row.len() - 1, header))
}

fn is_picture(b: &[u8]) -> bool {
    b.starts_with(&[0xFF, 0xD8, 0xFF]) || b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
}

/// Collect the wanted files of `work/out` into `keep`, check them by kind and hand tables and pictures to the sink. A file the script did
/// not make is said in words; the others are still collected. One note per file.
pub fn collect(work: &Path, io: &RunIo, keep: &Path, sink: &dyn Sink) -> Result<Vec<String>, String> {
    let mut notes = Vec::new();
    for w in &io.outputs {
        plain_name(&w.file)?;
        let src = work.join("out").join(&w.file);
        let Ok(m) = std::fs::symlink_metadata(&src) else {
            notes.push(format!("{}: the script did not make it", w.file));
            continue;
        };
        if !m.is_file() {
            notes.push(format!("{}: not a plain file, left out", w.file));
            continue;
        }
        if m.len() > OUTPUT_BYTES {
            notes.push(format!("{}: larger than {} MB, left out", w.file, OUTPUT_BYTES >> 20));
            continue;
        }
        let bytes = std::fs::read(&src).map_err(|e| e.to_string())?;
        let note = match w.kind {
            Kind::Table => match check_csv(&bytes) {
                Ok((rows, cols)) => {
                    std::fs::create_dir_all(keep).map_err(|e| e.to_string())?;
                    std::fs::write(keep.join(&w.file), &bytes).map_err(|e| e.to_string())?;
                    match sink.table(&w.file, &keep.join(&w.file), rows, cols) {
                        Ok(said) => format!("{}: table, {rows} rows, {cols} columns. {said}", w.file),
                        Err(e) => format!("{}: table kept in {} but not added to Data: {e}", w.file, keep.display()),
                    }
                }
                Err(e) => format!("{}: not a table ({e}), left out", w.file),
            },
            Kind::Image if is_picture(&bytes) => {
                std::fs::create_dir_all(keep).map_err(|e| e.to_string())?;
                std::fs::write(keep.join(&w.file), &bytes).map_err(|e| e.to_string())?;
                match sink.image(&w.file, &keep.join(&w.file)) {
                    Ok(said) => format!("{}: picture ({} bytes). {said}", w.file, bytes.len()),
                    Err(e) => format!("{}: picture kept in {} but not added to your pictures: {e}", w.file, keep.display()),
                }
            }
            Kind::Image => format!("{}: not a JPEG or PNG picture, left out", w.file),
            Kind::File => {
                std::fs::create_dir_all(keep).map_err(|e| e.to_string())?;
                std::fs::write(keep.join(&w.file), &bytes).map_err(|e| e.to_string())?;
                format!("{} ({} bytes)", w.file, bytes.len())
            }
        };
        notes.push(note);
    }
    Ok(notes)
}

#[cfg(test)]
#[path = "datasci_io_tests.rs"]
mod tests;
