//! The commands: `check`, `test`, `shot`, `emulate`. `run` returns the exit code and writes to `out`.

use crate::emulator::{Emulator, Mode, SIZES};
use crate::project::Project;
use crate::{png, script};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const USAGE: &str = "usage: iyakki-sdk-emulator <command> <tile folder>
  check TILE                       load the tile, print the review text of the install card, refuse a bad manifest
  test TILE [--script FILE]... [--golden DIR] [--update] [--scratch DIR]
                                   run input scripts (default: TILE/tests/emulator/*.json) and compare each shot with its golden PNG
  shot TILE --out FILE.png [--size WxH] [--dark]
                                   draw the first picture of the tile to a PNG
  emulate TILE [--size WxH] [--mouse] [--dark]
                                   open the emulator window (needs the iyakki-sdk-emulate-window program; see docs/SDK-EMULATOR.md)";

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

pub fn parse_size(s: &str) -> Result<(i32, i32), String> {
    let (w, h) = s.split_once('x').ok_or("size: WIDTHxHEIGHT, for example 800x480")?;
    match (w.parse::<i32>(), h.parse::<i32>()) {
        (Ok(w), Ok(h)) if (100..=4096).contains(&w) && (100..=4096).contains(&h) => Ok((w, h)),
        _ => Err("size: WIDTHxHEIGHT between 100 and 4096".into()),
    }
}

fn scratch_for(p: &Project, args: &[String]) -> PathBuf {
    flag(args, "--scratch").map(PathBuf::from).unwrap_or_else(|| p.dir.join(".emulator-scratch"))
}

pub fn run(args: &[String], out: &mut dyn Write) -> i32 {
    match run_inner(args, out) {
        Ok(code) => code,
        Err(e) => {
            let _ = writeln!(out, "{e}");
            1
        }
    }
}

fn run_inner(args: &[String], out: &mut dyn Write) -> Result<i32, String> {
    let w = |out: &mut dyn Write, s: String| {
        let _ = writeln!(out, "{s}");
    };
    let (cmd, tile) = match (args.first().map(String::as_str), args.get(1)) {
        (Some(c @ ("check" | "test" | "shot" | "emulate")), Some(t)) => (c, t.clone()),
        _ => return Err(USAGE.into()),
    };
    let project = Project::load(Path::new(&tile))?;
    let size = match flag(args, "--size") {
        Some(s) => parse_size(&s)?,
        None => SIZES[0],
    };
    let dark = args.iter().any(|a| a == "--dark");
    let mode = if args.iter().any(|a| a == "--mouse") { Mode::Mouse } else { Mode::Touch };
    // every command starts the tile once: a tile that cannot load is refused here, with the device's own sentence
    let scratch = scratch_for(&project, args);
    // (`test` starts the tile inside each script instead, so a script can expect a refusal at start-up)
    let emu = if cmd == "test" { None } else { Some(Emulator::new(project.clone(), scratch.clone(), size, mode, dark)?) };
    w(out, project.review_text().trim_end().to_string());
    match cmd {
        "check" => {
            w(out, format!("ok: {} {} starts at {}x{}", project.name, project.version, size.0, size.1));
            Ok(0)
        }
        "shot" => {
            let to = flag(args, "--out").ok_or("shot needs --out FILE.png")?;
            let img = emu.ok_or("no tile")?.image();
            std::fs::write(&to, png::encode(img.w as u32, img.h as u32, &img.rgba)).map_err(|e| format!("{to}: {e}"))?;
            w(out, format!("wrote {to}"));
            Ok(0)
        }
        "emulate" => {
            let exe = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("iyakki-sdk-emulate-window")));
            let prog = exe.filter(|p| p.is_file()).unwrap_or_else(|| PathBuf::from("iyakki-sdk-emulate-window"));
            let mut c = std::process::Command::new(&prog);
            c.arg(&tile).args(args.iter().skip(2));
            match c.status() {
                Ok(s) => Ok(s.code().unwrap_or(1)),
                Err(_) => Err("the window program iyakki-sdk-emulate-window was not found next to this program or on PATH. Build it with: cargo build --release --manifest-path crates/iyakki-gui/Cargo.toml --bin iyakki-sdk-emulate-window".into()),
            }
        }
        _ => {
            let golden = flag(args, "--golden").map(PathBuf::from).unwrap_or_else(|| project.dir.join("tests/emulator/golden"));
            let update = args.iter().any(|a| a == "--update");
            let mut scripts: Vec<PathBuf> = args.iter().enumerate().filter(|(_, a)| *a == "--script").filter_map(|(i, _)| args.get(i + 1)).map(PathBuf::from).collect();
            if scripts.is_empty() {
                if let Ok(rd) = std::fs::read_dir(project.dir.join("tests/emulator")) {
                    scripts = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
                    scripts.sort();
                }
            }
            if scripts.is_empty() {
                return Err("no scripts: put JSON scripts in tests/emulator/ or pass --script FILE (see docs/SDK-EMULATOR.md)".into());
            }
            let mut bad = 0;
            for s in scripts {
                let name = s.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let text = std::fs::read_to_string(&s).map_err(|e| format!("{}: {e}", s.display()))?;
                let rep = script::run(&project, &name, &text, &golden, update, &scratch);
                w(out, format!("{} {name}: {} step(s)", if rep.passed() { "ok  " } else { "FAIL" }, rep.steps));
                for l in &rep.shots {
                    w(out, format!("     picture {l}"));
                }
                for f in &rep.failures {
                    w(out, format!("     {f}"));
                }
                bad += usize::from(!rep.passed());
            }
            Ok(i32::from(bad > 0))
        }
    }
}
