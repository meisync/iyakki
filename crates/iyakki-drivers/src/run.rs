//! The only place this crate starts programs: `modprobe` and `insmod` (which already ship with the image), through a
//! trait so tests record the calls instead.

use std::process::{Command, Stdio};

pub trait Runner {
    fn run(&self, program: &str, args: &[&str]) -> Result<(), String>;
}

/// Runs the real programs.
pub struct System;

impl Runner for System {
    fn run(&self, program: &str, args: &[&str]) -> Result<(), String> {
        let out = Command::new(program).args(args).stdin(Stdio::null()).output().map_err(|e| format!("{program}: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("{program} {}: {}", args.join(" "), err.trim()))
        }
    }
}

/// Does nothing and succeeds: used when the files are in a sandbox folder (not the device), so no real driver is touched.
pub struct Dry;

impl Runner for Dry {
    fn run(&self, _program: &str, _args: &[&str]) -> Result<(), String> {
        Ok(())
    }
}

/// Records calls (for tests). `fail` makes calls whose joined text contains it fail.
#[derive(Default)]
pub struct Recorder {
    pub calls: std::sync::Mutex<Vec<String>>,
    pub fail: Option<String>,
}

impl Runner for Recorder {
    fn run(&self, program: &str, args: &[&str]) -> Result<(), String> {
        let line = format!("{program} {}", args.join(" "));
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(line.clone());
        match &self.fail {
            Some(f) if line.contains(f.as_str()) => Err(format!("{line}: failed")),
            _ => Ok(()),
        }
    }
}
