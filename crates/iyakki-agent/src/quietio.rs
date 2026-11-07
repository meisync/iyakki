//! Output that can never panic. `println!` and `eprintln!` panic when the write fails ("failed printing to stdout"): a console routed to a serial port
//! that does not exist, a closed pipe or a job whose reader went away all do that, and on the shell or PID 1 path a panic stops the machine's session.
//! Everything in this crate that prints from non-test code uses [`qprintln!`] and [`qeprintln!`], which ignore a failed write (the line was only for
//! a person who is no longer listening).

use std::io::Write;

/// Write `args` and a newline to `w`; a failure is ignored.
pub fn line_to(w: &mut dyn Write, args: std::fmt::Arguments) {
    let _ = w.write_fmt(args);
    let _ = w.write_all(b"\n");
}

pub fn out_line(args: std::fmt::Arguments) {
    let mut o = std::io::stdout().lock();
    line_to(&mut o, args);
    let _ = o.flush();
}

pub fn err_line(args: std::fmt::Arguments) {
    line_to(&mut std::io::stderr().lock(), args);
}

/// `println!` that ignores a failed write.
#[macro_export]
macro_rules! qprintln {
    () => { $crate::quietio::out_line(format_args!("")) };
    ($($t:tt)*) => { $crate::quietio::out_line(format_args!($($t)*)) };
}

/// `eprintln!` that ignores a failed write.
#[macro_export]
macro_rules! qeprintln {
    () => { $crate::quietio::err_line(format_args!("")) };
    ($($t:tt)*) => { $crate::quietio::err_line(format_args!($($t)*)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("the console is gone"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("the console is gone"))
        }
    }

    #[test]
    fn a_failed_write_is_ignored_where_println_would_panic() {
        line_to(&mut Broken, format_args!("lost {}", 1));
        let r = std::panic::catch_unwind(|| {
            let mut b = Broken;
            writeln!(b, "x").unwrap();
        });
        assert!(r.is_err(), "the plain writeln + unwrap path panics, which is what the helper avoids");
    }

    #[test]
    fn the_line_and_its_newline_are_written() {
        let mut v = Vec::new();
        line_to(&mut v, format_args!("hello {}", "there"));
        assert_eq!(v, b"hello there\n");
        out_line(format_args!("(test output)"));
        err_line(format_args!("(test output)"));
    }

    fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            if p.is_dir() {
                if name != "tests" {
                    sources(&p, out);
                }
            } else if name.ends_with(".rs") && !name.ends_with("_tests.rs") && name != "tests.rs" {
                out.push(p);
            }
        }
    }

    #[test]
    fn no_code_of_the_agent_prints_with_the_panicking_macros() {
        let mut files = Vec::new();
        sources(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        let mut bad = Vec::new();
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            let code = text.split("#[cfg(test)]").next().unwrap_or("");
            for (n, l) in code.lines().enumerate() {
                let t = l.trim_start();
                let raw = |m: &str| l.match_indices(m).any(|(i, _)| !l[..i].ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == ':'));
                if !t.starts_with("//") && (raw("println!(") || raw("eprintln!(")) {
                    bad.push(format!("{}:{}", f.display(), n + 1));
                }
            }
        }
        assert!(bad.is_empty(), "use qprintln!/qeprintln! (they ignore a failed write): {bad:?}");
    }
}
